use std::fs;
use std::path::PathBuf;
use std::time::Duration;

use host_win::supervisor::SessionChangeKind;
use host_win::svc_os_model::*;
use rsp_core::trust::Permission;

fn values(path: &str) -> DeployValues {
    DeployValues {
        pair_file: path.into(),
        permission: None,
        label: None,
        stamp: 123,
    }
}

#[test]
fn scm_control_mapping() {
    assert_eq!(map_control(1, 0, None), (Some(ScmEvent::Stop), 0));
    assert_eq!(map_control(5, 0, None), (Some(ScmEvent::Shutdown), 0));
    for (event_type, kind) in [
        (1, SessionChangeKind::ConsoleConnect),
        (7, SessionChangeKind::Lock),
        (8, SessionChangeKind::Unlock),
        (99, SessionChangeKind::Other(99)),
    ] {
        assert_eq!(
            map_control(14, event_type, Some(42)),
            (
                Some(ScmEvent::SessionChange {
                    kind,
                    session_id: 42
                }),
                0
            )
        );
    }
    assert_eq!(map_control(14, 7, None), (None, 0));
    assert_eq!(map_control(4, 0, None), (None, 0));
    assert_eq!(map_control(99, 0, Some(42)), (None, 120));
}

#[test]
fn scm_states_exit_codes_and_constants() {
    for (state, code, accepted) in [
        (ScmState::StartPending, 2, 0),
        (ScmState::Running, 4, ACCEPT_RUNNING),
        (ScmState::StopPending, 3, 0),
        (ScmState::Stopped, 1, 0),
    ] {
        assert_eq!(state_code(state), code);
        assert_eq!(controls_accepted(state), accepted);
    }
    assert_eq!(exit_codes(0), (0, 0));
    assert_eq!(exit_codes(7), (1066, 7));
    assert_eq!(ACCEPT_RUNNING, 1 | 4 | 0x80);
    assert_eq!(START_WAIT_HINT, Duration::from_secs(30));
    assert_eq!(STOP_WAIT_HINT, Duration::from_secs(15));
    assert_eq!(SERVICE_NAME, "RemoteHostService");
    assert_eq!(TRAY_EXE, "RemoteHost.exe");
    assert_eq!(DEPLOY_REG_KEY, r"SOFTWARE\Remote Host\Deploy");
    NoopStatus.running();
    NoopStatus.stopping(STOP_WAIT_HINT);
}

#[test]
fn image_names_compare_only_the_basename() {
    assert!(is_image("remotehost.EXE", "RemoteHost.exe"));
    assert!(is_image(
        r"C:\Program Files\Remote Host\RemoteHost.exe",
        TRAY_EXE
    ));
    assert!(is_image("C:/apps/RemoteHost.exe", r"D:\RemoteHost.EXE"));
    assert!(!is_image("RemoteHostService.exe", TRAY_EXE));
    assert!(!is_image("prefixRemoteHost.exe", TRAY_EXE));
}

#[test]
fn tray_decisions_and_precedence() {
    assert_eq!(
        decide_tray(Some(1), true, false, false),
        TrayDecision::Missing
    );
    assert_eq!(
        decide_tray(None, true, false, true),
        TrayDecision::NoSession
    );
    assert_eq!(
        decide_tray(Some(1), false, false, true),
        TrayDecision::NoUser
    );
    assert_eq!(
        decide_tray(Some(1), true, true, true),
        TrayDecision::AlreadyRunning
    );
    assert_eq!(
        decide_tray(Some(1), true, false, true),
        TrayDecision::Launch
    );
    assert_eq!(decide_tray(None, false, true, false), TrayDecision::Missing);
    assert_eq!(
        decide_tray(None, false, true, true),
        TrayDecision::NoSession
    );
    assert_eq!(
        decide_tray(Some(1), false, true, true),
        TrayDecision::NoUser
    );
}

