//! 会话名册：1 个控制者 + 最多 3 个观看者。
//!
//! This is a pure in-memory state machine. Callers provide unix timestamps in
//! microseconds and are responsible for generating structures only after the
//! roster accepts or resumes a participant.

use rsp_core::pb;

use crate::{auth, ipc};

/// How long a disconnected participant keeps their place in the roster.
pub const LINGER_US: u64 = 60_000_000;

/// Length of a `session_token` on the wire.
pub const TOKEN_LEN: usize = 16;

/// Caller-assigned identifier for one transport connection.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct ConnKey(pub u64);

/// Stable identifier for one admission to the roster.
///
/// The string is `s` followed by sixteen lowercase hexadecimal characters.
#[derive(Debug, Clone, PartialEq, Eq, Hash, PartialOrd, Ord)]
pub struct SessionId(pub String);

/// The role a participant currently holds in the roster.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RosterRole {
    Controller,
    Viewer,
}

/// Identity and routing information attached to a participant.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantInfo {
    pub device_id: String,
    pub device_name: String,
    /// WhoIs login name; empty on the intranet path.
    pub user: String,
    pub path: pb::PathKind,
    /// Whether the device and route allow control, independent of intent.
    pub can_control: bool,
}

/// Display and stream preferences retained across a resume.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionPrefs {
    pub display_id: u32,
    pub quality: i32,
}

/// Roster policy supplied by the caller.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RosterConfig {
    pub max_viewers: u8,
    pub linger_us: u64,
}

impl Default for RosterConfig {
    fn default() -> Self {
        Self {
            max_viewers: 3,
            linger_us: LINGER_US,
        }
    }
}

/// Result of admitting a new connection.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Admitted {
    pub session_id: SessionId,
    pub role: RosterRole,
    pub token: [u8; TOKEN_LEN],
    /// The active connection displaced by a same-device admission.
    pub replaced: Option<ConnKey>,
}

/// Result of resuming an existing admission.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Resumed {
    pub session_id: SessionId,
    pub role: RosterRole,
    pub token: [u8; TOKEN_LEN],
    pub prefs: SessionPrefs,
    pub replaced: Option<ConnKey>,
}

/// Result of a connection leaving the roster.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Left {
    pub session_id: SessionId,
    pub was_controller: bool,
    pub lingering: bool,
}

/// A lingering admission removed by [`Roster::tick`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Expired {
    pub session_id: SessionId,
    pub device_id: String,
    pub was_controller: bool,
}

/// The controller transition performed by [`Roster::set_controller`].
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Handoff {
    pub from: Option<SessionId>,
    pub to: SessionId,
}

/// Public view of one roster entry.
#[derive(Debug, Clone, PartialEq)]
pub struct ParticipantView {
    pub session_id: SessionId,
    pub conn: Option<ConnKey>,
    pub info: ParticipantInfo,
    pub role: RosterRole,
    pub since_us: u64,
    pub lingering: bool,
    pub linger_deadline_us: Option<u64>,
}

/// Errors returned by roster operations.
#[derive(Debug, thiserror::Error, Clone, PartialEq, Eq)]
pub enum RosterError {
    #[error("no viewer slots are available")]
    SessionFull,
    #[error("session is not active")]
    UnknownSession,
    #[error("session token is invalid")]
    InvalidToken,
    #[error("session token belongs to a different device")]
    TokenDeviceMismatch,
    #[error("device or path is view-only")]
    ViewOnlyDevice,
    #[error("random number generation failed")]
    Random,
}

#[derive(Debug, Clone, PartialEq)]
struct Entry {
    session_id: SessionId,
    conn: Option<ConnKey>,
    info: ParticipantInfo,
    role: RosterRole,
    since_us: u64,
    prefs: SessionPrefs,
    token: [u8; TOKEN_LEN],
    linger_deadline_us: Option<u64>,
    recording: bool,
}

impl Entry {
    fn is_lingering(&self) -> bool {
        self.linger_deadline_us.is_some()
    }

    fn is_active(&self) -> bool {
        self.conn.is_some()
    }

