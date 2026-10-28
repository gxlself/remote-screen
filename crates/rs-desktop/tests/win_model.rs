use rs_desktop::{
    platform::{CursorImage, KeyInput, MenuCommand, PlatformError, ScrollInput, SessionMenu},
    pointer::Viewport,
    win_model::{
        button_from_message, combo_text, cursor_bitmap, is_altgr_fake_ctrl, key_from_ll_hook,
        key_from_message, ll_is_fake_ctrl, lparam_point, premultiplied, remote_cursor_origin,
        session_sysmenu, sysmenu_command, to_logical, wheel_from_message, CursorBitmap, KeyFilter,
        WinMenuItem, ID_COMBO_BASE, ID_CTRL_ALT_DEL, ID_DISCONNECT, ID_DISPLAY_BASE, ID_FULLSCREEN,
        ID_KEYBOARD, ID_POINTER_LOCK, ID_QUALITY_BASE, ID_REQUEST_CONTROL, LLKHF_EXTENDED,
        LLKHF_INJECTED, LLKHF_UP, QUALITY_ORDER, VK_PACKET, VK_SNAPSHOT, WM_KEYDOWN, WM_KEYUP,
        WM_SYSKEYDOWN, WM_SYSKEYUP,
    },
};

fn key(scan: u16, extended: bool, vk: u16, down: bool, repeat: bool) -> KeyInput {
    KeyInput::WinKey {
        scan,
        extended,
        vk,
        down,
        repeat,
    }
}

#[test]
fn key_message_basic() {
    assert_eq!(
        key_from_message(WM_KEYDOWN, 0x41, 0x001E_0001),
        Some(key(0x1E, false, 0x41, true, false))
    );
    assert_eq!(
        key_from_message(WM_KEYDOWN, 0x41, 0x401E_0001),
        Some(key(0x1E, false, 0x41, true, true))
    );
    assert_eq!(
        key_from_message(WM_SYSKEYUP, 0x12, 0xC038_0001u32 as i32 as isize),
        Some(key(0x38, false, 0x12, false, false))
    );
    assert_eq!(
        key_from_message(WM_KEYDOWN, 0x11, 0x011D_0001),
        Some(key(0x1D, true, 0x11, true, false))
    );
    assert_eq!(
        key_from_message(WM_SYSKEYDOWN, 0x12, 0x0138_0001),
        Some(key(0x38, true, 0x12, true, false))
    );
    assert_eq!(
        key_from_message(WM_KEYUP, 0x41, 0xC01E_0001u32 as i32 as isize),
        Some(key(0x1E, false, 0x41, false, false))
    );
    assert_eq!(
        key_from_message(WM_KEYDOWN, VK_PACKET.into(), 0x001E_0001),
        None
    );
    assert_eq!(key_from_message(0x0102, 0x41, 0x001E_0001), None); // WM_CHAR
    assert_eq!(key_from_message(WM_KEYDOWN, 0x41, 1), None);
    assert_eq!(key_from_message(WM_KEYDOWN, 0x1_0041, 0x001E_0001), None);
}

