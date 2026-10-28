//! 后台时间线：Pause/Resume、30 s 告别、60 s 内用令牌恢复。
//!
//! iOS uses the default 30 s grace period; Android foreground-service sessions
//! can use `None` to remain paused without saying goodbye (see
//! `device.backgroundHold` in rs-mobile).
//!
//! This module contains pure state-machine logic for a session that is already
//! `Streaming`. It performs no I/O and reads no clock; callers pass in the
//! current [`Instant`]. Entering the background while awaiting approval or
//! while failover is in progress belongs to other tasks and must not be routed
//! through this timeline.
//!
//! Timers are based on planned timestamps rather than the time a delayed
//! `poll` happens to run. This matters on iOS where app suspension stops
//! timers: a wake-up at 45 s must still date the goodbye and the 60 s recovery
//! window from the 30 s boundary.

use std::time::Duration;

use tokio::time::Instant;

/// Time the session may stay paused before the App says goodbye.
pub const PAUSE_GRACE: Duration = Duration::from_secs(30);

/// Time the Host retains the session after `Goodbye(BACKGROUND)`.
pub const RESUME_TTL: Duration = Duration::from_secs(60);

/// Target time for a keyframe after returning to the foreground. If none has
/// arrived by this deadline, the timeline asks for one more keyframe.
pub const RESUME_KEYFRAME_WAIT: Duration = Duration::from_millis(500);

/// Background lifecycle state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgState {
    Foreground,
    Paused { since: Instant },
    Closed { deadline: Instant },
    Expired,
}

/// An effect emitted by [`BackgroundTimeline`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BgAction {
    SendReleaseAll,
    SendPauseStream,
    SuspendMonitors,
    SendResumeStream,
    ResumeMonitors,
    RequestKeyframe,
    SendGoodbyeBackground,
    CloseConnection,
    MarkTicketLost { at: Instant },
    StartResume,
    ExpireTicket,
    ReturnToDevices,
}

/// Pure background/resume timeline for a `Streaming` session.
#[derive(Debug, Clone)]
pub struct BackgroundTimeline {
    state: BgState,
    grace: Option<Duration>,
    keyframe_deadline: Option<Instant>,
    keyframe_requested: bool,
    expired_return_emitted: bool,
}

impl BackgroundTimeline {
    pub fn new() -> Self {
        Self::with_grace(Some(PAUSE_GRACE))
    }

    /// Creates a timeline with the supplied background grace period.
    ///
    /// `None` keeps a paused session alive until it returns to the foreground
    /// or the connection is lost.
    pub fn with_grace(grace: Option<Duration>) -> Self {
        Self {
            state: BgState::Foreground,
            grace,
            keyframe_deadline: None,
            keyframe_requested: false,
            expired_return_emitted: false,
        }
    }

    pub fn grace(&self) -> Option<Duration> {
        self.grace
    }

    /// Changes the grace period. While paused, the pause timer starts over at
    /// `now`; in every other state only the configured value is changed.
    pub fn set_grace(&mut self, grace: Option<Duration>, now: Instant) {
        self.grace = grace;
        if matches!(self.state, BgState::Paused { .. }) {
            self.set_state(BgState::Paused { since: now });
        }
    }

