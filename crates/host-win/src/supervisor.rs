//! Event-driven supervision of the active console session's host process.
//!
//! The caller supplies time, executes actions in order, and reports process and
//! pipe events. This module performs no operating-system calls or waiting.

use std::time::{Duration, Instant};

pub const NO_SESSION: u32 = 0xFFFF_FFFF;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SupervisorConfig {
    pub connect_timeout: Duration,
    pub backoff_initial: Duration,
    pub backoff_max: Duration,
    pub stable_after: Duration,
}

impl Default for SupervisorConfig {
    fn default() -> Self {
        Self {
            connect_timeout: Duration::from_secs(10),
            backoff_initial: Duration::from_secs(1),
            backoff_max: Duration::from_secs(30),
            stable_after: Duration::from_secs(60),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SessionChangeKind {
    ConsoleConnect,
    ConsoleDisconnect,
    RemoteConnect,
    RemoteDisconnect,
    Logon,
    Logoff,
    Lock,
    Unlock,
    Other(u32),
}

impl SessionChangeKind {
    pub fn from_wparam(w: u32) -> Self {
        match w {
            1 => Self::ConsoleConnect,
            2 => Self::ConsoleDisconnect,
            3 => Self::RemoteConnect,
            4 => Self::RemoteDisconnect,
            5 => Self::Logon,
            6 => Self::Logoff,
            7 => Self::Lock,
            8 => Self::Unlock,
            _ => Self::Other(w),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SupEvent {
    ConsoleSession(Option<u32>),
    SessionChange {
        kind: SessionChangeKind,
        session_id: u32,
    },
    Spawned {
        session_id: u32,
        pid: u32,
    },
    SpawnFailed {
        session_id: u32,
        error: String,
    },
    Connected {
        pid: u32,
    },
    Exited {
        pid: u32,
        code: u32,
    },
    Tick,
    Shutdown,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SupAction {
    Spawn { session_id: u32 },
    Kill { pid: u32 },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ChildState {
    Idle,
    Backoff {
        until: Instant,
    },
    Spawning {
        session_id: u32,
    },
    Starting {
        session_id: u32,
        pid: u32,
        since: Instant,
    },
    Running {
        session_id: u32,
        pid: u32,
        since: Instant,
    },
    Stopped,
}

pub struct Supervisor {
    cfg: SupervisorConfig,
    state: ChildState,
    target: Option<u32>,
    backoff: Duration,
}

impl Supervisor {
    pub fn new(cfg: SupervisorConfig) -> Self {
        Self {
            cfg,
            state: ChildState::Idle,
            target: None,
            backoff: cfg.backoff_initial,
        }
    }

    pub fn handle(&mut self, ev: SupEvent, now: Instant) -> Vec<SupAction> {
        if self.state == ChildState::Stopped {
            return Vec::new();
        }

        let mut actions = Vec::new();
        match ev {
            SupEvent::ConsoleSession(session_id) => {
                self.select_session(session_id, &mut actions);
            }
            SupEvent::SessionChange { kind, session_id } => match kind {
                SessionChangeKind::ConsoleConnect => {
                    self.select_session(Some(session_id), &mut actions);
                }
                SessionChangeKind::Logon | SessionChangeKind::Logoff
                    if self.target == Some(session_id) && self.current_pid().is_some() =>
                {
                    self.backoff = self.cfg.backoff_initial;
                    self.spawn(session_id, &mut actions);
                }
                _ => {}
            },
            SupEvent::Spawned { session_id, pid } => {
                if self.state == (ChildState::Spawning { session_id }) {
                    self.set_state(ChildState::Starting {
                        session_id,
                        pid,
                        since: now,
                    });
                } else {
                    actions.push(SupAction::Kill { pid });
                }
            }
            SupEvent::SpawnFailed { session_id, .. } => {
                if self.state == (ChildState::Spawning { session_id }) {
                    self.enter_backoff(now);
                }
            }
            SupEvent::Connected { pid } => {
                if let ChildState::Starting {
                    session_id,
                    pid: current_pid,
                    ..
                } = self.state
                {
                    if pid == current_pid {
                        self.set_state(ChildState::Running {
                            session_id,
                            pid,
                            since: now,
                        });
                    }
                }
            }
            SupEvent::Exited { pid, .. } => {
                if self.current_pid() == Some(pid) {
                    if let ChildState::Running { since, .. } = self.state {
                        if now.duration_since(since) >= self.cfg.stable_after {
                            self.backoff = self.cfg.backoff_initial;
                        }
                    }
                    self.enter_backoff(now);
                }
            }
            SupEvent::Tick => match self.state {
                ChildState::Backoff { until } if now >= until => {
                    if let Some(session_id) = self.target {
                        self.spawn(session_id, &mut actions);
                    }
                }
                ChildState::Starting { pid, since, .. }
                    if now.duration_since(since) >= self.cfg.connect_timeout =>
                {
                    actions.push(SupAction::Kill { pid });
                    self.enter_backoff(now);
                }
                _ => {}
            },
            SupEvent::Shutdown => {
                if let Some(pid) = self.current_pid() {
                    actions.push(SupAction::Kill { pid });
                }
                self.set_state(ChildState::Stopped);
            }
        }
        actions
    }

    pub fn next_deadline(&self) -> Option<Instant> {
        match self.state {
            ChildState::Backoff { until } => Some(until),
            ChildState::Starting { since, .. } => Some(since + self.cfg.connect_timeout),
            _ => None,
        }
    }

    pub fn state(&self) -> &ChildState {
        &self.state
    }

    pub fn target(&self) -> Option<u32> {
        self.target
    }

    /// The delay to use for the next failure, before doubling it for a later one.
    pub fn backoff(&self) -> Duration {
        self.backoff
    }

    fn current_pid(&self) -> Option<u32> {
        match self.state {
            ChildState::Starting { pid, .. } | ChildState::Running { pid, .. } => Some(pid),
            _ => None,
        }
    }

    fn select_session(&mut self, session_id: Option<u32>, actions: &mut Vec<SupAction>) {
        match session_id.filter(|id| *id != NO_SESSION) {
            Some(session_id) => {
                if self.target != Some(session_id) {
                    self.target = Some(session_id);
                    self.backoff = self.cfg.backoff_initial;
                    self.spawn(session_id, actions);
                } else if self.state == ChildState::Idle {
                    self.spawn(session_id, actions);
                }
            }
            None => {
                self.target = None;
                if let Some(pid) = self.current_pid() {
                    actions.push(SupAction::Kill { pid });
                }
                self.set_state(ChildState::Idle);
            }
        }
    }

    fn spawn(&mut self, session_id: u32, actions: &mut Vec<SupAction>) {
        if let Some(pid) = self.current_pid() {
            actions.push(SupAction::Kill { pid });
        }
        actions.push(SupAction::Spawn { session_id });
        self.set_state(ChildState::Spawning { session_id });
    }

    fn enter_backoff(&mut self, now: Instant) {
        self.set_state(ChildState::Backoff {
            until: now + self.backoff,
        });
        self.backoff = self.backoff.saturating_mul(2).min(self.cfg.backoff_max);
    }

    fn set_state(&mut self, state: ChildState) {
        if self.state != state {
            self.state = state;
            tracing::debug!(state = ?self.state);
        }
    }
}
