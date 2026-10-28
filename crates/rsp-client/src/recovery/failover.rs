//! Client-side path failover and session recovery.
//!
//! This module only produces actions. It does not open connections, read the
//! clock, sleep, or spawn tasks; the session layer supplies the current
//! [`Instant`] and performs the returned actions.
//!
//! A recovery token is single-use from the client's point of view: as soon as
//! an [`pb::AuthResult`] for that token is received, the token is no longer
//! retried, regardless of the result. A successful result replaces it with
//! the newly issued token; every other result clears it. A connection attempt
//! that ends before an `AuthResult` arrives may retry the same token. The host
//! consumes the token only when recovery succeeds.

use std::fmt;

use rsp_core::pb;
use tokio::time::{Duration, Instant};

/// Length of an RSP recovery token.
pub const TOKEN_LEN: usize = 16;
/// Recovery tokens remain usable for 60 seconds after the connection is lost.
pub const RESUME_TTL: Duration = Duration::from_secs(60);
/// Target time for a path switch from the protocol specification.
pub const SWITCH_TARGET: Duration = Duration::from_secs(1);
/// Acceptance line used only to mark a slow switch in telemetry.
pub const SWITCH_LIMIT: Duration = Duration::from_secs(2);
/// Time to wait for an `AuthResult` after a backup path sends `ClientHello`.
pub const BACKUP_ATTEMPT_TIMEOUT: Duration = Duration::from_millis(1000);
/// Dial plus handshake timeout for one redial attempt.
pub const REDIAL_ATTEMPT_TIMEOUT: Duration = Duration::from_secs(8);
/// Backoff before redial attempts two through six and later attempts.
pub const REDIAL_BACKOFF: [Duration; 5] = [
    Duration::from_secs(0),
    Duration::from_secs(1),
    Duration::from_secs(2),
    Duration::from_secs(4),
    Duration::from_secs(8),
];
/// Wait after recovery for a keyframe before requesting one explicitly.
pub const KEYFRAME_WAIT: Duration = Duration::from_millis(500);

/// A locally retained host recovery token.
#[derive(Default)]
pub struct TicketSlot {
    token: Option<[u8; TOKEN_LEN]>,
    host_fp: [u8; 32],
    lost_at: Option<Instant>,
}

impl fmt::Debug for TicketSlot {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        let token = if self.token.is_some() {
            "<redacted>"
        } else {
            "<none>"
        };
        let lost_at = if self.lost_at.is_some() {
            "Some(<set>)"
        } else {
            "None"
        };
        write!(
            f,
            "TicketSlot {{ token: {token}, held: {}, lost_at: {lost_at} }}",
            self.token.is_some()
        )
    }
}

impl TicketSlot {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store a new token, clear the slot, or reject malformed input.
    pub fn store(&mut self, token: &[u8], host_fp: [u8; 32]) -> bool {
        if token.is_empty() {
            self.clear();
            return true;
        }
        if token.len() != TOKEN_LEN {
            return false;
        }

        let mut value = [0u8; TOKEN_LEN];
        value.copy_from_slice(token);
        self.token = Some(value);
        self.host_fp = host_fp;
        self.lost_at = None;
        true
    }

    /// Mark the first instant at which the connection became unavailable.
    pub fn mark_lost(&mut self, now: Instant) {
        if self.token.is_some() && self.lost_at.is_none() {
            self.lost_at = Some(now);
        }
    }

    /// Return a token without consuming it when it belongs to this host and
    /// has not passed its post-loss lifetime.
    pub fn token_for(&self, host_fp: &[u8; 32], now: Instant) -> Option<[u8; TOKEN_LEN]> {
        if self.host_fp != *host_fp {
            return None;
        }
        if let Some(lost_at) = self.lost_at {
            if now.saturating_duration_since(lost_at) >= RESUME_TTL {
                return None;
            }
        }
        self.token
    }

    pub fn expires_at(&self) -> Option<Instant> {
        self.lost_at.map(|lost_at| lost_at + RESUME_TTL)
    }

    pub fn is_valid(&self, now: Instant) -> bool {
        self.token_for(&self.host_fp, now).is_some()
    }

