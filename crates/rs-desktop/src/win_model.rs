//! Windows 会话窗口的纯逻辑；只解释消息数值，不调用 Win32。

use std::{collections::HashSet, io::Cursor};

use crate::{
    keymap::win_decode_lparam,
    platform::{CursorImage, KeyInput, MenuCommand, PlatformError, ScrollInput, SessionMenu},
    pointer::Viewport,
};

pub const WM_KEYDOWN: u32 = 0x0100;
pub const WM_KEYUP: u32 = 0x0101;
pub const WM_SYSKEYDOWN: u32 = 0x0104;
pub const WM_SYSKEYUP: u32 = 0x0105;
pub const LLKHF_EXTENDED: u32 = 0x01;
pub const LLKHF_INJECTED: u32 = 0x10;
pub const LLKHF_UP: u32 = 0x80;
pub const VK_PACKET: u16 = 0xE7;
pub const VK_SNAPSHOT: u16 = 0x2C;

fn key_message_down(msg: u32) -> Option<bool> {
    match msg {
        WM_KEYDOWN | WM_SYSKEYDOWN => Some(true),
        WM_KEYUP | WM_SYSKEYUP => Some(false),
        _ => None,
    }
}

/// WM_(SYS)KEYDOWN/UP → 物理按键；VK_PACKET 只有注入的 Unicode 字符，没有物理键。
pub fn key_from_message(msg: u32, wparam: usize, lparam: isize) -> Option<KeyInput> {
    let down = key_message_down(msg)?;
    let vk = u16::try_from(wparam).ok()?;
    let param = win_decode_lparam(lparam);
    if vk == VK_PACKET || param.scan == 0 {
        return None;
    }
    Some(KeyInput::WinKey {
        scan: param.scan,
        extended: param.extended,
        vk,
        down,
        repeat: down && param.repeat,
    })
}

/// 低级键盘钩子 → 物理按键。钩子没有先前状态位，重复由 `KeyFilter` 推断。
/// scanCode 的 0x200 是 AltGr 假 Ctrl 标记；扫描码只取低 8 位，扩展位只取 flags。
pub fn key_from_ll_hook(wparam_msg: u32, vk: u32, scan_code: u32, flags: u32) -> Option<KeyInput> {
    let down = key_message_down(wparam_msg)? && flags & LLKHF_UP == 0;
    let vk = u16::try_from(vk).ok()?;
    let scan = (scan_code & 0xFF) as u16;
    if flags & LLKHF_INJECTED != 0 || vk == VK_PACKET || scan == 0 {
        return None;
    }
    Some(KeyInput::WinKey {
        scan,
        extended: flags & LLKHF_EXTENDED != 0,
        vk,
        down,
        repeat: false,
    })
}

/// AltGr 布局的低级钩子把合成左 Ctrl 的 scanCode 标成 0x21D。
pub fn ll_is_fake_ctrl(scan_code: u32) -> bool {
    scan_code & 0x200 != 0
}

/// AltGr 布局先发合成左 Ctrl 按下，紧接同一消息时间的右 Alt 按下；松开也会合成左 Ctrl。
/// 窗口层负责窥视下一条消息，这里只比较消息内容和时间。
pub fn is_altgr_fake_ctrl(
    cur: &KeyInput,
    cur_time: u32,
    next: Option<(u32, usize, isize, u32)>,
) -> bool {
    if !matches!(
        cur,
        KeyInput::WinKey {
            scan: 0x1D,
            extended: false,
            vk: 0x11,
            down: true,
            ..
        }
    ) {
        return false;
    }
    let Some((msg, wparam, lparam, time)) = next else {
        return false;
    };
    time == cur_time
        && matches!(msg, WM_KEYDOWN | WM_SYSKEYDOWN)
        && wparam == 0x12
        && win_decode_lparam(lparam).extended
}

