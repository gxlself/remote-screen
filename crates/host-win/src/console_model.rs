//! Platform-independent mapping of Windows console facts.

use std::mem::size_of;

use host_core::platform::ConsoleState;

/// WTS session lock state.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum LockFlag {
    Locked,
    Unlocked,
    Unknown,
}

/// Translate the WTS session flag values into a small platform-neutral model.
pub const fn lock_flag(session_flags: i32) -> LockFlag {
    match session_flags {
        0 => LockFlag::Locked,
        1 => LockFlag::Unlocked,
        _ => LockFlag::Unknown,
    }
}

/// WTS uses all bits set while the active console is switching sessions.
pub fn active_session_id(raw: u32) -> Option<u32> {
    (raw != u32::MAX).then_some(raw)
}

/// A WTS wide-string buffer must contain a whole number of UTF-16 units.
pub fn wts_utf16_len(bytes: u32) -> Option<usize> {
    let bytes = usize::try_from(bytes).ok()?;
    bytes
        .is_multiple_of(size_of::<u16>())
        .then_some(bytes / size_of::<u16>())
}

/// Decode a bounded, terminated WTS username. Invalid or empty names are absent.
pub fn parse_wts_user(units: &[u16]) -> Option<String> {
    let end = units.iter().position(|&unit| unit == 0)?;
    String::from_utf16(&units[..end])
        .ok()
        .filter(|user| !user.is_empty())
}

/// Only level 1 of WTSINFOEX defines the session lock flag.
pub const fn wts_lock_flag(level: u32, session_flags: i32) -> LockFlag {
    if level == 1 {
        lock_flag(session_flags)
    } else {
        LockFlag::Unknown
    }
}

/// Facts collected from the active Windows console session.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ConsoleFacts {
    pub session_id: Option<u32>,
    pub user: Option<String>,
    pub lock: LockFlag,
}

/// Map Windows facts to the host-core console state.
///
/// Windows permits remote input at the login screen. Until host-core can
/// express that platform policy, report it as Locked: LoginWindow would cause
/// console_verdict to reject the connection.
pub fn classify(facts: &ConsoleFacts) -> ConsoleState {
    if facts.session_id.is_none() {
        return ConsoleState::Unknown;
    }

    let has_user = facts.user.as_deref().is_some_and(|user| !user.is_empty());
    if !has_user || facts.lock == LockFlag::Locked {
        ConsoleState::Locked
    } else {
        ConsoleState::LoggedIn
    }
}
