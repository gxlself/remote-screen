//! 空闲超时与结束后锁屏策略。

use std::collections::HashMap;

use rsp_core::errors::ErrorCode;
use rsp_core::pb;

use crate::platform::{PlatformError, SessionActionKind};

/// 距离空闲截止点多久发送提醒。
pub const IDLE_WARNING_US: u64 = 60_000_000;

const MINUTE_US: u64 = 60_000_000;
const SECOND_US: u64 = 1_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum IdleEvent {
    Warn { seconds_left: u32 },
    Expire,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IdleTimer {
    timeout_min: u16,
    last_input_us: u64,
    warned: bool,
    expired: bool,
}

impl IdleTimer {
    pub fn new(idle_timeout_min: u16, now_us: u64) -> Self {
        Self {
            timeout_min: idle_timeout_min,
            last_input_us: now_us,
            warned: false,
            expired: false,
        }
    }

    pub fn set_timeout_min(&mut self, min: u16, now_us: u64) {
        let old_deadline = self.deadline();
        self.timeout_min = min;
        let new_deadline = self.deadline();

        let deadline_extended = new_deadline
            .is_some_and(|new| new > now_us && old_deadline.is_none_or(|old| new > old));
        if deadline_extended {
            self.warned = false;
        }
    }

    pub fn on_input(&mut self, now_us: u64) {
        if self.expired {
            return;
        }
        self.last_input_us = now_us;
        self.warned = false;
    }

    pub fn poll(&mut self, now_us: u64) -> Option<IdleEvent> {
        if self.expired {
            return None;
        }
        let deadline = self.deadline()?;
        if now_us >= deadline {
            self.expired = true;
            return Some(IdleEvent::Expire);
        }

        let warn_at = deadline.saturating_sub(IDLE_WARNING_US);
        if !self.warned && now_us >= warn_at {
            self.warned = true;
            let remaining_us = deadline - now_us;
            let seconds_left = remaining_us.div_ceil(SECOND_US).min(u64::from(u32::MAX)) as u32;
            return Some(IdleEvent::Warn {
                seconds_left: seconds_left.min(60),
            });
        }
        None
    }

    pub fn next_deadline(&self) -> Option<u64> {
        if self.expired {
            return None;
        }
        let deadline = self.deadline()?;
        if self.warned {
            Some(deadline)
        } else {
            Some(
                deadline
                    .saturating_sub(IDLE_WARNING_US)
                    .max(self.last_input_us),
            )
        }
    }

    pub fn deadline(&self) -> Option<u64> {
        if self.timeout_min == 0 {
            None
        } else {
            Some(
                self.last_input_us
                    .saturating_add(u64::from(self.timeout_min) * MINUTE_US),
            )
        }
    }
}

pub fn idle_warning_msg(seconds_left: u32, lock_on_end: bool) -> pb::IdleWarning {
    pb::IdleWarning {
        seconds_left,
        lock_on_end,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LockOnEnd {
    config_default: bool,
    session_override: Option<bool>,
}

impl LockOnEnd {
    pub fn new(config_default: bool) -> Self {
        Self {
            config_default,
            session_override: None,
        }
    }

    pub fn set_config_default(&mut self, v: bool) {
        self.config_default = v;
    }

    pub fn set_override(&mut self, enable: bool) {
        self.session_override = Some(enable);
    }

    pub fn clear_override(&mut self) {
        self.session_override = None;
    }

    pub fn enabled(&self) -> bool {
        self.session_override.unwrap_or(self.config_default)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    UserClosed,
    IdleTimeout,
    LingerExpired,
    Revoked,
    Replaced,
    Handoff,
    ClosedByLocalUser,
    HostShutdown,
}

pub fn should_lock_on_end(lock: &LockOnEnd, reason: EndReason, was_controller: bool) -> bool {
    was_controller
        && lock.enabled()
        && matches!(
            reason,
            EndReason::UserClosed
                | EndReason::IdleTimeout
                | EndReason::LingerExpired
                | EndReason::Revoked
        )
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionReject {
    NotController,
    Unsupported,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ActionDecision {
    Platform(SessionActionKind),
    SetLockOnEnd(bool),
    Reject(ActionReject),
}

pub fn decide_session_action(is_controller: bool, msg: &pb::SessionAction) -> ActionDecision {
    if !is_controller {
        return ActionDecision::Reject(ActionReject::NotController);
    }

    let Ok(kind) = SessionActionKind::try_from(msg.kind) else {
        return ActionDecision::Reject(ActionReject::Unsupported);
    };
    match kind {
        SessionActionKind::LockScreen
        | SessionActionKind::CtrlAltDel
        | SessionActionKind::WakeDisplay => ActionDecision::Platform(kind),
        SessionActionKind::LockOnEnd => ActionDecision::SetLockOnEnd(msg.enable),
        SessionActionKind::Unspecified => ActionDecision::Reject(ActionReject::Unsupported),
    }
}

pub fn reject_error(reject: ActionReject) -> Option<pb::ErrorInfo> {
    match reject {
        ActionReject::NotController => None,
        ActionReject::Unsupported => Some(pb::ErrorInfo {
            code: u32::from(ErrorCode::Other.code()),
            message: "unsupported session action".to_owned(),
            retry_after_s: 0,
            detail: HashMap::new(),
        }),
    }
}

pub fn platform_failure_error(kind: SessionActionKind, err: &PlatformError) -> pb::ErrorInfo {
    let code = if matches!(err, PlatformError::PermissionDenied(_)) {
        ErrorCode::MissingPermissions
    } else {
        ErrorCode::Other
    };
    pb::ErrorInfo {
        code: u32::from(code.code()),
        message: err.to_string(),
        retry_after_s: 0,
        detail: HashMap::from([("action".to_owned(), action_name(kind).to_owned())]),
    }
}

fn action_name(kind: SessionActionKind) -> &'static str {
    match kind {
        SessionActionKind::LockScreen => "lock_screen",
        SessionActionKind::CtrlAltDel => "ctrl_alt_del",
        SessionActionKind::LockOnEnd => "lock_on_end",
        SessionActionKind::WakeDisplay => "wake_display",
        SessionActionKind::Unspecified => "unspecified",
    }
}
