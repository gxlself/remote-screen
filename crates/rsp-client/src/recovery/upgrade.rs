//! TCP 模式下定期尝试升级回 QUIC。
//!
//! The state machine is clock- and network-free: callers supply the current
//! [`Instant`] and carry out the returned actions. See
//! `docs/dev/03-protocol.md` §1.3 and §10.4.

use std::time::Duration;

use rsp_core::pb;
use rsp_transport::TransportKind;
use tokio::time::Instant;

/// How often a TCP primary attempts an upgrade to QUIC.
pub const UPGRADE_EVERY: Duration = Duration::from_secs(30);

/// Timeout for establishing the candidate QUIC connection.
///
/// The specification does not prescribe a separate value; this follows the
/// overall dial timeout in §10.2.
pub const PROBE_TIMEOUT: Duration = Duration::from_secs(3);

/// Timeout from sending the resume `ClientHello` to receiving `AuthResult`.
///
/// The specification does not prescribe a separate value; this follows the
/// overall dial timeout in §10.2.
pub const RESUME_TIMEOUT: Duration = Duration::from_secs(3);

/// Current phase of the TCP-to-QUIC upgrade scheduler.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeState {
    /// QUIC is the primary transport, or the session is not streaming.
    Inactive,
    /// The next upgrade attempt is due at `next`.
    Waiting { next: Instant },
    /// A candidate QUIC connection is being established.
    Probing { started: Instant },
    /// The candidate QUIC connection is resuming the session.
    Resuming { started: Instant, since: Instant },
}

/// An effect requested by [`Upgrader`].
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum UpgradeAction {
    /// Dial QUIC only; do not add TCP to the race or open a control stream.
    ProbeQuic,
    /// Close the QUIC connection obtained by a probe.
    DropProbe,
    /// Open the control stream and send `ClientHello` with the current token.
    ResumeOnQuic,
    /// The resumed QUIC connection becomes the primary transport.
    Commit,
    /// The Host created a new session on QUIC and the stream must restart.
    CommitNewSession,
}

/// Pure scheduler for periodic TCP-to-QUIC upgrades.
#[derive(Debug, Clone)]
pub struct Upgrader {
    state: UpgradeState,
    blocked: bool,
    attempts: u32,
}

impl Upgrader {
    pub fn new() -> Self {
        Self {
            state: UpgradeState::Inactive,
            blocked: false,
            attempts: 0,
        }
    }

    /// Records the transport currently carrying the session.
    pub fn on_primary(&mut self, transport: TransportKind, now: Instant) -> Vec<UpgradeAction> {
        match transport {
            TransportKind::Tcp => {
                if self.state == UpgradeState::Inactive {
                    self.state = UpgradeState::Waiting {
                        next: now + UPGRADE_EVERY,
                    };
                }
                Vec::new()
            }
            TransportKind::Quic => {
                let actions = match self.state {
                    UpgradeState::Probing { .. } | UpgradeState::Resuming { .. } => {
                        vec![UpgradeAction::DropProbe]
                    }
                    UpgradeState::Inactive | UpgradeState::Waiting { .. } => Vec::new(),
                };
                self.state = UpgradeState::Inactive;
                actions
            }
        }
    }

    /// Pauses or resumes upgrade attempts.
    pub fn set_blocked(&mut self, blocked: bool, now: Instant) -> Vec<UpgradeAction> {
        self.blocked = blocked;

        if blocked {
            match self.state {
                UpgradeState::Probing { started } => {
                    self.state = UpgradeState::Waiting {
                        next: retry_at(started, now),
                    };
                    tracing::debug!(
                        attempt = self.attempts,
                        result = "blocked",
                        "QUIC upgrade probe stopped"
                    );
                    vec![UpgradeAction::DropProbe]
                }
                UpgradeState::Resuming { started, .. } => {
                    self.state = UpgradeState::Waiting {
                        next: retry_at(started, now),
                    };
                    tracing::debug!(
                        attempt = self.attempts,
                        result = "blocked",
                        "QUIC session resume stopped"
                    );
                    vec![UpgradeAction::DropProbe]
                }
                UpgradeState::Inactive | UpgradeState::Waiting { .. } => Vec::new(),
            }
        } else {
            if let UpgradeState::Waiting { next } = self.state {
                if next <= now {
                    self.state = UpgradeState::Waiting { next: now };
                }
            }
            Vec::new()
        }
    }

