//! Peak-based silence suppression for 10 ms audio frames.

pub const SILENCE_DBFS: f32 = -70.0;
/// Quiet frames still sent after silence begins (200 ms at a 10 ms cadence).
pub const SILENCE_HANG_FRAMES: u32 = 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Gate {
    Send,
    Resume,
    Skip,
}

pub struct SilenceGate {
    threshold: f32,
    hang_frames: u32,
    quiet_run: u32,
    suppressed: bool,
}

impl SilenceGate {
    /// Create a gate with the protocol defaults: -70 dBFS and 20 frames.
    pub fn new() -> Self {
        Self::with(SILENCE_DBFS, SILENCE_HANG_FRAMES)
    }

    pub fn with(threshold_dbfs: f32, hang_frames: u32) -> Self {
        Self {
            threshold: 10f32.powf(threshold_dbfs / 20.0),
            hang_frames,
            quiet_run: 0,
            suppressed: false,
        }
    }

    /// Classify one frame by its peak absolute sample value.
    pub fn push(&mut self, frame: &[f32]) -> Gate {
        let quiet = peak(frame) < self.threshold;
        if quiet {
            if self.suppressed {
                return Gate::Skip;
            }
            self.quiet_run = self.quiet_run.saturating_add(1);
            if self.quiet_run > self.hang_frames {
                self.suppressed = true;
                Gate::Skip
            } else {
                Gate::Send
            }
        } else {
            self.quiet_run = 0;
            if self.suppressed {
                self.suppressed = false;
                Gate::Resume
            } else {
                Gate::Send
            }
        }
    }

    pub fn suppressed(&self) -> bool {
        self.suppressed
    }

    pub fn reset(&mut self) {
        self.quiet_run = 0;
        self.suppressed = false;
    }
}

impl Default for SilenceGate {
    fn default() -> Self {
        Self::new()
    }
}

fn peak(frame: &[f32]) -> f32 {
    frame.iter().fold(0.0, |peak, sample| {
        let magnitude = if sample.is_finite() {
            sample.abs()
        } else {
            f32::INFINITY
        };
        if magnitude > peak {
            magnitude
        } else {
            peak
        }
    })
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn default_constants_match_contract() {
        assert_eq!(SILENCE_DBFS, -70.0);
        assert_eq!(SILENCE_HANG_FRAMES, 20);
        assert!(!SilenceGate::new().suppressed());
    }
}
