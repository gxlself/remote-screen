#[path = "update_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::Duration;

use host_core::auth;
use host_core::clock::Clock;
use host_core::config::{ConfigStore, HostConfig};
use host_core::platform::{
    CaptureConfig, CaptureHandle, ClipboardItem, ConsoleState, CursorUpdate, DisplayInfo, Encoder,
    EncoderConfig, FrameSink, HostInfo, InputEvent, KeyStore, Nic, PermissionKind, Permissions,
    Platform, PlatformError, Result as PlatformResult,
};
use host_core::session::handlers::roster;
use host_core::session::manager::{
    ConnCommand, ConnInfo, ConnKey, HostServices, NoEvents, ResumeOutcome, SessionManager,
};
use host_core::session::roster::LINGER_US;
use host_core::update::agent::MgrGate;
use host_core::update::install::{
    InstallState, PendingInstall, SessionGate, UpdateOutcome, Updater, UpdaterOptions, UpdaterParts,
};
use host_core::update::prepare::{PrepareContext, Trigger};
use host_core::update::status::Phase;
use host_core::update::types::{Arch, HostOs};
use rsp_core::pb;
use serde_json::json;
use support::{
    signed_manifest, trusted, FakeDownloader, FakeInstaller, FakeSource, FakeVerifier, TestClock,
    TestDir,
};
use tokio::sync::{mpsc, watch};

const SEED: [u8; 32] = [7; 32];
const PACKAGE: &[u8] = b"signed update package contents";
const CURRENT: &str = "0.3.1";
const NEXT: &str = "0.3.2";

#[derive(Default)]
struct TestKeyStore;

impl KeyStore for TestKeyStore {
    fn load(&self, _name: &str) -> PlatformResult<Option<Vec<u8>>> {
        Ok(None)
    }

    fn store(&self, _name: &str, _value: &[u8]) -> PlatformResult<()> {
        Ok(())
    }

    fn delete(&self, _name: &str) -> PlatformResult<()> {
        Ok(())
    }
}

#[derive(Default)]
struct TestPlatform {
    keystore: TestKeyStore,
}

impl Platform for TestPlatform {
    fn info(&self) -> HostInfo {
        HostInfo {
            name: "test-host".to_owned(),
            os: pb::Platform::Linux,
            os_version: "test".to_owned(),
            model: "test".to_owned(),
            arch: "test".to_owned(),
        }
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        Vec::new()
    }

    fn watch_displays(&self, _tx: mpsc::Sender<Vec<DisplayInfo>>) {}

    fn capture(
        &self,
        _display: u32,
        _cfg: CaptureConfig,
        _sink: FrameSink,
    ) -> PlatformResult<Box<dyn CaptureHandle>> {
        Err(PlatformError::Unsupported)
    }

    fn encoder(&self, _cfg: EncoderConfig) -> PlatformResult<Box<dyn Encoder>> {
        Err(PlatformError::Unsupported)
    }

    fn inject(&self, _ev: InputEvent, _display: &DisplayInfo) {}

    fn release_all(&self) {}

    fn session_action(&self, _kind: host_core::platform::SessionActionKind) -> PlatformResult<()> {
        Ok(())
    }

    fn watch_cursor(&self, _tx: mpsc::Sender<CursorUpdate>) {}

    fn watch_local_activity(&self, _tx: mpsc::Sender<bool>) {}

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        None
    }

    fn clipboard_set(&self, _item: ClipboardItem) -> PlatformResult<()> {
        Ok(())
    }

    fn watch_clipboard(&self, _tx: mpsc::Sender<()>) {}

    fn power_keep_awake(&self, _on: bool) {}

    fn console_state(&self) -> ConsoleState {
        ConsoleState::Unknown
    }

    fn permissions(&self) -> Permissions {
        Permissions::default()
    }

    fn request_permission(&self, _permission: PermissionKind) {}

    fn nics(&self) -> Vec<Nic> {
        Vec::new()
    }

    fn send_wol(&self, _mac: [u8; 6], _via: &Nic) -> PlatformResult<()> {
        Ok(())
    }

    fn keystore(&self) -> &dyn KeyStore {
        &self.keystore
    }
}

struct Harness {
    dir: TestDir,
    mgr: Arc<SessionManager<TestPlatform>>,
    clock: Arc<TestClock>,
    updater: Arc<Updater>,
    installer: Arc<FakeInstaller>,
    gate: Arc<MgrGate<TestPlatform>>,
}

