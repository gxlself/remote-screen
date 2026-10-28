//! 码率与 FEC 比例控制。见 docs/dev/03-protocol.md §7。
//!
//! 纯逻辑状态机：不读系统时钟，不碰网络和编码器。调用方把 unix 微秒传进来，
//! 每收到一次 `Feedback`（协议约定 200 ms 一次）调用 [`Controller::on_feedback`]，
//! 没有反馈时用 [`Controller::tick`] 推进「持续 N 秒」类计时。
//!
//! 规则与 §7.2–7.4 的对应关系（阈值都是公开常量，测试直接引用）：
//!
//! | §7 规则 | 常量 |
//! |---|---|
//! | 过载 = 梯度 > 5 ms/s 连续 2 次，或 `frames_lost > 0`，或 `decode_queue > 2` | [`OVERLOAD_GRADIENT_US_PER_S`] / [`OVERLOAD_STREAK`] / [`OVERLOAD_DECODE_QUEUE`] |
//! | 过载 ×0.85，400 ms 内最多一次 | [`OVERLOAD_FACTOR`] / [`OVERLOAD_INTERVAL_US`] |
//! | 空闲 = 梯度 < 1 ms/s 且 loss < 2% 持续 1 s | [`IDLE_GRADIENT_US_PER_S`] / [`IDLE_LOSS`] / [`IDLE_DWELL_US`] |
//! | 空闲 ×1.08，每秒最多一次，且 ≤ `recv_kbps × 1.5` | [`IDLE_FACTOR`] / [`IDLE_INTERVAL_US`] / [`RECV_HEADROOM`] |
//! | 贴着下限（`target ≤ min × 1.0`）超过 2 s 降一级 | [`FLOOR_FACTOR`] / [`FLOOR_DWELL_US`] |
//! | 超过下限 1.5 倍持续 5 s 升一级 | [`RECOVER_FACTOR`] / [`RECOVER_DWELL_US`] |
//! | 公司内网稳定 10 s 后 60 fps | [`INTRANET_STABLE_US`] |
//! | 低电量：「自动」按「流畅」处理，帧率上限 30 | [`LOW_POWER_MAX_FPS`] |
//! | 省流量：固定 30 fps、上限 4 Mbps、×0.75 起 | [`ECO_FPS`] / [`ECO_MAX_KBPS`] |
//!
//! [`Decision::config_changed`] 在帧率或缩放比例变化时为真，调用方据此发新的
//! `StreamConfig`（epoch + 1）；纯码率变化只发 `RateUpdate`，不动 epoch。
//! 第一条决策总是 `config_changed = true`：调用方按「`config_changed` 就发
//! `StreamConfig`」接线时，这条就是 epoch 0 的建流配置，不会漏发。
//!
//! `Feedback.epoch` 由调用方过滤：控制器不持有 epoch，旧 epoch 的反馈会照常
//! 参与计算，Host 侧应在送进来之前丢掉。

use crate::fec;
use crate::pb::{Feedback, Quality, RateUpdate};

/// 过载判定：单程时延斜率阈值（µs/s），对应 §7.2 的 5 ms/s。
pub const OVERLOAD_GRADIENT_US_PER_S: i32 = 5_000;
/// 过载判定：梯度超阈值需要连续出现的次数。
pub const OVERLOAD_STREAK: u32 = 2;
/// 过载判定：待解码帧数超过这个值即过载。
pub const OVERLOAD_DECODE_QUEUE: u32 = 2;
/// 过载下调因子。
pub const OVERLOAD_FACTOR: f32 = 0.85;
/// 两次过载下调之间的最小间隔（400 ms）。
pub const OVERLOAD_INTERVAL_US: u64 = 400_000;

/// 空闲判定：单程时延斜率阈值（µs/s），对应 §7.2 的 1 ms/s。
pub const IDLE_GRADIENT_US_PER_S: i32 = 1_000;
/// 空闲判定：丢包率阈值（2%）。
pub const IDLE_LOSS: f32 = 0.02;
/// 空闲条件需要持续的时间（1 s）。
pub const IDLE_DWELL_US: u64 = 1_000_000;
/// 空闲上调因子。
pub const IDLE_FACTOR: f32 = 1.08;
/// 两次空闲上调之间的最小间隔（1 s）。
pub const IDLE_INTERVAL_US: u64 = 1_000_000;
/// 空闲上调的上限系数：不超过 `recv_kbps × 1.5`。
pub const RECV_HEADROOM: f32 = 1.5;

