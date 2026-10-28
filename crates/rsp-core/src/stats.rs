//! Client and host link statistics. See docs/dev/03-protocol.md §3, §7.1 and
//! §9 (task A10).
//!
//! All collectors are pure state machines: the caller supplies unix-epoch
//! microseconds, so tests do not depend on a wall clock or tokio.

use std::collections::VecDeque;

use crate::pb::{Feedback, HostStats, PathKind, Transport};

/// Snapshot cadence of the client HUD / `-disconnectAfter` exporter (2 Hz).
pub const CLIENT_SNAPSHOT_INTERVAL_US: u64 = 500_000;
/// Window used by one client snapshot.
pub const CLIENT_SAMPLE_WINDOW_US: u64 = 500_000;
/// Feedback window (5 Hz in the protocol), independent of the HUD snapshot.
pub const FEEDBACK_WINDOW_US: u64 = 200_000;
/// HostStats cadence.
pub const HOST_STATS_INTERVAL_US: u64 = 1_000_000;
/// DelayGradient keeps samples from this trailing interval.
pub const DELAY_GRADIENT_WINDOW_US: u64 = 1_000_000;

const CLIENT_EVENT_HISTORY_US: u64 = 1_000_000;
const RTT_WINDOW_SAMPLES: usize = 5;

/// A fixed-capacity, nearest-rank percentile window.
///
/// For a sorted sample set of length `N`, rank `r = ceil(p * N)` (at least
/// one), and the result is element `r - 1`. Thus p50 of `[1, 2]` is `1` and
/// p95 is `2`. Empty windows report zero, which keeps HUD output defined at
/// startup.
#[derive(Debug, Clone)]
pub struct Percentiles {
    capacity: usize,
    samples: VecDeque<u64>,
}

impl Percentiles {
    /// `capacity == 0` is treated as one sample, so a value is never silently
    /// discarded before it can be reported.
    pub fn new(capacity: usize) -> Self {
        Self {
            capacity: capacity.max(1),
            samples: VecDeque::new(),
        }
    }

    /// Adds one sample and drops the oldest value when over capacity.
    pub fn push(&mut self, value: u64) {
        self.samples.push_back(value);
        while self.samples.len() > self.capacity {
            self.samples.pop_front();
        }
    }

    /// Adds values in order, retaining the most recent `capacity` samples.
    pub fn extend<I: IntoIterator<Item = u64>>(&mut self, values: I) {
        for value in values {
            self.push(value);
        }
    }

    pub fn len(&self) -> usize {
        self.samples.len()
    }

    pub fn is_empty(&self) -> bool {
        self.samples.is_empty()
    }

    pub fn p50(&self) -> u64 {
        nearest_rank(self.samples.iter().copied(), 50)
    }

    pub fn p95(&self) -> u64 {
        nearest_rank(self.samples.iter().copied(), 95)
    }

    pub fn p99(&self) -> u64 {
        nearest_rank(self.samples.iter().copied(), 99)
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

fn nearest_rank<I: IntoIterator<Item = u64>>(values: I, percentile: u8) -> u64 {
    let mut sorted: Vec<u64> = values.into_iter().collect();
    sorted.sort_unstable();
    nearest_rank_sorted(&sorted, percentile)
}

fn nearest_rank_sorted(sorted: &[u64], percentile: u8) -> u64 {
    if sorted.is_empty() {
        return 0;
    }
    let rank = (u128::from(percentile) * sorted.len() as u128)
        .div_ceil(100)
        .max(1);
    sorted[rank as usize - 1]
}

fn percentile_u32(samples: &mut [u64], percentile: u8) -> u32 {
    samples.sort_unstable();
    nearest_rank_sorted(samples, percentile).min(u64::from(u32::MAX)) as u32
}

/// One row of `session.stats`, emitted at 2 Hz.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ClientStats {
    pub fps: u32,
    pub recv_kbps: u32,
    pub loss_pct: f32,
    pub frames_lost: u32,
    pub fec_recovered: u32,
    pub decode_p50_us: u32,
    pub decode_p95_us: u32,
    pub latency_p50_us: u32,
    pub latency_p95_us: u32,
    pub rtt_us: u32,
    pub path: PathKind,
    pub transport: Transport,
}

/// Whole-session capture-to-decoded percentiles and frame counts.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct SessionSummary {
    pub latency_p50_us: u32,
    pub latency_p95_us: u32,
    pub latency_p99_us: u32,
    pub frames_decoded: u64,
    pub frames_lost: u64,
}

