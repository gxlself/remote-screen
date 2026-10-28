//! Viewport reporting: conversion, quantization, de-duplication, and timing.

use rsp_core::pb;

/// Minimum time between two viewport reports (at most 10 Hz).
pub const MIN_INTERVAL_US: u64 = 100_000;
/// Time without an active input before a report is marked settled.
pub const SETTLE_US: u64 = 250_000;
/// Pixel grid used for viewport and phone-view quantization.
pub const GRID_PX: u32 = 16;
/// A viewport this wide and high is treated as the full display.
pub const FULL_FRACTION: f64 = 0.98;

/// A normalized visible area supplied by a native view.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct ViewportInput {
    pub x: f32,
    pub y: f32,
    pub w: f32,
    pub h: f32,
    pub view_w_px: u32,
    pub view_h_px: u32,
    pub active: bool,
    pub crop_ok: bool,
}

impl ViewportInput {
    /// Reject non-finite or empty inputs and clip the visible area to the display.
    pub fn sanitized(self) -> Option<Self> {
        if !self.x.is_finite()
            || !self.y.is_finite()
            || !self.w.is_finite()
            || !self.h.is_finite()
            || self.w <= 0.0
            || self.h <= 0.0
        {
            return None;
        }

        let x = self.x.clamp(0.0, 1.0);
        let y = self.y.clamp(0.0, 1.0);
        let w = self.w.min(1.0 - x);
        let h = self.h.min(1.0 - y);
        if w <= 0.0 || h <= 0.0 {
            return None;
        }

        Some(Self { x, y, w, h, ..self })
    }
}

/// Convert a normalized viewport to a display-native pixel rectangle.
///
/// Coordinates are rounded outward to [`GRID_PX`] and then clipped to the
/// display. A viewport at least [`FULL_FRACTION`] wide and high represents the
/// whole display and therefore has no rectangle.
pub fn to_pixel_rect(
    input: &ViewportInput,
    display_w: u32,
    display_h: u32,
) -> Option<pb::PixelRect> {
    if display_w == 0 || display_h == 0 {
        return None;
    }

    let input = input.sanitized()?;
    if f64::from(input.w) >= FULL_FRACTION && f64::from(input.h) >= FULL_FRACTION {
        return None;
    }

    let x0 = f64::from(input.x) * f64::from(display_w) + 1e-3;
    let x1 = (f64::from(input.x) + f64::from(input.w)) * f64::from(display_w) - 1e-3;
    let y0 = f64::from(input.y) * f64::from(display_h) + 1e-3;
    let y1 = (f64::from(input.y) + f64::from(input.h)) * f64::from(display_h) - 1e-3;

    let x0 = align_down(clamp_floor(x0, display_w));
    let x1 = align_up(clamp_ceil(x1, display_w)).min(u64::from(display_w));
    let y0 = align_down(clamp_floor(y0, display_h));
    let y1 = align_up(clamp_ceil(y1, display_h)).min(u64::from(display_h));

    Some(pb::PixelRect {
        x: x0 as u32,
        y: y0 as u32,
        w: x1.saturating_sub(x0) as u32,
        h: y1.saturating_sub(y0) as u32,
    })
}

fn clamp_floor(value: f64, limit: u32) -> u64 {
    value.floor().clamp(0.0, f64::from(limit)) as u64
}

fn clamp_ceil(value: f64, limit: u32) -> u64 {
    value.ceil().clamp(0.0, f64::from(limit)) as u64
}

fn align_down(value: u64) -> u64 {
    value / u64::from(GRID_PX) * u64::from(GRID_PX)
}