    fn view(&self) -> ParticipantView {
        let lingering = self.is_lingering();
        ParticipantView {
            session_id: self.session_id.clone(),
            conn: if lingering { None } else { self.conn },
            info: self.info.clone(),
            role: self.role,
            since_us: self.since_us,
            lingering,
            linger_deadline_us: self.linger_deadline_us,
        }
    }

    fn pb_participant(&self) -> pb::Participant {
        pb::Participant {
            device_id: self.info.device_id.clone(),
            device_name: self.info.device_name.clone(),
            user: self.info.user.clone(),
            path: self.info.path as i32,
            since_us: self.since_us,
        }
    }
}

/// The roster for one host.
#[derive(Debug, Clone, PartialEq)]
pub struct Roster {
    config: RosterConfig,
    entries: Vec<Entry>,
    last_controller_user: Option<String>,
}

impl Default for Roster {
    fn default() -> Self {
        Self::new(RosterConfig::default())
    }
}

impl Roster {
    pub fn new(config: RosterConfig) -> Self {
        Self {
            config,
            entries: Vec::new(),
            last_controller_user: None,
        }
    }

    /// Change the viewer limit for future admissions. Existing viewers remain.
    pub fn set_max_viewers(&mut self, n: u8) {
        self.config.max_viewers = n.clamp(1, 3);
    }

    /// Build the authentication snapshot for one device.
    ///
    /// The device's own active or lingering entry is excluded so a reconnect
    /// does not count itself against capacity or the controller seat.
    pub fn auth_snapshot(&self, device_id: &str) -> auth::SessionSnapshot {
        let mut snapshot = auth::SessionSnapshot::default();
        for entry in &self.entries {
            if entry.info.device_id == device_id {
                continue;
            }
            match entry.role {
                RosterRole::Controller => snapshot.has_controller = true,
                RosterRole::Viewer if entry.is_active() => {
                    snapshot.viewer_count = snapshot.viewer_count.saturating_add(1);
                }
                RosterRole::Viewer => {}
            }
        }
        snapshot
    }

    /// Admit a connection after authentication has succeeded.
    pub fn admit(
        &mut self,
        conn: ConnKey,
        info: ParticipantInfo,
        requested: RosterRole,
        now_us: u64,
    ) -> Result<Admitted, RosterError> {
        let replaced_index = self
            .entries
            .iter()
            .position(|entry| entry.info.device_id == info.device_id);
        let replaced_role = replaced_index.map(|index| self.entries[index].role);

        let inherits_controller = replaced_role == Some(RosterRole::Controller) && info.can_control;
        let requests_free_controller = requested == RosterRole::Controller
            && info.can_control
            && !self.has_controller_except(replaced_index);
        let role = if inherits_controller || requests_free_controller {
            RosterRole::Controller
        } else {
            RosterRole::Viewer
        };

        if role == RosterRole::Viewer {
            let active_viewers = self
                .entries
                .iter()
                .enumerate()
                .filter(|(index, entry)| {
                    Some(*index) != replaced_index
                        && entry.role == RosterRole::Viewer
                        && entry.is_active()
                })
                .count();
            if active_viewers >= usize::from(self.config.max_viewers) {
                return Err(RosterError::SessionFull);
            }
        }

        let session_id = self.fresh_session_id()?;
        let token = self.fresh_token()?;

        let replaced = match replaced_index {
            Some(index) => {
                let old = self.entries.remove(index);
                let replaced = old.conn;
                self.note_ended_controller(&old);
                replaced
            }
            None => None,
        };

        if role == RosterRole::Controller {
            self.last_controller_user = None;
        }
        self.entries.push(Entry {
            session_id: session_id.clone(),
            conn: Some(conn),
            info,
            role,
            since_us: now_us,
            prefs: SessionPrefs::default(),
            token,
            linger_deadline_us: None,
            recording: false,
        });

        Ok(Admitted {
            session_id,
            role,
            token,
            replaced,
        })
    }

