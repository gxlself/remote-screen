use rs_desktop::{
    keymap::MacKeyboardKind,
    keys::{
        default_remap, remap_usage, ClientOs, HostOs, KeyAction, KeyRouter, KeyRouterConfig,
        ModifierRemap,
    },
    platform::KeyInput,
};
use rs_mobile::rscore::{combos::combo_inputs, dto_core::SessionCombo};
use rsp_core::hid::{InputMsg, Mods};

fn mac() -> KeyRouter {
    KeyRouter::new(KeyRouterConfig {
        client: ClientOs::Mac,
        host: HostOs::Mac,
        remap: ModifierRemap::None,
        mac_kind: MacKeyboardKind::Ansi,
    })
}

fn win() -> KeyRouter {
    KeyRouter::new(KeyRouterConfig {
        client: ClientOs::Windows,
        host: HostOs::Windows,
        remap: ModifierRemap::None,
        mac_kind: MacKeyboardKind::Ansi,
    })
}

fn key(usage: u16, down: bool, mods: Mods) -> KeyAction {
    KeyAction::Send(InputMsg::Key { usage, down, mods })
}

fn mac_key(vk: u16, down: bool) -> KeyInput {
    KeyInput::MacKey {
        vk,
        down,
        repeat: false,
    }
}

fn mac_flags(vk: u16, flags: u64) -> KeyInput {
    KeyInput::MacFlags { vk, flags }
}

fn win_key(scan: u16, extended: bool, down: bool) -> KeyInput {
    KeyInput::WinKey {
        scan,
        extended,
        vk: 0,
        down,
        repeat: false,
    }
}

#[test]
fn mac_letter() {
    let mut router = mac();
    assert_eq!(
        router.on_key(mac_key(0x00, true)),
        vec![key(0x04, true, Mods::NONE)]
    );
    assert_eq!(
        router.on_key(mac_key(0x00, false)),
        vec![key(0x04, false, Mods::NONE)]
    );
}

#[test]
fn mac_shift_letter() {
    let mut router = mac();
    assert_eq!(
        router.on_key(mac_flags(0x38, 0x0002_0002)),
        vec![key(0xE1, true, Mods::LSHIFT)]
    );
    assert_eq!(
        router.on_key(mac_key(0x00, true)),
        vec![key(0x04, true, Mods::LSHIFT)]
    );
    assert_eq!(
        router.on_key(mac_flags(0x38, 0x100)),
        vec![key(0xE1, false, Mods::NONE)]
    );
}

#[test]
fn cmd_to_ctrl_for_windows_host() {
    let mut router = KeyRouter::new(KeyRouterConfig {
        client: ClientOs::Mac,
        host: HostOs::Windows,
        remap: ModifierRemap::CmdToCtrl,
        mac_kind: MacKeyboardKind::Ansi,
    });
    assert_eq!(
        router.on_key(mac_flags(0x37, 0x0010_0008)),
        vec![key(0xE0, true, Mods::LCTRL)]
    );
    assert_eq!(
        router.on_key(mac_key(0x08, true)),
        vec![key(0x06, true, Mods::LCTRL)]
    );
}

#[test]
fn swap_ctrl_meta() {
    let mut router = KeyRouter::new(KeyRouterConfig {
        client: ClientOs::Windows,
        host: HostOs::Mac,
        remap: ModifierRemap::SwapCtrlMeta,
        mac_kind: MacKeyboardKind::Ansi,
    });
    assert_eq!(
        router.on_key(win_key(0x1D, false, true)),
        vec![key(0xE3, true, Mods::LMETA)]
    );
}

#[test]
fn autorepeat_forwarded() {
    let mut router = mac();
    assert_eq!(
        router.on_key(mac_key(0x00, true)),
        vec![key(0x04, true, Mods::NONE)]
    );
    assert_eq!(
        router.on_key(KeyInput::MacKey {
            vk: 0x00,
            down: true,
            repeat: true,
        }),
        vec![key(0x04, true, Mods::NONE)]
    );
}

#[test]
fn modifier_repeat_dropped() {
    let mut router = mac();
    assert_eq!(
        router.on_key(mac_flags(0x38, 0x0002_0002)),
        vec![key(0xE1, true, Mods::LSHIFT)]
    );
    assert_eq!(
        router.on_key(mac_flags(0x38, 0x0002_0002)),
        Vec::<KeyAction>::new()
    );
}

#[test]
fn stray_up_dropped() {
    let mut router = mac();
    assert!(router.on_key(mac_key(0x00, false)).is_empty());
}