#[test]
fn ll_hook_basic() {
    assert_eq!(
        key_from_ll_hook(WM_KEYDOWN, 0x5B, 0x5B, LLKHF_EXTENDED),
        Some(key(0x5B, true, 0x5B, true, false))
    );
    assert_eq!(
        key_from_ll_hook(WM_KEYDOWN, 0x5B, 0x5B, LLKHF_EXTENDED | LLKHF_UP),
        Some(key(0x5B, true, 0x5B, false, false))
    );
    assert_eq!(
        key_from_ll_hook(WM_KEYUP, 0x5B, 0x5B, LLKHF_EXTENDED),
        Some(key(0x5B, true, 0x5B, false, false))
    );
    assert_eq!(
        key_from_ll_hook(WM_SYSKEYDOWN, 0x12, 0x38, LLKHF_EXTENDED),
        Some(key(0x38, true, 0x12, true, false))
    );
    assert_eq!(
        key_from_ll_hook(WM_SYSKEYUP, 0x12, 0x38, LLKHF_EXTENDED | LLKHF_UP),
        Some(key(0x38, true, 0x12, false, false))
    );
    assert_eq!(
        key_from_ll_hook(WM_KEYDOWN, 0x5B, 0x5B, LLKHF_EXTENDED | LLKHF_INJECTED),
        None
    );
    assert_eq!(
        key_from_ll_hook(WM_KEYDOWN, 0x11, 0x21D, 0),
        Some(key(0x1D, false, 0x11, true, false))
    );
    assert_eq!(
        key_from_ll_hook(WM_KEYDOWN, 0x41, 0x11E, 0),
        Some(key(0x1E, false, 0x41, true, false))
    );
    assert!(ll_is_fake_ctrl(0x21D));
    assert!(!ll_is_fake_ctrl(0x1D));
    assert_eq!(
        key_from_ll_hook(WM_KEYDOWN, VK_PACKET.into(), 0x1E, 0),
        None
    );
    assert_eq!(key_from_ll_hook(WM_KEYDOWN, 0x41, 0x200, 0), None);
    assert_eq!(key_from_ll_hook(0x0102, 0x41, 0x1E, 0), None);
    assert_eq!(key_from_ll_hook(WM_KEYDOWN, 0x1_0041, 0x1E, 0), None);
}

#[test]
fn altgr_fake_ctrl_detection() {
    let ctrl = key(0x1D, false, 0x11, true, false);
    for msg in [WM_KEYDOWN, WM_SYSKEYDOWN] {
        assert!(is_altgr_fake_ctrl(
            &ctrl,
            1000,
            Some((msg, 0x12, 0x0138_0001, 1000))
        ));
    }
    for next in [
        Some((WM_KEYDOWN, 0x12, 0x0138_0001, 1001)),
        Some((WM_KEYDOWN, 0x12, 0x0038_0001, 1000)),
        Some((WM_KEYUP, 0x12, 0x0138_0001, 1000)),
        Some((WM_KEYDOWN, 0x41, 0x0138_0001, 1000)),
        None,
    ] {
        assert!(!is_altgr_fake_ctrl(&ctrl, 1000, next));
    }
    for cur in [
        key(0x1D, true, 0x11, true, false),
        key(0x1D, false, 0x11, false, false),
        key(0x1E, false, 0x41, true, false),
        KeyInput::MacFlags { vk: 0x3B, flags: 1 },
    ] {
        assert!(!is_altgr_fake_ctrl(
            &cur,
            1000,
            Some((WM_KEYDOWN, 0x12, 0x0138_0001, 1000))
        ));
    }
}

#[test]
fn filter_drops_fake_ctrl_pair() {
    let mut filter = KeyFilter::new();
    let ctrl_down = key(0x1D, false, 0x11, true, false);
    let ctrl_up = key(0x1D, false, 0x11, false, false);
    let alt_down = key(0x38, true, 0x12, true, false);
    let alt_up = key(0x38, true, 0x12, false, false);
    assert!(filter.on_key(ctrl_down, true).is_empty());
    assert_eq!(filter.on_key(alt_down, false), vec![alt_down]);
    assert_eq!(filter.on_key(alt_up, false), vec![alt_up]);
    assert!(filter.on_key(ctrl_up, false).is_empty());
    assert_eq!(filter.on_key(ctrl_down, false), vec![ctrl_down]);
    assert_eq!(filter.on_key(ctrl_up, false), vec![ctrl_up]);

    // 钩子路径在合成松开上也带 0x200 标记，仍按相同规则丢弃。
    assert!(filter.on_key(ctrl_down, true).is_empty());
    assert!(filter.on_key(ctrl_up, true).is_empty());
    assert_eq!(filter.on_key(ctrl_down, false), vec![ctrl_down]);
    assert_eq!(filter.on_key(ctrl_up, false), vec![ctrl_up]);
}

