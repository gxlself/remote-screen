//! 本机操作检测策略与输入闸门。
//!
//! 平台层的 [`crate::platform::Platform::watch_local_activity`] 流采用脉冲
//! 语义：每个 `true` 表示此刻检测到一次本机输入；`false` 不是状态通知，
//! 一律忽略。暂停恢复只取决于最后一次 `true` 之后的静默时长，静默满
//! [`RESUME_AFTER_US`] 后恢复。Mac 平台层（HM2）按此语义实现。
//!
//! 本模块假定平台层已经滤除带 `0x5253` 标记的注入事件，因此这里收到的
//! `true` 都代表真实的本地操作。

use std::sync::{Arc, Mutex};
use std::time::Duration;

use serde_json::json;
use tokio::sync::{mpsc, watch};
use tracing::trace;

use crate::clock::Clock;
use crate::ipc::{self, SessionRole};

/// 最后一次本机输入后，远端输入暂停的恢复静默时长。
pub const RESUME_AFTER_US: u64 = 3_000_000;
/// 鉴权时判断「电脑前有人」的回看窗口。
pub const RECENT_WINDOW_US: u64 = 60_000_000;

/// 本机操作策略产生的状态转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActivityTransition {
    Paused,
    Resumed,
}

/// 由本机输入脉冲驱动的暂停状态机。
#[derive(Debug, Clone)]
pub struct ActivityPolicy {
    pause_enabled: bool,
    paused: bool,
    last_input_us: Option<u64>,
}

impl ActivityPolicy {
    pub fn new(pause_enabled: bool) -> Self {
        Self {
            pause_enabled,
            paused: false,
            last_input_us: None,
        }
    }

    /// 记录一次本机输入。暂停未启用或已经暂停时不产生状态转换。
    pub fn on_local_input(&mut self, now_us: u64) -> Option<ActivityTransition> {
        self.last_input_us = Some(now_us);
        if self.pause_enabled && !self.paused {
            self.paused = true;
            Some(ActivityTransition::Paused)
        } else {
            None
        }
    }

    /// 在静默满 [`RESUME_AFTER_US`] 后恢复远端输入。
    pub fn poll(&mut self, now_us: u64) -> Option<ActivityTransition> {
        if self
            .last_input_us
            .is_some_and(|last| now_us.saturating_sub(last) >= RESUME_AFTER_US)
        {
            self.resume()
        } else {
            None
        }
    }

    /// 暂停中返回恢复时刻，否则返回 `None`。
    pub fn next_deadline(&self) -> Option<u64> {
        if self.paused {
            self.last_input_us
                .map(|last| last.saturating_add(RESUME_AFTER_US))
        } else {
            None
        }
    }

    pub fn is_paused(&self) -> bool {
        self.paused
    }

    /// 判断最近一次本机输入是否严格落在回看窗口内。
    pub fn active_within(&self, now_us: u64, window_us: u64) -> bool {
        self.last_input_us
            .is_some_and(|last| now_us.saturating_sub(last) < window_us)
    }

    /// 更新「本机操作时暂停」配置。暂停中关闭配置会立即恢复。
    pub fn set_pause_enabled(&mut self, enabled: bool, _now_us: u64) -> Option<ActivityTransition> {
        self.pause_enabled = enabled;
        if enabled {
            None
        } else {
            self.resume()
        }
    }

    fn resume(&mut self) -> Option<ActivityTransition> {
        if self.paused {
            self.paused = false;
            Some(ActivityTransition::Resumed)
        } else {
            None
        }
    }
}

/// 鉴权与输入路由共享的本机操作状态来源。
pub trait LocalActivitySource: Send + Sync {
    fn local_activity_within_60s(&self) -> bool;
    fn input_paused_local(&self) -> bool;
}

/// 可从任意线程查询和更新配置的本机操作句柄。
#[derive(Clone)]
pub struct ActivityHandle {
    policy: Arc<Mutex<ActivityPolicy>>,
    clock: Arc<dyn Clock>,
    paused_tx: watch::Sender<bool>,
}

/// 启动平台输入脉冲的异步跟踪任务。
pub fn spawn_tracker(
    rx: mpsc::Receiver<bool>,
    pause_enabled: bool,
    clock: Arc<dyn Clock>,
) -> (ActivityHandle, tokio::task::JoinHandle<()>) {
    let policy = Arc::new(Mutex::new(ActivityPolicy::new(pause_enabled)));
    let (paused_tx, _) = watch::channel(false);
    let handle = ActivityHandle {
        policy,
        clock,
        paused_tx,
    };
    let task_handle = handle.clone();
    let task = tokio::spawn(async move { tracker_loop(rx, task_handle).await });
    (handle, task)
}

