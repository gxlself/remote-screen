//! In-process Host and client fixtures for integration tests on any platform.
//!
//! Each Host owns an isolated state directory and loopback listeners on
//! ephemeral ports. Setup failures panic with context, as these helpers are
//! intended for tests. Dropping a Host aborts its runtime and removes its state.

use std::net::{Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use host_core::audit::AuditQueue;
use host_core::config::{ConfigStore, HostConfig};
use host_core::identity::HostIdentity;
use host_core::pairing::{
    PairApproval, PairApprovalRequest, PairApprover, PairingManager, PairingTicket,
};
use host_core::platform::Platform;
use host_core::session::handlers::approval;
use host_core::session::handshake::{
    ConnectionApprovalRequest, ConnectionApprover, HandshakeContext,
};
use host_core::session::manager::HostServices;
use host_core::store::{DevicePlatform, PairedDevice, PairingPath, PathPermissions, Store};
use host_core::{HostRuntime, RuntimeOptions};
use rsp_client::session::{
    EndReason, EventStream, Session, SessionError, SessionEvent, SessionHandle, SessionParams,
    SessionStateKind, SessionStats,
};
use rsp_core::clock::unix_micros;
use rsp_core::errors::ErrorCode;
use rsp_core::pb;
use rsp_core::trust::Permission;
use rsp_transport::dial::{Candidate, CandidateSource, PathKind};
use rsp_transport::identity::{CertKind, LocalIdentity};
use rsp_transport::TransportKind;
use tokio::task::JoinHandle;
use tokio::time::{timeout, Instant};

use crate::FakePlatform;

/// The immediate answer supplied by a fixture's local approver.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum Approval {
    Allow,
    Deny,
}

#[async_trait]
impl ConnectionApprover for Approval {
    async fn request(&self, _: ConnectionApprovalRequest) -> bool {
        *self == Self::Allow
    }
}

#[async_trait]
impl PairApprover for Approval {
    async fn request(&self, _: PairApprovalRequest) -> PairApproval {
        PairApproval {
            allow: *self == Self::Allow,
            permission: Permission::Control,
        }
    }
}

/// Host policy overrides. TCP and local pairing approval default to enabled.
#[derive(Clone)]
pub struct TestHostOptions {
    pub attended: Option<Approval>,
    pub view_only: bool,
    pub allow_remote_control: Option<bool>,
    pub fail: Option<ErrorCode>,
    pub pairing_approval: Option<Approval>,
    pub pairing_clock: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
    /// Session timers, freshness checks, and audit timestamps.
    pub services_clock: Option<Arc<dyn host_core::clock::Clock>>,
    /// Publish the runtime's TCP listener address. HostListener binds both
    /// transports; its current policy has no switch to disable TCP binding.
    pub tcp: bool,
    pub downloads_dir: Option<PathBuf>,
    pub config: Option<HostConfig>,
}

impl Default for TestHostOptions {
    fn default() -> Self {
        Self {
            attended: None,
            view_only: false,
            allow_remote_control: None,
            fail: None,
            pairing_approval: None,
            pairing_clock: None,
            services_clock: None,
            tcp: true,
            downloads_dir: None,
            config: None,
        }
    }
}

struct StateDirectory(PathBuf);

impl StateDirectory {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        loop {
            let path = std::env::temp_dir().join(format!(
                "host-fake-e2e-{}-{}-{}",
                std::process::id(),
                unix_micros(),
                NEXT.fetch_add(1, Ordering::Relaxed),
            ));
            match std::fs::create_dir(&path) {
                Ok(()) => return Self(path),
                Err(error) if error.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(error) => panic!("create Host fixture directory {}: {error}", path.display()),
            }
        }
    }
}

impl Drop for StateDirectory {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

/// A running, isolated Host. Keep this value alive for the entire client test.
pub struct TestHost {
    pub quic_addr: SocketAddr,
    pub tcp_addr: Option<SocketAddr>,
    pub fp: [u8; 32],
    pub host_id: String,
    pub state_dir: PathBuf,
    pub security: Arc<HandshakeContext>,
    task: Option<JoinHandle<host_core::RuntimeResult<()>>>,
    pairing_clock: Arc<dyn Fn() -> u64 + Send + Sync>,
    on_paired: Box<dyn Fn(&str) + Send + Sync>,
    _directory: StateDirectory,
}

impl TestHost {
    pub async fn start() -> Self {
        Self::start_with(TestHostOptions::default()).await
    }

