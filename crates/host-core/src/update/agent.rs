//! Host-side assembly for startup recovery and automatic updates.

use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use serde_json::json;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tracing::{info, warn};

use crate::clock::Clock;
use crate::config::UpdateChannel;
use crate::ipc::{Event, UpdateStatusResult};
use crate::owner::{host_arch, host_os};
use crate::platform::{CaptureConfig, Platform};
use crate::session::{
    handlers::roster,
    manager::{EndReason, SessionManager},
};

use super::install::{Installer, SessionGate, Updater, UpdaterOptions, UpdaterParts};
use super::manifest::TrustedKeys;
use super::prepare::{
    Downloader, PackageVerifier, PrepareContext, ReleaseSource, DEFAULT_MAX_PACKAGE_BYTES,
};
use super::recovery::{
    initial_status, recover_on_startup, startup_status, SelfCheck, StartupOutcome,
};
use super::status::UpdateStatus;
use super::types::{Channel, HeartbeatUpdate};

pub const DEFAULT_INITIAL_DELAY: Duration = Duration::from_secs(5 * 60);
pub const DEFAULT_CHECK_INTERVAL: Duration = Duration::from_secs(6 * 60 * 60);
pub const SELF_CHECK_LISTEN_WAIT: Duration = Duration::from_secs(40);
pub const SELF_CHECK_FRAME_WAIT: Duration = Duration::from_secs(15);

pub struct UpdatePlatform {
    pub installer: Arc<dyn Installer>,
    pub verifier: Arc<dyn PackageVerifier>,
    pub downloader: Arc<dyn Downloader>,
}

pub struct UpdateWiring {
    pub work_dir: PathBuf,
    pub current_version: String,
    pub keys: TrustedKeys,
    pub platform: UpdatePlatform,
    pub source: Option<Arc<dyn ReleaseSource>>,
    pub listening: watch::Receiver<bool>,
    pub audit: Option<crate::audit::AuditQueue>,
    pub initial_delay: Duration,
    pub check_interval: Duration,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpdateUnavailable {
    Disabled,
    Starting,
}

impl UpdateUnavailable {
    pub fn code(self) -> &'static str {
        match self {
            Self::Disabled => "unsupported",
            Self::Starting => "not_ready",
        }
    }

    pub fn message(self) -> &'static str {
        match self {
            Self::Disabled => "本机未启用自动更新",
            Self::Starting => "更新服务正在启动，请稍后重试",
        }
    }
}

pub struct UpdateService {
    status_tx: Arc<watch::Sender<UpdateStatus>>,
    outcome: Mutex<Option<StartupOutcome>>,
    recovered: watch::Sender<Option<StartupOutcome>>,
    updater: OnceLock<Arc<Updater>>,
    tasks: Mutex<Vec<JoinHandle<()>>>,
    stopped: AtomicBool,
}

impl UpdateService {
    fn new(status_tx: Arc<watch::Sender<UpdateStatus>>) -> Self {
        let (recovered, _) = watch::channel(None);
        Self {
            status_tx,
            outcome: Mutex::new(None),
            recovered,
            updater: OnceLock::new(),
            tasks: Mutex::new(Vec::new()),
            stopped: AtomicBool::new(false),
        }
    }

    fn set_outcome(&self, outcome: StartupOutcome) {
        let recovered = outcome.clone();
        *self
            .outcome
            .lock()
            .unwrap_or_else(|poison| poison.into_inner()) = Some(outcome);
        self.recovered.send_replace(Some(recovered));
    }

    fn set_updater(&self, updater: Arc<Updater>) {
        let _ = self.updater.set(updater);
    }

    fn add_task(&self, task: JoinHandle<()>) {
        let mut tasks = self
            .tasks
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        if self.stopped.load(Ordering::Acquire) {
            task.abort();
            return;
        }
        tasks.push(task);
    }

    fn disable_duplicate(&self) {
        let channel = self.status_tx.borrow().channel.clone();
        self.status_tx.send_replace(UpdateStatus::idle(channel));
        self.set_outcome(StartupOutcome::Clean);
    }

    pub fn status(&self) -> UpdateStatus {
        self.status_tx.borrow().clone()
    }

    pub fn subscribe(&self) -> watch::Receiver<UpdateStatus> {
        self.status_tx.subscribe()
    }

    pub fn ipc_status(&self) -> UpdateStatusResult {
        let status = self.status();
        UpdateStatusResult {
            state: status.ipc_state().to_owned(),
            version: status.pending,
            notes: status.notes,
        }
    }

    pub fn heartbeat(&self) -> HeartbeatUpdate {
        self.status().to_heartbeat()
    }

