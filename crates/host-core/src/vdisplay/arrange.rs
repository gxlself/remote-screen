//! Mac 上新虚拟屏的位置（M3 · V1）：所有屏右侧，与主屏上沿对齐。坐标单位是点。

use crate::platform::DisplayInfo;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct DeskRect {
    pub x: i32,
    pub y: i32,
    pub w: u32,
    pub h: u32,
}

/// Convert a display's pixel dimensions to a logical point rectangle.
pub fn mac_rect(display: &DisplayInfo) -> DeskRect {
    let scale = if display.scale.is_finite() && display.scale > 0.0 {
        f64::from(display.scale)
    } else {
        1.0
    };
    let w = (f64::from(display.width_px) / scale).round() as u32;
    let h = (f64::from(display.height_px) / scale).round() as u32;
    DeskRect {
        x: display.x,
        y: display.y,
        w,
        h,
    }
}

/// Return the new display's top-left point beside all existing displays.
pub fn place_beside(existing: &[(DeskRect, bool)]) -> (i32, i32) {
    let rightmost = existing
        .iter()
        .map(|(rect, _)| i64::from(rect.x) + i64::from(rect.w))
        .max()
        .unwrap_or(0);
    let x = rightmost.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32;
    let y = existing
        .iter()
        .find_map(|(rect, primary)| primary.then_some(rect.y))
        .unwrap_or(0);
    (x, y)
}
