//! Console/login/lock state from `CGSessionCopyCurrentDictionary`.

use host_core::platform::ConsoleState;

extern "C" {
    fn rs_console_session(on_console: *mut i32, screen_locked: *mut i32) -> i32;
}

/// Pure mapping used by the native query and by all console-state tests.
/// `None` for the outer value means that no graphical session dictionary was
/// available (the login window). A missing lock key is represented as
/// `Some(false)` by the native shim because the key is omitted while unlocked;
/// a missing on-console key or an unrecognized value remains Unknown.
pub fn classify_console_state(session: Option<(Option<bool>, Option<bool>)>) -> ConsoleState {
    let Some((on_console, locked)) = session else {
        return ConsoleState::LoginWindow;
    };
    match on_console {
        Some(false) => ConsoleState::OtherUser,
        Some(true) => match locked {
            Some(true) => ConsoleState::Locked,
            Some(false) => ConsoleState::LoggedIn,
            None => ConsoleState::Unknown,
        },
        None => ConsoleState::Unknown,
    }
}

pub fn state() -> ConsoleState {
    let (mut on_console, mut screen_locked) = (-1, -1);
    // SAFETY: both output pointers are valid for the duration of the shim call.
    let available = unsafe { rs_console_session(&mut on_console, &mut screen_locked) } != 0;
    if !available {
        return ConsoleState::LoginWindow;
    }
    classify_console_state(Some((
        (on_console >= 0).then_some(on_console != 0),
        (screen_locked >= 0).then_some(screen_locked != 0),
    )))
}
