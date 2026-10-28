//! 等会话结束、安装、重启与调度。

use std::fs;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use serde::{Deserialize, Serialize};
use serde_json::json;
use tokio::sync::{mpsc, watch, Notify};

use crate::audit::{AuditEvent, AuditQueue};
use crate::clock::Clock;
use crate::config::{HostConfig, UpdateChannel};

use super::prepare::{
    write_atomic, PrepareContext, PrepareDeps, PrepareOutcome, Prepared, Preparer, Trigger,
};
use super::status::{Phase, UpdateStatus};
use super::types::Channel;

/// Platform-specific installation and process restart operations.
pub trait Installer: Send + Sync + 'static {
    fn install(&self, package: &Path, backup_dir: &Path) -> Result<(), String>;
    fn restart(&self) -> Result<(), String>;
    fn rollback(&self, backup_dir: &Path) -> Result<(), String>;
}

/// Session lifecycle operations needed by the update coordinator.
#[async_trait]
pub trait SessionGate: Send + Sync + 'static {
    fn active_sessions(&self) -> usize;
    async fn wait_idle(&self);
    async fn close_all_for_update(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum InstallState {
    Installing,
    Restarting,
    RolledBack,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct PendingInstall {
    pub from: String,
    pub to: String,
    pub backup_dir: PathBuf,
    pub state: InstallState,
    pub boot_attempts: u32,
    pub reason: Option<String>,
    pub started_at_us: u64,
}

impl PendingInstall {
    pub fn path(work_dir: &Path) -> PathBuf {
        work_dir.join("pending_install.json")
    }

    pub fn load(work_dir: &Path) -> io::Result<Option<Self>> {
        let path = Self::path(work_dir);
        let bytes = match fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
            Err(error) => return Err(error),
        };
        serde_json::from_slice(&bytes)
            .map(Some)
            .map_err(|error| io::Error::new(io::ErrorKind::InvalidData, error))
    }

    pub fn save(&self, work_dir: &Path) -> io::Result<()> {
        let bytes = serde_json::to_vec(self).map_err(io::Error::other)?;
        write_atomic(&Self::path(work_dir), &bytes)
    }

    pub fn remove(work_dir: &Path) -> io::Result<()> {
        match fs::remove_file(Self::path(work_dir)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum UpdateOutcome {
    Disabled,
    NoUpdate,
    Skipped { version: String },
    Ready { version: String },
    Restarting { version: String },
    Failed { code: &'static str, message: String },
}

pub struct UpdaterOptions {
    pub prepare: PrepareContext,
    pub config: watch::Receiver<Arc<HostConfig>>,
    pub initial_delay: Duration,
    pub check_interval: Duration,
    pub initial_status: Option<UpdateStatus>,
}

pub struct UpdaterParts {
    pub source: Arc<dyn super::prepare::ReleaseSource>,
    pub downloader: Arc<dyn super::prepare::Downloader>,
    pub verifier: Arc<dyn super::prepare::PackageVerifier>,
    pub installer: Arc<dyn Installer>,
    pub sessions: Arc<dyn SessionGate>,
    pub audit: Option<AuditQueue>,
    pub clock: Arc<dyn Clock>,
}

enum Command {
    Trigger(Trigger),
    InstallReady,
}

pub struct Updater {
    preparer: Preparer,
    config: watch::Receiver<Arc<HostConfig>>,
    status: Arc<watch::Sender<UpdateStatus>>,
    status_rx: watch::Receiver<UpdateStatus>,
    installer: Arc<dyn Installer>,
    sessions: Arc<dyn SessionGate>,
    audit: Option<AuditQueue>,
    clock: Arc<dyn Clock>,
    current_version: String,
    work_dir: PathBuf,
    initial_delay: Duration,
    check_interval: Duration,
    commands: mpsc::Sender<Command>,
    receiver: tokio::sync::Mutex<Option<mpsc::Receiver<Command>>>,
    ready: Mutex<Option<Prepared>>,
    install_now: Notify,
}

impl Updater {
    pub fn new(opts: UpdaterOptions, parts: UpdaterParts) -> Arc<Self> {
        let channel = update_channel(&opts.config.borrow().update.channel);
        let initial = opts
            .initial_status
            .unwrap_or_else(|| UpdateStatus::idle(channel.clone()));
        let (status_tx, status_rx) = watch::channel(initial);
        let status = Arc::new(status_tx);
        let current_version = opts.prepare.current_version.clone();
        let work_dir = opts.prepare.work_dir.clone();
        let preparer = Preparer::new(
            opts.prepare,
            PrepareDeps {
                source: parts.source,
                downloader: parts.downloader,
                verifier: parts.verifier,
                audit: parts.audit.clone(),
                clock: Arc::clone(&parts.clock),
                status: Arc::clone(&status),
            },
        );
        let (commands, receiver) = mpsc::channel(16);
        Arc::new(Self {
            preparer,
            config: opts.config,
            status,
            status_rx,
            installer: parts.installer,
            sessions: parts.sessions,
            audit: parts.audit,
            clock: parts.clock,
            current_version,
            work_dir,
            initial_delay: opts.initial_delay,
            check_interval: opts.check_interval,
            commands,
            receiver: tokio::sync::Mutex::new(Some(receiver)),
            ready: Mutex::new(None),
            install_now: Notify::new(),
        })
    }

    pub fn status(&self) -> watch::Receiver<UpdateStatus> {
        self.status.subscribe()
    }

    pub async fn handle(&self, trigger: Trigger) -> UpdateOutcome {
        let config = self.config.borrow().clone();
        let channel = update_channel(&config.update.channel);
        match self.preparer.prepare(&trigger, channel).await {
            PrepareOutcome::Disabled => UpdateOutcome::Disabled,
            PrepareOutcome::NoUpdate => UpdateOutcome::NoUpdate,
            PrepareOutcome::Skipped { version } => UpdateOutcome::Skipped { version },
            PrepareOutcome::Failed(reason) => UpdateOutcome::Failed {
                code: reason.code(),
                message: reason.message().to_owned(),
            },
            PrepareOutcome::Prepared(prepared) => {
                if !config.update.auto_install {
                    let version = prepared.manifest.version.clone();
                    self.set_status(UpdateStatus {
                        phase: Phase::Ready,
                        channel: prepared.manifest.channel.clone(),
                        pending: Some(version.clone()),
                        notes: Some(prepared.manifest.notes.clone()),
                        error: None,
                    });
                    if let Ok(mut ready) = self.ready.lock() {
                        *ready = Some(prepared);
                    }
                    UpdateOutcome::Ready { version }
                } else {
                    self.install(prepared).await
                }
            }
        }
    }

    /// Queue a trigger without waiting for the update operation.
    pub fn submit(&self, trigger: Trigger) {
        let status = self.status_rx.borrow().clone();
        let busy = matches!(
            status.phase,
            Phase::Checking
                | Phase::Downloading
                | Phase::Verifying
                | Phase::WaitingSessionEnd
                | Phase::Installing
                | Phase::Restarting
        );
        if busy {
            return;
        }
        let _ = self.commands.try_send(Command::Trigger(trigger));
    }

    pub fn install_now(&self) {
        let status = self.status_rx.borrow().clone();
        match status.phase {
            Phase::WaitingSessionEnd => self.install_now.notify_waiters(),
            Phase::Ready => {
                if self.commands.try_send(Command::InstallReady).is_err() {
                    tracing::debug!(
                        event = "upgrade.install",
                        result = "queue_full",
                        "ready install command was not queued"
                    );
                }
            }
            _ => tracing::debug!(
                phase = status.ipc_state(),
                "ignoring update.installNow outside an installable state"
            ),
        }
    }

    pub async fn run(self: Arc<Self>) {
        let mut receiver = match self.receiver.lock().await.take() {
            Some(receiver) => receiver,
            None => return,
        };
        let mut initial = Box::pin(tokio::time::sleep(self.initial_delay));
        loop {
            tokio::select! {
                command = receiver.recv() => {
                    if !self.handle_command(command).await { return; }
                }
                _ = &mut initial => {
                    let _ = self.handle(Trigger::Periodic).await;
                    initial.as_mut().reset(tokio::time::Instant::now() + self.check_interval);
                }
            }
        }
    }

    async fn handle_command(&self, command: Option<Command>) -> bool {
        match command {
            Some(Command::Trigger(trigger)) => {
                let _ = self.handle(trigger).await;
                true
            }
            Some(Command::InstallReady) => {
                let prepared = self.ready.lock().ok().and_then(|mut ready| ready.take());
                if let Some(prepared) = prepared {
                    let _ = self.install(prepared).await;
                }
                true
            }
            None => false,
        }
    }

    async fn install(&self, prepared: Prepared) -> UpdateOutcome {
        let version = prepared.manifest.version.clone();
        let notes = prepared.manifest.notes.clone();
        let channel = prepared.manifest.channel.clone();
        let mut notified = Box::pin(self.install_now.notified());
        notified.as_mut().enable();
        if self.sessions.active_sessions() > 0 {
            self.set_status(UpdateStatus {
                phase: Phase::WaitingSessionEnd,
                channel: channel.clone(),
                pending: Some(version.clone()),
                notes: Some(notes.clone()),
                error: None,
            });
            tokio::select! {
                _ = self.sessions.wait_idle() => {}
                _ = &mut notified => { self.sessions.close_all_for_update().await; }
            }
        }

        let backup_dir = self.work_dir.join("backup").join(&self.current_version);
        let pending = PendingInstall {
            from: self.current_version.clone(),
            to: version.clone(),
            backup_dir: backup_dir.clone(),
            state: InstallState::Installing,
            boot_attempts: 0,
            reason: None,
            started_at_us: self.clock.now_micros(),
        };
        if let Err(error) = pending.save(&self.work_dir) {
            return self.install_failure(
                &channel,
                &version,
                &notes,
                "install",
                format!("安装记录写入失败: {error}"),
                false,
            );
        }
        self.set_status(UpdateStatus {
            phase: Phase::Installing,
            channel: channel.clone(),
            pending: Some(version.clone()),
            notes: Some(notes.clone()),
            error: None,
        });
        tracing::info!(event = "upgrade.install", from = %self.current_version, to = %version, result = "started", "installing update");
        let installer = Arc::clone(&self.installer);
        let package = prepared.package.clone();
        let backup_for_install = backup_dir.clone();
        let install_result =
            tokio::task::spawn_blocking(move || installer.install(&package, &backup_for_install))
                .await;
        if !matches!(install_result, Ok(Ok(()))) {
            let _ = PendingInstall::remove(&self.work_dir);
            return self.install_failure(
                &channel,
                &version,
                &notes,
                "install",
                "安装失败".to_owned(),
                true,
            );
        }

        let restarting = PendingInstall {
            state: InstallState::Restarting,
            ..pending
        };
        if let Err(error) = restarting.save(&self.work_dir) {
            return self.install_failure(
                &channel,
                &version,
                &notes,
                "install",
                format!("安装记录写入失败: {error}"),
                false,
            );
        }
        self.set_status(UpdateStatus {
            phase: Phase::Restarting,
            channel,
            pending: Some(version.clone()),
            notes: Some(notes),
            error: None,
        });
        tracing::info!(event = "upgrade.restart", from = %self.current_version, to = %version, result = "started", "restarting agent");
        let installer = Arc::clone(&self.installer);
        let restart_result = tokio::task::spawn_blocking(move || installer.restart()).await;
        if !matches!(restart_result, Ok(Ok(()))) {
            let failed_status = self.status_rx.borrow().clone();
            return self.install_failure(
                &failed_status.channel,
                &version,
                failed_status.notes.as_deref().unwrap_or_default(),
                "restart",
                "重启失败".to_owned(),
                true,
            );
        }
        tracing::info!(event = "upgrade.restart", from = %self.current_version, to = %version, result = "ok", "agent restart requested");
        UpdateOutcome::Restarting { version }
    }

    fn install_failure(
        &self,
        channel: &Channel,
        version: &str,
        notes: &str,
        code: &'static str,
        message: String,
        audit: bool,
    ) -> UpdateOutcome {
        self.set_status(UpdateStatus {
            phase: Phase::Failed,
            channel: channel.clone(),
            pending: Some(version.to_owned()),
            notes: Some(notes.to_owned()),
            error: Some(message.clone()),
        });
        if audit {
            self.audit_failure(version, code);
        }
        if code == "restart" {
            tracing::warn!(event = "upgrade.restart", from = %self.current_version, to = %version, result = "failed", reason = code, "update operation failed");
        } else {
            tracing::warn!(event = "upgrade.install", from = %self.current_version, to = %version, result = "failed", reason = code, "update operation failed");
        }
        UpdateOutcome::Failed { code, message }
    }

    fn audit_failure(&self, version: &str, reason: &str) {
        let Some(queue) = &self.audit else {
            return;
        };
        let event = AuditEvent {
            ts: self.clock.now_micros(),
            user: None,
            device_id: None,
            host_id: None,
            server: None,
            event: "upgrade".to_owned(),
            path: None,
            duration_s: None,
            detail: Some(json!({
                "phase": "failed",
                "version": version,
                "reason": reason,
            })),
        };
        if let Err(error) = queue.push(event) {
            tracing::warn!(
                event = "upgrade.install",
                result = "audit_error",
                "could not queue upgrade audit: {error}"
            );
        }
    }

    fn set_status(&self, status: UpdateStatus) {
        self.status.send_replace(status);
    }
}

fn update_channel(channel: &UpdateChannel) -> Channel {
    match channel {
        UpdateChannel::Stable => Channel::Stable,
        UpdateChannel::Beta => Channel::Beta,
    }
}
