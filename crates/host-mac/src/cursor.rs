//! macOS cursor polling and the platform-independent cursor decisions.

use std::collections::HashSet;
use std::ffi::c_double;
use std::thread;
use std::time::Duration;

use host_core::platform::{CursorUpdate, DisplayInfo};
use rsp_core::{cursor::shape_id, pb};
use tokio::sync::mpsc;

const CURSOR_PNG_CAPACITY: usize = 4 * 1024 * 1024;

extern "C" {
    fn rs_cursor_snapshot(
        x: *mut c_double,
        y: *mut c_double,
        visible: *mut i32,
        png: *mut u8,
        png_capacity: usize,
        png_len: *mut usize,
        width: *mut u32,
        height: *mut u32,
        hot_x: *mut c_double,
        hot_y: *mut c_double,
        scale: *mut c_double,
    ) -> i32;
}

/// One native cursor sample. Coordinates are in the global macOS desktop
/// point space; the PNG is already encoded at the selected cursor scale.
#[derive(Debug, Default)]
pub struct Snapshot {
    pub x: f64,
    pub y: f64,
    pub visible: bool,
    pub png: Vec<u8>,
    pub width: u32,
    pub height: u32,
    pub hot_x: f64,
    pub hot_y: f64,
    pub scale: f64,
}

/// The result of deciding whether a shape needs to be sent in full or reused.
#[derive(Debug, Clone, PartialEq)]
pub enum ShapeDecision {
    Shape(pb::CursorShape),
    Use(u64),
}

/// Decide the cursor stream message for `png` without mutating the known-shape
/// set. The caller inserts the id only after a successful channel send, so a
/// full channel cannot make a dropped `Shape` look cached.
pub fn shape_decision(
    png: &[u8],
    width: u32,
    height: u32,
    hot_x: f64,
    hot_y: f64,
    scale: f64,
    known: &HashSet<u64>,
) -> ShapeDecision {
    let id = shape_id(png);
    if known.contains(&id) {
        ShapeDecision::Use(id)
    } else {
        ShapeDecision::Shape(pb::CursorShape {
            id,
            width,
            height,
            hot_x: hot_x.max(0.0).round() as u32,
            hot_y: hot_y.max(0.0).round() as u32,
            scale: if scale.is_finite() && scale > 0.0 {
                scale as f32
            } else {
                1.0
            },
            png: png.to_vec(),
        })
    }
}

/// Convert a global desktop point into the display-relative normalized
/// coordinate used by RSP/1. Display geometry is in logical points while the
/// width and height fields are physical pixels.
pub fn normalize_position(x: f64, y: f64, displays: &[DisplayInfo]) -> (u32, f32, f32) {
    let mut nearest: Option<(&DisplayInfo, f64)> = None;
    for display in displays {
        let scale = if display.scale.is_finite() && display.scale > 0.0 {
            f64::from(display.scale)
        } else {
            1.0
        };
        let width = f64::from(display.width_px) / scale;
        let height = f64::from(display.height_px) / scale;
        if width <= 0.0 || height <= 0.0 {
            continue;
        }
        let left = f64::from(display.x);
        let top = f64::from(display.y);
        let right = left + width;
        let bottom = top + height;
        if x >= left && x < right && y >= top && y < bottom {
            return (
                display.id,
                ((x - left) / width).clamp(0.0, 1.0) as f32,
                ((y - top) / height).clamp(0.0, 1.0) as f32,
            );
        }
        let dx = if x < left {
            left - x
        } else if x > right {
            x - right
        } else {
            0.0
        };
        let dy = if y < top {
            top - y
        } else if y > bottom {
            y - bottom
        } else {
            0.0
        };
        let distance = dx * dx + dy * dy;
        if nearest.as_ref().is_none_or(|(_, old)| distance < *old) {
            nearest = Some((display, distance));
        }
    }
    let Some((display, _)) = nearest else {
        return (0, 0.5, 0.5);
    };
    let scale = if display.scale.is_finite() && display.scale > 0.0 {
        f64::from(display.scale)
    } else {
        1.0
    };
    let width = (f64::from(display.width_px) / scale).max(1.0);
    let height = (f64::from(display.height_px) / scale).max(1.0);
    (
        display.id,
        ((x - f64::from(display.x)) / width).clamp(0.0, 1.0) as f32,
        ((y - f64::from(display.y)) / height).clamp(0.0, 1.0) as f32,
    )
}

