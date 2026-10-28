//! 本机键码 → USB HID 用法码（Keyboard page 0x07），是 host-core 正向表的反查（03 §6.1、D22）。
//!
//! 这些函数只处理数值，不调用平台 API。Mac 和 Windows 的正向表分别位于
//! `host-core` 的注入模块；这里保留一份静态反查表，供桌面捕获层使用。

/// Mac 物理键盘类型（来自 `KBGetLayoutType(LMGetKbdType())`，由 Mac 窗口层传入）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum MacKeyboardKind {
    #[default]
    Ansi,
    Iso,
    Jis,
}

/// Mac `NSEvent.keyCode` → HID。不认识的键 → `None`。
pub fn mac_vk_to_hid(vk: u16, kind: MacKeyboardKind) -> Option<u16> {
    let usage = match vk {
        0x00 => 0x04,                                 // kVK_ANSI_A
        0x0B => 0x05,                                 // kVK_ANSI_B
        0x08 => 0x06,                                 // kVK_ANSI_C
        0x02 => 0x07,                                 // kVK_ANSI_D
        0x0E => 0x08,                                 // kVK_ANSI_E
        0x03 => 0x09,                                 // kVK_ANSI_F
        0x05 => 0x0A,                                 // kVK_ANSI_G
        0x04 => 0x0B,                                 // kVK_ANSI_H
        0x22 => 0x0C,                                 // kVK_ANSI_I
        0x26 => 0x0D,                                 // kVK_ANSI_J
        0x28 => 0x0E,                                 // kVK_ANSI_K
        0x25 => 0x0F,                                 // kVK_ANSI_L
        0x2E => 0x10,                                 // kVK_ANSI_M
        0x2D => 0x11,                                 // kVK_ANSI_N
        0x1F => 0x12,                                 // kVK_ANSI_O
        0x23 => 0x13,                                 // kVK_ANSI_P
        0x0C => 0x14,                                 // kVK_ANSI_Q
        0x0F => 0x15,                                 // kVK_ANSI_R
        0x01 => 0x16,                                 // kVK_ANSI_S
        0x11 => 0x17,                                 // kVK_ANSI_T
        0x20 => 0x18,                                 // kVK_ANSI_U
        0x09 => 0x19,                                 // kVK_ANSI_V
        0x0D => 0x1A,                                 // kVK_ANSI_W
        0x07 => 0x1B,                                 // kVK_ANSI_X
        0x10 => 0x1C,                                 // kVK_ANSI_Y
        0x06 => 0x1D,                                 // kVK_ANSI_Z
        0x12 => 0x1E,                                 // kVK_ANSI_1
        0x13 => 0x1F,                                 // kVK_ANSI_2
        0x14 => 0x20,                                 // kVK_ANSI_3
        0x15 => 0x21,                                 // kVK_ANSI_4
        0x17 => 0x22,                                 // kVK_ANSI_5
        0x16 => 0x23,                                 // kVK_ANSI_6
        0x1A => 0x24,                                 // kVK_ANSI_7
        0x1C => 0x25,                                 // kVK_ANSI_8
        0x19 => 0x26,                                 // kVK_ANSI_9
        0x1D => 0x27,                                 // kVK_ANSI_0
        0x24 => 0x28,                                 // kVK_Return
        0x35 => 0x29,                                 // kVK_Escape
        0x33 => 0x2A,                                 // kVK_Delete (backspace)
        0x30 => 0x2B,                                 // kVK_Tab
        0x31 => 0x2C,                                 // kVK_Space
        0x1B => 0x2D,                                 // kVK_ANSI_Minus
        0x18 => 0x2E,                                 // kVK_ANSI_Equal
        0x21 => 0x2F,                                 // kVK_ANSI_LeftBracket
        0x1E => 0x30,                                 // kVK_ANSI_RightBracket
        0x2A => 0x31,                                 // kVK_ANSI_Backslash; 0x32 is its alias
        0x29 => 0x33,                                 // kVK_ANSI_Semicolon
        0x27 => 0x34,                                 // kVK_ANSI_Quote
        0x32 if kind == MacKeyboardKind::Iso => 0x64, // kVK_ANSI_Grave on ISO hardware
        0x32 => 0x35,                                 // kVK_ANSI_Grave
        0x2B => 0x36,                                 // kVK_ANSI_Comma
        0x2F => 0x37,                                 // kVK_ANSI_Period
        0x2C => 0x38,                                 // kVK_ANSI_Slash
        0x39 => 0x39,                                 // kVK_CapsLock
        0x7A => 0x3A,                                 // kVK_F1
        0x78 => 0x3B,                                 // kVK_F2
        0x63 => 0x3C,                                 // kVK_F3
        0x76 => 0x3D,                                 // kVK_F4
        0x60 => 0x3E,                                 // kVK_F5
        0x61 => 0x3F,                                 // kVK_F6
        0x62 => 0x40,                                 // kVK_F7
        0x64 => 0x41,                                 // kVK_F8
        0x65 => 0x42,                                 // kVK_F9
        0x6D => 0x43,                                 // kVK_F10
        0x67 => 0x44,                                 // kVK_F11
        0x6F => 0x45,                                 // kVK_F12
        0x72 => 0x49,                                 // kVK_Help; 0x75 (Insert) is its alias
        0x73 => 0x4A,                                 // kVK_Home
        0x74 => 0x4B,                                 // kVK_PageUp
        0x75 => 0x4C,                                 // kVK_ForwardDelete
        0x77 => 0x4D,                                 // kVK_End
        0x79 => 0x4E,                                 // kVK_PageDown
        0x7C => 0x4F,                                 // kVK_RightArrow
        0x7B => 0x50,                                 // kVK_LeftArrow
        0x7D => 0x51,                                 // kVK_DownArrow
        0x7E => 0x52,                                 // kVK_UpArrow
        0x47 => 0x53,                                 // kVK_ANSI_KeypadClear; 0xD8 is its alias
        0x4B => 0x54,                                 // kVK_ANSI_KeypadDivide
        0x43 => 0x55,                                 // kVK_ANSI_KeypadMultiply
        0x4E => 0x56,                                 // kVK_ANSI_KeypadMinus
        0x45 => 0x57,                                 // kVK_ANSI_KeypadPlus
        0x4C => 0x58,                                 // kVK_ANSI_KeypadEnter
        0x53 => 0x59,                                 // kVK_ANSI_Keypad1
        0x54 => 0x5A,                                 // kVK_ANSI_Keypad2
        0x55 => 0x5B,                                 // kVK_ANSI_Keypad3
        0x56 => 0x5C,                                 // kVK_ANSI_Keypad4
        0x57 => 0x5D,                                 // kVK_ANSI_Keypad5
        0x58 => 0x5E,                                 // kVK_ANSI_Keypad6
        0x59 => 0x5F,                                 // kVK_ANSI_Keypad7
        0x5B => 0x60,                                 // kVK_ANSI_Keypad8
        0x5C => 0x61,                                 // kVK_ANSI_Keypad9
        0x52 => 0x62,                                 // kVK_ANSI_Keypad0
        0x41 => 0x63,                                 // kVK_ANSI_KeypadDecimal
        0x0A if kind == MacKeyboardKind::Iso => 0x35, // kVK_ISO_Section (native ISO swap)
        0x51 => 0x67,                                 // kVK_ANSI_KeypadEquals; 0x86 is its alias
        0x69 => 0x68,                                 // kVK_F13
        0x6B => 0x69,                                 // kVK_F14
        0x71 => 0x6A,                                 // kVK_F15
        0x6A => 0x6B,                                 // kVK_F16
        0x40 => 0x6C,                                 // kVK_F17
        0x4F => 0x6D,                                 // kVK_F18
        0x50 => 0x6E,                                 // kVK_F19
        0x5A => 0x6F,                                 // kVK_F20
        0x4A => 0x7F,                                 // kVK_Mute
        0x48 => 0x80,                                 // kVK_VolumeUp
        0x49 => 0x81,                                 // kVK_VolumeDown
        0x5F if kind == MacKeyboardKind::Jis => 0x85, // kVK_JIS_KeypadComma
        0x5E if kind == MacKeyboardKind::Jis => 0x87, // kVK_JIS_Underscore
        0x5D if kind == MacKeyboardKind::Jis => 0x89, // kVK_JIS_Yen
        0x68 if kind == MacKeyboardKind::Jis => 0x90, // kVK_JIS_Kana; LANG1 is the chosen alias
        0x66 if kind == MacKeyboardKind::Jis => 0x91, // kVK_JIS_Eisu; LANG2 is the chosen alias
        0x3B => 0xE0,                                 // kVK_Control
        0x38 => 0xE1,                                 // kVK_Shift
        0x3A => 0xE2,                                 // kVK_Option
        0x37 => 0xE3,                                 // kVK_Command
        0x3E => 0xE4,                                 // kVK_RightControl
        0x3C => 0xE5,                                 // kVK_RightShift
        0x3D => 0xE6,                                 // kVK_RightOption
        0x36 => 0xE7,                                 // kVK_RightCommand
        _ => return None,
    };
    Some(usage)
}

