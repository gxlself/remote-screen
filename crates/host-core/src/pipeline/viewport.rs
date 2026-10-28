//! Host-side viewport mode policy (03 §5.5).

pub use crate::platform::PixelRect;

/// Zoom at which a viewport starts using an ROI focus region.
pub const ROI_ENTER_ZOOM: f64 = 1.25;
/// Zoom below which an existing ROI focus region is removed.
pub const ROI_EXIT_ZOOM: f64 = 1.15;
/// Zoom at which a settled viewport starts a cropped stream.
pub const CROP_ENTER_ZOOM: f64 = 2.0;
/// Zoom below which an existing cropped stream is removed.
pub const CROP_EXIT_ZOOM: f64 = 1.6;
/// Extra crop margin divisor, applied independently on each side.
pub const CROP_MARGIN_DIV: u32 = 8;
/// Crop alignment in display pixels.
pub const CROP_ALIGN_PX: u32 = 16;
/// Minimum crop side in display pixels.
pub const CROP_MIN_SIDE_PX: u32 = 128;
/// Minimum time between crop changes.
pub const CROP_MIN_INTERVAL_US: u64 = 1_000_000;
/// Minimum time a crop is retained before a hold-based exit.
pub const CROP_HOLD_US: u64 = 300_000;
/// A settled crop this much larger than the new crop is tightened.
pub const CROP_TIGHTEN_RATIO: f64 = 2.25;
/// Extra ROI margin divisor, applied independently on each side.
pub const ROI_MARGIN_DIV: u32 = 10;
/// ROI alignment in display pixels.
pub const ROI_ALIGN_PX: u32 = 64;
/// Minimum ROI side in display pixels.
pub const ROI_MIN_SIDE_PX: u32 = 64;
/// Minimum time between ROI updates.
pub const ROI_MIN_INTERVAL_US: u64 = 200_000;

/// The currently effective viewport mode.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub enum ViewMode {
    #[default]
    Off,
    Roi,
    Crop,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewportCaps {
    pub crop: bool,
    pub roi: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ViewReport {
    pub seq: u32,
    pub rect: Option<PixelRect>,
    pub settled: bool,
    pub view_w: u32,
    pub view_h: u32,
    pub crop_ok: bool,
}

/// Output of one policy decision. `None` means that the corresponding mode
/// should be left unchanged.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct PolicyOutput {
    /// `Some(Some(r))` enters or changes to crop `r`; `Some(None)` exits crop.
    pub crop: Option<Option<PixelRect>>,
    /// `Some(Some(r))` sets an ROI focus region; `Some(None)` clears it.
    pub roi: Option<Option<PixelRect>>,
}

/// Return the zoom represented by a display-space viewport.
pub fn zoom_of(rect: Option<PixelRect>, display_w: u32, display_h: u32) -> f64 {
    let Some(rect) = rect else {
        return 1.0;
    };
    if rect.w == 0 || rect.h == 0 {
        return 1.0;
    }

    let display_area = u64::from(display_w).saturating_mul(u64::from(display_h));
    let view_area = u64::from(rect.w).saturating_mul(u64::from(rect.h));
    if view_area == 0 {
        return 1.0;
    }
    (display_area as f64 / view_area as f64).sqrt()
}

fn expand_axis(
    start: u32,
    length: u32,
    margin_div: u32,
    align: u32,
    min_side: u32,
    display: u32,
) -> (u32, u32) {
    if display == 0 {
        return (0, 0);
    }

    let margin = length.checked_div(margin_div).unwrap_or(0);
    let original_end = start.saturating_add(length);
    let mut low = start.saturating_sub(margin).min(display);
    let mut high = original_end.saturating_add(margin).min(display);

    if let Some(low_quotient) = low.checked_div(align) {
        low = low_quotient.saturating_mul(align);
        let quotient = high.checked_div(align).unwrap_or(0);
        let rounded_quotient = if high.is_multiple_of(align) {
            quotient
        } else {
            quotient.saturating_add(1)
        };
        high = rounded_quotient.saturating_mul(align).min(display);
    }
    if high < low {
        high = low;
    }

    let mut width = high.saturating_sub(low);
    if width < min_side {
        let max_even = display & !1;
        width = min_side.min(max_even);

        // Keep the original viewport center. Using u64 avoids overflow when
        // a malformed report places a rectangle near u32::MAX.
        let center = (u64::from(start).saturating_add(u64::from(original_end))) / 2;
        let center = center.min(u64::from(display));
        let half = u64::from(width / 2);
        let centered = center
            .saturating_sub(half)
            .min(u64::from(display.saturating_sub(width)));
        low = (centered as u32) & !1;
    }

    if width & 1 != 0 {
        width = width.saturating_sub(1);
    }
    let low = low.min(display);
    (low, width.min(display.saturating_sub(low)))
}