/// `loss_ewma` 里旧值的权重。
const LOSS_EWMA_DECAY: f32 = 0.8;
/// `loss_ewma` 里本次 `loss` 的权重。
const LOSS_EWMA_GAIN: f32 = 0.2;

/// 贴下限降档的阈值系数：`target ≤ min × FLOOR_FACTOR` 视为「贴着下限」。
pub const FLOOR_FACTOR: f32 = 1.0;
/// 贴着下限持续这么久降一级（2 s）。
pub const FLOOR_DWELL_US: u64 = 2_000_000;
/// 恢复阈值：`target > min × RECOVER_FACTOR` 才开始计时。
pub const RECOVER_FACTOR: f32 = 1.5;
/// 恢复条件需要持续的时间（5 s），到时升一级。
pub const RECOVER_DWELL_US: u64 = 5_000_000;

/// 公司内网：稳定这么久之后帧率上限从 30 升到 60（10 s）。
pub const INTRANET_STABLE_US: u64 = 10_000_000;
/// 公司内网稳定后的帧率上限。
pub const INTRANET_MAX_FPS: u32 = 60;
/// 低电量时的帧率上限。
pub const LOW_POWER_MAX_FPS: u32 = 30;
/// 降档到「帧率」这一级时的帧率。
pub const DEGRADED_FPS: u32 = 30;
/// 省流量档的固定帧率。
pub const ECO_FPS: u32 = 30;
/// 省流量档的码率上限（4 Mbps）。
pub const ECO_MAX_KBPS: u32 = 4_000;

/// 一条连接的路径类型，对应 03 §7.3 表的行。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Path {
    /// 局域网。
    Lan,
    /// 公司内网（UDP）。
    Intranet,
    /// tailnet 直连。
    TailnetDirect,
    /// DERP 中继。
    Derp,
    /// TCP 回退。
    Tcp,
}

impl Path {
    /// `RateUpdate.reason` 里用的路径标签（`"path:<label>"` 的后半段）。
    pub fn label(self) -> &'static str {
        match self {
            Path::Lan => "lan",
            Path::Intranet => "intranet",
            Path::TailnetDirect => "tailnet",
            Path::Derp => "derp",
            Path::Tcp => "tcp",
        }
    }

    /// 没有别的变化原因时，`RateUpdate.reason` 用的路径标签。
    pub fn reason(self) -> String {
        format!("path:{}", self.label())
    }
}

/// 一条路径的起点和边界，见 03 §7.3 表。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Bounds {
    /// 起始码率（kbps）。
    pub start_kbps: u32,
    /// 码率下限（kbps）。
    pub min_kbps: u32,
    /// 码率上限（kbps）。
    pub max_kbps: u32,
    /// 最高帧率。
    pub max_fps: u32,
    /// 最大分辨率高度；`None` 表示原生。
    pub max_height: Option<u32>,
}

/// 按 03 §7.3 表给出路径的起点和边界。
///
/// `high_refresh_client` 为真时局域网按 iPad Pro 的 120 Hz 给帧率上限，
/// 其余路径不受影响。
pub fn bounds_for(path: Path, high_refresh_client: bool) -> Bounds {
    match path {
        Path::Lan => Bounds {
            start_kbps: 20_000,
            min_kbps: 4_000,
            max_kbps: 40_000,
            max_fps: if high_refresh_client { 120 } else { 60 },
            max_height: None,
        },
        Path::Intranet => Bounds {
            start_kbps: 4_000,
            min_kbps: 2_000,
            max_kbps: 10_000,
            max_fps: 30,
            max_height: Some(1440),
        },
        Path::TailnetDirect => Bounds {
            start_kbps: 8_000,
            min_kbps: 2_000,
            max_kbps: 15_000,
            max_fps: 60,
            max_height: Some(1440),
        },
        Path::Derp => Bounds {
            start_kbps: 3_000,
            min_kbps: 1_500,
            max_kbps: 5_000,
            max_fps: 30,
            max_height: Some(1080),
        },
        Path::Tcp => Bounds {
            start_kbps: 4_000,
            min_kbps: 2_000,
            max_kbps: 6_000,
            max_fps: 30,
            max_height: Some(1080),
        },
    }
}

/// 一次控制决策。
///
/// `rate_update` 只在参数真的变化时才有值；`config_changed` 为真时调用方要发
/// 新的 `StreamConfig`（epoch + 1）。
#[derive(Debug, Clone, PartialEq)]
pub struct Decision {
    /// 目标码率，含 FEC 开销（kbps）。
    pub target_kbps: u32,
    /// 编码器码率 `target / (1 + r)`（kbps）。
    pub encoder_kbps: u32,
    /// 帧率。
    pub fps: u32,
    /// 缩放比例（1.0 / 0.75 / 0.5）。
    pub scale: f32,
    /// FEC 冗余比例 r。
    pub fec_ratio: f32,
    /// 当前降档级数，0 表示满档。
    pub level: u8,
    /// 是否需要新的 `StreamConfig`（epoch + 1）。
    pub config_changed: bool,
    /// 参数变化时要发给 App 的 `RateUpdate`。
    pub rate_update: Option<RateUpdate>,
}