#[derive(Debug, Clone, Copy)]
struct FrameEvent {
    at_us: u64,
    bytes: u64,
    shards_received: u32,
    shards_lost: u32,
    recovered_by_fec: bool,
}

#[derive(Debug, Clone, Copy)]
struct DecodeEvent {
    at_us: u64,
    decode_us: u64,
    capture_to_decoded_us: u64,
}

fn in_trailing_window(at_us: u64, cutoff_us: u64, now_us: u64) -> bool {
    at_us >= cutoff_us && at_us < now_us
}

/// Client-side collector for HUD snapshots and Feedback.
///
/// The 500 ms snapshot and 200 ms Feedback windows are independent trailing
/// windows. Reading either one does not consume samples from the other.
pub struct ClientStatsCollector {
    session_start_us: Option<u64>,
    last_snapshot_us: Option<u64>,
    frames: VecDeque<FrameEvent>,
    lost_frames: VecDeque<u64>,
    decodes: VecDeque<DecodeEvent>,
    recent_rtts: VecDeque<u64>,
    session_latencies: Vec<u64>,
    session_frames_decoded: u64,
    session_frames_lost: u64,
    path: PathKind,
    transport: Transport,
}

impl ClientStatsCollector {
    pub fn new() -> Self {
        Self {
            session_start_us: None,
            last_snapshot_us: None,
            frames: VecDeque::new(),
            lost_frames: VecDeque::new(),
            decodes: VecDeque::new(),
            recent_rtts: VecDeque::with_capacity(RTT_WINDOW_SAMPLES),
            session_latencies: Vec::new(),
            session_frames_decoded: 0,
            session_frames_lost: 0,
            path: PathKind::PathUnspecified,
            transport: Transport::Unspecified,
        }
    }

    /// Starts the snapshot cadence at a caller-known session time. The first
    /// snapshot becomes due 500 ms later.
    pub fn starting_at(start_us: u64) -> Self {
        let mut collector = Self::new();
        collector.session_start_us = Some(start_us);
        collector.last_snapshot_us = Some(start_us);
        collector
    }

    fn begin_session_if_needed(&mut self, now_us: u64) {
        if self.session_start_us.is_none() {
            self.session_start_us = Some(now_us);
            self.last_snapshot_us = Some(now_us);
        }
    }

    fn prune_history(&mut self, now_us: u64) {
        let cutoff_us = now_us.saturating_sub(CLIENT_EVENT_HISTORY_US);
        while self
            .frames
            .front()
            .is_some_and(|event| event.at_us < cutoff_us)
        {
            self.frames.pop_front();
        }
        while self
            .lost_frames
            .front()
            .is_some_and(|at_us| *at_us < cutoff_us)
        {
            self.lost_frames.pop_front();
        }
        while self
            .decodes
            .front()
            .is_some_and(|event| event.at_us < cutoff_us)
        {
            self.decodes.pop_front();
        }
    }

    pub fn on_frame_complete(
        &mut self,
        now_us: u64,
        bytes: usize,
        shards_received: u32,
        shards_lost: u32,
        recovered_by_fec: bool,
    ) {
        self.begin_session_if_needed(now_us);
        self.frames.push_back(FrameEvent {
            at_us: now_us,
            bytes: bytes as u64,
            shards_received,
            shards_lost,
            recovered_by_fec,
        });
        self.prune_history(now_us);
    }

