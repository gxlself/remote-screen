use host_core::inject::text_plan::{
    plan_text, plan_text_with_limit, TextPlan, TextStep, MAX_UNITS_PER_EVENT, VK_RETURN, VK_TAB,
};
use rsp_core::hid::MAX_TEXT_BYTES;

fn unicode_text(step: &TextStep) -> String {
    match step {
        TextStep::Unicode(units) => {
            String::from_utf16(units).expect("Unicode steps must decode losslessly")
        }
        TextStep::Key(vk) => panic!("expected a Unicode step, got key {vk:#04x}"),
    }
}

fn assert_unicode_chunks(plan: &TextPlan, expected: &[&str]) {
    let actual: Vec<_> = plan
        .steps
        .iter()
        .map(|step| match step {
            TextStep::Unicode(_) => unicode_text(step),
            TextStep::Key(vk) => panic!("expected only Unicode steps, got key {vk:#04x}"),
        })
        .collect();
    assert_eq!(actual, expected);
}

fn chunk_lengths(plan: &TextPlan) -> Vec<usize> {
    plan.steps
        .iter()
        .filter_map(|step| match step {
            TextStep::Unicode(units) => Some(units.len()),
            TextStep::Key(_) => None,
        })
        .collect()
}

fn is_dropped_control(ch: char) -> bool {
    matches!(ch, '\u{0000}'..='\u{001f}' | '\u{007f}' | '\u{0080}'..='\u{009f}')
        && !matches!(ch, '\r' | '\n' | '\t')
}

#[test]
fn ascii_is_chunked_twenty_twenty_five() {
    let text = "a".repeat(45);
    let plan = plan_text(&text);

    assert_eq!(chunk_lengths(&plan), vec![20, 20, 5]);
    assert_eq!(plan.steps.len(), 3);
    assert_eq!(plan.utf16_units, 45);
    assert_eq!(plan.dropped_controls, 0);
}

#[test]
fn line_breaks_and_tabs_become_real_keys() {
    assert_eq!(
        plan_text("a\r\nb").steps,
        vec![
            TextStep::Unicode("a".encode_utf16().collect()),
            TextStep::Key(VK_RETURN),
            TextStep::Unicode("b".encode_utf16().collect()),
        ]
    );
    assert_eq!(
        plan_text("a\rb\nc\td").steps,
        vec![
            TextStep::Unicode("a".encode_utf16().collect()),
            TextStep::Key(VK_RETURN),
            TextStep::Unicode("b".encode_utf16().collect()),
            TextStep::Key(VK_RETURN),
            TextStep::Unicode("c".encode_utf16().collect()),
            TextStep::Key(VK_TAB),
            TextStep::Unicode("d".encode_utf16().collect()),
        ]
    );
    assert_eq!(
        plan_text("\n\n").steps,
        vec![TextStep::Key(VK_RETURN), TextStep::Key(VK_RETURN)]
    );
    assert_eq!(
        plan_text("a\u{2028}b").steps,
        vec![
            TextStep::Unicode("a".encode_utf16().collect()),
            TextStep::Key(VK_RETURN),
            TextStep::Unicode("b".encode_utf16().collect()),
        ]
    );
}

#[test]
fn controls_are_dropped_and_do_not_break_text_runs() {
    let plan = plan_text("x\u{0}\u{7}\u{1b}[31m\u{7f}\u{85}y");
    assert_eq!(plan.steps.len(), 1);
    assert_unicode_chunks(&plan, &["x[31my"]);
    assert_eq!(plan.dropped_controls, 5);
    assert_eq!(plan.utf16_units, 6);

    let plan = plan_text("\u{8}\u{8}");
    assert!(plan.steps.is_empty());
    assert_eq!(plan.dropped_controls, 2);
    assert_eq!(plan.utf16_units, 0);
}

#[test]
fn cjk_is_counted_in_utf16_units() {
    let plan = plan_text(&"中".repeat(25));
    assert_eq!(chunk_lengths(&plan), vec![20, 5]);
    assert_eq!(plan.utf16_units, 25);

    let plan = plan_text("你好");
    assert_eq!(plan.steps.len(), 1);
    assert_unicode_chunks(&plan, &["你好"]);
}