#[test]
fn filter_preserves_real_ctrl_held_during_altgr() {
    let mut filter = KeyFilter::new();
    let down = key(0x1D, false, 0x11, true, false);
    let up = key(0x1D, false, 0x11, false, false);
    assert_eq!(filter.on_key(down, false), vec![down]);
    assert!(filter.on_key(down, true).is_empty());
    assert_eq!(filter.on_key(up, false), vec![up]);
    assert!(filter.on_key(up, true).is_empty());
}

#[test]
fn filter_print_screen() {
    let mut filter = KeyFilter::new();
    let up = key(0x37, true, VK_SNAPSHOT, false, false);
    let down = key(0x37, true, VK_SNAPSHOT, true, false);
    assert_eq!(filter.on_key(up, false), vec![down, up]);
    assert_eq!(filter.on_key(down, false), vec![down]);
    assert_eq!(filter.on_key(up, false), vec![up]);
    filter.on_key(key(0x1E, false, 0x41, true, false), false);
    filter.clear();
    assert!(filter
        .on_key(key(0x1E, false, 0x41, false, false), false)
        .is_empty());
    assert!(filter
        .on_key(key(0x37, false, 0x6A, false, false), false)
        .is_empty());
}

#[test]
fn filter_infers_repeat() {
    let mut filter = KeyFilter::default();
    let down = key_from_ll_hook(WM_KEYDOWN, 0x41, 0x1E, 0).unwrap();
    let up = key_from_ll_hook(WM_KEYUP, 0x41, 0x1E, LLKHF_UP).unwrap();
    assert_eq!(filter.on_key(down, false), vec![down]);
    assert_eq!(
        filter.on_key(down, false),
        vec![key(0x1E, false, 0x41, true, true)]
    );
    assert_eq!(filter.on_key(up, false), vec![up]);
    assert_eq!(filter.on_key(down, false), vec![down]);
    filter.clear();
    let repeat = key(0x1E, false, 0x41, true, true);
    assert_eq!(filter.on_key(repeat, false), vec![repeat]);
}

#[test]
fn filter_tracks_extended_keys_separately_and_passes_mac() {
    let mut filter = KeyFilter::new();
    for ev in [
        key(0x1D, false, 0x11, true, false),
        key(0x1D, true, 0x11, true, false),
        key(0x1D, false, 0x11, false, false),
        key(0x1D, true, 0x11, false, false),
        KeyInput::MacKey {
            vk: 0x00,
            down: true,
            repeat: false,
        },
        KeyInput::MacFlags { vk: 0x3B, flags: 1 },
    ] {
        assert_eq!(filter.on_key(ev, false), vec![ev]);
    }
    let mac = KeyInput::MacFlags { vk: 0x3B, flags: 0 };
    assert_eq!(filter.on_key(mac, true), vec![mac]);
}

#[test]
fn buttons_and_wheel() {
    for (button, down_msg, up_msg, double_msg) in [
        (0, 0x0201, 0x0202, 0x0203),
        (1, 0x0204, 0x0205, 0x0206),
        (2, 0x0207, 0x0208, 0x0209),
    ] {
        assert_eq!(button_from_message(down_msg, 0), Some((button, true)));
        assert_eq!(button_from_message(up_msg, 0), Some((button, false)));
        assert_eq!(button_from_message(double_msg, 0), Some((button, true)));
    }
    for (word, button) in [(1, 3), (2, 4)] {
        assert_eq!(
            button_from_message(0x020B, word << 16),
            Some((button, true))
        );
        assert_eq!(
            button_from_message(0x020C, word << 16),
            Some((button, false))
        );
        assert_eq!(
            button_from_message(0x020D, word << 16),
            Some((button, true))
        );
    }
    assert_eq!(button_from_message(0x020B, 3 << 16), None);
    assert_eq!(button_from_message(0x020B, 0), None);
    assert_eq!(button_from_message(0x0200, 0), None);
    for (word, delta) in [(0x0078_0000, 120), (0xFF88_0000, -120)] {
        for (msg, horizontal) in [(0x020A, false), (0x020E, true)] {
            assert_eq!(
                wheel_from_message(msg, word),
                Some(ScrollInput::WinWheel { delta, horizontal })
            );
        }
    }
    assert_eq!(wheel_from_message(0x0200, 0x0078_0000), None);
}

