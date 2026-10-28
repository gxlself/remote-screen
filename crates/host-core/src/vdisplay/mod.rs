//! 虚拟显示器（M3 · ）：平台后端接口与共用类型。
//! 纯逻辑在 mode（模式选择）、arrange（排列）、manager（生命周期）里，都能在 Linux 上测。

pub mod arrange;
pub mod manager;
pub mod mode;

/// 一块虚拟屏的模式。width_px / height_px 是像素，保持请求的横竖。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VdMode {
    pub width_px: u32,
    pub height_px: u32,
    /// 1.0 / 2.0（Mac HiDPI）；Windows 只作提示。
    pub scale: f32,
    pub refresh_hz: u32,
}

/// 平台能力。尺寸按长边 / 短边给，与横竖无关。
#[derive(Debug, Clone, PartialEq)]
pub struct VdCaps {
    pub max_long_px: u32,
    pub max_short_px: u32,
    pub min_long_px: u32,
    pub min_short_px: u32,
    /// 升序、去重，例如 [30, 60]。
    pub refresh_rates: Vec<u32>,
    /// 升序，例如 [1.0, 2.0]。
    pub scales: Vec<f32>,
    /// 宽高都取它的倍数（≥ 2）。
    pub align_px: u32,
    /// 平台同时最多几块。
    pub max_displays: u32,
}

/// 平台建好的一块虚拟屏。
#[derive(Debug, Clone, PartialEq)]
pub struct VdCreated {
    /// 后端内唯一，destroy 用。
    pub token: u64,
    /// 已知时给平台显示器 id（= DisplayInfo.id）。Mac 必填。
    pub platform_id: Option<u32>,
    /// platform_id 为 None 时，按 DisplayInfo.name 精确匹配认领（Windows："RS Virtual 01"）。
    pub match_name: Option<String>,
    /// 实际生效的模式。
    pub mode: VdMode,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VdError {
    #[error("virtual displays unsupported: {0}")]
    Unsupported(String),
    #[error("virtual display limit reached")]
    Limit,
    #[error("virtual display failed: {0}")]
    Failed(String),
}

/// 平台后端。create / destroy 可能阻塞数秒，host-core 在 spawn_blocking 里调用；
/// caps / cleanup_stale 必须在百毫秒内返回：Host 启动时同步调用一次，保证首个 ServerHello 就带上功能位。
pub trait VirtualDisplayBackend: Send + Sync + 'static {
    /// None = 本机现在造不了（私有 API 不在、驱动没装或设备没启动）。
    fn caps(&self) -> Option<VdCaps>;
    /// 按 mode 造一块。label 是给人看的名字（Mac 用作显示器名，Windows 忽略）。
    fn create(&self, mode: VdMode, label: &str) -> Result<VdCreated, VdError>;
    /// 销毁。token 不存在时返回 Ok（幂等）。
    fn destroy(&self, token: u64) -> Result<(), VdError>;
    /// Host 启动时调一次：清掉上次进程留下的虚拟屏，返回清掉几块。默认什么也不做。
    fn cleanup_stale(&self) -> usize {
        0
    }
}