#[test]
fn surrogate_pairs_are_never_split() {
    let text = format!("{}😀", "a".repeat(19));
    let plan = plan_text(&text);
    assert_eq!(chunk_lengths(&plan), vec![19, 2]);
    assert_unicode_chunks(&plan, &["aaaaaaaaaaaaaaaaaaa", "😀"]);

    let plan = plan_text(&"😀".repeat(11));
    assert_eq!(chunk_lengths(&plan), vec![20, 2]);
    for step in &plan.steps {
        let TextStep::Unicode(units) = step else {
            panic!("expected a Unicode step");
        };
        assert_eq!(units.len() % 2, 0);
        String::from_utf16(units).expect("both surrogate pairs must remain intact");
    }
}

#[test]
fn grapheme_clusters_and_regional_indicators_stay_intact() {
    let family = "👨‍👩‍👧‍👦";
    let plan = plan_text(&family.repeat(2));
    assert_eq!(chunk_lengths(&plan), vec![11, 11]);
    assert_unicode_chunks(&plan, &[family, family]);

    let flags = "🇯🇵🇨🇳🇺🇸";
    let plan = plan_text(flags);
    assert_eq!(chunk_lengths(&plan), vec![12]);
    assert_unicode_chunks(&plan, &[flags]);
}

#[test]
fn an_overlong_grapheme_cluster_is_split_into_separate_chunks() {
    let cluster = format!("e{}", "\u{301}".repeat(25));
    let plan = plan_text(&cluster);
    assert_eq!(chunk_lengths(&plan), vec![20, 6]);
    assert_eq!(replay(&plan), cluster);
    assert_eq!(plan.utf16_units, 26);

    let text = format!("a{cluster}b");
    let plan = plan_text(&text);
    assert_eq!(chunk_lengths(&plan), vec![1, 20, 6, 1]);
    assert_eq!(replay(&plan), text);
    assert_eq!(plan.utf16_units, 28);
}

#[test]
fn a_41_unit_cluster_is_split_twenty_twenty_one() {
    let text = format!("a{}", "\u{301}".repeat(40));
    let plan = plan_text(&text);
    assert_eq!(chunk_lengths(&plan), vec![20, 20, 1]);
    assert_eq!(replay(&plan), text);
    assert_eq!(plan.utf16_units, 41);
}

#[test]
fn an_overlong_cluster_preserves_surrogate_pairs() {
    let text = format!("😀{}", "\u{301}".repeat(25));
    let plan = plan_text(&text);
    assert_eq!(chunk_lengths(&plan), vec![20, 7]);
    assert_eq!(replay(&plan), text);
    assert_eq!(plan.utf16_units, 27);
    for step in &plan.steps {
        let TextStep::Unicode(units) = step else {
            panic!("expected a Unicode step");
        };
        assert!(!(0xdc00..=0xdfff).contains(&units[0]));
        String::from_utf16(units).expect("chunk must decode losslessly");
    }
}

#[test]
fn splitting_an_overlong_cluster_moves_a_boundary_before_a_surrogate_pair() {
    let text = format!("{}😀{}", "\u{600}".repeat(19), "\u{301}".repeat(5));
    let plan = plan_text(&text);
    assert_eq!(chunk_lengths(&plan), vec![19, 7]);
    assert_eq!(replay(&plan), text);
    assert_eq!(plan.utf16_units, 26);
}

#[test]
fn ordinary_emoji_clusters_are_packed_without_splitting() {
    let text = "😀".repeat(15);
    let plan = plan_text(&text);
    assert_eq!(chunk_lengths(&plan), vec![20, 10]);
    assert_eq!(replay(&plan), text);
    assert_eq!(plan.utf16_units, 30);
}

#[test]
fn empty_input_is_the_default_plan() {
    assert_eq!(plan_text(""), TextPlan::default());
}

