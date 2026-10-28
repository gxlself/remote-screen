//! Scroll parameters for CGEventCreateScrollWheelEvent2, without platform APIs.
//!
//! The App already applies natural scrolling. Both axes retain their incoming
//! sign here: the Host must not invert them again. Line deltas are converted to
//! pixels, so all returned events use pixel units.

use rsp_core::hid::{InputMsg, ScrollPhase, ScrollUnit};

/// Default line-to-pixel scale; callers can configure [`ScrollAccumulator`].
pub const DEFAULT_PIXELS_PER_LINE: f64 = 10.0;

/// Values for CGEventCreateScrollWheelEvent2 and its two phase fields.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MacScroll {
    pub units_pixel: bool,
    pub dy: i32,
    pub dx: i32,
    pub scroll_phase: u32,
    pub momentum_phase: u32,
}

/// Convert one scroll event at the default scale, rounding toward zero.
///
/// Non-scroll messages and non-finite deltas return `None`. For a stream of
/// events, use [`ScrollAccumulator::convert`] to retain fractional pixels.
pub fn to_mac_scroll(msg: &InputMsg) -> Option<MacScroll> {
    ScrollAccumulator::new().convert(msg)
}

/// Retain fractional pixels independently on both axes across scroll events.
///
/// A new gesture (`phase = Began`) clears old fractions. Ended and momentum
/// Began retain them so a gesture can hand off smoothly to inertia. Phaseless
/// wheel events retain them too. Call [`Self::reset`] on session teardown or
/// ReleaseAll to avoid carrying fractions between controllers. Rejected
/// messages leave state untouched. Values outside i32 saturate and discard
/// overflow instead of accumulating an unbounded backlog.
#[derive(Debug, Clone)]
pub struct ScrollAccumulator {
    pixels_per_line: f64,
    remainder_x: f64,
    remainder_y: f64,
}

impl Default for ScrollAccumulator {
    fn default() -> Self {
        Self::new()
    }
}

impl ScrollAccumulator {
    /// Create an accumulator with the default ten pixels per line.
    pub fn new() -> Self {
        Self::with_pixels_per_line(DEFAULT_PIXELS_PER_LINE)
    }

    /// Set the line-to-pixel scale. Non-positive or non-finite scales fall back
    /// to [`DEFAULT_PIXELS_PER_LINE`], preserving the incoming axis directions.
    pub fn with_pixels_per_line(pixels_per_line: f64) -> Self {
        Self {
            pixels_per_line: if pixels_per_line.is_finite() && pixels_per_line > 0.0 {
                pixels_per_line
            } else {
                DEFAULT_PIXELS_PER_LINE
            },
            remainder_x: 0.0,
            remainder_y: 0.0,
        }
    }

    /// Discard both fractional remainders without changing the scale.
    pub fn reset(&mut self) {
        self.remainder_x = 0.0;
        self.remainder_y = 0.0;
    }

    /// Convert a scroll event, retaining fractional pixels for the next call.
    ///
    /// CoreGraphics phase values differ from the protocol's discriminants:
    ///
    /// | Protocol | ScrollPhase | MomentumPhase |
    /// | --- | --- | --- |
    /// | None | 0 | 0 (None) |
    /// | Began | 1 (Began) | 1 (Begin) |
    /// | Changed | 2 (Changed) | 2 (Continue) |
    /// | Ended | 4 (Ended) | 3 (End) |
    ///
    /// CG ScrollPhase Cancelled=8 and MayBegin=128 have no equivalents in the
    /// current wire contract. This helper never invents those phases.
    pub fn convert(&mut self, msg: &InputMsg) -> Option<MacScroll> {
        let InputMsg::Scroll {
            dx,
            dy,
            phase,
            momentum,
            unit,
        } = *msg
        else {
            return None;
        };
        if !dx.is_finite() || !dy.is_finite() {
            return None;
        }
        if phase == ScrollPhase::Began {
            self.reset();
        }
        let scale = match unit {
            ScrollUnit::Pixel => 1.0,
            ScrollUnit::Line => self.pixels_per_line,
        };
        Some(MacScroll {
            units_pixel: true,
            dy: accumulate(f64::from(dy) * scale, &mut self.remainder_y),
            dx: accumulate(f64::from(dx) * scale, &mut self.remainder_x),
            scroll_phase: match phase {
                ScrollPhase::None => 0,
                ScrollPhase::Began => 1,   // kCGScrollPhaseBegan
                ScrollPhase::Changed => 2, // kCGScrollPhaseChanged
                ScrollPhase::Ended => 4,   // kCGScrollPhaseEnded
            },
            momentum_phase: match momentum {
                ScrollPhase::None => 0,    // kCGMomentumScrollPhaseNone
                ScrollPhase::Began => 1,   // kCGMomentumScrollPhaseBegin
                ScrollPhase::Changed => 2, // kCGMomentumScrollPhaseContinue
                ScrollPhase::Ended => 3,   // kCGMomentumScrollPhaseEnd
            },
        })
    }
}

fn accumulate(delta: f64, remainder: &mut f64) -> i32 {
    let sum = delta + *remainder;
    let whole = sum.clamp(f64::from(i32::MIN), f64::from(i32::MAX)).trunc() as i32;
    *remainder = if sum >= f64::from(i32::MAX) || sum <= f64::from(i32::MIN) {
        0.0
    } else {
        sum - f64::from(whole)
    };
    whole
}