/// 一个会话窗口一份；失焦或关闭捕获时调用 `clear`，远端释放由控制器负责。
#[derive(Debug, Default)]
pub struct KeyFilter {
    pressed: HashSet<(u16, bool)>,
    drop_fake_ctrl_up: bool,
}

impl KeyFilter {
    pub fn new() -> Self {
        Self::default()
    }

    /// 返回交给 `WindowEvents::key` 的 0–2 个事件，非 Windows 事件原样传递。
    pub fn on_key(&mut self, ev: KeyInput, altgr_fake: bool) -> Vec<KeyInput> {
        let KeyInput::WinKey {
            scan,
            extended,
            vk,
            down,
            repeat,
        } = ev
        else {
            return vec![ev];
        };
        if altgr_fake && down {
            self.drop_fake_ctrl_up = true;
            return Vec::new();
        }
        let key = (scan, extended);
        let was_down = self.pressed.contains(&key);
        if !down && key == (0x1D, false) && !was_down && self.drop_fake_ctrl_up {
            self.drop_fake_ctrl_up = false;
            return Vec::new();
        }
        if !down && !was_down {
            // 普通窗口的 Print Screen 通常只有扩展扫描码 0x37 的 WM_KEYUP，没有按下。
            return if key == (0x37, true) {
                vec![
                    KeyInput::WinKey {
                        scan,
                        extended,
                        vk,
                        down: true,
                        repeat: false,
                    },
                    ev,
                ]
            } else {
                Vec::new()
            };
        }
        if down {
            self.pressed.insert(key);
        } else {
            self.pressed.remove(&key);
        }
        vec![KeyInput::WinKey {
            scan,
            extended,
            vk,
            down,
            repeat: repeat || (down && was_down),
        }]
    }

    pub fn clear(&mut self) {
        self.pressed.clear();
        self.drop_fake_ctrl_up = false;
    }
}

/// 鼠标按钮消息 → (协议按钮号, 按下)。DBLCLK 也是按下，连击次数由控制器计数。
pub fn button_from_message(msg: u32, wparam: usize) -> Option<(u8, bool)> {
    match msg {
        0x0201 | 0x0203 => Some((0, true)), // WM_LBUTTONDOWN / DBLCLK
        0x0202 => Some((0, false)),         // WM_LBUTTONUP
        0x0204 | 0x0206 => Some((1, true)), // WM_RBUTTONDOWN / DBLCLK
        0x0205 => Some((1, false)),         // WM_RBUTTONUP
        0x0207 | 0x0209 => Some((2, true)), // WM_MBUTTONDOWN / DBLCLK
        0x0208 => Some((2, false)),         // WM_MBUTTONUP
        0x020B..=0x020D => {
            // WM_XBUTTONDOWN / UP / DBLCLK：HIWORD 只有 XBUTTON1 和 XBUTTON2 有效。
            let button = match (wparam >> 16) & 0xFFFF {
                1 => 3,
                2 => 4,
                _ => return None,
            };
            Some((button, msg != 0x020C))
        }
        _ => None,
    }
}

/// GET_X_LPARAM / GET_Y_LPARAM；多屏坐标可能为负，必须按有符号 16 位解释。
pub fn lparam_point(lparam: isize) -> (i32, i32) {
    (i32::from(lparam as i16), i32::from((lparam >> 16) as i16))
}

/// WM_MOUSEWHEEL / WM_MOUSEHWHEEL；保留 Win32 的有符号滚轮增量。
pub fn wheel_from_message(msg: u32, wparam: usize) -> Option<ScrollInput> {
    match msg {
        0x020A | 0x020E => Some(ScrollInput::WinWheel {
            delta: i32::from((wparam >> 16) as i16),
            horizontal: msg == 0x020E,
        }),
        _ => None,
    }
}

/// 客户区物理像素 → 逻辑点；未知 DPI 按 96 处理。
pub fn to_logical(px: i32, py: i32, dpi: u32) -> (f64, f64) {
    let dpi = f64::from(if dpi == 0 { 96 } else { dpi });
    (f64::from(px) * 96.0 / dpi, f64::from(py) * 96.0 / dpi)
}

