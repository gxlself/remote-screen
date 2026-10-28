//! Shared host services and commands for admitted connections.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use async_trait::async_trait;
use rsp_core::{errors::ErrorCode, pb};
use tokio::sync::{mpsc, oneshot};
use tracing::warn;

pub use super::roster::ConnKey;
use super::{approval, handlers};
use crate::{
    audit::AuditQueue,
    auth,
    clock::{Clock, SystemClock},
    config::{ConfigStore, HostConfig},
    ipc,
    platform::Platform,
};

#[derive(Clone)]
pub struct HostServices {
    pub events: Arc<dyn approval::PromptEvents>,
    pub config: ConfigStore,
    pub audit: Option<AuditQueue>,
    pub downloads_dir: PathBuf,
    pub state_dir: Option<PathBuf>,
    pub clock: Arc<dyn Clock>,
}

pub struct NoEvents;
impl approval::PromptEvents for NoEvents {
    fn emit(&self, _event: ipc::Event) -> bool {
        false
    }
}
impl Default for HostServices {
    fn default() -> Self {
        Self {
            events: Arc::new(NoEvents),
            config: ConfigStore::new(
                crate::default_state_dir().join("config.toml"),
                HostConfig::default(),
            ),
            audit: None,
            downloads_dir: std::env::var_os("HOME")
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from("."))
                .join("Downloads"),
            state_dir: None,
            clock: Arc::new(SystemClock),
        }
    }
}

#[derive(Clone, Debug)]
pub struct ConnInfo {
    pub key: ConnKey,
    pub session_id: String,
    pub device_id: String,
    pub device_fp: [u8; 32],
    pub device_name: String,
    pub platform: pb::Platform,
    pub user: String,
    pub node: String,
    pub path: pb::PathKind,
    pub derp_region: String,
    pub intent: pb::Intent,
    pub client_features: u64,
    pub role: auth::Role,
    pub can_control: bool,
    pub since_us: u64,
    pub resumed: bool,
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EndReason {
    Normal,
    Peer(i32),
    PeerClosed,
    LocalUser,
    Idle,
    Replaced,
    Revoked,
    Background,
    HostShutdown,
    HostUpdating,
    PermissionLost,
    ConsoleLeft,
    AuthDenied,
    Protocol,
    ProtocolImmediate,
}
impl EndReason {
    pub fn goodbye(self) -> Option<pb::goodbye::Reason> {
        use pb::goodbye::Reason as R;
        match self {
            Self::Normal
            | Self::Peer(_)
            | Self::PeerClosed
            | Self::AuthDenied
            | Self::Protocol
            | Self::ProtocolImmediate => None,
            Self::LocalUser => Some(R::HostClosedByLocalUser),
            Self::Idle => Some(R::IdleTimeout),
            Self::Replaced => Some(R::Replaced),
            Self::Revoked => Some(R::Revoked),
            Self::Background => Some(R::Background),
            Self::HostShutdown => Some(R::HostShutdown),
            Self::HostUpdating => Some(R::HostUpdating),
            Self::PermissionLost => Some(R::PermissionLost),
            Self::ConsoleLeft => Some(R::Unspecified),
        }
    }
    pub fn close_code(self) -> u32 {
        match self {
            Self::Protocol | Self::ProtocolImmediate => 1,
            Self::AuthDenied => 2,
            Self::Replaced => 3,
            Self::HostShutdown | Self::HostUpdating => 4,
            Self::Idle => 5,
            Self::Revoked => 6,
            _ => 0,
        }
    }
    pub fn lingers(self) -> bool {
        matches!(self, Self::PeerClosed | Self::Background)
            || self == Self::Peer(pb::goodbye::Reason::Background as i32)
    }
    pub fn waits_for_drain(self) -> bool {
        !matches!(self, Self::ProtocolImmediate)
    }
}

// The public command API carries envelopes inline; queues are bounded to 64 entries.
#[allow(clippy::large_enum_variant)]
pub enum ConnCommand {
    Send(pb::Envelope),
    End {
        reason: EndReason,
        done: Option<oneshot::Sender<()>>,
    },
    SetRole(auth::Role),
    Feature(handlers::FeatureEvent),
    Tick,
}
#[derive(Debug)]
pub struct Admission {
    pub session_id: String,
    pub role: auth::Role,
    pub token: [u8; 16],
    pub replaced: Vec<ConnKey>,
    pub resumed: bool,
}
pub enum ResumeOutcome {
    Fresh,
    Resumed(Admission),
    Deny(ErrorCode),
}

#[async_trait]
pub trait SessionControl: Send + Sync {
    fn sessions_list(&self) -> Vec<ipc::SessionSummary>;
    fn disconnect(&self, session_id: &str) -> bool;
    fn set_view_only(&self, session_id: &str) -> bool;
    fn approval_respond(
        &self,
        request_id: &str,
        allow: bool,
        permission: Option<ipc::Permission>,
    ) -> bool;
    fn approval_pending(&self) -> Vec<serde_json::Value>;
}

pub struct SessionManager<P: Platform> {
    platform: Arc<P>,
    services: HostServices,
    features: handlers::HostFeatures,
    conns: Mutex<HashMap<ConnKey, (ConnInfo, mpsc::Sender<ConnCommand>)>>,
    next_key: AtomicU64,
    started: AtomicBool,
    updater: OnceLock<Arc<crate::update::agent::UpdateService>>,
    owner_login: Mutex<Option<String>>,
    owner_warning_at: Mutex<Option<u64>>,
    // Serialize M0 replacement across listeners, without holding the registry lock.
    pub(crate) admission_lock: tokio::sync::Mutex<()>,
}
impl<P: Platform> SessionManager<P> {
    pub fn new(platform: Arc<P>, services: HostServices) -> Arc<Self> {
        Arc::new(Self {
            platform,
            features: handlers::HostFeatures::new(&services),
            services,
            conns: Mutex::new(HashMap::new()),
            next_key: AtomicU64::new(1),
            started: AtomicBool::new(false),
            updater: OnceLock::new(),
            owner_login: Mutex::new(None),
            owner_warning_at: Mutex::new(None),
            admission_lock: tokio::sync::Mutex::new(()),
        })
    }

