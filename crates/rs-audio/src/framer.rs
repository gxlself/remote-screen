//! Split captured PCM blocks into 10 ms stereo frames.
//!
//! Capture callbacks choose the block size; the wire protocol requires one
//! 48 kHz stereo frame per packet. A frame's timestamp is the collection time
//! of its first sample, derived from the block timestamp and sample offset.

use crate::{AudioError, CHANNELS, FRAME_LEN, FRAME_SAMPLES, SAMPLE_RATE};

/// A block timestamp within this distance of the sample-derived timeline is
/// treated as clock jitter. Only differences strictly greater than this value
/// are discontinuities.
const CLOCK_TOLERANCE_US: u64 = 20_000;

const US_PER_SECOND: u64 = 1_000_000;

/// One 10 ms interleaved stereo frame ready for encoding.
#[derive(Clone)]
pub struct AudioFrame {
    /// Collection time of this frame's first sample (Host unix microseconds).
    pub capture_ts_us: u64,
    pub pcm: Box<[f32; FRAME_LEN]>,
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct FramerStats {
    pub frames: u64,
    pub discontinuities: u64,
    /// Per-channel sample frames discarded at discontinuities.
    pub dropped_samples: u64,
}

pub struct Framer {
    /// Stereo interleaved samples waiting for the next complete frame.
    pending: Vec<f32>,
    /// Block timestamp that starts the current sample-derived timeline.
    anchor_ts_us: u64,
    /// Samples (per channel) received on the current timeline.
    timeline_samples: u64,
    /// Samples (per channel) already consumed from the current timeline.
    consumed_samples: u64,
    have_timeline: bool,
    stats: FramerStats,
}

impl Framer {
    pub fn new() -> Self {
        Self {
            pending: Vec::with_capacity(FRAME_LEN),
            anchor_ts_us: 0,
            timeline_samples: 0,
            consumed_samples: 0,
            have_timeline: false,
            stats: FramerStats::default(),
        }
    }

    /// Push one captured PCM block. `samples` is interleaved by `channels`.
    ///
    /// The input sample rate is fixed at 48 kHz. Invalid input leaves the
    /// framer state unchanged.
    pub fn push(
        &mut self,
        capture_ts_us: u64,
        channels: u8,
        samples: &[f32],
    ) -> Result<Vec<AudioFrame>, AudioError> {
        let channels = usize::from(channels);
        if channels == 0 {
            return Err(AudioError::Invalid("channels"));
        }
        if !samples.len().is_multiple_of(channels) {
            return Err(AudioError::Invalid("sample count"));
        }

        let block_frames = samples.len() / channels;
        if block_frames == 0 {
            return Ok(Vec::new());
        }

        let expected_ts_us = self
            .have_timeline
            .then(|| self.timestamp_at(self.timeline_samples));
        let discontinuity = expected_ts_us
            .is_some_and(|expected| capture_ts_us.abs_diff(expected) > CLOCK_TOLERANCE_US);

        if discontinuity {
            self.stats.discontinuities = self.stats.discontinuities.saturating_add(1);
            self.stats.dropped_samples = self
                .stats
                .dropped_samples
                .saturating_add(self.pending_frames() as u64);
            self.pending.clear();
            self.anchor_ts_us = capture_ts_us;
            self.timeline_samples = 0;
            self.consumed_samples = 0;
            self.have_timeline = true;
        } else if !self.have_timeline {
            self.anchor_ts_us = capture_ts_us;
            self.timeline_samples = 0;
            self.consumed_samples = 0;
            self.have_timeline = true;
        }

        self.timeline_samples = self.timeline_samples.saturating_add(block_frames as u64);
        Ok(self.append_block(channels, samples))
    }

    /// Discard buffered samples. The first block after this call starts a new
    /// timeline and is never reported as a discontinuity.
    pub fn reset(&mut self) {
        self.pending.clear();
        self.have_timeline = false;
    }

    pub fn stats(&self) -> FramerStats {
        self.stats
    }

    fn pending_frames(&self) -> usize {
        self.pending.len() / CHANNELS
    }

    fn timestamp_at(&self, samples: u64) -> u64 {
        self.anchor_ts_us.saturating_add(samples_us(samples))
    }

    fn append_block(&mut self, channels: usize, samples: &[f32]) -> Vec<AudioFrame> {
        let block_frames = samples.len() / channels;
        let mut frames =
            Vec::with_capacity((self.pending_frames() + block_frames).div_ceil(FRAME_SAMPLES));
        let mut consumed = 0;

        while consumed < block_frames {
            let room = FRAME_SAMPLES - self.pending_frames();
            let take = room.min(block_frames - consumed);
            let start = consumed * channels;
            let end = (consumed + take) * channels;
            append_as_stereo(&mut self.pending, &samples[start..end], channels);
            consumed += take;

            if self.pending_frames() == FRAME_SAMPLES {
                let frame_ts_us = self.timestamp_at(self.consumed_samples);
                let mut pcm = Box::new([0.0; FRAME_LEN]);
                for (out, sample) in pcm.iter_mut().zip(&self.pending) {
                    *out = sanitize(*sample);
                }
                frames.push(AudioFrame {
                    capture_ts_us: frame_ts_us,
                    pcm,
                });
                self.stats.frames = self.stats.frames.saturating_add(1);
                self.pending.clear();
                self.consumed_samples = self.consumed_samples.saturating_add(FRAME_SAMPLES as u64);
            }
        }

        frames
    }
}

impl Default for Framer {
    fn default() -> Self {
        Self::new()
    }
}

fn samples_us(samples: u64) -> u64 {
    samples.saturating_mul(US_PER_SECOND) / u64::from(SAMPLE_RATE)
}

fn append_as_stereo(pending: &mut Vec<f32>, samples: &[f32], channels: usize) {
    match channels {
        1 => {
            for &sample in samples {
                pending.push(sample);
                pending.push(sample);
            }
        }
        2 => pending.extend_from_slice(samples),
        _ => {
            for sample in samples.chunks_exact(channels) {
                pending.push(sample[0]);
                pending.push(sample[1]);
            }
        }
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
    use super::*;

    #[test]
    fn sanitizes_and_clamps_samples() {
        assert_eq!(sanitize(f32::NAN), 0.0);
        assert_eq!(sanitize(f32::INFINITY), 0.0);
        assert_eq!(sanitize(f32::NEG_INFINITY), 0.0);
        assert_eq!(sanitize(2.0), 1.0);
        assert_eq!(sanitize(-3.0), -1.0);
        assert_eq!(sanitize(0.5), 0.5);
    }
}
