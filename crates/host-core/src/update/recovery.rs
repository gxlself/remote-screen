//! 新版本自检、回滚与上报。

use std::fs;
use std::path::Path;
use std::time::Duration;

use async_trait::async_trait;
use serde_json::{json, Value};
use tracing::{error, warn};

use crate::audit::{AuditEvent, AuditQueue};
use crate::clock::Clock;

use super::install::{InstallState, Installer, PendingInstall};
use super::prepare::add_skipped_version;
use super::status::{Phase, UpdateStatus};
use super::types::Channel;

pub const SELF_CHECK_TIMEOUT: Duration = Duration::from_secs(60);
/// The protocol does not prescribe a crash-loop threshold.
pub const MAX_BOOT_ATTEMPTS: u32 = 3;
const MAX_REASON_CHARS: usize = 200;

/// The checks a newly installed version must pass before an update is final.
#[async_trait]
pub trait SelfCheck: Send + Sync {
    /// Check that listening is ready and one frame can be captured (06 update
    /// step 7). Heartbeats during the check do not count toward completion.
    /// `Err` is a short English reason used only for logs and audit.
    async fn run(&self) -> Result<(), String>;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StartupOutcome {
    /// No pending-install record existed.
    Clean,
    /// The new version passed its self-check.
    Confirmed { from: String, to: String },
    /// Rollback and restart were requested; the real process should exit.
    RollingBack {
        from: String,
        to: String,
        reason: String,
    },
    RollbackFailed {
        from: String,
        to: String,
        error: String,
    },
    /// The old version started after a completed rollback and reported it.
    RollbackReported {
        from: String,
        to: String,
        reason: String,
    },
    /// The old version started while the install record never progressed.
    InstallFailed { from: String, to: String },
    /// The record was corrupt or did not match the running version.
    Stale,
}

/// Reconcile an install record left by the updater before starting it.
pub async fn recover_on_startup(
    work_dir: &Path,
    current_version: &str,
    installer: &dyn Installer,
    check: &dyn SelfCheck,
    audit: Option<&AuditQueue>,
    clock: &dyn Clock,
) -> StartupOutcome {
    let mut record = match PendingInstall::load(work_dir) {
        Ok(Some(record)) => record,
        Ok(None) => return StartupOutcome::Clean,
        Err(error) => {
            warn!(
                event = "upgrade.recovery",
                result = "invalid_record",
                "discarding an unreadable pending install record: {error}"
            );
            remove_record(work_dir);
            return StartupOutcome::Stale;
        }
    };

    if current_version == record.to {
        if record.state == InstallState::RolledBack {
            let reason = recorded_reason(record.reason.as_deref());
            return rollback(work_dir, &mut record, installer, audit, clock, reason).await;
        }

        record.boot_attempts = record.boot_attempts.saturating_add(1);
        if let Err(error) = record.save(work_dir) {
            warn!(
                event = "upgrade.recovery",
                result = "save_error",
                "could not persist the new boot-attempt count: {error}"
            );
        }
        if record.boot_attempts > MAX_BOOT_ATTEMPTS {
            return rollback(
                work_dir,
                &mut record,
                installer,
                audit,
                clock,
                "boot_loop".to_owned(),
            )
            .await;
        }

        match tokio::time::timeout(SELF_CHECK_TIMEOUT, check.run()).await {
            Ok(Ok(())) => {
                remove_dir_if_exists(&record.backup_dir);
                remove_dir_if_exists(&work_dir.join("download"));
                remove_record(work_dir);
                push_audit(
                    audit,
                    clock,
                    json!({
                        "phase": "installed",
                        "from": record.from,
                        "to": record.to,
                    }),
                );
                StartupOutcome::Confirmed {
                    from: record.from,
                    to: record.to,
                }
            }
            Ok(Err(reason)) => {
                let reason = truncate_chars(format!("self_check: {reason}"), MAX_REASON_CHARS);
                rollback(work_dir, &mut record, installer, audit, clock, reason).await
            }
            Err(_) => {
                rollback(
                    work_dir,
                    &mut record,
                    installer,
                    audit,
                    clock,
                    "self_check_timeout".to_owned(),
                )
                .await
            }
        }
    } else if current_version == record.from {
        match record.state {
            InstallState::RolledBack => {
                let reason = recorded_reason(record.reason.as_deref());
                push_audit(
                    audit,
                    clock,
                    json!({
                        "phase": "rollback",
                        "from": record.from,
                        "to": record.to,
                        "reason": reason,
                    }),
                );
                warn!(
                    event = "upgrade.rollback",
                    from = %record.from,
                    to = %record.to,
                    reason = %reason,
                    "reported rollback to the previous version"
                );
                remove_record(work_dir);
                StartupOutcome::RollbackReported {
                    from: record.from,
                    to: record.to,
                    reason,
                }
            }
            InstallState::Installing | InstallState::Restarting => {
                push_audit(
                    audit,
                    clock,
                    json!({
                        "phase": "failed",
                        "from": record.from,
                        "to": record.to,
                        "reason": "install_incomplete",
                    }),
                );
                warn!(
                    event = "upgrade.install",
                    from = %record.from,
                    to = %record.to,
                    result = "incomplete",
                    "the previous install did not complete"
                );
                remove_record(work_dir);
                StartupOutcome::InstallFailed {
                    from: record.from,
                    to: record.to,
                }
            }
        }
    } else {
        warn!(
            event = "upgrade.recovery",
            result = "stale",
            current = current_version,
            from = %record.from,
            to = %record.to,
            "discarding a pending install record for other versions"
        );
        remove_record(work_dir);
        StartupOutcome::Stale
    }
}

async fn rollback(
    work_dir: &Path,
    record: &mut PendingInstall,
    installer: &dyn Installer,
    audit: Option<&AuditQueue>,
    clock: &dyn Clock,
    reason: String,
) -> StartupOutcome {
    if let Err(error) = add_skipped_version(work_dir, &record.to) {
        warn!(
            event = "upgrade.recovery",
            result = "skip_save_error",
            to = %record.to,
            "could not persist the skipped version: {error}"
        );
    }

    record.state = InstallState::RolledBack;
    record.reason = Some(reason.clone());
    if let Err(error) = record.save(work_dir) {
        warn!(
            event = "upgrade.recovery",
            result = "save_error",
            "could not persist the rollback record: {error}"
        );
    }

    if let Err(error) = installer.rollback(&record.backup_dir) {
        push_audit(
            audit,
            clock,
            json!({
                "phase": "rollback_failed",
                "from": record.from,
                "to": record.to,
                "error": error,
            }),
        );
        error!(
            event = "upgrade.rollback",
            from = %record.from,
            to = %record.to,
            reason = %reason,
            error = %error,
            "rollback failed"
        );
        return StartupOutcome::RollbackFailed {
            from: record.from.clone(),
            to: record.to.clone(),
            error,
        };
    }

    if let Err(error) = installer.restart() {
        error!(
            event = "upgrade.restart",
            from = %record.from,
            to = %record.to,
            error = %error,
            "rollback completed but restart failed"
        );
    }

    StartupOutcome::RollingBack {
        from: record.from.clone(),
        to: record.to.clone(),
        reason,
    }
}

/// Status while a newly started version is running its self-check.
pub fn self_check_status(to: &str, channel: Channel) -> UpdateStatus {
    UpdateStatus {
        phase: Phase::Restarting,
        channel,
        pending: Some(to.to_owned()),
        notes: None,
        error: None,
    }
}

/// Initial status for the heartbeat before `recover_on_startup` runs.
///
/// A pending record for the running version means the new version is
/// self-checking, so the heartbeat must not report `ready` yet. Missing,
/// unreadable, mismatched, and already rolled-back records are idle.
pub fn initial_status(work_dir: &Path, current_version: &str, channel: Channel) -> UpdateStatus {
    match PendingInstall::load(work_dir) {
        Ok(Some(record))
            if record.to == current_version && record.state != InstallState::RolledBack =>
        {
            self_check_status(&record.to, channel)
        }
        Ok(Some(_)) | Ok(None) | Err(_) => UpdateStatus::idle(channel),
    }
}

/// Convert startup reconciliation into the first heartbeat status.
pub fn startup_status(outcome: &StartupOutcome, channel: Channel) -> UpdateStatus {
    let failed = |pending: &str, error: &str| UpdateStatus {
        phase: Phase::Failed,
        channel: channel.clone(),
        pending: Some(pending.to_owned()),
        notes: None,
        error: Some(error.to_owned()),
    };

    match outcome {
        StartupOutcome::Confirmed { .. } => UpdateStatus {
            phase: Phase::Updated,
            channel: channel.clone(),
            pending: None,
            notes: None,
            error: None,
        },
        StartupOutcome::RollbackReported { to, .. } => UpdateStatus {
            phase: Phase::RolledBack,
            channel: channel.clone(),
            pending: Some(to.clone()),
            notes: None,
            error: Some("新版本自检失败，已回滚".to_owned()),
        },
        StartupOutcome::InstallFailed { to, .. } => failed(to, "安装未完成"),
        StartupOutcome::RollbackFailed { to, .. } => failed(to, "回滚失败"),
        StartupOutcome::Clean | StartupOutcome::RollingBack { .. } | StartupOutcome::Stale => {
            UpdateStatus::idle(channel)
        }
    }
}

fn recorded_reason(reason: Option<&str>) -> String {
    reason
        .filter(|reason| !reason.is_empty())
        .unwrap_or("unknown")
        .to_owned()
}

fn truncate_chars(value: String, max_chars: usize) -> String {
    if value.chars().count() <= max_chars {
        value
    } else {
        value.chars().take(max_chars).collect()
    }
}

fn remove_record(work_dir: &Path) {
    if let Err(error) = PendingInstall::remove(work_dir) {
        warn!(
            event = "upgrade.recovery",
            result = "remove_error",
            "could not remove the pending install record: {error}"
        );
    }
}

fn remove_dir_if_exists(path: &Path) {
    match fs::remove_dir_all(path) {
        Ok(()) => {}
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => warn!(
            event = "upgrade.recovery",
            result = "cleanup_error",
            "could not remove an update directory: {error}"
        ),
    }
}

fn push_audit(audit: Option<&AuditQueue>, clock: &dyn Clock, detail: Value) {
    let Some(audit) = audit else {
        return;
    };
    let event = AuditEvent {
        ts: clock.now_micros(),
        user: None,
        device_id: None,
        host_id: None,
        server: None,
        event: "upgrade".to_owned(),
        path: None,
        duration_s: None,
        detail: Some(detail),
    };
    if let Err(error) = audit.push(event) {
        warn!(
            event = "upgrade.recovery",
            result = "audit_error",
            "could not queue the upgrade audit event: {error}"
        );
    }
}