    pub async fn start_with(opts: TestHostOptions) -> Self {
        Self::start_with_platform(FakePlatform::new(), opts).await
    }

    pub async fn start_with_platform<P: Platform + 'static>(
        platform: P,
        opts: TestHostOptions,
    ) -> Self {
        let directory = StateDirectory::new();
        let state_dir = directory.0.clone();
        let store = Arc::new(
            Store::open(&state_dir)
                .unwrap_or_else(|error| panic!("open Host fixture store: {error}")),
        );
        let audit = AuditQueue::open(&state_dir)
            .unwrap_or_else(|error| panic!("open Host fixture audit queue: {error}"));
        let bind = SocketAddr::from((Ipv4Addr::LOCALHOST, 0));
        let mut options = RuntimeOptions {
            bind: Some(bind),
            store_dir: state_dir.clone(),
            view_only: opts.view_only,
            ..RuntimeOptions::default()
        };
        options.auth_policy.attended_mode = opts.attended.is_some();
        options.auth_policy.allow_remote_control = opts.allow_remote_control.unwrap_or(true);
        options.auth_policy.forced_denial = opts.fail;
        let pairing_clock = opts
            .pairing_clock
            .clone()
            .unwrap_or_else(|| Arc::new(unix_micros));
        let pairing = opts.pairing_clock.map(|clock| {
            let identity = HostIdentity::load_or_create(platform.keystore(), unix_micros())
                .unwrap_or_else(|error| panic!("load Host fixture identity: {error}"));
            // The ephemeral listener addresses are only known after bind().
            // new_pairing_code() adds the bound QUIC address to the QR ticket.
            PairingManager::new(
                store.clone(),
                identity.fp(),
                platform.info().name,
                vec![bind],
            )
            .with_clock(clock)
        });
        let mut runtime = HostRuntime::new(platform, options)
            .with_store(store)
            .with_audit(audit)
            .with_connection_approver(Arc::new(opts.attended.unwrap_or(Approval::Allow)))
            .with_pairing_approver(Arc::new(opts.pairing_approval.unwrap_or(Approval::Allow)));
        if opts.downloads_dir.is_some() || opts.config.is_some() || opts.services_clock.is_some() {
            let mut services = HostServices::default();
            if let Some(downloads_dir) = opts.downloads_dir {
                services.downloads_dir = downloads_dir;
            }
            if let Some(config) = opts.config {
                services.config = ConfigStore::new(state_dir.join("config.toml"), config);
            }
            if let Some(clock) = opts.services_clock {
                services.clock = clock;
            }
            runtime = runtime.with_services(services);
        }
        if let Some(pairing) = pairing {
            runtime = runtime.with_pairing_manager(pairing);
        }
        let host = runtime
            .bind()
            .await
            .unwrap_or_else(|error| panic!("bind Host fixture: {error}"));
        let addresses = host.listener.bound();
        let quic_addr = addresses
            .iter()
            .find_map(|(addr, _, kind)| (*kind == TransportKind::Quic).then_some(*addr))
            .unwrap_or_else(|| panic!("Host fixture has no QUIC listener"));
        let tcp_addr = opts.tcp.then(|| {
            addresses
                .iter()
                .find_map(|(addr, _, kind)| (*kind == TransportKind::Tcp).then_some(*addr))
                .unwrap_or_else(|| panic!("Host fixture has no TCP listener"))
        });
        let security = host.security.clone();
        // Keep TestHost platform-independent while using the running Host's
        // actual manager for the same approval hook as local pairing.
        let mgr = host.manager.clone();
        let on_paired = Box::new(move |device_id: &str| approval::on_paired(&mgr, device_id));
        Self {
            quic_addr,
            tcp_addr,
            fp: security.identity.fp(),
            host_id: security.identity.host_id(),
            state_dir,
            security,
            task: Some(tokio::spawn(host.run())),
            pairing_clock,
            on_paired,
            _directory: directory,
        }
    }

    /// Pre-pair a certificate as if it was approved locally, marking it seen.
    pub fn pair(&self, device: &LocalIdentity, permission: Permission) {
        self.pair_unseen(device, permission);
        (self.on_paired)(&rsp_core::ids::id_from_fp(&device.fingerprint()));
    }