    /// Resume an existing admission using its current one-time token.
    pub fn resume(
        &mut self,
        conn: ConnKey,
        token: &[u8],
        device_id: &str,
        now_us: u64,
    ) -> Result<Resumed, RosterError> {
        if token.len() != TOKEN_LEN {
            return Err(RosterError::InvalidToken);
        }
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.token.as_slice() == token)
        else {
            return Err(RosterError::InvalidToken);
        };

        if let Some(deadline) = self.entries[index].linger_deadline_us {
            if now_us >= deadline {
                let expired = self.entries.remove(index);
                self.note_ended_controller(&expired);
                return Err(RosterError::InvalidToken);
            }
        }

        if self.entries[index].info.device_id != device_id {
            return Err(RosterError::TokenDeviceMismatch);
        }

        let was_active = self.entries[index].is_active();
        let role = self.entries[index].role;
        if role == RosterRole::Viewer && !was_active {
            let active_viewers = self
                .entries
                .iter()
                .filter(|entry| entry.role == RosterRole::Viewer && entry.is_active())
                .count();
            if active_viewers >= usize::from(self.config.max_viewers) {
                return Err(RosterError::SessionFull);
            }
        }

        let new_token = self.fresh_token()?;
        let entry = &mut self.entries[index];
        let replaced = if was_active { entry.conn } else { None };
        entry.conn = Some(conn);
        entry.linger_deadline_us = None;
        entry.token = new_token;