/// 浮层左上角，单位为客户区物理像素；`vp` 和热点也必须使用物理像素。
pub fn remote_cursor_origin(vp: &Viewport, nx: f32, ny: f32, hot_x: u32, hot_y: u32) -> (i32, i32) {
    let (x, y) = vp.to_view(nx, ny);
    (
        (x - f64::from(hot_x)).round() as i32,
        (y - f64::from(hot_y)).round() as i32,
    )
}

// 自加系统菜单 ID 必须小于 0xF000，并留出 WM_SYSCOMMAND 系统自用的低 4 位。
pub const ID_FULLSCREEN: u32 = 0x1000;
pub const ID_KEYBOARD: u32 = 0x1010;
pub const ID_POINTER_LOCK: u32 = 0x1020;
pub const ID_CTRL_ALT_DEL: u32 = 0x1030;
pub const ID_REQUEST_CONTROL: u32 = 0x1040;
pub const ID_DISCONNECT: u32 = 0x1050;
pub const ID_QUALITY_BASE: u32 = 0x1100;
pub const ID_COMBO_BASE: u32 = 0x1200;
pub const ID_DISPLAY_BASE: u32 = 0x1400;
pub const QUALITY_ORDER: [(&str, &str); 4] = [
    ("auto", "自动"),
    ("smooth", "流畅"),
    ("clear", "清晰"),
    ("eco", "省流量"),
];

// 每项占 16 个 ID；组合键不能越过显示器区间，即使收到异常长的组合键列表。
const MAX_COMBOS: usize = ((ID_DISPLAY_BASE - ID_COMBO_BASE) / 16) as usize;
const MAX_DISPLAYS: usize = 32;

#[derive(Debug, Clone, PartialEq)]
pub enum WinMenuItem {
    Command {
        id: u32,
        text: String,
        checked: bool,
        radio: bool,
        enabled: bool,
    },
    Separator,
    Submenu {
        text: String,
        items: Vec<WinMenuItem>,
        enabled: bool,
    },
}

fn command(id: u32, text: &str, checked: bool, radio: bool, enabled: bool) -> WinMenuItem {
    WinMenuItem::Command {
        id,
        text: text.to_owned(),
        checked,
        radio,
        enabled,
    }
}

/// 追加在系统菜单原有项之后的会话菜单。
pub fn session_sysmenu(menu: &SessionMenu) -> Vec<WinMenuItem> {
    let control_enabled = !menu.viewer;
    let mut items = vec![WinMenuItem::Separator];
    if !menu.combos.is_empty() {
        items.push(WinMenuItem::Submenu {
            text: "发送组合键".into(),
            items: menu
                .combos
                .iter()
                .take(MAX_COMBOS)
                .enumerate()
                .map(|(i, name)| {
                    command(
                        ID_COMBO_BASE + 16 * i as u32,
                        combo_text(name),
                        false,
                        false,
                        control_enabled,
                    )
                })
                .collect(),
            enabled: control_enabled,
        });
    }
    if menu.ctrl_alt_del {
        items.push(command(
            ID_CTRL_ALT_DEL,
            "Ctrl+Alt+Del",
            false,
            false,
            control_enabled,
        ));
    }
    items.extend([
        WinMenuItem::Separator,
        command(ID_FULLSCREEN, "全屏", menu.fullscreen, false, true),
        command(
            ID_KEYBOARD,
            "捕获键盘",
            menu.keyboard_captured,
            false,
            control_enabled,
        ),
        command(
            ID_POINTER_LOCK,
            "锁定鼠标",
            menu.pointer_locked,
            false,
            control_enabled,
        ),
        WinMenuItem::Submenu {
            text: "画质".into(),
            items: QUALITY_ORDER
                .iter()
                .enumerate()
                .map(|(i, &(quality, text))| {
                    command(
                        ID_QUALITY_BASE + 16 * i as u32,
                        text,
                        menu.quality == quality,
                        true,
                        true,
                    )
                })
                .collect(),
            enabled: true,
        },
    ]);
    if menu.displays.len() >= 2 {
        items.push(WinMenuItem::Submenu {
            text: "显示器".into(),
            items: menu
                .displays
                .iter()
                .take(MAX_DISPLAYS)
                .enumerate()
                .map(|(i, (display_id, name))| {
                    command(
                        ID_DISPLAY_BASE + 16 * i as u32,
                        name,
                        menu.current_display == Some(*display_id),
                        true,
                        true,
                    )
                })
                .collect(),
            enabled: true,
        });
    }
    items.push(WinMenuItem::Separator);
    if menu.viewer {
        items.push(command(ID_REQUEST_CONTROL, "请求控制", false, false, true));
    }
    items.push(command(ID_DISCONNECT, "断开", false, false, true));
    items
}