impl Harness {
    fn new() -> Self {
        let dir = TestDir::new();
        let clock = Arc::new(TestClock::new());
        let mut config = HostConfig::default();
        config.update.auto_install = true;
        let config = ConfigStore::new(dir.path().join("config.toml"), config);
        let mgr = SessionManager::new(
            Arc::new(TestPlatform::default()),
            HostServices {
                events: Arc::new(NoEvents),
                config: config.clone(),
                audit: None,
                downloads_dir: dir.path().join("downloads"),
                state_dir: None,
                clock: clock.clone(),
            },
        );
        let source = Arc::new(FakeSource::bytes(signed_manifest(
            SEED,
            NEXT,
            PACKAGE,
            json!({}),
        )));
        let downloader = Arc::new(FakeDownloader::new(PACKAGE.to_vec()));
        let verifier = Arc::new(FakeVerifier::new());
        let installer = Arc::new(FakeInstaller::new());
        let gate = Arc::new(MgrGate::new(&mgr));
        let updater = Updater::new(
            UpdaterOptions {
                prepare: PrepareContext {
                    current_version: CURRENT.to_owned(),
                    os: HostOs::Macos,
                    arch: Arch::Universal,
                    work_dir: dir.path().join("update"),
                    keys: trusted(SEED),
                    max_package_bytes: 1024 * 1024,
                },
                config: config.subscribe(),
                initial_delay: Duration::from_secs(300),
                check_interval: Duration::from_secs(6 * 60 * 60),
                initial_status: None,
            },
            UpdaterParts {
                source,
                downloader,
                verifier,
                installer: installer.clone(),
                sessions: gate.clone(),
                audit: None,
                clock: clock.clone(),
            },
        );
        Self {
            dir,
            mgr,
            clock,
            updater,
            installer,
            gate,
        }
    }

    fn admit(&self, device: &str, role: auth::Role) -> (ConnInfo, [u8; 16]) {
        let mut info = ConnInfo {
            key: self.mgr.next_key(),
            session_id: String::new(),
            device_id: device.to_owned(),
            device_fp: [1; 32],
            device_name: device.to_owned(),
            platform: pb::Platform::Ios,
            user: "user".to_owned(),
            node: "phone.hs.example.com".to_owned(),
            path: pb::PathKind::TailnetDirect,
            derp_region: String::new(),
            intent: pb::Intent::Control,
            client_features: 0,
            role,
            can_control: true,
            since_us: self.mgr.now_us(),
            resumed: false,
        };
        let admission = roster::admit(&self.mgr, &info, role).expect("admit");
        info.session_id = admission.session_id;
        info.role = admission.role;
        (info, admission.token)
    }

    fn register(&self, info: ConnInfo) -> mpsc::Receiver<ConnCommand> {
        let (tx, rx) = mpsc::channel(64);
        self.mgr.register(info, tx);
        rx
    }

    fn leave(&self, key: ConnKey, linger: bool) {
        roster::with_roster(&self.mgr, |roster| {
            assert!(roster.leave(key, linger, self.mgr.now_us()).is_some());
        });
        self.mgr.unregister(key);
    }

    fn update(&self) -> tokio::task::JoinHandle<UpdateOutcome> {
        let updater = self.updater.clone();
        tokio::spawn(async move { updater.handle(Trigger::Periodic).await })
    }

    fn assert_waiting(&self) {
        assert_eq!(
            self.updater.status().borrow().phase,
            Phase::WaitingSessionEnd
        );
        assert!(self.installer.installs().is_empty());
        assert!(PendingInstall::load(&self.dir.path().join("update"))
            .expect("read pending install")
            .is_none());
    }

    async fn assert_installed(&self, task: tokio::task::JoinHandle<UpdateOutcome>) {
        assert_eq!(
            task.await.expect("update task"),
            UpdateOutcome::Restarting {
                version: NEXT.to_owned()
            }
        );
        assert_eq!(self.installer.installs().len(), 1);
        assert_eq!(self.installer.restart_count(), 1);
        let pending = PendingInstall::load(&self.dir.path().join("update"))
            .expect("read pending install")
            .expect("pending install exists");
        assert_eq!(pending.state, InstallState::Restarting);
    }
}

async fn wait_for_phase(
    status: &mut watch::Receiver<host_core::update::status::UpdateStatus>,
    phase: Phase,
) {
    tokio::time::timeout(Duration::from_secs(5), async {
        loop {
            assert_ne!(status.borrow().phase, Phase::Failed);
            if status.borrow().phase == phase {
                return;
            }
            status.changed().await.expect("status sender remains alive");
        }
    })
    .await
    .expect("update reaches expected phase");
}