#[test]
fn points_and_logical() {
    assert_eq!(lparam_point(0x0007_FFFB), (-5, 7));
    assert_eq!(lparam_point(0xFFFA_0007u32 as i32 as isize), (7, -6));
    assert_eq!(
        lparam_point(0x8000_7FFFu32 as i32 as isize),
        (32767, -32768)
    );
    assert_eq!(to_logical(300, 150, 144), (200.0, 100.0));
    assert_eq!(to_logical(-5, 7, 0), (-5.0, 7.0));
    assert_eq!(to_logical(-300, -150, 192), (-150.0, -75.0));
}

#[test]
fn remote_cursor_origin_center() {
    let vp = Viewport {
        view_w: 800.0,
        view_h: 600.0,
        video_w: 1920,
        video_h: 1080,
    };
    assert_eq!(remote_cursor_origin(&vp, 0.5, 0.5, 4, 4), (396, 296));
    assert_eq!(remote_cursor_origin(&vp, 0.0, 0.0, 4, 4), (-4, 71));
    assert_eq!(remote_cursor_origin(&vp, 1.0, 1.0, 4, 4), (796, 521));
    let portrait = Viewport {
        view_w: 600.0,
        view_h: 800.0,
        video_w: 1080,
        video_h: 1920,
    };
    assert_eq!(remote_cursor_origin(&portrait, 0.0, 0.0, 4, 4), (71, -4));
}

fn menu() -> SessionMenu {
    SessionMenu {
        displays: vec![(7, "主显示器".into()), (42, "外接显示器".into())],
        current_display: Some(42),
        quality: "smooth".into(),
        viewer: false,
        fullscreen: true,
        keyboard_captured: true,
        pointer_locked: false,
        combos: vec!["altTab".into(), "winKey".into()],
        ctrl_alt_del: true,
    }
}

fn command(id: u32, text: &str, checked: bool, radio: bool, enabled: bool) -> WinMenuItem {
    WinMenuItem::Command {
        id,
        text: text.into(),
        checked,
        radio,
        enabled,
    }
}

fn submenu(text: &str, items: Vec<WinMenuItem>, enabled: bool) -> WinMenuItem {
    WinMenuItem::Submenu {
        text: text.into(),
        items,
        enabled,
    }
}

#[test]
fn sysmenu_layout() {
    let mut menu = menu();
    assert_eq!(
        session_sysmenu(&menu),
        vec![
            WinMenuItem::Separator,
            submenu(
                "发送组合键",
                vec![
                    command(ID_COMBO_BASE, "Alt+Tab", false, false, true),
                    command(ID_COMBO_BASE + 16, "Win 键", false, false, true),
                ],
                true,
            ),
            command(ID_CTRL_ALT_DEL, "Ctrl+Alt+Del", false, false, true),
            WinMenuItem::Separator,
            command(ID_FULLSCREEN, "全屏", true, false, true),
            command(ID_KEYBOARD, "捕获键盘", true, false, true),
            command(ID_POINTER_LOCK, "锁定鼠标", false, false, true),
            submenu(
                "画质",
                vec![
                    command(ID_QUALITY_BASE, "自动", false, true, true),
                    command(ID_QUALITY_BASE + 16, "流畅", true, true, true),
                    command(ID_QUALITY_BASE + 32, "清晰", false, true, true),
                    command(ID_QUALITY_BASE + 48, "省流量", false, true, true),
                ],
                true,
            ),
            submenu(
                "显示器",
                vec![
                    command(ID_DISPLAY_BASE, "主显示器", false, true, true),
                    command(ID_DISPLAY_BASE + 16, "外接显示器", true, true, true),
                ],
                true,
            ),
            WinMenuItem::Separator,
            command(ID_DISCONNECT, "断开", false, false, true),
        ]
    );
    menu.viewer = true;
    let viewer = session_sysmenu(&menu);
    assert_eq!(
        viewer[1],
        submenu(
            "发送组合键",
            vec![
                command(ID_COMBO_BASE, "Alt+Tab", false, false, false),
                command(ID_COMBO_BASE + 16, "Win 键", false, false, false),
            ],
            false,
        )
    );
    assert_eq!(
        viewer[2],
        command(ID_CTRL_ALT_DEL, "Ctrl+Alt+Del", false, false, false)
    );
    assert_eq!(viewer[4], command(ID_FULLSCREEN, "全屏", true, false, true));
    assert_eq!(
        viewer[5],
        command(ID_KEYBOARD, "捕获键盘", true, false, false)
    );
    assert_eq!(
        viewer[6],
        command(ID_POINTER_LOCK, "锁定鼠标", false, false, false)
    );
    assert_eq!(
        viewer[10],
        command(ID_REQUEST_CONTROL, "请求控制", false, false, true)
    );
    assert_eq!(
        viewer[11],
        command(ID_DISCONNECT, "断开", false, false, true)
    );

    menu.displays.truncate(1);
    menu.combos.clear();
    menu.ctrl_alt_del = false;
    let items = session_sysmenu(&menu);
    assert!(!items.iter().any(|item| matches!(item, WinMenuItem::Submenu { text, .. } if text == "显示器" || text == "发送组合键")));
    assert!(!items.iter().any(|item| matches!(
        item,
        WinMenuItem::Command {
            id: ID_CTRL_ALT_DEL,
            ..
        }
    )));
}