/// 忽略 WM_SYSCOMMAND 系统自用的低 4 位；只解释自定义 ID，不执行命令。
pub fn sysmenu_command(id: u32, menu: &SessionMenu) -> Option<MenuCommand> {
    if id >= 0xF000 {
        return None;
    }
    let id = id & 0xFFF0;
    match id {
        ID_FULLSCREEN => Some(MenuCommand::ToggleFullscreen),
        ID_KEYBOARD => Some(MenuCommand::ToggleKeyboardCapture),
        ID_POINTER_LOCK => Some(MenuCommand::TogglePointerLock),
        ID_CTRL_ALT_DEL => Some(MenuCommand::CtrlAltDel),
        ID_REQUEST_CONTROL => Some(MenuCommand::RequestControl),
        ID_DISCONNECT => Some(MenuCommand::Disconnect),
        ID_QUALITY_BASE..ID_COMBO_BASE => QUALITY_ORDER
            .get(((id - ID_QUALITY_BASE) / 16) as usize)
            .map(|&(name, _)| MenuCommand::SetQuality(name.into())),
        ID_COMBO_BASE..ID_DISPLAY_BASE => menu
            .combos
            .get(((id - ID_COMBO_BASE) / 16) as usize)
            .cloned()
            .map(MenuCommand::SendCombo),
        ID_DISPLAY_BASE..=0x15F0 => menu
            .displays
            .get(((id - ID_DISPLAY_BASE) / 16) as usize)
            .map(|&(display_id, _)| MenuCommand::SelectDisplay(display_id)),
        _ => None,
    }
}

