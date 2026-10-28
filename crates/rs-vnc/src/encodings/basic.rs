//! Raw, CopyRect, Cursor and desktop-size rectangles.

use tracing::warn;

use crate::pixel::{convert_row, is_supported};
use crate::surface::{Screen, Surface};
use crate::types::{PixelFormat, Rect, VncError, MAX_FB_SIDE};

fn protocol(message: &'static str) -> VncError {
    VncError::Protocol(message.to_owned())
}

fn check_fits(r: Rect, s: &dyn Surface) -> Result<(), VncError> {
    let (width, height) = s.size();
    if r.fits(width, height) {
        Ok(())
    } else {
        Err(protocol("rect out of bounds"))
    }
}

/// Number of wire bytes in a Raw rectangle.
pub fn raw_len(pf: &PixelFormat, r: Rect) -> usize {
    usize::from(r.w) * usize::from(r.h) * pf.bytes_per_pixel()
}

/// Decode a Raw rectangle into a BGRA surface.
pub fn decode_raw(
    pf: &PixelFormat,
    r: Rect,
    data: &[u8],
    s: &mut dyn Surface,
) -> Result<(), VncError> {
    check_fits(r, s)?;
    if data.len() != raw_len(pf, r) {
        return Err(protocol("invalid raw rectangle length"));
    }
    if r.is_empty() {
        return Ok(());
    }
    if !is_supported(pf) {
        return Err(protocol("unsupported pixel format"));
    }

    let width = usize::from(r.w);
    let height = usize::from(r.h);
    let source_stride = width * pf.bytes_per_pixel();
    let destination_stride = width * 4;
    let mut bgra = vec![0u8; destination_stride * height];
    for row in 0..height {
        let source = &data[row * source_stride..(row + 1) * source_stride];
        let destination = &mut bgra[row * destination_stride..(row + 1) * destination_stride];
        convert_row(pf, source, destination);
    }
    s.put_bgra(r, &bgra, destination_stride);
    Ok(())
}

pub const COPY_RECT_LEN: usize = 4;

/// Decode a CopyRect rectangle.
pub fn decode_copy_rect(r: Rect, data: &[u8], s: &mut dyn Surface) -> Result<(), VncError> {
    check_fits(r, s)?;
    if data.len() != COPY_RECT_LEN {
        return Err(protocol("invalid copy rect length"));
    }
    let source_x = u16::from_be_bytes([data[0], data[1]]);
    let source_y = u16::from_be_bytes([data[2], data[3]]);
    let source = Rect::new(source_x, source_y, r.w, r.h);
    if !source.fits(s.size().0, s.size().1) {
        return Err(protocol("rect out of bounds"));
    }
    s.copy_rect(source_x, source_y, r);
    Ok(())
}

/// Number of bytes in a Cursor pseudo-encoding rectangle.
pub fn cursor_len(pf: &PixelFormat, w: u16, h: u16) -> usize {
    let pixels = usize::from(w) * usize::from(h) * pf.bytes_per_pixel();
    let mask_stride = usize::from(w).div_ceil(8);
    pixels + mask_stride * usize::from(h)
}

/// Decode a Cursor pseudo-encoding rectangle.
pub fn decode_cursor(
    pf: &PixelFormat,
    r: Rect,
    data: &[u8],
    s: &mut dyn Surface,
) -> Result<(), VncError> {
    if data.len() != cursor_len(pf, r.w, r.h) {
        return Err(protocol("invalid cursor rectangle length"));
    }
    if r.w == 0 || r.h == 0 {
        s.cursor(0, 0, 0, 0, &[]);
        return Ok(());
    }
    if !is_supported(pf) {
        return Err(protocol("unsupported pixel format"));
    }

    let width = usize::from(r.w);
    let height = usize::from(r.h);
    let source_stride = width * pf.bytes_per_pixel();
    let destination_stride = width * 4;
    let pixel_bytes = source_stride * height;
    let mask_stride = width.div_ceil(8);
    let mut bgra = vec![0u8; destination_stride * height];

    for row in 0..height {
        let source = &data[row * source_stride..(row + 1) * source_stride];
        let destination = &mut bgra[row * destination_stride..(row + 1) * destination_stride];
        convert_row(pf, source, destination);
        let mask_row =
            &data[pixel_bytes + row * mask_stride..pixel_bytes + (row + 1) * mask_stride];
        for column in 0..width {
            if mask_row[column / 8] & (0x80 >> (column % 8)) == 0 {
                destination[column * 4..column * 4 + 4].fill(0);
            }
        }
    }

    let hot_x = r.x.min(r.w - 1);
    let hot_y = r.y.min(r.h - 1);
    s.cursor(r.w, r.h, hot_x, hot_y, &bgra);
    Ok(())
}

/// Apply a DesktopSize pseudo-encoding rectangle.
pub fn apply_desktop_size(r: Rect, s: &mut dyn Surface) -> Result<(), VncError> {
    if r.w == 0 || r.h == 0 || r.w > MAX_FB_SIDE || r.h > MAX_FB_SIDE {
        return Err(protocol("invalid desktop size"));
    }
    s.resize(r.w, r.h);
    Ok(())
}

/// Return the ExtendedDesktopSize payload length for a screen count byte.
pub fn extended_desktop_size_len(first_byte: u8) -> usize {
    4 + usize::from(first_byte) * 16
}

/// Apply an ExtendedDesktopSize pseudo-encoding rectangle. `r.y` is the
/// status: non-zero answers a refused request and leaves the size alone. A
/// screen list that fits the framebuffer goes to [`Surface::screens`].
pub fn apply_extended_desktop_size(
    r: Rect,
    data: &[u8],
    s: &mut dyn Surface,
) -> Result<(), VncError> {
    let Some(&screen_count) = data.first() else {
        return Err(protocol("invalid extended desktop size length"));
    };
    if data.len() != extended_desktop_size_len(screen_count) {
        return Err(protocol("invalid extended desktop size length"));
    }
    if r.y == 0 {
        apply_desktop_size(r, s)?;
    }
    let screens: Vec<Screen> = data[4..].chunks_exact(16).map(decode_screen).collect();
    if screens.is_empty() {
        return Ok(());
    }
    let (width, height) = s.size();
    if screens
        .iter()
        .all(|screen| !screen.rect.is_empty() && screen.rect.fits(width, height))
    {
        s.screens(&screens);
    } else {
        warn!(
            count = screens.len(),
            width, height, "ignoring a screen layout outside the framebuffer"
        );
    }
    Ok(())
}

/// One 16-byte SCREEN entry: id, x, y, width, height, flags.
fn decode_screen(b: &[u8]) -> Screen {
    let u16_at = |at: usize| u16::from_be_bytes([b[at], b[at + 1]]);
    let u32_at = |at: usize| u32::from_be_bytes([b[at], b[at + 1], b[at + 2], b[at + 3]]);
    Screen {
        id: u32_at(0),
        rect: Rect::new(u16_at(4), u16_at(6), u16_at(8), u16_at(10)),
        flags: u32_at(12),
    }
}

/// Apply a PointerPos pseudo-encoding rectangle.
pub fn apply_pointer_pos(r: Rect, s: &mut dyn Surface) {
    let (width, height) = s.size();
    let x = r.x.min(width.saturating_sub(1));
    let y = r.y.min(height.saturating_sub(1));
    s.pointer_pos(x, y);
}
