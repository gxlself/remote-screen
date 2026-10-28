//! Shared encoder decision logic for native host backends.
//!
//! The platform backends consume this module's tables and state machines. The backend
//! applies [`FfOpt`] entries in order and treats optional-option failures as
//! debug-only. The encoder worker submits frames through [`Mailbox`], takes a keyframe flag
//! before deciding whether a still-frame refine is requested, maps PTS values
//! with [`PtsMap`], restores parameter sets with [`ParamSetCache`], and gates
//! output with [`OutputGate`].  A dropped sink packet requests a keyframe.
//! A still refine is always a non-keyframe; when a keyframe request and refine
//! request coincide, the frame is encoded as a keyframe and is not marked
//! `still_refine`.
//! Rate changes use [`RateApplier`]; applying an Idr rate makes that frame an
//! IDR, while applying a Reopen rate closes and reopens the backend.
//! The encoder worker validates before putting work in the mailbox; its worker takes the key
//! flag, resolves refine, allocates a PTS, sends the frame, fixes parameter
//! sets, then admits the access unit through the output gate.  An encode error
//! is reported as a sticky session failure and subsequent frames are not
//! forwarded until the session reconnects.
//!
//! The rate hysteresis follows protocol section 7.2: overload steps of x0.85
//! are kept about 500 ms apart for IDR backends, while small x1.08 idle steps
//! accumulate until a 20% increase or a stable five-second interval.  The
//! first decrease is immediate, meeting the 200 ms rate-response requirement.

use std::collections::VecDeque;
use std::fmt;

use crate::config::VideoEncoder;
use rsp_core::nal::{hevc_nal_type, split_annexb};
use rsp_core::pb::Codec;

use crate::encoder_pick::{self, EncoderKind, VENDOR_AMD, VENDOR_INTEL, VENDOR_NVIDIA};

// ---- backends ----

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub enum Backend {
    FfNvenc,
    FfAmf,
    FfQsv,
    MfHw,
}

impl Backend {
    pub fn kind(self) -> EncoderKind {
        match self {
            Self::FfNvenc => EncoderKind::Nvenc,
            Self::FfAmf => EncoderKind::Amf,
            Self::FfQsv => EncoderKind::Qsv,
            Self::MfHw => EncoderKind::MediaFoundation,
        }
    }

    pub fn from_kind(kind: EncoderKind) -> Option<Self> {
        match kind {
            EncoderKind::Nvenc => Some(Self::FfNvenc),
            EncoderKind::Amf => Some(Self::FfAmf),
            EncoderKind::Qsv => Some(Self::FfQsv),
            EncoderKind::MediaFoundation => Some(Self::MfHw),
            EncoderKind::OpenH264 => None,
        }
    }

    pub fn is_ffmpeg(self) -> bool {
        !matches!(self, Self::MfHw)
    }

    pub fn name(self) -> &'static str {
        match self {
            Self::FfNvenc => "nvenc",
            Self::FfAmf => "amf",
            Self::FfQsv => "qsv",
            Self::MfHw => "mf",
        }
    }
}

// ---- validation and FFmpeg options ----

pub const MIN_DIM: u32 = 128;
pub const MAX_DIM_H264: u32 = 4096;
pub const MAX_DIM_HEVC: u32 = 8192;
pub const MIN_FPS: u32 = 1;
pub const MAX_FPS: u32 = 240;
pub const MIN_BPS: u64 = 100_000;
pub const MAX_BPS: u64 = 400_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncParams {
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ParamError {
    Codec,
    Size { width: u32, height: u32 },
    Fps(u32),
    Bitrate(u32),
}

impl fmt::Display for ParamError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Codec => f.write_str("unsupported codec"),
            Self::Size { width, height } => write!(f, "invalid size {width}x{height}"),
            Self::Fps(fps) => write!(f, "invalid fps {fps}"),
            Self::Bitrate(bps) => write!(f, "invalid bitrate {bps} kbps"),
        }
    }
}