    pub fn install_now(&self) -> Result<(), UpdateUnavailable> {
        if let Some(updater) = self.updater.get() {
            updater.install_now();
            return Ok(());
        }
        if self.outcome().is_none() {
            Err(UpdateUnavailable::Starting)
        } else {
            Err(UpdateUnavailable::Disabled)
        }
    }

    pub fn outcome(&self) -> Option<StartupOutcome> {
        self.outcome
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .clone()
    }

    /// Wait until startup recovery, including any rollback operation, has
    /// completed and return its outcome.
    pub async fn wait_outcome(&self) -> StartupOutcome {
        let mut recovered = self.recovered.subscribe();
        loop {
            if let Some(outcome) = recovered.borrow().clone() {
                return outcome;
            }
            if recovered.changed().await.is_err() {
                // The sender is owned by this service and cannot normally
                // close; retain the contract if it is ever changed.
                continue;
            }
        }
    }

    pub async fn recovered(&self) -> StartupOutcome {
        self.wait_outcome().await
    }

    pub fn shutdown(&self) {
        self.stopped.store(true, Ordering::Release);
        let tasks = std::mem::take(
            &mut *self
                .tasks
                .lock()
                .unwrap_or_else(|poison| poison.into_inner()),
        );
        for task in tasks {
            task.abort();
        }
    }
}

/// Attach the update coordinator to a manager. The function only schedules
/// asynchronous work; startup recovery is awaited by [`UpdateService::wait_outcome`].
pub fn attach<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    wiring: UpdateWiring,
) -> Arc<UpdateService> {
    let channel = update_channel(&mgr.services().config.get().update.channel);
    let initial = initial_status(&wiring.work_dir, &wiring.current_version, channel.clone());
    let status_tx = Arc::new(watch::channel(initial).0);
    let service = Arc::new(UpdateService::new(status_tx.clone()));
    if !mgr.attach_updater(Arc::clone(&service)) {
        tracing::error!("updater already attached");
        service.disable_duplicate();
        return service;
    }

    let weak = Arc::downgrade(mgr);
    let clock = mgr.services().clock.clone();
    let config = mgr.services().config.subscribe();
    let task_service = Arc::clone(&service);
    let task = tokio::spawn(async move {
        run_agent(
            weak,
            task_service,
            wiring,
            channel,
            status_tx,
            clock,
            config,
        )
        .await;
    });
    service.add_task(task);
    service
}

fn update_channel(channel: &UpdateChannel) -> Channel {
    match channel {
        UpdateChannel::Stable => Channel::Stable,
        UpdateChannel::Beta => Channel::Beta,
    }
}

async fn run_agent<P: Platform>(
    weak: Weak<SessionManager<P>>,
    service: Arc<UpdateService>,
    wiring: UpdateWiring,
    channel: Channel,
    status_tx: Arc<watch::Sender<UpdateStatus>>,
    clock: Arc<dyn Clock>,
    config: watch::Receiver<Arc<crate::config::HostConfig>>,
) {
    let check = AgentSelfCheck {
        mgr: weak.clone(),
        listening: wiring.listening,
    };
    let outcome = recover_on_startup(
        &wiring.work_dir,
        &wiring.current_version,
        &*wiring.platform.installer,
        &check,
        wiring.audit.as_ref(),
        &*clock,
    )
    .await;
    service.set_outcome(outcome.clone());

    let startup = startup_status(&outcome, channel.clone());
    status_tx.send_replace(startup.clone());
    emit_update(&weak, &startup);

    if matches!(
        outcome,
        StartupOutcome::RollingBack { .. } | StartupOutcome::RollbackFailed { .. }
    ) {
        warn!("update recovery requested rollback; automatic updates disabled");
        return;
    }
    if wiring.keys.is_empty() {
        info!("automatic updates disabled: no trusted release keys");
        return;
    }
    let Some(source) = wiring.source else {
        info!("automatic updates disabled: no release source");
        return;
    };

    let sessions: Arc<dyn SessionGate> = Arc::new(MgrGate { mgr: weak.clone() });
    let updater = Updater::new(
        UpdaterOptions {
            prepare: PrepareContext {
                current_version: wiring.current_version.clone(),
                os: host_os(),
                arch: host_arch(),
                work_dir: wiring.work_dir.clone(),
                keys: wiring.keys,
                max_package_bytes: DEFAULT_MAX_PACKAGE_BYTES,
            },
            config,
            initial_delay: wiring.initial_delay,
            check_interval: wiring.check_interval,
            initial_status: Some(status_tx.borrow().clone()),
        },
        UpdaterParts {
            source,
            downloader: wiring.platform.downloader,
            verifier: wiring.platform.verifier,
            installer: wiring.platform.installer,
            sessions,
            audit: wiring.audit,
            clock,
        },
    );
    service.set_updater(Arc::clone(&updater));

    let updater_status = updater.status();
    let run = tokio::spawn(Arc::clone(&updater).run());
    service.add_task(run);
    let forward_service = Arc::clone(&service);
    let forward_weak = weak.clone();
    let forward = tokio::spawn(async move {
        let mut rx = updater_status;
        let status = rx.borrow().clone();
        let phase_changed = forward_service.status().phase != status.phase;
        forward_service.status_tx.send_replace(status.clone());
        if phase_changed {
            emit_update(&forward_weak, &status);
        }
        loop {
            if rx.changed().await.is_err() {
                break;
            }
            let status = rx.borrow().clone();
            let phase_changed = forward_service.status().phase != status.phase;
            forward_service.status_tx.send_replace(status.clone());
            if phase_changed {
                emit_update(&forward_weak, &status);
            }
        }
    });
    service.add_task(forward);
}