        let resumed = Resumed {
            session_id: entry.session_id.clone(),
            role,
            token: new_token,
            prefs: entry.prefs,
            replaced,
        };
        if role == RosterRole::Controller {
            self.last_controller_user = None;
        }
        Ok(resumed)
    }

    /// Record that a connection left. `linger` keeps the entry for later
    /// resume; otherwise the entry is removed immediately.
    pub fn leave(&mut self, conn: ConnKey, linger: bool, now_us: u64) -> Option<Left> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.conn == Some(conn))?;
        let was_controller = self.entries[index].role == RosterRole::Controller;
        let session_id = self.entries[index].session_id.clone();

        if linger {
            let entry = &mut self.entries[index];
            entry.conn = None;
            entry.linger_deadline_us = Some(now_us.saturating_add(self.config.linger_us));
            Some(Left {
                session_id,
                was_controller,
                lingering: true,
            })
        } else {
            let entry = self.entries.remove(index);
            self.note_ended_controller(&entry);
            Some(Left {
                session_id,
                was_controller,
                lingering: false,
            })
        }
    }

    /// Remove all lingering entries whose deadline has been reached.
    pub fn tick(&mut self, now_us: u64) -> Vec<Expired> {
        let mut expired = Vec::new();
        let mut index = 0;
        while index < self.entries.len() {
            let deadline = self.entries[index].linger_deadline_us;
            if matches!(deadline, Some(deadline) if deadline <= now_us) {
                let entry = self.entries.remove(index);
                let was_controller = entry.role == RosterRole::Controller;
                self.note_ended_controller(&entry);
                expired.push(Expired {
                    session_id: entry.session_id,
                    device_id: entry.info.device_id,
                    was_controller,
                });
            } else {
                index += 1;
            }
        }
        expired
    }

    /// Earliest linger deadline, if any participant is lingering.
    pub fn next_deadline(&self) -> Option<u64> {
        self.entries
            .iter()
            .filter_map(|entry| entry.linger_deadline_us)
            .min()
    }

    /// Move control to an active participant.
    pub fn set_controller(&mut self, session: &SessionId) -> Result<Handoff, RosterError> {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.session_id == *session && entry.is_active())
        else {
            return Err(RosterError::UnknownSession);
        };
        if !self.entries[index].info.can_control {
            return Err(RosterError::ViewOnlyDevice);
        }
        if self.entries[index].role == RosterRole::Controller {
            return Ok(Handoff {
                from: None,
                to: session.clone(),
            });
        }

        let previous_controller = self
            .entries
            .iter()
            .position(|entry| entry.role == RosterRole::Controller);
        let from = previous_controller.map(|old| self.entries[old].session_id.clone());
        if let Some(old) = previous_controller {
            self.entries[old].role = RosterRole::Viewer;
        }
        self.entries[index].role = RosterRole::Controller;
        self.last_controller_user = None;
        Ok(Handoff {
            from,
            to: session.clone(),
        })
    }

    /// Remove control permission from a participant.
    pub fn set_view_only(&mut self, session: &SessionId) -> Result<(), RosterError> {
        let Some(index) = self
            .entries
            .iter()
            .position(|entry| entry.session_id == *session)
        else {
            return Err(RosterError::UnknownSession);
        };

        if self.entries[index].role == RosterRole::Controller && self.entries[index].is_active() {
            let active_viewers = self
                .entries
                .iter()
                .filter(|entry| entry.role == RosterRole::Viewer && entry.is_active())
                .count();
            if active_viewers >= usize::from(self.config.max_viewers) {
                return Err(RosterError::SessionFull);
            }
            self.entries[index].role = RosterRole::Viewer;
        }
        self.entries[index].info.can_control = false;
        Ok(())
    }

    /// Permanently remove a participant and return its active connection.
    pub fn remove(&mut self, session: &SessionId) -> Option<ConnKey> {
        let index = self
            .entries
            .iter()
            .position(|entry| entry.session_id == *session)?;
        let entry = self.entries.remove(index);
        self.note_ended_controller(&entry);
        entry.conn
    }

    pub fn sessions_for_device(&self, device_id: &str) -> Vec<SessionId> {
        let mut sessions: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.info.device_id == device_id)
            .map(|entry| entry.session_id.clone())
            .collect();
        sessions.sort();
        sessions
    }

    pub fn set_prefs(
        &mut self,
        session: &SessionId,
        prefs: SessionPrefs,
    ) -> Result<(), RosterError> {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.session_id == *session)
        else {
            return Err(RosterError::UnknownSession);
        };
        entry.prefs = prefs;
        Ok(())
    }

    /// M3 V5：App 告知是否在本地录制。返回是否有变化；会话不存在返回 false。
    pub fn set_recording(&mut self, session: &SessionId, recording: bool) -> bool {
        let Some(entry) = self
            .entries
            .iter_mut()
            .find(|entry| entry.session_id == *session)
        else {
            return false;
        };
        let changed = entry.recording != recording;
        entry.recording = recording;
        changed
    }

    pub fn conn_of(&self, session: &SessionId) -> Option<ConnKey> {
        self.entries
            .iter()
            .find(|entry| entry.session_id == *session)
            .and_then(|entry| entry.conn)
    }

    pub fn session_of(&self, conn: ConnKey) -> Option<SessionId> {
        self.entries
            .iter()
            .find(|entry| entry.conn == Some(conn))
            .map(|entry| entry.session_id.clone())
    }

    pub fn role_of(&self, session: &SessionId) -> Option<RosterRole> {
        self.entries
            .iter()
            .find(|entry| entry.session_id == *session)
            .map(|entry| entry.role)
    }

    /// Look up an active session by token without consuming the token.
    pub fn by_token(&self, token: &[u8]) -> Option<SessionId> {
        if token.len() != TOKEN_LEN {
            return None;
        }
        self.entries
            .iter()
            .find(|entry| entry.is_active() && entry.token.as_slice() == token)
            .map(|entry| entry.session_id.clone())
    }

    pub fn controller(&self) -> Option<ParticipantView> {
        self.entries
            .iter()
            .find(|entry| entry.role == RosterRole::Controller && entry.is_active())
            .map(Entry::view)
    }

    pub fn lingering_controller(&self) -> Option<ParticipantView> {
        self.entries
            .iter()
            .find(|entry| entry.role == RosterRole::Controller && entry.is_lingering())
            .map(Entry::view)
    }

    /// User of the most recently ended controller, until a new controller appears.
    pub fn last_controller_user(&self) -> Option<String> {
        self.last_controller_user.clone()
    }

    /// All active and lingering participants, ordered by `since_us` then id.
    pub fn participants(&self) -> Vec<ParticipantView> {
        let mut views: Vec<_> = self.entries.iter().map(Entry::view).collect();
        views.sort_by(|left, right| {
            left.since_us
                .cmp(&right.since_us)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        views
    }

    /// Build the `SessionState` sent to one participant.
    pub fn session_state_for(
        &self,
        session: &SessionId,
        input_paused_local: bool,
    ) -> Option<pb::SessionState> {
        let entry = self
            .entries
            .iter()
            .find(|entry| entry.session_id == *session)?;
        let controller = self
            .entries
            .iter()
            .find(|entry| entry.role == RosterRole::Controller && entry.is_active())
            .map(Entry::pb_participant);
        let mut viewers: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.role == RosterRole::Viewer && entry.is_active())
            .collect();
        viewers.sort_by(|left, right| {
            left.since_us
                .cmp(&right.since_us)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        Some(pb::SessionState {
            your_role: pb_role(entry.role),
            controller,
            viewers: viewers.into_iter().map(Entry::pb_participant).collect(),
            input_paused_local,
        })
    }

    /// Active sessions in the shape used by IPC and heartbeats.
    pub fn summaries(&self) -> Vec<ipc::SessionSummary> {
        let mut entries: Vec<_> = self
            .entries
            .iter()
            .filter(|entry| entry.is_active())
            .collect();
        entries.sort_by(|left, right| {
            left.since_us
                .cmp(&right.since_us)
                .then_with(|| left.session_id.cmp(&right.session_id))
        });
        entries
            .into_iter()
            .map(|entry| ipc::SessionSummary {
                session_id: entry.session_id.0.clone(),
                device_id: entry.info.device_id.clone(),
                device_name: entry.info.device_name.clone(),
                role: match entry.role {
                    RosterRole::Controller => ipc::SessionRole::Controller,
                    RosterRole::Viewer => ipc::SessionRole::Viewer,
                },
                connected_at_us: entry.since_us,
                recording: entry.recording,
            })
            .collect()
    }

    fn has_controller_except(&self, excluded: Option<usize>) -> bool {
        self.entries
            .iter()
            .enumerate()
            .any(|(index, entry)| Some(index) != excluded && entry.role == RosterRole::Controller)
    }

    fn fresh_session_id(&self) -> Result<SessionId, RosterError> {
        for _ in 0..8 {
            let mut bytes = [0_u8; 8];
            getrandom::fill(&mut bytes).map_err(|_| RosterError::Random)?;
            let id = SessionId(hex_session_id(bytes));
            if !self.entries.iter().any(|entry| entry.session_id == id) {
                return Ok(id);
            }
        }
        Err(RosterError::Random)
    }

    fn fresh_token(&self) -> Result<[u8; TOKEN_LEN], RosterError> {
        for _ in 0..8 {
            let mut token = [0_u8; TOKEN_LEN];
            getrandom::fill(&mut token).map_err(|_| RosterError::Random)?;
            if !self.entries.iter().any(|entry| entry.token == token) {
                return Ok(token);
            }
        }
        Err(RosterError::Random)
    }

    fn note_ended_controller(&mut self, entry: &Entry) {
        if entry.role == RosterRole::Controller {
            self.last_controller_user = Some(entry.info.user.clone());
        }
    }
}

fn pb_role(role: RosterRole) -> i32 {
    match role {
        RosterRole::Controller => pb::Role::Controller as i32,
        RosterRole::Viewer => pb::Role::Viewer as i32,
    }
}

/// Build the wire participant for a public roster view.
pub fn participant_pb(view: &ParticipantView) -> pb::Participant {
    pb::Participant {
        device_id: view.info.device_id.clone(),
        device_name: view.info.device_name.clone(),
        user: view.info.user.clone(),
        path: view.info.path as i32,
        since_us: view.since_us,
    }
}

fn hex_session_id(bytes: [u8; 8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut id = String::with_capacity(17);
    id.push('s');
    for byte in bytes {
        id.push(HEX[usize::from(byte >> 4)] as char);
        id.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    id
}
