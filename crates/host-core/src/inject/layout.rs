//! 键盘布局判定（ANSI / ISO / JIS）与键位差异表。
//!
//! [`resolve_vk`] 在 [`hid_to_vk`] 的规范映射之上复现 Apple
//! IOHIDKeyboard 对 ISO 键盘的物理位置处理。IOHIDKeyboard.cpp 的
//! `handlerID()` 在 ISO 设备上交换 0x35/0x64 的 USB-to-ADB 表项，使 0x35
//! 对应 `kVK_ISO_Section` (0x0A)、0x64 对应 `kVK_ANSI_Grave` (0x32)。
//! 这里只在最后对虚拟键码交换一次：0x32 <-> 0x0A。[`hid_to_vk`] 自身仍
//! 保持规范值（0x35 -> 0x32；ISO 上 0x64 -> 0x0A），投递层不得再换第二次。
//! 依据：<https://github.com/apple-oss-distributions/IOHIDFamily/blob/main/IOHIDFamily/IOHIDKeyboard.cpp>。
//!
//! 本模块是纯函数，不读取系统状态，也不在 Linux/macOS 间分叉。

use crate::inject::keymap_mac::{hid_to_vk, MacLayout};

/// Carbon `kKeyboardANSI`（四字符码 `'ANSI'`）。
pub const KB_LAYOUT_ANSI: u32 = u32::from_be_bytes(*b"ANSI");
/// Carbon `kKeyboardISO`（四字符码 `'ISO '`）。
pub const KB_LAYOUT_ISO: u32 = u32::from_be_bytes(*b"ISO ");
/// Carbon `kKeyboardJIS`（四字符码 `'JIS '`）。
pub const KB_LAYOUT_JIS: u32 = u32::from_be_bytes(*b"JIS ");

/// 无物理键盘时使用的 ANSI 键盘类型。
///
/// 文档未规定此值；40（0x28）是暂定默认值，需要用
/// `LMGetKbdType` 在真机上核对。
pub const DEFAULT_KBD_TYPE_ANSI: u32 = 40;
/// 无物理键盘时使用的 ISO 键盘类型；暂定 41（0x29），待真机核对。
pub const DEFAULT_KBD_TYPE_ISO: u32 = 41;
/// 无物理键盘时使用的 JIS 键盘类型；暂定 42（0x2A），待真机核对。
pub const DEFAULT_KBD_TYPE_JIS: u32 = 42;

const VK_ISO_SECTION: u16 = 0x0A; // kVK_ISO_Section
const VK_ANSI_GRAVE: u16 = 0x32; // kVK_ANSI_Grave
const VK_JIS_EISU: u16 = 0x66; // kVK_JIS_Eisu
const VK_JIS_KANA: u16 = 0x68; // kVK_JIS_Kana

/// 当前 Mac 的物理键盘档案。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct KeyboardProfile {
    pub layout: MacLayout,
    /// 注入事件的 `kCGKeyboardEventKeyboardType`。
    pub keyboard_type: u32,
    /// 是否应用 Apple ISO 键盘驱动的 0x32/0x0A 位置互换。
    pub iso_position_swap: bool,
}

impl KeyboardProfile {
    /// 用布局的暂定默认键盘类型建立档案；ISO 默认开启位置互换。
    pub fn new(layout: MacLayout) -> Self {
        Self {
            layout,
            keyboard_type: default_keyboard_type(layout),
            iso_position_swap: layout == MacLayout::Iso,
        }
    }

    /// 从 `KBGetLayoutType(LMGetKbdType())` 与 `LMGetKbdType()` 的结果建立档案。
    ///
    /// `layout_type` 缺失或不是三个已知四字符码时按 ANSI 处理；`keyboard_type`
    /// 缺失或为 0 时使用该布局的默认值。非零的键盘类型原样保留。
    pub fn detect(layout_type: Option<u32>, keyboard_type: Option<u32>) -> Self {
        let layout = layout_type
            .and_then(layout_from_kb_layout_type)
            .unwrap_or(MacLayout::Ansi);
        let keyboard_type = match keyboard_type {
            Some(keyboard_type) if keyboard_type != 0 => keyboard_type,
            _ => default_keyboard_type(layout),
        };
        Self {
            layout,
            keyboard_type,
            iso_position_swap: layout == MacLayout::Iso,
        }
    }
}

impl Default for KeyboardProfile {
    fn default() -> Self {
        Self::new(MacLayout::Ansi)
    }
}

fn default_keyboard_type(layout: MacLayout) -> u32 {
    match layout {
        MacLayout::Ansi => DEFAULT_KBD_TYPE_ANSI,
        MacLayout::Iso => DEFAULT_KBD_TYPE_ISO,
        MacLayout::Jis => DEFAULT_KBD_TYPE_JIS,
    }
}

/// 把 Carbon `KBGetLayoutType` 的四字符码转换成 [`MacLayout`]。
pub fn layout_from_kb_layout_type(layout_type: u32) -> Option<MacLayout> {
    match layout_type {
        KB_LAYOUT_ANSI => Some(MacLayout::Ansi),
        KB_LAYOUT_ISO => Some(MacLayout::Iso),
        KB_LAYOUT_JIS => Some(MacLayout::Jis),
        _ => None,
    }
}

