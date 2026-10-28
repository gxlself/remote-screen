use std::collections::{BTreeMap, BTreeSet};

use host_core::inject::keymap_mac::{hid_to_vk, MacLayout};
use host_core::inject::keymap_win::{hid_to_win, modifier_bit, WinKey, MODIFIER_USAGES};
use rsp_core::hid::Mods;

fn scan(scan: u16) -> WinKey {
    WinKey {
        scan,
        extended: false,
        vk: 0,
    }
}

fn extended(scan: u16) -> WinKey {
    WinKey {
        scan,
        extended: true,
        vk: 0,
    }
}

fn virtual_key(scan: u16, vk: u16) -> WinKey {
    WinKey {
        scan,
        extended: false,
        vk,
    }
}

#[test]
fn maps_every_supported_usage_to_its_windows_set_1_key() {
    let cases = [
        // Letters A-Z.
        (0x04, scan(0x1E)),
        (0x05, scan(0x30)),
        (0x06, scan(0x2E)),
        (0x07, scan(0x20)),
        (0x08, scan(0x12)),
        (0x09, scan(0x21)),
        (0x0A, scan(0x22)),
        (0x0B, scan(0x23)),
        (0x0C, scan(0x17)),
        (0x0D, scan(0x24)),
        (0x0E, scan(0x25)),
        (0x0F, scan(0x26)),
        (0x10, scan(0x32)),
        (0x11, scan(0x31)),
        (0x12, scan(0x18)),
        (0x13, scan(0x19)),
        (0x14, scan(0x10)),
        (0x15, scan(0x13)),
        (0x16, scan(0x1F)),
        (0x17, scan(0x14)),
        (0x18, scan(0x16)),
        (0x19, scan(0x2F)),
        (0x1A, scan(0x11)),
        (0x1B, scan(0x2D)),
        (0x1C, scan(0x15)),
        (0x1D, scan(0x2C)),
        // Number row 1-9, 0.
        (0x1E, scan(0x02)),
        (0x1F, scan(0x03)),
        (0x20, scan(0x04)),
        (0x21, scan(0x05)),
        (0x22, scan(0x06)),
        (0x23, scan(0x07)),
        (0x24, scan(0x08)),
        (0x25, scan(0x09)),
        (0x26, scan(0x0A)),
        (0x27, scan(0x0B)),
        // Basic editing and punctuation.
        (0x28, scan(0x1C)), // Enter
        (0x29, scan(0x01)), // Escape
        (0x2A, scan(0x0E)), // Backspace
        (0x2B, scan(0x0F)), // Tab
        (0x2C, scan(0x39)), // Space
        (0x2D, scan(0x0C)), // Minus
        (0x2E, scan(0x0D)), // Equal
        (0x2F, scan(0x1A)), // Left Bracket
        (0x30, scan(0x1B)), // Right Bracket
        (0x31, scan(0x2B)), // Backslash
        (0x32, scan(0x2B)), // Non-US #; alias of Backslash
        (0x33, scan(0x27)), // Semicolon
        (0x34, scan(0x28)), // Apostrophe
        (0x35, scan(0x29)), // Grave
        (0x36, scan(0x33)), // Comma
        (0x37, scan(0x34)), // Period
        (0x38, scan(0x35)), // Slash
        (0x39, scan(0x3A)), // Caps Lock
        // Function keys F1-F12.
        (0x3A, scan(0x3B)),
        (0x3B, scan(0x3C)),
        (0x3C, scan(0x3D)),
        (0x3D, scan(0x3E)),
        (0x3E, scan(0x3F)),
        (0x3F, scan(0x40)),
        (0x40, scan(0x41)),
        (0x41, scan(0x42)),
        (0x42, scan(0x43)),
        (0x43, scan(0x44)),
        (0x44, scan(0x57)),
        (0x45, scan(0x58)),
        // System and navigation keys.
        (0x46, extended(0x37)),          // Print Screen
        (0x47, scan(0x46)),              // Scroll Lock
        (0x48, virtual_key(0x45, 0x13)), // Pause
        (0x49, extended(0x52)),          // Insert
        (0x4A, extended(0x47)),          // Home
        (0x4B, extended(0x49)),          // Page Up
        (0x4C, extended(0x53)),          // Delete
        (0x4D, extended(0x4F)),          // End
        (0x4E, extended(0x51)),          // Page Down
        (0x4F, extended(0x4D)),          // Right Arrow
        (0x50, extended(0x4B)),          // Left Arrow
        (0x51, extended(0x50)),          // Down Arrow
        (0x52, extended(0x48)),          // Up Arrow
        // Keypad.
        (0x53, extended(0x45)), // Num Lock
        (0x54, extended(0x35)), // Keypad /
        (0x55, scan(0x37)),     // Keypad *
        (0x56, scan(0x4A)),     // Keypad -
        (0x57, scan(0x4E)),     // Keypad +
        (0x58, extended(0x1C)), // Keypad Enter
        (0x59, scan(0x4F)),     // Keypad 1
        (0x5A, scan(0x50)),     // Keypad 2
        (0x5B, scan(0x51)),     // Keypad 3
        (0x5C, scan(0x4B)),     // Keypad 4
        (0x5D, scan(0x4C)),     // Keypad 5
        (0x5E, scan(0x4D)),     // Keypad 6
        (0x5F, scan(0x47)),     // Keypad 7
        (0x60, scan(0x48)),     // Keypad 8
        (0x61, scan(0x49)),     // Keypad 9
        (0x62, scan(0x52)),     // Keypad 0
        (0x63, scan(0x53)),     // Keypad .
        (0x64, scan(0x56)),     // International Backslash
        (0x65, extended(0x5D)), // Context Menu
        (0x67, scan(0x59)),     // Keypad =
        (0x86, scan(0x59)),     // Keypad = (AS/400); alias of 0x67
        // Function keys F13-F24.
        (0x68, scan(0x64)),
        (0x69, scan(0x65)),
        (0x6A, scan(0x66)),
        (0x6B, scan(0x67)),
        (0x6C, scan(0x68)),
        (0x6D, scan(0x69)),
        (0x6E, scan(0x6A)),
        (0x6F, scan(0x6B)),
        (0x70, scan(0x6C)),
        (0x71, scan(0x6D)),
        (0x72, scan(0x6E)),
        (0x73, scan(0x76)),
        // Consumer and international keys.
        (0x7F, extended(0x20)), // Mute
        (0x80, extended(0x30)), // Volume Up
        (0x81, extended(0x2E)), // Volume Down
        (0x85, scan(0x7E)),     // Keypad ,
        (0x87, scan(0x73)),     // International1 (Ro)
        (0x88, scan(0x70)),     // Kana Mode
        (0x89, scan(0x7D)),     // International3 (Yen)
        (0x8A, scan(0x79)),     // Convert
        (0x8B, scan(0x7B)),     // Non-convert
        (0x90, scan(0x72)),     // LANG1
        (0x91, scan(0x71)),     // LANG2
        // Side-specific modifiers.
        (0xE0, scan(0x1D)),     // Left Ctrl
        (0xE1, scan(0x2A)),     // Left Shift
        (0xE2, scan(0x38)),     // Left Alt
        (0xE3, extended(0x5B)), // Left Meta
        (0xE4, extended(0x1D)), // Right Ctrl
        (0xE5, scan(0x36)),     // Right Shift
        (0xE6, extended(0x38)), // Right Alt
        (0xE7, extended(0x5C)), // Right Meta
    ];

    for (usage, expected) in cases {
        assert_eq!(hid_to_win(usage), Some(expected), "usage {usage:#06x}");
    }
}

