//! Pure capture-side models shared by the Linux PipeWire and X11 backends.

use std::collections::HashMap;

use host_core::platform::{CaptureConfig, CapturedFrame, FrameSink};
use rsp_core::pb;

const HEADER_CORRUPTED: u32 = 1 << 1;
const CHUNK_CORRUPTED: i32 = 1 << 0;
const CHUNK_EMPTY: i32 = 1 << 1;

/// The pieces of PipeWire metadata needed to decide whether a buffer carries
/// a frame, cursor update, or no useful update.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct BufferInfo {
    /// `spa_meta_header.flags`; `None` means that header metadata was absent.
    pub header_flags: Option<u32>,
    /// Number of valid regions in VideoDamage; `None` means that metadata was absent.
    pub damage_regions: Option<usize>,
    pub chunk_size: u32,
    pub chunk_flags: i32,
    /// The buffer brought a new cursor position or shape.
    pub cursor_changed: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FrameDecision {
    Encode,
    CursorOnly,
    Skip,
}

/// Tracks whether a stream still needs its first complete frame.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct FrameGate {
    fresh: bool,
}

impl FrameGate {
    /// Creates a gate that requires one complete frame before trusting damage metadata.
    pub fn new() -> Self {
        Self { fresh: true }
    }

    /// Re-arms the first-frame path after a format or stream restart.
    pub fn reset(&mut self) {
        self.fresh = true;
    }

    pub fn decide(&mut self, info: &BufferInfo) -> FrameDecision {
        let cursor_or_skip = || {
            if info.cursor_changed {
                FrameDecision::CursorOnly
            } else {
                FrameDecision::Skip
            }
        };

        if info
            .header_flags
            .is_some_and(|flags| flags & HEADER_CORRUPTED != 0)
            || info.chunk_flags & CHUNK_CORRUPTED != 0
        {
            return cursor_or_skip();
        }
        if info.chunk_flags & CHUNK_EMPTY != 0 || info.chunk_size == 0 {
            return cursor_or_skip();
        }
        if self.fresh {
            self.fresh = false;
            return FrameDecision::Encode;
        }
        if info.damage_regions == Some(0) {
            return cursor_or_skip();
        }
        FrameDecision::Encode
    }
}

impl Default for FrameGate {
    fn default() -> Self {
        Self::new()
    }
}

pub const MAX_CURSOR_SIDE: u32 = 256;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum CursorPixels {
    Bgra,
    Rgba,
    Argb,
    Abgr,
}

/// Maps SPA's video format ids to the byte order used by a cursor bitmap.
pub fn cursor_pixels_from_spa(spa_format: u32) -> Option<CursorPixels> {
    match spa_format {
        12 => Some(CursorPixels::Bgra),
        11 => Some(CursorPixels::Rgba),
        13 => Some(CursorPixels::Argb),
        14 => Some(CursorPixels::Abgr),
        _ => None,
    }
}

