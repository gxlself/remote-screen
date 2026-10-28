//! Keyboard/Keypad page (0x07) usages to HIToolbox/Events.h virtual keycodes.
//!
//! Sources: USB HID Usage Tables 1.4, chapter 10; macOS SDK Events.h and
//! IOKit/hidsystem/IOLLEvent.h. Numeric literals are annotated with their SDK
//! constant names; importing those headers is deliberately unnecessary.

use rsp_core::hid::Mods;

/// Physical keyboard geometry, independent of the active input language.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacLayout {
    Ansi,
    Iso,
    Jis,
}

/// Convert a Keyboard/Keypad page usage to a macOS virtual keycode.
///
/// Unknown usages, unsupported extended keypad operations and F21+ return
/// `None`; the caller should discard and count them. Fn is a vendor-specific
/// usage, not a standard page-0x07 key, so it is deliberately not mapped.
///
/// ISO policy: retain the canonical Events.h identities: usage 0x35 is
/// `kVK_ANSI_Grave` (0x32), and 0x64 is `kVK_ISO_Section` (0x0A). Apple's
/// IOHIDKeyboard.cpp `handlerID()` instead swaps those two USB-to-ADB entries
/// on ISO devices (0x35 -> 0x0A, 0x64 -> 0x32). That is a hardware-driver
/// convention, not applied by this canonical CGEvent mapping. The Mac posting
/// layer must account for its event keyboard type if it needs that native ISO
/// convention; it must not silently swap this table a second time.
/// See <https://github.com/apple-oss-distributions/IOHIDFamily/blob/main/IOHIDFamily/IOHIDKeyboard.cpp>.
///
/// Intentional aliases (all other mapped usages have distinct keycodes):
/// - 0x31/0x32: US backslash and Non-US # describe the same physical key.
/// - 0x49/0x75: Insert and Help use the Mac Help key (no separate Insert VK).
/// - 0x53/0xD8: Num Lock/Clear and Keypad Clear use KeypadClear.
/// - 0x67/0x86: Keypad Equals and its AS/400 variant use KeypadEquals.
/// - JIS 0x88/0x8A/0x90: International2, Henkan and LANG1 select Kana.
/// - JIS 0x8B/0x91: Muhenkan and LANG2 select Eisu.
///
/// Henkan/Muhenkan have no distinct Events.h VKs. Following Mac input-mode
/// keys, Henkan selects Kana and Muhenkan selects Eisu; this does not emulate
/// the separate Windows conversion/non-conversion commands.
pub fn hid_to_vk(usage: u16, layout: MacLayout) -> Option<u16> {
    let vk = match usage {
        0x04 => 0x00,                                           // kVK_ANSI_A
        0x05 => 0x0B,                                           // kVK_ANSI_B
        0x06 => 0x08,                                           // kVK_ANSI_C
        0x07 => 0x02,                                           // kVK_ANSI_D
        0x08 => 0x0E,                                           // kVK_ANSI_E
        0x09 => 0x03,                                           // kVK_ANSI_F
        0x0A => 0x05,                                           // kVK_ANSI_G
        0x0B => 0x04,                                           // kVK_ANSI_H
        0x0C => 0x22,                                           // kVK_ANSI_I
        0x0D => 0x26,                                           // kVK_ANSI_J
        0x0E => 0x28,                                           // kVK_ANSI_K
        0x0F => 0x25,                                           // kVK_ANSI_L
        0x10 => 0x2E,                                           // kVK_ANSI_M
        0x11 => 0x2D,                                           // kVK_ANSI_N
        0x12 => 0x1F,                                           // kVK_ANSI_O
        0x13 => 0x23,                                           // kVK_ANSI_P
        0x14 => 0x0C,                                           // kVK_ANSI_Q
        0x15 => 0x0F,                                           // kVK_ANSI_R
        0x16 => 0x01,                                           // kVK_ANSI_S
        0x17 => 0x11,                                           // kVK_ANSI_T
        0x18 => 0x20,                                           // kVK_ANSI_U
        0x19 => 0x09,                                           // kVK_ANSI_V
        0x1A => 0x0D,                                           // kVK_ANSI_W
        0x1B => 0x07,                                           // kVK_ANSI_X
        0x1C => 0x10,                                           // kVK_ANSI_Y
        0x1D => 0x06,                                           // kVK_ANSI_Z
        0x1E => 0x12,                                           // kVK_ANSI_1
        0x1F => 0x13,                                           // kVK_ANSI_2
        0x20 => 0x14,                                           // kVK_ANSI_3
        0x21 => 0x15,                                           // kVK_ANSI_4
        0x22 => 0x17,                                           // kVK_ANSI_5
        0x23 => 0x16,                                           // kVK_ANSI_6
        0x24 => 0x1A,                                           // kVK_ANSI_7
        0x25 => 0x1C,                                           // kVK_ANSI_8
        0x26 => 0x19,                                           // kVK_ANSI_9
        0x27 => 0x1D,                                           // kVK_ANSI_0
        0x28 => 0x24,                                           // kVK_Return
        0x29 => 0x35,                                           // kVK_Escape
        0x2A => 0x33,                                           // kVK_Delete (backspace)
        0x2B => 0x30,                                           // kVK_Tab
        0x2C => 0x31,                                           // kVK_Space
        0x2D => 0x1B,                                           // kVK_ANSI_Minus
        0x2E => 0x18,                                           // kVK_ANSI_Equal
        0x2F => 0x21,                                           // kVK_ANSI_LeftBracket
        0x30 => 0x1E,                                           // kVK_ANSI_RightBracket
        0x31 | 0x32 => 0x2A,                                    // kVK_ANSI_Backslash
        0x33 => 0x29,                                           // kVK_ANSI_Semicolon
        0x34 => 0x27,                                           // kVK_ANSI_Quote
        0x35 => 0x32,        // kVK_ANSI_Grave; canonical ISO policy above
        0x36 => 0x2B,        // kVK_ANSI_Comma
        0x37 => 0x2F,        // kVK_ANSI_Period
        0x38 => 0x2C,        // kVK_ANSI_Slash
        0x39 => 0x39,        // kVK_CapsLock
        0x3A => 0x7A,        // kVK_F1
        0x3B => 0x78,        // kVK_F2
        0x3C => 0x63,        // kVK_F3
        0x3D => 0x76,        // kVK_F4
        0x3E => 0x60,        // kVK_F5
        0x3F => 0x61,        // kVK_F6
        0x40 => 0x62,        // kVK_F7
        0x41 => 0x64,        // kVK_F8
        0x42 => 0x65,        // kVK_F9
        0x43 => 0x6D,        // kVK_F10
        0x44 => 0x67,        // kVK_F11
        0x45 => 0x6F,        // kVK_F12
        0x49 | 0x75 => 0x72, // kVK_Help
        0x4A => 0x73,        // kVK_Home
        0x4B => 0x74,        // kVK_PageUp
        0x4C => 0x75,        // kVK_ForwardDelete
        0x4D => 0x77,        // kVK_End
        0x4E => 0x79,        // kVK_PageDown
        0x4F => 0x7C,        // kVK_RightArrow
        0x50 => 0x7B,        // kVK_LeftArrow
        0x51 => 0x7D,        // kVK_DownArrow
        0x52 => 0x7E,        // kVK_UpArrow
        0x53 | 0xD8 => 0x47, // kVK_ANSI_KeypadClear
        0x54 => 0x4B,        // kVK_ANSI_KeypadDivide
        0x55 => 0x43,        // kVK_ANSI_KeypadMultiply
        0x56 => 0x4E,        // kVK_ANSI_KeypadMinus
        0x57 => 0x45,        // kVK_ANSI_KeypadPlus
        0x58 => 0x4C,        // kVK_ANSI_KeypadEnter
        0x59 => 0x53,        // kVK_ANSI_Keypad1
        0x5A => 0x54,        // kVK_ANSI_Keypad2
        0x5B => 0x55,        // kVK_ANSI_Keypad3
        0x5C => 0x56,        // kVK_ANSI_Keypad4
        0x5D => 0x57,        // kVK_ANSI_Keypad5
        0x5E => 0x58,        // kVK_ANSI_Keypad6
        0x5F => 0x59,        // kVK_ANSI_Keypad7
        0x60 => 0x5B,        // kVK_ANSI_Keypad8
        0x61 => 0x5C,        // kVK_ANSI_Keypad9
        0x62 => 0x52,        // kVK_ANSI_Keypad0
        0x63 => 0x41,        // kVK_ANSI_KeypadDecimal
        0x64 if layout == MacLayout::Iso => 0x0A, // kVK_ISO_Section
        0x67 | 0x86 => 0x51, // kVK_ANSI_KeypadEquals
        0x68 => 0x69,        // kVK_F13
        0x69 => 0x6B,        // kVK_F14
        0x6A => 0x71,        // kVK_F15
        0x6B => 0x6A,        // kVK_F16
        0x6C => 0x40,        // kVK_F17
        0x6D => 0x4F,        // kVK_F18
        0x6E => 0x50,        // kVK_F19
        0x6F => 0x5A,        // kVK_F20
        0x7F => 0x4A,        // kVK_Mute
        0x80 => 0x48,        // kVK_VolumeUp
        0x81 => 0x49,        // kVK_VolumeDown
        0x85 if layout == MacLayout::Jis => 0x5F, // kVK_JIS_KeypadComma
        0x87 if layout == MacLayout::Jis => 0x5E, // kVK_JIS_Underscore
        0x89 if layout == MacLayout::Jis => 0x5D, // kVK_JIS_Yen
        0x88 | 0x8A | 0x90 if layout == MacLayout::Jis => 0x68, // kVK_JIS_Kana
        0x8B | 0x91 if layout == MacLayout::Jis => 0x66, // kVK_JIS_Eisu
        _ => return modifier_usage_to_vk(usage),
    };
    Some(vk)
}