#[test]
fn mac_cmd_release_flushes_keys() {
    let mut router = mac();
    assert_eq!(
        router.on_key(mac_flags(0x37, 0x0010_0008)),
        vec![key(0xE3, true, Mods::LMETA)]
    );
    assert_eq!(
        router.on_key(mac_key(0x08, true)),
        vec![key(0x06, true, Mods::LMETA)]
    );
    assert_eq!(
        router.on_key(mac_flags(0x37, 0x100)),
        vec![key(0x06, false, Mods::LMETA), key(0xE3, false, Mods::NONE)]
    );
    assert!(router.on_key(mac_key(0x08, false)).is_empty());
}

#[test]
fn mac_cmd_release_waits_for_other_cmd() {
    let mut router = mac();
    router.on_key(mac_flags(0x37, 0x0010_0008));
    router.on_key(mac_flags(0x36, 0x0010));
    router.on_key(mac_key(0x08, true));
    assert_eq!(
        router.on_key(mac_flags(0x37, 0x100)),
        vec![key(0xE3, false, Mods::RMETA)]
    );
    assert_eq!(
        router.on_key(mac_flags(0x36, 0x100)),
        vec![key(0x06, false, Mods::RMETA), key(0xE7, false, Mods::NONE)]
    );
}

#[test]
fn caps_lock_pair() {
    let mut router = mac();
    assert_eq!(
        router.on_key(mac_flags(0x39, 0x1_0000)),
        vec![key(0x39, true, Mods::NONE), key(0x39, false, Mods::NONE)]
    );
}

#[test]
fn chord_releases_capture() {
    let mut router = win();
    assert_eq!(
        router.on_key(win_key(0x1D, false, true)),
        vec![key(0xE0, true, Mods::LCTRL)]
    );
    assert_eq!(
        router.on_key(win_key(0x38, false, true)),
        vec![key(0xE2, true, Mods::LCTRL | Mods::LALT)]
    );
    assert_eq!(
        router.on_key(win_key(0x38, false, false)),
        vec![key(0xE2, false, Mods::LCTRL)]
    );
    assert_eq!(
        router.on_key(win_key(0x1D, false, false)),
        vec![
            key(0xE0, false, Mods::NONE),
            KeyAction::ReleaseCapture,
            KeyAction::Send(InputMsg::ReleaseAll)
        ]
    );
    assert!(!router.captured());
    assert!(router.on_key(win_key(0x1E, false, true)).is_empty());
}

#[test]
fn chord_resumes_capture() {
    let mut router = win();
    for event in [
        win_key(0x1D, false, true),
        win_key(0x38, false, true),
        win_key(0x38, false, false),
        win_key(0x1D, false, false),
    ] {
        router.on_key(event);
    }
    assert!(!router.captured());
    assert_eq!(
        router.on_key(win_key(0x1D, false, true)),
        Vec::<KeyAction>::new()
    );
    assert_eq!(
        router.on_key(win_key(0x38, false, true)),
        Vec::<KeyAction>::new()
    );
    assert_eq!(
        router.on_key(win_key(0x38, false, false)),
        Vec::<KeyAction>::new()
    );
    assert_eq!(
        router.on_key(win_key(0x1D, false, false)),
        vec![KeyAction::ResumeCapture]
    );
    assert!(router.captured());
}

#[test]
fn chord_spoiled() {
    let mut router = win();
    for event in [
        win_key(0x1D, false, true),
        win_key(0x38, false, true),
        win_key(0x53, true, true),
        win_key(0x53, true, false),
        win_key(0x38, false, false),
        win_key(0x1D, false, false),
    ] {
        router.on_key(event);
    }
    assert!(router.captured());
}

#[test]
fn chord_needs_left_keys() {
    let mut router = win();
    for event in [
        win_key(0x1D, true, true),
        win_key(0x38, true, true),
        win_key(0x38, true, false),
        win_key(0x1D, true, false),
    ] {
        router.on_key(event);
    }
    assert!(router.captured());
}

#[test]
fn chord_needs_empty_start() {
    let mut router = win();
    for event in [
        win_key(0x1E, false, true),
        win_key(0x1D, false, true),
        win_key(0x38, false, true),
        win_key(0x38, false, false),
        win_key(0x1D, false, false),
        win_key(0x1E, false, false),
    ] {
        router.on_key(event);
    }
    assert!(router.captured());
}

#[test]
fn set_captured_false_releases() {
    let mut router = mac();
    router.on_key(mac_key(0x00, true));
    assert_eq!(
        router.set_captured(false),
        vec![KeyAction::Send(InputMsg::ReleaseAll)]
    );
    assert!(!router.captured());
    assert!(router.set_captured(false).is_empty());
    assert!(router.set_captured(true).is_empty());
    assert!(router.captured());
    assert!(router.on_key(mac_key(0x00, false)).is_empty());
}

