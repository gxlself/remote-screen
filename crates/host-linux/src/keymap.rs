//! HID 键盘页用法 → evdev 键码、鼠标键、字符 → keysym（V8 Linux 注入用，纯查表）。

use rsp_core::hid::{Button, Mods};

/// Linux evdev button code for the primary mouse button.
pub const BTN_LEFT: u16 = 0x110;
/// Linux evdev button code for the secondary mouse button.
pub const BTN_RIGHT: u16 = 0x111;
/// Linux evdev button code for the middle mouse button.
pub const BTN_MIDDLE: u16 = 0x112;
/// Linux evdev button code for the back mouse button.
pub const BTN_SIDE: u16 = 0x113;
/// Linux evdev button code for the forward mouse button.
pub const BTN_EXTRA: u16 = 0x114;

/// Linux evdev key code for Enter.
pub const KEY_ENTER: u16 = 28;
/// Linux evdev key code for Tab.
pub const KEY_TAB: u16 = 15;

/// HID Keyboard/Keypad page usage to Linux evdev key code.
///
/// Usages 0x00–0x03 are reserved/error codes. Power (0x66) is intentionally
/// omitted so remote input cannot power off the host. Locking keys 0x82–0x84,
/// AS/400 Keypad Equals (0x86), and Clear (0x9C and 0xD8) are also omitted;
/// the latter is mapped to Delete by the kernel and could cause an unintended
/// deletion. Usages 0xE8 and above are vendor-specific or reserved. Every
/// other usage not listed below is unmapped as well.
pub fn hid_to_evdev(usage: u16) -> Option<u16> {
    Some(match usage {
        0x04 => 30,  // A
        0x05 => 48,  // B
        0x06 => 46,  // C
        0x07 => 32,  // D
        0x08 => 18,  // E
        0x09 => 33,  // F
        0x0A => 34,  // G
        0x0B => 35,  // H
        0x0C => 23,  // I
        0x0D => 36,  // J
        0x0E => 37,  // K
        0x0F => 38,  // L
        0x10 => 50,  // M
        0x11 => 49,  // N
        0x12 => 24,  // O
        0x13 => 25,  // P
        0x14 => 16,  // Q
        0x15 => 19,  // R
        0x16 => 31,  // S
        0x17 => 20,  // T
        0x18 => 22,  // U
        0x19 => 47,  // V
        0x1A => 17,  // W
        0x1B => 45,  // X
        0x1C => 21,  // Y
        0x1D => 44,  // Z
        0x1E => 2,   // 1
        0x1F => 3,   // 2
        0x20 => 4,   // 3
        0x21 => 5,   // 4
        0x22 => 6,   // 5
        0x23 => 7,   // 6
        0x24 => 8,   // 7
        0x25 => 9,   // 8
        0x26 => 10,  // 9
        0x27 => 11,  // 0
        0x28 => 28,  // Enter
        0x29 => 1,   // Escape
        0x2A => 14,  // Backspace
        0x2B => 15,  // Tab
        0x2C => 57,  // Space
        0x2D => 12,  // Minus
        0x2E => 13,  // Equal
        0x2F => 26,  // Left Brace
        0x30 => 27,  // Right Brace
        0x31 => 43,  // Backslash
        0x32 => 43,  // Non-US #; alias of Backslash
        0x33 => 39,  // Semicolon
        0x34 => 40,  // Apostrophe
        0x35 => 41,  // Grave
        0x36 => 51,  // Comma
        0x37 => 52,  // Dot
        0x38 => 53,  // Slash
        0x39 => 58,  // Caps Lock
        0x3A => 59,  // F1
        0x3B => 60,  // F2
        0x3C => 61,  // F3
        0x3D => 62,  // F4
        0x3E => 63,  // F5
        0x3F => 64,  // F6
        0x40 => 65,  // F7
        0x41 => 66,  // F8
        0x42 => 67,  // F9
        0x43 => 68,  // F10
        0x44 => 87,  // F11
        0x45 => 88,  // F12
        0x46 => 99,  // Print Screen (SysRq)
        0x47 => 70,  // Scroll Lock
        0x48 => 119, // Pause
        0x49 => 110, // Insert
        0x4A => 102, // Home
        0x4B => 104, // Page Up
        0x4C => 111, // Delete
        0x4D => 107, // End
        0x4E => 109, // Page Down
        0x4F => 106, // Right
        0x50 => 105, // Left
        0x51 => 108, // Down
        0x52 => 103, // Up
        0x53 => 69,  // Num Lock
        0x54 => 98,  // Keypad /
        0x55 => 55,  // Keypad *
        0x56 => 74,  // Keypad -
        0x57 => 78,  // Keypad +
        0x58 => 96,  // Keypad Enter
        0x59 => 79,  // Keypad 1
        0x5A => 80,  // Keypad 2
        0x5B => 81,  // Keypad 3
        0x5C => 75,  // Keypad 4
        0x5D => 76,  // Keypad 5
        0x5E => 77,  // Keypad 6
        0x5F => 71,  // Keypad 7
        0x60 => 72,  // Keypad 8
        0x61 => 73,  // Keypad 9
        0x62 => 82,  // Keypad 0
        0x63 => 83,  // Keypad .
        0x64 => 86,  // Non-US \\ (102ND)
        0x65 => 127, // Application (Compose)
        0x67 => 117, // Keypad =
        0x68 => 183, // F13
        0x69 => 184, // F14
        0x6A => 185, // F15
        0x6B => 186, // F16
        0x6C => 187, // F17
        0x6D => 188, // F18
        0x6E => 189, // F19
        0x6F => 190, // F20
        0x70 => 191, // F21
        0x71 => 192, // F22
        0x72 => 193, // F23
        0x73 => 194, // F24
        0x74 => 134, // Execute (Open)
        0x75 => 138, // Help
        0x76 => 130, // Menu (Props)
        0x77 => 132, // Select (Front)
        0x78 => 128, // Stop
        0x79 => 129, // Again
        0x7A => 131, // Undo
        0x7B => 137, // Cut
        0x7C => 133, // Copy
        0x7D => 135, // Paste
        0x7E => 136, // Find
        0x7F => 113, // Mute
        0x80 => 115, // Volume Up
        0x81 => 114, // Volume Down
        0x85 => 121, // Keypad Comma
        0x87 => 89,  // International1 (RO)
        0x88 => 93,  // International2 (Katakana/Hiragana)
        0x89 => 124, // International3 (Yen)
        0x8A => 92,  // International4 (Henkan)
        0x8B => 94,  // International5 (Muhenkan)
        0x8C => 95,  // International6 (Keypad Jp Comma)
        0x90 => 122, // LANG1 (Hangeul)
        0x91 => 123, // LANG2 (Hanja)
        0x92 => 90,  // LANG3 (Katakana)
        0x93 => 91,  // LANG4 (Hiragana)
        0x94 => 85,  // LANG5 (Zenkaku/Hankaku)
        0xB6 => 179, // Keypad (
        0xB7 => 180, // Keypad )
        0xE0 => 29,  // Left Ctrl
        0xE1 => 42,  // Left Shift
        0xE2 => 56,  // Left Alt
        0xE3 => 125, // Left Meta
        0xE4 => 97,  // Right Ctrl
        0xE5 => 54,  // Right Shift
        0xE6 => 100, // Right Alt
        0xE7 => 126, // Right Meta
        _ => return None,
    })
}

