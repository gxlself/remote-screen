//! Native viewport reports and stream crop coordinates (03 §5.5).

use rsp_client::viewport::ViewportInput;
use rsp_core::pb;

use super::ffi_session::{guard, with_handle};

pub const RS_VIEWPORT_ACTIVE: u32 = 1;
pub const RS_VIEWPORT_CROP_OK: u32 = 2;

/// Normalize a stream crop to the display, clipping it to the display bounds.
/// Missing or empty crops and unknown display dimensions mean the full display.
pub fn crop_src(c: &pb::StreamConfig) -> [f32; 4] {
    let Some(crop) = c.crop.as_ref() else {
        return [0.0, 0.0, 1.0, 1.0];
    };
    if c.display_width_px == 0 || c.display_height_px == 0 || crop.w == 0 || crop.h == 0 {
        return [0.0, 0.0, 1.0, 1.0];
    }

    let display_w = f64::from(c.display_width_px);
    let display_h = f64::from(c.display_height_px);
    let x = (f64::from(crop.x) / display_w).clamp(0.0, 1.0);
    let y = (f64::from(crop.y) / display_h).clamp(0.0, 1.0);
    let w = (f64::from(crop.w) / display_w).clamp(0.0, 1.0 - x);
    let h = (f64::from(crop.h) / display_h).clamp(0.0, 1.0 - y);
    [x as f32, y as f32, w as f32, h as f32]
}

/// See `rs_mobile.h`.
/// # Safety
/// No pointer arguments or additional caller requirements.
#[no_mangle]
pub extern "C" fn rs_session_set_viewport(
    id: u64,
    x: f32,
    y: f32,
    w: f32,
    h: f32,
    view_w_px: u32,
    view_h_px: u32,
    flags: u32,
) {
    guard(|| {
        let Some(input) = (ViewportInput {
            x,
            y,
            w,
            h,
            view_w_px,
            view_h_px,
            active: flags & RS_VIEWPORT_ACTIVE != 0,
            crop_ok: flags & RS_VIEWPORT_CROP_OK != 0,
        })
        .sanitized() else {
            return;
        };
        with_handle(id, |h| h.set_viewport(input));
    });
}