#[test]
fn rejects_intentionally_unmapped_usages() {
    for usage in [
        0x66, // Power: remote input must not be able to power off the host.
        0x75, // Help.
        0xD8, // Keypad Clear: no independent Windows key.
        0x00, 0x03, // Reserved.
        0x74, // Execute.
        0x7A, // Paste: no Set 1 keyboard make code.
        0xE8, // Beyond modifiers.
        0xFFFF,
    ] {
        assert_eq!(hid_to_win(usage), None, "usage {usage:#06x}");
    }

    for usage in (0x76..=0x7E).chain(0x82..=0x84) {
        assert_eq!(hid_to_win(usage), None, "usage {usage:#06x}");
    }
}

#[test]
fn only_the_documented_alias_pairs_share_a_scan_identity() {
    let mut usages_by_key: BTreeMap<(u16, bool), Vec<u16>> = BTreeMap::new();
    for usage in 0x00..=0xFF {
        if let Some(key) = hid_to_win(usage) {
            usages_by_key
                .entry((key.scan, key.extended))
                .or_default()
                .push(usage);
        }
    }

    let aliases = usages_by_key
        .into_values()
        .filter(|usages| usages.len() > 1)
        .collect::<BTreeSet<_>>();
    assert_eq!(
        aliases,
        BTreeSet::from([vec![0x31, 0x32], vec![0x67, 0x86]])
    );
}

#[test]
fn scan_codes_stay_below_0x80_and_only_pause_uses_a_virtual_key() {
    for usage in 0x00..=u16::MAX {
        if let Some(key) = hid_to_win(usage) {
            assert!(key.scan < 0x80, "usage {usage:#06x} scan {:#06x}", key.scan);
            if key.vk != 0 {
                assert_eq!(usage, 0x48, "unexpected virtual-key usage {usage:#06x}");
                assert_eq!(key.vk, 0x13);
            }
        }
    }
}

#[test]
fn mac_supported_usages_are_supported_except_help_and_keypad_clear() {
    let mut mac_only = Vec::new();
    for usage in 0x00..=0xFF {
        let mac_supports = hid_to_vk(usage, MacLayout::Ansi).is_some()
            || hid_to_vk(usage, MacLayout::Jis).is_some();
        if mac_supports && hid_to_win(usage).is_none() {
            mac_only.push(usage);
        }
    }

    // macOS maps Help to the Help VK, and Keypad Clear to KeypadClear. Windows
    // intentionally has no mapping for either usage in this table.
    assert_eq!(mac_only, [0x75, 0xD8]);
}

#[test]
fn side_specific_modifiers_have_the_expected_bits() {
    assert_eq!(modifier_bit(0xE0), Some(Mods::LCTRL));
    assert_eq!(modifier_bit(0xE1), Some(Mods::LSHIFT));
    assert_eq!(modifier_bit(0xE2), Some(Mods::LALT));
    assert_eq!(modifier_bit(0xE3), Some(Mods::LMETA));
    assert_eq!(modifier_bit(0xE4), Some(Mods::RCTRL));
    assert_eq!(modifier_bit(0xE5), Some(Mods::RSHIFT));
    assert_eq!(modifier_bit(0xE6), Some(Mods::RALT));
    assert_eq!(modifier_bit(0xE7), Some(Mods::RMETA));
    assert_eq!(modifier_bit(0x04), None);

    assert_eq!(
        MODIFIER_USAGES,
        [0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7]
    );
    for usage in MODIFIER_USAGES {
        assert!(hid_to_win(usage).is_some(), "usage {usage:#06x}");
    }
}