    pub fn on_frame_lost(&mut self, now_us: u64) {
        self.begin_session_if_needed(now_us);
        self.lost_frames.push_back(now_us);
        self.session_frames_lost = self.session_frames_lost.saturating_add(1);
        self.prune_history(now_us);
    }

    pub fn on_decoded(&mut self, now_us: u64, decode_us: u64, capture_to_decoded_us: u64) {
        self.begin_session_if_needed(now_us);
        self.decodes.push_back(DecodeEvent {
            at_us: now_us,
            decode_us,
            capture_to_decoded_us,
        });
        self.session_frames_decoded = self.session_frames_decoded.saturating_add(1);
        self.session_latencies.push(capture_to_decoded_us);
        self.prune_history(now_us);
    }

    pub fn on_rtt(&mut self, rtt_us: u64) {
        self.recent_rtts.push_back(rtt_us);
        while self.recent_rtts.len() > RTT_WINDOW_SAMPLES {
            self.recent_rtts.pop_front();
        }
    }

    pub fn set_path(&mut self, path: PathKind, transport: Transport) {
        self.path = path;
        self.transport = transport;
    }

    pub fn path(&self) -> PathKind {
        self.path
    }

    pub fn transport(&self) -> Transport {
        self.transport
    }

    /// Returns a 2 Hz snapshot of the preceding 500 ms. Returns `None` until
    /// 500 ms have elapsed since the previous snapshot. The window is
    /// `[now - 500 ms, now)`, so back-to-back snapshots never count the same
    /// event twice and a delayed snapshot still covers only its trailing
    /// window.
    pub fn snapshot(&mut self, now_us: u64) -> Option<ClientStats> {
        self.begin_session_if_needed(now_us);
        let last_snapshot_us = self.last_snapshot_us?;
        if now_us.saturating_sub(last_snapshot_us) < CLIENT_SNAPSHOT_INTERVAL_US {
            return None;
        }

        let cutoff_us = now_us.saturating_sub(CLIENT_SAMPLE_WINDOW_US);
        let mut frames_received = 0u64;
        let mut bytes = 0u64;
        let mut frames_recovered = 0u32;
        for event in self
            .frames
            .iter()
            .filter(|event| in_trailing_window(event.at_us, cutoff_us, now_us))
        {
            frames_received = frames_received.saturating_add(1);
            bytes = bytes.saturating_add(event.bytes);
            frames_recovered = frames_recovered.saturating_add(u32::from(event.recovered_by_fec));
        }
        let frames_lost = self
            .lost_frames
            .iter()
            .filter(|at_us| in_trailing_window(**at_us, cutoff_us, now_us))
            .count();
        let frames_lost = frames_lost.min(u32::MAX as usize) as u32;

        let mut decode_us = Vec::new();
        let mut latencies = Vec::new();
        for event in self
            .decodes
            .iter()
            .filter(|event| in_trailing_window(event.at_us, cutoff_us, now_us))
        {
            decode_us.push(event.decode_us);
            latencies.push(event.capture_to_decoded_us);
        }

        let total_frames = frames_received.saturating_add(u64::from(frames_lost));
        let loss_pct = if total_frames == 0 {
            0.0
        } else {
            (frames_lost as f64 / total_frames as f64 * 100.0) as f32
        };

        self.last_snapshot_us = Some(now_us);
        Some(ClientStats {
            fps: fps(frames_received, CLIENT_SAMPLE_WINDOW_US),
            recv_kbps: kbps(bytes, CLIENT_SAMPLE_WINDOW_US),
            loss_pct,
            frames_lost,
            fec_recovered: frames_recovered,
            decode_p50_us: percentile_u32(&mut decode_us, 50),
            decode_p95_us: percentile_u32(&mut decode_us, 95),
            latency_p50_us: percentile_u32(&mut latencies, 50),
            latency_p95_us: percentile_u32(&mut latencies, 95),
            rtt_us: median_u32(&self.recent_rtts),
            path: self.path,
            transport: self.transport,
        })
    }

