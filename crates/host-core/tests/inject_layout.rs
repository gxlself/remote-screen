use std::collections::{BTreeMap, BTreeSet};

use host_core::inject::layout::{
    layout_diffs_for, layout_from_kb_layout_type, resolve_vk, KeyboardProfile, LayoutDiff,
    DEFAULT_KBD_TYPE_ANSI, DEFAULT_KBD_TYPE_ISO, DEFAULT_KBD_TYPE_JIS, KB_LAYOUT_ANSI,
    KB_LAYOUT_ISO, KB_LAYOUT_JIS, LAYOUT_DIFFS,
};
use host_core::inject::{hid_to_vk, modifier_usage_to_vk, MacLayout};

const LAYOUTS: [MacLayout; 3] = [MacLayout::Ansi, MacLayout::Iso, MacLayout::Jis];
const UNKNOWN_LAYOUT_TYPE: u32 = u32::from_be_bytes(*b"XXXX");

fn profile(layout: MacLayout, iso_position_swap: bool) -> KeyboardProfile {
    KeyboardProfile {
        layout,
        keyboard_type: KeyboardProfile::new(layout).keyboard_type,
        iso_position_swap,
    }
}

#[test]
fn carbon_layout_type_four_character_codes_are_recognized() {
    assert_eq!(
        layout_from_kb_layout_type(KB_LAYOUT_ANSI),
        Some(MacLayout::Ansi)
    );
    assert_eq!(
        layout_from_kb_layout_type(KB_LAYOUT_ISO),
        Some(MacLayout::Iso)
    );
    assert_eq!(
        layout_from_kb_layout_type(KB_LAYOUT_JIS),
        Some(MacLayout::Jis)
    );
    assert_eq!(layout_from_kb_layout_type(0), None);
    assert_eq!(layout_from_kb_layout_type(UNKNOWN_LAYOUT_TYPE), None);
}

#[test]
fn detect_uses_layout_defaults_and_preserves_reported_keyboard_type() {
    let iso = KeyboardProfile::detect(Some(KB_LAYOUT_ISO), Some(43));
    assert_eq!(iso.layout, MacLayout::Iso);
    assert_eq!(iso.keyboard_type, 43);
    assert!(iso.iso_position_swap);

    let jis = KeyboardProfile::detect(Some(KB_LAYOUT_JIS), None);
    assert_eq!(jis.layout, MacLayout::Jis);
    assert_eq!(jis.keyboard_type, DEFAULT_KBD_TYPE_JIS);
    assert!(!jis.iso_position_swap);

    let missing = KeyboardProfile::detect(None, None);
    assert_eq!(missing.layout, MacLayout::Ansi);
    assert_eq!(missing.keyboard_type, DEFAULT_KBD_TYPE_ANSI);
    assert!(!missing.iso_position_swap);

    let unknown_and_zero = KeyboardProfile::detect(Some(UNKNOWN_LAYOUT_TYPE), Some(0));
    assert_eq!(unknown_and_zero.layout, MacLayout::Ansi);
    assert_eq!(unknown_and_zero.keyboard_type, DEFAULT_KBD_TYPE_ANSI);

    let known_layout_nonzero_type = KeyboardProfile::detect(Some(KB_LAYOUT_ANSI), Some(43));
    assert_eq!(known_layout_nonzero_type.layout, MacLayout::Ansi);
    assert_eq!(known_layout_nonzero_type.keyboard_type, 43);

    assert_eq!(
        KeyboardProfile::default(),
        KeyboardProfile::new(MacLayout::Ansi)
    );
    assert_eq!(DEFAULT_KBD_TYPE_ANSI, 40);
    assert_eq!(DEFAULT_KBD_TYPE_ISO, 41);
    assert_eq!(DEFAULT_KBD_TYPE_JIS, 42);
}

#[test]
fn iso_profile_swaps_grave_and_section_exactly_once() {
    let iso = KeyboardProfile::new(MacLayout::Iso);
    assert_eq!(resolve_vk(0x35, &iso), Some(0x0A));
    assert_eq!(resolve_vk(0x64, &iso), Some(0x32));

    let canonical_iso = profile(MacLayout::Iso, false);
    assert_eq!(resolve_vk(0x35, &canonical_iso), Some(0x32));
    assert_eq!(resolve_vk(0x64, &canonical_iso), Some(0x0A));

    for layout in [MacLayout::Ansi, MacLayout::Jis] {
        let profile = KeyboardProfile::new(layout);
        assert_eq!(resolve_vk(0x35, &profile), Some(0x32));
        assert_eq!(resolve_vk(0x64, &profile), None);
    }
}

