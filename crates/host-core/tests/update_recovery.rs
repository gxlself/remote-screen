#[path = "update_support/mod.rs"]
mod support;

use host_core::audit::AuditEvent;
use host_core::clock::Clock;
use host_core::update::install::{InstallState, PendingInstall};
use host_core::update::prepare::skipped_versions;
use host_core::update::recovery::{
    initial_status, recover_on_startup, self_check_status, startup_status, StartupOutcome,
    MAX_BOOT_ATTEMPTS,
};
use host_core::update::status::{Phase, UpdateStatus};
use host_core::update::types::{Channel, UpdateState};
use serde_json::json;
use std::fs;
use std::path::PathBuf;
use std::time::Duration;
use support::{FakeSelfCheck, RecordingInstaller, TestClock, TestDir};

const FROM: &str = "0.3.1";
const TO: &str = "0.3.2";

struct Harness {
    _dir: TestDir,
    work_dir: PathBuf,
    backup_dir: PathBuf,
    installer: RecordingInstaller,
    check: FakeSelfCheck,
    clock: TestClock,
    audit: host_core::audit::AuditQueue,
}

impl Harness {
    fn new() -> Self {
        let dir = TestDir::new();
        let work_dir = dir.path().join("update");
        let backup_dir = work_dir.join("backup").join(FROM);
        fs::create_dir_all(&backup_dir).expect("create backup directory");
        let audit =
            host_core::audit::AuditQueue::open(dir.path().join("audit")).expect("open audit queue");
        Self {
            _dir: dir,
            work_dir,
            backup_dir,
            installer: RecordingInstaller::new(),
            check: FakeSelfCheck::new(),
            clock: TestClock::new(),
            audit,
        }
    }

    fn save(&self, state: InstallState, boot_attempts: u32, reason: Option<&str>) {
        PendingInstall {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            backup_dir: self.backup_dir.clone(),
            state,
            boot_attempts,
            reason: reason.map(str::to_owned),
            started_at_us: self.clock.now_micros(),
        }
        .save(&self.work_dir)
        .expect("save pending install");
    }

    async fn recover(&self, current_version: &str) -> StartupOutcome {
        recover_on_startup(
            &self.work_dir,
            current_version,
            &self.installer,
            &self.check,
            Some(&self.audit),
            &self.clock,
        )
        .await
    }

    fn audit(&self) -> Vec<AuditEvent> {
        self.audit
            .batch(100)
            .expect("read audit queue")
            .items
            .into_iter()
            .map(|item| item.record)
            .collect()
    }

    fn pending(&self) -> Option<PendingInstall> {
        PendingInstall::load(&self.work_dir).expect("load pending install")
    }

    fn assert_backup_exists(&self) {
        assert!(self.backup_dir.is_dir(), "backup directory must exist");
    }
}

#[tokio::test(start_paused = true)]
async fn missing_record_is_clean() {
    let harness = Harness::new();
    assert_eq!(harness.recover(TO).await, StartupOutcome::Clean);
    assert!(harness.audit().is_empty());
}

#[tokio::test(start_paused = true)]
async fn successful_self_check_cleans_up_and_records_installation() {
    let harness = Harness::new();
    fs::create_dir_all(harness.work_dir.join("download")).expect("create download directory");
    harness.save(InstallState::Restarting, 0, None);

    let outcome = harness.recover(TO).await;
    assert_eq!(
        outcome,
        StartupOutcome::Confirmed {
            from: FROM.to_owned(),
            to: TO.to_owned(),
        }
    );
    assert_eq!(harness.check.calls(), 1);
    assert!(harness.pending().is_none());
    assert!(!harness.backup_dir.exists());
    assert!(!harness.work_dir.join("download").exists());

    let events = harness.audit();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "upgrade");
    assert_eq!(
        events[0].detail,
        Some(json!({"phase": "installed", "from": FROM, "to": TO}))
    );

    let status = startup_status(&outcome, Channel::Stable);
    assert_eq!(status.phase, Phase::Updated);
    assert_eq!(status.to_heartbeat().state, UpdateState::Ready);
    assert_eq!(status.ipc_state(), "idle");
    assert_eq!(status.pending, None);
}

