//! 防睡眠与控制台状态策略。

use std::collections::HashMap;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use rsp_core::errors::ErrorCode;
use rsp_core::pb;
use tokio::sync::watch;
use tracing::{debug, warn};

use crate::platform::{ConsoleState, Platform, PlatformError, SessionActionKind};

/// Platform operations needed by the power and console policies.
pub trait PowerPort: Send + Sync {
    fn keep_awake(&self, on: bool);
    fn wake_display(&self) -> Result<(), PlatformError>;
    fn console_state(&self) -> ConsoleState;
}

/// Adapts a platform implementation to [`PowerPort`].
pub struct PlatformPower<P: Platform>(pub Arc<P>);

impl<P: Platform> PowerPort for PlatformPower<P> {
    fn keep_awake(&self, on: bool) {
        self.0.power_keep_awake(on);
    }

    fn wake_display(&self) -> Result<(), PlatformError> {
        self.0.session_action(SessionActionKind::WakeDisplay)
    }

    fn console_state(&self) -> ConsoleState {
        self.0.console_state()
    }
}

/// Reference-counted display-sleep inhibitor shared by all sessions.
pub struct KeepAwake {
    port: Arc<dyn PowerPort>,
    count: Mutex<usize>,
}

/// Keeps the display awake until this guard is dropped.
#[must_use = "keeping the guard alive holds the display awake"]
pub struct AwakeGuard {
    owner: Option<Arc<KeepAwake>>,
}

impl KeepAwake {
    pub fn new(port: Arc<dyn PowerPort>) -> Arc<Self> {
        Arc::new(Self {
            port,
            count: Mutex::new(0),
        })
    }

    /// Acquires one reference, enabling the inhibitor on the first reference.
    pub fn acquire(self: &Arc<Self>) -> AwakeGuard {
        let Some(mut count) = self.lock_count("acquire") else {
            return AwakeGuard { owner: None };
        };
        *count += 1;
        if *count == 1 {
            self.port.keep_awake(true);
        }
        AwakeGuard {
            owner: Some(Arc::clone(self)),
        }
    }

    pub fn active(&self) -> usize {
        self.lock_count("read").map_or(0, |count| *count)
    }

    fn lock_count(&self, operation: &'static str) -> Option<MutexGuard<'_, usize>> {
        match self.count.lock() {
            Ok(count) => Some(count),
            Err(_) => {
                warn!(
                    operation,
                    "keep-awake reference count lock is poisoned; count is unavailable"
                );
                None
            }
        }
    }
}

impl Drop for AwakeGuard {
    fn drop(&mut self) {
        let Some(owner) = self.owner.take() else {
            return;
        };
        let Some(mut count) = owner.lock_count("release") else {
            return;
        };
        if *count == 0 {
            warn!("keep-awake reference count underflow");
            return;
        }
        *count -= 1;
        if *count == 0 {
            owner.port.keep_awake(false);
        }
    }
}

/// Wakes the display when a stream starts. A failure never affects streaming.
pub fn wake_on_stream_start(port: &dyn PowerPort) {
    match port.wake_display() {
        Ok(()) => {}
        Err(PlatformError::Unsupported) => {
            debug!("waking the display is unsupported on this platform");
        }
        Err(error) => warn!(%error, "failed to wake the display for stream start"),
    }
}

/// Result of checking whether a remote session may use the local console.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleVerdict {
    Allow,
    Deny(pb::ErrorInfo),
}

/// A console-state change relevant to an active session.
#[derive(Debug, Clone, PartialEq)]
pub enum ConsoleChange {
    Left { error: pb::ErrorInfo },
    Returned,
}

/// Locked and unknown states still belong to the logged-in console user.
pub fn in_console(state: ConsoleState) -> bool {
    matches!(
        state,
        ConsoleState::LoggedIn | ConsoleState::Locked | ConsoleState::Unknown
    )
}

pub fn console_verdict(state: ConsoleState, screen_sharing: Option<bool>) -> ConsoleVerdict {
    if state == ConsoleState::Unknown {
        debug!("host console state is unknown; allowing the session");
    }
    if in_console(state) {
        ConsoleVerdict::Allow
    } else {
        ConsoleVerdict::Deny(console_error(state, screen_sharing))
    }
}

/// Builds the `RS-E190` error for a console that remote sessions cannot use.
pub fn console_error(state: ConsoleState, screen_sharing: Option<bool>) -> pb::ErrorInfo {
    let mut detail = HashMap::new();
    let console = if state == ConsoleState::OtherUser {
        "other_user"
    } else {
        "login_window"
    };
    detail.insert("console".to_owned(), console.to_owned());
    if let Some(screen_sharing) = screen_sharing {
        detail.insert(
            "screen_sharing".to_owned(),
            if screen_sharing { "on" } else { "off" }.to_owned(),
        );
    }
    pb::ErrorInfo {
        code: u32::from(ErrorCode::LoginWindow.code()),
        message: "host console is not available".to_owned(),
        retry_after_s: 0,
        detail,
    }
}

pub fn console_transition(old: ConsoleState, new: ConsoleState) -> Option<ConsoleChange> {
    match (in_console(old), in_console(new)) {
        (true, false) => Some(ConsoleChange::Left {
            error: console_error(new, None),
        }),
        (false, true) => Some(ConsoleChange::Returned),
        _ => None,
    }
}

/// Default console polling period; the protocol does not prescribe one.
pub const CONSOLE_POLL: Duration = Duration::from_secs(2);

/// Polls the console state and publishes changes to the returned receiver.
pub fn spawn_console_watcher(
    port: Arc<dyn PowerPort>,
    interval: Duration,
) -> (watch::Receiver<ConsoleState>, tokio::task::JoinHandle<()>) {
    let initial = port.console_state();
    let (sender, receiver) = watch::channel(initial);
    let first_poll = tokio::time::Instant::now() + interval;
    let handle = tokio::spawn(async move {
        let mut previous = initial;
        let mut deadline = first_poll;
        loop {
            if sender.is_closed() {
                break;
            }
            tokio::time::sleep_until(deadline).await;
            if sender.is_closed() {
                break;
            }
            let state = port.console_state();
            if state != previous {
                previous = state;
                sender.send_replace(state);
            }
            deadline += interval;
        }
    });
    (receiver, handle)
}