    /// Advances timers and starts a probe when one is due.
    pub fn poll(&mut self, now: Instant) -> Vec<UpgradeAction> {
        match self.state {
            UpgradeState::Waiting { next } if !self.blocked && now >= next => {
                self.state = UpgradeState::Probing { started: now };
                self.attempts = self.attempts.saturating_add(1);
                tracing::debug!(
                    attempt = self.attempts,
                    result = "started",
                    "QUIC upgrade probe started"
                );
                vec![UpgradeAction::ProbeQuic]
            }
            UpgradeState::Probing { started } => {
                if deadline_reached(started, PROBE_TIMEOUT, now) {
                    self.state = UpgradeState::Waiting {
                        next: retry_at(started, now),
                    };
                    tracing::debug!(
                        attempt = self.attempts,
                        result = "probe_timeout",
                        "QUIC upgrade probe timed out"
                    );
                    vec![UpgradeAction::DropProbe]
                } else {
                    Vec::new()
                }
            }
            UpgradeState::Resuming { started, since } => {
                if deadline_reached(since, RESUME_TIMEOUT, now) {
                    self.state = UpgradeState::Waiting {
                        next: retry_at(started, now),
                    };
                    tracing::debug!(
                        attempt = self.attempts,
                        result = "resume_timeout",
                        "QUIC session resume timed out"
                    );
                    vec![UpgradeAction::DropProbe]
                } else {
                    Vec::new()
                }
            }
            UpgradeState::Inactive | UpgradeState::Waiting { .. } => Vec::new(),
        }
    }

    /// Handles a successful candidate QUIC handshake.
    pub fn on_probe_ok(&mut self, now: Instant) -> Vec<UpgradeAction> {
        match self.state {
            UpgradeState::Probing { started } => {
                self.state = UpgradeState::Resuming {
                    started,
                    since: now,
                };
                tracing::debug!(
                    attempt = self.attempts,
                    result = "probe_ok",
                    "QUIC upgrade probe succeeded"
                );
                vec![UpgradeAction::ResumeOnQuic]
            }
            UpgradeState::Inactive
            | UpgradeState::Waiting { .. }
            | UpgradeState::Resuming { .. } => vec![UpgradeAction::DropProbe],
        }
    }

    /// Handles a failed candidate QUIC handshake.
    pub fn on_probe_failed(&mut self, now: Instant) -> Vec<UpgradeAction> {
        if let UpgradeState::Probing { started } = self.state {
            self.state = UpgradeState::Waiting {
                next: retry_at(started, now),
            };
            tracing::debug!(
                attempt = self.attempts,
                result = "probe_failed",
                "QUIC upgrade probe failed"
            );
        }
        Vec::new()
    }

    /// Handles `AuthResult` received while resuming on the candidate connection.
    pub fn on_auth_result(&mut self, result: &pb::AuthResult, now: Instant) -> Vec<UpgradeAction> {
        let UpgradeState::Resuming { started, .. } = self.state else {
            return Vec::new();
        };

        if result.status == pb::auth_result::Status::Ok as i32 {
            self.state = UpgradeState::Inactive;
            tracing::debug!(
                attempt = self.attempts,
                result = "committed",
                resumed = result.resumed,
                "QUIC upgrade committed"
            );
            tracing::info!(
                event = "path.switch",
                attempt = self.attempts,
                resumed = result.resumed,
                "QUIC upgrade committed"
            );
            if result.resumed {
                return vec![UpgradeAction::Commit];
            }
            return vec![UpgradeAction::CommitNewSession];
        }

        self.state = UpgradeState::Waiting {
            next: retry_at(started, now),
        };
        tracing::debug!(
            attempt = self.attempts,
            result = "resume_rejected",
            status = result.status,
            "QUIC session resume rejected"
        );
        vec![UpgradeAction::DropProbe]
    }

    /// Whether the old TCP connection being replaced is expected right now.
    pub fn expects_replacement(&self) -> bool {
        matches!(self.state, UpgradeState::Resuming { .. })
    }

    pub fn state(&self) -> UpgradeState {
        self.state
    }

    pub fn attempts(&self) -> u32 {
        self.attempts
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            UpgradeState::Waiting { next } if !self.blocked => Some(next),
            UpgradeState::Probing { started } => started.checked_add(PROBE_TIMEOUT),
            UpgradeState::Resuming { since, .. } => since.checked_add(RESUME_TIMEOUT),
            UpgradeState::Inactive | UpgradeState::Waiting { .. } => None,
        }
    }
}

impl Default for Upgrader {
    fn default() -> Self {
        Self::new()
    }
}

fn retry_at(attempt_started: Instant, now: Instant) -> Instant {
    match attempt_started.checked_add(UPGRADE_EVERY) {
        Some(next) if next > now => next,
        _ => now,
    }
}

fn deadline_reached(started: Instant, timeout: Duration, now: Instant) -> bool {
    match started.checked_add(timeout) {
        Some(deadline) => now >= deadline,
        None => true,
    }
}