    pub fn summary(&self) -> SessionSummary {
        let mut latencies = self.session_latencies.clone();
        latencies.sort_unstable();
        SessionSummary {
            latency_p50_us: nearest_rank_sorted(&latencies, 50).min(u64::from(u32::MAX)) as u32,
            latency_p95_us: nearest_rank_sorted(&latencies, 95).min(u64::from(u32::MAX)) as u32,
            latency_p99_us: nearest_rank_sorted(&latencies, 99).min(u64::from(u32::MAX)) as u32,
            frames_decoded: self.session_frames_decoded,
            frames_lost: self.session_frames_lost,
        }
    }

    /// Builds a Feedback from the trailing 200 ms window.
    ///
    /// Frame, shard, bitrate, decode and render fields come from this
    /// collector. `decode_queue` and the delay gradient are caller-supplied
    /// because they are owned by the decoder/scheduling layer. Battery fields
    /// are outside this task and stay neutral.
    pub fn feedback(
        &self,
        now_us: u64,
        epoch: u32,
        decode_queue: u32,
        delay_gradient_us_per_s: i32,
    ) -> Feedback {
        self.feedback_with_low_power(now_us, epoch, decode_queue, delay_gradient_us_per_s, false)
    }

    /// As [`Self::feedback`], with the caller-owned device power state.
    pub fn feedback_with_low_power(
        &self,
        now_us: u64,
        epoch: u32,
        decode_queue: u32,
        delay_gradient_us_per_s: i32,
        low_power: bool,
    ) -> Feedback {
        let cutoff_us = now_us.saturating_sub(FEEDBACK_WINDOW_US);

        let mut frames_received = 0u32;
        let mut frames_recovered = 0u32;
        let mut shards_received = 0u32;
        let mut shards_lost = 0u32;
        let mut bytes = 0u64;
        for event in self
            .frames
            .iter()
            .filter(|event| in_trailing_window(event.at_us, cutoff_us, now_us))
        {
            frames_received = frames_received.saturating_add(1);
            frames_recovered = frames_recovered.saturating_add(u32::from(event.recovered_by_fec));
            shards_received = shards_received.saturating_add(event.shards_received);
            shards_lost = shards_lost.saturating_add(event.shards_lost);
            bytes = bytes.saturating_add(event.bytes);
        }
        let frames_lost = self
            .lost_frames
            .iter()
            .filter(|at_us| in_trailing_window(**at_us, cutoff_us, now_us))
            .count();
        let frames_lost = frames_lost.min(u32::MAX as usize) as u32;

        let mut decode_us = Vec::new();
        let mut latencies = Vec::new();
        let mut frames_decoded = 0u32;
        for event in self
            .decodes
            .iter()
            .filter(|event| in_trailing_window(event.at_us, cutoff_us, now_us))
        {
            frames_decoded = frames_decoded.saturating_add(1);
            decode_us.push(event.decode_us);
            latencies.push(event.capture_to_decoded_us);
        }

        Feedback {
            epoch,
            interval_ms: (FEEDBACK_WINDOW_US / 1_000) as u32,
            frames_received,
            frames_lost,
            frames_recovered,
            shards_received,
            shards_lost,
            recv_kbps: kbps(bytes, FEEDBACK_WINDOW_US),
            delay_gradient_us_per_s,
            // `owd_p50_us` is capture-to-received, which this collector does
            // not observe; the caller can fill it from its shard reorderer.
            owd_p50_us: 0,
            decode_queue,
            decode_p50_us: percentile_u32(&mut decode_us, 50),
            render_fps: fps(u64::from(frames_decoded), FEEDBACK_WINDOW_US),
            battery_pct: 0,
            low_power,
        }
    }
}

impl Default for ClientStatsCollector {
    fn default() -> Self {
        Self::new()
    }
}

