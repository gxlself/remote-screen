//! 48 kHz stereo playout of raw PCM (adapter sessions: RDP rdpsnd, ).
//!
//! Interleaved i16 input at any rate is resampled by linear interpolation as it
//! is pushed; mono is copied to both channels and channels beyond the first two
//! are dropped. Playback starts once 80 ms are buffered; an underrun outputs
//! silence until 80 ms are buffered again; a push that leaves more than 300 ms
//! buffered drops the oldest audio down to the newest 80 ms. Pulls have the
//! same shape as [`crate::playout::AudioPlayout`]. The caller owns
//! synchronization; storage is created by `new`, so neither pushes nor pulls
//! allocate.

use crate::jitter::JitterStats;
use crate::playout::PlayoutStats;
use crate::{CHANNELS, FRAME_SAMPLES, SAMPLE_RATE};

/// Buffered audio needed before playback starts or resumes after an underrun.
pub const PCM_TARGET_MS: u32 = 80;
/// Buffered audio above this after a push is trimmed to [`PCM_TARGET_MS`].
pub const PCM_MAX_MS: u32 = 300;

const TARGET_FRAMES: usize = (SAMPLE_RATE * PCM_TARGET_MS / 1000) as usize;
const MAX_FRAMES: usize = (SAMPLE_RATE * PCM_MAX_MS / 1000) as usize;
// One frame over the limit: a push that overflows the ring has kept its newest
// frames and is then trimmed like any other backlog.
const CAPACITY: usize = MAX_FRAMES + 1;
/// Mute ramp per channel: 5 ms, as `PlayoutConfig::default().fade_samples`.
const FADE_SAMPLES: usize = 240;

/// A resampler, a fixed ring of 48 kHz stereo frames, and a mute ramp.
pub struct PcmPlayout {
    rate: u32,
    channels: usize,
    // The last input frame; the next output frame lies pos / SAMPLE_RATE input
    // frames after it.
    prev: [f32; CHANNELS],
    pos: u64,
    ring: Box<[f32]>,
    head: usize,
    len: usize,
    playing: bool,
    muted: bool,
    volume: Ramp,
    pushes: u64,
    underruns: u64,
    trims: u64,
    played: u64,
    silent: u64,
}

impl Default for PcmPlayout {
    fn default() -> Self {
        Self::new()
    }
}

impl PcmPlayout {
    /// Starts as 48 kHz stereo input.
    pub fn new() -> Self {
        Self {
            rate: SAMPLE_RATE,
            channels: CHANNELS,
            prev: [0.0; CHANNELS],
            pos: u64::from(SAMPLE_RATE),
            ring: vec![0.0; CAPACITY * CHANNELS].into_boxed_slice(),
            head: 0,
            len: 0,
            playing: false,
            muted: false,
            volume: Ramp::new(1.0),
            pushes: 0,
            underruns: 0,
            trims: 0,
            played: 0,
            silent: 0,
        }
    }

    /// Input format of later pushes. Audio already buffered is kept. A zero
    /// rate or channel count ignores pushes until the next valid format.
    pub fn set_format(&mut self, sample_rate: u32, channels: u16) {
        let channels = usize::from(channels);
        if (sample_rate, channels) != (self.rate, self.channels) {
            self.rate = sample_rate;
            self.channels = channels;
            self.restart_resampler();
        }
    }

    /// Interleaved samples in the current format; a trailing partial frame is
    /// ignored.
    pub fn push(&mut self, pcm: &[i16]) {
        if self.rate == 0 || self.channels == 0 || pcm.len() < self.channels {
            return;
        }
        for frame in pcm.chunks_exact(self.channels) {
            let left = f32::from(frame[0]) / 32768.0;
            let right = frame
                .get(1)
                .map_or(left, |&sample| f32::from(sample) / 32768.0);
            self.resample([left, right]);
        }
        self.pushes += 1;
        if self.len > MAX_FRAMES {
            self.discard(self.len - TARGET_FRAMES);
            self.trims += 1;
        }
    }

    /// Remote audio stopped: drop everything buffered. The format and the mute
    /// state are kept; the next audio prebuffers again.
    pub fn stop(&mut self) {
        self.head = 0;
        self.len = 0;
        self.playing = false;
        self.restart_resampler();
    }

    /// Fill any number of stereo frames, including zero. For mismatched slices,
    /// only the common prefix consumes audio; the longer tail is zeroed.
    pub fn pull_planar(&mut self, left: &mut [f32], right: &mut [f32]) -> usize {
        let len = left.len().min(right.len());
        for index in 0..len {
            let (l, r) = self.next_sample();
            left[index] = l;
            right[index] = r;
        }
        left[len..].fill(0.0);
        right[len..].fill(0.0);
        len
    }

