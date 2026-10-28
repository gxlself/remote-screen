//! 视口裁剪的跨进程帧包装与源矩形换算。

use std::any::Any;
use std::sync::Arc;

use host_core::platform::{CapturedFrame, PixelRect};

use crate::vpmf_model::{rotated_size, vp_rotation, RectU, VpRotation};

/// 本组第一版是否对 host-core 声明支持区域采集。 在实机验证
/// VideoProcessor 裁剪后改成 true。
pub const CROP_SUPPORTED: bool = false;

/// 会话进程编码前给帧套的包装：inner 是原来的 CapturedFrame.native。
pub struct RegionFrame {
    pub inner: Arc<dyn Any + Send + Sync>,
    pub rect: PixelRect,
    pub display_w: u32,
    pub display_h: u32,
}

/// region 为 None 时保留原 native；否则把原 native 包在 RegionFrame 中。
pub fn wrap_frame(frame: CapturedFrame, region: Option<(PixelRect, u32, u32)>) -> CapturedFrame {
    let CapturedFrame {
        display_id,
        width,
        height,
        capture_ts_us,
        native,
    } = frame;
    let native = match region {
        Some((rect, display_w, display_h)) => Arc::new(RegionFrame {
            inner: native,
            rect,
            display_w,
            display_h,
        }) as Arc<dyn Any + Send + Sync>,
        None => native,
    };
    CapturedFrame {
        display_id,
        width,
        height,
        capture_ts_us,
        native,
    }
}

/// Removes a RegionFrame wrapper, returning the original native and its region.
pub fn split_native(
    native: &(dyn Any + Send + Sync),
) -> (&(dyn Any + Send + Sync), Option<(PixelRect, u32, u32)>) {
    let Some(region) = native.downcast_ref::<RegionFrame>() else {
        return (native, None);
    };
    (
        region.inner.as_ref(),
        Some((region.rect, region.display_w, region.display_h)),
    )
}

/// Converts a display-space rectangle (after display rotation) to the source
/// rectangle in the unrotated texture.
pub fn texture_source_rect(
    rect: PixelRect,
    display: (u32, u32),
    tex: (u32, u32),
    rotation: u32,
) -> RectU {
    let (tw, th) = tex;
    if display.0 == 0 || display.1 == 0 || tw == 0 || th == 0 {
        return RectU {
            left: 0,
            top: 0,
            right: tw,
            bottom: th,
        };
    }

    let (rot_w, rot_h) = rotated_size(tw, th, rotation);
    let display_w = u64::from(display.0);
    let display_h = u64::from(display.1);
    let scale_x = |value: u64| -> u64 { value.saturating_mul(u64::from(rot_w)) / display_w };
    let scale_y = |value: u64| -> u64 { value.saturating_mul(u64::from(rot_h)) / display_h };
    let ceil_scale_x = |value: u64| -> u64 {
        let numerator = value.saturating_mul(u64::from(rot_w));
        let quotient = numerator / display_w;
        quotient + u64::from(numerator % display_w != 0)
    };
    let ceil_scale_y = |value: u64| -> u64 {
        let numerator = value.saturating_mul(u64::from(rot_h));
        let quotient = numerator / display_h;
        quotient + u64::from(numerator % display_h != 0)
    };

    let x = scale_x(u64::from(rect.x)).min(u64::from(rot_w));
    let y = scale_y(u64::from(rect.y)).min(u64::from(rot_h));
    let right =
        ceil_scale_x(u64::from(rect.x).saturating_add(u64::from(rect.w))).min(u64::from(rot_w));
    let bottom =
        ceil_scale_y(u64::from(rect.y).saturating_add(u64::from(rect.h))).min(u64::from(rot_h));

    let (left, top, source_right, source_bottom) = match vp_rotation(rotation) {
        VpRotation::Identity => (x, y, right, bottom),
        VpRotation::R90 => (
            y,
            u64::from(th).saturating_sub(right),
            bottom,
            u64::from(th).saturating_sub(x),
        ),
        VpRotation::R180 => (
            u64::from(tw).saturating_sub(right),
            u64::from(th).saturating_sub(bottom),
            u64::from(tw).saturating_sub(x),
            u64::from(th).saturating_sub(y),
        ),
        VpRotation::R270 => (
            u64::from(tw).saturating_sub(bottom),
            x,
            u64::from(tw).saturating_sub(y),
            right,
        ),
    };

    RectU {
        left: left.min(u64::from(tw)) as u32,
        top: top.min(u64::from(th)) as u32,
        right: source_right.min(u64::from(tw)) as u32,
        bottom: source_bottom.min(u64::from(th)) as u32,
    }
}