/// Expand, align, clamp, and enforce a minimum even side for a display-space
/// rectangle.
pub fn expand_align(
    rect: PixelRect,
    margin_div: u32,
    align: u32,
    min_side: u32,
    display_w: u32,
    display_h: u32,
) -> PixelRect {
    let (x, w) = expand_axis(rect.x, rect.w, margin_div, align, min_side, display_w);
    let (y, h) = expand_axis(rect.y, rect.h, margin_div, align, min_side, display_h);
    PixelRect { x, y, w, h }
}

/// Whether `inner` is fully contained by `outer`.
pub fn contains(outer: PixelRect, inner: PixelRect) -> bool {
    inner.x >= outer.x
        && inner.y >= outer.y
        && inner.x.saturating_add(inner.w) <= outer.x.saturating_add(outer.w)
        && inner.y.saturating_add(inner.h) <= outer.y.saturating_add(outer.h)
}

/// Expand a viewport into a crop rectangle.
pub fn expand_crop(rect: PixelRect, display_w: u32, display_h: u32) -> PixelRect {
    expand_align(
        rect,
        CROP_MARGIN_DIV,
        CROP_ALIGN_PX,
        CROP_MIN_SIDE_PX,
        display_w,
        display_h,
    )
}

/// Expand a viewport into an ROI focus rectangle.
pub fn expand_roi(rect: PixelRect, display_w: u32, display_h: u32) -> PixelRect {
    expand_align(
        rect,
        ROI_MARGIN_DIV,
        ROI_ALIGN_PX,
        ROI_MIN_SIDE_PX,
        display_w,
        display_h,
    )
}

#[derive(Debug)]
pub struct ViewportPolicy {
    display_w: u32,
    display_h: u32,
    caps: ViewportCaps,
    last_report: Option<ViewReport>,
    crop: Option<PixelRect>,
    crop_seq: u32,
    crop_changed_at: Option<u64>,
    crop_deadline: Option<u64>,
    roi: Option<PixelRect>,
    roi_set_at: Option<u64>,
    roi_deadline: Option<u64>,
}

impl ViewportPolicy {
    pub fn new(display_w: u32, display_h: u32, caps: ViewportCaps) -> Self {
        Self {
            display_w,
            display_h,
            caps,
            last_report: None,
            crop: None,
            crop_seq: 0,
            crop_changed_at: None,
            crop_deadline: None,
            roi: None,
            roi_set_at: None,
            roi_deadline: None,
        }
    }

    pub fn caps(&self) -> ViewportCaps {
        self.caps
    }

    pub fn set_caps(&mut self, now_us: u64, caps: ViewportCaps) -> PolicyOutput {
        self.caps = caps;
        self.step(now_us)
    }

    pub fn on_report(&mut self, now_us: u64, report: ViewReport) -> PolicyOutput {
        self.last_report = Some(report);
        self.step(now_us)
    }

    pub fn poll(&mut self, now_us: u64) -> PolicyOutput {
        self.step(now_us)
    }

    pub fn deadline(&self) -> Option<u64> {
        match (self.crop_deadline, self.roi_deadline) {
            (Some(crop), Some(roi)) => Some(crop.min(roi)),
            (Some(crop), None) => Some(crop),
            (None, Some(roi)) => Some(roi),
            (None, None) => None,
        }
    }

    pub fn mode(&self) -> ViewMode {
        if self.crop.is_some() {
            ViewMode::Crop
        } else if self.roi.is_some() {
            ViewMode::Roi
        } else {
            ViewMode::Off
        }
    }

    pub fn crop(&self) -> Option<PixelRect> {
        self.crop
    }

    pub fn crop_seq(&self) -> u32 {
        self.crop_seq
    }

    pub fn roi(&self) -> Option<PixelRect> {
        self.roi
    }

    pub fn last_report(&self) -> Option<ViewReport> {
        self.last_report
    }

    /// The pipeline could not rebuild the current crop. Treat that as a crop
    /// change and disable future crop attempts, without emitting an output.
    pub fn crop_failed(&mut self, now_us: u64) {
        self.crop = None;
        self.crop_seq = 0;
        self.crop_changed_at = Some(now_us);
        self.crop_deadline = None;
        self.caps.crop = false;
    }