#[test]
fn release_all_only_when_held() {
    let mut router = mac();
    assert!(router.release_all().is_empty());
    router.on_key(mac_flags(0x38, 0x0002_0002));
    assert_eq!(
        router.release_all(),
        vec![KeyAction::Send(InputMsg::ReleaseAll)]
    );
    assert!(router.release_all().is_empty());
}

#[test]
fn set_host_releases_held() {
    let mut router = mac();
    router.on_key(mac_key(0x00, true));
    assert_eq!(
        router.set_host(HostOs::Windows, ModifierRemap::CmdToCtrl),
        vec![KeyAction::Send(InputMsg::ReleaseAll)]
    );
    assert_eq!(router.config().host, HostOs::Windows);
    assert_eq!(router.config().remap, ModifierRemap::CmdToCtrl);
    assert!(router.on_key(mac_key(0x00, false)).is_empty());
    assert_eq!(
        router.on_key(mac_flags(0x37, 0x0010_0008)),
        vec![key(0xE0, true, Mods::LCTRL)]
    );
}

#[test]
fn win_fake_shift_and_pause() {
    let mut router = win();
    assert!(router
        .on_key(KeyInput::WinKey {
            scan: 0x2A,
            extended: true,
            vk: 0x10,
            down: true,
            repeat: false,
        })
        .is_empty());
    assert_eq!(
        router.on_key(KeyInput::WinKey {
            scan: 0,
            extended: false,
            vk: 0x13,
            down: true,
            repeat: false,
        }),
        vec![key(0x48, true, Mods::NONE)]
    );
}

#[test]
fn pressed_limit_32() {
    let mut router = win();
    let scans = [
        0x1E, 0x30, 0x2E, 0x20, 0x12, 0x21, 0x22, 0x23, 0x17, 0x24, 0x25, 0x26, 0x32, 0x31, 0x18,
        0x19, 0x10, 0x13, 0x1F, 0x14, 0x16, 0x2F, 0x11, 0x2D, 0x15, 0x2C, 0x02, 0x03, 0x04, 0x05,
        0x06, 0x07, 0x08,
    ];
    let mut sent = 0;
    for scan in scans {
        if !router.on_key(win_key(scan, false, true)).is_empty() {
            sent += 1;
        }
    }
    assert_eq!(sent, 32);
    assert!(router.on_key(win_key(0x08, false, false)).is_empty());
    // Held keys still repeat at capacity; modifiers do not consume a slot.
    assert_eq!(
        router.on_key(win_key(0x1E, false, true)),
        vec![key(0x04, true, Mods::NONE)]
    );
    assert_eq!(
        router.on_key(win_key(0x2A, false, true)),
        vec![key(0xE1, true, Mods::LSHIFT)]
    );
    assert_eq!(
        router.on_key(win_key(0x1E, false, false)),
        vec![key(0x04, false, Mods::LSHIFT)]
    );
    assert_eq!(
        router.on_key(win_key(0x08, false, true)),
        vec![key(0x24, true, Mods::LSHIFT)]
    );
    assert_eq!(
        router.on_key(win_key(0x08, false, false)),
        vec![key(0x24, false, Mods::LSHIFT)]
    );
}

#[test]
fn combo_lookup() {
    for (name, combo) in [
        ("cmdTab", SessionCombo::CmdTab),
        ("cmdSpace", SessionCombo::CmdSpace),
        ("missionControl", SessionCombo::MissionControl),
        ("altTab", SessionCombo::AltTab),
        ("winKey", SessionCombo::WinKey),
    ] {
        assert_eq!(KeyRouter::combo(name), Some(combo_inputs(combo)));
    }
    for unknown in ["bogus", "CmdTab", "\"cmdTab\"", " cmdTab", ""] {
        assert_eq!(KeyRouter::combo(unknown), None);
    }
}

#[test]
fn default_remap_table() {
    assert_eq!(
        default_remap(ClientOs::Mac, HostOs::Windows),
        ModifierRemap::CmdToCtrl
    );
    for (client, host) in [
        (ClientOs::Mac, HostOs::Mac),
        (ClientOs::Windows, HostOs::Mac),
        (ClientOs::Windows, HostOs::Windows),
        (ClientOs::Mac, HostOs::Unknown),
        (ClientOs::Windows, HostOs::Unknown),
    ] {
        assert_eq!(default_remap(client, host), ModifierRemap::None);
    }
    assert_eq!(remap_usage(0xE3, ModifierRemap::CmdToCtrl), 0xE0);
    assert_eq!(remap_usage(0xE0, ModifierRemap::SwapCtrlMeta), 0xE3);
}

#[test]
fn host_os_from_hub() {
    assert_eq!(HostOs::from_os_name("macos"), HostOs::Mac);
    assert_eq!(HostOs::from_os_name("windows"), HostOs::Windows);
    assert_eq!(HostOs::from_os_name("linux"), HostOs::Unknown);
}
