use std::collections::BTreeMap;

use host_core::inject::{
    hid_to_vk, modifier_usage_to_vk, mods_to_cg_flags, to_mac_scroll, utf16_chunks, vk_is_modifier,
    MacLayout, MacScroll, ScrollAccumulator,
};
use rsp_core::hid::{InputMsg, Mods, ScrollPhase, ScrollUnit};

const LAYOUTS: [MacLayout; 3] = [MacLayout::Ansi, MacLayout::Iso, MacLayout::Jis];

#[test]
fn representative_keys_cover_each_shared_category() {
    // Values checked against HIToolbox/Events.h, not inferred from HID order.
    let categories: &[(&str, &[(u16, u16)])] = &[
        ("letters", &[(0x04, 0x00), (0x10, 0x2E), (0x1D, 0x06)]),
        ("numbers", &[(0x1E, 0x12), (0x22, 0x17), (0x27, 0x1D)]),
        (
            "punctuation",
            &[
                (0x2D, 0x1B),
                (0x2E, 0x18),
                (0x2F, 0x21),
                (0x30, 0x1E),
                (0x31, 0x2A),
                (0x32, 0x2A),
                (0x33, 0x29),
                (0x34, 0x27),
                (0x35, 0x32),
                (0x36, 0x2B),
                (0x37, 0x2F),
                (0x38, 0x2C),
            ],
        ),
        (
            "editing and whitespace",
            &[
                (0x28, 0x24), // Return
                (0x29, 0x35), // Escape
                (0x2A, 0x33), // Delete/backspace
                (0x2B, 0x30), // Tab
                (0x2C, 0x31), // Space
                (0x39, 0x39), // Caps Lock
            ],
        ),
        (
            "navigation",
            &[
                (0x49, 0x72), // Insert uses Mac Help
                (0x4A, 0x73), // Home
                (0x4B, 0x74), // Page Up
                (0x4C, 0x75), // Forward Delete
                (0x4D, 0x77), // End
                (0x4E, 0x79), // Page Down
                (0x75, 0x72), // Help
            ],
        ),
        (
            "arrows",
            &[(0x4F, 0x7C), (0x50, 0x7B), (0x51, 0x7D), (0x52, 0x7E)],
        ),
        ("audio", &[(0x7F, 0x4A), (0x80, 0x48), (0x81, 0x49)]),
    ];
    for layout in LAYOUTS {
        for &(category, cases) in categories {
            for &(usage, vk) in cases {
                assert_eq!(
                    hid_to_vk(usage, layout),
                    Some(vk),
                    "{layout:?} {category}: {usage:#04x}"
                );
            }
        }
    }
}

#[test]
fn all_twenty_function_keys_and_standard_keypad_keys_are_supported() {
    let function_keys = [
        (0x3A, 0x7A),
        (0x3B, 0x78),
        (0x3C, 0x63),
        (0x3D, 0x76), // F1-F4
        (0x3E, 0x60),
        (0x3F, 0x61),
        (0x40, 0x62),
        (0x41, 0x64), // F5-F8
        (0x42, 0x65),
        (0x43, 0x6D),
        (0x44, 0x67),
        (0x45, 0x6F), // F9-F12
        (0x68, 0x69),
        (0x69, 0x6B),
        (0x6A, 0x71),
        (0x6B, 0x6A), // F13-F16
        (0x6C, 0x40),
        (0x6D, 0x4F),
        (0x6E, 0x50),
        (0x6F, 0x5A), // F17-F20
    ];
    let keypad = [
        (0x53, 0x47), // Clear/Num Lock
        (0x54, 0x4B), // Divide
        (0x55, 0x43), // Multiply
        (0x56, 0x4E), // Minus
        (0x57, 0x45), // Plus
        (0x58, 0x4C), // Enter
        (0x59, 0x53),
        (0x5A, 0x54),
        (0x5B, 0x55), // 1-3
        (0x5C, 0x56),
        (0x5D, 0x57),
        (0x5E, 0x58), // 4-6
        (0x5F, 0x59),
        (0x60, 0x5B),
        (0x61, 0x5C), // 7-9
        (0x62, 0x52), // 0
        (0x63, 0x41), // Decimal
        (0x67, 0x51), // Equals
        (0x86, 0x51), // AS/400 Equals
        (0xD8, 0x47), // Keypad Clear
    ];
    for layout in LAYOUTS {
        for &(usage, vk) in function_keys.iter().chain(&keypad) {
            assert_eq!(hid_to_vk(usage, layout), Some(vk));
        }
    }
}