fn align_up(value: u64) -> u64 {
    let grid = u64::from(GRID_PX);
    value.div_ceil(grid) * grid
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct RectKey {
    x: u32,
    y: u32,
    w: u32,
    h: u32,
}

impl RectKey {
    fn from_proto(rect: &pb::PixelRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }

    fn into_proto(self) -> pb::PixelRect {
        pb::PixelRect {
            x: self.x,
            y: self.y,
            w: self.w,
            h: self.h,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
struct ViewportKey {
    display_id: u32,
    rect: Option<RectKey>,
    view_width_px: u32,
    view_height_px: u32,
    crop_ok: bool,
}

#[derive(Debug, Clone, Copy)]
struct StreamConfigState {
    display_id: u32,
    display_w: u32,
    display_h: u32,
}

/// Pure state machine for producing throttled `pb::Viewport` reports.
#[derive(Debug)]
pub struct ViewportReporter {
    stream_config: Option<StreamConfigState>,
    last_input: Option<ViewportInput>,
    effective_rect: Option<RectKey>,
    effective_anchor: Option<ViewportInput>,
    quiet_since_us: Option<u64>,
    last_sent_key: Option<ViewportKey>,
    last_sent_settled: bool,
    last_sent_at_us: Option<u64>,
    next_seq: u32,
    highest_sent_seq: Option<u32>,
    ack_seq: Option<u32>,
    ack_mode: pb::viewport_ack::Mode,
}

impl Default for ViewportReporter {
    fn default() -> Self {
        Self::new()
    }
}

impl ViewportReporter {
    pub fn new() -> Self {
        Self {
            stream_config: None,
            last_input: None,
            effective_rect: None,
            effective_anchor: None,
            quiet_since_us: None,
            last_sent_key: None,
            last_sent_settled: false,
            last_sent_at_us: None,
            next_seq: 1,
            highest_sent_seq: None,
            ack_seq: None,
            ack_mode: pb::viewport_ack::Mode::Off,
        }
    }

    /// Record the display associated with the current stream epoch.
    pub fn on_stream_config(&mut self, display_id: u32, display_w: u32, display_h: u32) {
        self.stream_config = Some(StreamConfigState {
            display_id,
            display_w,
            display_h,
        });
        self.effective_rect = self
            .last_input
            .and_then(|input| self.rect_for_config(&input));
        self.effective_anchor = self.last_input;
    }

    /// Record a native-view update. Invalid updates are ignored completely.
    pub fn on_input(&mut self, now_us: u64, input: ViewportInput) {
        let Some(input) = input.sanitized() else {
            return;
        };

        let previous_input = self.last_input;
        let previous_key = self.current_key();
        let previous_rect = self.effective_rect;
        let previous_anchor = self.effective_anchor;
        self.last_input = Some(input);
        let current_rect = self.rect_for_config(&input);
        // Keep a quantized edge stable while native floating-point updates
        // move it by less than one grid cell. This avoids reports caused by
        // sub-grid jitter at an outward-rounded edge.
        if let (Some(previous_anchor), Some(config), Some(previous_rect), Some(current_rect)) = (
            previous_anchor,
            self.stream_config,
            previous_rect,
            current_rect,
        ) {
            if previous_rect != current_rect
                && rect_motion_below_grid(previous_anchor, input, config)
            {
                self.effective_rect = Some(previous_rect);
            } else {
                self.effective_rect = Some(current_rect);
                self.effective_anchor = Some(input);
            }
        } else {
            self.effective_rect = current_rect;
            self.effective_anchor = Some(input);
        }
        let current_key = self.current_key();

        let key_changed = match (previous_input, self.stream_config) {
            (None, _) => true,
            (Some(_previous), Some(_)) => previous_key != current_key,
            (Some(previous), None) => input_key_changed_without_display(previous, input),
        };
        let previous_active = previous_input.is_some_and(|previous| previous.active);
        if key_changed || input.active || previous_active {
            self.quiet_since_us = Some(now_us);
        }
    }

    /// Return a report if its key or settled state changed and throttling allows it.
    pub fn poll(&mut self, now_us: u64) -> Option<pb::Viewport> {
        let config = self.stream_config?;
        if config.display_w == 0 || config.display_h == 0 || self.last_input.is_none() {
            return None;
        }
        let key = self.current_key()?;
        let settled = self.is_settled(now_us);
        let key_changed = self.last_sent_key != Some(key);
        let settled_changed = settled && !self.last_sent_settled;
        if !key_changed && !settled_changed {
            return None;
        }

        if let Some(last_sent_at_us) = self.last_sent_at_us {
            if now_us < last_sent_at_us.saturating_add(MIN_INTERVAL_US) {
                return None;
            }
        }

        let seq = self.next_seq;
        self.next_seq = self.next_seq.saturating_add(1);
        self.highest_sent_seq = Some(seq);
        self.last_sent_key = Some(key);
        self.last_sent_settled = settled;
        self.last_sent_at_us = Some(now_us);

        Some(pb::Viewport {
            seq,
            display_id: config.display_id,
            rect: key.rect.map(RectKey::into_proto),
            settled,
            view_width_px: key.view_width_px,
            view_height_px: key.view_height_px,
            crop_ok: key.crop_ok,
        })
    }

    /// Return the earliest time at which a future `poll` may produce a report.
    pub fn deadline(&self) -> Option<u64> {
        let config = self.stream_config?;
        if config.display_w == 0 || config.display_h == 0 || self.last_input.is_none() {
            return None;
        }
        let key = self.current_key()?;
        let quiet_since_us = self.quiet_since_us?;

        if self.last_sent_key != Some(key) {
            return Some(match self.last_sent_at_us {
                Some(last_sent_at_us) => {
                    quiet_since_us.max(last_sent_at_us.saturating_add(MIN_INTERVAL_US))
                }
                None => quiet_since_us,
            });
        }

        let last_input = self.last_input?;
        if !last_input.active && !self.last_sent_settled {
            let settled_at = quiet_since_us.saturating_add(SETTLE_US);
            let throttle_at = self
                .last_sent_at_us
                .map(|sent_at| sent_at.saturating_add(MIN_INTERVAL_US))
                .unwrap_or(quiet_since_us);
            return Some(settled_at.max(throttle_at));
        }

        None
    }

    /// Record a host acknowledgement for a report that was actually sent.
    pub fn on_ack(&mut self, ack: &pb::ViewportAck) {
        let Some(highest_sent_seq) = self.highest_sent_seq else {
            return;
        };
        if ack.seq > highest_sent_seq {
            return;
        }
        if self.ack_seq.is_some_and(|ack_seq| ack.seq < ack_seq) {
            return;
        }

        self.ack_seq = Some(ack.seq);
        self.ack_mode =
            pb::viewport_ack::Mode::try_from(ack.mode).unwrap_or(pb::viewport_ack::Mode::Off);
    }

    /// Return the mode from the most recent valid acknowledgement.
    pub fn host_mode(&self) -> pb::viewport_ack::Mode {
        self.ack_mode
    }

    fn current_key(&self) -> Option<ViewportKey> {
        let config = self.stream_config?;
        let input = self.last_input?;
        let rect = if config.display_w == 0 || config.display_h == 0 {
            None
        } else {
            self.effective_rect
        };
        Some(ViewportKey {
            display_id: config.display_id,
            rect,
            view_width_px: quantize_view_px(input.view_w_px),
            view_height_px: quantize_view_px(input.view_h_px),
            crop_ok: input.crop_ok,
        })
    }

    fn rect_for_config(&self, input: &ViewportInput) -> Option<RectKey> {
        let config = self.stream_config?;
        if config.display_w == 0 || config.display_h == 0 {
            return None;
        }
        to_pixel_rect(input, config.display_w, config.display_h)
            .as_ref()
            .map(RectKey::from_proto)
    }

    fn is_settled(&self, now_us: u64) -> bool {
        let Some(input) = self.last_input else {
            return false;
        };
        let Some(quiet_since_us) = self.quiet_since_us else {
            return false;
        };
        !input.active && now_us >= quiet_since_us.saturating_add(SETTLE_US)
    }
}

fn quantize_view_px(value: u32) -> u32 {
    value / GRID_PX * GRID_PX
}

fn rect_motion_below_grid(
    previous: ViewportInput,
    current: ViewportInput,
    config: StreamConfigState,
) -> bool {
    let previous_right = f64::from(previous.x) + f64::from(previous.w);
    let current_right = f64::from(current.x) + f64::from(current.w);
    let previous_bottom = f64::from(previous.y) + f64::from(previous.h);
    let current_bottom = f64::from(current.y) + f64::from(current.h);
    let grid = f64::from(GRID_PX);

    (f64::from(previous.x) - f64::from(current.x)).abs() * f64::from(config.display_w) < grid
        && (previous_right - current_right).abs() * f64::from(config.display_w) < grid
        && (f64::from(previous.y) - f64::from(current.y)).abs() * f64::from(config.display_h) < grid
        && (previous_bottom - current_bottom).abs() * f64::from(config.display_h) < grid
}

fn input_key_changed_without_display(previous: ViewportInput, current: ViewportInput) -> bool {
    previous.x != current.x
        || previous.y != current.y
        || previous.w != current.w
        || previous.h != current.h
        || quantize_view_px(previous.view_w_px) != quantize_view_px(current.view_w_px)
        || quantize_view_px(previous.view_h_px) != quantize_view_px(current.view_h_px)
        || previous.crop_ok != current.crop_ok
}
