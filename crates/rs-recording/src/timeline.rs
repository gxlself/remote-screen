//! 录制与回放的时间线：媒体时钟、跳转索引、节拍器和 epoch 重映射。
//! 都不读系统时钟，时间一律由调用方传入。

use crate::record::GapReason;

pub const GAP_THRESHOLD_US: u64 = 3_000_000;
pub const GAP_COMPRESSED_US: u64 = 100_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GapMark {
    pub reason: GapReason,
    pub real_us: u64,
}

/// 把本机单调时间换算成录制的媒体时间；超过 3 秒的空档压缩成 100 ms。
#[derive(Debug, Default)]
pub struct MediaClock {
    last_real: Option<u64>,
    media: u64,
    pending: Option<GapReason>,
}

impl MediaClock {
    pub fn new() -> Self {
        Self::default()
    }

    /// real_us：调用方的单调时间。
    /// 返回（这一条记录的媒体时间，需要先写入的 Gap）。
    pub fn map(&mut self, real_us: u64) -> (u64, Option<GapMark>) {
        let Some(last_real) = self.last_real.replace(real_us) else {
            return (self.media, None);
        };
        let delta = real_us.saturating_sub(last_real);
        if delta <= GAP_THRESHOLD_US {
            self.media = self.media.saturating_add(delta);
            self.pending = None;
            return (self.media, None);
        }
        self.media = self.media.saturating_add(GAP_COMPRESSED_US);
        let reason = self.pending.take().unwrap_or(GapReason::Clock);
        (
            self.media,
            Some(GapMark {
                reason,
                real_us: delta,
            }),
        )
    }

    /// 下一个空档的原因记为 Paused（会话进后台）。
    pub fn mark_paused(&mut self) {
        self.pending = Some(GapReason::Paused);
    }

    /// 下一个空档的原因记为 Reconnect。
    pub fn mark_reconnect(&mut self) {
        self.pending = Some(GapReason::Reconnect);
    }

    /// 最近一次 map 的结果。
    pub fn media_now(&self) -> u64 {
        self.media
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SeekPoint {
    pub chunk: usize,
    pub t_us: u64,
}

/// 回放的跳转点：每个关键帧块一个，按媒体时间排好。
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct SeekIndex {
    points: Vec<SeekPoint>,
    duration_us: u64,
}

impl SeekIndex {
    /// 输入：（普通块序号, keyframe_start, t_start_us）。只收 keyframe_start 的块；t 不单调时丢掉倒退的点。
    pub fn from_chunks(
        chunks: impl IntoIterator<Item = (usize, bool, u64)>,
        duration_us: u64,
    ) -> Self {
        let mut points: Vec<SeekPoint> = Vec::new();
        for (chunk, keyframe_start, t_us) in chunks {
            if !keyframe_start || points.last().is_some_and(|last| t_us < last.t_us) {
                continue;
            }
            points.push(SeekPoint { chunk, t_us });
        }
        Self {
            points,
            duration_us,
        }
    }

    /// t_us <= target 的最后一个点。target 早于第一个点时返回第一个点；没有点时返回 None。
    /// target 大于 duration 时按 duration 处理。
    pub fn locate(&self, target_us: u64) -> Option<SeekPoint> {
        let target_us = target_us.min(self.duration_us);
        let after = self.points.partition_point(|p| p.t_us <= target_us);
        self.points.get(after.saturating_sub(1)).copied()
    }

    pub fn points(&self) -> &[SeekPoint] {
        &self.points
    }

    pub fn duration_us(&self) -> u64 {
        self.duration_us
    }
}

/// 回放的播放、暂停和跳转：在媒体时间和调用方的真实时间之间换算。
#[derive(Debug, Default)]
pub struct Pacer {
    playing: bool,
    anchor_media: u64,
    anchor_real: u64,
}

impl Pacer {
    /// 初始状态：暂停在媒体时间 0。
    pub fn new() -> Self {
        Self::default()
    }

    pub fn play(&mut self, now_real_us: u64) {
        if self.playing {
            return;
        }
        self.playing = true;
        self.anchor_real = now_real_us;
    }

    pub fn pause(&mut self, now_real_us: u64) {
        if !self.playing {
            return;
        }
        self.anchor_media = self.position(now_real_us);
        self.playing = false;
    }

    /// 播放或暂停的状态不变，位置改到 media_us。
    pub fn seek(&mut self, media_us: u64, now_real_us: u64) {
        self.anchor_media = media_us;
        self.anchor_real = now_real_us;
    }

    pub fn is_playing(&self) -> bool {
        self.playing
    }

    /// 播放中返回 anchor_media + (now - anchor_real)；暂停时返回暂停的位置。
    pub fn position(&self, now_real_us: u64) -> u64 {
        if !self.playing {
            return self.anchor_media;
        }
        self.anchor_media
            .saturating_add(now_real_us.saturating_sub(self.anchor_real))
    }

    /// 媒体时间 t 应该在哪个真实时刻送出。暂停时返回 None；t 早于当前位置时，返回的时刻 <= now。
    pub fn due_at(&self, media_us: u64) -> Option<u64> {
        if !self.playing {
            return None;
        }
        Some(if media_us >= self.anchor_media {
            self.anchor_real
                .saturating_add(media_us - self.anchor_media)
        } else {
            self.anchor_real
                .saturating_sub(self.anchor_media - media_us)
        })
    }
}

/// 回放送给下游的 epoch：每次跳转、每次录制里的配置变化都换新，让原生解码器重置并等关键帧。
#[derive(Debug, Default)]
pub struct EpochRemap {
    out: u32,
    recorded: Option<u32>,
    reset: bool,
}

impl EpochRemap {
    /// 输出 epoch 从 1 开始（0 留给「从没配置过」）。
    pub fn new() -> Self {
        Self::default()
    }

    /// 从指定的输出 epoch 开始（下一次换新时输出 start + 1）；给回绕测试用，也可以在生产代码里用。
    pub fn with_start(start: u32) -> Self {
        Self {
            out: start,
            ..Self::default()
        }
    }

    /// 跳转之后调用。下一次 on_config 一定换新 epoch。
    pub fn on_seek(&mut self) {
        self.reset = true;
    }

    /// 送出一条 StreamConfig 之前调用，返回要写进这条配置的 epoch。
    /// 录制里的 epoch 和上一次不同，或者调用过 on_seek，或者这是第一次调用：输出 epoch + 1；否则保持不变。
    /// 输出 epoch 到 u32::MAX 后回绕到 1。
    pub fn on_config(&mut self, recorded_epoch: u32) -> u32 {
        if self.reset || self.recorded != Some(recorded_epoch) {
            self.out = self.out.checked_add(1).unwrap_or(1);
            self.reset = false;
        }
        self.recorded = Some(recorded_epoch);
        self.out
    }

    /// 送出视频帧时用的 epoch：当前输出 epoch 的低 8 位。
    pub fn frame_epoch(&self) -> u8 {
        self.out as u8
    }

    /// 录制里这一帧的 epoch 和当前配置的录制 epoch 低 8 位不一致时返回 false，调用方应丢掉这一帧。
    /// 还没有配置时也返回 false。
    pub fn frame_matches(&self, recorded_frame_epoch: u8) -> bool {
        self.recorded
            .is_some_and(|epoch| epoch as u8 == recorded_frame_epoch)
    }
}