pub fn validate(
    codec: Codec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_kbps: u32,
) -> Result<EncParams, ParamError> {
    let max_dim = match codec {
        Codec::H264 => MAX_DIM_H264,
        Codec::Hevc => MAX_DIM_HEVC,
        Codec::Unspecified => return Err(ParamError::Codec),
    };
    let even_width = width & !1;
    let even_height = height & !1;
    if even_width < MIN_DIM
        || even_height < MIN_DIM
        || even_width > max_dim
        || even_height > max_dim
    {
        return Err(ParamError::Size { width, height });
    }
    if !(MIN_FPS..=MAX_FPS).contains(&fps) {
        return Err(ParamError::Fps(fps));
    }
    if bitrate_kbps == 0 {
        return Err(ParamError::Bitrate(0));
    }
    let bitrate_bps = (u64::from(bitrate_kbps) * 1000).clamp(MIN_BPS, MAX_BPS);
    Ok(EncParams {
        codec,
        width: even_width,
        height: even_height,
        fps,
        bitrate_bps,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfOpt {
    pub name: &'static str,
    pub value: String,
    pub required: bool,
}

fn opt(name: &'static str, value: impl Into<String>, required: bool) -> FfOpt {
    FfOpt {
        name,
        value: value.into(),
        required,
    }
}

pub fn ffmpeg_codec_name(b: Backend, codec: Codec) -> Option<&'static str> {
    encoder_pick::ffmpeg_encoder_name(b.kind(), codec)
}

pub fn hw_pixel_format(b: Backend) -> Option<&'static str> {
    match b {
        Backend::FfNvenc | Backend::FfAmf => Some("d3d11"),
        Backend::FfQsv => Some("qsv"),
        Backend::MfHw => None,
    }
}

fn common_options(b: Backend, p: &EncParams) -> Vec<FfOpt> {
    let mut out = Vec::new();
    let Some(pixel_format) = hw_pixel_format(b) else {
        return out;
    };
    out.push(opt("pixel_format", pixel_format, true));
    out.push(opt("video_size", format!("{}x{}", p.width, p.height), true));
    out.push(opt("time_base", format!("1/{}", p.fps), true));
    let bitrate = if matches!(b, Backend::FfQsv) {
        p.bitrate_bps.saturating_sub(1)
    } else {
        p.bitrate_bps
    };
    out.push(opt("b", bitrate.to_string(), true));
    out.push(opt("maxrate", p.bitrate_bps.to_string(), true));
    if !matches!(b, Backend::FfQsv) {
        out.push(opt(
            "bufsize",
            vbv_bits(p.bitrate_bps, p.fps).to_string(),
            true,
        ));
    }
    out.push(opt("bf", "0", true));
    out.push(opt("g", "2147483647", true));
    out.push(opt("keyint_min", "2147483647", false));
    out.push(opt("flags", "+cgop+low_delay", false));
    out.push(opt("color_range", "tv", true));
    out.push(opt("colorspace", "bt709", true));
    out.push(opt("color_primaries", "bt709", true));
    out.push(opt("color_trc", "bt709", true));
    out
}

pub fn ffmpeg_options(b: Backend, p: &EncParams) -> Vec<FfOpt> {
    let mut out = common_options(b, p);
    match b {
        Backend::FfNvenc => {
            out.extend([
                opt("preset", "p1", true),
                opt("tune", "ull", true),
                opt("rc", "cbr", true),
                opt("zerolatency", "1", true),
                opt("delay", "0", true),
                opt("forced-idr", "1", true),
                opt(
                    "profile",
                    if p.codec == Codec::H264 {
                        "high"
                    } else {
                        "main"
                    },
                    false,
                ),
            ]);
        }
        Backend::FfAmf => {
            out.extend([
                opt("usage", "ultralowlatency", true),
                opt("quality", "speed", true),
                opt("rc", "cbr", true),
                opt("latency", "1", false),
                opt("async_depth", "1", false),
                opt("forced_idr", "1", true),
                opt("filler_data", "0", false),
            ]);
            if p.codec == Codec::H264 {
                out.extend([
                    opt("frame_skipping", "0", false),
                    opt("profile", "high", false),
                ]);
            } else {
                out.extend([
                    opt("skip_frame", "0", false),
                    opt("gops_per_idr", "1", false),
                    opt("header_insertion_mode", "idr", false),
                    opt("profile", "main", false),
                ]);
            }
        }
        Backend::FfQsv => {
            out.extend([
                opt("preset", "veryfast", true),
                opt("forced_idr", "1", true),
                opt("async_depth", "1", true),
                opt("low_delay_brc", "1", false),
                opt("low_power", "1", false),
                opt("scenario", "displayremoting", false),
            ]);
            if p.codec == Codec::H264 {
                out.extend([
                    opt("recovery_point_sei", "0", false),
                    opt("pic_timing_sei", "0", false),
                    opt("profile", "high", false),
                ]);
            } else {
                out.push(opt("profile", "main", false));
            }
        }
        Backend::MfHw => {}
    }
    out
}

pub fn ffmpeg_options_retry(b: Backend, p: &EncParams) -> Option<Vec<FfOpt>> {
    match b {
        Backend::FfQsv => Some(
            ffmpeg_options(b, p)
                .into_iter()
                .filter(|item| item.name != "low_power")
                .collect(),
        ),
        Backend::FfAmf => Some(
            ffmpeg_options(b, p)
                .into_iter()
                .map(|mut item| {
                    if item.name == "usage" {
                        item.value = "lowlatency".to_owned();
                    }
                    item
                })
                .collect(),
        ),
        Backend::FfNvenc | Backend::MfHw => None,
    }
}

pub fn ffmpeg_rate_options(b: Backend, bitrate_bps: u64, fps: u32) -> Vec<FfOpt> {
    match b {
        Backend::FfNvenc | Backend::FfAmf => vec![
            opt("b", bitrate_bps.to_string(), true),
            opt("maxrate", bitrate_bps.to_string(), true),
            opt("bufsize", vbv_bits(bitrate_bps, fps).to_string(), true),
        ],
        Backend::FfQsv => vec![
            opt("b", bitrate_bps.saturating_sub(1).to_string(), true),
            opt("maxrate", bitrate_bps.to_string(), true),
        ],
        Backend::MfHw => Vec::new(),
    }
}

// ---- rate ----

pub fn effective_bps(kbps: u32, fps: u32, open_fps: u32) -> u64 {
    let effective_fps = if fps == 0 { open_fps } else { fps };
    let denominator = u128::from(effective_fps.max(1));
    let value = u128::from(kbps) * 1000 * u128::from(open_fps) / denominator;
    value.min(u128::from(MAX_BPS)).max(u128::from(MIN_BPS)) as u64
}

pub fn vbv_bits(bps: u64, fps: u32) -> u64 {
    let denom = u64::from(fps.max(1));
    bps.saturating_add(denom - 1).saturating_div(denom).max(1)
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RateCost {
    Free,
    Idr,
    Reopen,
}

pub fn rate_cost(b: Backend) -> RateCost {
    match b {
        Backend::FfNvenc | Backend::FfQsv => RateCost::Idr,
        Backend::FfAmf => RateCost::Reopen,
        Backend::MfHw => RateCost::Free,
    }
}

pub const RATE_DOWN_GAP_IDR_US: u64 = 500_000;
pub const RATE_DOWN_GAP_REOPEN_US: u64 = 1_000_000;
pub const RATE_GAP_IDR_US: u64 = 2_000_000;
pub const RATE_GAP_REOPEN_US: u64 = 4_000_000;
pub const RATE_SETTLE_US: u64 = 5_000_000;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RateApplier {
    cost: RateCost,
    current: u64,
    last_applied: Option<u64>,
    pending: Option<u64>,
    pending_since: Option<u64>,
}

impl RateApplier {
    pub fn new(cost: RateCost, initial_bps: u64) -> Self {
        Self {
            cost,
            current: initial_bps,
            last_applied: None,
            pending: None,
            pending_since: None,
        }
    }

    fn gaps(&self) -> (u64, u64) {
        match self.cost {
            RateCost::Idr => (RATE_DOWN_GAP_IDR_US, RATE_GAP_IDR_US),
            RateCost::Reopen => (RATE_DOWN_GAP_REOPEN_US, RATE_GAP_REOPEN_US),
            RateCost::Free => (0, 0),
        }
    }

    fn apply(&mut self, now_us: u64, value: u64) -> Option<u64> {
        self.current = value;
        self.last_applied = Some(now_us);
        self.pending = None;
        self.pending_since = None;
        Some(value)
    }

    fn clear_pending(&mut self) {
        self.pending = None;
        self.pending_since = None;
    }

    fn deadline_for(&self, p: u64) -> Option<u64> {
        if self.pending.is_none() || matches!(self.cost, RateCost::Free) || p == self.current {
            return None;
        }
        if p.saturating_mul(2) <= self.current {
            return None;
        }
        let since = self.pending_since?;
        let last = self.last_applied?;
        let (gap_down, gap) = self.gaps();
        let down = u128::from(p) * 10 <= u128::from(self.current) * 9;
        let up = u128::from(p) * 5 >= u128::from(self.current) * 6;
        let base = if down {
            last.saturating_add(gap_down)
        } else if up {
            last.saturating_add(gap)
        } else {
            last.saturating_add(gap)
                .max(since.saturating_add(RATE_SETTLE_US))
        };
        Some(base)
    }

    fn evaluate(&mut self, now_us: u64) -> Option<u64> {
        let p = self.pending?;
        if p == self.current {
            self.clear_pending();
            return None;
        }
        if matches!(self.cost, RateCost::Free) {
            let diff = p.abs_diff(self.current);
            if u128::from(diff) * 100 >= u128::from(self.current) * 2 {
                return self.apply(now_us, p);
            }
            self.clear_pending();
            return None;
        }
        if p.saturating_mul(2) <= self.current {
            return self.apply(now_us, p);
        }
        // A freshly opened backend has no spacing constraint.  The first
        // requested rate therefore takes effect immediately, regardless of
        // whether it is a decrease or increase.
        if self.last_applied.is_none() {
            return self.apply(now_us, p);
        }
        let deadline = self.deadline_for(p)?;
        if now_us >= deadline {
            self.apply(now_us, p)
        } else {
            None
        }
    }

    pub fn request(&mut self, now_us: u64, bps: u64) -> Option<u64> {
        if self.pending.is_none() {
            self.pending_since = Some(now_us);
        }
        self.pending = Some(bps);
        self.evaluate(now_us)
    }

    pub fn poll(&mut self, now_us: u64) -> Option<u64> {
        self.evaluate(now_us)
    }

    pub fn deadline(&self) -> Option<u64> {
        self.pending.and_then(|p| self.deadline_for(p))
    }

    pub fn current(&self) -> u64 {
        self.current
    }

    pub fn reset(&mut self, cost: RateCost, bps: u64) {
        self.cost = cost;
        self.current = bps;
        self.last_applied = None;
        self.clear_pending();
    }
}

// ---- keyframe and output gate ----

pub const KEY_IDLE_RETRY_US: u64 = 60_000;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct KeyTracker {
    since: Option<u64>,
}

impl KeyTracker {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn request(&mut self, now_us: u64) {
        if self.since.is_none() {
            self.since = Some(now_us);
        }
    }

    pub fn take_for_frame(&mut self) -> bool {
        self.since.take().is_some()
    }

    pub fn poll(&mut self, now_us: u64) -> bool {
        if let Some(since) = self.since {
            if now_us >= since.saturating_add(KEY_IDLE_RETRY_US) {
                self.since = None;
                return true;
            }
        }
        false
    }

    pub fn deadline(&self) -> Option<u64> {
        self.since.map(|s| s.saturating_add(KEY_IDLE_RETRY_US))
    }

    pub fn pending(&self) -> bool {
        self.since.is_some()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct OutputGate {
    waiting_key: bool,
}

impl Default for OutputGate {
    fn default() -> Self {
        Self::new()
    }
}

impl OutputGate {
    pub fn new() -> Self {
        Self { waiting_key: true }
    }

    pub fn on_reopen(&mut self) {
        self.waiting_key = true;
    }

    pub fn on_sink_drop(&mut self) {
        self.waiting_key = true;
    }

    pub fn admit(&mut self, keyframe: bool) -> bool {
        if self.waiting_key {
            if !keyframe {
                return false;
            }
            self.waiting_key = false;
        }
        true
    }
}

// ---- still-frame refine ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineMode {
    Repeat,
    RoiDeltaQp(i32),
    BitrateBump { num: u32, den: u32 },
}

pub const REFINE_ROI_DELTA_QP: i32 = -8;

pub fn refine_mode(b: Backend) -> RefineMode {
    match b {
        Backend::FfNvenc | Backend::FfAmf => RefineMode::Repeat,
        Backend::FfQsv => RefineMode::RoiDeltaQp(REFINE_ROI_DELTA_QP),
        Backend::MfHw => RefineMode::BitrateBump { num: 2, den: 1 },
    }
}

pub fn roi_qoffset(delta_qp: i32) -> (i32, i32) {
    (delta_qp.clamp(-51, 51), 51)
}

pub fn refine_bump_bps(bps: u64, mode: RefineMode) -> u64 {
    match mode {
        RefineMode::BitrateBump { num, den } if den != 0 => {
            (u128::from(bps) * u128::from(num) / u128::from(den)).min(u128::from(MAX_BPS)) as u64
        }
        _ => bps,
    }
}

// ---- mailbox ----

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JobFlags {
    pub force_key: bool,
    pub refine: bool,
}

#[derive(Debug)]
pub struct Mailbox<T> {
    item: Option<(T, JobFlags)>,
    dropped: u64,
}

impl<T> Mailbox<T> {
    pub fn new() -> Self {
        Self {
            item: None,
            dropped: 0,
        }
    }

    pub fn put(&mut self, item: T, flags: JobFlags) -> Option<T> {
        match self.item.take() {
            Some((old_item, old_flags)) => {
                self.item = Some((
                    item,
                    JobFlags {
                        force_key: flags.force_key || old_flags.force_key,
                        refine: flags.refine && old_flags.refine,
                    },
                ));
                self.dropped = self.dropped.saturating_add(1);
                Some(old_item)
            }
            None => {
                self.item = Some((item, flags));
                None
            }
        }
    }

    pub fn take(&mut self) -> Option<(T, JobFlags)> {
        self.item.take()
    }

    pub fn is_empty(&self) -> bool {
        self.item.is_none()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<T> Default for Mailbox<T> {
    fn default() -> Self {
        Self::new()
    }
}

// ---- PTS ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameMeta {
    pub capture_ts_us: u64,
    pub refine: bool,
    pub submit_us: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PtsMap {
    next: i64,
    entries: VecDeque<(i64, FrameMeta)>,
}

impl PtsMap {
    pub const CAP: usize = 32;

    pub fn new() -> Self {
        Self {
            next: 0,
            entries: VecDeque::new(),
        }
    }

    pub fn next_pts(&mut self, meta: FrameMeta) -> i64 {
        let pts = self.next;
        self.next = self.next.saturating_add(1);
        if self.entries.len() >= Self::CAP {
            self.entries.pop_front();
        }
        self.entries.push_back((pts, meta));
        pts
    }

    pub fn take(&mut self, pts: i64) -> Option<FrameMeta> {
        while self
            .entries
            .front()
            .is_some_and(|(entry_pts, _)| *entry_pts < pts)
        {
            self.entries.pop_front();
        }
        if self
            .entries
            .front()
            .is_some_and(|(entry_pts, _)| *entry_pts == pts)
        {
            self.entries.pop_front().map(|(_, meta)| meta)
        } else {
            None
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl Default for PtsMap {
    fn default() -> Self {
        Self::new()
    }
}

// ---- NAL and parameter sets ----

pub fn nal_type(codec: Codec, nal: &[u8]) -> Option<u8> {
    match codec {
        Codec::H264 => nal.first().map(|byte| byte & 0x1f),
        Codec::Hevc => hevc_nal_type(nal),
        Codec::Unspecified => None,
    }
}

pub fn is_param_set(codec: Codec, nal_type: u8) -> bool {
    match codec {
        Codec::H264 => matches!(nal_type, 7 | 8),
        Codec::Hevc => matches!(nal_type, 32..=34),
        Codec::Unspecified => false,
    }
}

pub fn au_is_key(codec: Codec, au: &[u8]) -> bool {
    split_annexb(au).into_iter().any(|nal| {
        nal_type(codec, nal).is_some_and(|ty| match codec {
            Codec::H264 => ty == 5,
            Codec::Hevc => (16..=21).contains(&ty),
            Codec::Unspecified => false,
        })
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ParamSetCache {
    codec: Codec,
    sets: [Option<Vec<u8>>; 3],
}

impl ParamSetCache {
    pub fn new(codec: Codec) -> Self {
        Self {
            codec,
            sets: [None, None, None],
        }
    }

    fn slot(&self, ty: u8) -> Option<usize> {
        match self.codec {
            Codec::H264 => match ty {
                7 => Some(0),
                8 => Some(1),
                _ => None,
            },
            Codec::Hevc => match ty {
                32 => Some(0),
                33 => Some(1),
                34 => Some(2),
                _ => None,
            },
            Codec::Unspecified => None,
        }
    }

    pub fn fix(&mut self, au: &[u8], keyframe: bool) -> Vec<u8> {
        let mut has_param = false;
        for nal in split_annexb(au) {
            if let Some(ty) = nal_type(self.codec, nal) {
                if let Some(slot) = self.slot(ty) {
                    self.sets[slot] = Some(nal.to_vec());
                    has_param = true;
                }
            }
        }
        if !keyframe || has_param || self.sets.iter().all(Option::is_none) {
            return au.to_vec();
        }
        let mut out = Vec::new();
        for set in self.sets.iter().flatten() {
            out.extend_from_slice(&[0, 0, 0, 1]);
            out.extend_from_slice(set);
        }
        out.extend_from_slice(au);
        out
    }

    pub fn clear(&mut self) {
        self.sets = [None, None, None];
    }
}

// ---- probing and selection ----

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ProbeResult {
    Ok,
    Unsupported,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ProbeEntry {
    pub luid: i64,
    pub vendor_id: u32,
    pub has_output: bool,
    pub backend: Backend,
    pub codec: Codec,
    pub result: ProbeResult,
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct ProbeTable {
    entries: Vec<ProbeEntry>,
}

impl ProbeTable {
    pub fn record(&mut self, entry: ProbeEntry) {
        if let Some(old) = self.entries.iter_mut().find(|old| {
            old.luid == entry.luid && old.backend == entry.backend && old.codec == entry.codec
        }) {
            *old = entry;
        } else {
            self.entries.push(entry);
        }
    }

    pub fn get(&self, luid: i64, backend: Backend, codec: Codec) -> Option<ProbeResult> {
        self.entries
            .iter()
            .find(|entry| entry.luid == luid && entry.backend == backend && entry.codec == codec)
            .map(|entry| entry.result)
    }

    pub fn entries(&self) -> &[ProbeEntry] {
        &self.entries
    }

    pub fn codec_supported(&self, codec: Codec) -> Option<bool> {
        let mut luids = Vec::new();
        for entry in &self.entries {
            if entry.has_output && !luids.contains(&entry.luid) {
                luids.push(entry.luid);
            }
        }
        if luids.is_empty() {
            return None;
        }
        let mut all_ok = true;
        let mut any_entry = false;
        for luid in luids {
            let rows: Vec<_> = self
                .entries
                .iter()
                .filter(|entry| entry.luid == luid && entry.codec == codec)
                .collect();
            if rows.is_empty() {
                all_ok = false;
                continue;
            }
            any_entry = true;
            if !rows.iter().any(|entry| entry.result == ProbeResult::Ok) {
                return Some(false);
            }
        }
        if all_ok && any_entry {
            Some(true)
        } else {
            None
        }
    }
}

pub const VENDOR_MICROSOFT_BASIC: u32 = 0x1414;

pub fn probe_plan(vendor_id: u32) -> Vec<Backend> {
    match vendor_id {
        VENDOR_NVIDIA => vec![Backend::FfNvenc, Backend::MfHw],
        VENDOR_AMD => vec![Backend::FfAmf, Backend::MfHw],
        VENDOR_INTEL => vec![Backend::FfQsv, Backend::MfHw],
        VENDOR_MICROSOFT_BASIC => Vec::new(),
        _ => vec![Backend::MfHw],
    }
}

pub fn plan(
    vendor_id: u32,
    luid: i64,
    pref: &VideoEncoder,
    codec: Codec,
    ffmpeg_ok: bool,
    probes: &ProbeTable,
) -> Vec<Backend> {
    encoder_pick::candidates(Some(vendor_id), pref.clone(), codec)
        .into_iter()
        .filter_map(Backend::from_kind)
        .filter(|backend| {
            (!backend.is_ffmpeg() || ffmpeg_ok)
                && !matches!(
                    probes.get(luid, *backend, codec),
                    Some(ProbeResult::Unsupported | ProbeResult::Failed)
                )
        })
        .collect()
}

// ---- failure and reopen ----

pub const FAIL_REPORT_GAP_US: u64 = 5_000_000;

pub fn reopen_backoff(failures: u32) -> u64 {
    if failures == 0 {
        return 0;
    }
    let shift = failures.saturating_sub(1).min(4);
    (500_000u64 << shift).min(8_000_000)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FailureState {
    failures: u32,
    retry_at: Option<u64>,
    last_report: Option<u64>,
}

impl FailureState {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_failure(&mut self, now_us: u64) {
        self.failures = self.failures.saturating_add(1);
        self.retry_at = Some(now_us.saturating_add(reopen_backoff(self.failures)));
    }

    pub fn on_success(&mut self) {
        self.failures = 0;
        self.retry_at = None;
        self.last_report = None;
    }

    pub fn on_device_change(&mut self) {
        if self.failed() {
            self.retry_at = Some(0);
        }
    }

    pub fn failed(&self) -> bool {
        self.failures != 0
    }

    pub fn retry_at(&self) -> Option<u64> {
        self.retry_at
    }

    pub fn should_report(&mut self, now_us: u64) -> bool {
        if !self.failed() {
            return false;
        }
        if self
            .last_report
            .is_some_and(|last| now_us < last.saturating_add(FAIL_REPORT_GAP_US))
        {
            return false;
        }
        self.last_report = Some(now_us);
        true
    }
}

// ---- statistics ----

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct EncodeStats {
    pub submitted: u64,
    pub encoded: u64,
    pub keyframes: u64,
    pub refines: u64,
    pub mailbox_dropped: u64,
    pub sink_dropped: u64,
    pub gated: u64,
    pub rate_applied: u64,
    pub reopens: u64,
    pub failures: u64,
    pub idle_key_retries: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LatencyWindow {
    values: VecDeque<u64>,
}

impl LatencyWindow {
    pub const CAP: usize = 1024;

    pub fn new() -> Self {
        Self {
            values: VecDeque::with_capacity(Self::CAP),
        }
    }

    pub fn push(&mut self, us: u64) {
        if self.values.len() >= Self::CAP {
            self.values.pop_front();
        }
        self.values.push_back(us);
    }

    pub fn percentile(&self, p: u8) -> Option<u64> {
        if self.values.is_empty() {
            return None;
        }
        let mut values: Vec<u64> = self.values.iter().copied().collect();
        values.sort_unstable();
        let rank = (usize::from(p).max(1) * values.len())
            .div_ceil(100)
            .min(values.len());
        Some(values[rank - 1])
    }

    pub fn len(&self) -> usize {
        self.values.len()
    }

    pub fn is_empty(&self) -> bool {
        self.values.is_empty()
    }

    pub fn clear(&mut self) {
        self.values.clear();
    }
}

impl Default for LatencyWindow {
    fn default() -> Self {
        Self::new()
    }
}

// ---- session decisions ----

pub const ENCODER_ENV: &str = "RS_VIDEO_ENCODER";

pub fn parse_encoder_pref(s: &str) -> Option<VideoEncoder> {
    match s.trim().to_ascii_lowercase().as_str() {
        "auto" => Some(VideoEncoder::Auto),
        "nvenc" => Some(VideoEncoder::Nvenc),
        "amf" => Some(VideoEncoder::Amf),
        "qsv" => Some(VideoEncoder::Qsv),
        "mf" => Some(VideoEncoder::Mf),
        "openh264" => Some(VideoEncoder::OpenH264),
        _ => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SourceKey {
    pub device_gen: u64,
    pub width: u32,
    pub height: u32,
    pub rotation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SourceChange {
    Same,
    Reproc,
    Reopen,
}

pub fn source_change(old: Option<SourceKey>, new: SourceKey) -> SourceChange {
    let Some(old) = old else {
        return SourceChange::Reopen;
    };
    if old.device_gen != new.device_gen {
        SourceChange::Reopen
    } else if old.width != new.width || old.height != new.height || old.rotation != new.rotation {
        SourceChange::Reproc
    } else {
        SourceChange::Same
    }
}

#[derive(Debug, Default, Clone, PartialEq, Eq)]
pub struct BackendChooser {
    excluded: Vec<Backend>,
}

impl BackendChooser {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn choose(
        &self,
        vendor_id: u32,
        luid: i64,
        pref: &VideoEncoder,
        codec: Codec,
        ffmpeg_ok: bool,
        probes: &ProbeTable,
    ) -> (Vec<Backend>, bool) {
        let filtered = |preference: &VideoEncoder| {
            plan(vendor_id, luid, preference, codec, ffmpeg_ok, probes)
                .into_iter()
                .filter(|backend| !self.excluded.contains(backend))
                .collect::<Vec<_>>()
        };
        let list = filtered(pref);
        if list.is_empty() && *pref != VideoEncoder::Auto {
            (filtered(&VideoEncoder::Auto), true)
        } else {
            (list, false)
        }
    }

    pub fn exclude(&mut self, b: Backend) {
        if !self.excluded.contains(&b) {
            self.excluded.push(b);
        }
    }

    pub fn excluded(&self) -> &[Backend] {
        &self.excluded
    }

    pub fn reset(&mut self) {
        self.excluded.clear();
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RefineAction {
    Normal,
    Skip,
    Repeat,
    Roi(i32, i32),
    Bump(u64),
}

pub fn refine_action(
    mode: RefineMode,
    requested: bool,
    idr: bool,
    enabled: bool,
    current_bps: u64,
) -> RefineAction {
    if !requested || idr {
        return RefineAction::Normal;
    }
    if !enabled {
        return RefineAction::Skip;
    }
    match mode {
        RefineMode::Repeat => RefineAction::Repeat,
        RefineMode::RoiDeltaQp(delta) => {
            let (num, den) = roi_qoffset(delta);
            RefineAction::Roi(num, den)
        }
        RefineMode::BitrateBump { .. } => RefineAction::Bump(refine_bump_bps(current_bps, mode)),
    }
}