/// `SessionCombo` 的 JSON 名 → 与 Mac 会话菜单一致的中文文字。
pub fn combo_text(name: &str) -> &str {
    match name {
        "cmdTab" => "⌘Tab",
        "cmdSpace" => "⌘空格",
        "missionControl" => "调度中心",
        "altTab" => "Alt+Tab",
        "winKey" => "Win 键",
        _ => name,
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBitmap {
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    /// 非预乘 BGRA，行优先自上而下。
    pub bgra: Vec<u8>,
}

fn png_error(err: png::DecodingError) -> PlatformError {
    PlatformError::Failed(format!("cursor PNG: {err}"))
}

/// PNG → 本机 DPI 下的 BGRA。先读 IHDR 限制为 1024×1024，再分配、解码。
/// 使用最近邻缩放，目标每边为 round(像素 / 图片倍率 × 本机倍率)，夹到 1..=256。
pub fn cursor_bitmap(img: &CursorImage, dst_scale: f32) -> Result<CursorBitmap, PlatformError> {
    if img.width == 0
        || img.height == 0
        || !img.scale.is_finite()
        || img.scale <= 0.0
        || !dst_scale.is_finite()
        || dst_scale <= 0.0
    {
        return Err(PlatformError::Failed(
            "invalid cursor dimensions or scale".into(),
        ));
    }
    let mut decoder = png::Decoder::new(Cursor::new(&img.png));
    decoder.set_limits(png::Limits {
        bytes: 16 * 1024 * 1024,
    });
    decoder.set_transformations(png::Transformations::EXPAND | png::Transformations::STRIP_16);
    let header = decoder.read_header_info().map_err(png_error)?;
    if header.width == 0 || header.height == 0 || header.width > 1024 || header.height > 1024 {
        return Err(PlatformError::Failed(
            "cursor PNG dimensions must be within 1..=1024".into(),
        ));
    }
    if header.width != img.width || header.height != img.height {
        return Err(PlatformError::Failed(
            "cursor PNG dimensions do not match CursorImage".into(),
        ));
    }
    let mut reader = decoder.read_info().map_err(png_error)?;
    let size = reader
        .output_buffer_size()
        .filter(|&size| size <= 1024 * 1024 * 4)
        .ok_or_else(|| PlatformError::Failed("cursor PNG output is too large".into()))?;
    let mut pixels = vec![0; size];
    let info = reader.next_frame(&mut pixels).map_err(png_error)?;
    reader.finish().map_err(png_error)?;
    if info.width != img.width
        || info.height != img.height
        || info.bit_depth != png::BitDepth::Eight
    {
        return Err(PlatformError::Failed("invalid cursor PNG frame".into()));
    }
    let channels = match info.color_type {
        png::ColorType::Rgba => 4,
        png::ColorType::Rgb => 3,
        png::ColorType::GrayscaleAlpha => 2,
        png::ColorType::Grayscale => 1,
        png::ColorType::Indexed => {
            return Err(PlatformError::Failed(
                "cursor PNG palette was not expanded".into(),
            ));
        }
    };
    let scale = f64::from(dst_scale) / f64::from(img.scale);
    let width = (f64::from(info.width) * scale).round().clamp(1.0, 256.0) as u32;
    let height = (f64::from(info.height) * scale).round().clamp(1.0, 256.0) as u32;
    let mut bgra = Vec::with_capacity(width as usize * height as usize * 4);
    for y in 0..height {
        let src_y = y * info.height / height;
        for x in 0..width {
            let src_x = x * info.width / width;
            let offset = src_y as usize * info.line_size + src_x as usize * channels;
            let pixel = pixels.get(offset..offset + channels).ok_or_else(|| {
                PlatformError::Failed("cursor PNG frame has incomplete pixels".into())
            })?;
            let rgba = match info.color_type {
                png::ColorType::Rgba => [pixel[0], pixel[1], pixel[2], pixel[3]],
                png::ColorType::Rgb => [pixel[0], pixel[1], pixel[2], 255],
                png::ColorType::GrayscaleAlpha => [pixel[0], pixel[0], pixel[0], pixel[1]],
                _ => [pixel[0], pixel[0], pixel[0], 255],
            };
            bgra.extend_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
        }
    }
    // 用夹限后的真实尺寸同比缩放热点，而不是仍用夹限前的 DPI 倍率。
    let hot_x = (f64::from(img.hot_x) * f64::from(width) / f64::from(info.width))
        .round()
        .clamp(0.0, f64::from(width - 1)) as u32;
    let hot_y = (f64::from(img.hot_y) * f64::from(height) / f64::from(info.height))
        .round()
        .clamp(0.0, f64::from(height - 1)) as u32;
    Ok(CursorBitmap {
        width,
        height,
        hot_x,
        hot_y,
        bgra,
    })
}

/// 预乘 alpha 的 BGRA，用于 UpdateLayeredWindow；四舍五入到最近的 8 位通道值。
pub fn premultiplied(bmp: &CursorBitmap) -> Vec<u8> {
    let mut pixels = bmp.bgra.clone();
    for pixel in pixels.chunks_exact_mut(4) {
        let alpha = u16::from(pixel[3]);
        for channel in &mut pixel[..3] {
            *channel = ((u16::from(*channel) * alpha + 127) / 255) as u8;
        }
    }
    pixels
}