/// 在标准映射之上按 Mac 物理键盘档案解析虚拟键码。
///
/// 0x90/0x91（LANG1/LANG2）在非 JIS 键盘上也映射到 Kana/Eisu，以兼容手机
/// 外接日文/韩文键盘在 Mac 日文输入模式下切换输入模式的行为。该宽松映射是
/// 暂定约定，待真机核对；其余 JIS 专有键在非 JIS 上仍为
/// `None`。ISO 档案开启位置互换时，最后把 0x32 与 0x0A 互换一次。
pub fn resolve_vk(usage: u16, profile: &KeyboardProfile) -> Option<u16> {
    let mut vk = match usage {
        0x90 if profile.layout != MacLayout::Jis => Some(VK_JIS_KANA),
        0x91 if profile.layout != MacLayout::Jis => Some(VK_JIS_EISU),
        _ => hid_to_vk(usage, profile.layout),
    };
    if profile.layout == MacLayout::Iso && profile.iso_position_swap {
        vk = vk.map(swap_iso_position);
    }
    vk
}

fn swap_iso_position(vk: u16) -> u16 {
    match vk {
        VK_ANSI_GRAVE => VK_ISO_SECTION,
        VK_ISO_SECTION => VK_ANSI_GRAVE,
        other => other,
    }
}

/// 一个 HID 用法码在三种物理键盘布局下的解析结果。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct LayoutDiff {
    pub usage: u16,
    pub key: &'static str,
    pub ansi: Option<u16>,
    pub iso: Option<u16>,
    pub jis: Option<u16>,
}

/// 三种默认键盘档案的完整布局差异表，按 HID 用法码升序排列。
///
/// 表中 ISO 列是开启位置互换后的投递结果；JIS 的 Kana/Eisu 别名在三种布局
/// 下经宽松映射后相同，因此 LANG1/LANG2 不在表内。
pub const LAYOUT_DIFFS: &[LayoutDiff] = &[
    LayoutDiff {
        usage: 0x35,
        key: "Grave / §（1 左边）",
        ansi: Some(VK_ANSI_GRAVE),
        iso: Some(VK_ISO_SECTION),
        jis: Some(VK_ANSI_GRAVE),
    },
    LayoutDiff {
        usage: 0x64,
        key: "Non-US \\（左 Shift 右边）",
        ansi: None,
        iso: Some(VK_ANSI_GRAVE),
        jis: None,
    },
    LayoutDiff {
        usage: 0x85,
        key: "Keypad ,（JIS）",
        ansi: None,
        iso: None,
        jis: Some(0x5F), // kVK_JIS_KeypadComma
    },
    LayoutDiff {
        usage: 0x87,
        key: "International1 ろ / _",
        ansi: None,
        iso: None,
        jis: Some(0x5E), // kVK_JIS_Underscore
    },
    LayoutDiff {
        usage: 0x88,
        key: "International2 かな",
        ansi: None,
        iso: None,
        jis: Some(VK_JIS_KANA),
    },
    LayoutDiff {
        usage: 0x89,
        key: "International3 ¥",
        ansi: None,
        iso: None,
        jis: Some(0x5D), // kVK_JIS_Yen
    },
    LayoutDiff {
        usage: 0x8A,
        key: "International4 変換",
        ansi: None,
        iso: None,
        jis: Some(VK_JIS_KANA),
    },
    LayoutDiff {
        usage: 0x8B,
        key: "International5 無変換",
        ansi: None,
        iso: None,
        jis: Some(VK_JIS_EISU),
    },
];

fn layout_diff_key(usage: u16) -> &'static str {
    match usage {
        0x35 => "Grave / §（1 左边）",
        0x64 => "Non-US \\（左 Shift 右边）",
        0x85 => "Keypad ,（JIS）",
        0x87 => "International1 ろ / _",
        0x88 => "International2 かな",
        0x89 => "International3 ¥",
        0x8A => "International4 変換",
        0x8B => "International5 無変換",
        _ => "未登记布局差异",
    }
}

/// 遍历 0x04..=0xE7，返回三种档案解析结果不全相同的用法码。
///
/// 顺序与 [`LAYOUT_DIFFS`] 一样按 usage 升序；测试用两者完全相等来保证常量表
/// 没有遗漏或过时。
pub fn layout_diffs_for(
    ansi: &KeyboardProfile,
    iso: &KeyboardProfile,
    jis: &KeyboardProfile,
) -> Vec<LayoutDiff> {
    let mut diffs = Vec::new();
    for usage in 0x04..=0xE7 {
        let ansi_vk = resolve_vk(usage, ansi);
        let iso_vk = resolve_vk(usage, iso);
        let jis_vk = resolve_vk(usage, jis);
        if ansi_vk != iso_vk || iso_vk != jis_vk {
            diffs.push(LayoutDiff {
                usage,
                key: layout_diff_key(usage),
                ansi: ansi_vk,
                iso: iso_vk,
                jis: jis_vk,
            });
        }
    }
    diffs
}