/// Builds a Feedback from a collector's current 200 ms window. This is the
/// free-function form of [`ClientStatsCollector::feedback`].
pub fn feedback_from(
    collector: &ClientStatsCollector,
    now_us: u64,
    epoch: u32,
    decode_queue: u32,
    delay_gradient_us_per_s: i32,
) -> Feedback {
    collector.feedback(now_us, epoch, decode_queue, delay_gradient_us_per_s)
}

fn fps(frames: u64, window_us: u64) -> u32 {
    let per_second = frames.saturating_mul(1_000_000) / window_us.max(1);
    per_second.min(u64::from(u32::MAX)) as u32
}

fn kbps(bytes: u64, window_us: u64) -> u32 {
    let bits_per_second = bytes.saturating_mul(8).saturating_mul(1_000_000) / window_us.max(1);
    (bits_per_second / 1_000).min(u64::from(u32::MAX)) as u32
}

fn median_u32(samples: &VecDeque<u64>) -> u32 {
    if samples.is_empty() {
        return 0;
    }
    let mut sorted: Vec<u64> = samples.iter().copied().collect();
    sorted.sort_unstable();
    let middle = sorted.len() / 2;
    let value = if sorted.len() % 2 == 1 {
        sorted[middle]
    } else {
        let sum = u128::from(sorted[middle - 1]) + u128::from(sorted[middle]);
        (sum / 2) as u64
    };
    value.min(u64::from(u32::MAX)) as u32
}

/// Linear-regression slope of `(received_at_us, owd_us)` pairs over the most
/// recent second. The result is in µs/s and is positive when queueing delay is
/// increasing. Samples outside the one-second window are ignored; fewer than
/// two samples, or zero time spread, yields zero.
#[derive(Debug, Clone, Default)]
pub struct DelayGradient {
    samples: VecDeque<(u64, i64)>,
}

impl DelayGradient {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds one observation. Old samples are pruned as newer timestamps
    /// arrive, keeping memory bounded during long sessions.
    pub fn push(&mut self, received_at_us: u64, owd_us: i64) {
        let cutoff_us = received_at_us.saturating_sub(DELAY_GRADIENT_WINDOW_US);
        while self
            .samples
            .front()
            .is_some_and(|(at_us, _)| *at_us < cutoff_us)
        {
            self.samples.pop_front();
        }
        self.samples.push_back((received_at_us, owd_us));
    }

    /// Alias for callers that describe their argument as a sample.
    pub fn push_sample(&mut self, received_at_us: u64, owd_us: i64) {
        self.push(received_at_us, owd_us);
    }

    /// Slope over the last second ending at `now_us`, in µs per second.
    pub fn gradient_us_per_s(&self, now_us: u64) -> i32 {
        let cutoff_us = now_us.saturating_sub(DELAY_GRADIENT_WINDOW_US);
        let points: Vec<(u64, i64)> = self
            .samples
            .iter()
            .copied()
            .filter(|(at_us, _)| *at_us >= cutoff_us && *at_us <= now_us)
            .collect();
        delay_slope(&points)
    }

    /// Adds one observation and returns the slope over the last second ending
    /// at `now_us`.
    pub fn analyze(&mut self, received_at_us: u64, owd_us: i64, now_us: u64) -> i32 {
        self.push(received_at_us, owd_us);
        self.gradient_us_per_s(now_us)
    }

    pub fn clear(&mut self) {
        self.samples.clear();
    }
}