    pub fn start(self: &Arc<Self>) {
        if !self.started.swap(true, Ordering::AcqRel) {
            handlers::on_host_start(self);
        }
    }
    /// Read the stable Host ID from the certificate initialized by the runtime.
    pub fn host_id(&self) -> Result<String, crate::identity::IdentityError> {
        let cert = self
            .platform
            .keystore()
            .load("host-cert-v1")?
            .ok_or(crate::identity::IdentityError::InvalidCertificate)?;
        let fp = rsp_transport::identity::spki_fingerprint(&cert)?;
        Ok(rsp_core::ids::id_from_fp(&fp))
    }

    pub fn platform(&self) -> &Arc<P> {
        &self.platform
    }
    pub fn services(&self) -> &HostServices {
        &self.services
    }
    pub fn features(&self) -> &handlers::HostFeatures {
        &self.features
    }
    pub fn now_us(&self) -> u64 {
        self.services.clock.now_micros()
    }
    pub fn next_key(&self) -> ConnKey {
        ConnKey(self.next_key.fetch_add(1, Ordering::Relaxed))
    }
    pub fn register(&self, info: ConnInfo, tx: mpsc::Sender<ConnCommand>) {
        self.conns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(info.key, (info, tx));
    }
    pub fn unregister(&self, key: ConnKey) -> Option<ConnInfo> {
        self.conns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&key)
            .map(|(info, _)| info)
    }
    pub fn info(&self, key: ConnKey) -> Option<ConnInfo> {
        self.conns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
            .map(|(info, _)| info.clone())
    }
    pub fn conns(&self) -> Vec<ConnInfo> {
        let mut infos: Vec<_> = self
            .conns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .values()
            .map(|(info, _)| info.clone())
            .collect();
        infos.sort_by_key(|info| (info.since_us, info.key.0));
        infos
    }
    pub fn update(&self, key: ConnKey, f: impl FnOnce(&mut ConnInfo)) -> bool {
        let mut conns = self.conns.lock().unwrap_or_else(|p| p.into_inner());
        if let Some((info, _)) = conns.get_mut(&key) {
            f(info);
            true
        } else {
            false
        }
    }
    fn sender(&self, key: ConnKey) -> Option<mpsc::Sender<ConnCommand>> {
        self.conns
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(&key)
            .map(|(_, tx)| tx.clone())
    }
    fn command(&self, key: ConnKey, cmd: ConnCommand) -> bool {
        let Some(tx) = self.sender(key) else {
            return false;
        };
        match tx.try_send(cmd) {
            Ok(()) => true,
            Err(error) => {
                warn!(key = key.0, %error, "connection command dropped");
                false
            }
        }
    }
    pub fn send(&self, key: ConnKey, env: impl Into<pb::Envelope>) -> bool {
        self.command(key, ConnCommand::Send(env.into()))
    }
    pub fn broadcast(&self, env: pb::Envelope) -> usize {
        self.conns()
            .iter()
            .filter(|info| self.send(info.key, env.clone()))
            .count()
    }
    pub fn post(&self, key: ConnKey, ev: handlers::FeatureEvent) -> bool {
        self.command(key, ConnCommand::Feature(ev))
    }
    pub fn post_all(&self, ev: impl Fn(&ConnInfo) -> Option<handlers::FeatureEvent>) -> usize {
        self.conns()
            .iter()
            .filter(|info| ev(info).is_some_and(|ev| self.post(info.key, ev)))
            .count()
    }
    pub fn set_role(&self, key: ConnKey, role: auth::Role) -> bool {
        self.command(key, ConnCommand::SetRole(role))
    }
    pub fn tick_all(&self) -> usize {
        self.conns()
            .iter()
            .filter(|info| self.command(info.key, ConnCommand::Tick))
            .count()
    }
    pub fn end_nowait(&self, key: ConnKey, reason: EndReason) -> bool {
        self.command(key, ConnCommand::End { reason, done: None })
    }
    pub async fn end(&self, key: ConnKey, reason: EndReason) -> bool {
        let Some(tx) = self.sender(key) else {
            return false;
        };
        let (done, rx) = oneshot::channel();
        matches!(
            tokio::time::timeout(Duration::from_secs(3), async {
                tx.send(ConnCommand::End {
                    reason,
                    done: Some(done),
                })
                .await
                .map_err(|_| ())?;
                rx.await.map_err(|_| ())
            })
            .await,
            Ok(Ok(()))
        )
    }
    pub async fn end_where(&self, pred: impl Fn(&ConnInfo) -> bool, reason: EndReason) -> usize {
        let mut pending = tokio::task::JoinSet::new();
        // Enqueue together; await receipts without retaining registry locks.
        for info in self.conns().into_iter().filter(pred) {
            if let Some(tx) = self.sender(info.key) {
                pending.spawn(async move {
                    let (done, rx) = oneshot::channel();
                    matches!(
                        tokio::time::timeout(Duration::from_secs(3), async {
                            tx.send(ConnCommand::End {
                                reason,
                                done: Some(done),
                            })
                            .await
                            .map_err(|_| ())?;
                            rx.await.map_err(|_| ())
                        })
                        .await,
                        Ok(Ok(()))
                    )
                });
            }
        }
        let mut count = 0;
        while let Some(result) = pending.join_next().await {
            if matches!(result, Ok(true)) {
                count += 1;
            }
        }
        count
    }
    pub fn attach_updater(&self, svc: Arc<crate::update::agent::UpdateService>) -> bool {
        self.updater.set(svc).is_ok()
    }
    pub fn updater(&self) -> Option<Arc<crate::update::agent::UpdateService>> {
        self.updater.get().cloned()
    }
    pub fn emit(&self, event: ipc::Event) -> bool {
        self.services.events.emit(event)
    }
    pub fn set_owner_login(&self, login: Option<String>) {
        *self.owner_login.lock().unwrap_or_else(|p| p.into_inner()) = login;
    }
    pub fn owner_login(&self) -> Option<String> {
        self.owner_login
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    pub(crate) fn warn_owner_unknown(&self) {
        let now = self.now_us();
        let mut last = self
            .owner_warning_at
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        if last.is_none_or(|last| now.saturating_sub(last) >= 60_000_000) {
            *last = Some(now);
            warn!("allowed_users is empty and local Tailscale login is unknown; tailnet users will be denied");
        }
    }
    pub fn session_control(self: &Arc<Self>) -> Arc<dyn SessionControl> {
        Arc::new(Control(self.clone()))
    }
}
struct Control<P: Platform>(Arc<SessionManager<P>>);
#[async_trait]
impl<P: Platform> SessionControl for Control<P> {
    fn sessions_list(&self) -> Vec<ipc::SessionSummary> {
        handlers::roster::summaries(&self.0)
    }
    fn disconnect(&self, id: &str) -> bool {
        handlers::roster::disconnect(&self.0, id)
    }
    fn set_view_only(&self, id: &str) -> bool {
        handlers::roster::set_view_only(&self.0, id)
    }
    fn approval_respond(&self, id: &str, allow: bool, permission: Option<ipc::Permission>) -> bool {
        handlers::approval::respond(&self.0, id, allow, permission)
    }
    fn approval_pending(&self) -> Vec<serde_json::Value> {
        handlers::approval::pending(&self.0)
    }
}
