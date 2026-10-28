//! Host-wide local prompts and the history of admitted devices.
use super::ConnCx;
use crate::{
    auth, ipc,
    pairing::{PairApproval, PairApprovalRequest, PairApprover},
    platform::Platform,
    session::{
        approval::{
            path_name, reason_name, LocalPrompts, PromptKind, PromptOutcome, PromptRequest,
            APPROVAL_TIMEOUT,
        },
        handshake::{ConnectionApprovalOutcome, ConnectionApprovalRequest, ConnectionApprover},
        manager::{EndReason, HostServices, SessionManager},
    },
};
use async_trait::async_trait;
use rsp_core::ids::id_from_fp;
use serde::{Deserialize, Serialize};
use std::{
    collections::VecDeque,
    fs,
    path::Path,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::watch;
use tracing::warn;

const MAX_SEEN: usize = 1024;
const STATE_FILE: &str = "approval-state.json";

/// A device approved by a verified trust-log statement.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ApprovedDevice {
    pub fp: [u8; 32],
    pub device_id: String,
    pub name: String,
    pub signer_fp: [u8; 32],
    pub seq: u64,
}

#[derive(Serialize, Deserialize)]
struct NewDeviceState {
    since_us: u64,
    seen: VecDeque<String>,
}

impl NewDeviceState {
    fn load(s: &HostServices) -> Self {
        if let Some(dir) = &s.state_dir {
            match fs::read(dir.join(STATE_FILE)) {
                Ok(bytes) => match serde_json::from_slice(&bytes) {
                    Ok(state) => return state,
                    Err(error) => warn!(%error, "could not parse approval state"),
                },
                Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
                Err(error) => warn!(%error, "could not read approval state"),
            }
        }
        let state = Self {
            since_us: s.clock.now_micros(),
            seen: VecDeque::new(),
        };
        state.save(s.state_dir.as_deref());
        state
    }

    fn save(&self, dir: Option<&Path>) {
        let Some(dir) = dir else { return };
        // All writes are serialized by Host::seen. Replace the complete file
        // so interruption cannot leave a partially written device history.
        let result = (|| -> std::io::Result<()> {
            let bytes = serde_json::to_vec(self).map_err(std::io::Error::other)?;
            fs::create_dir_all(dir)?;
            let temporary = dir.join("approval-state.json.tmp");
            fs::write(&temporary, bytes)?;
            fs::rename(temporary, dir.join(STATE_FILE))?;
            Ok(())
        })();
        if let Err(error) = result {
            warn!(%error, "could not persist approval state; keeping it in memory");
        }
    }
}

pub struct Host {
    pub prompts: LocalPrompts,
    pending_connections: Arc<AtomicUsize>,
    seen: Mutex<NewDeviceState>,
}
impl Host {
    fn has_seen(&self, device_id: &str) -> bool {
        match self.seen.lock() {
            Ok(state) => state.seen.iter().any(|id| id == device_id),
            Err(_) => {
                warn!("approval device history lock was poisoned");
                false
            }
        }
    }

    pub fn new(s: &HostServices) -> Self {
        Self {
            prompts: LocalPrompts::new(s.events.clone()),
            pending_connections: Arc::new(AtomicUsize::new(0)),
            seen: Mutex::new(NewDeviceState::load(s)),
        }
    }

    fn remember(&self, device_id: &str, state_dir: Option<&Path>) {
        if device_id.starts_with("anon-") {
            return;
        }
        let mut state = match self.seen.lock() {
            Ok(state) => state,
            Err(_) => {
                warn!("approval device history lock was poisoned");
                return;
            }
        };
        if state.seen.iter().any(|id| id == device_id) {
            return;
        }
        state.seen.push_back(device_id.to_owned());
        while state.seen.len() > MAX_SEEN {
            state.seen.pop_front();
        }
        state.save(state_dir);
    }
}
#[derive(Default)]
pub struct Conn {}
#[derive(Debug)]
pub enum Event {}
pub const FEATURES: u64 = 0;
pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}
pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    on_paired(cx.mgr, &cx.info.device_id);
}

/// A successful local pairing already includes a host-side approval, even
/// when an embedding application supplies its own PairApprover.
pub fn on_paired<P: Platform>(mgr: &Arc<SessionManager<P>>, device_id: &str) {
    mgr.features()
        .approval
        .remember(device_id, mgr.services().state_dir.as_deref());
}