    /// Handles an app foreground/background transition.
    pub fn set_background(&mut self, on: bool, now: Instant) -> Vec<BgAction> {
        match self.state {
            BgState::Foreground => {
                if !on {
                    return Vec::new();
                }
                self.clear_keyframe_watch();
                self.set_state(BgState::Paused { since: now });
                vec![
                    BgAction::SendReleaseAll,
                    BgAction::SendPauseStream,
                    BgAction::SuspendMonitors,
                ]
            }
            BgState::Paused { since } => {
                if on {
                    return Vec::new();
                }

                let Some(grace) = self.grace else {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Foreground);
                    self.keyframe_deadline = Some(now + RESUME_KEYFRAME_WAIT);
                    return vec![BgAction::SendResumeStream, BgAction::ResumeMonitors];
                };
                let goodbye_at = since + grace;
                let deadline = goodbye_at + RESUME_TTL;
                if now < goodbye_at {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Foreground);
                    self.keyframe_deadline = Some(now + RESUME_KEYFRAME_WAIT);
                    vec![BgAction::SendResumeStream, BgAction::ResumeMonitors]
                } else if now < deadline {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Foreground);
                    vec![
                        BgAction::SendGoodbyeBackground,
                        BgAction::CloseConnection,
                        BgAction::MarkTicketLost { at: goodbye_at },
                        BgAction::StartResume,
                    ]
                } else {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Expired);
                    self.expired_return_emitted = true;
                    vec![
                        BgAction::SendGoodbyeBackground,
                        BgAction::CloseConnection,
                        BgAction::ExpireTicket,
                        BgAction::ReturnToDevices,
                    ]
                }
            }
            BgState::Closed { deadline } => {
                if on {
                    return Vec::new();
                }

                if now < deadline {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Foreground);
                    vec![BgAction::StartResume]
                } else {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Expired);
                    self.expired_return_emitted = true;
                    vec![BgAction::ExpireTicket, BgAction::ReturnToDevices]
                }
            }
            BgState::Expired => {
                if !on && !self.expired_return_emitted {
                    self.expired_return_emitted = true;
                    tracing::debug!("background timeline returned to devices after expiry");
                    vec![BgAction::ReturnToDevices]
                } else {
                    Vec::new()
                }
            }
        }
    }

    /// Handles loss of the main connection. During the pause this starts the
    /// recovery-token window at the actual loss time; in the foreground route
    /// recovery is owned by /.
    pub fn on_connection_lost(&mut self, now: Instant) -> Vec<BgAction> {
        match self.state {
            BgState::Paused { .. } => {
                self.clear_keyframe_watch();
                let deadline = now + RESUME_TTL;
                self.set_state(BgState::Closed { deadline });
                vec![BgAction::MarkTicketLost { at: now }]
            }
            BgState::Foreground => {
                self.clear_keyframe_watch();
                Vec::new()
            }
            BgState::Closed { .. } | BgState::Expired => Vec::new(),
        }
    }

    /// Stops the resume keyframe watchdog when a frame arrives.
    pub fn on_keyframe(&mut self, now: Instant) {
        if self.state != BgState::Foreground {
            return;
        }
        if let Some(deadline) = self.keyframe_deadline.take() {
            self.keyframe_requested = false;
            tracing::debug!(
                late = now >= deadline,
                "background timeline observed resume keyframe"
            );
        }
    }

    /// Emits any actions whose planned deadline has been reached.
    pub fn poll(&mut self, now: Instant) -> Vec<BgAction> {
        match self.state {
            BgState::Paused { since } => {
                let Some(grace) = self.grace else {
                    return Vec::new();
                };
                let goodbye_at = since + grace;
                let deadline = goodbye_at + RESUME_TTL;
                if now >= deadline {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Expired);
                    self.expired_return_emitted = false;
                    vec![
                        BgAction::SendGoodbyeBackground,
                        BgAction::CloseConnection,
                        BgAction::ExpireTicket,
                    ]
                } else if now >= goodbye_at {
                    self.clear_keyframe_watch();
                    self.set_state(BgState::Closed { deadline });
                    vec![
                        BgAction::SendGoodbyeBackground,
                        BgAction::CloseConnection,
                        BgAction::MarkTicketLost { at: goodbye_at },
                    ]
                } else {
                    Vec::new()
                }
            }
            BgState::Closed { deadline } => {
                if now >= deadline {
                    self.set_state(BgState::Expired);
                    self.expired_return_emitted = false;
                    vec![BgAction::ExpireTicket]
                } else {
                    Vec::new()
                }
            }
            BgState::Foreground => {
                let Some(deadline) = self.keyframe_deadline else {
                    return Vec::new();
                };
                if !self.keyframe_requested && now >= deadline {
                    self.keyframe_requested = true;
                    tracing::debug!(?deadline, "background keyframe watchdog fired");
                    vec![BgAction::RequestKeyframe]
                } else {
                    Vec::new()
                }
            }
            BgState::Expired => Vec::new(),
        }
    }

    pub fn state(&self) -> BgState {
        self.state
    }

    /// Returns the next timestamp with a pending action, if any.
    pub fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            BgState::Paused { since } => self.grace.map(|grace| since + grace),
            BgState::Closed { deadline } => Some(deadline),
            BgState::Foreground if !self.keyframe_requested => self.keyframe_deadline,
            BgState::Foreground | BgState::Expired => None,
        }
    }

    fn set_state(&mut self, state: BgState) {
        if self.state == state {
            return;
        }
        tracing::debug!(
            from = ?self.state,
            to = ?state,
            "background timeline state changed"
        );
        self.state = state;
    }

    fn clear_keyframe_watch(&mut self) {
        self.keyframe_deadline = None;
        self.keyframe_requested = false;
    }
}

impl Default for BackgroundTimeline {
    fn default() -> Self {
        Self::new()
    }
}