#[tokio::test(start_paused = true)]
async fn failed_self_check_rolls_back_and_skips_bad_version() {
    let harness = Harness::new();
    harness.check.fail("no frame");
    harness.save(InstallState::Restarting, 0, None);

    assert_eq!(
        harness.recover(TO).await,
        StartupOutcome::RollingBack {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            reason: "self_check: no frame".to_owned(),
        }
    );
    assert_eq!(
        harness.installer.rollbacks(),
        vec![harness.backup_dir.clone()]
    );
    assert_eq!(harness.installer.restart_count(), 1);
    let pending = harness.pending().expect("rollback record retained");
    assert_eq!(pending.state, InstallState::RolledBack);
    assert!(pending
        .reason
        .as_deref()
        .is_some_and(|reason| reason.starts_with("self_check")));
    assert_eq!(skipped_versions(&harness.work_dir), vec![TO.to_owned()]);
    assert!(!harness.audit().iter().any(|event| {
        event
            .detail
            .as_ref()
            .and_then(|detail| detail.get("phase"))
            .and_then(|phase| phase.as_str())
            == Some("rollback")
    }));
}

#[tokio::test(start_paused = true)]
async fn self_check_timeout_rolls_back_after_sixty_seconds() {
    let harness = Harness::new();
    harness.check.never_returns();
    harness.save(InstallState::Restarting, 0, None);

    let recovery = recover_on_startup(
        &harness.work_dir,
        TO,
        &harness.installer,
        &harness.check,
        Some(&harness.audit),
        &harness.clock,
    );
    tokio::pin!(recovery);
    tokio::select! {
        _ = &mut recovery => panic!("self-check returned before the timeout"),
        _ = tokio::time::sleep(Duration::from_secs(59)) => {}
    }
    tokio::time::advance(Duration::from_secs(1)).await;

    assert_eq!(
        recovery.await,
        StartupOutcome::RollingBack {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            reason: "self_check_timeout".to_owned(),
        }
    );
    assert_eq!(harness.check.calls(), 1);
    assert_eq!(harness.installer.rollback_count(), 1);
}

#[tokio::test(start_paused = true)]
async fn boot_loop_is_bounded_and_attempt_is_saved_before_self_check() {
    let harness = Harness::new();
    harness.save(InstallState::Restarting, MAX_BOOT_ATTEMPTS, None);

    assert_eq!(
        harness.recover(TO).await,
        StartupOutcome::RollingBack {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            reason: "boot_loop".to_owned(),
        }
    );
    assert_eq!(harness.check.calls(), 0);
    assert_eq!(
        harness.pending().expect("rollback record").boot_attempts,
        MAX_BOOT_ATTEMPTS + 1
    );

    let harness = Harness::new();
    harness.check.fail("not ready");
    harness.save(InstallState::Restarting, 1, None);
    assert!(matches!(
        harness.recover(TO).await,
        StartupOutcome::RollingBack { .. }
    ));
    assert_eq!(harness.check.calls(), 1);
    assert_eq!(harness.pending().expect("rollback record").boot_attempts, 2);
}

#[tokio::test(start_paused = true)]
async fn rollback_failure_is_reported_and_record_retained() {
    let harness = Harness::new();
    harness.check.fail("capture failed");
    harness.installer.fail_rollback("permission denied");
    harness.save(InstallState::Restarting, 0, None);

    assert_eq!(
        harness.recover(TO).await,
        StartupOutcome::RollbackFailed {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            error: "permission denied".to_owned(),
        }
    );
    assert_eq!(harness.installer.restart_count(), 0);
    harness.assert_backup_exists();
    assert_eq!(
        harness.pending().expect("failed rollback record").state,
        InstallState::RolledBack
    );
    let events = harness.audit();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].detail,
        Some(json!({
            "phase": "rollback_failed",
            "from": FROM,
            "to": TO,
            "error": "permission denied",
        }))
    );
}