/// Map the eight HID side-specific modifiers, preserving left/right identity.
pub fn modifier_usage_to_vk(usage: u16) -> Option<u16> {
    Some(match usage {
        0xE0 => 0x3B, // kVK_Control
        0xE1 => 0x38, // kVK_Shift
        0xE2 => 0x3A, // kVK_Option
        0xE3 => 0x37, // kVK_Command
        0xE4 => 0x3E, // kVK_RightControl
        0xE5 => 0x3C, // kVK_RightShift
        0xE6 => 0x3D, // kVK_RightOption
        0xE7 => 0x36, // kVK_RightCommand
        _ => return None,
    })
}

/// Recognize the eight side-specific modifiers, Caps Lock and Fn.
///
/// Fn (`kVK_Function`) is recognized here for native event classification,
/// even though [`hid_to_vk`] never produces it.
pub fn vk_is_modifier(vk: u16) -> bool {
    // Events.h assigns consecutive values to kVK_RightCommand, kVK_Command,
    // kVK_Shift, kVK_CapsLock, kVK_Option, kVK_Control, kVK_RightShift,
    // kVK_RightOption, kVK_RightControl and kVK_Function, respectively.
    matches!(vk, 0x36..=0x3F) // kVK_RightCommand ..= kVK_Function
}