/// 上一次发出去的参数，用来判断「参数真的变化」。
#[derive(Debug, Clone, Copy, PartialEq)]
struct Emitted {
    target_kbps: u32,
    fps: u32,
    scale: f32,
    fec_ratio_pct: u32,
}

/// 每条连接的码率控制器。见 03 §7。
///
/// 时间只来自传入的 `now_us`（unix 微秒），同一个控制器可以重放任意时间线。
#[derive(Debug, Clone)]
pub struct Controller {
    path: Path,
    bounds: Bounds,
    quality: Quality,
    low_power: bool,
    native: (u32, u32),
    target_kbps: f32,
    loss_ewma: f32,
    gradient_streak: u32,
    idle_since: Option<u64>,
    floor_since: Option<u64>,
    recover_since: Option<u64>,
    last_decrease_us: Option<u64>,
    last_increase_us: Option<u64>,
    last_overload_us: Option<u64>,
    created_us: u64,
    intranet_fps_boosted: bool,
    level: u8,
    last_config: Option<(u32, f32)>,
    last_emitted: Option<Emitted>,
}

impl Controller {
    /// 新建控制器。`requested_start_kbps` 是 `StartStream.bitrate_kbps`，
    /// 0 表示按路径起点；两者都会被夹到 `[min, max]`。`native` 是原生分辨率
    /// `(宽, 高)`，只用于 [`Controller::stream_size`]。
    pub fn new(
        path: Path,
        quality: Quality,
        requested_start_kbps: u32,
        native: (u32, u32),
        now_us: u64,
    ) -> Self {
        Self::new_with_high_refresh(path, quality, requested_start_kbps, native, false, now_us)
    }

    /// 与 [`Controller::new`] 相同，但可以声明客户端支持高刷新率
    /// （局域网 + iPad Pro 时帧率上限按 120 算）。
    pub fn new_with_high_refresh(
        path: Path,
        quality: Quality,
        requested_start_kbps: u32,
        native: (u32, u32),
        high_refresh_client: bool,
        now_us: u64,
    ) -> Self {
        let bounds = bounds_for(path, high_refresh_client);
        let mut controller = Self {
            path,
            bounds,
            quality,
            low_power: false,
            native,
            target_kbps: 0.0,
            loss_ewma: 0.0,
            gradient_streak: 0,
            idle_since: None,
            floor_since: None,
            recover_since: None,
            last_decrease_us: None,
            last_increase_us: None,
            last_overload_us: None,
            created_us: now_us,
            intranet_fps_boosted: false,
            level: 0,
            last_config: None,
            last_emitted: None,
        };
        let start = if requested_start_kbps == 0 {
            bounds.start_kbps
        } else {
            requested_start_kbps
        };
        controller.target_kbps = start as f32;
        controller.clamp_target();
        controller
    }

    /// 路径的起点和边界。
    pub fn bounds(&self) -> Bounds {
        self.bounds
    }

    /// 当前目标码率（kbps）。
    pub fn target_kbps(&self) -> u32 {
        self.target_kbps.round().max(0.0) as u32
    }

    /// 当前 `loss_ewma`。
    pub fn loss_ewma(&self) -> f32 {
        self.loss_ewma
    }

    /// 当前降档级数。
    pub fn level(&self) -> u8 {
        self.level
    }

    /// 当前档位下的编码尺寸：原生分辨率先按路径的 `max_height` 等比收窄，
    /// 再乘当前缩放比例，宽高都取偶数（编码器宏块对齐）。
    pub fn stream_size(&self) -> (u32, u32) {
        let (native_w, native_h) = self.native;
        if native_w == 0 || native_h == 0 {
            return (0, 0);
        }
        let height = match self.bounds.max_height {
            Some(max) if max > 0 => native_h.min(max),
            _ => native_h,
        };
        let width = (u64::from(native_w) * u64::from(height) / u64::from(native_h)) as u32;
        let scale = self.step_params().1;
        (even(width as f32 * scale), even(height as f32 * scale))
    }

    /// 收到一次 `Feedback`。`now_us` 是 unix 微秒。
    pub fn on_feedback(&mut self, fb: &Feedback, now_us: u64) -> Decision {
        self.step(now_us, Some(fb), None)
    }