#[test]
fn public_downloads_are_windows_paths_on_every_platform() {
    for public in [
        r"D:\Users\Public",
        r"D:\Users\Public\",
        r"D:\Users\Public\\",
    ] {
        assert_eq!(
            downloads_dir(Some(public)),
            PathBuf::from(r"D:\Users\Public\Downloads")
        );
    }
    for public in [None, Some("relative"), Some(""), Some(r"D:relative")] {
        assert_eq!(
            downloads_dir(public),
            PathBuf::from(r"C:\Users\Public\Downloads")
        );
    }
    assert_eq!(
        downloads_dir(Some(r"\\hs.example.com\public\")),
        PathBuf::from(r"\\hs.example.com\public\Downloads")
    );
}

#[test]
fn deploy_directories_and_files() {
    for path in [r"C:\x", r"C:\x\", r"  C:\x\  "] {
        let parsed = parse_deploy(&values(path)).unwrap();
        assert_eq!(parsed.dir, PathBuf::from(r"C:\x"));
        assert_eq!(parsed.file_name, None);
    }
    for (path, dir, name) in [
        (r"C:\x\host.RSPAIR", r"C:\x", Some("host.RSPAIR")),
        ("C:/x/host.rspair", "C:/x", Some("host.rspair")),
        (
            r"\\hs.example.com\share\host.rspair",
            r"\\hs.example.com\share",
            Some("host.rspair"),
        ),
        (r"\\hs.example.com\share", r"\\hs.example.com\share", None),
        (r"C:\host.rspair", r"C:\", Some("host.rspair")),
    ] {
        let parsed = parse_deploy(&values(path)).unwrap();
        assert_eq!(parsed.dir, PathBuf::from(dir));
        assert_eq!(parsed.file_name.as_deref(), name);
    }
}

#[test]
fn deploy_rejects_relative_paths_and_bad_filenames() {
    for path in ["deploy", "", r"C:deploy", r"\deploy", "/deploy"] {
        assert_eq!(parse_deploy(&values(path)), Err(DeployError::NotAbsolute));
    }
    for bad in [':', '*', '?', '"', '<', '>', '|', '\0', '\n'] {
        assert_eq!(
            parse_deploy(&values(&format!(r"C:\x\a{bad}.rspair"))),
            Err(DeployError::BadFileName)
        );
    }
}

#[test]
fn deploy_permissions() {
    for (raw, expected) in [
        (None, Permission::Control),
        (Some(""), Permission::Control),
        (Some("view_only"), Permission::ViewOnly),
        (Some("control"), Permission::Control),
        (Some("unattended"), Permission::Unattended),
    ] {
        let mut input = values(r"C:\x");
        input.permission = raw.map(str::to_owned);
        assert_eq!(parse_deploy(&input).unwrap().permission, expected);
    }
    let mut input = values(r"C:\x");
    input.permission = Some("admin".into());
    assert_eq!(
        parse_deploy(&input),
        Err(DeployError::BadPermission("admin".into()))
    );
}

#[test]
fn deploy_labels_count_unicode_characters() {
    let mut input = values(r"C:\x");
    input.label = Some("网".repeat(64));
    assert_eq!(parse_deploy(&input).unwrap().label, input.label);
    input.label = Some("网".repeat(65));
    assert_eq!(parse_deploy(&input), Err(DeployError::LabelTooLong));
    input.label = Some(String::new());
    assert_eq!(parse_deploy(&input).unwrap().label, None);
}

#[test]
fn fingerprint_changes_for_every_field_and_registry_rewrite() {
    let input = values(r"C:\x");
    let fingerprint = deploy_fingerprint(&input);
    // Reference SHA-256 of C:\x, three NUL separators and little-endian 123.
    assert_eq!(
        fingerprint,
        "7ce5c9d039ffcfa9f2419df2292c6a8cee9bbcca0fb2d247b3f4dfdeacf04cae"
    );
    assert_eq!(fingerprint, deploy_fingerprint(&input));
    assert_eq!(fingerprint.len(), 64);
    assert!(fingerprint
        .bytes()
        .all(|b| b.is_ascii_digit() || (b'a'..=b'f').contains(&b)));
    let mut changed = input.clone();
    changed.pair_file.push('y');
    assert_ne!(deploy_fingerprint(&changed), fingerprint);
    changed = input.clone();
    changed.permission = Some("control".into());
    assert_ne!(deploy_fingerprint(&changed), fingerprint);
    changed = input.clone();
    changed.label = Some("IT".into());
    assert_ne!(deploy_fingerprint(&changed), fingerprint);
    changed = input.clone();
    changed.stamp += 1;
    assert_ne!(deploy_fingerprint(&changed), fingerprint);
    changed = input.clone();
    changed.permission = Some(String::new());
    changed.label = Some(String::new());
    assert_eq!(deploy_fingerprint(&changed), fingerprint);
}

#[test]
fn export_requires_values_and_a_different_or_missing_marker() {
    let input = values(r"C:\x");
    let mut marker = DeployMarker {
        fingerprint: deploy_fingerprint(&input),
        file_id: "example-id".into(),
        path: r"C:\x\host.rspair".into(),
        exported_at_us: 123_000_000,
    };
    assert!(!needs_export(None, None));
    assert!(!needs_export(None, Some(&marker)));
    assert!(needs_export(Some(&input), None));
    assert!(!needs_export(Some(&input), Some(&marker)));
    marker.fingerprint.clear();
    assert!(needs_export(Some(&input), Some(&marker)));
}

#[test]
fn marker_roundtrip_replacement_corruption_and_temporary_cleanup() {
    let dir = std::env::temp_dir().join(format!("rs-svc-os-marker-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(DEPLOY_MARKER);
    assert_eq!(load_marker(&path), None);
    let mut marker = DeployMarker {
        fingerprint: deploy_fingerprint(&values(r"C:\x")),
        file_id: "example-id".into(),
        path: r"C:\x\host.rspair".into(),
        exported_at_us: 123_000_000,
    };
    save_marker(&path, &marker).unwrap();
    assert_eq!(load_marker(&path), Some(marker.clone()));
    marker.exported_at_us += 1;
    save_marker(&path, &marker).unwrap();
    assert_eq!(load_marker(&path), Some(marker));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::write(&path, b"broken JSON").unwrap();
    assert_eq!(load_marker(&path), None);
    fs::remove_dir_all(dir).unwrap();
}
