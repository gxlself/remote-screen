//! 电脑端平台层接口：窗口、呈现、输入与会话生命周期。
//!
//! 线程约定：除注明外，trait 方法可在任意线程调用，实现自行切到主线程
//! （Mac：主队列；Windows：窗口线程，经 PostMessage）。

use std::{path::Path, sync::Arc};

use rsp_transport::identity::ExternalSigner;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PlatformError {
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("failed: {0}")]
    Failed(String),
}

// ---------------------------------------------------------------- 系统

/// 本机（控制端）系统。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClientOs {
    Mac,
    Windows,
}

impl ClientOs {
    /// 编译目标；Linux（只跑测试）当作 Mac。
    pub const fn current() -> Self {
        if cfg!(windows) {
            Self::Windows
        } else {
            Self::Mac
        }
    }
}

/// 被控 Host 的系统（前端 `OpenSessionArgs.hostOs`）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HostOs {
    Mac,
    Windows,
    Unknown,
}

impl HostOs {
    /// "macos" → Mac，"windows" → Windows，其他 → Unknown。
    pub fn from_os_name(os: &str) -> Self {
        match os {
            "macos" => Self::Mac,
            "windows" => Self::Windows,
            _ => Self::Unknown,
        }
    }
}

// ---------------------------------------------------------------- 视频

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VideoCodec {
    Hevc,
    H264,
}

/// 平台解码器要的码流格式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BitstreamFormat {
    /// 原始 Annex B（Windows FFmpeg）。
    AnnexB,
    /// 4 字节大端长度前缀的 NAL 串；关键帧另带参数集（Mac VideoToolbox）。
    Avcc,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamInfo {
    pub codec: VideoCodec,
    pub width: u32,
    pub height: u32,
    pub display_id: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DesktopFrame {
    pub frame_id: u32,
    pub epoch: u32,
    pub display_id: u32,
    pub codec: VideoCodec,
    pub keyframe: bool,
    /// `AnnexB`：原始 Annex B；`Avcc`：长度前缀 NAL 串（不含参数集）。
    pub data: Vec<u8>,
    /// 只在 `Avcc` 且关键帧时有；`vps` 只有 HEVC 有。
    pub vps: Option<Vec<u8>>,
    pub sps: Option<Vec<u8>>,
    pub pps: Option<Vec<u8>>,
    /// Host 采集时刻（Host unix 微秒）。
    pub capture_ts_us: u64,
    /// 换算到本机时钟的采集时刻（unix 微秒）；时钟未同步时为 None。
    pub capture_local_us: Option<i64>,
    /// 本机收齐该帧的时刻（unix 微秒）。
    pub received_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DecodeResult {
    /// 已解码并交给显示层；`decode_us` 为提交到出图的耗时。
    Shown { decode_us: u64 },
    /// 解码失败。`frames` 会按 `DecoderReset` 请求关键帧。
    Failed,
}

/// 每帧一个回执。必须调用一次 `finish`；没调用就被 drop，视为 `Failed`。
pub struct FrameDone(Option<Box<dyn FnOnce(DecodeResult) + Send>>);

impl FrameDone {
    pub fn new(f: impl FnOnce(DecodeResult) + Send + 'static) -> Self {
        Self(Some(Box::new(f)))
    }
    pub fn finish(mut self, result: DecodeResult) {
        if let Some(f) = self.0.take() {
            f(result);
        }
    }
}

impl Drop for FrameDone {
    fn drop(&mut self) {
        if let Some(f) = self.0.take() {
            f(DecodeResult::Failed);
        }
    }
}

impl std::fmt::Debug for FrameDone {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str("FrameDone")
    }
}

/// 平台解码 + 显示。由 `frames` 在 RsCore 工作线程上调用。
pub trait FrameSink: Send + Sync + 'static {
    fn format(&self) -> BitstreamFormat;
    /// 流配置变化（首次、换分辨率、换显示器）。之后第一帧一定是带参数集的关键帧。
    fn configure(&self, info: &StreamInfo);
    /// 提交一帧；不得阻塞超过 1 ms（实现内部排队到解码线程）。
    fn submit(&self, frame: DesktopFrame, done: FrameDone);
    /// 会话结束：释放解码器，画面保留最后一帧直到窗口关闭。
    fn close(&self);
}

// ---------------------------------------------------------------- 窗口