/// Converts XFixes' little-endian ARGB words to BGRA bytes.
pub fn argb32_to_bgra(pixels: &[u32]) -> Vec<u8> {
    let mut out = Vec::with_capacity(pixels.len().saturating_mul(4));
    for &pixel in pixels {
        let [blue, green, red, alpha] = pixel.to_le_bytes();
        out.extend_from_slice(&[blue, green, red, alpha]);
    }
    out
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum CursorError {
    #[error("empty cursor")]
    Empty,
    #[error("cursor too large")]
    TooLarge,
    #[error("cursor bitmap truncated")]
    Truncated,
    #[error("png: {0}")]
    Png(String),
}

/// Converts a cursor bitmap into the RSP RGBA8 PNG representation.
#[allow(clippy::too_many_arguments)]
pub fn cursor_to_shape(
    px: CursorPixels,
    width: u32,
    height: u32,
    stride: u32,
    pixels: &[u8],
    hot_x: i32,
    hot_y: i32,
    scale: f32,
    premultiplied: bool,
) -> Result<pb::CursorShape, CursorError> {
    if width == 0 || height == 0 {
        return Err(CursorError::Empty);
    }
    if width > MAX_CURSOR_SIDE || height > MAX_CURSOR_SIDE {
        return Err(CursorError::TooLarge);
    }

    let width_usize = usize::try_from(width).map_err(|_| CursorError::Truncated)?;
    let height_usize = usize::try_from(height).map_err(|_| CursorError::Truncated)?;
    let row_len = width_usize.checked_mul(4).ok_or(CursorError::Truncated)?;
    let stride_usize = usize::try_from(stride).map_err(|_| CursorError::Truncated)?;
    let required = stride_usize
        .checked_mul(height_usize.saturating_sub(1))
        .and_then(|n| n.checked_add(row_len))
        .ok_or(CursorError::Truncated)?;
    if stride_usize < row_len || pixels.len() < required {
        return Err(CursorError::Truncated);
    }

    let mut rgba = Vec::with_capacity(row_len.saturating_mul(height_usize));
    for row in 0..height_usize {
        let start = row * stride_usize;
        let source = &pixels[start..start + row_len];
        for raw in source.chunks_exact(4) {
            let (red, green, blue, alpha) = match px {
                CursorPixels::Bgra => (raw[2], raw[1], raw[0], raw[3]),
                CursorPixels::Rgba => (raw[0], raw[1], raw[2], raw[3]),
                CursorPixels::Argb => (raw[1], raw[2], raw[3], raw[0]),
                CursorPixels::Abgr => (raw[3], raw[2], raw[1], raw[0]),
            };
            let (red, green, blue) = if premultiplied {
                unpremultiply(red, green, blue, alpha)
            } else {
                (red, green, blue)
            };
            rgba.extend_from_slice(&[red, green, blue, alpha]);
        }
    }

    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| CursorError::Png(error.to_string()))?;
        writer
            .write_image_data(&rgba)
            .map_err(|error| CursorError::Png(error.to_string()))?;
        writer
            .finish()
            .map_err(|error| CursorError::Png(error.to_string()))?;
    }

    let max_hot_x = width - 1;
    let max_hot_y = height - 1;
    let hot_x = hot_x.clamp(0, max_hot_x as i32) as u32;
    let hot_y = hot_y.clamp(0, max_hot_y as i32) as u32;
    Ok(pb::CursorShape {
        id: rsp_core::cursor::shape_id(&png),
        width,
        height,
        hot_x,
        hot_y,
        scale,
        png,
    })
}

fn unpremultiply(red: u8, green: u8, blue: u8, alpha: u8) -> (u8, u8, u8) {
    if alpha == 0 {
        return (0, 0, 0);
    }
    if alpha == u8::MAX {
        return (red, green, blue);
    }
    let convert = |component: u8| {
        let value = (u32::from(component) * 255 + u32::from(alpha) / 2) / u32::from(alpha);
        value.min(255) as u8
    };
    (convert(red), convert(green), convert(blue))
}

/// Creates a report whose coordinates describe the cursor's top-left corner.
pub fn pointer_report(
    display_id: u32,
    seq: i64,
    visible: bool,
    pos: (i32, i32),
    hot: (i32, i32),
    size: (u32, u32),
) -> host_core::capture_model::PointerReport {
    host_core::capture_model::PointerReport {
        display_id,
        update_time: seq.max(1),
        visible,
        x: pos.0.saturating_sub(hot.0),
        y: pos.1.saturating_sub(hot.1),
        width: size.0,
        height: size.1,
    }
}

/// Stable FNV-1a id for a RandR output name.
pub fn display_id_for_name(name: &str) -> u32 {
    let mut hash = 0x811C9DC5_u32;
    for byte in name.as_bytes() {
        hash ^= u32::from(*byte);
        hash = hash.wrapping_mul(0x01000193);
    }
    let id = hash & 0x7FFF_FFFF;
    if id == 0 {
        1
    } else {
        id
    }
}

struct Entry {
    id: u64,
    sink: FrameSink,
    cfg: CaptureConfig,
}

/// Fans each native capture stream out to all clients watching the same display.
pub struct CaptureHub<K> {
    entries: HashMap<K, Vec<Entry>>,
    next_id: u64,
    dropped: u64,
}

impl<K: Eq + std::hash::Hash + Clone> CaptureHub<K> {
    pub fn new() -> Self {
        Self {
            entries: HashMap::new(),
            next_id: 1,
            dropped: 0,
        }
    }

    /// Adds a sink and reports whether this was the first sink for `key`.
    pub fn add(&mut self, key: K, sink: FrameSink, cfg: CaptureConfig) -> (u64, bool) {
        let list = self.entries.entry(key).or_default();
        let first = list.is_empty();
        let id = self.next_id;
        self.next_id = self.next_id.wrapping_add(1);
        list.push(Entry { id, sink, cfg });
        (id, first)
    }