/// Convert an evdev key code to the corresponding X11 keycode.
///
/// X11 keycodes are evdev codes offset by eight. Values above 247 (and button
/// codes such as [`BTN_LEFT`]) therefore have no representable X11 keycode.
pub fn evdev_to_x11(code: u16) -> Option<u8> {
    code.checked_add(8).and_then(|code| u8::try_from(code).ok())
}

/// Convert an RSP mouse button to its Linux evdev button code.
pub const fn button_to_evdev(button: Button) -> u16 {
    match button {
        Button::Left => BTN_LEFT,
        Button::Right => BTN_RIGHT,
        Button::Middle => BTN_MIDDLE,
        Button::Back => BTN_SIDE,
        Button::Forward => BTN_EXTRA,
    }
}

/// Convert an RSP mouse button to its X11 button number.
pub const fn button_to_x11(button: Button) -> u8 {
    match button {
        Button::Left => 1,
        Button::Middle => 2,
        Button::Right => 3,
        Button::Back => 8,
        Button::Forward => 9,
    }
}

/// Side-specific modifier bits and their HID usages, in protocol bit order.
pub const MODIFIER_USAGES: [(Mods, u16); 8] = [
    (Mods::LCTRL, 0xE0),
    (Mods::LSHIFT, 0xE1),
    (Mods::LALT, 0xE2),
    (Mods::LMETA, 0xE3),
    (Mods::RCTRL, 0xE4),
    (Mods::RSHIFT, 0xE5),
    (Mods::RALT, 0xE6),
    (Mods::RMETA, 0xE7),
];

/// Return the [`Mods`] bit for a side-specific modifier usage (0xE0–0xE7).
pub const fn modifier_bit(usage: u16) -> Option<Mods> {
    Some(match usage {
        0xE0 => Mods::LCTRL,
        0xE1 => Mods::LSHIFT,
        0xE2 => Mods::LALT,
        0xE3 => Mods::LMETA,
        0xE4 => Mods::RCTRL,
        0xE5 => Mods::RSHIFT,
        0xE6 => Mods::RALT,
        0xE7 => Mods::RMETA,
        _ => return None,
    })
}

/// Convert one Unicode scalar to an X11 keysym.
///
/// Printable ASCII (U+0020–U+007E) and Latin-1 supplement characters
/// (U+00A0–U+00FF) use their code points directly. Other Unicode scalars use
/// the X11 Unicode keysym range (`0x0100_0000 | code point`). C0 controls,
/// DEL, and C1 controls are intentionally unsupported.
pub fn char_to_keysym(c: char) -> Option<u32> {
    let codepoint = u32::from(c);
    match codepoint {
        0x20..=0x7E | 0xA0..=0xFF => Some(codepoint),
        0x100.. => Some(0x0100_0000 | codepoint),
        _ => None,
    }
}
