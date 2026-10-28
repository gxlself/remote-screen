use std::cell::RefCell;
use std::time::{Duration, Instant};

use host_win::user_dirs_model::{
    cache_fresh, choose, fallback_dir, forbidden_roots, is_forbidden, is_under, strip_verbatim,
    Choice, Fallback, Probe, CACHE_TTL,
};

const DATA: &str = r"C:\ProgramData\RemoteHost";
const DOWNLOADS: &str = r"C:\Users\a\Downloads";
const PROFILE: &str = r"C:\Users\a";

fn roots() -> Vec<String> {
    forbidden_roots(&|_| None, DATA)
}

fn user() -> Probe {
    Probe {
        console: Some(1),
        has_user: true,
        downloads: Some(DOWNLOADS.to_owned()),
        profile: Some(PROFILE.to_owned()),
        downloads_writable: true,
    }
}

#[test]
fn fallback_trims_trailing_backslashes() {
    for data in [
        DATA,
        r"C:\ProgramData\RemoteHost\",
        r"C:\ProgramData\RemoteHost\\",
    ] {
        assert_eq!(fallback_dir(data), r"C:\ProgramData\RemoteHost\received");
    }
}

#[test]
fn verbatim_drive_and_unc_paths() {
    for (path, expected) in [
        (r"\\?\C:\x", r"C:\x"),
        (r"\\?\UNC\srv\share\x", r"\\srv\share\x"),
        (r"\\?\unc\srv\share\x", r"\\srv\share\x"),
        (r"C:\x", r"C:\x"),
        (r"\\srv\s", r"\\srv\s"),
        (r"\\?\GLOBALROOT\Device\Mup", r"\\?\GLOBALROOT\Device\Mup"),
        (r"\\.\pipe\example", r"\\.\pipe\example"),
        (r"\\?\", r"\\?\"),
        ("", ""),
    ] {
        assert_eq!(strip_verbatim(path), expected);
    }
}

#[test]
fn prefixes_match_whole_case_insensitive_components() {
    for root in [r"C:\Windows", r"C:\Windows\"] {
        for path in [
            r"C:\Windows",
            r"c:\WINDOWS",
            r"c:/windows\Temp",
            r"C:\Windows\",
        ] {
            assert!(is_under(path, root), "{path} / {root}");
        }
        assert!(!is_under(r"C:\Windows2", root));
        assert!(!is_under(r"C:\Window", root));
    }
    assert!(is_under(r"C:\x\y", r"C:\"));
    assert!(is_under(r"c:\x/y", "C:/"));
    assert!(!is_under(r"D:\x", r"C:\"));
    assert!(!is_under(r"C:\x", ""));
}

#[test]
fn forbidden_defaults_and_data_directory() {
    assert_eq!(roots(), vec![r"C:\Windows", DATA]);
}

#[test]
fn forbidden_environment_is_exact_and_deduplicated() {
    let read = RefCell::new(Vec::new());
    let roots = forbidden_roots(
        &|name| {
            read.borrow_mut().push(name.to_owned());
            match name {
                "SystemRoot" | "ProgramFiles(x86)" => Some(String::new()),
                "ProgramFiles" => Some(r"C:\Program Files".to_owned()),
                "ProgramW6432" => Some(r"c:\PROGRAM FILES\".to_owned()),
                _ => panic!("unexpected environment variable"),
            }
        },
        DATA,
    );
    assert_eq!(roots, vec![r"C:\Program Files", DATA]);
    assert_eq!(
        *read.borrow(),
        [
            "SystemRoot",
            "ProgramFiles",
            "ProgramFiles(x86)",
            "ProgramW6432"
        ]
    );
}

#[test]
fn forbidden_retains_custom_system_and_program_roots() {
    let roots = forbidden_roots(
        &|name| match name {
            "SystemRoot" => Some(r"D:\Windows".to_owned()),
            "ProgramFiles(x86)" => Some(r"D:\Program Files (x86)".to_owned()),
            _ => None,
        },
        r"d:\WINDOWS\",
    );
    assert_eq!(roots, vec![r"D:\Windows", r"D:\Program Files (x86)"]);
}

#[test]
fn forbidden_rejects_unc_and_protected_descendants() {
    for path in [
        r"\\srv\share\Downloads",
        r"C:\Windows\Temp",
        r"C:\ProgramData\RemoteHost\received",
    ] {
        assert!(is_forbidden(path, &roots()), "{path}");
    }
    for path in [DOWNLOADS, r"D:\dl", r"C:\Windows2"] {
        assert!(!is_forbidden(path, &roots()), "{path}");
    }
    assert!(is_forbidden(r"\\srv\share\Downloads", &[]));
}

#[test]
fn no_console_takes_precedence_and_discards_home() {
    let mut probe = user();
    probe.console = None;
    assert_eq!(
        choose(&probe, &roots()),
        Choice::Fallback {
            reason: Fallback::NoConsole,
            home: None
        }
    );
}

#[test]
fn no_user_takes_precedence_and_discards_home() {
    let mut probe = user();
    probe.has_user = false;
    assert_eq!(
        choose(&probe, &roots()),
        Choice::Fallback {
            reason: Fallback::NoUser,
            home: None
        }
    );
}

#[test]
fn missing_profile_does_not_reject_downloads() {
    let mut probe = user();
    probe.profile = None;
    assert_eq!(
        choose(&probe, &roots()),
        Choice::User {
            downloads: DOWNLOADS.to_owned(),
            home: None
        }
    );
}

#[test]
fn missing_downloads_preserves_home() {
    let mut probe = user();
    probe.downloads = None;
    probe.downloads_writable = false;
    assert_eq!(
        choose(&probe, &roots()),
        Choice::Fallback {
            reason: Fallback::NoFolder,
            home: Some(PROFILE.to_owned())
        }
    );
}

#[test]
fn forbidden_downloads_precede_access_denial_and_preserve_home() {
    for downloads in [
        r"C:\Windows\Temp",
        r"\\srv\share\Downloads",
        r"C:\ProgramData\RemoteHost\received",
    ] {
        let mut probe = user();
        probe.downloads = Some(downloads.to_owned());
        probe.downloads_writable = false;
        assert_eq!(
            choose(&probe, &roots()),
            Choice::Fallback {
                reason: Fallback::Forbidden,
                home: Some(PROFILE.to_owned())
            }
        );
    }
}

#[test]
fn access_denial_preserves_home() {
    let mut probe = user();
    probe.downloads_writable = false;
    assert_eq!(
        choose(&probe, &roots()),
        Choice::Fallback {
            reason: Fallback::NotWritable,
            home: Some(PROFILE.to_owned())
        }
    );
}

#[test]
fn eligible_user_keeps_both_directories() {
    assert_eq!(
        choose(&user(), &roots()),
        Choice::User {
            downloads: DOWNLOADS.to_owned(),
            home: Some(PROFILE.to_owned())
        }
    );
}

#[test]
fn forbidden_profile_is_removed_without_rejecting_downloads() {
    for profile in [r"C:\Windows", DATA, r"\\srv\share\a"] {
        let mut probe = user();
        probe.profile = Some(profile.to_owned());
        assert_eq!(
            choose(&probe, &roots()),
            Choice::User {
                downloads: DOWNLOADS.to_owned(),
                home: None
            }
        );
        probe.downloads = None;
        assert_eq!(
            choose(&probe, &roots()),
            Choice::Fallback {
                reason: Fallback::NoFolder,
                home: None
            }
        );
    }
}

#[test]
fn cache_is_fresh_only_for_the_same_session_before_ttl() {
    let at = Instant::now();
    assert!(cache_fresh(1, at, 1, at, CACHE_TTL));
    assert!(cache_fresh(
        1,
        at,
        1,
        at + CACHE_TTL - Duration::from_nanos(1),
        CACHE_TTL
    ));
    assert!(!cache_fresh(1, at, 2, at, CACHE_TTL));
    assert!(!cache_fresh(1, at, 1, at + CACHE_TTL, CACHE_TTL));
    assert!(!cache_fresh(
        1,
        at,
        1,
        at + CACHE_TTL + Duration::from_secs(1),
        CACHE_TTL
    ));
    assert!(!cache_fresh(
        1,
        at + Duration::from_secs(1),
        1,
        at,
        CACHE_TTL
    ));
    assert!(!cache_fresh(1, at, 1, at, Duration::ZERO));
}