#[tokio::test(start_paused = true)]
async fn old_version_reports_completed_rollback_and_failed_heartbeat() {
    let harness = Harness::new();
    harness.save(InstallState::RolledBack, 2, Some("self_check: no frame"));

    assert_eq!(
        harness.recover(FROM).await,
        StartupOutcome::RollbackReported {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            reason: "self_check: no frame".to_owned(),
        }
    );
    assert!(harness.pending().is_none());
    let events = harness.audit();
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "upgrade");
    assert_eq!(
        events[0].detail,
        Some(json!({
            "phase": "rollback",
            "from": FROM,
            "to": TO,
            "reason": "self_check: no frame",
        }))
    );

    let status = startup_status(
        &StartupOutcome::RollbackReported {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            reason: "self_check: no frame".to_owned(),
        },
        Channel::Stable,
    );
    assert_eq!(status.phase, Phase::RolledBack);
    assert_eq!(status.pending.as_deref(), Some(TO));
    assert_eq!(status.error.as_deref(), Some("新版本自检失败，已回滚"));
    assert_eq!(status.to_heartbeat().state, UpdateState::RolledBack);
}

#[tokio::test(start_paused = true)]
async fn old_version_reports_incomplete_install() {
    let harness = Harness::new();
    harness.save(InstallState::Restarting, 1, None);

    assert_eq!(
        harness.recover(FROM).await,
        StartupOutcome::InstallFailed {
            from: FROM.to_owned(),
            to: TO.to_owned(),
        }
    );
    assert!(harness.pending().is_none());
    let events = harness.audit();
    assert_eq!(events.len(), 1);
    assert_eq!(
        events[0].detail,
        Some(json!({
            "phase": "failed",
            "from": FROM,
            "to": TO,
            "reason": "install_incomplete",
        }))
    );
    let status = startup_status(
        &StartupOutcome::InstallFailed {
            from: FROM.to_owned(),
            to: TO.to_owned(),
        },
        Channel::Stable,
    );
    assert_eq!(status.phase, Phase::Failed);
    assert_eq!(status.pending.as_deref(), Some(TO));
    assert_eq!(status.error.as_deref(), Some("安装未完成"));
}

#[tokio::test(start_paused = true)]
async fn mismatched_and_corrupt_records_are_stale() {
    let harness = Harness::new();
    harness.save(InstallState::Restarting, 0, None);
    assert_eq!(harness.recover("0.4.0").await, StartupOutcome::Stale);
    assert!(harness.pending().is_none());

    let harness = Harness::new();
    fs::write(PendingInstall::path(&harness.work_dir), b"{not json")
        .expect("write corrupt pending install");
    assert_eq!(harness.recover(TO).await, StartupOutcome::Stale);
    assert!(!PendingInstall::path(&harness.work_dir).exists());
}

#[tokio::test(start_paused = true)]
async fn rolled_back_new_version_retries_rollback() {
    let harness = Harness::new();
    harness.save(InstallState::RolledBack, 1, Some("self_check: no frame"));

    assert_eq!(
        harness.recover(TO).await,
        StartupOutcome::RollingBack {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            reason: "self_check: no frame".to_owned(),
        }
    );
    assert_eq!(harness.check.calls(), 0);
    assert_eq!(harness.installer.rollback_count(), 1);
    assert_eq!(harness.installer.restart_count(), 1);
    assert_eq!(skipped_versions(&harness.work_dir), vec![TO.to_owned()]);
    let pending = harness.pending().expect("rollback record retained");
    assert_eq!(pending.state, InstallState::RolledBack);
    assert_eq!(pending.reason.as_deref(), Some("self_check: no frame"));
}

#[tokio::test(start_paused = true)]
async fn rollback_failed_status_is_failed() {
    let status = startup_status(
        &StartupOutcome::RollbackFailed {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            error: "permission denied".to_owned(),
        },
        Channel::Beta,
    );
    assert_eq!(status.phase, Phase::Failed);
    assert_eq!(status.pending.as_deref(), Some(TO));
    assert_eq!(status.error.as_deref(), Some("回滚失败"));
}

#[test]
fn startup_status_maps_success_to_updated_heartbeat() {
    let outcome = StartupOutcome::Confirmed {
        from: FROM.to_owned(),
        to: TO.to_owned(),
    };
    let status = startup_status(&outcome, Channel::Stable);
    assert_eq!(status.phase, Phase::Updated);
    assert_eq!(status.to_heartbeat().state, UpdateState::Ready);
    assert_eq!(status.ipc_state(), "idle");
    assert_eq!(status.pending, None);
}

