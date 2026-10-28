use host_core::inject::keymap_mac::{hid_to_vk, modifier_usage_to_vk, mods_to_cg_flags, MacLayout};
use host_core::inject::keymap_win::{hid_to_win, modifier_bit};
use rs_desktop::keymap::{
    mac_flags_change, mac_vk_to_hid, win_decode_lparam, win_scan_to_hid, MacFlagsChange,
    MacKeyboardKind, WinKeyParam, MAC_MODIFIERS,
};

fn mac_alias(u: u16, c: u16) -> bool {
    matches!(
        (u, c),
        (0x31, 0x32)
            | (0x32, 0x31)
            | (0x49, 0x75)
            | (0x75, 0x49)
            | (0x53, 0xD8)
            | (0xD8, 0x53)
            | (0x67, 0x86)
            | (0x86, 0x67)
            | (0x88, 0x90)
            | (0x90, 0x88)
            | (0x8A, 0x90)
            | (0x90, 0x8A)
            | (0x8B, 0x91)
            | (0x91, 0x8B)
    )
}

fn win_alias(u: u16, c: u16) -> bool {
    matches!(
        (u, c),
        (0x31, 0x32) | (0x32, 0x31) | (0x67, 0x86) | (0x86, 0x67)
    )
}

#[test]
fn mac_roundtrip_ansi() {
    for u in 0..=u16::MAX {
        if let Some(vk) = hid_to_vk(u, MacLayout::Ansi) {
            let c = mac_vk_to_hid(vk, MacKeyboardKind::Ansi).unwrap_or_else(|| {
                panic!("missing reverse mapping for HID 0x{u:02X}, VK 0x{vk:02X}")
            });
            assert!(
                c == u || mac_alias(u, c),
                "HID 0x{u:02X} -> VK 0x{vk:02X} -> HID 0x{c:02X}"
            );
        }
    }
}

#[test]
fn mac_roundtrip_jis() {
    for u in 0..=u16::MAX {
        if let Some(vk) = hid_to_vk(u, MacLayout::Jis) {
            let c = mac_vk_to_hid(vk, MacKeyboardKind::Jis).unwrap_or_else(|| {
                panic!("missing reverse mapping for HID 0x{u:02X}, VK 0x{vk:02X}")
            });
            assert!(
                c == u || mac_alias(u, c),
                "HID 0x{u:02X} -> VK 0x{vk:02X} -> HID 0x{c:02X}"
            );
        }
    }
}

#[test]
fn mac_roundtrip_iso_except_swap() {
    for u in 0..=u16::MAX {
        if u == 0x35 || u == 0x64 {
            continue;
        }
        if let Some(vk) = hid_to_vk(u, MacLayout::Iso) {
            let c = mac_vk_to_hid(vk, MacKeyboardKind::Iso).unwrap_or_else(|| {
                panic!("missing reverse mapping for HID 0x{u:02X}, VK 0x{vk:02X}")
            });
            assert!(
                c == u || mac_alias(u, c),
                "HID 0x{u:02X} -> VK 0x{vk:02X} -> HID 0x{c:02X}"
            );
        }
    }
}

#[test]
fn mac_iso_swap() {
    assert_eq!(mac_vk_to_hid(0x0A, MacKeyboardKind::Iso), Some(0x35));
    assert_eq!(mac_vk_to_hid(0x32, MacKeyboardKind::Iso), Some(0x64));
    assert_eq!(mac_vk_to_hid(0x32, MacKeyboardKind::Ansi), Some(0x35));
    assert_eq!(mac_vk_to_hid(0x0A, MacKeyboardKind::Ansi), None);
}

#[test]
fn mac_aliases() {
    assert_eq!(mac_vk_to_hid(0x2A, MacKeyboardKind::Ansi), Some(0x31));
    assert_eq!(mac_vk_to_hid(0x72, MacKeyboardKind::Ansi), Some(0x49));
    assert_eq!(mac_vk_to_hid(0x47, MacKeyboardKind::Ansi), Some(0x53));
    assert_eq!(mac_vk_to_hid(0x51, MacKeyboardKind::Ansi), Some(0x67));
    assert_eq!(mac_vk_to_hid(0x68, MacKeyboardKind::Jis), Some(0x90));
    assert_eq!(mac_vk_to_hid(0x66, MacKeyboardKind::Jis), Some(0x91));
}

