//! 编码策略纯状态机：静止补帧、关键帧请求合并与编码格式兜底（03 §5.3、§5.4，06「采集与编码」）。

use rsp_core::pb;

/// 画面静止多久后补帧（03 §5.4）。
pub const STILL_IDLE_US: u64 = 300_000;
/// 两次静止补帧之间的最短间隔（03 §5.4）。
pub const STILL_COOLDOWN_US: u64 = 1_000_000;
/// 关键帧请求的最小响应间隔（03 §5.3）。
pub const KEYFRAME_MIN_INTERVAL_US: u64 = 300_000;
/// Repeated local reference-chain repairs back off after the first two responses.
pub const KEYFRAME_MAX_INTERVAL_US: u64 = 2_000_000;
/// A quiet recovery window restores the ordinary 300 ms response interval.
pub const KEYFRAME_RECOVERY_US: u64 = 2_000_000;

/// 静止补帧状态机，只判断时机而不读取时钟或操作编码器（03 §5.4、06「采集与编码」）。
#[derive(Debug, Clone)]
pub struct StillRefiner {
    enabled: bool,
    last_frame_us: Option<u64>,
    refined_last_frame: bool,
    last_refine_us: Option<u64>,
}

impl StillRefiner {
    /// 创建静止补帧状态机；`enabled` 对应客户端是否声明 `HQ_REFRESH`（03 §11、§5.4）。
    pub fn new(enabled: bool) -> Self {
        Self {
            enabled,
            last_frame_us: None,
            refined_last_frame: false,
            last_refine_us: None,
        }
    }

    /// 开关静止补帧，客户端未声明 `HQ_REFRESH` 时应关闭（03 §11、§5.4）。
    pub fn set_enabled(&mut self, on: bool) {
        self.enabled = on;
    }

    /// 记录一帧新的采集画面，并允许这帧静止后补帧一次（03 §5.4）。
    pub fn on_frame(&mut self, now_us: u64) {
        self.last_frame_us = Some(now_us);
        self.refined_last_frame = false;
    }

    /// 到点且冷却结束后返回一次 `true`，表示应把最后一帧以高质量重编（03 §5.4、06「采集与编码」）。
    pub fn poll(&mut self, now_us: u64) -> bool {
        if !self.enabled
            || self.refined_last_frame
            || self
                .last_frame_us
                .is_none_or(|last| now_us.saturating_sub(last) < STILL_IDLE_US)
            || self
                .last_refine_us
                .is_some_and(|last| now_us.saturating_sub(last) < STILL_COOLDOWN_US)
        {
            return false;
        }
        self.refined_last_frame = true;
        self.last_refine_us = Some(now_us);
        true
    }

    /// 返回下一次 [`Self::poll`] 可能为 `true` 的时刻；没有待办时返回 `None`（03 §5.4）。
    pub fn deadline(&self) -> Option<u64> {
        if !self.enabled || self.refined_last_frame {
            return None;
        }
        let idle_at = self.last_frame_us?.saturating_add(STILL_IDLE_US);
        let cooldown_at = self
            .last_refine_us
            .map(|last| last.saturating_add(STILL_COOLDOWN_US));
        Some(cooldown_at.map_or(idle_at, |cooldown| idle_at.max(cooldown)))
    }

    /// 清除补帧计时与最后一帧，保留启用开关；用于重建管线、暂停推流或 epoch 变化（03 §5.4）。
    pub fn reset(&mut self) {
        self.last_frame_us = None;
        self.refined_last_frame = false;
        self.last_refine_us = None;
    }
}

/// 关键帧请求限流与合并状态机（03 §5.3）。
#[derive(Debug, Clone, Default)]
pub struct KeyframeGate {
    last_response_us: Option<u64>,
    pending: bool,
    in_flight: bool,
    adaptive: bool,
    responses: u32,
    last_request_us: Option<u64>,
}

impl KeyframeGate {
    /// Opt in to sustained-request backoff. The default retains the legacy API's
    /// fixed cooldown; a new pipeline gets a fresh adaptive recovery window.
    pub fn with_backoff() -> Self {
        Self {
            adaptive: true,
            ..Self::default()
        }
    }

    /// Local reference-chain failures may back off; remote loss requests on an
    /// otherwise draining pipeline retain the ordinary recovery cadence.
    pub fn set_backoff(&mut self, enabled: bool) {
        if self.adaptive != enabled {
            self.responses = 0;
        }
        self.adaptive = enabled;
    }