/// Mac 修饰键 keyCode → (HID, NX 设备相关位)。CapsLock、Fn 不在内。
pub const MAC_MODIFIERS: [(u16, u16, u64); 8] = [
    (0x3B, 0xE0, 0x0001),
    (0x38, 0xE1, 0x0002),
    (0x3A, 0xE2, 0x0020),
    (0x37, 0xE3, 0x0008),
    (0x3E, 0xE4, 0x2000),
    (0x3C, 0xE5, 0x0004),
    (0x3D, 0xE6, 0x0040),
    (0x36, 0xE7, 0x0010),
];

pub const MAC_VK_CAPS_LOCK: u16 = 0x39;
pub const MAC_VK_FUNCTION: u16 = 0x3F;
pub const HID_CAPS_LOCK: u16 = 0x39;

/// Mac flagsChanged 的解码结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MacFlagsChange {
    /// 一个左右区分的修饰键按下 / 松开。
    Modifier { usage: u16, down: bool },
    /// CapsLock 被按了一次（macOS 只在切换时发 flagsChanged）。调用方发一对 down/up。
    CapsLockPressed,
    /// Fn 或未知 keyCode：忽略。
    Ignored,
}

/// `vk` = NSEvent.keyCode，`flags` = NSEvent.modifierFlags.rawValue。
/// 修饰键是按下还是松开，看 `flags` 里该键自己的设备相关位。不看聚合位，因为另一侧同名键可能还按着。
pub fn mac_flags_change(vk: u16, flags: u64) -> MacFlagsChange {
    if let Some(&(_, usage, device_bit)) = MAC_MODIFIERS
        .iter()
        .find(|&&(key_code, _, _)| key_code == vk)
    {
        return MacFlagsChange::Modifier {
            usage,
            down: flags & device_bit != 0,
        };
    }
    if vk == MAC_VK_CAPS_LOCK {
        return MacFlagsChange::CapsLockPressed;
    }
    MacFlagsChange::Ignored
}