#[derive(Debug, Clone, PartialEq)]
pub struct CursorImage {
    /// CursorShape.id（xxh3）。
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    /// 图片像素 / 点。
    pub scale: f32,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerCapture {
    /// 不捕获，本机光标自由进出。
    Free,
    /// 系统把光标限制在视频区域内（Windows `ClipCursor`），坐标仍取绝对位置。
    Confine,
    /// 隐藏并锁住系统光标，只读相对位移；窗口按 `MoveVirtualCursor` 画虚拟光标（Mac）。
    Virtual,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowCommand {
    SetTitle(String),
    SetFullscreen(bool),
    /// true：窗口把所有按键（含 ⌘ 快捷键、Alt+F4、单按 Alt）交给 `WindowEvents::key`，不让本机菜单 / 系统处理；
    /// false：按键照常先给本机菜单和系统（Mac 不拦 `performKeyEquivalent`，Windows 调 DefWindowProc），仍会转给 `WindowEvents::key`。
    SetKeyboardCapture(bool),
    /// Windows：true 时在窗口有焦点期间装 `WH_KEYBOARD_LL`，截 Win、Alt+Tab、Ctrl+Esc、Alt+Esc；Mac：无操作（v1 不装事件 tap）。
    SetSystemKeyGrab(bool),
    SetPointerCapture(PointerCapture),
    /// None = 系统箭头。
    SetCursorImage(Option<CursorImage>),
    /// 光标在视频区域上方时是否隐藏（Host 发来 `hidden`）。
    SetCursorHidden(bool),
    /// true：窗口在画面上按 `MoveVirtualCursor` 的位置画远端光标图（`Virtual` 捕获时、观看者时）；
    /// false：不画（自由 / 限制模式下本机光标就是远端光标）。
    ShowRemoteCursor(bool),
    /// 远端光标位置，视频内容归一化坐标 0..1（`Virtual` 时是虚拟光标，观看者时是 Host 光标）。
    MoveVirtualCursor {
        x: f32,
        y: f32,
    },
    /// 画面上叠一行状态文字（如「正在重连…」）；None 清除。
    SetOverlayText(Option<String>),
    /// 会话菜单状态。Mac 上无操作（菜单栏由外壳按 `SessionMenu` 画）；
    /// Windows 上由会话窗口画进系统菜单。
    SetSessionMenu(SessionMenu),
    Focus,
    Close,
}

/// 会话菜单的当前状态。
#[derive(Debug, Clone, PartialEq, Default)]
pub struct SessionMenu {
    /// (display_id, 名称)。少于两个时不显示「显示器」子菜单。
    pub displays: Vec<(u32, String)>,
    pub current_display: Option<u32>,
    /// auto / clear / smooth / eco。
    pub quality: String,
    /// 观看者：组合键、Ctrl+Alt+Del 置灰，显示「请求控制」。
    pub viewer: bool,
    pub fullscreen: bool,
    pub keyboard_captured: bool,
    pub pointer_locked: bool,
    /// 要列出的组合键（`SessionCombo` 的 JSON 名），按 Host 系统给：
    /// Mac Host = cmdTab / cmdSpace / missionControl；Windows Host = altTab / winKey；未知 = 全部。
    pub combos: Vec<String>,
    /// Host 是 Windows 时为 true，菜单显示 Ctrl+Alt+Del。
    pub ctrl_alt_del: bool,
}

pub trait SessionWindow: Send + Sync + 'static {
    fn frame_sink(&self) -> Arc<dyn FrameSink>;
    fn apply(&self, cmd: WindowCommand);
}

// ---------------------------------------------------------------- 输入（平台 → 控制器）

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyInput {
    /// Mac keyDown / keyUp：`NSEvent.keyCode`；`repeat` = `NSEvent.isARepeat`。
    MacKey { vk: u16, down: bool, repeat: bool },
    /// Mac flagsChanged：`NSEvent.keyCode` + `NSEvent.modifierFlags.rawValue`（含设备相关低位）。
    MacFlags { vk: u16, flags: u64 },
    /// Windows：WM_(SYS)KEYDOWN/UP 的 lParam（位 16–23 扫描码、位 24 扩展、位 30 先前状态 = 重复），
    /// 或 LL 钩子的 KBDLLHOOKSTRUCT（scanCode、LLKHF_EXTENDED）。
    WinKey {
        scan: u16,
        extended: bool,
        vk: u16,
        down: bool,
        repeat: bool,
    },
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum PointerInput {
    /// 视图坐标（逻辑点，左上原点）。`dx` / `dy` 为原始相对位移（`Virtual` 捕获时用；拿不到填 0）。
    Move { x: f64, y: f64, dx: f64, dy: f64 },
    /// `button`：协议编号，0 左 1 右 2 中 3 后退 4 前进。`clicks`：平台给的连击数，拿不到填 0，由控制器计数。
    Button {
        button: u8,
        down: bool,
        x: f64,
        y: f64,
        clicks: u8,
    },
    /// 光标离开视图。
    Leave,
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum ScrollInput {
    /// Mac `hasPreciseScrollingDeltas == true`：scrollingDeltaX/Y（点）；
    /// `phase` / `momentum` 为 `NSEvent.phase` / `momentumPhase` 原始位。
    MacPrecise {
        dx: f64,
        dy: f64,
        phase: u64,
        momentum: u64,
    },
    /// Mac 普通滚轮：scrollingDeltaX/Y（行）。
    MacLines { dx: f64, dy: f64 },
    /// Windows WM_MOUSEWHEEL / WM_MOUSEHWHEEL 的 GET_WHEEL_DELTA_WPARAM。
    WinWheel { delta: i32, horizontal: bool },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum MenuCommand {
    /// rs-mobile `SessionCombo` 的 JSON 名：cmdTab / cmdSpace / missionControl / altTab / winKey。
    SendCombo(String),
    CtrlAltDel,
    ToggleFullscreen,
    ToggleKeyboardCapture,
    TogglePointerLock,
    /// auto / clear / smooth / eco。
    SetQuality(String),
    SelectDisplay(u32),
    RequestControl,
    Disconnect,
}

#[derive(Debug, Clone, PartialEq)]
pub enum WindowChange {
    /// 视图逻辑尺寸与像素比。
    Resized {
        width: f64,
        height: f64,
        scale: f64,
    },
    Focus(bool),
    Fullscreen(bool),
    Minimized(bool),
    CloseRequested,
    Menu(MenuCommand),
}

/// 控制器实现；平台窗口在主线程调用，实现不得阻塞。
pub trait WindowEvents: Send + Sync + 'static {
    fn key(&self, ev: KeyInput);
    fn pointer(&self, ev: PointerInput);
    fn scroll(&self, ev: ScrollInput);
    fn window(&self, ev: WindowChange);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WindowSpec {
    pub session_id: u64,
    pub host_id: String,
    pub title: String,
    /// 初始内容尺寸（逻辑点）。
    pub width: u32,
    pub height: u32,
}

pub trait SessionWindowFactory: Send + Sync + 'static {
    /// 创建并显示窗口。调用方不在主线程；实现切到主线程执行，可阻塞等到创建完成。
    fn create(
        &self,
        spec: &WindowSpec,
        events: Arc<dyn WindowEvents>,
    ) -> Result<Arc<dyn SessionWindow>, PlatformError>;
    /// 本机能否硬解该编码（选 `OpenOpts.codec` 用）。
    fn can_decode(&self, codec: VideoCodec) -> bool;
    /// 主窗口所在屏幕的像素宽（选 `OpenOpts.max_width` 用）。
    fn screen_pixel_width(&self) -> u32;
}

// ---------------------------------------------------------------- 入口

/// 本机设备密钥（04 · 电脑端）。
pub fn device_signer(data_dir: &Path) -> Result<Arc<dyn ExternalSigner>, PlatformError> {
    #[cfg(target_os = "macos")]
    {
        crate::mac_signer::open_or_create(data_dir)
    }
    #[cfg(windows)]
    {
        crate::win_signer::open_or_create(data_dir)
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        let _ = data_dir;
        Err(PlatformError::Unsupported(
            "no device signer on this platform".into(),
        ))
    }
}

/// 本机会话窗口工厂；Linux 上为 None。
pub fn window_factory() -> Option<Arc<dyn SessionWindowFactory>> {
    #[cfg(target_os = "macos")]
    {
        crate::mac_window::factory()
    }
    #[cfg(windows)]
    {
        crate::win_window::factory()
    }
    #[cfg(not(any(target_os = "macos", windows)))]
    {
        None
    }
}
