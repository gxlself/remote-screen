#[path = "update_support/mod.rs"]
mod support;

use std::sync::Arc;
use std::time::{Duration, Instant};

use host_core::config::HostConfig;
use host_core::update::install::{
    InstallState, PendingInstall, UpdateOutcome, Updater, UpdaterOptions, UpdaterParts,
};
use host_core::update::prepare::{PrepareContext, Trigger};
use host_core::update::status::Phase;
use host_core::update::types::{Arch, HostOs};
use serde_json::json;
use support::{
    signed_manifest, trusted, FakeDownloader, FakeGate, FakeInstaller, FakeSource, FakeVerifier,
    TestClock, TestDir,
};
use tokio::sync::watch;

const SEED: [u8; 32] = [7; 32];
const PACKAGE: &[u8] = b"signed update package contents";
const CURRENT: &str = "0.3.1";
const NEXT: &str = "0.3.2";

struct Harness {
    dir: TestDir,
    updater: Arc<Updater>,
    installer: Arc<FakeInstaller>,
    gate: Arc<FakeGate>,
}

impl Harness {
    fn new(auto_install: bool, active_sessions: usize) -> Self {
        let dir = TestDir::new();
        let source = Arc::new(FakeSource::bytes(signed_manifest(
            SEED,
            NEXT,
            PACKAGE,
            json!({}),
        )));
        let downloader = Arc::new(FakeDownloader::new(PACKAGE.to_vec()));
        let verifier = Arc::new(FakeVerifier::new());
        let installer = Arc::new(FakeInstaller::new());
        let gate = Arc::new(FakeGate::new(active_sessions));
        let mut config = HostConfig::default();
        config.update.auto_install = auto_install;
        let (_config_tx, config_rx) = watch::channel(Arc::new(config));
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
                config: config_rx,
                initial_delay: Duration::from_secs(300),
                check_interval: Duration::from_secs(6 * 60 * 60),
                initial_status: None,
            },
            UpdaterParts {
                source: source.clone(),
                downloader,
                verifier,
                installer: installer.clone(),
                sessions: gate.clone(),
                audit: None,
                clock: Arc::new(TestClock::new()),
            },
        );
        Self {
            dir,
            updater,
            installer,
            gate,
        }
    }

    fn periodic(&self) -> Trigger {
        Trigger::Periodic
    }
}

#[tokio::test(start_paused = true)]
async fn installs_and_restarts_without_sessions() {
    let harness = Harness::new(true, 0);
    let status = harness.updater.status();
    let outcome = harness.updater.handle(harness.periodic()).await;
    assert_eq!(
        outcome,
        UpdateOutcome::Restarting {
            version: NEXT.into()
        }
    );
    assert_eq!(harness.installer.installs().len(), 1);
    assert_eq!(
        harness.installer.installs()[0].1,
        harness.dir.path().join("update/backup/0.3.1")
    );
    assert_eq!(harness.installer.restart_count(), 1);
    let pending = PendingInstall::load(&harness.dir.path().join("update"))
        .expect("load pending install")
        .expect("pending install");
    assert_eq!(pending.state, InstallState::Restarting);
    assert_eq!(pending.from, CURRENT);
    assert_eq!(pending.to, NEXT);
    assert_eq!(status.borrow().phase, Phase::Restarting);
}

#[tokio::test(start_paused = true)]
async fn waits_for_sessions_then_installs() {
    let harness = Harness::new(true, 1);
    let updater = harness.updater.clone();
    let task = tokio::spawn(async move { updater.handle(Trigger::Periodic).await });
    let mut status = harness.updater.status();
    wait_for_phase(&mut status, Phase::WaitingSessionEnd).await;
    assert!(harness.installer.installs().is_empty());
    harness.gate.release();
    assert_eq!(
        task.await.expect("update task"),
        UpdateOutcome::Restarting {
            version: NEXT.into()
        }
    );
}

#[tokio::test(start_paused = true)]
async fn install_now_closes_sessions() {
    let harness = Harness::new(true, 1);
    let updater = harness.updater.clone();
    let task = tokio::spawn(async move { updater.handle(Trigger::Periodic).await });
    let mut status = harness.updater.status();
    wait_for_phase(&mut status, Phase::WaitingSessionEnd).await;
    harness.updater.install_now();
    assert_eq!(
        task.await.expect("update task"),
        UpdateOutcome::Restarting {
            version: NEXT.into()
        }
    );
    assert_eq!(harness.gate.close_calls(), 1);
}