#[test]
fn startup_status_keeps_failed_outcomes_failed() {
    for outcome in [
        StartupOutcome::InstallFailed {
            from: FROM.to_owned(),
            to: TO.to_owned(),
        },
        StartupOutcome::RollbackFailed {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            error: "permission denied".to_owned(),
        },
    ] {
        let status = startup_status(&outcome, Channel::Stable);
        assert_eq!(status.phase, Phase::Failed);
        assert_eq!(status.to_heartbeat().state, UpdateState::Failed);
    }
}

#[test]
fn startup_status_maps_clean_rolling_back_and_stale_to_idle() {
    for outcome in [
        StartupOutcome::Clean,
        StartupOutcome::RollingBack {
            from: FROM.to_owned(),
            to: TO.to_owned(),
            reason: "self_check: no frame".to_owned(),
        },
        StartupOutcome::Stale,
    ] {
        let status = startup_status(&outcome, Channel::Stable);
        assert_eq!(status.phase, Phase::Idle);
        assert_eq!(status.to_heartbeat().state, UpdateState::Idle);
    }
}

#[test]
fn self_check_status_reports_restarting_with_target_pending() {
    let status = self_check_status(TO, Channel::Beta);
    assert_eq!(status.phase, Phase::Restarting);
    assert_eq!(status.pending.as_deref(), Some(TO));
    assert_eq!(status.to_heartbeat().state, UpdateState::Restarting);
    assert_ne!(status.to_heartbeat().state, UpdateState::Ready);
}

#[test]
fn initial_status_uses_pending_record_for_self_check() {
    let harness = Harness::new();
    harness.save(InstallState::Installing, 0, None);

    let status = initial_status(&harness.work_dir, TO, Channel::Stable);
    assert_eq!(status.phase, Phase::Restarting);
    assert_eq!(status.pending.as_deref(), Some(TO));
    assert_eq!(status.to_heartbeat().state, UpdateState::Restarting);
    assert_ne!(status.to_heartbeat().state, UpdateState::Ready);

    assert_eq!(
        initial_status(&harness.work_dir, FROM, Channel::Stable),
        UpdateStatus::idle(Channel::Stable)
    );

    harness.save(InstallState::Restarting, 0, None);
    assert_eq!(
        initial_status(&harness.work_dir, TO, Channel::Stable)
            .to_heartbeat()
            .state,
        UpdateState::Restarting
    );
}

#[test]
fn initial_status_is_idle_for_rolled_back_missing_and_corrupt_records() {
    let harness = Harness::new();
    harness.save(InstallState::RolledBack, 1, Some("self_check: no frame"));
    assert_eq!(
        initial_status(&harness.work_dir, TO, Channel::Stable),
        UpdateStatus::idle(Channel::Stable)
    );

    let empty = Harness::new();
    assert_eq!(
        initial_status(&empty.work_dir, TO, Channel::Stable),
        UpdateStatus::idle(Channel::Stable)
    );

    fs::write(PendingInstall::path(&harness.work_dir), b"{not json")
        .expect("write corrupt pending install");
    assert_eq!(
        initial_status(&harness.work_dir, TO, Channel::Stable),
        UpdateStatus::idle(Channel::Stable)
    );
}

#[tokio::test(start_paused = true)]
async fn failed_self_check_then_old_version_reports_rolled_back() {
    let harness = Harness::new();
    harness.check.fail("no frame");
    harness.save(InstallState::Restarting, 0, None);

    let status = initial_status(&harness.work_dir, TO, Channel::Stable);
    assert_eq!(status.to_heartbeat().state, UpdateState::Restarting);

    assert!(matches!(
        harness.recover(TO).await,
        StartupOutcome::RollingBack { .. }
    ));

    let status = initial_status(&harness.work_dir, FROM, Channel::Stable);
    assert_eq!(status.to_heartbeat().state, UpdateState::Idle);

    let outcome = harness.recover(FROM).await;
    assert!(matches!(outcome, StartupOutcome::RollbackReported { .. }));
    let status = startup_status(&outcome, Channel::Stable);
    assert_eq!(status.phase, Phase::RolledBack);
    assert_eq!(status.to_heartbeat().state, UpdateState::RolledBack);
}
