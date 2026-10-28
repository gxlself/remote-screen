//! Platform-independent 48 kHz stereo playout.
//!
//! Pulls keep a partially consumed 10 ms frame, so callback sizes do not change
//! decoding, fades, or packet ordering. Fades and crossfades are linear, with
//! lengths measured in samples **per channel**. The caller owns synchronization;
//! this type neither locks nor logs. Storage is created by `new` and reused.
//!
//! Malformed-packet handling is allocation-free too, as guaranteed by
//! `tests/playout_alloc.rs`.

use crate::codec::AudioDecoder;
use crate::jitter::{JitterBuffer, JitterConfig, JitterStats, Pop, PushResult};
use crate::{AudioError, CHANNELS, FRAME_LEN, FRAME_SAMPLES, MAX_PACKET};

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PlayoutConfig {
    pub jitter: JitterConfig,
    /// Linear crossfade length per channel; clamped to one 10 ms frame.
    pub crossfade_samples: usize,
    /// Linear start/stop and mute ramp length per channel. Zero disables ramps.
    pub fade_samples: usize,
}

impl Default for PlayoutConfig {
    fn default() -> Self {
        Self {
            jitter: JitterConfig::default(),
            crossfade_samples: 120,
            fade_samples: 240,
        }
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct PlayoutStats {
    pub jitter: JitterStats,
    pub decoded: u64,
    pub concealed: u64,
    pub fec_recovered: u64,
    pub decode_errors: u64,
    pub silent_frames: u64,
    /// Whether the current frame came from a packet or a missing sequence.
    pub playing: bool,
    /// Requested mute state, including while its ramp is still running.
    pub muted: bool,
    /// Capture time of the current decoded packet; PLC, FEC, and silence have
    /// no packet timestamp. Muting does not change this media timestamp.
    pub current_capture_ts_us: Option<u64>,
}

/// A packet queue, Opus decoder, and sample-driven output envelopes.
pub struct AudioPlayout {
    cfg: PlayoutConfig,
    jitter: JitterBuffer,
    decoder: AudioDecoder,
    // Mirrors the media operations, with additional speculative PLC at a
    // discontinuity. Prediction must never advance the main decoder's clock.
    predictor: AudioDecoder,
    packet: [u8; MAX_PACKET],
    frame: [f32; FRAME_LEN],
    continuation: [f32; FRAME_LEN],
    cursor: usize,
    source: Ramp,
    volume: Ramp,
    counters: PlayoutStats,
}

impl AudioPlayout {
    pub fn new(mut cfg: PlayoutConfig) -> Result<Self, AudioError> {
        cfg.crossfade_samples = cfg.crossfade_samples.min(FRAME_SAMPLES);
        Ok(Self {
            cfg,
            jitter: JitterBuffer::new(cfg.jitter),
            decoder: AudioDecoder::new()?,
            predictor: AudioDecoder::new()?,
            packet: [0; MAX_PACKET],
            frame: [0.0; FRAME_LEN],
            continuation: [0.0; FRAME_LEN],
            cursor: FRAME_SAMPLES,
            source: Ramp::new(0.0),
            volume: Ramp::new(1.0),
            counters: PlayoutStats::default(),
        })
    }

    pub fn push(
        &mut self,
        seq: u16,
        capture_ts_us: u64,
        payload: &[u8],
        recv_us: u64,
    ) -> PushResult {
        self.jitter.push(seq, capture_ts_us, payload, recv_us)
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
    /// advancing the media cursor.
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

    pub fn set_muted(&mut self, muted: bool) {
        if self.counters.muted != muted {
            self.counters.muted = muted;
            self.volume
                .to(if muted { 0.0 } else { 1.0 }, self.cfg.fade_samples);
        }
    }

    /// Discard the current segment and statistics, retaining the user's mute
    /// preference. The next segment prebuffers and fades in from silence.
    pub fn reset(&mut self) {
        self.jitter.reset();
        // A valid, owned libopus decoder accepts OPUS_RESET_STATE. Keep this API
        // infallible, matching JitterBuffer::reset and the public contract.
        let _ = self.decoder.reset();
        let _ = self.predictor.reset();
        self.packet.fill(0);
        self.frame.fill(0.0);
        self.continuation.fill(0.0);
        self.cursor = FRAME_SAMPLES;
        self.source = Ramp::new(0.0);
        let muted = self.counters.muted;
        self.volume = Ramp::new(if muted { 0.0 } else { 1.0 });
        self.counters = PlayoutStats {
            muted,
            ..PlayoutStats::default()
        };
    }

    pub fn stats(&self) -> PlayoutStats {
        PlayoutStats {
            jitter: self.jitter.stats(),
            ..self.counters
        }
    }

    fn next_sample(&mut self) -> (f32, f32) {
        if self.cursor == FRAME_SAMPLES {
            self.next_frame();
        }
        let gain = self.source.next() * self.volume.next();
        let offset = self.cursor * CHANNELS;
        self.cursor += 1;
        (
            sanitize(self.frame[offset] * gain),
            sanitize(self.frame[offset + 1] * gain),
        )
    }

    fn next_frame(&mut self) {
        let was_playing = self.counters.playing;
        self.counters.current_capture_ts_us = None;
        let mut advance = Advance::None;
        match self.jitter.pop(&mut self.packet) {
            Pop::Packet {
                capture_ts_us,
                len,
                discontinuity,
                ..
            } => {
                let crossfade = discontinuity && was_playing && self.cfg.crossfade_samples != 0;
                if crossfade && self.predictor.conceal(&mut self.continuation).is_err() {
                    self.counters.decode_errors += 1;
                    self.continuation.fill(0.0);
                }
                if self
                    .decoder
                    .decode(&self.packet[..len], &mut self.frame)
                    .is_ok()
                {
                    self.counters.decoded += 1;
                    self.counters.current_capture_ts_us = Some(capture_ts_us);
                    advance = Advance::Decode(len);
                } else {
                    self.counters.decode_errors += 1;
                    self.conceal();
                    advance = Advance::Conceal;
                }
                if crossfade {
                    for index in 0..self.cfg.crossfade_samples {
                        let mix = (index + 1) as f32 / self.cfg.crossfade_samples as f32;
                        for channel in 0..CHANNELS {
                            let offset = index * CHANNELS + channel;
                            self.frame[offset] = sanitize(self.continuation[offset]) * (1.0 - mix)
                                + sanitize(self.frame[offset]) * mix;
                        }
                    }
                }
                self.counters.playing = true;
            }
            Pop::Missing { fec_len, .. } => {
                if let Some(len) = fec_len {
                    if self
                        .decoder
                        .recover(&self.packet[..len], &mut self.frame)
                        .is_ok()
                    {
                        self.counters.fec_recovered += 1;
                        advance = Advance::Recover(len);
                    } else {
                        self.counters.decode_errors += 1;
                        self.conceal();
                        advance = Advance::Conceal;
                    }
                } else {
                    self.conceal();
                    advance = Advance::Conceal;
                }
                self.counters.playing = true;
            }
            Pop::Empty => {
                self.counters.silent_frames += 1;
                self.counters.playing = false;
                if self.source.value > 0.0 && self.cfg.fade_samples != 0 {
                    advance = Advance::Conceal;
                    if self.decoder.conceal(&mut self.frame).is_err() {
                        self.counters.decode_errors += 1;
                        self.frame.fill(0.0);
                    }
                } else {
                    self.frame.fill(0.0);
                }
            }
        }
        // Keep the prediction decoder on the same packet/PLC/FEC sequence.
        // After speculative PLC its state converges through subsequent media
        // operations; its samples are used only for the brief crossfade. Errors
        // and media statistics are counted on the main decode, not this mirror.
        let _ = match advance {
            Advance::Decode(len) => self
                .predictor
                .decode(&self.packet[..len], &mut self.continuation),
            Advance::Recover(len) => self
                .predictor
                .recover(&self.packet[..len], &mut self.continuation),
            Advance::Conceal => self.predictor.conceal(&mut self.continuation),
            Advance::None => Ok(FRAME_SAMPLES),
        };
        if self.counters.playing != was_playing {
            self.source.to(
                if self.counters.playing { 1.0 } else { 0.0 },
                self.cfg.fade_samples,
            );
        }
        self.cursor = 0;
    }

    fn conceal(&mut self) {
        self.counters.concealed += 1;
        if self.decoder.conceal(&mut self.frame).is_err() {
            self.counters.decode_errors += 1;
            self.frame.fill(0.0);
        }
    }
}

enum Advance {
    Decode(usize),
    Recover(usize),
    Conceal,
    None,
}

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

fn sanitize(sample: f32) -> f32 {
    if sample.is_finite() {
        sample.clamp(-1.0, 1.0)
    } else {
        0.0
    }
}

#[cfg(test)]
mod tests {
    use super::sanitize;

    #[test]
    fn output_is_finite_and_limited() {
        assert_eq!(sanitize(f32::NAN), 0.0);
        assert_eq!(sanitize(f32::INFINITY), 0.0);
        assert_eq!(sanitize(f32::NEG_INFINITY), 0.0);
        assert_eq!(sanitize(1.5), 1.0);
        assert_eq!(sanitize(-1.5), -1.0);
        assert_eq!(sanitize(0.25), 0.25);
    }
}
