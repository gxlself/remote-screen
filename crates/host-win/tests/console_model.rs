use host_core::platform::ConsoleState;
use host_win::console_model::{
    active_session_id, classify, lock_flag, parse_wts_user, wts_lock_flag, wts_utf16_len,
    ConsoleFacts, LockFlag,
};

#[test]
fn lock_flags_are_mapped() {
    assert_eq!(lock_flag(0), LockFlag::Locked);
    assert_eq!(lock_flag(1), LockFlag::Unlocked);
    assert_eq!(lock_flag(-1), LockFlag::Unknown);
    assert_eq!(lock_flag(2), LockFlag::Unknown);
}

#[test]
fn console_facts_are_classified() {
    let facts = |session_id, user: Option<&str>, lock| ConsoleFacts {
        session_id,
        user: user.map(str::to_owned),
        lock,
    };

    assert_eq!(
        classify(&facts(None, None, LockFlag::Unknown)),
        ConsoleState::Unknown
    );
    assert_eq!(
        classify(&facts(Some(1), None, LockFlag::Unknown)),
        ConsoleState::Locked
    );
    assert_eq!(
        classify(&facts(Some(1), Some("alice"), LockFlag::Locked)),
        ConsoleState::Locked
    );
    assert_eq!(
        classify(&facts(Some(1), Some("alice"), LockFlag::Unlocked)),
        ConsoleState::LoggedIn
    );
    assert_eq!(
        classify(&facts(Some(1), Some("alice"), LockFlag::Unknown)),
        ConsoleState::LoggedIn
    );
    assert_eq!(
        classify(&facts(Some(1), Some(""), LockFlag::Unlocked)),
        ConsoleState::Locked
    );
}

#[test]
fn wts_data_is_normalized_without_windows() {
    assert_eq!(active_session_id(u32::MAX), None);
    assert_eq!(active_session_id(0), Some(0));
    assert_eq!(active_session_id(7), Some(7));
    assert_eq!(wts_utf16_len(0), Some(0));
    assert_eq!(wts_utf16_len(12), Some(6));
    assert_eq!(wts_utf16_len(3), None);
    let alice: Vec<u16> = "alice\0ignored".encode_utf16().collect();
    assert_eq!(parse_wts_user(&alice).as_deref(), Some("alice"));
    let unicode: Vec<u16> = "用户\0".encode_utf16().collect();
    assert_eq!(parse_wts_user(&unicode).as_deref(), Some("用户"));
    assert_eq!(parse_wts_user(&[]), None);
    assert_eq!(parse_wts_user(&[0]), None);
    assert_eq!(parse_wts_user(&[0x61]), None);
    assert_eq!(parse_wts_user(&[0xD800, 0]), None);
    assert_eq!(wts_lock_flag(1, 0), LockFlag::Locked);
    assert_eq!(wts_lock_flag(1, 1), LockFlag::Unlocked);
    assert_eq!(wts_lock_flag(1, -1), LockFlag::Unknown);
    assert_eq!(wts_lock_flag(0, 0), LockFlag::Unknown);
    assert_eq!(wts_lock_flag(2, 1), LockFlag::Unknown);
}
