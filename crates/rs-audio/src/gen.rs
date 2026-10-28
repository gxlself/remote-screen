//! Deterministic audio test signals driven by absolute sample numbers.
//!
//! The generator is deliberately stateless: a sample's value depends only on
//! its absolute sample index. Callers that produce a continuous stream should
//! advance by sample index and derive timestamps with [`sample_ts_us`]. That
//! keeps chunk boundaries from shifting by a sample after timestamp rounding.

use std::f64::consts::TAU;

use crate::SAMPLE_RATE;

const SAMPLE_RATE_U64: u64 = SAMPLE_RATE as u64;
const MICROS_PER_SECOND: u64 = 1_000_000;

/// A deterministic test signal.
#[derive(Debug, Clone, Copy, PartialEq)]
pub enum Signal {
    Silence,
    Sine {
        freq_hz: u32,
        amp: f32,
    },
    /// Emit a sine wave while `n % period_samples < on_samples`, otherwise silence.
    ///
    /// A burst always starts at an integer multiple of `period_us`. The sine
    /// phase is based on the absolute sample index, so it stays continuous
    /// across render calls. `period_us` must map to a whole number of samples;
    /// otherwise the entire signal is treated as [`Signal::Silence`]. The
    /// on-duration is mapped to samples with floor rounding.
    Bursts {
        freq_hz: u32,
        amp: f32,
        period_us: u64,
        on_us: u64,
    },
    /// Uniform white noise in `[-amp, amp]`, hashed from `(seed, sample index)`.
    Noise {
        amp: f32,
        seed: u64,
    },
}

impl Signal {
    /// Fill `out` with samples starting at absolute sample index `index`.
    ///
    /// Samples are interleaved and every channel receives the same value. If
    /// `channels` is zero or `out` is not a whole number of frames, only full
    /// frames are filled and the remaining elements are set to zero.
    pub fn render_index(&self, index: u64, channels: usize, out: &mut [f32]) {
        let frames = out.len().checked_div(channels).unwrap_or(0);
        let filled = frames.saturating_mul(channels);

        match *self {
            Signal::Silence => out[..filled].fill(0.0),
            Signal::Sine { freq_hz, amp } => {
                for frame in 0..frames {
                    let sample = sine_sample(freq_hz, amp, index.saturating_add(frame as u64));
                    out[frame * channels..(frame + 1) * channels].fill(sample);
                }
            }
            Signal::Bursts {
                freq_hz,
                amp,
                period_us,
                on_us,
            } => {
                if let Some((period_samples, on_samples)) = burst_samples(period_us, on_us) {
                    for frame in 0..frames {
                        let sample_index = index.saturating_add(frame as u64);
                        let sample = if sample_index % period_samples < on_samples {
                            sine_sample(freq_hz, amp, sample_index)
                        } else {
                            0.0
                        };
                        out[frame * channels..(frame + 1) * channels].fill(sample);
                    }
                } else {
                    out[..filled].fill(0.0);
                }
            }
            Signal::Noise { amp, seed } => {
                for frame in 0..frames {
                    let sample_index = index.saturating_add(frame as u64);
                    let sample = noise_sample(amp, seed, sample_index);
                    out[frame * channels..(frame + 1) * channels].fill(sample);
                }
            }
        }

        out[filled..].fill(0.0);
    }

    /// Render starting at Unix timestamp `start_ts_us`.
    ///
    /// The first sample uses [`sample_index`] of the timestamp. This method is
    /// suitable for one-off renders; continuous producers should use
    /// [`Signal::render_index`] with a sample counter.
    pub fn render(&self, start_ts_us: u64, channels: usize, out: &mut [f32]) {
        self.render_index(sample_index(start_ts_us), channels, out);
    }
}

/// Convert Unix microseconds to an absolute 48 kHz sample index, rounding down.
pub fn sample_index(ts_us: u64) -> u64 {
    (u128::from(ts_us) * u128::from(SAMPLE_RATE_U64) / u128::from(MICROS_PER_SECOND)) as u64
}

/// Convert an absolute 48 kHz sample index to a Unix-microsecond timestamp.
///
/// The result is rounded up, guaranteeing `sample_index(sample_ts_us(i)) == i`.
/// For indices so large that the timestamp cannot fit in `u64`, the result
/// saturates at `u64::MAX`; Unix timestamps in practical use are far below it.
pub fn sample_ts_us(index: u64) -> u64 {
    let scaled = u128::from(index) * u128::from(MICROS_PER_SECOND);
    let rounded_up = scaled.div_ceil(u128::from(SAMPLE_RATE_U64));
    u64::try_from(rounded_up).unwrap_or(u64::MAX)
}

fn sine_sample(freq_hz: u32, amp: f32, index: u64) -> f32 {
    let cycles = (u64::from(freq_hz) * (index % SAMPLE_RATE_U64)) % SAMPLE_RATE_U64;
    let phase = TAU * cycles as f64 / SAMPLE_RATE_U64 as f64;
    (f64::from(amp) * phase.sin()) as f32
}

fn burst_samples(period_us: u64, on_us: u64) -> Option<(u64, u64)> {
    let period_times_rate = u128::from(period_us) * 48;
    if !period_times_rate.is_multiple_of(1000) {
        return None;
    }
    let period_samples = u64::try_from(period_times_rate / 1000).ok()?;
    if period_samples == 0 {
        return None;
    }

    let on_samples = u128::from(on_us) * 48 / 1000;
    let on_samples = u64::try_from(on_samples).unwrap_or(u64::MAX);
    Some((period_samples, on_samples.min(period_samples)))
}

fn noise_sample(amp: f32, seed: u64, index: u64) -> f32 {
    let random = splitmix64(seed ^ index);
    let unit = (random >> 40) as f64 / ((1_u64 << 24) - 1) as f64;
    ((unit * 2.0 - 1.0) * f64::from(amp)) as f32
}

fn splitmix64(mut value: u64) -> u64 {
    value = value.wrapping_add(0x9E37_79B9_7F4A_7C15);
    value = (value ^ (value >> 30)).wrapping_mul(0xBF58_476D_1CE4_E5B9);
    value = (value ^ (value >> 27)).wrapping_mul(0x94D0_49BB_1331_11EB);
    value ^ (value >> 31)
}
