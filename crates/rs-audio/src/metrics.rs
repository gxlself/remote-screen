//! Objective mono metrics for generated or captured audio.

use crate::SAMPLE_RATE;

const WINDOW_SAMPLES: usize = SAMPLE_RATE as usize / 200;
const CLICK_REFRACTORY_SAMPLES: u64 = 48;
const ONSET_QUIET_SAMPLES: u64 = SAMPLE_RATE as u64 * 20 / 1000;

/// Root-mean-square level in dBFS. An empty or all-zero slice is silent.
pub fn rms_dbfs(x: &[f32]) -> f32 {
    if x.is_empty() {
        return f32::NEG_INFINITY;
    }
    let sum_squares: f64 = x
        .iter()
        .map(|&sample| {
            let sample = f64::from(sample);
            sample * sample
        })
        .sum();
    let mean_square = sum_squares / x.len() as f64;
    if mean_square <= 0.0 {
        f32::NEG_INFINITY
    } else {
        (10.0 * mean_square.log10()) as f32
    }
}

/// Largest absolute sample value. An empty slice has peak zero.
pub fn peak(x: &[f32]) -> f32 {
    x.iter()
        .fold(0.0_f32, |maximum, &sample| maximum.max(sample.abs()))
}

/// Counts sample-to-sample jumps that are likely to be audible clicks.
#[derive(Debug, Clone)]
pub struct ClickMeter {
    threshold: f32,
    next_position: u64,
    last_sample: Option<f32>,
    skip_remaining: u64,
    last_click: Option<u64>,
    clicks: u64,
    positions: Vec<u64>,
}

impl ClickMeter {
    pub fn new(threshold: f32) -> Self {
        Self {
            threshold,
            next_position: 0,
            last_sample: None,
            skip_remaining: 0,
            last_click: None,
            clicks: 0,
            positions: Vec::new(),
        }
    }

    /// Build a meter whose threshold is three times a sine wave's maximum slope.
    pub fn for_sine(freq_hz: u32, amp: f32) -> Self {
        let max_slope = std::f32::consts::TAU * freq_hz as f32 * amp.abs() / SAMPLE_RATE as f32;
        Self::new(3.0 * max_slope)
    }

    /// Append samples. State, including the previous sample and positions, is
    /// preserved across calls.
    pub fn push(&mut self, x: &[f32]) {
        for &sample in x {
            let position = self.next_position;
            self.next_position = self.next_position.saturating_add(1);

            if self.skip_remaining > 0 {
                self.skip_remaining -= 1;
                self.last_sample = Some(sample);
                continue;
            }

            if let Some(previous) = self.last_sample {
                if (sample - previous).abs() > self.threshold {
                    let allowed = match self.last_click {
                        Some(last) => position.saturating_sub(last) >= CLICK_REFRACTORY_SAMPLES,
                        None => true,
                    };
                    if allowed {
                        self.clicks += 1;
                        self.positions.push(position);
                        self.last_click = Some(position);
                    }
                }
            }
            self.last_sample = Some(sample);
        }
    }

    /// Ignore detection for the next `n` samples while keeping stream state.
    pub fn skip(&mut self, n: usize) {
        self.skip_remaining = self.skip_remaining.saturating_add(n as u64);
    }

    pub fn clicks(&self) -> u64 {
        self.clicks
    }

    /// Positions use the first sample passed to the meter as position zero.
    pub fn positions(&self) -> &[u64] {
        &self.positions
    }
}

/// Collects 5 ms windows and reports RMS-level discontinuities and gaps.
#[derive(Debug, Clone)]
pub struct WindowMeter {
    sum_squares: f64,
    filled: usize,
    windows: Vec<f32>,
}

impl WindowMeter {
    pub fn new() -> Self {
        Self {
            sum_squares: 0.0,
            filled: 0,
            windows: Vec::new(),
        }
    }

    /// Append samples. Only complete 240-sample windows are recorded; an
    /// incomplete tail is retained for the next call.
    pub fn push(&mut self, x: &[f32]) {
        for &sample in x {
            let sample = f64::from(sample);
            self.sum_squares += sample * sample;
            self.filled += 1;
            if self.filled == WINDOW_SAMPLES {
                let mean_square = self.sum_squares / WINDOW_SAMPLES as f64;
                let db = if mean_square <= 0.0 {
                    f32::NEG_INFINITY
                } else {
                    (10.0 * mean_square.log10()) as f32
                };
                self.windows.push(db);
                self.sum_squares = 0.0;
                self.filled = 0;
            }
        }
    }