    /// 切换画质档。降档级数保留（超出新档位上限时截断），贴下限 / 恢复计时重置。
    pub fn set_quality(&mut self, q: Quality, now_us: u64) -> Decision {
        self.quality = q;
        self.level = self.level.min(self.max_level());
        self.floor_since = None;
        self.recover_since = None;
        self.step(now_us, None, Some("quality"))
    }

    /// 没有 `Feedback` 时推进「持续 N 秒」类计时（贴下限降档、恢复、内网升帧率）。
    /// 每次调用最多升 / 降一级。
    pub fn tick(&mut self, now_us: u64) -> Decision {
        self.step(now_us, None, None)
    }

    fn step(&mut self, now_us: u64, fb: Option<&Feedback>, hint: Option<&'static str>) -> Decision {
        let mut reason = hint;

        if let Some(fb) = fb {
            self.low_power = fb.low_power;
            let loss = shard_loss(fb);
            self.loss_ewma = LOSS_EWMA_DECAY * self.loss_ewma + LOSS_EWMA_GAIN * loss;
            if !self.loss_ewma.is_finite() {
                self.loss_ewma = 0.0;
            }

            let gradient = fb.delay_gradient_us_per_s;
            if gradient > OVERLOAD_GRADIENT_US_PER_S {
                self.gradient_streak = self.gradient_streak.saturating_add(1);
            } else {
                self.gradient_streak = 0;
            }

            let overload = self.gradient_streak >= OVERLOAD_STREAK
                || fb.frames_lost > 0
                || fb.decode_queue > OVERLOAD_DECODE_QUEUE;

            if overload {
                self.last_overload_us = Some(now_us);
                self.idle_since = None;
                if can_adjust(self.last_decrease_us, now_us, OVERLOAD_INTERVAL_US) {
                    self.target_kbps *= OVERLOAD_FACTOR;
                    self.last_decrease_us = Some(now_us);
                    reason = Some(overload_reason(fb, self.gradient_streak));
                }
            } else if gradient < IDLE_GRADIENT_US_PER_S && loss < IDLE_LOSS {
                let since = *self.idle_since.get_or_insert(now_us);
                if now_us.saturating_sub(since) >= IDLE_DWELL_US
                    && can_adjust(self.last_increase_us, now_us, IDLE_INTERVAL_US)
                {
                    // 按 §7.2 的字面意思取 `min(target × 1.08, recv_kbps × 1.5)`：
                    // 接收端报的数字低说明链路吃不下更多，这时宁可停在这里。
                    // 真要是发得太多，丢包会先触发过载分支，轮不到这里。
                    let cap = fb.recv_kbps as f32 * RECV_HEADROOM;
                    self.target_kbps = (self.target_kbps * IDLE_FACTOR).min(cap);
                    self.last_increase_us = Some(now_us);
                    self.idle_since = Some(now_us);
                    reason = Some("idle");
                }
            } else {
                self.idle_since = None;
            }
        }

        self.clamp_target();

        // 贴着下限超过 2 s 降一级；超过下限 1.5 倍持续 5 s 升一级。
        let min_kbps = self.bounds.min_kbps as f32;
        if self.target_kbps <= min_kbps * FLOOR_FACTOR {
            self.recover_since = None;
            let since = *self.floor_since.get_or_insert(now_us);
            if self.level < self.max_level() && now_us.saturating_sub(since) >= FLOOR_DWELL_US {
                self.level += 1;
                self.floor_since = Some(now_us);
                reason = Some("floor");
            }
        } else {
            self.floor_since = None;
            if self.level > 0 && self.target_kbps > min_kbps * RECOVER_FACTOR {
                let since = *self.recover_since.get_or_insert(now_us);
                if now_us.saturating_sub(since) >= RECOVER_DWELL_US {
                    self.level -= 1;
                    self.recover_since = Some(now_us);
                    reason = Some("recover");
                }
            } else {
                self.recover_since = None;
            }
        }

        // 公司内网：距上次过载满 10 s 就把帧率上限升到 60，升上去不再回落。
        if self.path == Path::Intranet && !self.intranet_fps_boosted {
            let stable_from = self.last_overload_us.unwrap_or(self.created_us);
            if now_us.saturating_sub(stable_from) >= INTRANET_STABLE_US {
                self.intranet_fps_boosted = true;
                reason = Some("path:intranet");
            }
        }

        let (fps, scale) = self.step_params();
        let r = fec::redundancy_ratio(self.loss_ewma);
        let target_kbps = self.target_kbps();
        let encoder_kbps = (target_kbps as f32 / (1.0 + r)).round().max(1.0) as u32;
        let fec_ratio_pct = (r * 100.0).round() as u32;

        let config_changed = self.last_config != Some((fps, scale));
        self.last_config = Some((fps, scale));

        let emitted = Emitted {
            target_kbps,
            fps,
            scale,
            fec_ratio_pct,
        };
        let rate_update = if self.last_emitted == Some(emitted) {
            None
        } else {
            self.last_emitted = Some(emitted);
            Some(RateUpdate {
                target_kbps,
                fps,
                scale,
                fec_ratio_pct,
                reason: reason.map_or_else(|| self.path.reason(), str::to_owned),
            })
        };

        Decision {
            target_kbps,
            encoder_kbps,
            fps,
            scale,
            fec_ratio: r,
            level: self.level,
            config_changed,
            rate_update,
        }
    }

