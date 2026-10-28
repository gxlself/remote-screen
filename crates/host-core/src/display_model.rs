//! Platform-independent display geometry and identity helpers.

use std::cmp::Ordering;

use crate::platform::DisplayInfo;
use tracing::warn;
use xxhash_rust::xxh3::xxh3_64;

/// A monitor rectangle in physical pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectPx {
    pub left: i32,
    pub top: i32,
    pub width: u32,
    pub height: u32,
}

/// The native values collected for one Windows monitor.
#[derive(Debug, Clone, PartialEq)]
pub struct MonitorRecord {
    pub device_name: String,
    pub friendly_name: String,
    pub rect: RectPx,
    pub primary: bool,
    pub dpi: u32,
    pub refresh_hz: u32,
    pub orientation: u32,
}

const DISPLAY_PREFIX: &str = r"\\.\DISPLAY";

/// Return the stable display id used by the Windows platform layer.
pub fn display_id(device_name: &str) -> u32 {
    if let Some(prefix) = device_name.get(..DISPLAY_PREFIX.len()) {
        if prefix.eq_ignore_ascii_case(DISPLAY_PREFIX) {
            let suffix = &device_name[DISPLAY_PREFIX.len()..];
            if !suffix.is_empty() && suffix.bytes().all(|byte| byte.is_ascii_digit()) {
                if let Ok(number) = suffix.parse::<u32>() {
                    if (1..=0x7fff_ffff).contains(&number) {
                        return number;
                    }
                }
            }
        }
    }

    (xxh3_64(device_name.as_bytes()) as u32) | 0x8000_0000
}

/// Convert native records to the platform-independent display representation.
pub fn to_display_infos(records: &[MonitorRecord]) -> Vec<DisplayInfo> {
    let mut displays = Vec::with_capacity(records.len());
    let mut ids = Vec::with_capacity(records.len());

    for record in records {
        let id = display_id(&record.device_name);
        if ids.contains(&id) {
            warn!(
                device_name = %record.device_name,
                display_id = id,
                "duplicate Windows display id; keeping the first record"
            );
            continue;
        }
        ids.push(id);

        displays.push(DisplayInfo {
            id,
            name: if record.friendly_name.is_empty() {
                record.device_name.clone()
            } else {
                record.friendly_name.clone()
            },
            width_px: record.rect.width,
            height_px: record.rect.height,
            scale: if record.dpi == 0 {
                1.0
            } else {
                record.dpi as f32 / 96.0
            },
            // Windows supplies physical coordinates here.  Mixed-DPI logical
            // desktop coordinates do not have one consistent interpretation.
            x: record.rect.left,
            y: record.rect.top,
            refresh_hz: match record.refresh_hz {
                0 | 1 => 60,
                refresh_hz => refresh_hz,
            },
            primary: record.primary,
            rotation: rotation_degrees(record.orientation),
        });
    }

    displays.sort_by(|left, right| {
        right
            .primary
            .cmp(&left.primary)
            .then_with(|| left.x.cmp(&right.x))
            .then_with(|| left.y.cmp(&right.y))
    });
    displays
}

/// Return the physical-pixel union of all monitor rectangles.
pub fn virtual_desk(records: &[MonitorRecord]) -> RectPx {
    let Some(first) = records.first() else {
        return RectPx {
            left: 0,
            top: 0,
            width: 1,
            height: 1,
        };
    };

    let mut left = i64::from(first.rect.left);
    let mut top = i64::from(first.rect.top);
    let mut right = rect_right(first.rect);
    let mut bottom = rect_bottom(first.rect);
    for record in &records[1..] {
        left = left.min(i64::from(record.rect.left));
        top = top.min(i64::from(record.rect.top));
        right = right.max(rect_right(record.rect));
        bottom = bottom.max(rect_bottom(record.rect));
    }

    RectPx {
        left: saturating_i64_to_i32(left),
        top: saturating_i64_to_i32(top),
        width: nonnegative_extent_to_u32(right - left),
        height: nonnegative_extent_to_u32(bottom - top),
    }
}

/// Find the first native monitor record with the requested stable id.
pub fn rect_for(records: &[MonitorRecord], requested_id: u32) -> Option<RectPx> {
    records
        .iter()
        .find(|record| display_id(&record.device_name) == requested_id)
        .map(|record| record.rect)
}

/// Convert the Windows display orientation enum to clockwise degrees.
pub fn rotation_degrees(orientation: u32) -> u32 {
    match orientation {
        0 => 0,
        1 => 90,
        2 => 180,
        3 => 270,
        _ => 0,
    }
}

/// Compare sorted display lists for any observable change.
pub fn displays_changed(old: &[DisplayInfo], new: &[DisplayInfo]) -> bool {
    old != new
}

fn rect_right(rect: RectPx) -> i64 {
    i64::from(rect.left) + i64::from(rect.width)
}

fn rect_bottom(rect: RectPx) -> i64 {
    i64::from(rect.top) + i64::from(rect.height)
}

fn saturating_i64_to_i32(value: i64) -> i32 {
    match value.cmp(&i64::from(i32::MIN)) {
        Ordering::Less => i32::MIN,
        Ordering::Equal | Ordering::Greater => {
            if value > i64::from(i32::MAX) {
                i32::MAX
            } else {
                value as i32
            }
        }
    }
}

fn nonnegative_extent_to_u32(value: i64) -> u32 {
    if value <= 0 {
        0
    } else if value > i64::from(u32::MAX) {
        u32::MAX
    } else {
        value as u32
    }
}