/// Convert protocol modifiers to CGEventFlags, including device-specific bits.
///
/// The four aggregate flags are set when either side is pressed. Caps Lock and
/// Fn have no bit in [`Mods`], so this function does not synthesize those flags.
pub fn mods_to_cg_flags(m: Mods) -> u64 {
    const FLAGS: [(Mods, u64); 8] = [
        (Mods::LCTRL, (1 << 18) | 0x0001), // kCGEventFlagMaskControl | NX_DEVICELCTLKEYMASK
        (Mods::LSHIFT, (1 << 17) | 0x0002), // kCGEventFlagMaskShift | NX_DEVICELSHIFTKEYMASK
        (Mods::LALT, (1 << 19) | 0x0020),  // kCGEventFlagMaskAlternate | NX_DEVICELALTKEYMASK
        (Mods::LMETA, (1 << 20) | 0x0008), // kCGEventFlagMaskCommand | NX_DEVICELCMDKEYMASK
        (Mods::RCTRL, (1 << 18) | 0x2000), // kCGEventFlagMaskControl | NX_DEVICERCTLKEYMASK
        (Mods::RSHIFT, (1 << 17) | 0x0004), // kCGEventFlagMaskShift | NX_DEVICERSHIFTKEYMASK
        (Mods::RALT, (1 << 19) | 0x0040),  // kCGEventFlagMaskAlternate | NX_DEVICERALTKEYMASK
        (Mods::RMETA, (1 << 20) | 0x0010), // kCGEventFlagMaskCommand | NX_DEVICERCMDKEYMASK
    ];
    FLAGS.iter().fold(0, |flags, &(modifier, mask)| {
        if m.contains(modifier) {
            flags | mask
        } else {
            flags
        }
    })
}