    /// Add a pairing without local approval, as for a device approved elsewhere.
    /// A fresh device remains subject to the new-device confirmation policy.
    pub fn pair_unseen(&self, device: &LocalIdentity, permission: Permission) {
        self.security
            .store
            .upsert(PairedDevice {
                fp: device.fingerprint(),
                device_id: rsp_core::ids::id_from_fp(&device.fingerprint()),
                name: "e2e".into(),
                cert_der: device.cert_der.as_ref().to_vec(),
                permission,
                paired_at_us: unix_micros(),
                require_confirm: false,
                platform: DevicePlatform::Ios,
                paired_via: PairingPath::Lan,
                whois_user: None,
                whois_node: None,
                whois_node_name: None,
                path_permissions: PathPermissions::default(),
            })
            .unwrap_or_else(|error| panic!("pair Host fixture device: {error}"));
    }

    pub fn revoke(&self, fp: [u8; 32]) {
        self.security
            .store
            .revoke(fp, 1)
            .unwrap_or_else(|error| panic!("revoke Host fixture device: {error}"));
    }

    pub fn new_pairing_code(&self) -> PairingTicket {
        let mut ticket = self
            .security
            .pairing
            .start((self.pairing_clock)())
            .unwrap_or_else(|error| panic!("start Host fixture pairing: {error}"));
        let mut qr = rsp_core::qr::PairQr::parse(&ticket.qr_url)
            .unwrap_or_else(|error| panic!("parse Host fixture pairing QR: {error}"));
        qr.addrs = vec![self.quic_addr];
        ticket.qr_url = qr.to_url();
        ticket
    }

    /// Request immediate cancellation, without waiting for task cleanup.
    pub fn abort(&self) {
        if let Some(task) = &self.task {
            task.abort();
        }
    }

    /// Cancel the runtime and wait for its future to be dropped.
    pub async fn stop(mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
            let _ = task.await;
        }
    }
}

impl Drop for TestHost {
    fn drop(&mut self) {
        self.abort();
    }
}

/// Generate a fresh software App certificate for each call.
pub fn app_identity() -> Arc<LocalIdentity> {
    Arc::new(
        LocalIdentity::generate_software(CertKind::App)
            .unwrap_or_else(|error| panic!("generate App fixture identity: {error}")),
    )
}

pub fn params(host: &TestHost, id: Arc<LocalIdentity>, addrs: &[SocketAddr]) -> SessionParams {
    let candidates = addrs
        .iter()
        .map(|addr| Candidate {
            addr: *addr,
            kind: PathKind::Manual,
            source: CandidateSource::Manual,
        })
        .collect();
    let mut params = SessionParams::from_shared(candidates, host.fp, id);
    params.device_name = "e2e".into();
    params.platform = pb::Platform::Ios;
    params
}

pub fn direct_params(host: &TestHost, id: Arc<LocalIdentity>) -> SessionParams {
    params(host, id, &[host.quic_addr])
}

pub async fn open(p: SessionParams) -> Result<(SessionHandle, EventStream), SessionError> {
    Session::open(p).await
}

#[derive(Debug, Default)]
pub struct FrameReport {
    pub frames: u64,
    pub keyframes: u64,
    pub first_frame: Option<Duration>,
    pub max_gap: Duration,
    pub ended: Option<(Option<ErrorCode>, EndReason)>,
    pub last_stats: Option<SessionStats>,
    pub stream: Option<pb::StreamConfig>,
    pub states: Vec<SessionStateKind>,
    pub errors: Vec<(ErrorCode, String)>,
}

/// Collect up to `n` frames, stopping at the deadline or the end of the stream.
/// `first_frame` measures time from entry to this helper, excluding open().
pub async fn wait_frames(ev: &mut EventStream, n: u64, timeout: Duration) -> FrameReport {
    collect_frames(ev, Some(n), timeout).await
}

/// Collect for a fixed duration, or until the session ends.
pub async fn run_for(ev: &mut EventStream, dur: Duration) -> FrameReport {
    collect_frames(ev, None, dur).await
}

async fn collect_frames(ev: &mut EventStream, limit: Option<u64>, dur: Duration) -> FrameReport {
    let started = Instant::now();
    let mut report = FrameReport::default();
    let mut last_frame = None;
    let _ = timeout(dur, async {
        while limit.is_none_or(|n| report.frames < n) {
            let Some(event) = ev.recv().await else {
                break;
            };
            match event {
                SessionEvent::Frame(frame) => {
                    let now = started.elapsed();
                    report.frames += 1;
                    report.keyframes += u64::from(frame.frame.keyframe);
                    report.first_frame.get_or_insert(now);
                    if let Some(previous) = last_frame {
                        report.max_gap = report.max_gap.max(now - previous);
                    }
                    last_frame = Some(now);
                }
                SessionEvent::Stats(stats) => report.last_stats = Some(stats),
                SessionEvent::StreamConfig(config) => report.stream = Some(config),
                SessionEvent::StateChanged(state) => report.states.push(state),
                SessionEvent::Error { code, message } => report.errors.push((code, message)),
                SessionEvent::Ended { code, kind, .. } => {
                    report.ended = Some((code, kind));
                    break;
                }
                _ => {}
            }
        }
    })
    .await;
    report
}