fn menu_commands(items: &[WinMenuItem]) -> Vec<u32> {
    let mut ids = Vec::new();
    for item in items {
        match item {
            WinMenuItem::Command { id, .. } => ids.push(*id),
            WinMenuItem::Submenu { items, .. } => ids.extend(menu_commands(items)),
            WinMenuItem::Separator => {}
        }
    }
    ids
}

#[test]
fn sysmenu_ids() {
    let menu = menu();
    for (id, expected) in [
        (0x1000, MenuCommand::ToggleFullscreen),
        (0x1003, MenuCommand::ToggleFullscreen),
        (ID_KEYBOARD, MenuCommand::ToggleKeyboardCapture),
        (ID_POINTER_LOCK, MenuCommand::TogglePointerLock),
        (ID_CTRL_ALT_DEL, MenuCommand::CtrlAltDel),
        (ID_REQUEST_CONTROL, MenuCommand::RequestControl),
        (ID_DISCONNECT, MenuCommand::Disconnect),
        (0x1110, MenuCommand::SetQuality("smooth".into())),
        (0x1210, MenuCommand::SendCombo(menu.combos[1].clone())),
        (0x1410, MenuCommand::SelectDisplay(menu.displays[1].0)),
    ] {
        assert_eq!(sysmenu_command(id, &menu), Some(expected));
    }
    for id in [
        0x1420, 0xF060, 0xF000, 0x1_1000, 0x1060, 0x1140, 0x1220, 0x1600, 0,
    ] {
        assert_eq!(sysmenu_command(id, &menu), None, "ID {id:#x}");
    }
    for id in menu_commands(&session_sysmenu(&menu)) {
        assert!(id < 0xF000);
        assert_eq!(id % 16, 0);
        assert!(sysmenu_command(id | 0xF, &menu).is_some());
    }
    for (i, &(quality, _)) in QUALITY_ORDER.iter().enumerate() {
        assert_eq!(
            sysmenu_command(ID_QUALITY_BASE + 16 * i as u32, &menu),
            Some(MenuCommand::SetQuality(quality.into()))
        );
    }
}

#[test]
fn sysmenu_limits_dynamic_ids_without_collisions() {
    let mut menu = menu();
    menu.displays = (0..40).map(|i| (i, format!("显示器 {i}"))).collect();
    menu.combos = (0..40).map(|i| format!("combo{i}")).collect();
    let mut ids = menu_commands(&session_sysmenu(&menu));
    let count = ids.len();
    ids.sort_unstable();
    ids.dedup();
    assert_eq!(ids.len(), count);
    assert_eq!(
        ids.iter()
            .filter(|&&id| (0x1200..0x1400).contains(&id))
            .count(),
        32
    );
    assert_eq!(
        ids.iter()
            .filter(|&&id| (0x1400..0x1600).contains(&id))
            .count(),
        32
    );
    assert_eq!(
        sysmenu_command(0x15F0, &menu),
        Some(MenuCommand::SelectDisplay(31))
    );
    assert_eq!(sysmenu_command(0x1600, &menu), None);
    assert!(ids.iter().all(|id| *id < 0xF000 && id.is_multiple_of(16)));
}