    /// Count adjacent windows whose RMS levels differ by more than `db`.
    ///
    /// Pairs where both windows are below -60 dBFS are ignored.
    pub fn energy_jumps(&self, db: f32) -> u64 {
        self.windows
            .windows(2)
            .filter(|pair| !(pair[0] < -60.0 && pair[1] < -60.0) && (pair[0] - pair[1]).abs() > db)
            .count() as u64
    }

    /// Count runs below `floor_db` lasting at least `min_ms`.
    ///
    /// Duration is measured in complete 5 ms windows, so the minimum is
    /// rounded up to whole windows. A run still open at the end of the input
    /// counts once it has reached the minimum length.
    pub fn gaps(&self, floor_db: f32, min_ms: u32) -> u64 {
        let min_windows = (min_ms as usize).div_ceil(5).max(1);
        let mut gaps = 0_u64;
        let mut run_windows = 0_usize;

        for &db in &self.windows {
            if db < floor_db {
                run_windows += 1;
            } else {
                if run_windows >= min_windows {
                    gaps += 1;
                }
                run_windows = 0;
            }
        }
        if run_windows >= min_windows {
            gaps += 1;
        }
        gaps
    }

    /// RMS level of each complete window, in dBFS.
    pub fn windows(&self) -> &[f32] {
        &self.windows
    }
}

impl Default for WindowMeter {
    fn default() -> Self {
        Self::new()
    }
}

/// Detects onsets after at least 20 ms of quiet audio.
#[derive(Debug, Clone)]
pub struct OnsetDetector {
    off_amplitude: f32,
    on_amplitude: f32,
    quiet_samples: u64,
    armed: bool,
    onsets: Vec<u64>,
}

impl OnsetDetector {
    /// `off_db` arms the detector; `on_db` is the level that triggers it.
    pub fn new(off_db: f32, on_db: f32) -> Self {
        Self {
            off_amplitude: db_to_amplitude(off_db),
            on_amplitude: db_to_amplitude(on_db),
            quiet_samples: 0,
            armed: false,
            onsets: Vec::new(),
        }
    }

    /// Append samples whose first output time is `first_sample_time_us`.
    pub fn push(&mut self, x: &[f32], first_sample_time_us: u64) {
        for (index, &sample) in x.iter().enumerate() {
            let magnitude = sample.abs();
            if magnitude < self.off_amplitude {
                self.quiet_samples = self.quiet_samples.saturating_add(1);
                if self.quiet_samples >= ONSET_QUIET_SAMPLES {
                    self.armed = true;
                }
            } else {
                self.quiet_samples = 0;
            }

            if self.armed && magnitude > self.on_amplitude {
                let offset_us = (index as u128) * 1_000_000 / u128::from(SAMPLE_RATE as u64);
                let timestamp = u128::from(first_sample_time_us) + offset_us;
                self.onsets
                    .push(u64::try_from(timestamp).unwrap_or(u64::MAX));
                self.armed = false;
                self.quiet_samples = 0;
            }
        }
    }

    /// Output timestamps of detected onsets, in push order.
    pub fn onsets(&self) -> &[u64] {
        &self.onsets
    }
}

/// Convert onset timestamps to latency within each burst period.
///
/// This assumes latency is below one period and that capture and output clocks
/// share a time base. A zero period yields an empty result.
pub fn burst_latencies_us(onsets: &[u64], period_us: u64) -> Vec<u64> {
    if period_us == 0 {
        return Vec::new();
    }
    onsets.iter().map(|&onset| onset % period_us).collect()
}

/// Nearest-rank percentile. The input need not be sorted.
///
/// `p` is clamped to `[0, 1]`; a NaN percentile returns `None`.
pub fn percentile(sorted_or_not: &[u64], p: f64) -> Option<u64> {
    if sorted_or_not.is_empty() || p.is_nan() {
        return None;
    }
    let mut values = sorted_or_not.to_vec();
    values.sort_unstable();
    let p = p.clamp(0.0, 1.0);
    let rank = ((p * values.len() as f64).ceil() as usize)
        .saturating_sub(1)
        .min(values.len() - 1);
    values.get(rank).copied()
}

fn db_to_amplitude(db: f32) -> f32 {
    10.0_f32.powf(db / 20.0)
}