#[test]
fn language_keys_use_lenient_kana_and_eisu_mapping_on_every_layout() {
    for layout in LAYOUTS {
        let profile = KeyboardProfile::new(layout);
        assert_eq!(resolve_vk(0x90, &profile), Some(0x68), "{layout:?} LANG1");
        assert_eq!(resolve_vk(0x91, &profile), Some(0x66), "{layout:?} LANG2");
    }

    for layout in [MacLayout::Ansi, MacLayout::Iso] {
        let profile = KeyboardProfile::new(layout);
        for usage in [0x85, 0x87, 0x88, 0x89, 0x8A, 0x8B] {
            assert_eq!(resolve_vk(usage, &profile), None, "{layout:?} {usage:#04x}");
        }
    }
}

#[test]
fn layout_diff_constant_is_complete_and_matches_the_resolver() {
    let expected = vec![
        LayoutDiff {
            usage: 0x35,
            key: "Grave / §（1 左边）",
            ansi: Some(0x32),
            iso: Some(0x0A),
            jis: Some(0x32),
        },
        LayoutDiff {
            usage: 0x64,
            key: "Non-US \\（左 Shift 右边）",
            ansi: None,
            iso: Some(0x32),
            jis: None,
        },
        LayoutDiff {
            usage: 0x85,
            key: "Keypad ,（JIS）",
            ansi: None,
            iso: None,
            jis: Some(0x5F),
        },
        LayoutDiff {
            usage: 0x87,
            key: "International1 ろ / _",
            ansi: None,
            iso: None,
            jis: Some(0x5E),
        },
        LayoutDiff {
            usage: 0x88,
            key: "International2 かな",
            ansi: None,
            iso: None,
            jis: Some(0x68),
        },
        LayoutDiff {
            usage: 0x89,
            key: "International3 ¥",
            ansi: None,
            iso: None,
            jis: Some(0x5D),
        },
        LayoutDiff {
            usage: 0x8A,
            key: "International4 変換",
            ansi: None,
            iso: None,
            jis: Some(0x68),
        },
        LayoutDiff {
            usage: 0x8B,
            key: "International5 無変換",
            ansi: None,
            iso: None,
            jis: Some(0x66),
        },
    ];

    assert_eq!(LAYOUT_DIFFS, expected.as_slice());
    assert_eq!(
        LAYOUT_DIFFS,
        layout_diffs_for(
            &KeyboardProfile::new(MacLayout::Ansi),
            &KeyboardProfile::new(MacLayout::Iso),
            &KeyboardProfile::new(MacLayout::Jis),
        )
    );
    assert!(!LAYOUT_DIFFS
        .iter()
        .any(|diff| matches!(diff.usage, 0x90 | 0x91)));
}

#[test]
fn usages_outside_the_diff_table_are_layout_independent_and_canonical() {
    let ansi = KeyboardProfile::new(MacLayout::Ansi);
    let iso = KeyboardProfile::new(MacLayout::Iso);
    let jis = KeyboardProfile::new(MacLayout::Jis);
    let diff_usages: BTreeSet<u16> = LAYOUT_DIFFS.iter().map(|diff| diff.usage).collect();

    for usage in 0x04..=0xE7 {
        if diff_usages.contains(&usage) {
            continue;
        }
        let ansi_vk = resolve_vk(usage, &ansi);
        assert_eq!(resolve_vk(usage, &iso), ansi_vk, "ISO {usage:#04x}");
        assert_eq!(resolve_vk(usage, &jis), ansi_vk, "JIS {usage:#04x}");

        if matches!(usage, 0x90 | 0x91) {
            // Requirement 4 intentionally adds these two cross-layout aliases on
            // ANSI/ISO, so they cannot equal the canonical canonical-table result.
            assert_eq!(hid_to_vk(usage, MacLayout::Ansi), None);
        } else {
            assert_eq!(
                ansi_vk,
                hid_to_vk(usage, MacLayout::Ansi),
                "canonical ANSI {usage:#04x}"
            );
        }
    }
}

#[test]
fn iso_swapped_profile_has_only_the_documented_aliases() {
    let iso = KeyboardProfile::new(MacLayout::Iso);
    let mut by_vk: BTreeMap<u16, Vec<u16>> = BTreeMap::new();
    for usage in 0x04..=0xE7 {
        if let Some(vk) = resolve_vk(usage, &iso) {
            by_vk.entry(vk).or_default().push(usage);
        }
    }
    let aliases: BTreeMap<_, _> = by_vk
        .into_iter()
        .filter(|(_, usages)| usages.len() > 1)
        .collect();
    assert_eq!(
        aliases,
        BTreeMap::from([
            (0x2A, vec![0x31, 0x32]),
            (0x47, vec![0x53, 0xD8]),
            (0x51, vec![0x67, 0x86]),
            (0x72, vec![0x49, 0x75]),
        ])
    );
}

#[test]
fn modifiers_are_unchanged_in_all_six_profiles() {
    for layout in LAYOUTS {
        for iso_position_swap in [false, true] {
            let profile = profile(layout, iso_position_swap);
            for usage in 0xE0..=0xE7 {
                assert_eq!(
                    resolve_vk(usage, &profile),
                    modifier_usage_to_vk(usage),
                    "{layout:?} swap={iso_position_swap} {usage:#04x}"
                );
            }
        }
    }
}
