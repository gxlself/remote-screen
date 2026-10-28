use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use host_core::clock::SystemClock;
use host_core::config::{ConfigStore, HostConfig};
use host_core::ipc::Event;
use host_core::session::approval::PromptEvents;
use host_core::session::manager::{HostServices, SessionManager};
use host_core::update::agent::{
    attach, UpdatePlatform, UpdateUnavailable, UpdateWiring, DEFAULT_CHECK_INTERVAL,
    DEFAULT_INITIAL_DELAY,
};
use host_core::update::install::{InstallState, Installer, PendingInstall};
use host_core::update::manifest::TrustedKeys;
use host_core::update::prepare::{Downloader, PackageVerifier, ReleaseSource};
use host_core::update::types::{Arch, Channel, HostOs, UpdateState};
use tokio::sync::watch;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const TEST_KEY: &str = "11qYAYKxCrfVS_7TyWQHOg7hcvPapiMlrwIaaPcHURo";

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-h23b-update-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        std::fs::create_dir_all(&path).expect("create update test directory");
        Self(path)
    }
    fn path(&self) -> &Path {
        &self.0
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

#[derive(Default)]
struct RecordingEvents(Mutex<Vec<Event>>);
impl RecordingEvents {
    fn events(&self) -> Vec<Event> {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).clone()
    }
}
impl PromptEvents for RecordingEvents {
    fn emit(&self, event: Event) -> bool {
        self.0.lock().unwrap_or_else(|p| p.into_inner()).push(event);
        true
    }
}

#[derive(Default)]
struct FakeInstaller {
    installs: AtomicUsize,
    restarts: AtomicUsize,
    rollbacks: AtomicUsize,
    rollback_ok: AtomicBool,
}
impl FakeInstaller {
    fn with_rollback_ok(ok: bool) -> Self {
        Self {
            rollback_ok: AtomicBool::new(ok),
            ..Self::default()
        }
    }
}
impl Installer for FakeInstaller {
    fn install(&self, _package: &Path, _backup_dir: &Path) -> Result<(), String> {
        self.installs.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn restart(&self) -> Result<(), String> {
        self.restarts.fetch_add(1, Ordering::SeqCst);
        Ok(())
    }
    fn rollback(&self, _backup_dir: &Path) -> Result<(), String> {
        self.rollbacks.fetch_add(1, Ordering::SeqCst);
        if self.rollback_ok.load(Ordering::SeqCst) {
            Ok(())
        } else {
            Err("rollback failed".to_owned())
        }
    }
}

#[derive(Default)]
struct FakeVerifier;
impl PackageVerifier for FakeVerifier {
    fn verify_platform(&self, _package: &Path) -> Result<(), String> {
        Ok(())
    }
}

#[derive(Default)]
struct FakeDownloader;
#[async_trait]
impl Downloader for FakeDownloader {
    async fn download(&self, _url: &str, _dest: &Path, _max_bytes: u64) -> Result<(), String> {
        Err("download unavailable in test".to_owned())
    }
}

#[derive(Default)]
struct FakeSource {
    calls: AtomicUsize,
}
#[async_trait]
impl ReleaseSource for FakeSource {
    async fn fetch(
        &self,
        _channel: Channel,
        _os: HostOs,
        _arch: Arch,
        _from: &str,
    ) -> Result<Option<Vec<u8>>, String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        Ok(None)
    }
}

fn manager(
    dir: &Path,
    platform: &host_fake::FakePlatform,
    events: Arc<RecordingEvents>,
) -> Arc<SessionManager<host_fake::FakePlatform>> {
    let config = ConfigStore::new(dir.join("config.toml"), HostConfig::default());
    SessionManager::new(
        Arc::new(platform.clone()),
        HostServices {
            events,
            config,
            audit: None,
            downloads_dir: dir.join("downloads"),
            state_dir: Some(dir.to_owned()),
            clock: Arc::new(SystemClock),
        },
    )
}

