//! 每条连接的推流闸门：PauseStream / ResumeStream 与恢复结果。
//!
//! The gate is deliberately transport-independent. Callers supply unix
//! timestamps in microseconds and execute the returned actions against their
//! encoder and cursor sender. A paused connection resumes automatically when
//! the fallback expires; it stays connected.

use rsp_core::errors::ErrorCode;
use rsp_core::pb;

use super::roster::{self, RosterError, RosterRole};

/// How long a connection may stay paused before the host automatically resumes
/// streaming. Zero disables the fallback.
pub const MAX_PAUSE_US: u64 = 45_000_000;

/// State of the stream owned by one connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateState {
    /// No `StartStream` has been accepted yet.
    Idle,
    /// The connection is encoding frames.
    Live,
    /// The connection is retained but paused.
    Paused { since_us: u64 },
}

/// Work the caller must perform after a gate transition.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    /// Start or restart the encoder for this connection. Send `StreamConfig`
    /// before doing so when wiring the gate into the runtime.
    StartEncoder,
    /// Stop encoding for this connection. Capture may continue when another
    /// connection for the same display still needs frames.
    StopEncoder,
    /// Make the next encoded frame a keyframe.
    ForceKeyframe,
    /// Send the current cursor shape before the first frame.
    SendCursorShape,
}

/// Per-connection stream gate.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StreamGate {
    started: bool,
    paused_since: Option<u64>,
    max_pause_us: u64,
}

impl StreamGate {
    /// Create an idle gate. `max_pause_us = 0` disables the pause fallback.
    pub fn new(max_pause_us: u64) -> Self {
        Self {
            started: false,
            paused_since: None,
            max_pause_us,
        }
    }

    /// Create a gate for a successfully resumed session.
    pub fn resumed(max_pause_us: u64) -> (Self, Vec<GateAction>) {
        let gate = Self {
            started: true,
            paused_since: None,
            max_pause_us,
        };
        (gate, start_stream_actions())
    }

    /// Handle `StartStream`.
    pub fn on_start_stream(&mut self, _now_us: u64) -> Vec<GateAction> {
        if self.paused_since.is_some() {
            self.started = true;
            Vec::new()
        } else if self.started {
            vec![GateAction::ForceKeyframe]
        } else {
            self.started = true;
            start_stream_actions()
        }
    }

    /// Handle `PauseStream`.
    pub fn on_pause(&mut self, now_us: u64) -> Vec<GateAction> {
        if self.paused_since.is_some() {
            return Vec::new();
        }

        self.paused_since = Some(now_us);
        if self.started {
            vec![GateAction::StopEncoder]
        } else {
            Vec::new()
        }
    }

    /// Handle `ResumeStream`.
    pub fn on_resume(&mut self, _now_us: u64) -> Vec<GateAction> {
        match self.paused_since.take() {
            Some(_) => {
                if self.started {
                    start_stream_actions()
                } else {
                    Vec::new()
                }
            }
            None if self.started => vec![GateAction::ForceKeyframe],
            None => Vec::new(),
        }
    }

    /// Clear a timed-out pause and restart streaming if a start was accepted.
    pub fn poll(&mut self, now_us: u64) -> Vec<GateAction> {
        let Some(since_us) = self.paused_since else {
            return Vec::new();
        };
        if self.max_pause_us == 0 {
            return Vec::new();
        }

        // A clock jump backwards is treated as no elapsed time.
        if now_us.saturating_sub(since_us) >= self.max_pause_us {
            self.paused_since = None;
            if self.started {
                start_stream_actions()
            } else {
                Vec::new()
            }
        } else {
            Vec::new()
        }
    }

    /// Report the current state. A pause takes precedence even if the app sent
    /// `StartStream` while it was in the background.
    pub fn state(&self) -> GateState {
        match (self.started, self.paused_since) {
            (_, Some(since_us)) => GateState::Paused { since_us },
            (true, None) => GateState::Live,
            (false, None) => GateState::Idle,
        }
    }

