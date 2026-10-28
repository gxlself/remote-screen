//! Packet-only adaptive audio buffering. All time comes from `push`; playout
//! advances by one frame per `pop`. Neither operation allocates or reads a clock.

use crate::{FRAME_US, MAX_PACKET};

const FRAME_MS: u32 = (FRAME_US / 1_000) as u32;
const MAX_SAMPLES: usize = 256;
const RESYNC_DISTANCE: i16 = 1_000;
const TALKSPURT_GAP_US: i128 = 50_000;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct JitterConfig {
    pub min_target_ms: u32,
    pub max_target_ms: u32,
    pub window_us: u64,
    pub margin_ms: u32,
    pub overflow_ms: u32,
    pub capacity: usize,
}

impl Default for JitterConfig {
    fn default() -> Self {
        Self {
            min_target_ms: 40,
            max_target_ms: 80,
            window_us: 2_000_000,
            margin_ms: 5,
            overflow_ms: 20,
            capacity: 64,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PushResult {
    Accepted,
    Late,
    Duplicate,
    /// A sequence jump or a timestamp-confirmed new talkspurt starts a new
    /// sequence ordering interval. Already concealed sequences may recur here.
    Resync,
    Invalid,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Pop {
    /// The payload is in `out[..len]`. A discontinuity requires a crossfade.
    Packet {
        seq: u16,
        capture_ts_us: u64,
        len: usize,
        discontinuity: bool,
    },
    /// The next packet, if present, is copied for FEC without consuming it.
    Missing { seq: u16, fec_len: Option<usize> },
    /// Prebuffering or rebuffering; the caller can output silence.
    Empty,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct JitterStats {
    /// Valid incoming packets, including late packets and duplicates.
    pub received: u64,
    pub late: u64,
    pub duplicate: u64,
    /// Missing sequences with a newer packet buffered (not empty underruns).
    pub lost: u64,
    pub resyncs: u64,
    pub underruns: u64,
    /// Buffered packets discarded by overflow or slot capacity pressure.
    pub overflow_drops: u64,
    pub accelerated: u64,
    pub target_ms: u32,
    pub level_ms: u32,
}

#[derive(Clone)]
struct Slot {
    seq: u16,
    capture_ts_us: u64,
    len: usize,
    payload: [u8; MAX_PACKET],
}

impl Default for Slot {
    fn default() -> Self {
        Self {
            seq: 0,
            capture_ts_us: 0,
            len: 0,
            payload: [0; MAX_PACKET],
        }
    }
}

#[derive(Default, Clone, Copy)]
struct Sample {
    recv_us: u64,
    transit: i64,
}

/// Storage is allocated once by `new`. Slots are searched directly, so sequence
/// wraparound does not alias slots even when capacity does not divide 65536.
pub struct JitterBuffer {
    cfg: JitterConfig,
    slots: Vec<Slot>,
    next_seq: Option<u16>,
    // During rebuffering the earliest buffered packet can move backwards, but
    // already output/concealed/discarded sequences must remain late.
    late_before: Option<u16>,
    playing: bool,
    empty_pops: u8,
    high_pops: u8,
    discontinuity: bool,
    overflow_burst: bool,
    last_packet: Option<(u16, u64)>,
    counters: JitterStats,
    target_ms: u32,
    samples: [Sample; MAX_SAMPLES],
    sample_len: usize,
    recv_watermark: u64,
    previous_want_ms: u32,
    lower_since_us: Option<u64>,
    lowered: bool,
}

impl JitterBuffer {
    /// Targets are rounded to whole frames (minimum one frame, maximum 1000
    /// frames). A reversed range is made nonempty and zero capacity becomes one.
    /// Capacity should hold at least `max_target_ms / 10` packets for playout.
    pub fn new(mut cfg: JitterConfig) -> Self {
        let max_ms = RESYNC_DISTANCE as u32 * FRAME_MS;
        cfg.min_target_ms = cfg.min_target_ms.clamp(FRAME_MS, max_ms).div_ceil(FRAME_MS) * FRAME_MS;
        cfg.max_target_ms =
            (cfg.max_target_ms.clamp(cfg.min_target_ms, max_ms) / FRAME_MS) * FRAME_MS;
        cfg.capacity = cfg.capacity.max(1);
        Self {
            cfg,
            slots: vec![Slot::default(); cfg.capacity],
            next_seq: None,
            late_before: None,
            playing: false,
            empty_pops: 0,
            high_pops: 0,
            discontinuity: false,
            overflow_burst: false,
            last_packet: None,
            counters: JitterStats::default(),
            target_ms: cfg.min_target_ms,
            samples: [Sample::default(); MAX_SAMPLES],
            sample_len: 0,
            recv_watermark: 0,
            previous_want_ms: cfg.min_target_ms,
            lower_since_us: None,
            lowered: false,
        }
    }

    /// `recv_us` is the receiver's timestamp; only differences are used, so
    /// capture and receive clocks need not be synchronized.
    pub fn push(
        &mut self,
        seq: u16,
        capture_ts_us: u64,
        payload: &[u8],
        recv_us: u64,
    ) -> PushResult {
        if payload.is_empty() || payload.len() > MAX_PACKET {
            return PushResult::Invalid;
        }
        self.counters.received += 1;
        let mut result = PushResult::Accepted;
        if let Some(next) = self.next_seq {
            let delta = seq_delta(seq, next);
            if !(-RESYNC_DISTANCE..=RESYNC_DISTANCE).contains(&delta) {
                self.start_stream(seq);
                result = PushResult::Resync;
            } else if self
                .late_before
                .is_some_and(|floor| seq_delta(seq, floor) < 0)
            {
                let new_talkspurt = !self.playing
                    && self.last_packet.is_some_and(|(last_seq, last_ts)| {
                        let expected = i128::from(last_ts)
                            + i128::from(seq_delta(seq, last_seq)) * i128::from(FRAME_US);
                        i128::from(capture_ts_us) - expected >= TALKSPURT_GAP_US
                    });
                if new_talkspurt {
                    self.start_stream(seq);
                    result = PushResult::Resync;
                } else {
                    self.counters.late += 1;
                    self.raise_target();
                    return PushResult::Late;
                }
            } else if delta < 0 {
                // A reordered packet which has not been output or discarded
                // can still become the start of the prebuffer.
                self.next_seq = Some(seq);
            }
        } else {
            self.next_seq = Some(seq);
        }
        if self.find(seq).is_some() {
            self.counters.duplicate += 1;
            return PushResult::Duplicate;
        }

        self.sample(capture_ts_us, recv_us);
        // A small custom capacity also stays bounded. Keep the newest packets,
        // and never allocate a replacement slot on the receive path.
        let index = if let Some(index) = self.slots.iter().position(|slot| slot.len == 0) {
            index
        } else {
            let next = self.next_seq.unwrap_or(seq);
            let oldest = self
                .slots
                .iter()
                .enumerate()
                .min_by_key(|(_, slot)| seq_delta(slot.seq, next))
                .map(|(index, _)| index)
                .unwrap_or(0);
            self.counters.overflow_drops += 1;
            self.discontinuity = true;
            if seq_delta(seq, self.slots[oldest].seq) <= 0 {
                self.next_seq = Some(self.slots[oldest].seq);
                self.late_before = self.next_seq;
                return result;
            }
            self.next_seq = Some(self.slots[oldest].seq.wrapping_add(1));
            self.late_before = self.next_seq;
            self.slots[oldest].len = 0;
            oldest
        };
        let slot = &mut self.slots[index];
        slot.seq = seq;
        slot.capture_ts_us = capture_ts_us;
        slot.len = payload.len();
        slot.payload[..payload.len()].copy_from_slice(payload);
        self.start_at_oldest();
        self.trim_overflow();
        self.start_at_oldest();
        result
    }

    /// Called once per 10 ms output frame. `out` must hold `MAX_PACKET` bytes.
    /// A shorter slice returns `Empty` without advancing or consuming anything.
    pub fn pop(&mut self, out: &mut [u8]) -> Pop {
        if out.len() < MAX_PACKET {
            return Pop::Empty;
        }
        self.overflow_burst = false;
        if !self.playing {
            if self.level_ms() < self.target_ms {
                return Pop::Empty;
            }
            self.playing = true;
            self.empty_pops = 0;
        }

        if self.level_ms() > self.target_ms.saturating_add(20) {
            self.high_pops += 1;
            if self.high_pops == 100 {
                if let Some(next) = self.next_seq {
                    if let Some(index) = self.find(next) {
                        self.slots[index].len = 0;
                    }
                    self.next_seq = Some(next.wrapping_add(1));
                    self.late_before = self.next_seq;
                }
                self.high_pops = 0;
                self.counters.accelerated += 1;
                self.discontinuity = true;
            }
        } else {
            self.high_pops = 0;
        }

        let Some(seq) = self.next_seq else {
            return Pop::Empty;
        };
        if let Some(index) = self.find(seq) {
            let slot = &mut self.slots[index];
            let len = slot.len;
            let capture_ts_us = slot.capture_ts_us;
            out[..len].copy_from_slice(&slot.payload[..len]);
            slot.len = 0;
            self.next_seq = Some(seq.wrapping_add(1));
            self.late_before = self.next_seq;
            self.last_packet = Some((seq, capture_ts_us));
            self.empty_pops = 0;
            let discontinuity = self.discontinuity;
            self.discontinuity = false;
            return Pop::Packet {
                seq,
                capture_ts_us,
                len,
                discontinuity,
            };
        }

        if self.level_ms() == 0 {
            if self.empty_pops == 3 {
                self.playing = false;
                self.empty_pops = 0;
                self.high_pops = 0;
                return Pop::Empty;
            }
            self.empty_pops += 1;
            self.counters.underruns += 1;
            self.raise_target();
        } else {
            self.empty_pops = 0;
            self.counters.lost += 1;
        }
        let fec_len = self.find(seq.wrapping_add(1)).map(|index| {
            let slot = &self.slots[index];
            out[..slot.len].copy_from_slice(&slot.payload[..slot.len]);
            slot.len
        });
        self.next_seq = Some(seq.wrapping_add(1));
        self.late_before = self.next_seq;
        self.discontinuity = true;
        Pop::Missing { seq, fec_len }
    }

    /// Start a new connection, including fresh statistics and target history.
    pub fn reset(&mut self) {
        self.clear_packets();
        self.next_seq = None;
        self.discontinuity = false;
        self.counters = JitterStats::default();
        self.target_ms = self.cfg.min_target_ms;
        self.clear_samples();
    }

    pub fn target_ms(&self) -> u32 {
        self.target_ms
    }

    /// Inclusive sequence span from the next playout sequence to the highest
    /// buffered sequence, including holes. An empty buffer has zero level.
    pub fn level_ms(&self) -> u32 {
        let Some(next) = self.next_seq else {
            return 0;
        };
        self.slots
            .iter()
            .filter(|slot| slot.len != 0)
            .map(|slot| i32::from(seq_delta(slot.seq, next)) + 1)
            .max()
            .unwrap_or(0)
            .max(0) as u32
            * FRAME_MS
    }

    pub fn stats(&self) -> JitterStats {
        JitterStats {
            target_ms: self.target_ms(),
            level_ms: self.level_ms(),
            ..self.counters
        }
    }

    fn find(&self, seq: u16) -> Option<usize> {
        self.slots
            .iter()
            .position(|slot| slot.len != 0 && slot.seq == seq)
    }

    fn clear_packets(&mut self) {
        for slot in &mut self.slots {
            slot.len = 0;
        }
        self.playing = false;
        self.empty_pops = 0;
        self.high_pops = 0;
        self.overflow_burst = false;
        self.last_packet = None;
        self.late_before = None;
    }

    fn start_at_oldest(&mut self) {
        if self.playing {
            return;
        }
        if let Some(next) = self.next_seq {
            self.next_seq = self
                .slots
                .iter()
                .filter(|slot| slot.len != 0)
                .min_by_key(|slot| seq_delta(slot.seq, next))
                .map(|slot| slot.seq)
                .or(self.next_seq);
        }
    }

    fn clear_samples(&mut self) {
        self.sample_len = 0;
        self.recv_watermark = 0;
        self.previous_want_ms = self.target_ms;
        self.lower_since_us = None;
        self.lowered = false;
    }

    fn start_stream(&mut self, seq: u16) {
        self.clear_packets();
        self.clear_samples();
        self.next_seq = Some(seq);
        self.counters.resyncs += 1;
        self.discontinuity = true;
    }

    fn raise_target(&mut self) {
        self.target_ms = self
            .target_ms
            .saturating_add(FRAME_MS)
            .min(self.cfg.max_target_ms);
        self.lower_since_us = None;
        self.lowered = false;
    }

    fn trim_overflow(&mut self) {
        let level = self.level_ms();
        if level > self.cfg.max_target_ms.saturating_add(self.cfg.overflow_ms) {
            self.overflow_burst = true;
        }
        // Keep trimming the remainder of this burst until the next playout
        // tick, rather than letting the same overflowing burst grow again.
        if !self.overflow_burst || level <= self.target_ms {
            return;
        }
        let skip = (level - self.target_ms) / FRAME_MS;
        if let Some(next) = self.next_seq {
            let new_next = next.wrapping_add(skip as u16);
            for slot in &mut self.slots {
                if slot.len != 0 && seq_delta(slot.seq, new_next) < 0 {
                    slot.len = 0;
                    self.counters.overflow_drops += 1;
                }
            }
            self.next_seq = Some(new_next);
            self.late_before = self.next_seq;
        }
        self.discontinuity = true;
        self.high_pops = 0;
    }

    fn sample(&mut self, capture_ts_us: u64, recv_us: u64) {
        self.recv_watermark = self.recv_watermark.max(recv_us);
        let now = self.recv_watermark;
        let mut len = 0;
        for index in 0..self.sample_len {
            let sample = self.samples[index];
            if now.saturating_sub(sample.recv_us) <= self.cfg.window_us {
                self.samples[len] = sample;
                len += 1;
            }
        }
        self.sample_len = len;
        let transit = (i128::from(recv_us) - i128::from(capture_ts_us))
            .clamp(i128::from(i64::MIN), i128::from(i64::MAX)) as i64;
        if now.saturating_sub(recv_us) <= self.cfg.window_us {
            let index = if len < MAX_SAMPLES {
                self.sample_len += 1;
                len
            } else {
                self.samples
                    .iter()
                    .enumerate()
                    .min_by_key(|(_, sample)| sample.recv_us)
                    .map(|(index, _)| index)
                    .unwrap_or(0)
            };
            if len < MAX_SAMPLES || recv_us >= self.samples[index].recv_us {
                // Preserve insertion order for equal receive timestamps, so a
                // large same-timestamp burst still retains its latest samples.
                if len == MAX_SAMPLES {
                    self.samples.copy_within(index + 1..MAX_SAMPLES, index);
                    self.samples[MAX_SAMPLES - 1] = Sample { recv_us, transit };
                } else {
                    self.samples[index] = Sample { recv_us, transit };
                }
            }
        }

        let mut transits = [0_i64; MAX_SAMPLES];
        for (transit, sample) in transits.iter_mut().zip(&self.samples[..self.sample_len]) {
            *transit = sample.transit;
        }
        let transits = &mut transits[..self.sample_len];
        transits.sort_unstable();
        let spread = if transits.is_empty() {
            0
        } else {
            let p95 = (transits.len() * 95).div_ceil(100) - 1;
            (i128::from(transits[p95]) - i128::from(transits[0])) as u64
        };
        let want_us = spread
            .saturating_add(FRAME_US)
            .saturating_add(u64::from(self.cfg.margin_ms) * 1_000);
        let want_ms = want_us
            .div_ceil(FRAME_US)
            .saturating_mul(u64::from(FRAME_MS))
            .clamp(
                u64::from(self.cfg.min_target_ms),
                u64::from(self.cfg.max_target_ms),
            ) as u32;
        if want_ms >= self.target_ms || want_ms > self.previous_want_ms {
            self.lower_since_us = None;
            self.lowered = false;
        }
        if want_ms > self.target_ms {
            self.target_ms = want_ms;
        } else if want_ms < self.target_ms {
            let since = *self.lower_since_us.get_or_insert(now);
            let wait_us = if self.lowered { 2_000_000 } else { 5_000_000 };
            if now.saturating_sub(since) >= wait_us {
                self.target_ms = (self.target_ms - FRAME_MS).max(want_ms);
                self.lower_since_us = Some(now);
                self.lowered = true;
            }
        }
        self.previous_want_ms = want_ms;
    }
}

// Keep rs-audio independent of rsp-core; AU1B's helper is not available yet.
fn seq_delta(a: u16, b: u16) -> i16 {
    a.wrapping_sub(b) as i16
}