#[test]
fn combo_labels() {
    for (name, text) in [
        ("cmdTab", "⌘Tab"),
        ("cmdSpace", "⌘空格"),
        ("missionControl", "调度中心"),
        ("altTab", "Alt+Tab"),
        ("winKey", "Win 键"),
        ("futureCombo", "futureCombo"),
    ] {
        assert_eq!(combo_text(name), text);
    }
}

fn encode_png(width: u32, height: u32, color: png::ColorType, pixels: &[u8]) -> Vec<u8> {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(color);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(pixels).unwrap();
        writer.finish().unwrap();
    }
    png
}

fn image() -> CursorImage {
    let pixels: Vec<u8> = (0..16).flat_map(|i| [i, i + 16, i + 32, 255 - i]).collect();
    CursorImage {
        id: 1,
        width: 4,
        height: 4,
        hot_x: 2,
        hot_y: 2,
        scale: 2.0,
        png: encode_png(4, 4, png::ColorType::Rgba, &pixels),
    }
}

#[test]
fn cursor_bitmap_scaling() {
    let image = image();
    let bmp = cursor_bitmap(&image, 1.0).unwrap();
    assert_eq!((bmp.width, bmp.height, bmp.hot_x, bmp.hot_y), (2, 2, 1, 1));
    // 最近邻选择源像素 0、2、8、10；每行自上而下，RGBA 转为非预乘 BGRA。
    assert_eq!(
        bmp.bgra,
        [32, 16, 0, 255, 34, 18, 2, 253, 40, 24, 8, 247, 42, 26, 10, 245]
    );
    let bmp = cursor_bitmap(&image, 1.5).unwrap();
    assert_eq!((bmp.width, bmp.height, bmp.hot_x, bmp.hot_y), (3, 3, 2, 2));
    let red: Vec<u8> = bmp.bgra.chunks_exact(4).map(|pixel| pixel[2]).collect();
    assert_eq!(red, [0, 1, 2, 4, 5, 6, 8, 9, 10]);
    let bmp = cursor_bitmap(&image, 200.0).unwrap();
    assert_eq!((bmp.width, bmp.height), (256, 256));
    assert_eq!((bmp.hot_x, bmp.hot_y), (128, 128));
    assert_eq!(bmp.bgra.len(), 256 * 256 * 4);
    assert_eq!(&bmp.bgra[..4], &[32, 16, 0, 255]);
    assert_eq!(&bmp.bgra[bmp.bgra.len() - 4..], &[47, 31, 15, 240]);

    let bmp = cursor_bitmap(&image, 0.001).unwrap();
    assert_eq!((bmp.width, bmp.height, bmp.hot_x, bmp.hot_y), (1, 1, 0, 0));
    let outside_hotspot = CursorImage {
        hot_x: u32::MAX,
        hot_y: u32::MAX,
        ..image
    };
    let bmp = cursor_bitmap(&outside_hotspot, 1.0).unwrap();
    assert_eq!((bmp.hot_x, bmp.hot_y), (1, 1));
}

