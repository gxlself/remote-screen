//! 协议版本与功能位。见 docs/dev/03-protocol.md §11。

/// M1 控制面协议版本；保留 crate 根部的 M0 `u16` 常量。
pub const PROTOCOL_VERSION: u32 = 1;

/// 视频 Reed-Solomon；首个版本 0.3.0。
pub const FEC_RS: u64 = 1 << 0;

/// cursor 流；首个版本 0.3.0。
pub const CURSOR_SHAPE: u64 = 1 << 1;

/// 指针数据报；首个版本 0.3.0。
pub const POINTER_DGRAM: u64 = 1 << 2;

/// 文本剪贴板；首个版本 0.3.0。
pub const CLIPBOARD_TEXT: u64 = 1 << 3;

/// 图片剪贴板；首个版本 0.4.0。
pub const CLIPBOARD_IMAGE: u64 = 1 << 4;

/// 多显示器选择；首个版本 0.3.0。
pub const MULTI_DISPLAY: u64 = 1 << 5;

/// HID 键盘事件；首个版本 0.3.0。
pub const KEY_HID: u64 = 1 << 6;

/// Text 事件；首个版本 0.3.0。
pub const TEXT_INPUT: u64 = 1 << 7;

/// TCP 回退；首个版本 0.3.0。
pub const TCP_TRANSPORT: u64 = 1 << 8;

/// 会话恢复；首个版本 0.3.0。
pub const RESUME: u64 = 1 << 9;

/// 观看者与接管；首个版本 0.3.0。
pub const MULTI_VIEWER: u64 = 1 << 10;

/// 静止补帧；首个版本 0.3.0。
pub const HQ_REFRESH: u64 = 1 << 11;

/// Feedback / RateUpdate；首个版本 0.3.0。
pub const RATE_FEEDBACK: u64 = 1 << 12;

/// 锁屏、结束后锁屏等；首个版本 0.3.0。
pub const SESSION_ACTIONS: u64 = 1 << 13;

/// 「内网连接每次确认」；首个版本 0.3.2。
pub const INTRANET_CONFIRM_EACH: u64 = 1 << 14;

/// 声音；首个版本 0.4.0。
pub const AUDIO_OPUS: u64 = 1 << 15;

/// 文件；首个版本 0.4.0。
pub const FILE_TRANSFER: u64 = 1 << 16;

/// Windows 原生触控；首个版本 0.4.0。
pub const TOUCH_NATIVE: u64 = 1 << 17;

/// SAS；首个版本 0.4.0。
pub const CTRL_ALT_DEL: u64 = 1 << 18;

/// 认识 `PairPassword`（账户密码配对；与开关是否打开无关）；首个版本 0.4.0。
pub const PAIR_PASSWORD: u64 = 1 << 19;

/// 认识 `PairStart.pair_file_id`（配对文件）；首个版本 0.4.0。
pub const PAIR_FILE: u64 = 1 << 20;

/// File browsing and pull: FileList / FileRequest / FileQuery / FileStatus / FileCancel,
/// FileHeader.dest_dir, and Intent FILES. Implies FILE_TRANSFER.
pub const FILE_BROWSE: u64 = 1 << 21;

/// 视口感知编码：Viewport / ViewportAck、StreamConfig.crop（03 §5.5）；首个版本 0.5.0。
pub const VIEWPORT: u64 = 1 << 23;

/// Host 能新建虚拟显示器（CreateVirtualDisplay / VirtualDisplayResult / ReleaseVirtualDisplay、Display.is_virtual）。
/// 只在平台后端可用时声明；首个版本 0.5.0。
pub const VIRTUAL_DISPLAY: u64 = 1 << 22;

/// App 本地录制时告知 Host：RecordingNotice；Host 在胶囊显示「录制中」并写审计 `recording`（M3 V5）。
pub const RECORDING_NOTICE: u64 = 1 << 24;
