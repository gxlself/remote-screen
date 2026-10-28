//! 帧闸门与 VideoConsumer 适配（复用 iOS SessionFrameGate 的处理方式）。

use std::sync::{Arc, Mutex, MutexGuard, Weak};

use rs_mobile::rscore::{ffi_session::h264_to_avcc, tables::VideoConsumer};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::{
    nal::{hevc_nal_type, hevc_to_avcc, hevc_type, split_annexb, HevcAccessUnit},
    pb,
};

use crate::platform::{
    BitstreamFormat, DecodeResult, DesktopFrame, FrameDone, FrameSink, StreamInfo, VideoCodec,
};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum KeyframeReason {
    DecoderReset,
    Backlog,
}

impl KeyframeReason {
    pub fn to_pb(self) -> pb::keyframe_request::Reason {
        match self {
            Self::DecoderReset => pb::keyframe_request::Reason::DecoderReset,
            Self::Backlog => pb::keyframe_request::Reason::Backlog,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GateAction {
    Submit(u32),
    Drop(u32),
    RequestKeyframe(KeyframeReason),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct GateConfig {
    /// 已提交、尚未回执的帧数上限。默认 3：一帧解码 + iOS 的两帧排队。
    pub max_in_flight: usize,
    /// 等待关键帧时的重请求间隔，默认 500_000 微秒。
    pub rerequest_after_us: u64,
    /// 所有关键帧请求共用的最小间隔，默认 300_000 微秒。
    pub min_request_interval_us: u64,
}

impl Default for GateConfig {
    fn default() -> Self {
        Self {
            max_in_flight: 3,
            rerequest_after_us: 500_000,
            min_request_interval_us: 300_000,
        }
    }
}

pub struct FrameGate {
    cfg: GateConfig,
    in_flight: Vec<u32>,
    waiting: bool,
    waiting_reason: Option<KeyframeReason>,
    reset_waiting_since_us: Option<u64>,
    last_request_us: Option<u64>,
}

impl FrameGate {
    pub fn new(cfg: GateConfig) -> Self {
        Self {
            cfg,
            in_flight: Vec::new(),
            waiting: false,
            waiting_reason: None,
            reset_waiting_since_us: None,
            last_request_us: None,
        }
    }

    pub fn on_frame(
        &mut self,
        frame_id: u32,
        keyframe: bool,
        has_params: bool,
        now_us: u64,
    ) -> Vec<GateAction> {
        if self.waiting {
            if !keyframe || !has_params {
                return vec![GateAction::Drop(frame_id)];
            }
            self.waiting = false;
            self.waiting_reason = None;
            self.reset_waiting_since_us = None;
            // Recovery keyframes must pass even while old submissions await
            // receipts: those frames cannot be recalled from the sink.
            self.in_flight.push(frame_id);
            return vec![GateAction::Submit(frame_id)];
        }

        self.in_flight.push(frame_id);
        if self.in_flight.len() > self.cfg.max_in_flight {
            self.in_flight.pop();
            let mut actions = vec![GateAction::Drop(frame_id)];
            actions.extend(self.recover(KeyframeReason::Backlog, now_us));
            actions
        } else {
            vec![GateAction::Submit(frame_id)]
        }
    }

    /// `ok = false` 表示解码失败；reset 之前的回执忽略。
    pub fn on_done(&mut self, frame_id: u32, ok: bool, now_us: u64) -> Vec<GateAction> {
        let Some(index) = self.in_flight.iter().position(|&id| id == frame_id) else {
            return Vec::new();
        };
        self.in_flight.swap_remove(index);
        if !ok && !self.waiting {
            self.recover(KeyframeReason::DecoderReset, now_us)
        } else {
            Vec::new()
        }
    }

    pub fn on_tick(&mut self, now_us: u64) -> Vec<GateAction> {
        if !self.waiting {
            return Vec::new();
        }
        if let Some(reason) = self.waiting_reason {
            if self
                .last_request_us
                .is_some_and(|last| now_us.saturating_sub(last) >= self.cfg.rerequest_after_us)
            {
                return self.request(reason, now_us);
            }
        } else if self
            .reset_waiting_since_us
            .is_some_and(|since| now_us.saturating_sub(since) >= self.cfg.rerequest_after_us)
        {
            self.waiting_reason = Some(KeyframeReason::Backlog);
            self.reset_waiting_since_us = None;
            return self.request(KeyframeReason::Backlog, now_us);
        }
        Vec::new()
    }

    /// 清空在途记录。请求节流时间保留（同 iOS）。
    pub fn reset(&mut self, wait_for_keyframe: bool, now_us: u64) {
        self.in_flight.clear();
        self.waiting = wait_for_keyframe;
        self.waiting_reason = None;
        self.reset_waiting_since_us = wait_for_keyframe.then_some(now_us);
    }

    pub fn in_flight(&self) -> usize {
        self.in_flight.len()
    }

    pub fn waiting_for_keyframe(&self) -> bool {
        self.waiting
    }

    fn recover(&mut self, reason: KeyframeReason, now_us: u64) -> Vec<GateAction> {
        self.waiting = true;
        self.waiting_reason = Some(reason);
        self.reset_waiting_since_us = None;
        self.request(reason, now_us)
    }

    fn request(&mut self, reason: KeyframeReason, now_us: u64) -> Vec<GateAction> {
        if self
            .last_request_us
            .is_some_and(|last| now_us.saturating_sub(last) < self.cfg.min_request_interval_us)
        {
            return Vec::new();
        }
        self.last_request_us = Some(now_us);
        vec![GateAction::RequestKeyframe(reason)]
    }
}

pub trait VideoControl: Send + Sync + 'static {
    fn request_keyframe(&self, reason: KeyframeReason);
    fn report_decoded(&self, decode_us: u64, latency_us: u64, queue: u32);
    /// 本机 unix 微秒（与 `ReceivedFrame.received_us` 同一时钟）。
    fn now_us(&self) -> u64;
}

pub trait VideoEvents: Send + Sync + 'static {
    /// 新流配置（首次或配置变化后，在 sink.configure 之后调用）。
    fn stream(&self, info: &StreamInfo);
    fn cursor_msg(&self, m: &pb::CursorMsg);
    /// Host 光标位置（StreamConfig.cursor_* 与 CursorEvent）。
    fn host_pointer(&self, x: f32, y: f32, visible: bool);
    fn closed(&self, reason: &str);
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DecodeStats {
    pub submitted: u64,
    pub shown: u64,
    pub failed: u64,
    pub dropped: u64,
    pub keyframe_requests: u64,
    pub last_decode_us: u64,
}

/// 转成平台帧。Avcc 提取参数集；非关键帧清空参数集；AnnexB 原样复制。
pub fn to_desktop_frame(
    f: &ReceivedFrame,
    codec: VideoCodec,
    format: BitstreamFormat,
    epoch: u32,
) -> DesktopFrame {
    let mut au = match format {
        BitstreamFormat::AnnexB => HevcAccessUnit {
            avcc: f.frame.data.clone(),
            ..HevcAccessUnit::default()
        },
        BitstreamFormat::Avcc => match codec {
            VideoCodec::Hevc => hevc_to_avcc(&f.frame.data),
            VideoCodec::H264 => h264_to_avcc(&f.frame.data),
        },
    };
    if !f.frame.keyframe {
        au.vps = None;
        au.sps = None;
        au.pps = None;
    }
    DesktopFrame {
        frame_id: f.frame.frame_id,
        epoch,
        display_id: u32::from(f.frame.display_id),
        codec,
        keyframe: f.frame.keyframe,
        data: au.avcc,
        vps: au.vps,
        sps: au.sps,
        pps: au.pps,
        capture_ts_us: f.frame.capture_ts_us,
        capture_local_us: f.capture_local_us,
        received_us: f.received_us,
    }
}

/// 只扫 Annex B NAL 头，不复制载荷。HEVC 需 VPS/SPS/PPS，H.264 需 SPS/PPS。
pub fn has_parameter_sets(data: &[u8], codec: VideoCodec) -> bool {
    let mut found = 0;
    let required = match codec {
        VideoCodec::Hevc => 7,
        VideoCodec::H264 => 3,
    };
    for nal in split_annexb(data) {
        found |= match codec {
            VideoCodec::Hevc => match hevc_nal_type(nal) {
                Some(hevc_type::VPS) => 1,
                Some(hevc_type::SPS) => 2,
                Some(hevc_type::PPS) => 4,
                _ => 0,
            },
            VideoCodec::H264 => match nal[0] & 0x1f {
                7 => 1,
                8 => 2,
                _ => 0,
            },
        };
        if found == required {
            return true;
        }
    }
    false
}

struct Inner {
    config: Option<pb::StreamConfig>,
    generation: u64,
    gate: FrameGate,
    stats: DecodeStats,
    closed: bool,
}

pub struct FrameAdapter {
    sink: Arc<dyn FrameSink>,
    control: Arc<dyn VideoControl>,
    events: Arc<dyn VideoEvents>,
    format: BitstreamFormat,
    this: Weak<Self>,
    inner: Mutex<Inner>,
}

impl FrameAdapter {
    pub fn new(
        sink: Arc<dyn FrameSink>,
        control: Arc<dyn VideoControl>,
        events: Arc<dyn VideoEvents>,
        cfg: GateConfig,
    ) -> Arc<Self> {
        let format = sink.format();
        Arc::new_cyclic(|this| Self {
            sink,
            control,
            events,
            format,
            this: this.clone(),
            inner: Mutex::new(Inner {
                config: None,
                generation: 0,
                gate: FrameGate::new(cfg),
                stats: DecodeStats::default(),
                closed: false,
            }),
        })
    }

    /// 控制器每 ≤ 8 ms 调一次。
    pub fn tick(&self) {
        let now = self.control.now_us();
        let (generation, actions) = {
            let mut inner = self.lock();
            if inner.closed {
                return;
            }
            (inner.generation, inner.gate.on_tick(now))
        };
        self.request_keyframes(generation, &actions);
    }

    pub fn stats(&self) -> DecodeStats {
        self.lock().stats
    }

    /// 停止接收：之后的回调全部忽略，调 sink.close()。可重复调用。
    pub fn close(&self) {
        self.close_once();
    }

    fn lock(&self) -> MutexGuard<'_, Inner> {
        self.inner.lock().unwrap_or_else(|error| error.into_inner())
    }

    fn close_once(&self) -> bool {
        {
            let mut inner = self.lock();
            if inner.closed {
                return false;
            }
            inner.closed = true;
            inner.generation = inner.generation.wrapping_add(1);
        }
        self.sink.close();
        true
    }

    fn request_keyframes(&self, generation: u64, actions: &[GateAction]) {
        for action in actions {
            if let GateAction::RequestKeyframe(reason) = action {
                {
                    let mut inner = self.lock();
                    if inner.closed || inner.generation != generation {
                        return;
                    }
                    inner.stats.keyframe_requests += 1;
                }
                self.control.request_keyframe(*reason);
            }
        }
    }

    fn done(&self, generation: u64, frame_id: u32, received_us: u64, result: DecodeResult) {
        let now = self.control.now_us();
        let (actions, queue) = {
            let mut inner = self.lock();
            if inner.closed || inner.generation != generation {
                return;
            }
            let actions =
                inner
                    .gate
                    .on_done(frame_id, matches!(result, DecodeResult::Shown { .. }), now);
            match result {
                DecodeResult::Shown { decode_us } => {
                    inner.stats.shown += 1;
                    inner.stats.last_decode_us = decode_us;
                }
                DecodeResult::Failed => inner.stats.failed += 1,
            }
            let queue = u32::try_from(inner.gate.in_flight()).unwrap_or(u32::MAX);
            (actions, queue)
        };
        if let DecodeResult::Shown { decode_us } = result {
            self.control
                .report_decoded(decode_us, now.saturating_sub(received_us), queue);
        }
        self.request_keyframes(generation, &actions);
    }
}

impl VideoConsumer for FrameAdapter {
    fn frame(&self, f: &ReceivedFrame) {
        let (config, generation) = {
            let mut inner = self.lock();
            let config = inner
                .config
                .filter(|c| !inner.closed && f.frame.epoch == (c.epoch & 0xff) as u8);
            let Some(config) = config else {
                inner.stats.dropped += 1;
                return;
            };
            (config, inner.generation)
        };
        // Only known codecs enter the stored configuration.
        let codec = if config.codec == pb::Codec::Hevc as i32 {
            VideoCodec::Hevc
        } else {
            VideoCodec::H264
        };
        let has_params = f.frame.keyframe && has_parameter_sets(&f.frame.data, codec);
        let now = self.control.now_us();
        let actions = {
            let mut inner = self.lock();
            if inner.closed || inner.generation != generation {
                inner.stats.dropped += 1;
                return;
            }
            let actions = inner
                .gate
                .on_frame(f.frame.frame_id, f.frame.keyframe, has_params, now);
            for action in &actions {
                match action {
                    GateAction::Submit(_) => inner.stats.submitted += 1,
                    GateAction::Drop(_) => inner.stats.dropped += 1,
                    GateAction::RequestKeyframe(_) => {}
                }
            }
            actions
        };
        for action in &actions {
            if let GateAction::Submit(frame_id) = *action {
                let frame = to_desktop_frame(f, codec, self.format, config.epoch);
                let this = self.this.clone();
                let received_us = f.received_us;
                let done = FrameDone::new(move |result| {
                    if let Some(adapter) = this.upgrade() {
                        adapter.done(generation, frame_id, received_us, result);
                    }
                });
                // A sink may finish synchronously, including by dropping done.
                self.sink.submit(frame, done);
            }
        }
        self.request_keyframes(generation, &actions);
    }

    fn stream_config(&self, c: &pb::StreamConfig) {
        let codec = match pb::Codec::try_from(c.codec) {
            Ok(pb::Codec::Hevc) => VideoCodec::Hevc,
            Ok(pb::Codec::H264) => VideoCodec::H264,
            _ => {
                tracing::warn!(codec = c.codec, "ignoring unsupported desktop video codec");
                return;
            }
        };
        let info = StreamInfo {
            codec,
            width: c.width,
            height: c.height,
            display_id: c.display_id,
        };
        let now = self.control.now_us();
        let changed = {
            let mut inner = self.lock();
            if inner.closed {
                return;
            }
            if inner.config.as_ref() == Some(c) {
                false
            } else {
                inner.generation = inner.generation.wrapping_add(1);
                inner.gate.reset(true, now);
                inner.config = Some(*c);
                true
            }
        };
        if changed {
            self.sink.configure(&info);
            self.events.stream(&info);
        }
        self.events
            .host_pointer(c.cursor_x, c.cursor_y, c.cursor_visible);
    }

    fn cursor(&self, c: &CursorEvent) {
        if !self.lock().closed {
            self.events.host_pointer(c.x, c.y, c.visible);
        }
    }

    fn cursor_msg(&self, m: &pb::CursorMsg) {
        if !self.lock().closed {
            self.events.cursor_msg(m);
        }
    }

    fn clock(&self, _rtt_us: u64, _offset_us: i64) {}

    fn closed(&self, reason: &str) {
        if self.close_once() {
            self.events.closed(reason);
        }
    }
}