fn wiring(
    dir: &Path,
    listening: bool,
    installer: Arc<FakeInstaller>,
    keys: TrustedKeys,
    source: Option<Arc<dyn ReleaseSource>>,
) -> UpdateWiring {
    let (_tx, rx) = watch::channel(listening);
    UpdateWiring {
        work_dir: dir.to_owned(),
        current_version: "1.0.0".to_owned(),
        keys,
        platform: UpdatePlatform {
            installer,
            verifier: Arc::new(FakeVerifier),
            downloader: Arc::new(FakeDownloader),
        },
        source,
        listening: rx,
        audit: None,
        initial_delay: DEFAULT_INITIAL_DELAY,
        check_interval: DEFAULT_CHECK_INTERVAL,
    }
}

fn pending_record(dir: &Path) {
    PendingInstall {
        from: "0.0.1".to_owned(),
        to: "1.0.0".to_owned(),
        backup_dir: dir.join("backup"),
        state: InstallState::Restarting,
        boot_attempts: 0,
        reason: None,
        started_at_us: 0,
    }
    .save(dir)
    .expect("write pending install record");
}

#[tokio::test]
async fn disabled_without_keys_or_source() {
    let dir = TestDir::new();
    let events = Arc::new(RecordingEvents::default());
    let platform = host_fake::FakePlatform::new();
    let mgr = manager(dir.path(), &platform, Arc::clone(&events));
    let installer = Arc::new(FakeInstaller::with_rollback_ok(true));
    let service = attach(
        &mgr,
        wiring(
            dir.path(),
            true,
            installer,
            TrustedKeys::from_base64url(&[]).expect("empty keys"),
            None,
        ),
    );
    assert_eq!(
        service.recovered().await,
        host_core::update::recovery::StartupOutcome::Clean
    );
    assert_eq!(service.ipc_status().state, "idle");
    assert_eq!(service.install_now(), Err(UpdateUnavailable::Disabled));
    assert_eq!(service.heartbeat().state, UpdateState::Idle);
    service.shutdown();
}

#[tokio::test]
async fn self_check_confirms_and_emits_update() {
    let dir = TestDir::new();
    pending_record(dir.path());
    let events = Arc::new(RecordingEvents::default());
    let platform = host_fake::FakePlatform::new();
    let mgr = manager(dir.path(), &platform, Arc::clone(&events));
    let service = attach(
        &mgr,
        wiring(
            dir.path(),
            true,
            Arc::new(FakeInstaller::with_rollback_ok(true)),
            TrustedKeys::from_base64url(&[]).expect("empty keys"),
            None,
        ),
    );
    assert!(matches!(
        service.recovered().await,
        host_core::update::recovery::StartupOutcome::Confirmed { .. }
    ));
    assert_eq!(
        service.status().phase,
        host_core::update::status::Phase::Updated
    );
    assert_eq!(service.heartbeat().state, UpdateState::Ready);
    assert_eq!(service.ipc_status().state, "idle");
    assert!(events
        .events()
        .iter()
        .any(|event| matches!(event, Event::UpdateState(_))));
    service.shutdown();
}

#[tokio::test(start_paused = true)]
async fn no_display_rolls_back_or_reports_failure() {
    for rollback_ok in [true, false] {
        let dir = TestDir::new();
        pending_record(dir.path());
        let events = Arc::new(RecordingEvents::default());
        let platform = host_fake::FakePlatform::new();
        platform.set_displays(Vec::new());
        let mgr = manager(dir.path(), &platform, events);
        let installer = Arc::new(FakeInstaller::with_rollback_ok(rollback_ok));
        let service = attach(
            &mgr,
            wiring(
                dir.path(),
                true,
                Arc::clone(&installer),
                TrustedKeys::from_base64url(&[]).expect("empty keys"),
                None,
            ),
        );
        tokio::task::yield_now().await;
        let waiter = tokio::spawn({
            let service = Arc::clone(&service);
            async move { service.recovered().await }
        });
        tokio::time::advance(Duration::from_secs(16)).await;
        let outcome = waiter.await.expect("recovery waiter");
        assert!(matches!(
            outcome,
            host_core::update::recovery::StartupOutcome::RollingBack { .. }
                | host_core::update::recovery::StartupOutcome::RollbackFailed { .. }
        ));
        assert_eq!(installer.rollbacks.load(Ordering::SeqCst), 1);
        assert_eq!(service.install_now(), Err(UpdateUnavailable::Disabled));
        service.shutdown();
    }
}