#[test]
fn cursor_bitmap_rgb_and_palette() {
    let mut image = CursorImage {
        id: 2,
        width: 2,
        height: 1,
        hot_x: 0,
        hot_y: 0,
        scale: 1.0,
        png: encode_png(2, 1, png::ColorType::Rgb, &[10, 20, 30, 40, 50, 60]),
    };
    assert_eq!(
        cursor_bitmap(&image, 1.0).unwrap().bgra,
        [30, 20, 10, 255, 60, 50, 40, 255]
    );

    for transparency in [None, Some(vec![128, 0])] {
        let mut png = Vec::new();
        {
            let mut encoder = png::Encoder::new(&mut png, 2, 1);
            encoder.set_color(png::ColorType::Indexed);
            encoder.set_depth(png::BitDepth::One);
            encoder.set_palette(vec![10, 20, 30, 40, 50, 60]);
            if let Some(alpha) = transparency.clone() {
                encoder.set_trns(alpha);
            }
            let mut writer = encoder.write_header().unwrap();
            writer.write_image_data(&[0b0100_0000]).unwrap();
            writer.finish().unwrap();
        }
        image.png = png;
        let (a0, a1) = if transparency.is_some() {
            (128, 0)
        } else {
            (255, 255)
        };
        assert_eq!(
            cursor_bitmap(&image, 1.0).unwrap().bgra,
            [30, 20, 10, a0, 60, 50, 40, a1]
        );
    }
}

#[test]
fn cursor_bitmap_grayscale() {
    for (color, pixels, expected) in [
        (
            png::ColorType::Grayscale,
            vec![70, 90],
            vec![70, 70, 70, 255, 90, 90, 90, 255],
        ),
        (
            png::ColorType::GrayscaleAlpha,
            vec![70, 128, 90, 0],
            vec![70, 70, 70, 128, 90, 90, 90, 0],
        ),
    ] {
        let image = CursorImage {
            id: 3,
            width: 2,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            scale: 1.0,
            png: encode_png(2, 1, color, &pixels),
        };
        assert_eq!(cursor_bitmap(&image, 1.0).unwrap().bgra, expected);
    }
}

#[test]
fn cursor_bitmap_errors() {
    let image = image();
    for png in [
        Vec::new(),
        b"bad PNG".to_vec(),
        image.png[..33].to_vec(),
        image.png[..image.png.len() - 12].to_vec(),
    ] {
        let bad = CursorImage {
            png,
            ..image.clone()
        };
        assert!(matches!(
            cursor_bitmap(&bad, 1.0),
            Err(PlatformError::Failed(_))
        ));
    }
    for scale in [0.0, -1.0, f32::NAN, f32::INFINITY] {
        let bad = CursorImage {
            scale,
            ..image.clone()
        };
        assert!(matches!(
            cursor_bitmap(&bad, 1.0),
            Err(PlatformError::Failed(_))
        ));
        assert!(matches!(
            cursor_bitmap(&image, scale),
            Err(PlatformError::Failed(_))
        ));
    }
    for (width, height) in [(0, 4), (4, 0), (5, 4), (4, 5)] {
        let bad = CursorImage {
            width,
            height,
            ..image.clone()
        };
        assert!(matches!(
            cursor_bitmap(&bad, 1.0),
            Err(PlatformError::Failed(_))
        ));
    }
}

#[test]
fn cursor_bitmap_checks_png_header_before_decoding() {
    // 伪造 CursorImage 的小尺寸，PNG 本身超过上限。只留下合法 IHDR，确保先拒绝尺寸，
    // 不先分配图像输出缓冲，也不因缺失 IDAT 而走到解码阶段。
    for (width, height) in [(1025, 1), (1, 1025)] {
        let png = encode_png(
            width,
            height,
            png::ColorType::Rgb,
            &vec![0; (width * height * 3) as usize],
        );
        let image = CursorImage {
            png: png[..33].to_vec(),
            ..image()
        };
        assert_eq!(
            cursor_bitmap(&image, 1.0),
            Err(PlatformError::Failed(
                "cursor PNG dimensions must be within 1..=1024".into()
            ))
        );
    }
}

#[test]
fn premultiply() {
    let bmp = CursorBitmap {
        width: 4,
        height: 1,
        hot_x: 0,
        hot_y: 0,
        bgra: vec![
            0, 0, 255, 128, 100, 200, 50, 0, 10, 20, 30, 255, 255, 127, 1, 128,
        ],
    };
    assert_eq!(
        premultiplied(&bmp),
        [0, 0, 128, 128, 0, 0, 0, 0, 10, 20, 30, 255, 128, 64, 1, 128]
    );
    assert_eq!(bmp.bgra[2], 255);
}
