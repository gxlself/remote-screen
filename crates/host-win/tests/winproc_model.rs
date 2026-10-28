use host_win::winproc_model::{
    command_line, console_session_id, desktop_name_from_utf16, nul_terminated_utf16, quote_arg,
    verify_client, verify_console_client, DesktopTracker, SESSION_PIPE_SDDL, UI_PIPE_SDDL,
};

#[test]
fn quote_windows_arguments() {
    for (arg, expected) in [
        ("abc", "abc"),
        ("a b", "\"a b\""),
        ("", "\"\""),
        (r#"a"b"#, r#""a\"b""#),
        (r"C:\Program Files\x\", r#""C:\Program Files\x\\""#),
        (r#"a\\"b"#, r#""a\\\\\"b""#),
        (r"a\b", r"a\b"),
        ("a\tb", "\"a\tb\""),
        ("a\nb", "\"a\nb\""),
        ("a\u{b}b", "\"a\u{b}b\""),
        ("a\rb", "a\rb"),
        (r"a\", r"a\"),
        (r"a\ b", r#""a\ b""#),
        ("中文 路径", "\"中文 路径\""),
    ] {
        assert_eq!(quote_arg(arg), expected, "argument: {arg:?}");
    }
}

#[test]
fn command_line_quotes_executable_and_each_argument() {
    assert_eq!(
        command_line(
            r"C:\Program Files\Remote Host\rs-host-session.exe",
            &["--session", "2"]
        ),
        "\"C:\\Program Files\\Remote Host\\rs-host-session.exe\" --session 2"
    );
    assert_eq!(command_line("app.exe", &[]), "app.exe");
    assert_eq!(
        command_line("app.exe", &["", "two words"]),
        "app.exe \"\" \"two words\""
    );
}

#[test]
fn only_the_expected_child_is_verified() {
    assert!(verify_client(Some(10), 10));
    assert!(!verify_client(Some(10), 11));
    assert!(!verify_client(None, 10));
}

#[test]
fn tracks_desktop_changes_across_failed_queries() {
    let mut tracker = DesktopTracker::new();
    assert_eq!(tracker.observe(None), None);
    assert_eq!(tracker.observe(Some("Default")), Some("Default".into()));
    assert_eq!(tracker.observe(Some("default")), None);
    assert_eq!(tracker.observe(Some("Winlogon")), Some("Winlogon".into()));
    assert_eq!(tracker.observe(None), None);
    assert_eq!(tracker.observe(Some("Winlogon")), None);
    assert_eq!(tracker.observe(Some("DEFAULT")), Some("DEFAULT".into()));
}

#[test]
fn pipe_acl_constants_are_restricted() {
    assert_eq!(SESSION_PIPE_SDDL, "D:P(A;;GA;;;SY)");
    assert_eq!(UI_PIPE_SDDL, "D:P(A;;GA;;;SY)(A;;0x12018b;;;S-1-2-1)");
}

#[test]
fn ui_client_must_match_an_existing_console_session() {
    assert_eq!(console_session_id(u32::MAX), None);
    assert_eq!(console_session_id(0), Some(0));
    assert_eq!(console_session_id(2), Some(2));
    assert!(verify_console_client(Some(2), 2));
    assert!(!verify_console_client(Some(2), 3));
    assert!(!verify_console_client(None, 2));
    assert!(!verify_console_client(None, u32::MAX));
    assert!(!verify_console_client(Some(u32::MAX), u32::MAX));
}

#[test]
fn win32_strings_preserve_unicode_and_reject_truncation() {
    assert_eq!(nul_terminated_utf16(""), Some(vec![0]));
    assert_eq!(nul_terminated_utf16("a\0b"), None);
    let wide = nul_terminated_utf16("桌面🖥").unwrap();
    assert_eq!(desktop_name_from_utf16(&wide), Some("桌面🖥".into()));
    assert_eq!(desktop_name_from_utf16(&[]), None);
    assert_eq!(desktop_name_from_utf16(&[0]), None);
    assert_eq!(desktop_name_from_utf16(&[65]), None);
    assert_eq!(desktop_name_from_utf16(&[0xd800, 0]), None);
    assert_eq!(desktop_name_from_utf16(&[65, 0, 66]), Some("A".into()));
}
