//! Platform boundary for capture, encoding and local OS services (06-host).

use std::any::Any;
use std::net::IpAddr;
use std::path::Path;
use std::sync::Arc;

use rsp_core::pb;
use tokio::sync::mpsc;

/// RSP/1 input events (03 section 6.1).
pub use rsp_core::hid::InputMsg;
/// M0 input events; A5 will extend the wire representation.
pub use rsp_core::input::InputEvent;
/// Session actions share the control-plane enum.
pub use rsp_core::pb::session_action::Kind as SessionActionKind;

/// Downgrade RSP/1 mouse events to M0, discarding scroll phases and momentum.
/// Line deltas are converted to pixels; unsupported buttons and other input
/// events have no M0 representation.
pub fn input_to_m0(msg: &InputMsg) -> Option<InputEvent> {
    use rsp_core::hid::{Button, ScrollUnit};
    use rsp_core::input::MouseButton;

    match *msg {
        InputMsg::MouseMove { x, y } => Some(InputEvent::MouseMove { x, y }),
        InputMsg::MouseButton {
            button,
            down,
            x,
            y,
            clicks,
        } => Some(InputEvent::MouseButton {
            button: match button {
                Button::Left => MouseButton::Left,
                Button::Right => MouseButton::Right,
                Button::Middle => MouseButton::Middle,
                Button::Back | Button::Forward => return None,
            },
            down,
            x,
            y,
            clicks,
        }),
        InputMsg::Scroll { dx, dy, unit, .. } => {
            let pixels_per_unit = match unit {
                ScrollUnit::Pixel => 1.0,
                ScrollUnit::Line => crate::inject::DEFAULT_PIXELS_PER_LINE,
            };
            Some(InputEvent::Scroll {
                dx: (f64::from(dx) * pixels_per_unit) as f32,
                dy: (f64::from(dy) * pixels_per_unit) as f32,
            })
        }
        InputMsg::Key { .. }
        | InputMsg::Text(_)
        | InputMsg::ReleaseAll
        | InputMsg::Touch { .. } => None,
    }
}

/// Convert M0 mouse input to RSP/1. Scroll deltas are pixels with no phases.
pub fn input_from_m0(ev: InputEvent) -> InputMsg {
    use rsp_core::hid::{Button, ScrollPhase, ScrollUnit};
    use rsp_core::input::MouseButton;

    match ev {
        InputEvent::MouseMove { x, y } => InputMsg::MouseMove { x, y },
        InputEvent::MouseButton {
            button,
            down,
            x,
            y,
            clicks,
        } => InputMsg::MouseButton {
            button: match button {
                MouseButton::Left => Button::Left,
                MouseButton::Right => Button::Right,
                MouseButton::Middle => Button::Middle,
            },
            down,
            x,
            y,
            clicks,
        },
        InputEvent::Scroll { dx, dy } => InputMsg::Scroll {
            dx,
            dy,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        },
    }
}

/// Errors returned by platform operations.
#[derive(Debug, thiserror::Error)]
pub enum PlatformError {
    #[error("unsupported platform operation")]
    Unsupported,
    #[error("permission denied: {0}")]
    PermissionDenied(String),
    #[error("platform operation failed: {0}")]
    Platform(String),
    #[error(transparent)]
    Io(#[from] std::io::Error),
}

/// Compatibility name for the original platform error.
pub type Error = PlatformError;

/// Result of a platform operation.
pub type Result<T> = std::result::Result<T, PlatformError>;

/// Host name, OS version, model and architecture.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct HostInfo {
    pub name: String,
    pub os: pb::Platform,
    pub os_version: String,
    pub model: String,
    pub arch: String,
}

/// Display geometry; desktop positions are in logical points.
#[derive(Debug, Clone, PartialEq)]
pub struct DisplayInfo {
    pub id: u32,
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f32,
    pub x: i32,
    pub y: i32,
    pub refresh_hz: u32,
    pub primary: bool,
    pub rotation: u32,
}

/// Capture rate and pixel limits; zero dimensions select the native size.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CaptureConfig {
    pub fps: u32,
    pub max_width: u32,
    pub max_height: u32,
}

/// 显示器原生像素矩形，左上角原点（与 `DisplayInfo.width_px/height_px` 同一坐标系；03 §5.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Default)]
pub struct PixelRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

/// 编码器的一块 QP 偏移区域。`rect` 用编码分辨率像素；列表里靠前的优先（重叠处按第一个）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RoiRegion {
    pub rect: PixelRect,
    /// 负数 = 更清晰；实现方自行夹到编码器允许的范围。
    pub qp_delta: i8,
}

