//! 更新状态与 ack 规则。

use std::sync::Arc;

use tokio::sync::watch;

use super::types::{Channel, HeartbeatUpdate, UpdateState};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Phase {
    Idle,
    Checking,
    Downloading,
    Verifying,
    Ready,
    WaitingSessionEnd,
    Installing,
    Restarting,
    /// A newly installed version started and passed its self-check.
    Updated,
    /// A newly installed version failed its self-check and was rolled back.
    RolledBack,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct UpdateStatus {
    pub phase: Phase,
    pub channel: Channel,
    /// The version currently being handled, or the version ready to install.
    pub pending: Option<String>,
    /// Release notes from the verified manifest.
    pub notes: Option<String>,
    /// A Chinese user-facing reason while `phase` is [`Phase::Failed`] or
    /// [`Phase::RolledBack`].
    pub error: Option<String>,
}

impl UpdateStatus {
    pub fn idle(channel: Channel) -> Self {
        Self {
            phase: Phase::Idle,
            channel,
            pending: None,
            notes: None,
            error: None,
        }
    }

    /// Map to the coarse update state. `Ready` means installation
    /// preparation finished and is reported as `idle` while retaining
    /// `pending`; only `Updated` (post-restart self-check passed) is `ready`.
    pub fn to_heartbeat(&self) -> HeartbeatUpdate {
        let state = match self.phase {
            Phase::Idle | Phase::Checking | Phase::Ready => UpdateState::Idle,
            Phase::Downloading => UpdateState::Downloading,
            Phase::Verifying => UpdateState::Verifying,
            Phase::WaitingSessionEnd => UpdateState::WaitingSessionEnd,
            Phase::Installing => UpdateState::Installing,
            Phase::Restarting => UpdateState::Restarting,
            Phase::Updated => UpdateState::Ready,
            Phase::RolledBack => UpdateState::RolledBack,
            Phase::Failed => UpdateState::Failed,
        };
        HeartbeatUpdate {
            channel: self.channel.clone(),
            pending: self.pending.clone(),
            state,
        }
    }

    /// State string used by the local IPC `update.status` result.
    pub fn ipc_state(&self) -> &'static str {
        match self.phase {
            Phase::Idle => "idle",
            Phase::Checking => "checking",
            Phase::Downloading => "downloading",
            Phase::Verifying => "verifying",
            Phase::Ready => "ready",
            Phase::WaitingSessionEnd => "waiting_session_end",
            Phase::Installing => "installing",
            Phase::Restarting => "restarting",
            Phase::Updated => "idle",
            Phase::RolledBack => "failed",
            Phase::Failed => "failed",
        }
    }
}

pub type StatusTx = Arc<watch::Sender<UpdateStatus>>;
