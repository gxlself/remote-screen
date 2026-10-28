//! Toolbar shortcuts expressed as USB HID key events (RSP/1 §6.1).

use rsp_core::hid::{InputMsg, Mods};

use super::dto_core::SessionCombo;

pub const HID_H: u16 = 0x0B;
pub const HID_4: u16 = 0x21;
pub const HID_TAB: u16 = 0x2B;
pub const HID_SPACE: u16 = 0x2C;
pub const HID_UP: u16 = 0x52;
pub const HID_LCTRL: u16 = 0xE0;
pub const HID_LSHIFT: u16 = 0xE1;
pub const HID_LALT: u16 = 0xE2;
pub const HID_LGUI: u16 = 0xE3;

fn modifier_bit(usage: u16) -> Mods {
    match usage {
        HID_LCTRL => Mods::LCTRL,
        HID_LSHIFT => Mods::LSHIFT,
        HID_LALT => Mods::LALT,
        _ => Mods::LMETA,
    }
}

/// Each event carries the modifier state after that event; all keys end up released.
pub fn combo_inputs(combo: SessionCombo) -> Vec<InputMsg> {
    let (modifiers, key): (&[u16], Option<u16>) = match combo {
        SessionCombo::CmdTab => (&[HID_LGUI], Some(HID_TAB)),
        SessionCombo::CmdSpace => (&[HID_LGUI], Some(HID_SPACE)),
        // macOS's default Mission Control shortcut is Ctrl+Up.
        SessionCombo::MissionControl => (&[HID_LCTRL], Some(HID_UP)),
        SessionCombo::AltTab => (&[HID_LALT], Some(HID_TAB)),
        SessionCombo::WinKey => (&[HID_LGUI], None),
        // macOS's default "Select the previous input source".
        SessionCombo::CtrlSpace => (&[HID_LCTRL], Some(HID_SPACE)),
        SessionCombo::CmdH => (&[HID_LGUI], Some(HID_H)),
        SessionCombo::CmdShift4 => (&[HID_LSHIFT, HID_LGUI], Some(HID_4)),
        SessionCombo::WinSpace => (&[HID_LGUI], Some(HID_SPACE)),
    };
    let mut mods = Mods::NONE;
    let mut inputs = Vec::with_capacity(modifiers.len() * 2 + 2);
    for &usage in modifiers {
        mods |= modifier_bit(usage);
        inputs.push(InputMsg::Key {
            usage,
            down: true,
            mods,
        });
    }
    if let Some(usage) = key {
        inputs.extend([
            InputMsg::Key {
                usage,
                down: true,
                mods,
            },
            InputMsg::Key {
                usage,
                down: false,
                mods,
            },
        ]);
    }
    for &usage in modifiers.iter().rev() {
        mods = Mods::from_bits(mods.bits() & !modifier_bit(usage).bits());
        inputs.push(InputMsg::Key {
            usage,
            down: false,
            mods,
        });
    }
    inputs
}