/// 系统声音采集配置。M2 只用 48 kHz、双声道、排除本进程。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioCaptureConfig {
    pub sample_rate: u32,
    pub channels: u8,
    /// 排除 Host 进程自己发出的声音。
    pub exclude_self: bool,
}

/// 一块采集到的 PCM。
#[derive(Debug, Clone, PartialEq)]
pub struct AudioChunk {
    /// 块首样本的采集时刻（unix 微秒，与视频 capture_ts_us 同一时钟）。
    pub capture_ts_us: u64,
    pub channels: u8,
    /// 每声道样本数。
    pub frames: u32,
    /// 交错 f32，[-1, 1]，长度 = frames × channels。
    pub samples: Vec<f32>,
}

/// 平台用 try_send 交块，满了就丢，绝不阻塞采集线程。
pub type AudioChunkSink = tokio::sync::mpsc::Sender<AudioChunk>;

/// Platform callbacks use try_send and never block their native threads.
pub type FrameSink = mpsc::Sender<CapturedFrame>;

/// Dropping this handle stops capture and drains native callbacks.
pub trait CaptureHandle: Send {}

/// Encoder parameters and its bounded asynchronous output channel.
#[derive(Clone)]
pub struct EncoderConfig {
    pub codec: pb::Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub sink: mpsc::Sender<EncodedFrame>,
}

/// Per-frame keyframe and still-frame quality options.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EncodeOpts {
    pub force_keyframe: bool,
    pub quality_boost: bool,
}

/// Zero-copy native image. Only the platform downcasts native; its Arc owns
/// the image lifetime. capture_ts_us is a Unix timestamp in microseconds.
#[derive(Clone)]
pub struct CapturedFrame {
    pub display_id: u32,
    pub width: u32,
    pub height: u32,
    pub capture_ts_us: u64,
    pub native: Arc<dyn Any + Send + Sync>,
}

/// Encoded Annex-B access unit; timestamps are Unix microseconds.
#[derive(Clone)]
pub struct EncodedFrame {
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Non-keyframe re-encode of a still picture at higher quality (03 section 5.1 FLAG_STILL).
    pub still_refine: bool,
    pub capture_ts_us: u64,
    pub encoded_ts_us: u64,
}

/// Cursor position, reusable shape and visibility updates (03 section 6).
#[derive(Debug, Clone, PartialEq)]
pub enum CursorUpdate {
    Shape(pb::CursorShape),
    Use(u64),
    Hidden(bool),
    Position {
        display_id: u32,
        seq: u32,
        x: f32,
        y: f32,
        visible: bool,
    },
}

/// UTF-8 text or PNG (03 section 8.1); never log the contents.
#[derive(Clone)]
pub struct ClipboardItem {
    pub kind: pb::clipboard_header::Kind,
    pub data: Vec<u8>,
    pub sha256: [u8; 32],
}

/// Local console state; Unknown means the platform cannot query it yet.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ConsoleState {
    Unknown,
    LoggedIn,
    Locked,
    LoginWindow,
    OtherUser,
}

/// Screen recording and accessibility permissions.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Permissions {
    pub screen_recording: bool,
    pub accessibility: bool,
}

/// System permission to request.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PermissionKind {
    ScreenRecording,
    Accessibility,
}

/// Interface addresses, CIDR networks, MAC address and tailnet membership.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Nic {
    pub name: String,
    pub addresses: Vec<IpAddr>,
    pub networks: Vec<String>,
    pub mac: Option<[u8; 6]>,
    pub tailnet: bool,
}

/// Platform secure storage; names are application keys, never log values.
pub trait KeyStore: Send + Sync {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>>;
    fn store(&self, name: &str, value: &[u8]) -> Result<()>;
    fn delete(&self, name: &str) -> Result<()>;
}