fn emit_update<P: Platform>(mgr: &Weak<SessionManager<P>>, status: &UpdateStatus) {
    let Some(mgr) = mgr.upgrade() else {
        return;
    };
    let _ = mgr.emit(Event::UpdateState(json!({
        "state": status.ipc_state(),
        "version": status.pending,
        "notes": status.notes,
    })));
}

/// Session lifecycle adapter for the host update coordinator.
pub struct MgrGate<P: Platform> {
    mgr: Weak<SessionManager<P>>,
}

impl<P: Platform> MgrGate<P> {
    pub fn new(mgr: &Arc<SessionManager<P>>) -> Self {
        Self {
            mgr: Arc::downgrade(mgr),
        }
    }
}

#[async_trait]
impl<P: Platform> SessionGate for MgrGate<P> {
    fn active_sessions(&self) -> usize {
        self.mgr
            .upgrade()
            .map_or(0, |mgr| roster::active_sessions(&mgr))
    }

    async fn wait_idle(&self) {
        loop {
            if self.active_sessions() == 0 {
                return;
            }
            let Some(mgr) = self.mgr.upgrade() else {
                return;
            };
            let clock = mgr.services().clock.clone();
            clock.sleep(Duration::from_secs(1)).await;
        }
    }

    async fn close_all_for_update(&self) {
        if let Some(mgr) = self.mgr.upgrade() {
            let _ = mgr.end_where(|_| true, EndReason::HostUpdating).await;
        }
    }
}

struct AgentSelfCheck<P: Platform> {
    mgr: Weak<SessionManager<P>>,
    listening: watch::Receiver<bool>,
}

#[async_trait]
impl<P: Platform> SelfCheck for AgentSelfCheck<P> {
    async fn run(&self) -> Result<(), String> {
        let mut listening = self.listening.clone();
        let listening_ready = async {
            loop {
                if *listening.borrow() {
                    return true;
                }
                if listening.changed().await.is_err() {
                    return false;
                }
            }
        };
        if !tokio::time::timeout(SELF_CHECK_LISTEN_WAIT, listening_ready)
            .await
            .unwrap_or(false)
        {
            return Err("监听未就绪".to_owned());
        }

        let deadline = tokio::time::Instant::now() + SELF_CHECK_FRAME_WAIT;
        loop {
            if tokio::time::Instant::now() >= deadline {
                return Err("未能采集到画面".to_owned());
            }
            let Some(mgr) = self.mgr.upgrade() else {
                return Err("进程正在退出".to_owned());
            };
            let displays = mgr.platform().displays();
            if let Some(display) = displays.first() {
                let display_id = display.id;
                let (tx, mut rx) = mpsc::channel(2);
                let platform = Arc::clone(mgr.platform());
                let result = tokio::task::spawn_blocking(move || {
                    platform.capture(
                        display_id,
                        CaptureConfig {
                            fps: 5,
                            max_width: 640,
                            max_height: 360,
                        },
                        tx,
                    )
                })
                .await;
                if let Ok(Ok(handle)) = result {
                    let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
                    if matches!(
                        tokio::time::timeout(remaining, rx.recv()).await,
                        Ok(Some(_))
                    ) {
                        drop(handle);
                        return Ok(());
                    }
                    drop(handle);
                }
            }
            let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
            if remaining.is_zero() {
                return Err("未能采集到画面".to_owned());
            }
            tokio::time::sleep(Duration::from_secs(1).min(remaining)).await;
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[tokio::test]
    async fn wait_outcome_returns_after_startup_recovery() {
        let status_tx = Arc::new(watch::channel(UpdateStatus::idle(Channel::Stable)).0);
        let service = Arc::new(UpdateService::new(status_tx));
        let waiting = Arc::clone(&service);
        let task = tokio::spawn(async move { waiting.wait_outcome().await });

        tokio::task::yield_now().await;
        service.set_outcome(StartupOutcome::Clean);

        assert_eq!(task.await.unwrap(), StartupOutcome::Clean);
    }
}