#[test]
fn mac_jis_only_keys() {
    assert_eq!(mac_vk_to_hid(0x5D, MacKeyboardKind::Jis), Some(0x89));
    assert_eq!(mac_vk_to_hid(0x5D, MacKeyboardKind::Ansi), None);
    assert_eq!(mac_vk_to_hid(0x5E, MacKeyboardKind::Jis), Some(0x87));
    assert_eq!(mac_vk_to_hid(0x5E, MacKeyboardKind::Ansi), None);
    assert_eq!(mac_vk_to_hid(0x5F, MacKeyboardKind::Jis), Some(0x85));
    assert_eq!(mac_vk_to_hid(0x5F, MacKeyboardKind::Ansi), None);
    assert_eq!(mac_vk_to_hid(0x68, MacKeyboardKind::Jis), Some(0x90));
    assert_eq!(mac_vk_to_hid(0x68, MacKeyboardKind::Ansi), None);
}

#[test]
fn mac_modifiers_table_matches_host() {
    for &(vk, usage, device_bit) in &MAC_MODIFIERS {
        assert_eq!(modifier_usage_to_vk(usage), Some(vk));
        let modifier = modifier_bit(usage).expect("MAC_MODIFIERS contains only HID modifiers");
        assert_eq!(mods_to_cg_flags(modifier) & 0xFFFF, device_bit);
    }
}

#[test]
fn mac_flags_change_cases() {
    assert_eq!(
        mac_flags_change(0x37, 0x0010_0008),
        MacFlagsChange::Modifier {
            usage: 0xE3,
            down: true,
        }
    );
    assert_eq!(
        mac_flags_change(0x37, 0x0010_0010),
        MacFlagsChange::Modifier {
            usage: 0xE3,
            down: false,
        }
    );
    assert_eq!(mac_flags_change(0x39, 0), MacFlagsChange::CapsLockPressed);
    assert_eq!(mac_flags_change(0x3F, 0x80_0000), MacFlagsChange::Ignored);
}

#[test]
fn win_roundtrip() {
    for u in 0..=u16::MAX {
        if let Some(key) = hid_to_win(u) {
            let c = win_scan_to_hid(key.scan, key.extended).unwrap_or_else(|| {
                panic!(
                    "missing reverse mapping for HID 0x{u:02X}, scan 0x{:02X}, extended {}",
                    key.scan, key.extended
                )
            });
            assert!(
                c == u || win_alias(u, c),
                "HID 0x{u:02X} -> scan 0x{:02X}/{} -> HID 0x{c:02X}",
                key.scan,
                key.extended
            );
        }
    }
    let pause = hid_to_win(0x48).expect("Pause is mapped by host-core");
    assert_eq!(pause.scan, 0x45);
    assert!(!pause.extended);
}

#[test]
fn win_special_cases() {
    assert_eq!(win_scan_to_hid(0x46, true), Some(0x48));
    assert_eq!(win_scan_to_hid(0x54, false), Some(0x46));
    assert_eq!(win_scan_to_hid(0x2A, true), None);
    assert_eq!(win_scan_to_hid(0x36, true), None);
    assert_eq!(win_scan_to_hid(0, false), None);
}

#[test]
fn win_lparam_decode() {
    assert_eq!(
        win_decode_lparam(0x001E_0001),
        WinKeyParam {
            scan: 0x1E,
            extended: false,
            repeat: false,
            up: false,
        }
    );
    assert_eq!(
        win_decode_lparam(0x401E_0001),
        WinKeyParam {
            scan: 0x1E,
            extended: false,
            repeat: true,
            up: false,
        }
    );
    assert_eq!(
        win_decode_lparam(0xC01E_0001u32 as i32 as isize),
        WinKeyParam {
            scan: 0x1E,
            extended: false,
            repeat: false,
            up: true,
        }
    );
    assert_eq!(
        win_decode_lparam(0x0148_0001),
        WinKeyParam {
            scan: 0x48,
            extended: true,
            repeat: false,
            up: false,
        }
    );
}

#[test]
fn unknown_keys_are_none() {
    assert_eq!(mac_vk_to_hid(0x7F, MacKeyboardKind::Ansi), None);
    assert_eq!(win_scan_to_hid(0x7F, false), None);
}
