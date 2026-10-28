use proptest::prelude::*;
use rsp_core::file_name::{
    candidate, is_safe_component, parse_partial_name, partial_meta_name, partial_name, sanitize,
    NameError, PartialKind, MAX_CANDIDATES, MAX_NAME_BYTES,
};

#[test]
fn sanitize_path_components_and_empty_names() {
    let cases = [
        ("../../etc/passwd", "passwd"),
        ("/abs/path/x.txt", "x.txt"),
        ("..\\..\\win.ini", "win.ini"),
        ("a/b/", "b"),
        ("/", "file"),
        ("..", "file"),
        (".", "file"),
        ("", "file"),
    ];

    for (input, expected) in cases {
        assert_eq!(sanitize(input), expected, "input: {input:?}");
    }
}

#[test]
fn sanitize_hidden_and_windows_reserved_names() {
    let cases = [
        (".ssh", "_.ssh"),
        (".bashrc", "_.bashrc"),
        ("con.txt", "_con.txt"),
        ("LPT9", "_LPT9"),
        ("console.txt", "console.txt"),
        ("  .con  ", "_.con"),
        ("NUL.tar.gz", "_NUL.tar.gz"),
    ];

    for (input, expected) in cases {
        assert_eq!(sanitize(input), expected, "input: {input:?}");
    }
}

#[test]
fn sanitize_reserved_invisible_and_whitespace_characters() {
    let cases = [
        ("a:b?.txt", "a_b_.txt"),
        ("name. . ", "name"),
        ("  spaced  ", "spaced"),
        ("evil\u{202E}txt.exe", "eviltxt.exe"),
        ("a\u{0000}b", "ab"),
        ("tab\there", "tabhere"),
        ("zero\u{200B}width", "zerowidth"),
        ("bom\u{FEFF}name", "bomname"),
        ("c1\u{0085}name", "c1name"),
        ("del\u{007F}name", "delname"),
        ("ltr\u{200E}mark", "ltrmark"),
        ("iso\u{2067}late", "isolate"),
        ("a\u{3000}.", "a"),
        ("a.\u{3000}", "a"),
        ("<quote>|\"*", "_quote____"),
    ];

    for (input, expected) in cases {
        assert_eq!(sanitize(input), expected, "input: {input:?}");
    }
}

#[test]
fn sanitize_preserves_unicode() {
    assert_eq!(sanitize("报告 2026.pdf"), "报告 2026.pdf");
    assert_eq!(sanitize("😀.png"), "😀.png");
}

#[test]
fn sanitize_truncates_long_names_at_character_boundaries() {
    let with_extension = format!("{}.txt", "汉".repeat(300));
    let result = sanitize(&with_extension);
    assert!(result.len() <= MAX_NAME_BYTES);
    assert!(result.ends_with(".txt"));
    assert!(std::str::from_utf8(result.as_bytes()).is_ok());
    assert!(result
        .trim_end_matches(".txt")
        .chars()
        .all(|character| character == '汉'));

    let without_extension = "a".repeat(300);
    let result = sanitize(&without_extension);
    assert_eq!(result.len(), MAX_NAME_BYTES);
    assert_eq!(result, "a".repeat(MAX_NAME_BYTES));
}

#[test]
fn candidate_inserts_suffix_before_last_dot() {
    let cases = [
        ("report.pdf", 1, "report (1).pdf"),
        ("a.tar.gz", 1, "a.tar (1).gz"),
        ("README", 2, "README (2)"),
        ("_.bashrc", 1, "_ (1).bashrc"),
        ("report.pdf", 0, "report.pdf"),
    ];

    for (name, number, expected) in cases {
        assert_eq!(candidate(name, number), expected);
    }
}

#[test]
fn candidate_truncates_stem_but_keeps_suffix_and_extension() {
    let name = format!("{}.txt", "a".repeat(250));
    let result = candidate(&name, 999);
    assert!(result.len() <= MAX_NAME_BYTES);
    assert!(result.ends_with(" (999).txt"));
}

#[test]
fn partial_names_round_trip_and_reject_invalid_ids() {
    let transfer_id = "00112233445566778899aabbccddeeff";
    assert_eq!(
        partial_name(transfer_id).expect("valid transfer id"),
        ".rs-00112233445566778899aabbccddeeff.part"
    );
    assert_eq!(
        partial_meta_name(transfer_id).expect("valid transfer id"),
        ".rs-00112233445566778899aabbccddeeff.json"
    );

    assert_eq!(
        parse_partial_name(".rs-00112233445566778899aabbccddeeff.part"),
        Some((transfer_id.to_owned(), PartialKind::Data))
    );
    assert_eq!(
        parse_partial_name(".rs-00112233445566778899aabbccddeeff.json"),
        Some((transfer_id.to_owned(), PartialKind::Meta))
    );
    assert_eq!(parse_partial_name("report.pdf"), None);
    assert_eq!(parse_partial_name(".rs-xyz.part"), None);
    assert_eq!(
        parse_partial_name(".rs-00112233445566778899AABBCCDDEEFF.part"),
        None
    );

    for invalid in [
        "",
        "00112233445566778899aabbccddeef",
        "00112233445566778899aabbccddeeff0",
        "00112233445566778899aabbccddeefg",
        "00112233445566778899AABBCCDDEEFF",
    ] {
        assert_eq!(
            partial_name(invalid),
            Err(NameError::InvalidTransferId),
            "id: {invalid:?}"
        );
        assert_eq!(
            partial_meta_name(invalid),
            Err(NameError::InvalidTransferId),
            "id: {invalid:?}"
        );
    }
}

#[test]
fn safe_component_rejects_paths_and_hidden_names() {
    assert!(is_safe_component("report.pdf"));
    assert!(is_safe_component("报告 2026.pdf"));
    assert!(!is_safe_component(""));
    assert!(!is_safe_component("."));
    assert!(!is_safe_component(".."));
    assert!(!is_safe_component(".hidden"));
    assert!(!is_safe_component("a/b"));
    assert!(!is_safe_component("a\\b"));
    assert!(!is_safe_component("a\0b"));
    assert!(!is_safe_component(&"a".repeat(MAX_NAME_BYTES + 1)));
}

fn contains_forbidden_character(name: &str) -> bool {
    name.chars().any(|character| {
        matches!(
            character,
            '\u{0000}'..='\u{001F}'
                | '\u{007F}'
                | '\u{0080}'..='\u{009F}'
                | '\u{200B}'..='\u{200F}'
                | '\u{202A}'..='\u{202E}'
                | '\u{2066}'..='\u{2069}'
                | '\u{FEFF}'
        ) || matches!(character, '<' | '>' | ':' | '"' | '|' | '?' | '*')
    })
}

proptest! {
    #![proptest_config(ProptestConfig::with_cases(256))]

    #[test]
    fn sanitize_outputs_safe_idempotent_components(input in any::<String>()) {
        let sanitized = sanitize(&input);

        prop_assert!(!sanitized.is_empty());
        prop_assert!(sanitized.len() <= MAX_NAME_BYTES);
        prop_assert!(is_safe_component(&sanitized));
        prop_assert!(!contains_forbidden_character(&sanitized));
        prop_assert_eq!(sanitize(&sanitized), sanitized);
    }

    #[test]
    fn collision_candidates_are_safe_components(
        input in any::<String>(),
        number in 0u32..=MAX_CANDIDATES,
    ) {
        let sanitized = sanitize(&input);
        let candidate = candidate(&sanitized, number);

        prop_assert!(is_safe_component(&candidate));
    }
}
