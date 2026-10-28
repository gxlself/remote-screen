//! Desktop size negotiation.

/// Maximum RDP desktop long side before the size is reduced.
pub const MAX_LONG_SIDE: u32 = 1920;
/// Minimum RDP desktop short side before the size is enlarged.
pub const MIN_SHORT_SIDE: u32 = 600;

const FALLBACK_SIZE: (u16, u16) = (1280, 720);
const MIN_SIZE: u64 = 200;
const MAX_SIZE: u64 = 8192;

/// Calculate an RDP desktop size from a view size and display scale.
pub fn desktop_size(view_w: f32, view_h: f32, scale: f32) -> (u16, u16) {
    if !view_w.is_finite()
        || !view_h.is_finite()
        || !scale.is_finite()
        || view_w <= 0.0
        || view_h <= 0.0
        || scale <= 0.0
    {
        return FALLBACK_SIZE;
    }
    let Some(mut width) = rounded_dimension(view_w, scale) else {
        return FALLBACK_SIZE;
    };
    let Some(mut height) = rounded_dimension(view_h, scale) else {
        return FALLBACK_SIZE;
    };

    if width >= height {
        if width > u64::from(MAX_LONG_SIDE) {
            height = mul_div(height, u64::from(MAX_LONG_SIDE), width);
            width = u64::from(MAX_LONG_SIDE);
        }
    } else if height > u64::from(MAX_LONG_SIDE) {
        width = mul_div(width, u64::from(MAX_LONG_SIDE), height);
        height = u64::from(MAX_LONG_SIDE);
    }

    if width <= height {
        if width < u64::from(MIN_SHORT_SIDE) {
            let old_width = width.max(1);
            width = u64::from(MIN_SHORT_SIDE);
            height = mul_div(height, u64::from(MIN_SHORT_SIDE), old_width);
            if height > u64::from(MAX_LONG_SIDE) {
                height = u64::from(MAX_LONG_SIDE);
            }
        }
    } else if height < u64::from(MIN_SHORT_SIDE) {
        let old_height = height.max(1);
        height = u64::from(MIN_SHORT_SIDE);
        width = mul_div(width, u64::from(MIN_SHORT_SIDE), old_height);
        if width > u64::from(MAX_LONG_SIDE) {
            width = u64::from(MAX_LONG_SIDE);
        }
    }

    width -= width % 8;
    height -= height % 2;
    width = width.clamp(MIN_SIZE, MAX_SIZE);
    height = height.clamp(MIN_SIZE, MAX_SIZE);
    (width as u16, height as u16)
}

/// Like [`desktop_size`], but the long side is always the width: a phone
/// held upright still gets a landscape desktop, shown scaled to fit, and
/// rotating the phone keeps the same desktop size.
pub fn landscape_desktop_size(view_w: f32, view_h: f32, scale: f32) -> (u16, u16) {
    if view_h > view_w {
        desktop_size(view_h, view_w, scale)
    } else {
        desktop_size(view_w, view_h, scale)
    }
}

fn rounded_dimension(view: f32, scale: f32) -> Option<u64> {
    let scaled = (view * scale).round();
    if !scaled.is_finite() || scaled <= 0.0 {
        return None;
    }
    Some(scaled as u64)
}

fn mul_div(value: u64, numerator: u64, denominator: u64) -> u64 {
    if denominator == 0 {
        return u64::MAX;
    }
    let result = (u128::from(value) * u128::from(numerator)) / u128::from(denominator);
    result.min(u128::from(u64::MAX)) as u64
}