    /// Removes one sink and reports whether no sinks remain for this key.
    pub fn remove(&mut self, key: &K, id: u64) -> bool {
        let Some(list) = self.entries.get_mut(key) else {
            return false;
        };
        let Some(index) = list.iter().position(|entry| entry.id == id) else {
            return false;
        };
        list.remove(index);
        if list.is_empty() {
            self.entries.remove(key);
            true
        } else {
            false
        }
    }

    /// Tries to send a clone to every sink, removing closed receivers.
    pub fn deliver(&mut self, key: &K, frame: &CapturedFrame) -> usize {
        let Some(mut list) = self.entries.remove(key) else {
            return 0;
        };
        list.retain(|entry| match entry.sink.try_send(frame.clone()) {
            Ok(()) => true,
            Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                self.dropped = self.dropped.saturating_add(1);
                true
            }
            Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
        });
        let remaining = list.len();
        if remaining != 0 {
            self.entries.insert(key.clone(), list);
        }
        remaining
    }

    /// Returns the aggregate capture request for a display.
    pub fn effective(&self, key: &K) -> Option<CaptureConfig> {
        let list = self.entries.get(key)?;
        if list.is_empty() {
            return None;
        }
        let fps = list.iter().map(|entry| entry.cfg.fps).max().unwrap_or(0);
        let max_width = if list.iter().any(|entry| entry.cfg.max_width == 0) {
            0
        } else {
            list.iter()
                .map(|entry| entry.cfg.max_width)
                .max()
                .unwrap_or(0)
        };
        let max_height = if list.iter().any(|entry| entry.cfg.max_height == 0) {
            0
        } else {
            list.iter()
                .map(|entry| entry.cfg.max_height)
                .max()
                .unwrap_or(0)
        };
        Some(CaptureConfig {
            fps,
            max_width,
            max_height,
        })
    }

    pub fn keys(&self) -> Vec<K> {
        self.entries.keys().cloned().collect()
    }

    pub fn dropped(&self) -> u64 {
        self.dropped
    }
}

impl<K: Eq + std::hash::Hash + Clone> Default for CaptureHub<K> {
    fn default() -> Self {
        Self::new()
    }
}

/// A capture handle that runs a cleanup callback exactly once when dropped.
pub struct HubHandle {
    on_drop: Option<Box<dyn FnOnce() + Send>>,
}

impl HubHandle {
    pub fn new(on_drop: impl FnOnce() + Send + 'static) -> Self {
        Self {
            on_drop: Some(Box::new(on_drop)),
        }
    }
}

impl host_core::platform::CaptureHandle for HubHandle {}

impl Drop for HubHandle {
    fn drop(&mut self) {
        if let Some(on_drop) = self.on_drop.take() {
            on_drop();
        }
    }
}

pub const DEVIATION_PX: f64 = 3.0;
pub const INJECT_SETTLE_US: u64 = 150_000;

#[derive(Debug, Default)]
pub struct CursorDeviation {
    last_cursor: Option<(u32, f64, f64)>,
    last_inject: Option<(u32, f64, f64, u64)>,
}

impl CursorDeviation {
    pub fn new() -> Self {
        Self::default()
    }

    /// Records the target of an injected cursor move.
    pub fn on_injected(&mut self, display: u32, x: f64, y: f64, now_us: u64) {
        self.last_inject = Some((display, x, y, now_us));
    }

    /// Returns whether a cursor movement is likely local user activity.
    pub fn on_cursor(&mut self, display: u32, x: f64, y: f64, now_us: u64) -> bool {
        let previous = self.last_cursor;
        let first = previous.is_none();
        let moved = match previous {
            None => true,
            Some((old_display, old_x, old_y)) => {
                old_display != display || (x - old_x).hypot(y - old_y) > DEVIATION_PX
            }
        };
        self.last_cursor = Some((display, x, y));
        if !moved {
            return false;
        }
        if let Some((injected_display, injected_x, injected_y, injected_at)) = self.last_inject {
            if now_us.saturating_sub(injected_at) < INJECT_SETTLE_US {
                return false;
            }
            if injected_display == display && (x - injected_x).hypot(y - injected_y) <= DEVIATION_PX
            {
                return false;
            }
        }
        if first {
            return false;
        }
        true
    }
}