    /// 当前档位的帧率和缩放比例。
    fn step_params(&self) -> (u32, f32) {
        let level = self.level.min(self.max_level());
        let max_fps = self.max_fps();
        let (fps, scale) = match self.effective_quality() {
            // 省流量：固定 30 fps、上限 4 Mbps、×0.75 起（表里只有第 1 级）。
            Quality::Eco => (ECO_FPS.min(max_fps), if level == 0 { 1.0 } else { 0.75 }),
            // 流畅：×0.75 → ×0.5 → 30 fps。
            Quality::Smooth => match level {
                0 => (max_fps, 1.0),
                1 => (max_fps, 0.75),
                2 => (max_fps, 0.5),
                _ => (DEGRADED_FPS.min(max_fps), 0.5),
            },
            // 自动 / 清晰：30 fps → ×0.75 → ×0.5。
            Quality::Auto | Quality::Clear => match level {
                0 => (max_fps, 1.0),
                1 => (DEGRADED_FPS.min(max_fps), 1.0),
                2 => (DEGRADED_FPS.min(max_fps), 0.75),
                _ => (DEGRADED_FPS.min(max_fps), 0.5),
            },
        };
        (fps.min(max_fps), scale)
    }

    /// 低电量时「自动」按「流畅」处理。
    fn effective_quality(&self) -> Quality {
        if self.low_power && self.quality == Quality::Auto {
            Quality::Smooth
        } else {
            self.quality
        }
    }

    /// 当前画质档的降档级数上限。
    fn max_level(&self) -> u8 {
        match self.effective_quality() {
            Quality::Eco => 1,
            _ => 3,
        }
    }

    fn max_fps(&self) -> u32 {
        let mut max_fps = self.bounds.max_fps;
        if self.path == Path::Intranet && self.intranet_fps_boosted {
            max_fps = max_fps.max(INTRANET_MAX_FPS);
        }
        if self.low_power {
            max_fps = max_fps.min(LOW_POWER_MAX_FPS);
        }
        max_fps
    }

    fn max_kbps(&self) -> u32 {
        let mut max_kbps = self.bounds.max_kbps;
        if self.effective_quality() == Quality::Eco {
            max_kbps = max_kbps.min(ECO_MAX_KBPS);
        }
        max_kbps
    }

    fn clamp_target(&mut self) {
        let min_kbps = self.bounds.min_kbps as f32;
        let max_kbps = self.max_kbps().max(self.bounds.min_kbps) as f32;
        self.target_kbps = self.target_kbps.clamp(min_kbps, max_kbps);
    }
}

/// §7.2 的 `loss = shards_lost / (shards_received + shards_lost)`。
fn shard_loss(fb: &Feedback) -> f32 {
    let total = u64::from(fb.shards_received) + u64::from(fb.shards_lost);
    if total == 0 {
        0.0
    } else {
        fb.shards_lost as f32 / total as f32
    }
}

/// 过载的原因，用于 `RateUpdate.reason`。
fn overload_reason(fb: &Feedback, streak: u32) -> &'static str {
    if fb.frames_lost > 0 {
        "loss"
    } else if fb.decode_queue > OVERLOAD_DECODE_QUEUE {
        "queue"
    } else if streak >= OVERLOAD_STREAK {
        "delay"
    } else {
        "loss"
    }
}

fn can_adjust(last_us: Option<u64>, now_us: u64, interval_us: u64) -> bool {
    last_us.is_none_or(|last| now_us.saturating_sub(last) >= interval_us)
}

/// 取偶数（编码器宏块对齐），最小 2。
fn even(value: f32) -> u32 {
    let rounded = value.round().max(2.0) as u32;
    rounded & !1
}
