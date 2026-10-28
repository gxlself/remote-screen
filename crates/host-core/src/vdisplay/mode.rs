//! 虚拟屏模式选择（M3 · V1）：请求 + 平台能力 → 实际模式。纯函数，无 I/O。

use std::cmp::Ordering;

use super::{VdCaps, VdMode};

/// App 请求（来自 pb::CreateVirtualDisplay）。scale 为 0 或非有限值表示由 Host 选；
/// refresh_hz 为 0 表示 60。
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct VdRequest {
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f32,
    pub refresh_hz: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum ModeError {
    #[error("width and height are required")]
    MissingSize,
    #[error("requested size {width}x{height} is below the minimum")]
    TooSmall { width: u32, height: u32 },
}

/// Select an actual mode from an app request and the platform capabilities.
pub fn choose(req: &VdRequest, caps: &VdCaps) -> Result<VdMode, ModeError> {
    if req.width_px == 0 || req.height_px == 0 {
        return Err(ModeError::MissingSize);
    }

    let landscape = req.width_px >= req.height_px;
    let long = u64::from(req.width_px.max(req.height_px));
    let short = u64::from(req.width_px.min(req.height_px));
    let max_long = u64::from(caps.max_long_px);
    let max_short = u64::from(caps.max_short_px);

    let (long1, short1) = if long <= max_long && short <= max_short {
        (long, short)
    } else if max_long * short <= max_short * long {
        (max_long, short * max_long / long)
    } else {
        (long * max_short / short, max_short)
    };

    let align = u64::from(caps.align_px.max(2));
    let long2 = long1 / align * align;
    let short2 = short1 / align * align;

    if long2 < u64::from(caps.min_long_px) || short2 < u64::from(caps.min_short_px) {
        return Err(ModeError::TooSmall {
            width: req.width_px,
            height: req.height_px,
        });
    }

    let scale = select_scale(req.scale, short2, &caps.scales);
    let target_refresh = if req.refresh_hz == 0 {
        60
    } else {
        req.refresh_hz
    };
    let refresh_hz = select_refresh(target_refresh, &caps.refresh_rates);

    let (width_px, height_px) = if landscape {
        (long2 as u32, short2 as u32)
    } else {
        (short2 as u32, long2 as u32)
    };

    Ok(VdMode {
        width_px,
        height_px,
        scale,
        refresh_hz,
    })
}

fn select_scale(requested: f32, short_px: u64, scales: &[f32]) -> f32 {
    let Some(&first) = scales.first() else {
        return 1.0;
    };

    if requested.is_finite() && requested > 0.0 {
        let mut best = first;
        let mut best_diff = (best - requested).abs();
        for &candidate in &scales[1..] {
            let difference = (candidate - requested).abs();
            let better = difference.total_cmp(&best_diff) == Ordering::Less
                || (difference.total_cmp(&best_diff) == Ordering::Equal
                    && candidate.total_cmp(&best) == Ordering::Less);
            if better {
                best = candidate;
                best_diff = difference;
            }
        }
        return best;
    }

    let has_two = scales.iter().any(|&scale| (scale - 2.0).abs() < 1e-3);
    if has_two && short_px / 2 >= 600 {
        return 2.0;
    }

    let has_one = scales.iter().any(|&scale| (scale - 1.0).abs() < 1e-3);
    if has_one {
        1.0
    } else {
        first
    }
}

fn select_refresh(target: u32, rates: &[u32]) -> u32 {
    let Some(&minimum) = rates.iter().min() else {
        return 60;
    };
    rates
        .iter()
        .copied()
        .filter(|&rate| rate <= target)
        .max()
        .unwrap_or(minimum)
}

/// EDID physical size in millimetres.
pub fn physical_size_mm(mode: &VdMode) -> (u32, u32) {
    let scale = f64::from(mode.scale.max(1.0));
    let width = (f64::from(mode.width_px) * 25.4 / (96.0 * scale)).round() as u32;
    let height = (f64::from(mode.height_px) * 25.4 / (96.0 * scale)).round() as u32;
    (width, height)
}

/// Return the logical point dimensions of a mode.
pub fn points(mode: &VdMode) -> (u32, u32) {
    let scale = if mode.scale.is_finite() && mode.scale > 0.0 {
        f64::from(mode.scale)
    } else {
        1.0
    };
    let width = (f64::from(mode.width_px) / scale).round() as u32;
    let height = (f64::from(mode.height_px) / scale).round() as u32;
    (width, height)
}