#[tokio::test(start_paused = true)]
async fn lingering_session_blocks_install_until_expiry() {
    for role in [auth::Role::Controller, auth::Role::Viewer] {
        let harness = Harness::new();
        let (info, _) = harness.admit("phone", role);
        let _rx = harness.register(info.clone());
        harness.leave(info.key, true);
        roster::on_host_start(&harness.mgr);
        assert!(harness.mgr.conns().is_empty());
        assert_eq!(harness.gate.active_sessions(), 1);

        let task = harness.update();
        let mut status = harness.updater.status();
        wait_for_phase(&mut status, Phase::WaitingSessionEnd).await;
        tokio::time::advance(Duration::from_micros(LINGER_US - 1)).await;
        harness.assert_waiting();
        assert_eq!(harness.gate.active_sessions(), 1);

        tokio::time::advance(Duration::from_secs(1)).await;
        harness.assert_installed(task).await;
        assert_eq!(harness.gate.active_sessions(), 0);
        assert!(roster::with_roster(&harness.mgr, |roster| roster
            .participants()
            .is_empty()));
    }
}

#[tokio::test(start_paused = true)]
async fn resumed_lingering_session_keeps_update_waiting_until_end() {
    let harness = Harness::new();
    let (mut info, token) = harness.admit("phone", auth::Role::Controller);
    let _old_rx = harness.register(info.clone());
    harness.leave(info.key, true);
    roster::on_host_start(&harness.mgr);
    let task = harness.update();
    let mut status = harness.updater.status();
    wait_for_phase(&mut status, Phase::WaitingSessionEnd).await;
    tokio::time::advance(Duration::from_secs(30)).await;

    info.key = harness.mgr.next_key();
    let ResumeOutcome::Resumed(resumed) =
        roster::resume(&harness.mgr, info.key, &token, &info.device_id)
    else {
        panic!("phone resumes during linger");
    };
    assert_eq!(resumed.session_id, info.session_id);
    info.resumed = true;

    // Authentication consumes the roster token before the transport registry
    // is populated. That admission must keep the gate closed throughout.
    tokio::time::advance(Duration::from_secs(2)).await;
    harness.assert_waiting();
    assert_eq!(harness.gate.active_sessions(), 1);
    let _rx = harness.register(info.clone());

    tokio::time::advance(Duration::from_micros(LINGER_US)).await;
    harness.assert_waiting();
    assert_eq!(harness.gate.active_sessions(), 1);

    harness.leave(info.key, false);
    tokio::time::advance(Duration::from_secs(1)).await;
    harness.assert_installed(task).await;
}

#[tokio::test(start_paused = true)]
async fn no_sessions_install_immediately() {
    let harness = Harness::new();
    assert_eq!(harness.gate.active_sessions(), 0);
    harness.assert_installed(harness.update()).await;
}

#[tokio::test(start_paused = true)]
async fn expired_entries_do_not_block_or_get_removed_by_query() {
    let harness = Harness::new();
    let (info, _) = harness.admit("phone", auth::Role::Viewer);
    harness.leave(info.key, true);
    let deadline = harness.clock.now_micros() + LINGER_US;
    let before = roster::with_roster(&harness.mgr, |roster| roster.clone());
    tokio::time::advance(Duration::from_micros(LINGER_US - 1)).await;
    assert_eq!(harness.gate.active_sessions(), 1);
    tokio::time::advance(Duration::from_micros(1)).await;
    assert_eq!(harness.clock.now_micros(), deadline);
    assert_eq!(harness.gate.active_sessions(), 0);
    assert_eq!(
        before,
        roster::with_roster(&harness.mgr, |roster| roster.clone())
    );
    harness.assert_installed(harness.update()).await;
}

#[tokio::test(start_paused = true)]
async fn gate_counts_connected_and_lingering_sessions_including_files() {
    let harness = Harness::new();
    let (controller, _) = harness.admit("controller", auth::Role::Controller);
    let _controller_rx = harness.register(controller.clone());
    let (viewer, _) = harness.admit("viewer", auth::Role::Viewer);
    harness.leave(viewer.key, true);

    let mut files = viewer;
    files.key = harness.mgr.next_key();
    files.intent = pb::Intent::Files;
    let admission = roster::admit(&harness.mgr, &files, auth::Role::Viewer).expect("admit files");
    files.session_id = admission.session_id;
    let _files_rx = harness.register(files.clone());
    assert_eq!(harness.gate.active_sessions(), 3);

    let task = harness.update();
    let mut status = harness.updater.status();
    wait_for_phase(&mut status, Phase::WaitingSessionEnd).await;
    harness.leave(controller.key, false);
    tokio::time::advance(Duration::from_micros(LINGER_US)).await;
    harness.assert_waiting();
    assert_eq!(harness.gate.active_sessions(), 1);

    harness.mgr.unregister(files.key);
    tokio::time::advance(Duration::from_secs(1)).await;
    harness.assert_installed(task).await;
}
