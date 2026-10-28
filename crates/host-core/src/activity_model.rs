//! Platform-independent local input activity state and event filtering.

use std::time::Duration;

/// Do not emit another active marker more often than this interval.
pub const ACTIVE_THROTTLE: Duration = Duration::from_millis(200);
/// Emit an inactive marker after this much time without local input.
pub const SILENCE: Duration = Duration::from_secs(2);

/// Marker attached to every input event injected by Remote Screen.
pub const RS_EXTRA_INFO: usize = 0x5253;
/// Low-level keyboard hook flag for injected input.
pub const LLKHF_INJECTED: u32 = 0x10;
/// Low-level mouse hook flag for injected input.
pub const LLMHF_INJECTED: u32 = 0x01;

/// Whether a low-level keyboard event was injected by Remote Screen itself.
pub const fn keyboard_is_ours(flags: u32, extra_info: usize) -> bool {
    flags & LLKHF_INJECTED != 0 && extra_info == RS_EXTRA_INFO
}

/// Whether a low-level mouse event was injected by Remote Screen itself.
pub const fn mouse_is_ours(flags: u32, extra_info: usize) -> bool {
    flags & LLMHF_INJECTED != 0 && extra_info == RS_EXTRA_INFO
}

/// Throttle/silence state machine for local activity notifications.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActivityState {
    active: bool,
    last_event: Option<Duration>,
    last_emit: Option<Duration>,
}

impl ActivityState {
    /// Record local input at `now`, returning an active marker when due.
    pub fn on_event(&mut self, now: Duration) -> Option<bool> {
        let emit = if !self.active {
            self.active = true;
            true
        } else {
            self.last_emit
                .map(|last| now.saturating_sub(last) >= ACTIVE_THROTTLE)
                .unwrap_or(true)
        };
        self.last_event = Some(now);
        if emit {
            self.last_emit = Some(now);
            Some(true)
        } else {
            None
        }
    }

    /// Advance the silence timer, returning an inactive marker when due.
    pub fn on_tick(&mut self, now: Duration) -> Option<bool> {
        if self.active
            && self
                .last_event
                .map(|last| now.saturating_sub(last) >= SILENCE)
                .unwrap_or(false)
        {
            self.active = false;
            self.last_emit = None;
            return Some(false);
        }
        None
    }
}

/// De-duplicates calls to the platform power API.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeepAwakeLatch {
    on: bool,
}

impl KeepAwakeLatch {
    /// Change the desired state, returning it only when it changed.
    pub fn set(&mut self, on: bool) -> Option<bool> {
        if self.on == on {
            None
        } else {
            self.on = on;
            Some(on)
        }
    }
}