#[test]
fn layout_specific_keys_follow_the_canonical_iso_and_jis_policy() {
    let cases = [
        (0x35, [Some(0x32), Some(0x32), Some(0x32)]), // Grave: no driver swap
        (0x64, [None, Some(0x0A), None]),             // ISO Section
        (0x85, [None, None, Some(0x5F)]),             // JIS Keypad Comma
        (0x87, [None, None, Some(0x5E)]),             // Ro/underscore
        (0x89, [None, None, Some(0x5D)]),             // Yen
        (0x88, [None, None, Some(0x68)]),             // Kana
        (0x8A, [None, None, Some(0x68)]),             // Henkan -> Kana
        (0x8B, [None, None, Some(0x66)]),             // Muhenkan -> Eisu
        (0x90, [None, None, Some(0x68)]),             // LANG1 -> Kana
        (0x91, [None, None, Some(0x66)]),             // LANG2 -> Eisu
    ];
    for (usage, expected) in cases {
        for (layout, vk) in LAYOUTS.into_iter().zip(expected) {
            assert_eq!(hid_to_vk(usage, layout), vk, "{layout:?} {usage:#04x}");
        }
    }
}

#[test]
fn usage_sweep_has_only_documented_aliases_and_never_maps_fn() {
    for layout in LAYOUTS {
        let mut by_vk: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
        for usage in 0x04..=0xE7 {
            if let Some(vk) = hid_to_vk(usage, layout) {
                assert_ne!(vk, 0x3F, "Fn must not be synthesized");
                by_vk.entry(vk).or_default().push(usage);
            }
        }
        let aliases: BTreeMap<_, _> = by_vk
            .into_iter()
            .filter(|(_, usages)| usages.len() > 1)
            .collect();
        // These are physical/semantic HID aliases documented by hid_to_vk.
        // In particular, the required JIS Kana/Eisu aliases preclude strict
        // one-to-one mapping. Left/right modifiers are NOT aliases on macOS.
        let mut expected = BTreeMap::from([
            (0x2A, vec![0x31, 0x32]),
            (0x47, vec![0x53, 0xD8]),
            (0x51, vec![0x67, 0x86]),
            (0x72, vec![0x49, 0x75]),
        ]);
        if layout == MacLayout::Jis {
            expected.insert(0x66, vec![0x8B, 0x91]);
            expected.insert(0x68, vec![0x88, 0x8A, 0x90]);
        }
        assert_eq!(aliases, expected, "unexpected collision in {layout:?}");
        for usage in [0, 1, 3, 0x70, 0x73, 0x8C, 0xE8, 0xFF, 0xFFFF] {
            assert_eq!(hid_to_vk(usage, layout), None);
        }
    }
}

#[test]
fn modifier_keycodes_preserve_sides_and_classify_native_modifiers() {
    let expected = [0x3B, 0x38, 0x3A, 0x37, 0x3E, 0x3C, 0x3D, 0x36];
    for (usage, vk) in (0xE0..=0xE7).zip(expected) {
        assert_eq!(modifier_usage_to_vk(usage), Some(vk));
        assert!(vk_is_modifier(vk));
        for layout in LAYOUTS {
            assert_eq!(hid_to_vk(usage, layout), Some(vk));
        }
    }
    for usage in [0, 0x39, 0xDF, 0xE8, 0xFFFF] {
        assert_eq!(modifier_usage_to_vk(usage), None);
    }
    for vk in 0..=0xFF {
        assert_eq!(
            vk_is_modifier(vk),
            expected.contains(&vk) || vk == 0x39 || vk == 0x3F
        );
    }
    assert!(!vk_is_modifier(u16::MAX));
}

#[test]
fn modifier_flags_cover_eight_single_bits_and_every_combination() {
    // Aggregate CG flag plus the exact IOLLEvent.h NX_DEVICE side bit.
    let single = [
        (Mods::LCTRL, 0x0004_0001),
        (Mods::LSHIFT, 0x0002_0002),
        (Mods::LALT, 0x0008_0020),
        (Mods::LMETA, 0x0010_0008),
        (Mods::RCTRL, 0x0004_2000),
        (Mods::RSHIFT, 0x0002_0004),
        (Mods::RALT, 0x0008_0040),
        (Mods::RMETA, 0x0010_0010),
    ];
    for (modifier, expected) in single {
        assert_eq!(mods_to_cg_flags(modifier), expected);
    }
    assert_eq!(mods_to_cg_flags(Mods::NONE), 0);
    assert_eq!(mods_to_cg_flags(Mods::LCTRL | Mods::RCTRL), 0x0004_2001);
    assert_eq!(mods_to_cg_flags(Mods::from_bits(0xFF)), 0x001E_207F);
    for bits in 0..=u8::MAX {
        let mods = Mods::from_bits(bits);
        let expected = single
            .iter()
            .filter(|(modifier, _)| mods.contains(*modifier))
            .fold(0, |flags, (_, mask)| flags | mask);
        assert_eq!(mods_to_cg_flags(mods), expected, "{bits:#04x}");
    }
}