pub fn on_trust_approved<P: Platform>(mgr: &Arc<SessionManager<P>>, a: &ApprovedDevice) {
    if mgr.features().approval.has_seen(&id_from_fp(&a.signer_fp)) {
        mgr.features()
            .approval
            .remember(&a.device_id, mgr.services().state_dir.as_deref());
    }
    let host_id = match mgr.host_id() {
        Ok(host_id) => host_id,
        Err(error) => {
            warn!(%error, "could not read Host identity for pairing notification");
            return;
        }
    };
    mgr.emit(ipc::Event::PairingResult(serde_json::json!({
        "status": 1,
        "host_id": host_id,
        "host_name": mgr.platform().info().name,
        "attempts_left": 0,
        "device_fp": rsp_core::ids::fp_b64url(&a.fp),
        "method": "approve",
        "device_name": a.name,
    })));
}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}
pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn on_closed<P: Platform>(_cx: &mut ConnCx<'_, P>, _reason: EndReason) {}

struct PromptApprover(LocalPrompts);

#[async_trait]
impl ConnectionApprover for PromptApprover {
    async fn request(&self, req: ConnectionApprovalRequest) -> bool {
        self.request_until(req, tokio::time::Instant::now() + APPROVAL_TIMEOUT)
            .await
            == ConnectionApprovalOutcome::Allowed
    }

    async fn request_until(
        &self,
        req: ConnectionApprovalRequest,
        deadline: tokio::time::Instant,
    ) -> ConnectionApprovalOutcome {
        let outcome = self
            .0
            .ask_until(
                PromptRequest {
                    kind: PromptKind::Connection,
                    device_name: req.device_name,
                    device_id: req.device_id,
                    platform: req.platform,
                    whois_login: (!req.peer.user.is_empty()).then_some(req.peer.user),
                    path: path_name(req.path).map(str::to_owned),
                    reasons: req
                        .reasons
                        .iter()
                        .map(|r| reason_name(*r).to_owned())
                        .collect(),
                    device_fp: Some(req.device_fp),
                    intent: req.intent,
                },
                deadline,
            )
            .await;
        match outcome {
            PromptOutcome::TimedOut => ConnectionApprovalOutcome::TimedOut,
            outcome if outcome.allowed() => ConnectionApprovalOutcome::Allowed,
            _ => ConnectionApprovalOutcome::Rejected,
        }
    }
}

struct PairPromptApprover<P: Platform> {
    prompts: LocalPrompts,
    mgr: Arc<SessionManager<P>>,
}

#[async_trait]
impl<P: Platform> PairApprover for PairPromptApprover<P> {
    async fn request(&self, req: PairApprovalRequest) -> PairApproval {
        let device_id = id_from_fp(&req.device_fp);
        let result = self.prompts.request(req).await;
        if result.allow {
            on_paired(&self.mgr, &device_id);
        }
        result
    }
}

pub fn connection_approver<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
) -> Option<Arc<dyn ConnectionApprover>> {
    Some(Arc::new(PromptApprover(
        mgr.features().approval.prompts.clone(),
    )))
}
pub fn pair_approver<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Option<Arc<dyn PairApprover>> {
    Some(Arc::new(PairPromptApprover {
        prompts: mgr.features().approval.prompts.clone(),
        mgr: mgr.clone(),
    }))
}
pub fn respond<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    request_id: &str,
    allow: bool,
    permission: Option<ipc::Permission>,
) -> bool {
    mgr.features()
        .approval
        .prompts
        .respond(request_id, allow, permission)
}
pub fn prompt_watch<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Option<watch::Receiver<bool>> {
    Some(mgr.features().approval.prompts.subscribe_gate())
}
pub fn prompts<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Option<LocalPrompts> {
    Some(mgr.features().approval.prompts.clone())
}
pub fn pending<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Vec<serde_json::Value> {
    mgr.features().approval.prompts.pending_requests()
}

/// Counts one connection waiting for local approval so updates stay blocked.
pub struct PendingConnection(Arc<AtomicUsize>);
impl Drop for PendingConnection {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

/// This count belongs to the update gate and does not affect approval itself.
pub fn pending_connection<P: Platform>(mgr: &Arc<SessionManager<P>>) -> PendingConnection {
    let count = mgr.features().approval.pending_connections.clone();
    count.fetch_add(1, Ordering::AcqRel);
    PendingConnection(count)
}

pub fn pending_connections<P: Platform>(mgr: &Arc<SessionManager<P>>) -> usize {
    mgr.features()
        .approval
        .pending_connections
        .load(Ordering::Acquire)
}

pub fn confirm_new_device<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    device_id: &str,
    paired_at_us: Option<u64>,
) -> bool {
    if !mgr.services().config.get().security.confirm_new_devices {
        return false;
    }
    let state = match mgr.features().approval.seen.lock() {
        Ok(state) => state,
        Err(poisoned) => {
            warn!("approval device history lock was poisoned");
            poisoned.into_inner()
        }
    };
    paired_at_us.is_some_and(|at| at >= state.since_us)
        && !state.seen.iter().any(|id| id == device_id)
}

#[cfg(test)]
mod tests {
    use super::*;

    struct FixedClock;
    impl crate::clock::Clock for FixedClock {
        fn now_micros(&self) -> u64 {
            123
        }
    }

    #[test]
    fn memory_history_is_bounded_unique_and_excludes_anonymous_devices() {
        let services = HostServices {
            clock: Arc::new(FixedClock),
            ..HostServices::default()
        };
        let host = Host::new(&services);
        assert_eq!(host.seen.lock().unwrap().since_us, 123);
        host.remember("anon-connection", None);
        assert!(host.seen.lock().unwrap().seen.is_empty());
        for n in 0..=MAX_SEEN {
            host.remember(&format!("d-{n}"), None);
        }
        host.remember("d-2", None);
        let state = host.seen.lock().unwrap();
        assert_eq!(state.seen.len(), MAX_SEEN);
        assert_eq!(state.seen.front().unwrap(), "d-1");
        assert_eq!(state.seen.back().unwrap(), "d-1024");
    }

    #[test]
    fn failed_state_writes_keep_the_memory_history() {
        let directory = std::env::temp_dir().join(format!(
            "rs-approval-write-{}-{}",
            std::process::id(),
            rsp_core::clock::unix_micros()
        ));
        // An existing directory where the state file belongs makes both the
        // initial read and replacement fail, without relying on Unix modes.
        fs::create_dir_all(directory.join(STATE_FILE)).unwrap();
        let services = HostServices {
            state_dir: Some(directory.clone()),
            clock: Arc::new(FixedClock),
            ..HostServices::default()
        };
        let host = Host::new(&services);
        host.remember("d-approved", Some(&directory));
        let state = host.seen.lock().unwrap();
        assert_eq!(state.since_us, 123);
        assert_eq!(state.seen, VecDeque::from(["d-approved".to_owned()]));
        fs::remove_dir_all(directory).unwrap();
    }
}
