//! Capture sizing without inspecting the platform's native image.

use crate::platform::{DisplayInfo, PlatformError, Result};

/// Fit within pixel limits, preserving aspect ratio and even encoder sizes.
pub fn dimensions(display: &DisplayInfo, max_width: u32, max_height: u32) -> Result<(u32, u32)> {
    if display.width_px < 2 || display.height_px < 2 {
        return Err(PlatformError::Platform("invalid display dimensions".into()));
    }
    let scale = |native: u32, limit: u32| {
        if limit == 0 {
            1.0
        } else {
            (f64::from(limit) / f64::from(native)).min(1.0)
        }
    };
    let ratio = scale(display.width_px, max_width).min(scale(display.height_px, max_height));
    let width = ((f64::from(display.width_px) * ratio).round() as u32 & !1).max(2);
    let height = ((f64::from(display.height_px) * ratio).round() as u32 & !1).max(2);
    Ok((width, height))
}