impl ActivityHandle {
    pub fn is_paused(&self) -> bool {
        self.policy().is_paused()
    }

    /// 订阅暂停状态；只在状态转换时发布新值。
    pub fn subscribe(&self) -> watch::Receiver<bool> {
        self.paused_tx.subscribe()
    }

    /// 配置热更新。暂停中关闭配置会立即发布恢复状态。
    pub fn set_pause_enabled(&self, enabled: bool) {
        let now_us = self.clock.now_micros();
        self.transition(|policy| policy.set_pause_enabled(enabled, now_us));
    }

    pub fn active_within_60s(&self) -> bool {
        let now_us = self.clock.now_micros();
        self.policy().active_within(now_us, RECENT_WINDOW_US)
    }

    fn policy(&self) -> std::sync::MutexGuard<'_, ActivityPolicy> {
        self.policy
            .lock()
            .unwrap_or_else(std::sync::PoisonError::into_inner)
    }

    /// Updates the policy and publishes its transition in the same short
    /// critical section, so subscribers cannot observe an older state after a
    /// newer one. The caller must not await while the lock is held.
    fn transition(&self, update: impl FnOnce(&mut ActivityPolicy) -> Option<ActivityTransition>) {
        let mut policy = self.policy();
        if let Some(transition) = update(&mut policy) {
            self.paused_tx
                .send_replace(transition == ActivityTransition::Paused);
        }
    }
}

impl LocalActivitySource for ActivityHandle {
    fn local_activity_within_60s(&self) -> bool {
        self.active_within_60s()
    }

    fn input_paused_local(&self) -> bool {
        self.is_paused()
    }
}

async fn tracker_loop(mut rx: mpsc::Receiver<bool>, handle: ActivityHandle) {
    loop {
        let deadline = handle.policy().next_deadline();
        tokio::select! {
            biased;
            event = rx.recv() => {
                match event {
                    Some(true) => {
                        let now_us = handle.clock.now_micros();
                        handle.transition(|policy| policy.on_local_input(now_us));
                    }
                    Some(false) => trace!("ignoring local activity false pulse"),
                    None => {
                        handle.transition(ActivityPolicy::resume);
                        return;
                    }
                }
            }
            () = wait_for_deadline(deadline, Arc::clone(&handle.clock)) => {
                let now_us = handle.clock.now_micros();
                handle.transition(|policy| policy.poll(now_us));
            }
        }
    }
}

async fn wait_for_deadline(deadline: Option<u64>, clock: Arc<dyn Clock>) {
    match deadline {
        Some(deadline) => {
            let remaining_us = deadline.saturating_sub(clock.now_micros());
            tokio::time::sleep(Duration::from_micros(remaining_us)).await;
        }
        None => std::future::pending().await,
    }
}

/// 远端输入闸门的状态转换。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateChange {
    Unchanged,
    Opened,
    Closed,
}

/// 合并控制者身份、本机操作暂停和本机确认弹窗的输入闸门。
#[derive(Debug, Default)]
pub struct InputGate {
    open: bool,
}

impl InputGate {
    pub fn update(
        &mut self,
        is_controller: bool,
        input_paused_local: bool,
        prompt_open: bool,
    ) -> GateChange {
        let open = is_controller && !input_paused_local && !prompt_open;
        match (self.open, open) {
            (false, true) => {
                self.open = true;
                GateChange::Opened
            }
            (true, false) => {
                self.open = false;
                GateChange::Closed
            }
            _ => GateChange::Unchanged,
        }
    }

    pub fn allowed(&self) -> bool {
        self.open
    }
}

/// 构造发给参与者的本机操作通知。
pub fn local_activity_msg(paused: bool) -> rsp_core::pb::LocalActivity {
    rsp_core::pb::LocalActivity { active: paused }
}

/// 构造 host-ui 的 `event.sessionsChanged` 载荷。
pub fn sessions_changed_event(
    sessions: &[ipc::SessionSummary],
    input_paused_local: bool,
) -> ipc::Event {
    let controller = sessions
        .iter()
        .find(|session| session.role == SessionRole::Controller)
        .map(|session| {
            json!({
                "session_id": session.session_id,
                "device_id": session.device_id,
                "device_name": session.device_name,
                "connected_at_us": session.connected_at_us,
            })
        });
    let viewer_count = sessions
        .iter()
        .filter(|session| session.role == SessionRole::Viewer)
        .count();

    ipc::Event::SessionsChanged(json!({
        "sessions": sessions,
        "capsule": {
            "visible": !sessions.is_empty(),
            "controller": controller,
            "viewer_count": viewer_count,
            "input_paused_local": input_paused_local,
        }
    }))
}