    pub fn clear(&mut self) {
        self.token = None;
        self.host_fp = [0; 32];
        self.lost_at = None;
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnId(pub u64);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ReconnectReason {
    PathLost,
    Background,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Route {
    Backup,
    Redial,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GiveUp {
    NoTicket,
    TicketExpired,
    Denied(u32),
    Closed(u64),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverPhase {
    Streaming,
    Switching {
        since: Instant,
        reason: ReconnectReason,
        route: Route,
        attempt: u32,
    },
    AwaitingKeyframe {
        since: Instant,
    },
    Failed(GiveUp),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FailoverAction {
    EnterReconnecting {
        reason: ReconnectReason,
    },
    FreezeVideo,
    ResetReassembler,
    CloseConn(ConnId),
    ResumeOn {
        conn: ConnId,
        token: [u8; TOKEN_LEN],
    },
    Redial {
        attempt: u32,
    },
    SendReleaseAll,
    RequestKeyframe,
    StartNewSession,
    HandToApproval {
        approval_timeout_s: u32,
    },
    Recovered {
        elapsed: Duration,
        resumed: bool,
        slow: bool,
    },
    GiveUp(GiveUp),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum InputVerdict {
    Send,
    Drop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameVerdict {
    Deliver,
    Drop,
    DeliverAndUnfreeze,
}

/// Pure failover state machine. The caller owns all I/O and timing.
#[derive(Debug)]
pub struct Failover {
    current: ConnId,
    host_fp: [u8; 32],
    phase: FailoverPhase,
    connected: bool,
    deadline: Option<Instant>,
    ticket_deadline: Option<Instant>,
    waiting_redial: bool,
    keyframe_requested: bool,
    dropped_inputs: u64,
}

impl Failover {
    pub fn new(primary: ConnId, host_fp: [u8; 32]) -> Self {
        Self {
            current: primary,
            host_fp,
            phase: FailoverPhase::Streaming,
            connected: true,
            deadline: None,
            ticket_deadline: None,
            waiting_redial: false,
            keyframe_requested: false,
            dropped_inputs: 0,
        }
    }

    pub fn current(&self) -> ConnId {
        self.current
    }

    pub fn phase(&self) -> &FailoverPhase {
        &self.phase
    }

    pub fn start(
        &mut self,
        reason: ReconnectReason,
        failed: ConnId,
        backup: Option<ConnId>,
        tickets: &mut TicketSlot,
        now: Instant,
    ) -> Vec<FailoverAction> {
        if !matches!(
            self.phase,
            FailoverPhase::Streaming | FailoverPhase::AwaitingKeyframe { .. }
        ) {
            return Vec::new();
        }

        tickets.mark_lost(now);
        let Some(token) = tickets.token_for(&self.host_fp, now) else {
            self.connected = false;
            self.phase = FailoverPhase::Failed(GiveUp::NoTicket);
            tracing::info!(reason = ?GiveUp::NoTicket, "session recovery gave up");
            return vec![
                FailoverAction::CloseConn(failed),
                FailoverAction::GiveUp(GiveUp::NoTicket),
            ];
        };

        self.phase = FailoverPhase::Switching {
            since: now,
            reason,
            route: if backup.is_some() {
                Route::Backup
            } else {
                Route::Redial
            },
            attempt: 1,
        };
        self.keyframe_requested = false;
        self.waiting_redial = false;
        self.ticket_deadline = tickets.expires_at();

        let mut actions = vec![
            FailoverAction::EnterReconnecting { reason },
            FailoverAction::FreezeVideo,
            FailoverAction::ResetReassembler,
            FailoverAction::CloseConn(failed),
        ];

        if let Some(backup) = backup {
            self.current = backup;
            self.connected = true;
            self.deadline = Some(now + BACKUP_ATTEMPT_TIMEOUT);
            actions.push(FailoverAction::ResumeOn {
                conn: backup,
                token,
            });
        } else {
            self.connected = false;
            self.deadline = Some(now + REDIAL_ATTEMPT_TIMEOUT);
            actions.push(FailoverAction::Redial { attempt: 1 });
        }
        actions
    }

    pub fn on_conn_closed(
        &mut self,
        conn: ConnId,
        close_code: Option<u64>,
        backup: Option<ConnId>,
        tickets: &mut TicketSlot,
        now: Instant,
    ) -> Vec<FailoverAction> {
        if conn != self.current {
            return Vec::new();
        }

        match self.phase.clone() {
            FailoverPhase::Streaming | FailoverPhase::AwaitingKeyframe { .. } => {
                if close_code.is_none() {
                    self.start(ReconnectReason::PathLost, conn, backup, tickets, now)
                } else {
                    Vec::new()
                }
            }
            FailoverPhase::Switching {
                since,
                reason,
                route,
                attempt,
            } => {
                self.connected = false;
                if let Some(code @ (2 | 6)) = close_code {
                    self.give_up(GiveUp::Closed(code))
                } else if route == Route::Backup {
                    self.phase = FailoverPhase::Switching {
                        since,
                        reason,
                        route: Route::Redial,
                        attempt: 1,
                    };
                    self.waiting_redial = false;
                    self.deadline = Some(now + REDIAL_ATTEMPT_TIMEOUT);
                    vec![FailoverAction::Redial { attempt: 1 }]
                } else {
                    self.schedule_redial_failure(since, reason, attempt, now);
                    Vec::new()
                }
            }
            FailoverPhase::Failed(_) => Vec::new(),
        }
    }

    pub fn on_connected(
        &mut self,
        conn: ConnId,
        tickets: &TicketSlot,
        now: Instant,
    ) -> Vec<FailoverAction> {
        let FailoverPhase::Switching {
            route: Route::Redial,
            ..
        } = self.phase
        else {
            return Vec::new();
        };
        if self.connected {
            return Vec::new();
        }

        self.current = conn;
        self.connected = true;
        self.waiting_redial = false;
        self.deadline = Some(now + BACKUP_ATTEMPT_TIMEOUT);
        let Some(token) = tickets.token_for(&self.host_fp, now) else {
            self.connected = false;
            self.phase = FailoverPhase::Failed(GiveUp::TicketExpired);
            tracing::info!(reason = ?GiveUp::TicketExpired, "session recovery gave up");
            return vec![
                FailoverAction::CloseConn(conn),
                FailoverAction::GiveUp(GiveUp::TicketExpired),
            ];
        };
        vec![FailoverAction::ResumeOn { conn, token }]
    }

    pub fn on_redial_failed(
        &mut self,
        _tickets: &mut TicketSlot,
        now: Instant,
    ) -> Vec<FailoverAction> {
        let FailoverPhase::Switching {
            since,
            reason,
            route,
            attempt,
        } = self.phase.clone()
        else {
            return Vec::new();
        };

        if route == Route::Backup {
            self.phase = FailoverPhase::Switching {
                since,
                reason,
                route: Route::Redial,
                attempt: 1,
            };
            self.connected = false;
            self.waiting_redial = false;
            self.deadline = Some(now + REDIAL_ATTEMPT_TIMEOUT);
            return vec![FailoverAction::Redial { attempt: 1 }];
        }

        self.schedule_redial_failure(since, reason, attempt, now);
        Vec::new()
    }

    pub fn on_auth_result(
        &mut self,
        conn: ConnId,
        result: &pb::AuthResult,
        tickets: &mut TicketSlot,
        now: Instant,
    ) -> Vec<FailoverAction> {
        let FailoverPhase::Switching {
            since,
            reason,
            route,
            attempt,
            ..
        } = self.phase.clone()
        else {
            return Vec::new();
        };
        if conn != self.current {
            return Vec::new();
        }

        let elapsed = now.saturating_duration_since(since);
        match pb::auth_result::Status::try_from(result.status) {
            Ok(pb::auth_result::Status::Ok) => {
                let resumed = result.resumed;
                let _ = tickets.store(&result.session_token, self.host_fp);
                self.phase = FailoverPhase::AwaitingKeyframe { since: now };
                self.deadline = None;
                self.ticket_deadline = tickets.expires_at();
                self.keyframe_requested = false;
                self.waiting_redial = false;
                let slow = elapsed > SWITCH_LIMIT;
                tracing::info!(
                    dur_ms = elapsed.as_millis() as u64,
                    resumed,
                    route = ?route,
                    attempt,
                    "session path recovered"
                );
                let mut actions = vec![FailoverAction::SendReleaseAll];
                if !resumed {
                    actions.push(FailoverAction::StartNewSession);
                }
                actions.push(FailoverAction::Recovered {
                    elapsed,
                    resumed,
                    slow,
                });
                actions
            }
            Ok(pb::auth_result::Status::PendingApproval) => {
                tickets.clear();
                self.phase = FailoverPhase::AwaitingKeyframe { since: now };
                self.deadline = None;
                self.ticket_deadline = None;
                self.keyframe_requested = false;
                vec![FailoverAction::HandToApproval {
                    approval_timeout_s: result.approval_timeout_s,
                }]
            }
            Ok(pb::auth_result::Status::Denied) => {
                tickets.clear();
                self.ticket_deadline = None;
                let code = result.error.as_ref().map_or(199, |error| error.code);
                self.give_up(GiveUp::Denied(code))
            }
            _ => {
                // An unknown status still consumes the token associated with
                // this AuthResult. The failed attempt is moved to redial.
                tickets.clear();
                self.ticket_deadline = None;
                self.connected = false;
                let mut actions = vec![FailoverAction::CloseConn(conn)];
                self.schedule_redial_failure(since, reason, attempt, now);
                actions.extend(self.poll(tickets, now));
                actions
            }
        }
    }

    pub fn poll(&mut self, tickets: &mut TicketSlot, now: Instant) -> Vec<FailoverAction> {
        let phase = self.phase.clone();
        match phase {
            FailoverPhase::Switching {
                since,
                reason,
                route,
                attempt,
            } => {
                if let Some(expires_at) = tickets.expires_at().or(self.ticket_deadline) {
                    if now >= expires_at {
                        // Expiry consumes the locally retained ticket.  A
                        // failed recovery must not leave an unusable token
                        // visible through SessionHandle::resume_token().
                        tickets.clear();
                        let mut actions = Vec::new();
                        if self.connected {
                            actions.push(FailoverAction::CloseConn(self.current));
                            self.connected = false;
                        }
                        actions.extend(self.give_up(GiveUp::TicketExpired));
                        return actions;
                    }
                }

                let Some(deadline) = self.deadline else {
                    return Vec::new();
                };
                if now < deadline {
                    return Vec::new();
                }

                match (route, self.waiting_redial) {
                    (Route::Backup, _) => {
                        self.connected = false;
                        self.phase = FailoverPhase::Switching {
                            since,
                            reason,
                            route: Route::Redial,
                            attempt: 1,
                        };
                        self.waiting_redial = false;
                        self.deadline = Some(now + REDIAL_ATTEMPT_TIMEOUT);
                        vec![
                            FailoverAction::CloseConn(self.current),
                            FailoverAction::Redial { attempt: 1 },
                        ]
                    }
                    (Route::Redial, true) => {
                        self.waiting_redial = false;
                        self.deadline = Some(now + REDIAL_ATTEMPT_TIMEOUT);
                        vec![FailoverAction::Redial { attempt }]
                    }
                    (Route::Redial, false) => {
                        let mut actions = Vec::new();
                        if self.connected {
                            actions.push(FailoverAction::CloseConn(self.current));
                            self.connected = false;
                        }
                        self.schedule_redial_failure(since, reason, attempt, now);
                        actions
                    }
                }
            }
            FailoverPhase::AwaitingKeyframe { since } => {
                if !self.keyframe_requested && now.saturating_duration_since(since) >= KEYFRAME_WAIT
                {
                    self.keyframe_requested = true;
                    vec![FailoverAction::RequestKeyframe]
                } else {
                    Vec::new()
                }
            }
            FailoverPhase::Streaming | FailoverPhase::Failed(_) => Vec::new(),
        }
    }

    pub fn adopt(&mut self, conn: ConnId) -> bool {
        if !matches!(self.phase, FailoverPhase::Streaming) {
            return false;
        }
        self.current = conn;
        self.connected = true;
        true
    }

    pub fn input_verdict(&mut self) -> InputVerdict {
        match self.phase {
            FailoverPhase::Streaming | FailoverPhase::AwaitingKeyframe { .. } => InputVerdict::Send,
            FailoverPhase::Switching { .. } | FailoverPhase::Failed(_) => {
                self.dropped_inputs = self.dropped_inputs.saturating_add(1);
                InputVerdict::Drop
            }
        }
    }

    pub fn frame_verdict(&mut self, conn: ConnId, keyframe: bool) -> FrameVerdict {
        if conn != self.current {
            return FrameVerdict::Drop;
        }
        match self.phase {
            FailoverPhase::Streaming => FrameVerdict::Deliver,
            FailoverPhase::Switching { .. } | FailoverPhase::Failed(_) => FrameVerdict::Drop,
            FailoverPhase::AwaitingKeyframe { .. } => {
                if keyframe {
                    self.phase = FailoverPhase::Streaming;
                    self.keyframe_requested = false;
                    FrameVerdict::DeliverAndUnfreeze
                } else {
                    FrameVerdict::Drop
                }
            }
        }
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match self.phase {
            FailoverPhase::Switching { .. } => match (self.deadline, self.ticket_deadline) {
                (Some(deadline), Some(expiry)) => Some(std::cmp::min(deadline, expiry)),
                (Some(deadline), None) => Some(deadline),
                (None, Some(expiry)) => Some(expiry),
                (None, None) => None,
            },
            FailoverPhase::AwaitingKeyframe { since } if !self.keyframe_requested => {
                Some(since + KEYFRAME_WAIT)
            }
            _ => None,
        }
    }

    pub fn dropped_inputs(&self) -> u64 {
        self.dropped_inputs
    }

    fn schedule_redial_failure(
        &mut self,
        since: Instant,
        reason: ReconnectReason,
        attempt: u32,
        now: Instant,
    ) {
        let next_attempt = attempt.saturating_add(1);
        let backoff = REDIAL_BACKOFF[usize::min(next_attempt.saturating_sub(1) as usize, 4)];
        self.phase = FailoverPhase::Switching {
            since,
            reason,
            route: Route::Redial,
            attempt: next_attempt,
        };
        self.connected = false;
        self.waiting_redial = true;
        self.deadline = Some(now + backoff);
    }

    fn give_up(&mut self, reason: GiveUp) -> Vec<FailoverAction> {
        self.phase = FailoverPhase::Failed(reason);
        self.deadline = None;
        self.ticket_deadline = None;
        self.waiting_redial = false;
        self.connected = false;
        tracing::info!(reason = ?reason, "session recovery gave up");
        vec![FailoverAction::GiveUp(reason)]
    }
}
