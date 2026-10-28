//! Keyboard/Keypad page (0x07) usages to Windows Set 1 scan codes.
//!
//! A `WinKey` with `vk == 0` is injected with `KEYEVENTF_SCANCODE`; the caller
//! must add `KEYEVENTF_EXTENDEDKEY` when `extended` is set. The one exception
//! is Pause, which carries a virtual-key code instead. This module is pure and
//! does not call `SendInput` or any other platform API.

use rsp_core::hid::Mods;

/// A Windows keyboard key.
///
/// When `vk == 0`, inject by scan code; otherwise inject by virtual key. Only
/// Pause uses the virtual-key path.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct WinKey {
    pub scan: u16,
    pub extended: bool,
    pub vk: u16,
}

/// Convert a Keyboard/Keypad page usage to a Windows Set 1 key.
///
/// Every mapped Set 1 make code is below 0x80. Intentional aliases are limited
/// to 0x31/0x32 (US backslash and Non-US # are the same physical key) and
/// 0x67/0x86 (Keypad Equals and its AS/400 variant). All other mapped usages
/// have distinct `(scan, extended)` identities.
///
/// Pause uses virtual key 0x13 (`VK_PAUSE`) because its scan-code sequence is
/// `E1 1D 45`; `SendInput`'s scan-code path cannot express the `E1` prefix.
///
/// The following usages are deliberately unmapped:
/// - 0x66 Power: a remote client must not be able to power off the host.
/// - 0x75 Help: Windows has no distinct Set 1 physical key for it here.
/// - 0xD8 Keypad Clear: Windows has no independent key for it.
/// - 0x74 and 0x76..=0x7E (Execute, Menu/Select/Stop/Again/Undo/Cut/Copy/Paste/
///   Find): there are no corresponding Set 1 keyboard make codes.
/// - 0x82..=0x84: locking keys have no corresponding Set 1 make codes.
/// - 0x00..=0x03 and 0xE8..=0xFFFF: reserved, non-keyboard, or beyond the
///   side-specific modifier usages handled by this page.
pub fn hid_to_win(usage: u16) -> Option<WinKey> {
    let (scan, extended) = match usage {
        0x04 => (0x1E, false), // A
        0x05 => (0x30, false), // B
        0x06 => (0x2E, false), // C
        0x07 => (0x20, false), // D
        0x08 => (0x12, false), // E
        0x09 => (0x21, false), // F
        0x0A => (0x22, false), // G
        0x0B => (0x23, false), // H
        0x0C => (0x17, false), // I
        0x0D => (0x24, false), // J
        0x0E => (0x25, false), // K
        0x0F => (0x26, false), // L
        0x10 => (0x32, false), // M
        0x11 => (0x31, false), // N
        0x12 => (0x18, false), // O
        0x13 => (0x19, false), // P
        0x14 => (0x10, false), // Q
        0x15 => (0x13, false), // R
        0x16 => (0x1F, false), // S
        0x17 => (0x14, false), // T
        0x18 => (0x16, false), // U
        0x19 => (0x2F, false), // V
        0x1A => (0x11, false), // W
        0x1B => (0x2D, false), // X
        0x1C => (0x15, false), // Y
        0x1D => (0x2C, false), // Z
        0x1E => (0x02, false), // 1
        0x1F => (0x03, false), // 2
        0x20 => (0x04, false), // 3
        0x21 => (0x05, false), // 4
        0x22 => (0x06, false), // 5
        0x23 => (0x07, false), // 6
        0x24 => (0x08, false), // 7
        0x25 => (0x09, false), // 8
        0x26 => (0x0A, false), // 9
        0x27 => (0x0B, false), // 0
        0x28 => (0x1C, false), // Enter
        0x29 => (0x01, false), // Escape
        0x2A => (0x0E, false), // Backspace
        0x2B => (0x0F, false), // Tab
        0x2C => (0x39, false), // Space
        0x2D => (0x0C, false), // Minus
        0x2E => (0x0D, false), // Equal
        0x2F => (0x1A, false), // Left Bracket
        0x30 => (0x1B, false), // Right Bracket
        0x31 => (0x2B, false), // Backslash
        0x32 => (0x2B, false), // Non-US #; alias of 0x31
        0x33 => (0x27, false), // Semicolon
        0x34 => (0x28, false), // Apostrophe
        0x35 => (0x29, false), // Grave
        0x36 => (0x33, false), // Comma
        0x37 => (0x34, false), // Period
        0x38 => (0x35, false), // Slash
        0x39 => (0x3A, false), // Caps Lock
        0x3A => (0x3B, false), // F1
        0x3B => (0x3C, false), // F2
        0x3C => (0x3D, false), // F3
        0x3D => (0x3E, false), // F4
        0x3E => (0x3F, false), // F5
        0x3F => (0x40, false), // F6
        0x40 => (0x41, false), // F7
        0x41 => (0x42, false), // F8
        0x42 => (0x43, false), // F9
        0x43 => (0x44, false), // F10
        0x44 => (0x57, false), // F11
        0x45 => (0x58, false), // F12
        0x46 => (0x37, true),  // Print Screen
        0x47 => (0x46, false), // Scroll Lock
        0x48 => {
            return Some(WinKey {
                scan: 0x45,
                extended: false,
                vk: 0x13,
            })
        } // Pause
        0x49 => (0x52, true),  // Insert
        0x4A => (0x47, true),  // Home
        0x4B => (0x49, true),  // Page Up
        0x4C => (0x53, true),  // Delete
        0x4D => (0x4F, true),  // End
        0x4E => (0x51, true),  // Page Down
        0x4F => (0x4D, true),  // Right Arrow
        0x50 => (0x4B, true),  // Left Arrow
        0x51 => (0x50, true),  // Down Arrow
        0x52 => (0x48, true),  // Up Arrow
        0x53 => (0x45, true),  // Num Lock
        0x54 => (0x35, true),  // Keypad /
        0x55 => (0x37, false), // Keypad *
        0x56 => (0x4A, false), // Keypad -
        0x57 => (0x4E, false), // Keypad +
        0x58 => (0x1C, true),  // Keypad Enter
        0x59 => (0x4F, false), // Keypad 1
        0x5A => (0x50, false), // Keypad 2
        0x5B => (0x51, false), // Keypad 3
        0x5C => (0x4B, false), // Keypad 4
        0x5D => (0x4C, false), // Keypad 5
        0x5E => (0x4D, false), // Keypad 6
        0x5F => (0x47, false), // Keypad 7
        0x60 => (0x48, false), // Keypad 8
        0x61 => (0x49, false), // Keypad 9
        0x62 => (0x52, false), // Keypad 0
        0x63 => (0x53, false), // Keypad .
        0x64 => (0x56, false), // International Backslash
        0x65 => (0x5D, true),  // Context Menu
        0x67 => (0x59, false), // Keypad =
        0x68 => (0x64, false), // F13
        0x69 => (0x65, false), // F14
        0x6A => (0x66, false), // F15
        0x6B => (0x67, false), // F16
        0x6C => (0x68, false), // F17
        0x6D => (0x69, false), // F18
        0x6E => (0x6A, false), // F19
        0x6F => (0x6B, false), // F20
        0x70 => (0x6C, false), // F21
        0x71 => (0x6D, false), // F22
        0x72 => (0x6E, false), // F23
        0x73 => (0x76, false), // F24
        0x7F => (0x20, true),  // Mute
        0x80 => (0x30, true),  // Volume Up
        0x81 => (0x2E, true),  // Volume Down
        0x85 => (0x7E, false), // Keypad ,
        0x86 => (0x59, false), // Keypad = (AS/400); alias of 0x67
        0x87 => (0x73, false), // International1 (Ro)
        0x88 => (0x70, false), // Kana Mode
        0x89 => (0x7D, false), // International3 (Yen)
        0x8A => (0x79, false), // Convert
        0x8B => (0x7B, false), // Non-convert
        0x90 => (0x72, false), // LANG1
        0x91 => (0x71, false), // LANG2
        0xE0 => (0x1D, false), // Left Ctrl
        0xE1 => (0x2A, false), // Left Shift
        0xE2 => (0x38, false), // Left Alt
        0xE3 => (0x5B, true),  // Left Meta
        0xE4 => (0x1D, true),  // Right Ctrl
        0xE5 => (0x36, false), // Right Shift
        0xE6 => (0x38, true),  // Right Alt
        0xE7 => (0x5C, true),  // Right Meta
        _ => return None,
    };
    Some(WinKey {
        scan,
        extended,
        vk: 0,
    })
}

/// Return the [`Mods`] bit for a side-specific modifier usage (0xE0..=0xE7).
pub fn modifier_bit(usage: u16) -> Option<Mods> {
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

/// The eight side-specific modifier usages, in [`Mods`] bit order.
pub const MODIFIER_USAGES: [u16; 8] = [0xE0, 0xE1, 0xE2, 0xE3, 0xE4, 0xE5, 0xE6, 0xE7];