fn delay_slope(points: &[(u64, i64)]) -> i32 {
    if points.len() < 2 {
        return 0;
    }

    // Work relative to the first timestamp to keep the fixed-point products
    // small. A direct integer least-squares fit avoids losing the slope's
    // microsecond-per-microsecond scale before the final µs/s conversion.
    let origin_us = points[0].0;
    let count = points.len() as i128;
    let mut sum_x = 0i128;
    let mut sum_y = 0i128;
    let mut sum_xx = 0i128;
    let mut sum_xy = 0i128;
    for (at_us, owd_us) in points {
        let x = i128::from(at_us.saturating_sub(origin_us));
        let y = i128::from(*owd_us);
        sum_x += x;
        sum_y += y;
        sum_xx += x * x;
        sum_xy += x * y;
    }

    let denominator = count * sum_xx - sum_x * sum_x;
    if denominator == 0 {
        return 0;
    }
    let numerator = count * sum_xy - sum_x * sum_y;
    let value = rounded_div(numerator.saturating_mul(1_000_000), denominator);
    value.clamp(i128::from(i32::MIN), i128::from(i32::MAX)) as i32
}

fn rounded_div(numerator: i128, denominator: i128) -> i128 {
    let quotient = numerator / denominator;
    let remainder = numerator % denominator;
    if remainder.abs().saturating_mul(2) >= denominator.abs() {
        quotient + numerator.signum()
    } else {
        quotient
    }
}

/// Host-side 1 Hz HostStats collector.
pub struct HostStatsCollector {
    window_start_us: Option<u64>,
    last_tick_us: Option<u64>,
    captured: u64,
    sent_bytes: u64,
    encode_samples: Vec<u64>,
}

impl HostStatsCollector {
    pub fn new() -> Self {
        Self {
            window_start_us: None,
            last_tick_us: None,
            captured: 0,
            sent_bytes: 0,
            encode_samples: Vec::new(),
        }
    }

    /// Starts the host statistics window at a caller-known session time.
    pub fn starting_at(start_us: u64) -> Self {
        Self {
            window_start_us: Some(start_us),
            last_tick_us: Some(start_us),
            captured: 0,
            sent_bytes: 0,
            encode_samples: Vec::new(),
        }
    }

    fn begin_if_needed(&mut self, now_us: u64) {
        if self.window_start_us.is_none() {
            self.window_start_us = Some(now_us);
            self.last_tick_us = Some(now_us);
        }
    }

    pub fn on_captured(&mut self, now_us: u64) {
        self.begin_if_needed(now_us);
        self.captured = self.captured.saturating_add(1);
    }

    pub fn on_encoded(&mut self, encode_us: u64) {
        self.encode_samples.push(encode_us);
    }

    pub fn on_sent(&mut self, bytes: usize) {
        self.sent_bytes = self.sent_bytes.saturating_add(bytes as u64);
    }

    /// Emits HostStats once per second. The first call starts a lazily created
    /// window and returns `None`; use [`Self::starting_at`] when the session
    /// start is already known.
    pub fn tick(
        &mut self,
        now_us: u64,
        cpu_pct: u32,
        path: PathKind,
        derp_region: impl Into<String>,
    ) -> Option<HostStats> {
        self.begin_if_needed(now_us);
        let last_tick_us = self.last_tick_us?;
        if now_us.saturating_sub(last_tick_us) < HOST_STATS_INTERVAL_US {
            return None;
        }

        let window_start_us = self.window_start_us?;
        let elapsed_us = now_us.saturating_sub(window_start_us).max(1);
        let captured = std::mem::take(&mut self.captured);
        let sent_bytes = std::mem::take(&mut self.sent_bytes);
        let mut encode_samples = std::mem::take(&mut self.encode_samples);
        encode_samples.sort_unstable();
        self.window_start_us = Some(now_us);
        self.last_tick_us = Some(now_us);

        Some(HostStats {
            capture_fps: fps(captured, elapsed_us),
            encode_p50_us: nearest_rank_sorted(&encode_samples, 50).min(u64::from(u32::MAX)) as u32,
            encode_p95_us: nearest_rank_sorted(&encode_samples, 95).min(u64::from(u32::MAX)) as u32,
            cpu_pct,
            path: path as i32,
            derp_region: derp_region.into(),
            send_kbps: kbps(sent_bytes, elapsed_us),
        })
    }
}

impl Default for HostStatsCollector {
    fn default() -> Self {
        Self::new()
    }
}