/// Windows Set 1 扫描码 + 扩展位 → HID。
pub fn win_scan_to_hid(scan: u16, extended: bool) -> Option<u16> {
    let usage = match (scan, extended) {
        (0, _) => return None,
        (0x1E, false) => 0x04,                      // A
        (0x30, false) => 0x05,                      // B
        (0x2E, false) => 0x06,                      // C
        (0x20, false) => 0x07,                      // D
        (0x12, false) => 0x08,                      // E
        (0x21, false) => 0x09,                      // F
        (0x22, false) => 0x0A,                      // G
        (0x23, false) => 0x0B,                      // H
        (0x17, false) => 0x0C,                      // I
        (0x24, false) => 0x0D,                      // J
        (0x25, false) => 0x0E,                      // K
        (0x26, false) => 0x0F,                      // L
        (0x32, false) => 0x10,                      // M
        (0x31, false) => 0x11,                      // N
        (0x18, false) => 0x12,                      // O
        (0x19, false) => 0x13,                      // P
        (0x10, false) => 0x14,                      // Q
        (0x13, false) => 0x15,                      // R
        (0x1F, false) => 0x16,                      // S
        (0x14, false) => 0x17,                      // T
        (0x16, false) => 0x18,                      // U
        (0x2F, false) => 0x19,                      // V
        (0x11, false) => 0x1A,                      // W
        (0x2D, false) => 0x1B,                      // X
        (0x15, false) => 0x1C,                      // Y
        (0x2C, false) => 0x1D,                      // Z
        (0x02, false) => 0x1E,                      // 1
        (0x03, false) => 0x1F,                      // 2
        (0x04, false) => 0x20,                      // 3
        (0x05, false) => 0x21,                      // 4
        (0x06, false) => 0x22,                      // 5
        (0x07, false) => 0x23,                      // 6
        (0x08, false) => 0x24,                      // 7
        (0x09, false) => 0x25,                      // 8
        (0x0A, false) => 0x26,                      // 9
        (0x0B, false) => 0x27,                      // 0
        (0x1C, false) => 0x28,                      // Enter
        (0x01, false) => 0x29,                      // Escape
        (0x0E, false) => 0x2A,                      // Backspace
        (0x0F, false) => 0x2B,                      // Tab
        (0x39, false) => 0x2C,                      // Space
        (0x0C, false) => 0x2D,                      // Minus
        (0x0D, false) => 0x2E,                      // Equal
        (0x1A, false) => 0x2F,                      // Left Bracket
        (0x1B, false) => 0x30,                      // Right Bracket
        (0x2B, false) => 0x31,                      // Backslash; 0x32 is its alias
        (0x27, false) => 0x33,                      // Semicolon
        (0x28, false) => 0x34,                      // Apostrophe
        (0x29, false) => 0x35,                      // Grave
        (0x33, false) => 0x36,                      // Comma
        (0x34, false) => 0x37,                      // Period
        (0x35, false) => 0x38,                      // Slash
        (0x3A, false) => 0x39,                      // Caps Lock
        (0x3B, false) => 0x3A,                      // F1
        (0x3C, false) => 0x3B,                      // F2
        (0x3D, false) => 0x3C,                      // F3
        (0x3E, false) => 0x3D,                      // F4
        (0x3F, false) => 0x3E,                      // F5
        (0x40, false) => 0x3F,                      // F6
        (0x41, false) => 0x40,                      // F7
        (0x42, false) => 0x41,                      // F8
        (0x43, false) => 0x42,                      // F9
        (0x44, false) => 0x43,                      // F10
        (0x57, false) => 0x44,                      // F11
        (0x58, false) => 0x45,                      // F12
        (0x37, true) => 0x46,                       // Print Screen
        (0x46, false) => 0x47,                      // Scroll Lock
        (0x45, false) => 0x48,                      // Pause (VK_PAUSE)
        (0x52, true) => 0x49,                       // Insert
        (0x47, true) => 0x4A,                       // Home
        (0x49, true) => 0x4B,                       // Page Up
        (0x53, true) => 0x4C,                       // Delete
        (0x4F, true) => 0x4D,                       // End
        (0x51, true) => 0x4E,                       // Page Down
        (0x4D, true) => 0x4F,                       // Right Arrow
        (0x4B, true) => 0x50,                       // Left Arrow
        (0x50, true) => 0x51,                       // Down Arrow
        (0x48, true) => 0x52,                       // Up Arrow
        (0x45, true) => 0x53,                       // Num Lock
        (0x35, true) => 0x54,                       // Keypad /
        (0x37, false) => 0x55,                      // Keypad *
        (0x4A, false) => 0x56,                      // Keypad -
        (0x4E, false) => 0x57,                      // Keypad +
        (0x1C, true) => 0x58,                       // Keypad Enter
        (0x4F, false) => 0x59,                      // Keypad 1
        (0x50, false) => 0x5A,                      // Keypad 2
        (0x51, false) => 0x5B,                      // Keypad 3
        (0x4B, false) => 0x5C,                      // Keypad 4
        (0x4C, false) => 0x5D,                      // Keypad 5
        (0x4D, false) => 0x5E,                      // Keypad 6
        (0x47, false) => 0x5F,                      // Keypad 7
        (0x48, false) => 0x60,                      // Keypad 8
        (0x49, false) => 0x61,                      // Keypad 9
        (0x52, false) => 0x62,                      // Keypad 0
        (0x53, false) => 0x63,                      // Keypad .
        (0x56, false) => 0x64,                      // International Backslash
        (0x5D, true) => 0x65,                       // Context Menu
        (0x59, false) => 0x67,                      // Keypad =; 0x86 is its alias
        (0x64, false) => 0x68,                      // F13
        (0x65, false) => 0x69,                      // F14
        (0x66, false) => 0x6A,                      // F15
        (0x67, false) => 0x6B,                      // F16
        (0x68, false) => 0x6C,                      // F17
        (0x69, false) => 0x6D,                      // F18
        (0x6A, false) => 0x6E,                      // F19
        (0x6B, false) => 0x6F,                      // F20
        (0x6C, false) => 0x70,                      // F21
        (0x6D, false) => 0x71,                      // F22
        (0x6E, false) => 0x72,                      // F23
        (0x76, false) => 0x73,                      // F24
        (0x20, true) => 0x7F,                       // Mute
        (0x30, true) => 0x80,                       // Volume Up
        (0x2E, true) => 0x81,                       // Volume Down
        (0x7E, false) => 0x85,                      // Keypad ,
        (0x73, false) => 0x87,                      // International1 (Ro)
        (0x70, false) => 0x88,                      // Kana Mode
        (0x7D, false) => 0x89,                      // International3 (Yen)
        (0x79, false) => 0x8A,                      // Convert
        (0x7B, false) => 0x8B,                      // Non-convert
        (0x72, false) => 0x90,                      // LANG1
        (0x71, false) => 0x91,                      // LANG2
        (0x1D, false) => 0xE0,                      // Left Ctrl
        (0x2A, false) => 0xE1,                      // Left Shift
        (0x38, false) => 0xE2,                      // Left Alt
        (0x5B, true) => 0xE3,                       // Left Meta
        (0x1D, true) => 0xE4,                       // Right Ctrl
        (0x36, false) => 0xE5,                      // Right Shift
        (0x38, true) => 0xE6,                       // Right Alt
        (0x5C, true) => 0xE7,                       // Right Meta
        (0x46, true) => 0x48,                       // Ctrl+Pause Break
        (0x54, false) => 0x46,                      // Alt+PrintScreen SysRq
        (0x2A, true) | (0x36, true) => return None, // Fake Shift around extended keys
        _ => return None,
    };
    Some(usage)
}

/// Windows 键盘消息 lParam 的拆解（WM_KEYDOWN / WM_KEYUP / WM_SYSKEYDOWN / WM_SYSKEYUP）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct WinKeyParam {
    pub scan: u16,
    pub extended: bool,
    pub repeat: bool,
    pub up: bool,
}

/// 位 16–23 扫描码；位 24 扩展；位 30 先前状态（down 时 = 自动重复）；位 31 转换状态（1 = 松开）。
pub fn win_decode_lparam(lparam: isize) -> WinKeyParam {
    let bits = lparam as u32;
    let up = (bits >> 31) & 1 == 1;
    WinKeyParam {
        scan: ((bits >> 16) & 0xFF) as u16,
        extended: (bits >> 24) & 1 == 1,
        repeat: !up && (bits >> 30) & 1 == 1,
        up,
    }
}