/// 平台接口，签名来自 docs/dev/06-host.md 平台接口。
pub trait Platform: Send + Sync + 'static {
    /// A file from a phone was verified and renamed into place (03 §8.2).
    /// macOS marks it quarantined. Default: nothing.
    fn on_file_received(&self, path: &Path, device_name: &str) {
        let _ = (path, device_name);
    }

    fn info(&self) -> HostInfo; // 名称、系统版本、机型
    fn displays(&self) -> Vec<DisplayInfo>;
    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>); // 插拔、分辨率变化

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>>;
    /// Capture the primary display as a bounded JPEG thumbnail.
    ///
    /// Implementations scale the image proportionally so its width is at most
    /// `max_width` and the encoded result is at most `max_bytes` bytes. Return
    /// `None` when capture, encoding or either bound cannot be satisfied.
    /// This call may block; callers should run it on a blocking executor.
    fn thumbnail_jpeg(&self, max_width: u32, max_bytes: usize) -> Option<Vec<u8>> {
        let _ = (max_width, max_bytes);
        None
    }
    /// 能否只采显示器的一块区域（M3 视口裁剪，03 §5.5）。
    fn capture_region_supported(&self) -> bool {
        false
    }
    /// 只要显示器 `display` 的 `rect`（原生像素）。
    fn capture_region(
        &self,
        display: u32,
        rect: PixelRect,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let _ = (display, rect, cfg, sink);
        Err(PlatformError::Unsupported)
    }
    /// 能否采系统声音（macOS 13+ 的 SCK、Windows WASAPI loopback）。
    fn audio_capture_supported(&self) -> bool {
        false
    }
    /// 开始采系统声音。返回的 handle 被 drop 时停止。
    ///
    /// 平台用 try_send 交块，满了就丢，绝不阻塞采集线程。
    /// 采样率必须正好等于请求值，做不到返回 Unsupported；host-core 不重采样。
    /// drop handle 后 100 ms 内停止采集并释放 sink（通道随之关闭）。
    /// 与视频采集互相独立：视频重建、换屏不影响声音。
    /// 权限不足返回 PermissionDenied；macOS「屏幕与系统录音」一个授权同时管画面和声音。
    fn capture_audio(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let _ = (cfg, sink);
        Err(PlatformError::Unsupported)
    }
    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>>; // 硬件编码器

    fn inject(&self, ev: InputEvent, display: &DisplayInfo);
    /// RSP/1 input (03 section 6.1), with a fallback for M0 platforms.
    /// Unsupported input is discarded without logging its contents.
    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        if let Some(ev) = input_to_m0(msg) {
            self.inject(ev, display);
        } else if matches!(msg, InputMsg::ReleaseAll) {
            self.release_all();
        } else {
            tracing::debug!("discarding input unsupported by the M0 platform");
        }
    }
    fn release_all(&self);
    fn session_action(&self, kind: SessionActionKind) -> Result<()>; // 锁屏、Ctrl+Alt+Del、唤醒显示器

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>); // 形状 + 位置
    fn watch_local_activity(&self, tx: mpsc::Sender<bool>); // 电脑前有人操作（排除我们注入的事件）
    fn clipboard_get(&self) -> Option<ClipboardItem>;
    fn clipboard_set(&self, item: ClipboardItem) -> Result<()>;
    fn watch_clipboard(&self, tx: mpsc::Sender<()>);

    fn power_keep_awake(&self, on: bool);
    fn console_state(&self) -> ConsoleState; // 已登录 / 锁屏 / 登录窗口
    /// 系统「屏幕共享」是否开启；None = 无法判断（D37）。
    fn screen_sharing_enabled(&self) -> Option<bool> {
        None
    }
    /// FileVault 是否开启；None = 无法判断。不表示当前磁盘是否已解锁（D37）。
    fn filevault_enabled(&self) -> Option<bool> {
        None
    }
    fn permissions(&self) -> Permissions; // 屏幕录制、辅助功能
    fn request_permission(&self, p: PermissionKind);
    fn nics(&self) -> Vec<Nic>; // 名称、地址、网段、MAC、是否 tailnet
    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()>;
    fn keystore(&self) -> &dyn KeyStore; // 主机密钥存取
    /// 系统账户校验器；None = 本平台不支持账户密码配对（04「其他配对方式」）。
    fn system_authenticator(
        &self,
    ) -> Option<std::sync::Arc<dyn crate::sysauth::SystemAuthenticator>> {
        None
    }

    /// 虚拟显示器后端；None = 本平台没有（M3 · V1）。host-core 在 Host 启动时调一次并缓存。
    fn virtual_displays(&self) -> Option<Arc<dyn crate::vdisplay::VirtualDisplayBackend>> {
        None
    }
}

/// 编码器接口，签名来自 docs/dev/06-host.md 平台接口。
pub trait Encoder: Send {
    fn encode(&mut self, frame: CapturedFrame, opts: EncodeOpts) -> Result<()>; // 异步输出到 sink
    fn set_rate(&mut self, kbps: u32, fps: u32);
    fn force_keyframe(&mut self);
    fn set_quality_boost(&mut self, on: bool); // 静止补帧
    /// 能否按区域调 QP（M3 视口 ROI，03 §5.5）。
    fn roi_supported(&self) -> bool {
        false
    }
    /// 替换当前 ROI 列表，对之后编码的帧生效；空列表 = 清除。
    fn set_roi(&mut self, regions: &[RoiRegion]) -> Result<()> {
        let _ = regions;
        Err(PlatformError::Unsupported)
    }
}