pub async fn wait_state(ev: &mut EventStream, want: SessionStateKind, dur: Duration) -> bool {
    timeout(dur, async {
        while let Some(event) = ev.recv().await {
            match event {
                SessionEvent::StateChanged(state) if state == want => return true,
                SessionEvent::Ended { .. } => return false,
                _ => {}
            }
        }
        false
    })
    .await
    .unwrap_or(false)
}

pub async fn wait_end(
    ev: &mut EventStream,
    dur: Duration,
) -> Option<(Option<ErrorCode>, EndReason, String)> {
    timeout(dur, async {
        while let Some(event) = ev.recv().await {
            if let SessionEvent::Ended { code, kind, reason } = event {
                return Some((code, kind, reason));
            }
        }
        None
    })
    .await
    .ok()
    .flatten()
}

/// Run pairing through the real client and collect results for at most 5 s.
pub async fn pair_with_code(
    host: &TestHost,
    id: Arc<LocalIdentity>,
    code: &str,
) -> Vec<pb::PairResult> {
    // PairingManager retains its latest explicit timestamp. Refresh it before
    // an attempt so an injected clock can expire a previously issued ticket.
    host.security
        .pairing
        .code_state((host.pairing_clock)())
        .unwrap_or_else(|error| panic!("refresh Host fixture pairing clock: {error}"));
    let mut params = direct_params(host, id).with_pairing_code(code);
    params.intent = pb::Intent::Pair;
    let mut results = Vec::new();
    let mut session = None;
    let _ = timeout(Duration::from_secs(5), async {
        let (handle, mut events) = open(params)
            .await
            .unwrap_or_else(|error| panic!("open pairing fixture session: {error}"));
        session = Some(handle);
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::PairResult(result) => results.push(result),
                SessionEvent::Ended { .. } => break,
                _ => {}
            }
        }
    })
    .await;
    if let Some(handle) = session {
        handle.close();
    }
    results
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn frame(keyframe: bool) -> SessionEvent {
        SessionEvent::Frame(rsp_client::ReceivedFrame {
            frame: rsp_core::video::Frame {
                frame_id: 1,
                epoch: 1,
                display_id: 1,
                flags: 0,
                keyframe,
                capture_ts_us: unix_micros(),
                data: vec![],
            },
            received_us: unix_micros(),
            capture_local_us: None,
        })
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn injected_pairing_clock_controls_ticket_and_attempt_time() {
        timeout(Duration::from_secs(9), async {
            let now = Arc::new(AtomicU64::new(1_000_000));
            let clock = now.clone();
            let host = TestHost::start_with_platform(
                FakePlatform::new(),
                TestHostOptions {
                    pairing_clock: Some(Arc::new(move || clock.load(Ordering::Relaxed))),
                    ..TestHostOptions::default()
                },
            )
            .await;
            let ticket = host.new_pairing_code();
            assert_eq!(
                ticket.expires_at,
                now.load(Ordering::Relaxed) + host_core::pairing::PAIRING_CODE_LIFETIME_US,
            );
            let qr = rsp_core::qr::PairQr::parse(&ticket.qr_url).unwrap();
            assert_eq!(qr.fp, host.fp);
            assert_eq!(qr.addrs, vec![host.quic_addr]);
            let results = pair_with_code(&host, app_identity(), &ticket.code).await;
            assert_eq!(
                results.last().unwrap().status,
                pb::pair_result::Status::Ok as i32
            );

            let ticket = host.new_pairing_code();
            now.store(ticket.expires_at, Ordering::Relaxed);
            let results = pair_with_code(&host, app_identity(), &ticket.code).await;
            assert_eq!(
                results.last().unwrap().status,
                pb::pair_result::Status::Expired as i32
            );
            host.stop().await;
        })
        .await
        .expect("injected-clock fixture check completes within 9 seconds");
    }

    #[tokio::test]
    async fn frame_limit_leaves_later_events_for_the_next_consumer() {
        let (sender, mut events) = mpsc::unbounded_channel();
        sender.send(SessionEvent::LocalUse(false)).unwrap();
        sender.send(frame(true)).unwrap();
        sender.send(frame(false)).unwrap();

        let first = wait_frames(&mut events, 1, Duration::from_secs(1)).await;
        assert_eq!(first.frames, 1);
        assert_eq!(first.keyframes, 1);
        assert!(first.first_frame.is_some());
        assert_eq!(first.max_gap, Duration::ZERO);
        let second = wait_frames(&mut events, 1, Duration::from_secs(1)).await;
        assert_eq!(second.frames, 1);
        assert_eq!(second.keyframes, 0);
        let empty = wait_frames(&mut events, 0, Duration::from_secs(1)).await;
        assert_eq!(empty.frames, 0);
        assert!(empty.first_frame.is_none());
    }

    #[tokio::test]
    async fn deadline_preserves_partial_frames_and_diagnostics() {
        let (sender, mut events) = mpsc::unbounded_channel();
        let config = pb::StreamConfig {
            epoch: 3,
            ..Default::default()
        };
        let stats = SessionStats {
            frames_received: 7,
            ..Default::default()
        };
        sender
            .send(SessionEvent::StateChanged(SessionStateKind::Streaming))
            .unwrap();
        sender.send(SessionEvent::StreamConfig(config)).unwrap();
        sender.send(SessionEvent::Stats(stats.clone())).unwrap();
        sender
            .send(SessionEvent::Error {
                code: ErrorCode::Other,
                message: "fixture diagnostic".into(),
            })
            .unwrap();
        sender.send(frame(true)).unwrap();

        // Keep the sender alive: returning requires the actual deadline.
        let report = wait_frames(&mut events, 2, Duration::from_millis(10)).await;
        assert_eq!(report.frames, 1);
        assert_eq!(report.states, vec![SessionStateKind::Streaming]);
        assert_eq!(report.stream, Some(config));
        assert_eq!(report.last_stats, Some(stats));
        assert_eq!(
            report.errors,
            vec![(ErrorCode::Other, "fixture diagnostic".into())]
        );
        assert!(report.ended.is_none());
        let started = Instant::now();
        let report = run_for(&mut events, Duration::from_millis(10)).await;
        assert!(started.elapsed() >= Duration::from_millis(10));
        assert_eq!(report.frames, 0);
        drop(sender);
    }

    #[tokio::test]
    async fn waiters_preserve_end_details_and_stop_collecting_at_end() {
        let (sender, mut events) = mpsc::unbounded_channel();
        sender.send(SessionEvent::LocalUse(false)).unwrap();
        sender
            .send(SessionEvent::StateChanged(SessionStateKind::Streaming))
            .unwrap();
        assert!(
            wait_state(
                &mut events,
                SessionStateKind::Streaming,
                Duration::from_secs(1)
            )
            .await
        );

        let reason = EndReason::Error(ErrorCode::NotPaired);
        sender
            .send(SessionEvent::Ended {
                code: Some(ErrorCode::NotPaired),
                reason: "unpaired fixture".into(),
                kind: reason,
            })
            .unwrap();
        assert_eq!(
            wait_end(&mut events, Duration::from_secs(1)).await,
            Some((
                Some(ErrorCode::NotPaired),
                reason,
                "unpaired fixture".into()
            ))
        );

        sender
            .send(SessionEvent::Ended {
                code: None,
                reason: "normal end".into(),
                kind: EndReason::Goodbye(pb::goodbye::Reason::UserClosed),
            })
            .unwrap();
        sender.send(frame(true)).unwrap();
        let report = run_for(&mut events, Duration::from_secs(1)).await;
        assert_eq!(report.frames, 0);
        assert_eq!(
            report.ended,
            Some((None, EndReason::Goodbye(pb::goodbye::Reason::UserClosed)))
        );
        assert_eq!(
            wait_frames(&mut events, 1, Duration::from_secs(1))
                .await
                .frames,
            1
        );

        assert!(
            !wait_state(
                &mut events,
                SessionStateKind::Ended,
                Duration::from_millis(10)
            )
            .await
        );
        assert_eq!(wait_end(&mut events, Duration::from_millis(10)).await, None);
        drop(sender);
        assert!(!wait_state(&mut events, SessionStateKind::Ended, Duration::from_secs(1)).await);
    }
}