#[tokio::test(start_paused = true)]
async fn ready_update_is_installed_by_background_run() {
    let harness = Harness::new(false, 0);
    let mut status = harness.updater.status();
    let run = tokio::spawn(harness.updater.clone().run());
    tokio::task::yield_now().await;
    tokio::time::advance(Duration::from_secs(300)).await;
    wait_for_phase(&mut status, Phase::Ready).await;
    assert_eq!(harness.updater.status().borrow().phase, Phase::Ready);
    harness.updater.install_now();
    wait_for_phase(&mut status, Phase::Restarting).await;
    assert_eq!(harness.updater.status().borrow().phase, Phase::Restarting);
    run.abort();
}

#[tokio::test(start_paused = true)]
async fn install_now_before_waiting_does_not_leave_a_permit() {
    let harness = Harness::new(true, 1);
    harness.updater.install_now();
    let updater = harness.updater.clone();
    let task = tokio::spawn(async move { updater.handle(Trigger::Periodic).await });
    let mut status = harness.updater.status();
    wait_for_phase(&mut status, Phase::WaitingSessionEnd).await;
    assert!(harness.installer.installs().is_empty());
    harness.gate.release();
    assert_eq!(
        task.await.expect("update task"),
        UpdateOutcome::Restarting {
            version: NEXT.into()
        }
    );
}

#[tokio::test(start_paused = true)]
async fn install_and_restart_failures_have_expected_records() {
    let harness = Harness::new(true, 0);
    harness.installer.fail_install("disk full");
    let outcome = harness.updater.handle(Trigger::Periodic).await;
    assert_eq!(
        outcome,
        UpdateOutcome::Failed {
            code: "install",
            message: "安装失败".to_owned()
        }
    );
    assert!(PendingInstall::load(&harness.dir.path().join("update"))
        .expect("load pending")
        .is_none());

    let harness = Harness::new(true, 0);
    harness.installer.fail_restart("launchctl failed");
    let outcome = harness.updater.handle(Trigger::Periodic).await;
    assert_eq!(
        outcome,
        UpdateOutcome::Failed {
            code: "restart",
            message: "重启失败".to_owned()
        }
    );
    assert_eq!(
        PendingInstall::load(&harness.dir.path().join("update"))
            .expect("load pending")
            .expect("pending record")
            .state,
        InstallState::Restarting
    );
}

#[tokio::test(start_paused = true)]
async fn pending_install_round_trip_and_bad_json() {
    let harness = Harness::new(true, 0);
    let work_dir = harness.dir.path().join("pending");
    let pending = PendingInstall {
        from: CURRENT.to_owned(),
        to: NEXT.to_owned(),
        backup_dir: work_dir.join("backup/0.3.1"),
        state: InstallState::Installing,
        boot_attempts: 2,
        reason: Some("test".to_owned()),
        started_at_us: 123,
    };
    pending.save(&work_dir).expect("save pending");
    assert_eq!(
        PendingInstall::load(&work_dir).expect("load pending"),
        Some(pending)
    );
    PendingInstall::remove(&work_dir).expect("remove pending");
    PendingInstall::remove(&work_dir).expect("remove missing pending");
    std::fs::create_dir_all(&work_dir).expect("create work dir");
    std::fs::write(PendingInstall::path(&work_dir), b"{").expect("write bad json");
    assert_eq!(
        PendingInstall::load(&work_dir)
            .expect_err("bad json must fail")
            .kind(),
        std::io::ErrorKind::InvalidData
    );
}

async fn wait_for_phase(
    status: &mut watch::Receiver<host_core::update::status::UpdateStatus>,
    phase: Phase,
) {
    let deadline = Instant::now() + Duration::from_secs(30);
    let observed = status.clone();
    // Keep the paused runtime runnable: a Tokio timeout could auto-advance
    // while package verification is still running on a real worker thread.
    let ready = status.wait_for(|state| state.phase == phase);
    tokio::pin!(ready);
    loop {
        tokio::select! {
            biased;
            result = &mut ready => {
                result.expect("status sender alive");
                return;
            }
            _ = tokio::task::yield_now() => {
                assert!(
                    Instant::now() < deadline,
                    "timed out waiting for {phase:?}; current status: {:?}",
                    *observed.borrow()
                );
            }
        }
    }
}