fn scroll(
    dx: f32,
    dy: f32,
    phase: ScrollPhase,
    momentum: ScrollPhase,
    unit: ScrollUnit,
) -> InputMsg {
    InputMsg::Scroll {
        dx,
        dy,
        phase,
        momentum,
        unit,
    }
}

fn pixels(dx: f32, dy: f32) -> InputMsg {
    scroll(
        dx,
        dy,
        ScrollPhase::Changed,
        ScrollPhase::None,
        ScrollUnit::Pixel,
    )
}

#[test]
fn scroll_and_momentum_phases_use_distinct_native_values_even_for_zero_deltas() {
    let cases = [
        (ScrollPhase::None, 0, 0),
        (ScrollPhase::Began, 1, 1),
        (ScrollPhase::Changed, 2, 2),
        (ScrollPhase::Ended, 4, 3),
    ];
    for (phase, native_phase, _) in cases {
        for (momentum, _, native_momentum) in cases {
            let msg = scroll(0.0, 0.0, phase, momentum, ScrollUnit::Pixel);
            assert_eq!(
                to_mac_scroll(&msg),
                Some(MacScroll {
                    units_pixel: true,
                    dy: 0,
                    dx: 0,
                    scroll_phase: native_phase,
                    momentum_phase: native_momentum,
                })
            );
        }
    }
}

#[test]
fn scroll_uses_pixels_with_configurable_line_scale_and_preserves_direction() {
    let event = to_mac_scroll(&pixels(-12.75, 24.5)).expect("scroll");
    assert_eq!((event.dx, event.dy), (-12, 24));
    assert!(event.units_pixel);
    let line = scroll(
        0.25,
        -1.5,
        ScrollPhase::None,
        ScrollPhase::None,
        ScrollUnit::Line,
    );
    let event = to_mac_scroll(&line).expect("line scroll");
    assert_eq!((event.dx, event.dy), (2, -15));
    assert!(event.units_pixel);
    let mut custom = ScrollAccumulator::with_pixels_per_line(16.0);
    let event = custom.convert(&line).expect("custom line scroll");
    assert_eq!((event.dx, event.dy), (4, -24));
    let event = custom.convert(&pixels(1.0, -2.0)).expect("pixel scroll");
    assert_eq!((event.dx, event.dy), (1, -2));
    for invalid in [0.0, -1.0, f64::NAN, f64::INFINITY] {
        let mut accumulator = ScrollAccumulator::with_pixels_per_line(invalid);
        assert_eq!(accumulator.convert(&line), to_mac_scroll(&line));
    }
}

#[test]
fn subpixel_scroll_accumulates_both_axes_including_direction_changes_and_units() {
    let mut accumulator = ScrollAccumulator::default();
    let events: Vec<_> = (0..8)
        .map(|_| accumulator.convert(&pixels(0.25, -0.375)).expect("scroll"))
        .collect();
    assert_eq!(
        events.iter().map(|event| event.dx).collect::<Vec<_>>(),
        [0, 0, 0, 1, 0, 0, 0, 1]
    );
    assert_eq!(
        events.iter().map(|event| event.dy).collect::<Vec<_>>(),
        [0, 0, -1, 0, 0, -1, 0, -1]
    );
    for (delta, expected) in [(0.75, 0), (-0.5, 0), (-0.75, 0), (-0.5, -1)] {
        assert_eq!(
            accumulator.convert(&pixels(delta, 0.0)).expect("scroll").dx,
            expected
        );
    }
    let line = scroll(
        0.0625,
        -0.0625,
        ScrollPhase::None,
        ScrollPhase::None,
        ScrollUnit::Line,
    );
    assert_eq!(accumulator.convert(&line).expect("line scroll").dx, 0);
    let event = accumulator
        .convert(&pixels(0.375, -0.375))
        .expect("pixel scroll");
    assert_eq!((event.dx, event.dy), (1, -1));
}

#[test]
fn gesture_handoff_retains_fractions_and_new_gestures_or_reset_clear_them() {
    let mut accumulator = ScrollAccumulator::new();
    let stages = [
        (0.75, ScrollPhase::Began, ScrollPhase::None, 0),
        (0.5, ScrollPhase::Changed, ScrollPhase::None, 1),
        (0.5, ScrollPhase::Ended, ScrollPhase::None, 0),
        (0.25, ScrollPhase::None, ScrollPhase::Began, 1),
        (0.75, ScrollPhase::None, ScrollPhase::Ended, 0),
        (0.25, ScrollPhase::Began, ScrollPhase::None, 0),
        (0.75, ScrollPhase::Changed, ScrollPhase::None, 1),
    ];
    for (dx, phase, momentum, expected) in stages {
        let event = scroll(dx, 0.0, phase, momentum, ScrollUnit::Pixel);
        assert_eq!(accumulator.convert(&event).expect("scroll").dx, expected);
    }
    accumulator.convert(&pixels(0.75, -0.75));
    accumulator.reset();
    let event = accumulator.convert(&pixels(0.25, -0.25)).expect("scroll");
    assert_eq!((event.dx, event.dy), (0, 0));
}