    fn step(&mut self, now_us: u64) -> PolicyOutput {
        let Some(report) = self.last_report else {
            self.crop_deadline = None;
            self.roi_deadline = None;
            return PolicyOutput::default();
        };

        let mut output = PolicyOutput::default();
        let mut just_exited_crop = false;
        let zoom = zoom_of(report.rect, self.display_w, self.display_h);
        let can_crop = self.caps.crop && report.crop_ok;

        if let Some(current_crop) = self.crop {
            let mut exit = false;
            let mut replacement = None;
            let mut immediate_exit = false;

            if !can_crop {
                exit = true;
                immediate_exit = true;
            } else if report.rect.is_none() || zoom < CROP_EXIT_ZOOM {
                exit = true;
            } else if let Some(viewport) = report.rect {
                if !contains(current_crop, viewport) {
                    if report.settled {
                        replacement = Some(expand_crop(viewport, self.display_w, self.display_h));
                    } else {
                        exit = true;
                    }
                } else if report.settled {
                    let tightened = expand_crop(viewport, self.display_w, self.display_h);
                    if area(current_crop) as f64 > CROP_TIGHTEN_RATIO * area(tightened) as f64 {
                        replacement = Some(tightened);
                    }
                }
            }

            if let Some(next_crop) = replacement {
                self.crop_deadline = None;
                if next_crop != current_crop {
                    let due = self
                        .crop_changed_at
                        .map(|at| at.saturating_add(CROP_MIN_INTERVAL_US));
                    if due.is_none_or(|at| now_us >= at) {
                        self.apply_crop(now_us, next_crop, report.seq, &mut output);
                    } else {
                        self.crop_deadline = due;
                    }
                }
            } else if exit {
                self.crop_deadline = None;
                let due = if immediate_exit {
                    Some(now_us)
                } else {
                    self.crop_changed_at
                        .map(|at| at.saturating_add(CROP_HOLD_US))
                };
                if due.is_none_or(|at| now_us >= at) {
                    self.apply_crop_exit(now_us, &mut output);
                    just_exited_crop = true;
                } else {
                    self.crop_deadline = due;
                }
            } else {
                self.crop_deadline = None;
            }
        } else if can_crop && report.settled && zoom >= CROP_ENTER_ZOOM {
            let next_crop = report
                .rect
                .map(|viewport| expand_crop(viewport, self.display_w, self.display_h));
            if let Some(next_crop) = next_crop {
                let due = self
                    .crop_changed_at
                    .map(|at| at.saturating_add(CROP_MIN_INTERVAL_US));
                if due.is_none_or(|at| now_us >= at) {
                    self.crop_deadline = None;
                    self.apply_crop(now_us, next_crop, report.seq, &mut output);
                } else {
                    self.crop_deadline = due;
                }
            } else {
                self.crop_deadline = None;
            }
        } else {
            self.crop_deadline = None;
        }

        // Crop takes precedence over ROI. A crop transition to a new region
        // has already cleared ROI in apply_crop.
        if self.crop.is_some() {
            self.roi_deadline = None;
            return output;
        }

        let roi_target = if !self.caps.roi {
            None
        } else if let Some(viewport) = report.rect {
            let keep = if self.roi.is_some() {
                zoom >= ROI_EXIT_ZOOM
            } else {
                zoom >= ROI_ENTER_ZOOM
            };
            keep.then(|| expand_roi(viewport, self.display_w, self.display_h))
        } else {
            None
        };

        if roi_target == self.roi {
            self.roi_deadline = None;
        } else if roi_target.is_none() {
            self.roi_deadline = None;
            if self.roi.is_some() {
                self.roi = None;
                output.roi = Some(None);
            }
        } else if just_exited_crop || self.roi_set_at.is_none() {
            self.roi_deadline = None;
            self.roi = roi_target;
            self.roi_set_at = Some(now_us);
            output.roi = Some(roi_target);
        } else {
            let due = self
                .roi_set_at
                .map(|at| at.saturating_add(ROI_MIN_INTERVAL_US));
            if due.is_none_or(|at| now_us >= at) {
                self.roi_deadline = None;
                self.roi = roi_target;
                self.roi_set_at = Some(now_us);
                output.roi = Some(roi_target);
            } else {
                self.roi_deadline = due;
            }
        }

        output
    }

    fn apply_crop(&mut self, now_us: u64, crop: PixelRect, seq: u32, output: &mut PolicyOutput) {
        self.crop = Some(crop);
        self.crop_seq = seq;
        self.crop_changed_at = Some(now_us);
        self.crop_deadline = None;
        output.crop = Some(Some(crop));
        if self.roi.is_some() {
            self.roi = None;
            self.roi_deadline = None;
            output.roi = Some(None);
        }
    }

    fn apply_crop_exit(&mut self, now_us: u64, output: &mut PolicyOutput) {
        self.crop = None;
        self.crop_seq = 0;
        self.crop_changed_at = Some(now_us);
        self.crop_deadline = None;
        output.crop = Some(None);
    }
}

fn area(rect: PixelRect) -> u64 {
    u64::from(rect.w).saturating_mul(u64::from(rect.h))
}
