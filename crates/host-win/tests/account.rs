use host_win::account::{
    canonical_short_name, is_admin, map_logon_error, parse_account, password_utf16,
    pick_by_full_name, AccountInput, LogonOutcome,
};
use zeroize::Zeroizing;

#[test]
fn parses_account_forms() {
    assert_eq!(
        parse_account("alice", "OFFICE-WIN"),
        Some(AccountInput::Local {
            user: "alice".into()
        })
    );
    assert_eq!(
        parse_account(r".\alice", "OFFICE-WIN"),
        Some(AccountInput::Local {
            user: "alice".into()
        })
    );
    assert_eq!(
        parse_account(r"office-win\alice", "OFFICE-WIN"),
        Some(AccountInput::Local {
            user: "alice".into()
        })
    );
    assert_eq!(
        parse_account(r"CORP\alice", "OFFICE-WIN"),
        Some(AccountInput::Domain {
            domain: "CORP".into(),
            user: "alice".into()
        })
    );
    assert_eq!(
        parse_account("alice@corp.example.com", "OFFICE-WIN"),
        Some(AccountInput::Upn {
            upn: "alice@corp.example.com".into()
        })
    );
    assert_eq!(
        parse_account("  alice  ", "OFFICE-WIN"),
        Some(AccountInput::Local {
            user: "alice".into()
        })
    );
}

#[test]
fn rejects_invalid_account_forms() {
    for input in [
        "", "   ", r"CORP\", "@corp", "alice@", "a/b", "a*b", "a\u{7}b",
    ] {
        assert_eq!(parse_account(input, "OFFICE-WIN"), None, "{input:?}");
    }
    assert_eq!(parse_account(&"a".repeat(257), "OFFICE-WIN"), None);
}

#[test]
fn canonicalizes_domain_name() {
    assert_eq!(
        canonical_short_name(&AccountInput::Domain {
            domain: "CORP".into(),
            user: "alice".into(),
        }),
        r"CORP\alice"
    );
    assert_eq!(
        canonical_short_name(&AccountInput::Local {
            user: "alice".into(),
        }),
        "alice"
    );
    assert_eq!(
        canonical_short_name(&AccountInput::Upn {
            upn: "alice@corp.example.com".into(),
        }),
        "alice@corp.example.com"
    );
}

#[test]
fn maps_logon_errors() {
    for code in [1326, 1327, 1330, 1331, 1907, 1909] {
        assert_eq!(map_logon_error(code, false), LogonOutcome::WrongPassword);
        assert_eq!(map_logon_error(code, true), LogonOutcome::WrongPassword);
    }
    assert_eq!(map_logon_error(1385, false), LogonOutcome::TryInteractive);
    assert_eq!(map_logon_error(1385, true), LogonOutcome::Unavailable);
    for code in [1311, 1355, 5] {
        assert_eq!(map_logon_error(code, false), LogonOutcome::Unavailable);
        assert_eq!(map_logon_error(code, true), LogonOutcome::Unavailable);
    }
}

#[test]
fn detects_builtin_administrators_sid() {
    assert!(is_admin(&["S-1-5-32-545".into(), "S-1-5-32-544".into()]));
    assert!(!is_admin(&["S-1-5-32-545".into()]));
    assert!(!is_admin(&[]));
}

#[test]
fn picks_unique_full_name_case_insensitively() {
    let candidates = vec![
        ("xl".into(), "Xiao Long".into()),
        ("zs".into(), "Zhang San".into()),
    ];
    assert_eq!(pick_by_full_name(&candidates, "xiao long"), Some("xl"));
    assert_eq!(pick_by_full_name(&candidates, " Xiao Long "), Some("xl"));
    let duplicate = vec![("a".into(), "Test".into()), ("b".into(), "Test".into())];
    assert_eq!(pick_by_full_name(&duplicate, "Test"), None);
    assert_eq!(pick_by_full_name(&candidates, "nobody"), None);
    assert_eq!(pick_by_full_name(&candidates, ""), None);
    assert_eq!(pick_by_full_name(&candidates, " \t "), None);
    assert_eq!(pick_by_full_name(&[], "Xiao Long"), None);
    let unicode = vec![("alice".into(), "  Élodie  ".into())];
    assert_eq!(pick_by_full_name(&unicode, "élodie"), Some("alice"));
    let duplicate = vec![("a".into(), "Test".into()), ("b".into(), " test ".into())];
    assert_eq!(pick_by_full_name(&duplicate, "TEST"), None);
}

#[test]
fn converts_password_to_zeroized_utf16() {
    let _: Option<Zeroizing<Vec<u16>>> = password_utf16(b"x");
    assert_eq!(
        password_utf16("pé".as_bytes()).map(|v| v.to_vec()),
        Some(vec![0x70, 0xe9, 0])
    );
    assert_eq!(password_utf16(&[0xff]), None);
    assert_eq!(password_utf16(b"a\0b"), None);
    assert_eq!(password_utf16(b"").map(|v| v.to_vec()), Some(vec![0]));
    assert_eq!(
        password_utf16("🔒".as_bytes()).map(|v| v.to_vec()),
        Some(vec![0xd83d, 0xdd12, 0])
    );
}

#[test]
fn rejects_forbidden_characters_in_each_account_component() {
    for character in [
        '/', '"', '[', ']', ':', ';', '|', '=', ',', '+', '*', '?', '<', '>', '\0', '\n', '\u{7f}',
    ] {
        for input in [
            format!("a{character}b"),
            format!(r"CORP\a{character}b"),
            format!(r"CO{character}RP\alice"),
            format!("a{character}b@corp.example.com"),
        ] {
            assert_eq!(parse_account(&input, "OFFICE-WIN"), None, "{input:?}");
        }
    }
    assert_eq!(parse_account(r"CORP\team\alice", "OFFICE-WIN"), None);
    assert_eq!(parse_account(r"\alice", "OFFICE-WIN"), None);
}

#[test]
fn account_limit_counts_characters_after_trimming() {
    assert!(parse_account(&"a".repeat(256), "OFFICE-WIN").is_some());
    assert!(parse_account(&format!("  {}  ", "名".repeat(256)), "OFFICE-WIN").is_some());
    assert_eq!(parse_account(&"名".repeat(257), "OFFICE-WIN"), None);
    assert_eq!(
        parse_account(r"ÉCRAN\alice", "écran"),
        Some(AccountInput::Local {
            user: "alice".into()
        })
    );
}