#[test]
fn maximum_sized_text_does_not_panic() {
    let text = format!("{}a", "中".repeat(1365));
    assert_eq!(text.len(), MAX_TEXT_BYTES);

    let plan = plan_text(&text);
    assert_eq!(plan.utf16_units, 1366);
    assert_eq!(
        chunk_lengths(&plan),
        vec![20; 68].into_iter().chain([6]).collect::<Vec<_>>()
    );
    assert_eq!(plan.steps.len(), 1366_usize.div_ceil(MAX_UNITS_PER_EVENT));
}

struct XorShift64(u64);

impl XorShift64 {
    fn next(&mut self) -> u64 {
        let mut value = self.0;
        value ^= value << 13;
        value ^= value >> 7;
        value ^= value << 17;
        self.0 = value;
        value
    }
}

fn normalize_text(text: &str) -> (String, usize) {
    let mut normalized = String::new();
    let mut dropped = 0;
    let mut chars = text.chars().peekable();
    while let Some(ch) = chars.next() {
        match ch {
            '\r' => {
                if chars.peek() == Some(&'\n') {
                    chars.next();
                }
                normalized.push('\n');
            }
            '\n' | '\u{2028}' | '\u{2029}' => normalized.push('\n'),
            ch if is_dropped_control(ch) => dropped += 1,
            ch => normalized.push(ch),
        }
    }
    (normalized, dropped)
}

fn replay(plan: &TextPlan) -> String {
    let mut text = String::new();
    for step in &plan.steps {
        match step {
            TextStep::Unicode(_) => text.push_str(&unicode_text(step)),
            TextStep::Key(VK_RETURN) => text.push('\n'),
            TextStep::Key(VK_TAB) => text.push('\t'),
            TextStep::Key(vk) => panic!("unexpected key {vk:#04x}"),
        }
    }
    text
}

#[test]
fn randomized_round_trip_and_chunk_invariants() {
    const TOKENS: &[&str] = &[
        "a",
        "Z",
        "0",
        " ",
        "~",
        "中",
        "汉",
        "😀",
        "👨‍👩‍👧‍👦",
        "🇯🇵",
        "\u{301}",
        "\r\n",
        "\r",
        "\n",
        "\t",
        "\u{0}",
        "\u{7}",
        "\u{1b}",
        "\u{7f}",
        "\u{85}",
        "\u{2028}",
        "\u{2029}",
        "\u{202a}",
        "\u{2066}",
        "\u{feff}",
    ];
    let overlong_cluster = format!("a{}", "\u{301}".repeat(40));
    let tokens: Vec<_> = TOKENS
        .iter()
        .copied()
        .chain([overlong_cluster.as_str()])
        .collect();
    let mut rng = XorShift64(0x9e37_79b9_7f4a_7c15);

    for _ in 0..200 {
        let token_count = (rng.next() % 100) as usize;
        let mut text = String::new();
        for _ in 0..token_count {
            text.push_str(tokens[(rng.next() as usize) % tokens.len()]);
        }

        let plan = plan_text(&text);
        let (expected, dropped) = normalize_text(&text);
        assert_eq!(replay(&plan), expected, "input: {text:?}");
        assert_eq!(plan.dropped_controls, dropped, "input: {text:?}");

        let mut units = 0;
        for step in &plan.steps {
            if let TextStep::Unicode(chunk) = step {
                assert!(!chunk.is_empty());
                units += chunk.len();
                assert!(chunk.len() <= MAX_UNITS_PER_EVENT);
                String::from_utf16(chunk).expect("chunk must decode losslessly");
            }
        }
        assert_eq!(plan.utf16_units, units, "input: {text:?}");
    }
}

#[test]
fn zero_and_one_limits_keep_chunks_within_two_units() {
    let text = "a中😀e\u{301}👨‍👩‍👧‍👦🇯🇵x";
    for limit in [0, 1] {
        let plan = plan_text_with_limit(text, limit);
        for step in &plan.steps {
            let TextStep::Unicode(units) = step else {
                panic!("expected a Unicode step");
            };
            assert!(units.len() <= 2);
            String::from_utf16(units).expect("chunk must decode losslessly");
        }
        assert_eq!(replay(&plan), text);
        assert_eq!(plan.utf16_units, text.encode_utf16().count());
        assert_eq!(plan_text_with_limit("", limit), TextPlan::default());
    }
}