fn snapshot(buffer: &mut [u8]) -> Snapshot {
    let mut result = Snapshot::default();
    let mut visible = 0i32;
    let mut png_len = 0usize;
    let mut width = 0u32;
    let mut height = 0u32;
    let mut hot_x = 0.0;
    let mut hot_y = 0.0;
    let mut scale = 1.0;
    // SAFETY: all pointers refer to local outputs or the live writable buffer;
    // the shim copies the PNG before returning.
    let status = unsafe {
        rs_cursor_snapshot(
            &mut result.x,
            &mut result.y,
            &mut visible,
            buffer.as_mut_ptr(),
            buffer.len(),
            &mut png_len,
            &mut width,
            &mut height,
            &mut hot_x,
            &mut hot_y,
            &mut scale,
        )
    };
    result.visible = visible != 0;
    if status >= 0 && png_len <= buffer.len() {
        result.png = buffer[..png_len].to_vec();
        result.width = width;
        result.height = height;
        result.hot_x = hot_x;
        result.hot_y = hot_y;
        result.scale = scale;
    }
    result
}

enum SendOutcome {
    Sent,
    Full,
    Closed,
}

fn try_send(tx: &mpsc::Sender<CursorUpdate>, update: CursorUpdate) -> SendOutcome {
    match tx.try_send(update) {
        Ok(()) => SendOutcome::Sent,
        Err(mpsc::error::TrySendError::Full(_)) => SendOutcome::Full,
        Err(mpsc::error::TrySendError::Closed(_)) => SendOutcome::Closed,
    }
}

/// Spawn one independent 33 ms cursor watcher. The callback is used to fetch
/// current display geometry without coupling this module to `MacPlatform`.
pub fn watch<F>(tx: mpsc::Sender<CursorUpdate>, displays: F)
where
    F: Fn() -> Vec<DisplayInfo> + Send + 'static,
{
    thread::spawn(move || {
        let mut buffer = vec![0u8; CURSOR_PNG_CAPACITY];
        let mut known_shapes = HashSet::new();
        let mut last_shape = None;
        let mut last_position: Option<(u32, f32, f32)> = None;
        let mut last_visible = None;
        let mut sequence = 0u32;

        loop {
            if tx.is_closed() {
                return;
            }
            let sample = snapshot(&mut buffer);
            let display_list = displays();
            let (display_id, x, y) = normalize_position(sample.x, sample.y, &display_list);

            if last_position != Some((display_id, x, y)) {
                let update = CursorUpdate::Position {
                    display_id,
                    seq: sequence,
                    x,
                    y,
                    visible: sample.visible,
                };
                match try_send(&tx, update) {
                    SendOutcome::Sent => {
                        last_position = Some((display_id, x, y));
                        sequence = sequence.wrapping_add(1);
                    }
                    SendOutcome::Full => {}
                    SendOutcome::Closed => return,
                }
            }

            if last_visible != Some(sample.visible) {
                if sample.visible || last_visible.is_some() {
                    match try_send(&tx, CursorUpdate::Hidden(!sample.visible)) {
                        SendOutcome::Sent => last_visible = Some(sample.visible),
                        SendOutcome::Full => {}
                        SendOutcome::Closed => return,
                    }
                } else {
                    last_visible = Some(sample.visible);
                }
            }

            if !sample.png.is_empty() {
                let id = shape_id(&sample.png);
                if last_shape != Some(id) {
                    let decision = shape_decision(
                        &sample.png,
                        sample.width,
                        sample.height,
                        sample.hot_x,
                        sample.hot_y,
                        sample.scale,
                        &known_shapes,
                    );
                    let sent = match decision {
                        ShapeDecision::Shape(shape) => try_send(&tx, CursorUpdate::Shape(shape)),
                        ShapeDecision::Use(id) => try_send(&tx, CursorUpdate::Use(id)),
                    };
                    match sent {
                        SendOutcome::Sent => {
                            known_shapes.insert(id);
                            last_shape = Some(id);
                        }
                        SendOutcome::Full => {}
                        SendOutcome::Closed => return,
                    }
                }
            }
            thread::sleep(Duration::from_millis(33));
        }
    });
}