#[tokio::test(start_paused = true)]
async fn listening_timeout_rolls_back() {
    let dir = TestDir::new();
    pending_record(dir.path());
    let events = Arc::new(RecordingEvents::default());
    let platform = host_fake::FakePlatform::new();
    let mgr = manager(dir.path(), &platform, events);
    let installer = Arc::new(FakeInstaller::with_rollback_ok(true));
    let service = attach(
        &mgr,
        wiring(
            dir.path(),
            false,
            Arc::clone(&installer),
            TrustedKeys::from_base64url(&[]).expect("empty keys"),
            None,
        ),
    );
    tokio::task::yield_now().await;
    let waiter = tokio::spawn({
        let service = Arc::clone(&service);
        async move { service.recovered().await }
    });
    tokio::time::advance(Duration::from_secs(41)).await;
    assert!(matches!(
        waiter.await.expect("recovery waiter"),
        host_core::update::recovery::StartupOutcome::RollingBack { .. }
    ));
    assert_eq!(installer.rollbacks.load(Ordering::SeqCst), 1);
    service.shutdown();
}

#[tokio::test]
async fn updater_runs_periodic_check_against_source() {
    let dir = TestDir::new();
    let events = Arc::new(RecordingEvents::default());
    let platform = host_fake::FakePlatform::new();
    let mgr = manager(dir.path(), &platform, events);
    let source = Arc::new(FakeSource::default());
    let mut wire = wiring(
        dir.path(),
        true,
        Arc::new(FakeInstaller::with_rollback_ok(true)),
        TrustedKeys::from_base64url(&[TEST_KEY]).expect("test key"),
        Some(Arc::clone(&source) as Arc<dyn ReleaseSource>),
    );
    wire.initial_delay = Duration::from_millis(10);
    let service = attach(&mgr, wire);
    assert_eq!(
        service.recovered().await,
        host_core::update::recovery::StartupOutcome::Clean
    );
    tokio::time::timeout(Duration::from_secs(5), async {
        while source.calls.load(Ordering::SeqCst) == 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("periodic release check");
    assert_eq!(service.heartbeat().state, UpdateState::Idle);
    assert_eq!(service.install_now(), Ok(()));
    service.shutdown();
}

#[tokio::test]
async fn repeated_attach_keeps_first_service() {
    let dir = TestDir::new();
    let platform = host_fake::FakePlatform::new();
    let mgr = manager(dir.path(), &platform, Arc::new(RecordingEvents::default()));
    let first = attach(
        &mgr,
        wiring(
            dir.path(),
            true,
            Arc::new(FakeInstaller::with_rollback_ok(true)),
            TrustedKeys::from_base64url(&[]).expect("empty keys"),
            None,
        ),
    );
    first.recovered().await;
    let second = attach(
        &mgr,
        wiring(
            dir.path(),
            true,
            Arc::new(FakeInstaller::with_rollback_ok(true)),
            TrustedKeys::from_base64url(&[]).expect("empty keys"),
            None,
        ),
    );
    assert!(Arc::ptr_eq(
        &mgr.updater().expect("manager updater"),
        &first
    ));
    assert_eq!(second.install_now(), Err(UpdateUnavailable::Disabled));
    second.shutdown();
    first.shutdown();
}

#[tokio::test]
async fn shutdown_allows_manager_drop() {
    let dir = TestDir::new();
    let mgr = manager(
        dir.path(),
        &host_fake::FakePlatform::new(),
        Arc::new(RecordingEvents::default()),
    );
    let weak = Arc::downgrade(&mgr);
    let service = attach(
        &mgr,
        wiring(
            dir.path(),
            true,
            Arc::new(FakeInstaller::with_rollback_ok(true)),
            TrustedKeys::from_base64url(&[]).expect("empty keys"),
            None,
        ),
    );
    service.shutdown();
    drop(mgr);
    tokio::task::yield_now().await;
    assert!(weak.upgrade().is_none());
}
