//! 视口裁剪尺寸、ROI 区域与坐标换算（03 §5.5）。

use crate::platform::{PixelRect, RoiRegion};

/// QP offset for the focused ROI.
pub const ROI_INSIDE_QP: i8 = -6;
/// QP offset for the rest of a small viewport.
pub const ROI_OUTSIDE_QP: i8 = 2;
/// The rest of the frame is raised in QP only for a viewport no larger than
/// half of the display.
pub const ROI_OUTSIDE_MAX_FRACTION: f64 = 0.5;
/// Encoder pixel alignment used by the viewport ROI path.
pub const STREAM_ALIGN_PX: u32 = 16;

/// Calculate an even output size for a crop while preserving its aspect ratio.
///
/// `budget` is the full-screen stream's pixel budget. A zero view component
/// disables the view constraint, and a zero limit component disables that
/// individual limit.
pub fn crop_output_size(
    crop: PixelRect,
    viewport: PixelRect,
    budget: (u32, u32),
    view: (u32, u32),
    limit: (u32, u32),
) -> (u32, u32) {
    if crop.w == 0 || crop.h == 0 {
        return (2, 2);
    }

    let crop_width = f64::from(crop.w);
    let crop_height = f64::from(crop.h);
    let mut scale = 1.0_f64;

    let budget_pixels = f64::from(budget.0) * f64::from(budget.1);
    let crop_pixels = crop_width * crop_height;
    scale = scale.min((budget_pixels / crop_pixels).sqrt());

    if view.0 > 0 && view.1 > 0 && viewport.w > 0 && viewport.h > 0 {
        scale = scale.min(
            (f64::from(view.0) / f64::from(viewport.w))
                .max(f64::from(view.1) / f64::from(viewport.h)),
        );
    }

    if limit.0 > 0 {
        scale = scale.min(f64::from(limit.0) / crop_width);
    }
    if limit.1 > 0 {
        scale = scale.min(f64::from(limit.1) / crop_height);
    }

    let width = even_floor_scaled(crop_width, scale);
    let height = even_floor_scaled(crop_height, scale);
    (width, height)
}

fn even_floor_scaled(value: f64, scale: f64) -> u32 {
    let floor = (value * scale + 1e-6).floor() as u32;
    (floor & !1).max(2)
}

/// Convert a display-space rectangle to stream pixels, clipping it to both
/// the display and the stream. Coordinates are integer-scaled and alignment is
/// applied outward to the requested stream-pixel boundary.
pub fn display_to_stream(
    rect: PixelRect,
    display: (u32, u32),
    stream: (u32, u32),
    align: u32,
) -> PixelRect {
    if display.0 == 0 || display.1 == 0 {
        return PixelRect {
            x: 0,
            y: 0,
            w: stream.0,
            h: stream.1,
        };
    }

    let x_start = rect.x.min(display.0);
    let x_end = rect.x.saturating_add(rect.w).min(display.0);
    let y_start = rect.y.min(display.1);
    let y_end = rect.y.saturating_add(rect.h).min(display.1);

    let mut x0 = scale_floor(x_start, display.0, stream.0);
    let mut x1 = scale_ceil(x_end, display.0, stream.0);
    let mut y0 = scale_floor(y_start, display.1, stream.1);
    let mut y1 = scale_ceil(y_end, display.1, stream.1);

    x0 = align_down(x0, align).min(u64::from(stream.0));
    x1 = align_up(x1, align).min(u64::from(stream.0));
    y0 = align_down(y0, align).min(u64::from(stream.1));
    y1 = align_up(y1, align).min(u64::from(stream.1));

    PixelRect {
        x: x0 as u32,
        y: y0 as u32,
        w: (x1 - x0) as u32,
        h: (y1 - y0) as u32,
    }
}

fn scale_floor(value: u32, source: u32, destination: u32) -> u64 {
    u64::from(value) * u64::from(destination) / u64::from(source)
}

fn scale_ceil(value: u32, source: u32, destination: u32) -> u64 {
    let numerator = u64::from(value) * u64::from(destination);
    let denominator = u64::from(source);
    numerator.div_ceil(denominator)
}

fn align_down(value: u64, align: u32) -> u64 {
    if align <= 1 {
        value
    } else {
        let align = u64::from(align);
        value - value % align
    }
}

fn align_up(value: u64, align: u32) -> u64 {
    if align <= 1 {
        value
    } else {
        let align = u64::from(align);
        value.div_ceil(align) * align
    }
}

/// Build the focused ROI and, for a small viewport, a lower-quality region
/// covering the rest of the stream.
pub fn focus_regions(
    focus: PixelRect,
    viewport: PixelRect,
    display: (u32, u32),
    stream: (u32, u32),
) -> Vec<RoiRegion> {
    if focus.w == 0 || focus.h == 0 {
        return Vec::new();
    }

    let focused = display_to_stream(focus, display, stream, STREAM_ALIGN_PX);
    if focused.w == 0 || focused.h == 0 {
        return Vec::new();
    }

    let mut regions = vec![RoiRegion {
        rect: focused,
        qp_delta: ROI_INSIDE_QP,
    }];

    let display_area = u64::from(display.0) * u64::from(display.1);
    let viewport_area = u64::from(viewport.w) * u64::from(viewport.h);
    if display_area > 0 && (viewport_area as f64 / display_area as f64) <= ROI_OUTSIDE_MAX_FRACTION
    {
        regions.push(RoiRegion {
            rect: PixelRect {
                x: 0,
                y: 0,
                w: stream.0,
                h: stream.1,
            },
            qp_delta: ROI_OUTSIDE_QP,
        });
    }

    regions
}

/// Check whether an output size has the crop's aspect ratio within the
/// permitted two-pixel rounding tolerance.
pub fn aspect_matches(crop: PixelRect, out: (u32, u32)) -> bool {
    let lhs = u64::from(out.0) * u64::from(crop.h);
    let rhs = u64::from(out.1) * u64::from(crop.w);
    let difference = lhs.abs_diff(rhs);
    difference <= 2 * (u64::from(crop.w) + u64::from(crop.h))
}

/// Return whether a rectangle is at least 2x2 and fully inside a display.
pub fn rect_in_display(rect: PixelRect, display: (u32, u32)) -> bool {
    rect.w >= 2
        && rect.h >= 2
        && rect.x <= display.0
        && rect.y <= display.1
        && rect.w <= display.0 - rect.x
        && rect.h <= display.1 - rect.y
}