#[test]
fn rejected_messages_preserve_fractions_and_extreme_scroll_saturates_without_backlog() {
    let mut accumulator = ScrollAccumulator::new();
    accumulator.convert(&pixels(0.75, -0.75));
    for msg in [
        InputMsg::ReleaseAll,
        InputMsg::Text("ignored".into()),
        pixels(f32::NAN, 0.0),
        pixels(0.0, f32::INFINITY),
        scroll(
            0.0,
            f32::NEG_INFINITY,
            ScrollPhase::Began,
            ScrollPhase::None,
            ScrollUnit::Pixel,
        ),
    ] {
        assert_eq!(to_mac_scroll(&msg), None);
        assert_eq!(accumulator.convert(&msg), None);
    }
    let event = accumulator.convert(&pixels(0.25, -0.25)).expect("scroll");
    assert_eq!((event.dx, event.dy), (1, -1));
    let event = accumulator
        .convert(&pixels(f32::MAX, f32::MIN))
        .expect("saturated scroll");
    assert_eq!((event.dx, event.dy), (i32::MAX, i32::MIN));
    let event = accumulator
        .convert(&pixels(1.0, -1.0))
        .expect("scroll after saturation");
    assert_eq!((event.dx, event.dy), (1, -1));
    let mut huge_scale = ScrollAccumulator::with_pixels_per_line(f64::MAX);
    let msg = scroll(
        f32::MAX,
        f32::MIN,
        ScrollPhase::None,
        ScrollPhase::None,
        ScrollUnit::Line,
    );
    let event = huge_scale.convert(&msg).expect("overflowing scaled deltas");
    assert_eq!((event.dx, event.dy), (i32::MAX, i32::MIN));
}

fn decoded_chunks(s: &str, limit: usize) -> Vec<String> {
    let chunks = utf16_chunks(s, limit);
    assert_eq!(chunks.concat(), s.encode_utf16().collect::<Vec<_>>());
    chunks
        .iter()
        .map(|chunk| String::from_utf16(chunk).expect("valid UTF-16 chunk"))
        .collect()
}

#[test]
fn ascii_text_is_split_into_twenty_twenty_five_units() {
    let text = "a".repeat(45);
    let chunks = utf16_chunks(&text, 20);
    assert_eq!(chunks.iter().map(Vec::len).collect::<Vec<_>>(), [20, 20, 5]);
    assert_eq!(decoded_chunks(&text, 20).concat(), text);
    assert_eq!(decoded_chunks("short", 20), ["short"]);
    assert!(utf16_chunks("", 20).is_empty());
    assert!(utf16_chunks("", 0).is_empty());
}

#[test]
fn text_keeps_surrogate_pairs_combining_characters_and_emoji_clusters_together() {
    let prefix = "a".repeat(19);
    for cluster in ["😀", "e\u{301}", "👍🏽", "🇯🇵", "👩‍👩‍👧‍👦"] {
        let text = format!("{prefix}{cluster}z");
        assert_eq!(
            decoded_chunks(&text, 20),
            [prefix.clone(), format!("{cluster}z")]
        );
    }
    assert_eq!(decoded_chunks("😀😀😀", 3), ["😀", "😀", "😀"]);
    assert_eq!(decoded_chunks("a\r\nb", 2), ["a", "\r\n", "b"]);
}

#[test]
fn oversized_graphemes_stay_in_their_own_chunk_and_zero_limit_makes_progress() {
    let cluster = "👩‍👩‍👧‍👦‍👩‍👩‍👧‍👦";
    assert!(cluster.encode_utf16().count() > 20);
    assert_eq!(
        decoded_chunks(&format!("ab{cluster}cd"), 20),
        ["ab", cluster, "cd"]
    );
    let combining = format!("e{}", "\u{301}".repeat(25));
    assert_eq!(
        decoded_chunks(&format!("x{combining}y"), 20),
        ["x", &combining, "y"]
    );
    assert_eq!(decoded_chunks("a😀e\u{301}", 0), ["a", "😀", "e\u{301}"]);
    assert_eq!(decoded_chunks("a😀e\u{301}", usize::MAX), ["a😀e\u{301}"]);
}
