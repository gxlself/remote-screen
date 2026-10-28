use std::collections::BTreeSet;

use rs_mobile::rscore::{combos::*, dto_core::SessionCombo};
use rsp_core::hid::{InputMsg, Mods};

pub fn key(usage: u16, down: bool, mods: Mods) -> InputMsg {
    InputMsg::Key { usage, down, mods }
}

#[test]
pub fn all_shortcuts_have_the_exact_hid_sequence() {
    let cases = [
        (
            SessionCombo::CmdTab,
            vec![
                key(0xE3, true, Mods::LMETA),
                key(0x2B, true, Mods::LMETA),
                key(0x2B, false, Mods::LMETA),
                key(0xE3, false, Mods::NONE),
            ],
        ),
        (
            SessionCombo::CmdSpace,
            vec![
                key(0xE3, true, Mods::LMETA),
                key(0x2C, true, Mods::LMETA),
                key(0x2C, false, Mods::LMETA),
                key(0xE3, false, Mods::NONE),
            ],
        ),
        (
            SessionCombo::MissionControl,
            vec![
                key(0xE0, true, Mods::LCTRL),
                key(0x52, true, Mods::LCTRL),
                key(0x52, false, Mods::LCTRL),
                key(0xE0, false, Mods::NONE),
            ],
        ),
        (
            SessionCombo::AltTab,
            vec![
                key(0xE2, true, Mods::LALT),
                key(0x2B, true, Mods::LALT),
                key(0x2B, false, Mods::LALT),
                key(0xE2, false, Mods::NONE),
            ],
        ),
        (
            SessionCombo::WinKey,
            vec![key(0xE3, true, Mods::LMETA), key(0xE3, false, Mods::NONE)],
        ),
        (
            SessionCombo::CtrlSpace,
            vec![
                key(0xE0, true, Mods::LCTRL),
                key(0x2C, true, Mods::LCTRL),
                key(0x2C, false, Mods::LCTRL),
                key(0xE0, false, Mods::NONE),
            ],
        ),
        (
            SessionCombo::CmdH,
            vec![
                key(0xE3, true, Mods::LMETA),
                key(0x0B, true, Mods::LMETA),
                key(0x0B, false, Mods::LMETA),
                key(0xE3, false, Mods::NONE),
            ],
        ),
        (
            SessionCombo::CmdShift4,
            vec![
                key(0xE1, true, Mods::LSHIFT),
                key(0xE3, true, Mods::LSHIFT | Mods::LMETA),
                key(0x21, true, Mods::LSHIFT | Mods::LMETA),
                key(0x21, false, Mods::LSHIFT | Mods::LMETA),
                key(0xE3, false, Mods::LSHIFT),
                key(0xE1, false, Mods::NONE),
            ],
        ),
        (
            SessionCombo::WinSpace,
            vec![
                key(0xE3, true, Mods::LMETA),
                key(0x2C, true, Mods::LMETA),
                key(0x2C, false, Mods::LMETA),
                key(0xE3, false, Mods::NONE),
            ],
        ),
    ];

    for (combo, expected) in cases {
        let inputs = combo_inputs(combo);
        assert_eq!(inputs, expected, "{combo:?}");
        assert!(matches!(
            inputs.last(),
            Some(InputMsg::Key {
                mods: Mods::NONE,
                ..
            })
        ));

        let mut pressed = BTreeSet::new();
        for input in inputs {
            let InputMsg::Key { usage, down, mods } = input else {
                panic!("{combo:?} produced a non-key event");
            };
            if down {
                assert!(pressed.insert(usage), "duplicate key-down in {combo:?}");
            } else {
                assert!(pressed.remove(&usage), "unmatched key-up in {combo:?}");
            }

            let mut expected_mods = Mods::NONE;
            for (usage, bit) in [
                (0xE0, Mods::LCTRL),
                (0xE1, Mods::LSHIFT),
                (0xE2, Mods::LALT),
                (0xE3, Mods::LMETA),
            ] {
                if pressed.contains(&usage) {
                    expected_mods |= bit;
                }
            }
            assert_eq!(mods, expected_mods, "modifier state after usage {usage:#x}");
        }
        assert!(pressed.is_empty(), "{combo:?} left a key pressed");
    }
}

#[test]
pub fn public_hid_constants_match_the_keyboard_usage_page() {
    assert_eq!(
        [HID_H, HID_4, HID_TAB, HID_SPACE, HID_UP, HID_LCTRL, HID_LSHIFT, HID_LALT, HID_LGUI],
        [0x0B, 0x21, 0x2B, 0x2C, 0x52, 0xE0, 0xE1, 0xE2, 0xE3]
    );
}

#[test]
pub fn ipad_shortcut_names_match_types_ts() {
    for (name, combo) in [
        ("ctrlSpace", SessionCombo::CtrlSpace),
        ("cmdH", SessionCombo::CmdH),
        ("cmdShift4", SessionCombo::CmdShift4),
        ("winSpace", SessionCombo::WinSpace),
    ] {
        assert_eq!(serde_json::to_value(combo).unwrap(), name);
        assert_eq!(
            serde_json::from_value::<SessionCombo>(serde_json::json!(name)).unwrap(),
            combo
        );
    }
}