    /// Fill interleaved stereo frames. An odd trailing sample is zeroed without
    /// consuming audio.
    pub fn pull_interleaved(&mut self, out: &mut [f32]) -> usize {
        let frames = out.len() / CHANNELS;
        for pair in out[..frames * CHANNELS].chunks_exact_mut(CHANNELS) {
            let (left, right) = self.next_sample();
            pair[0] = left;
            pair[1] = right;
        }
        out[frames * CHANNELS..].fill(0.0);
        frames
    }

    /// Linear 5 ms ramp; muted output still consumes buffered audio.
    pub fn set_muted(&mut self, muted: bool) {
        if self.muted != muted {
            self.muted = muted;
            self.volume.to(if muted { 0.0 } else { 1.0 }, FADE_SAMPLES);
        }
    }

    /// `jitter.received` counts pushes, `jitter.overflow_drops` backlog trims,
    /// and `decoded` / `silent_frames` 10 ms of output with and without
    /// buffered audio. Codec, loss, and timestamp fields stay zero.
    pub fn stats(&self) -> PlayoutStats {
        PlayoutStats {
            jitter: JitterStats {
                received: self.pushes,
                underruns: self.underruns,
                overflow_drops: self.trims,
                target_ms: PCM_TARGET_MS,
                level_ms: (self.len * 1000 / SAMPLE_RATE as usize) as u32,
                ..JitterStats::default()
            },
            decoded: self.played / FRAME_SAMPLES as u64,
            silent_frames: self.silent / FRAME_SAMPLES as u64,
            playing: self.playing,
            muted: self.muted,
            ..PlayoutStats::default()
        }
    }

    fn restart_resampler(&mut self) {
        // The first frame after a restart is output as is.
        self.prev = [0.0; CHANNELS];
        self.pos = u64::from(SAMPLE_RATE);
    }

    fn resample(&mut self, frame: [f32; CHANNELS]) {
        let one = u64::from(SAMPLE_RATE);
        while self.pos <= one {
            let t = self.pos as f32 / SAMPLE_RATE as f32;
            self.write([
                lerp(self.prev[0], frame[0], t),
                lerp(self.prev[1], frame[1], t),
            ]);
            self.pos += u64::from(self.rate);
        }
        self.pos -= one;
        self.prev = frame;
    }

    fn write(&mut self, frame: [f32; CHANNELS]) {
        if self.len == CAPACITY {
            self.discard(1);
        }
        let offset = ((self.head + self.len) % CAPACITY) * CHANNELS;
        self.ring[offset..offset + CHANNELS].copy_from_slice(&frame);
        self.len += 1;
    }

    fn discard(&mut self, frames: usize) {
        self.head = (self.head + frames) % CAPACITY;
        self.len -= frames;
    }

    fn next_sample(&mut self) -> (f32, f32) {
        let gain = self.volume.next();
        if !self.playing && self.len >= TARGET_FRAMES {
            self.playing = true;
        }
        if self.playing && self.len == 0 {
            self.playing = false;
            self.underruns += 1;
        }
        if !self.playing {
            self.silent += 1;
            return (0.0, 0.0);
        }
        let offset = self.head * CHANNELS;
        let (left, right) = (self.ring[offset], self.ring[offset + 1]);
        self.discard(1);
        self.played += 1;
        (left * gain, right * gain)
    }
}

// Exact at both ends: t == 1 yields `to` unchanged.
fn lerp(from: f32, to: f32, t: f32) -> f32 {
    from * (1.0 - t) + to * t
}

// Same envelope as playout.rs: `to` reaches its target on the last step.
struct Ramp {
    value: f32,
    target: f32,
    step: f32,
    remaining: usize,
}

impl Ramp {
    fn new(value: f32) -> Self {
        Self {
            value,
            target: value,
            step: 0.0,
            remaining: 0,
        }
    }

    fn to(&mut self, target: f32, samples: usize) {
        self.target = target;
        self.remaining = samples;
        if samples == 0 {
            self.value = target;
            self.step = 0.0;
        } else {
            self.step = (target - self.value) / samples as f32;
        }
    }

    fn next(&mut self) -> f32 {
        if self.remaining != 0 {
            self.remaining -= 1;
            self.value = if self.remaining == 0 {
                self.target
            } else {
                (self.value + self.step).clamp(0.0, 1.0)
            };
        }
        self.value
    }
}
