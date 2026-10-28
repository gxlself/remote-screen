use host_core::sysquery::{
    parse_fdesetup_status, parse_print_disabled, screen_sharing_state, LaunchdOverride,
};

#[test]
fn fdesetup_status_reports_on_and_off() {
    assert_eq!(parse_fdesetup_status("FileVault is On.\n"), Some(true));
    assert_eq!(parse_fdesetup_status("FileVault is Off.\n"), Some(false));
    assert_eq!(
        parse_fdesetup_status(
            "FileVault is On.\nEncryption in progress: Percent completed = 34.5\n"
        ),
        Some(true)
    );
    assert_eq!(
        parse_fdesetup_status(
            "FileVault is On.\nDecryption in progress: Percent completed = 80.0\n"
        ),
        Some(true)
    );
    assert_eq!(
        parse_fdesetup_status("FileVault is Off, but will be enabled after the next restart.\n"),
        Some(false)
    );
}

#[test]
fn fdesetup_status_rejects_unknown_output() {
    assert_eq!(parse_fdesetup_status(""), None);
    assert_eq!(parse_fdesetup_status("Error: ..."), None);
    assert_eq!(parse_fdesetup_status("filevault is on"), None);
}

#[test]
fn fdesetup_status_trims_leading_whitespace() {
    assert_eq!(parse_fdesetup_status("  FileVault is On.\n"), Some(true));
}

#[test]
fn print_disabled_finds_exact_label() {
    let output = "disabled services = {\n\t\"com.apple.screensharing\" => disabled\n\t\"com.apple.ftp-proxy\" => enabled\n}\n";
    assert_eq!(
        parse_print_disabled(output, "com.apple.screensharing"),
        Some(LaunchdOverride::Disabled)
    );
    assert_eq!(
        parse_print_disabled(
            "\t\"com.apple.screensharing\" => enabled\n",
            "com.apple.screensharing"
        ),
        Some(LaunchdOverride::Enabled)
    );
    assert_eq!(
        parse_print_disabled(
            "\"com.apple.screensharing\" => true\n",
            "com.apple.screensharing"
        ),
        Some(LaunchdOverride::Disabled)
    );
    assert_eq!(
        parse_print_disabled(
            "\"com.apple.screensharing\" => false\n",
            "com.apple.screensharing"
        ),
        Some(LaunchdOverride::Enabled)
    );
}

#[test]
fn print_disabled_rejects_partial_labels_and_unknown_values() {
    assert_eq!(
        parse_print_disabled(
            "\"com.apple.screensharing.agent\" => enabled\n",
            "com.apple.screensharing"
        ),
        None
    );
    assert_eq!(parse_print_disabled("", "com.apple.screensharing"), None);
    assert_eq!(
        parse_print_disabled(
            "\"com.apple.screensharing\" => maybe\n",
            "com.apple.screensharing"
        ),
        None
    );
}

#[test]
fn screen_sharing_state_prefers_override() {
    assert_eq!(
        screen_sharing_state(Some(LaunchdOverride::Enabled), Some(false)),
        Some(true)
    );
    assert_eq!(
        screen_sharing_state(Some(LaunchdOverride::Disabled), Some(true)),
        Some(false)
    );
}

#[test]
fn screen_sharing_state_falls_back_to_loaded_service() {
    assert_eq!(screen_sharing_state(None, Some(true)), Some(true));
    assert_eq!(screen_sharing_state(None, Some(false)), Some(false));
    assert_eq!(screen_sharing_state(None, None), None);
}