    /// Whether this connection currently wants encoded frames.
    pub fn wants_frames(&self) -> bool {
        self.started && self.paused_since.is_none()
    }

    /// Wake-up time for the pause fallback, if it is active and still pending.
    pub fn next_deadline(&self) -> Option<u64> {
        match self.paused_since {
            Some(since_us) if self.max_pause_us != 0 => {
                Some(since_us.saturating_add(self.max_pause_us))
            }
            _ => None,
        }
    }
}

fn start_stream_actions() -> Vec<GateAction> {
    vec![
        GateAction::StartEncoder,
        GateAction::ForceKeyframe,
        GateAction::SendCursorShape,
    ]
}

/// Whether at least one connection using a display still needs capture.
pub fn capture_needed<'a, I: IntoIterator<Item = &'a StreamGate>>(gates: I) -> bool {
    gates.into_iter().any(StreamGate::wants_frames)
}

/// Host response to a resume-token attempt and its follow-up work.
#[derive(Debug, Clone, PartialEq)]
pub enum ResumeReply {
    Resumed {
        auth: pb::AuthResult,
        prefs: roster::SessionPrefs,
        replaced: Option<roster::ConnKey>,
        release_all: bool,
    },
    /// Continue through the normal new-session authentication path.
    FreshSession {
        reason: &'static str,
    },
    Deny {
        auth: pb::AuthResult,
    },
}

/// Convert the roster result into the reply and follow-up actions required by
/// the handshake.
pub fn resume_reply(result: Result<roster::Resumed, roster::RosterError>) -> ResumeReply {
    match result {
        Ok(resumed) => {
            let release_all = resumed.replaced.is_some() && resumed.role == RosterRole::Controller;
            ResumeReply::Resumed {
                auth: auth_ok(resumed.role, &resumed.token, true),
                prefs: resumed.prefs,
                replaced: resumed.replaced,
                release_all,
            }
        }
        Err(RosterError::InvalidToken) => ResumeReply::FreshSession {
            reason: "invalid_token",
        },
        Err(RosterError::TokenDeviceMismatch) => {
            tracing::warn!(event = "security.resume_mismatch");
            ResumeReply::FreshSession {
                reason: "device_mismatch",
            }
        }
        Err(RosterError::SessionFull) => ResumeReply::Deny {
            auth: auth_denied(ErrorCode::SessionFull),
        },
        Err(RosterError::Random) => ResumeReply::Deny {
            auth: auth_denied(ErrorCode::Other),
        },
        Err(RosterError::UnknownSession | RosterError::ViewOnlyDevice) => {
            ResumeReply::FreshSession { reason: "other" }
        }
    }
}

/// Build an accepted authentication result, including the rotated token.
pub fn auth_ok(role: RosterRole, token: &[u8; 16], resumed: bool) -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::Ok as i32,
        role: match role {
            RosterRole::Controller => pb::Role::Controller as i32,
            RosterRole::Viewer => pb::Role::Viewer as i32,
        },
        session_token: token.to_vec(),
        approval_timeout_s: 0,
        error: None,
        resumed,
    }
}

/// Build a denied authentication result for a recovery attempt.
pub fn auth_denied(code: ErrorCode) -> pb::AuthResult {
    let message = match code {
        ErrorCode::SessionFull => "session is full",
        ErrorCode::Other => "internal error",
        _ => "request denied",
    };
    pb::AuthResult {
        status: pb::auth_result::Status::Denied as i32,
        role: pb::Role::Unspecified as i32,
        session_token: Vec::new(),
        approval_timeout_s: 0,
        error: Some(pb::ErrorInfo {
            code: u32::from(code.code()),
            message: message.to_owned(),
            retry_after_s: 0,
            detail: Default::default(),
        }),
        resumed: false,
    }
}