    pub fn interval_us(&self) -> u64 {
        if !self.adaptive {
            return KEYFRAME_MIN_INTERVAL_US;
        }
        (KEYFRAME_MIN_INTERVAL_US << self.responses.saturating_sub(1).min(3))
            .min(KEYFRAME_MAX_INTERVAL_US)
    }

    fn note_request(&mut self, now_us: u64) {
        if !self.in_flight
            && !self.pending
            && self
                .last_request_us
                .is_none_or(|last| now_us.saturating_sub(last) >= KEYFRAME_RECOVERY_US)
        {
            self.responses = 0;
        }
        self.last_request_us = Some(now_us);
    }

    /// Merge requests during cooldown or while the response is being sent.
    /// The first response after a quiet period keeps the ordinary 300 ms bound.
    pub fn request(&mut self, now_us: u64) -> bool {
        self.note_request(now_us);
        self.pending = true;
        self.poll(now_us)
    }

    /// Queue a reference-chain repair until the caller has drained its backlog.
    pub fn defer(&mut self) {
        self.pending = true;
    }

    pub fn pending(&self) -> bool {
        self.pending
    }

    /// Call only after actually asking the encoder for a keyframe. No further
    /// response may be forced before the packetizer finishes this keyframe.
    pub fn mark_in_flight(&mut self) {
        self.in_flight = true;
    }

    /// A synchronous encode failure produced no response; retain one repair.
    pub fn encode_failed(&mut self, now_us: u64) {
        self.set_backoff(true);
        self.note_request(now_us);
        self.in_flight = false;
        self.pending = true;
    }

    /// Return at most one response for all pending requests.
    pub fn poll(&mut self, now_us: u64) -> bool {
        if !self.pending || self.in_flight {
            return false;
        }
        if self
            .last_response_us
            .is_some_and(|last| now_us.saturating_sub(last) < self.interval_us())
        {
            return false;
        }
        self.last_response_us = Some(now_us);
        self.responses = self.responses.saturating_add(1);
        self.pending = false;
        true
    }

    /// No timer is armed while the encoder/packetizer owns the response.
    pub fn deadline(&self) -> Option<u64> {
        if !self.pending || self.in_flight {
            return None;
        }
        Some(
            self.last_response_us
                .map_or(0, |last| last.saturating_add(self.interval_us())),
        )
    }

    /// Account for an emitted keyframe, including periodic and opening frames.
    pub fn on_keyframe(&mut self, now_us: u64) {
        self.last_response_us = Some(now_us);
        self.pending = false;
        self.in_flight = false;
    }

    /// Finish a requested response, preserving one trailing merged request.
    pub fn on_response(&mut self, now_us: u64) {
        let pending = self.pending;
        self.on_keyframe(now_us);
        self.pending = pending;
    }

    pub fn reset(&mut self) {
        *self = Self {
            adaptive: self.adaptive,
            ..Self::default()
        };
    }
}

/// 按 App 声明顺序去重并过滤无效项，得到编码格式候选；空列表按 M0 约定回退到 HEVC（03 §3）。
pub fn codec_order(app_decode: &[pb::Codec]) -> Vec<pb::Codec> {
    let mut order = Vec::with_capacity(app_decode.len());
    for &codec in app_decode {
        if matches!(codec, pb::Codec::Hevc | pb::Codec::H264) && !order.contains(&codec) {
            order.push(codec);
        }
    }
    if order.is_empty() {
        order.push(pb::Codec::Hevc);
    }
    order
}

/// 按候选顺序选择编码格式，并在平台编码器不支持当前格式时回退（03 §3、06「H.264 兜底」）。
#[derive(Debug, Clone)]
pub struct CodecFallback {
    order: Vec<pb::Codec>,
    current: usize,
}

impl CodecFallback {
    /// 从 App 偏好顺序构建候选，并移除调用方禁用的格式（03 §3、06「H.264 兜底」）。
    pub fn new(app_decode: &[pb::Codec], disabled: &[pb::Codec]) -> Self {
        let order = codec_order(app_decode)
            .into_iter()
            .filter(|codec| !disabled.contains(codec))
            .collect();
        Self { order, current: 0 }
    }

    /// 返回当前应尝试的编码格式；没有可选项时返回 `None`（03 §3、06「H.264 兜底」）。
    pub fn current(&self) -> Option<pb::Codec> {
        self.order.get(self.current).copied()
    }

    /// 当前格式建编码器失败时切到下一候选；候选耗尽后返回 `None`（06「H.264 兜底」）。
    pub fn on_unsupported(&mut self) -> Option<pb::Codec> {
        if self.current < self.order.len() {
            self.current += 1;
        }
        self.current()
    }
}
