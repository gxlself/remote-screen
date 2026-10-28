use std::collections::BTreeMap;

use host_linux::keymap::{
    button_to_evdev, button_to_x11, char_to_keysym, evdev_to_x11, hid_to_evdev, modifier_bit,
    BTN_EXTRA, BTN_LEFT, BTN_MIDDLE, BTN_RIGHT, BTN_SIDE, KEY_ENTER, KEY_TAB, MODIFIER_USAGES,
};
use rsp_core::hid::{Button, Mods};

#[test]
fn letters_digits_and_controls() {
    let cases = [
        (0x04, 30),
        (0x1D, 44),
        (0x1E, 2),
        (0x27, 11),
        (0x28, 28),
        (0x29, 1),
        (0x2A, 14),
        (0x2B, 15),
        (0x2C, 57),
    ];
    for (usage, expected) in cases {
        assert_eq!(hid_to_evdev(usage), Some(expected), "usage {usage:#06x}");
    }
    assert_eq!(KEY_ENTER, 28);
    assert_eq!(KEY_TAB, 15);
}

#[test]
fn function_and_navigation() {
    let cases = [
        (0x3A, 59),
        (0x43, 68),
        (0x44, 87),
        (0x45, 88),
        (0x46, 99),
        (0x48, 119),
        (0x4C, 111),
        (0x4F, 106),
        (0x50, 105),
        (0x51, 108),
        (0x52, 103),
        (0x68, 183),
        (0x73, 194),
    ];
    for (usage, expected) in cases {
        assert_eq!(hid_to_evdev(usage), Some(expected), "usage {usage:#06x}");
    }
}

#[test]
fn keypad() {
    let cases = [
        (0x53, 69),
        (0x54, 98),
        (0x58, 96),
        (0x59, 79),
        (0x62, 82),
        (0x63, 83),
        (0x67, 117),
        (0x85, 121),
        (0xB6, 179),
        (0xB7, 180),
    ];
    for (usage, expected) in cases {
        assert_eq!(hid_to_evdev(usage), Some(expected), "usage {usage:#06x}");
    }
}

#[test]
fn international() {
    let cases = [
        (0x64, 86),
        (0x87, 89),
        (0x88, 93),
        (0x89, 124),
        (0x8A, 92),
        (0x8B, 94),
        (0x8C, 95),
        (0x90, 122),
        (0x91, 123),
        (0x92, 90),
        (0x93, 91),
        (0x94, 85),
    ];
    for (usage, expected) in cases {
        assert_eq!(hid_to_evdev(usage), Some(expected), "usage {usage:#06x}");
    }
}

#[test]
fn modifiers() {
    let expected_codes = [29, 42, 56, 125, 97, 54, 100, 126];
    for (index, expected) in expected_codes.into_iter().enumerate() {
        let usage = 0xE0 + index as u16;
        assert_eq!(hid_to_evdev(usage), Some(expected), "usage {usage:#06x}");
    }

    assert_eq!(modifier_bit(0xE3), Some(Mods::LMETA));
    assert_eq!(modifier_bit(0x04), None);
    assert_eq!(
        MODIFIER_USAGES.map(|(_, usage)| usage),
        [0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7]
    );
}

#[test]
fn deliberately_unmapped() {
    for usage in [
        0x00, 0x01, 0x03, 0x66, 0x82, 0x83, 0x84, 0x86, 0x9C, 0xA0, 0xD8, 0xE8, 0xFF, 0x1234,
    ] {
        assert_eq!(hid_to_evdev(usage), None, "usage {usage:#06x}");
    }
}

#[test]
fn only_documented_alias() {
    let mut usages_by_code = BTreeMap::<u16, Vec<u16>>::new();
    let mut mapped = 0;
    for usage in 0..=u16::MAX {
        if let Some(code) = hid_to_evdev(usage) {
            mapped += 1;
            usages_by_code.entry(code).or_default().push(usage);
            assert!((1..=194).contains(&code), "usage {usage:#06x} code {code}");
        }
    }

    assert_eq!(mapped, 147);
    assert_eq!(usages_by_code.len(), 146);
    let aliases = usages_by_code
        .values()
        .filter(|usages| usages.len() > 1)
        .cloned()
        .collect::<Vec<_>>();
    assert_eq!(aliases, vec![vec![0x31, 0x32]]);
}

#[test]
fn x11_codes() {
    assert_eq!(evdev_to_x11(30), Some(38));
    assert_eq!(evdev_to_x11(247), Some(255));
    assert_eq!(evdev_to_x11(248), None);
    assert_eq!(evdev_to_x11(BTN_LEFT), None);
}

#[test]
fn buttons() {
    assert_eq!(button_to_evdev(Button::Left), BTN_LEFT);
    assert_eq!(button_to_evdev(Button::Right), BTN_RIGHT);
    assert_eq!(button_to_evdev(Button::Middle), BTN_MIDDLE);
    assert_eq!(button_to_evdev(Button::Back), BTN_SIDE);
    assert_eq!(button_to_evdev(Button::Forward), BTN_EXTRA);

    let buttons = [
        (Button::Left, 1),
        (Button::Right, 3),
        (Button::Middle, 2),
        (Button::Back, 8),
        (Button::Forward, 9),
    ];
    for (button, expected) in buttons {
        assert_eq!(button_to_x11(button), expected);
    }
}

#[test]
fn keysyms() {
    let cases = [
        ('a', 0x61),
        ('A', 0x41),
        (' ', 0x20),
        ('~', 0x7E),
        ('\u{a0}', 0xA0),
        ('é', 0xE9),
        ('ÿ', 0xFF),
        ('Ā', 0x0100_0100),
        ('€', 0x0100_20AC),
        ('中', 0x0100_4E2D),
        ('😀', 0x0101_F600),
    ];
    for (character, expected) in cases {
        assert_eq!(
            char_to_keysym(character),
            Some(expected),
            "character {character:?}"
        );
    }

    for character in ['\u{1b}', '\u{7f}', '\u{85}', '\n'] {
        assert_eq!(char_to_keysym(character), None, "character {character:?}");
    }
}
