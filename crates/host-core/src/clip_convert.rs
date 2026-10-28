//! Clipboard format conversion shared by the Windows implementation and tests.
//!
//! Validation and wire normalization are independent of Win32 handles.

use std::io::Cursor;

use crate::platform::{ClipboardItem, PlatformError};
use rsp_core::pb;
use sha2::{Digest, Sha256};

pub const MAX_TEXT_BYTES: usize = 1 << 20;
pub const MAX_PNG_BYTES: usize = 10 << 20;
pub const MAX_IMAGE_SIDE: u32 = 16_384;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ClipError {
    #[error("unsupported bitmap")]
    Unsupported,
    #[error("bad bitmap")]
    Bad,
    #[error("image too large")]
    TooLarge,
    #[error("png: {0}")]
    Png(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipFormat {
    UnicodeText,
    Png,
    DibV5,
    Dib,
}

/// Convert a Windows UTF-16 clipboard string to the LF-normalized wire text.
pub fn text_to_wire(utf16: &[u16]) -> Option<String> {
    let end = utf16
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(utf16.len());
    let mut text = String::from_utf16_lossy(&utf16[..end]);
    if text.contains("\r\n") {
        text = text.replace("\r\n", "\n");
    }
    if text.is_empty() || text.len() > MAX_TEXT_BYTES {
        None
    } else {
        Some(text)
    }
}

/// Convert LF wire text to a Windows UTF-16 string including its NUL terminator.
pub fn text_from_wire(text: &str) -> Vec<u16> {
    let mut windows = String::with_capacity(text.len() + text.matches('\n').count());
    let mut previous_was_cr = false;
    for ch in text.chars() {
        if ch == '\n' && !previous_was_cr {
            windows.push('\r');
        }
        windows.push(ch);
        previous_was_cr = ch == '\r';
    }
    windows.encode_utf16().chain(std::iter::once(0)).collect()
}

/// Decode a clipboard DIB (without a BITMAPFILEHEADER) to a PNG image.
pub fn dib_to_png(dib: &[u8]) -> Result<Vec<u8>, ClipError> {
    if dib.len() < 4 {
        return Err(ClipError::Bad);
    }
    let header_len = read_u32(dib, 0).ok_or(ClipError::Bad)? as usize;
    if !matches!(header_len, 40 | 108 | 124) {
        return Err(ClipError::Unsupported);
    }
    if dib.len() < header_len {
        return Err(ClipError::Bad);
    }

    let width = read_i32(dib, 4).ok_or(ClipError::Bad)?;
    let height = read_i32(dib, 8).ok_or(ClipError::Bad)?;
    let planes = read_u16(dib, 12).ok_or(ClipError::Bad)?;
    let bit_count = read_u16(dib, 14).ok_or(ClipError::Bad)?;
    let compression = read_u32(dib, 16).ok_or(ClipError::Bad)?;
    if planes != 1 || width <= 0 || height == 0 {
        return Err(ClipError::Bad);
    }
    if !matches!(bit_count, 24 | 32) {
        return Err(ClipError::Unsupported);
    }
    if !matches!(compression, 0 | 3) {
        return Err(ClipError::Unsupported);
    }
    let height_abs = height.unsigned_abs();
    let width = width as u32;
    if width > MAX_IMAGE_SIDE || height_abs > MAX_IMAGE_SIDE {
        return Err(ClipError::TooLarge);
    }

    let alpha_mask_is_explicit = header_len == 124 && read_u32(dib, 52) == Some(0xff00_0000);
    let pixel_offset = if compression == 3 {
        let r = read_u32(dib, 40).ok_or(ClipError::Bad)?;
        let g = read_u32(dib, 44).ok_or(ClipError::Bad)?;
        let b = read_u32(dib, 48).ok_or(ClipError::Bad)?;
        if r != 0x00ff_0000 || g != 0x0000_ff00 || b != 0x0000_00ff {
            return Err(ClipError::Unsupported);
        }
        if header_len == 40 {
            52usize
        } else {
            header_len
        }
    } else {
        header_len
    };

    let bytes_per_pixel = usize::from(bit_count / 8);
    let row_stride = (usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(usize::from(bit_count)))
        .and_then(|bits| bits.checked_add(31))
        .ok_or(ClipError::Bad)?
        / 32)
        .checked_mul(4)
        .ok_or(ClipError::Bad)?;
    let pixel_bytes = row_stride
        .checked_mul(usize::try_from(height_abs).map_err(|_| ClipError::Bad)?)
        .ok_or(ClipError::Bad)?;
    let end = pixel_offset
        .checked_add(pixel_bytes)
        .ok_or(ClipError::Bad)?;
    if end > dib.len() {
        return Err(ClipError::Bad);
    }

    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(usize::try_from(height_abs).ok()?))
        .ok_or(ClipError::Bad)?;
    let rgba_len = pixel_count.checked_mul(4).ok_or(ClipError::TooLarge)?;
    let mut rgba = vec![0u8; rgba_len];
    let mut any_nonzero_alpha = false;
    if bit_count == 32 && !alpha_mask_is_explicit {
        for y in 0..height_abs as usize {
            let row = pixel_offset + y * row_stride;
            for x in 0..width as usize {
                if dib[row + x * 4 + 3] != 0 {
                    any_nonzero_alpha = true;
                    break;
                }
            }
            if any_nonzero_alpha {
                break;
            }
        }
    }

    for out_y in 0..height_abs as usize {
        let source_y = if height < 0 {
            out_y
        } else {
            height_abs as usize - 1 - out_y
        };
        let source_row = pixel_offset + source_y * row_stride;
        let dest_row = out_y * width as usize * 4;
        for x in 0..width as usize {
            let source = source_row + x * bytes_per_pixel;
            let dest = dest_row + x * 4;
            rgba[dest] = dib[source + 2];
            rgba[dest + 1] = dib[source + 1];
            rgba[dest + 2] = dib[source];
            rgba[dest + 3] = if bit_count == 24 {
                255
            } else if alpha_mask_is_explicit || any_nonzero_alpha {
                dib[source + 3]
            } else {
                255
            };
        }
    }

    encode_png(width, height_abs, &rgba)
}

/// Decode a PNG and produce a standard 32-bit bottom-up BITMAPV5HEADER DIB.
pub fn png_to_dibv5(png_data: &[u8]) -> Result<Vec<u8>, ClipError> {
    if png_data.len() > MAX_PNG_BYTES {
        return Err(ClipError::TooLarge);
    }
    let mut decoder = png::Decoder::new(Cursor::new(png_data));
    decoder.set_transformations(png::Transformations::normalize_to_color8());
    let mut reader = decoder
        .read_info()
        .map_err(|err| ClipError::Png(err.to_string()))?;
    let (width, height) = reader.info().size();
    if width == 0 || height == 0 {
        return Err(ClipError::Bad);
    }
    if width > MAX_IMAGE_SIDE || height > MAX_IMAGE_SIDE {
        return Err(ClipError::TooLarge);
    }
    let output_size = reader
        .output_buffer_size()
        .ok_or_else(|| ClipError::Png("image buffer too large".into()))?;
    let mut decoded = vec![0u8; output_size];
    let output = reader
        .next_frame(&mut decoded)
        .map_err(|err| ClipError::Png(err.to_string()))?;
    let decoded = &decoded[..output.buffer_size()];
    let pixel_count = usize::try_from(width)
        .ok()
        .and_then(|w| w.checked_mul(usize::try_from(height).ok()?))
        .ok_or(ClipError::TooLarge)?;
    let samples = output.color_type.samples();
    let expected_line = width as usize * samples;
    if output.bit_depth != png::BitDepth::Eight
        || output.width != width
        || output.height != height
        || output.line_size != expected_line
        || decoded.len() != expected_line * height as usize
        || output.color_type == png::ColorType::Indexed
    {
        return Err(ClipError::Png("unexpected decoder output".into()));
    }

    let image_size = pixel_count.checked_mul(4).ok_or(ClipError::TooLarge)?;
    let total_size = 124usize
        .checked_add(image_size)
        .ok_or(ClipError::TooLarge)?;
    let image_size_u32 = u32::try_from(image_size).map_err(|_| ClipError::TooLarge)?;
    let mut dib = vec![0u8; total_size];
    put_u32(&mut dib, 0, 124);
    put_i32(
        &mut dib,
        4,
        i32::try_from(width).map_err(|_| ClipError::TooLarge)?,
    );
    put_i32(
        &mut dib,
        8,
        i32::try_from(height).map_err(|_| ClipError::TooLarge)?,
    );
    put_u16(&mut dib, 12, 1);
    put_u16(&mut dib, 14, 32);
    put_u32(&mut dib, 16, 3);
    put_u32(&mut dib, 20, image_size_u32);
    put_u32(&mut dib, 40, 0x00ff_0000);
    put_u32(&mut dib, 44, 0x0000_ff00);
    put_u32(&mut dib, 48, 0x0000_00ff);
    put_u32(&mut dib, 52, 0xff00_0000);
    put_u32(&mut dib, 56, 0x7352_4742);
    put_u32(&mut dib, 108, 4);

    let pixels = &mut dib[124..];
    for out_y in 0..height as usize {
        let source_y = height as usize - 1 - out_y;
        for x in 0..width as usize {
            let source = (source_y * width as usize + x) * samples;
            let dest = (out_y * width as usize + x) * 4;
            let sample = &decoded[source..source + samples];
            let rgba = match output.color_type {
                png::ColorType::Grayscale => [sample[0], sample[0], sample[0], 255],
                png::ColorType::GrayscaleAlpha => [sample[0], sample[0], sample[0], sample[1]],
                png::ColorType::Rgb => [sample[0], sample[1], sample[2], 255],
                png::ColorType::Rgba => [sample[0], sample[1], sample[2], sample[3]],
                png::ColorType::Indexed => return Err(ClipError::Unsupported),
            };
            pixels[dest..dest + 4].copy_from_slice(&[rgba[2], rgba[1], rgba[0], rgba[3]]);
        }
    }
    Ok(dib)
}

pub fn pick_format(available: &[ClipFormat]) -> Option<ClipFormat> {
    [
        ClipFormat::UnicodeText,
        ClipFormat::Png,
        ClipFormat::DibV5,
        ClipFormat::Dib,
    ]
    .into_iter()
    .find(|candidate| available.contains(candidate))
}

pub fn make_item(kind: pb::clipboard_header::Kind, data: Vec<u8>) -> ClipboardItem {
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(Sha256::digest(&data).as_slice());
    ClipboardItem { kind, data, sha256 }
}

/// Interpret a copied clipboard allocation after closing the native clipboard.
pub fn read_item(format: ClipFormat, data: Vec<u8>) -> Option<ClipboardItem> {
    use pb::clipboard_header::Kind;

    match format {
        ClipFormat::UnicodeText => {
            if !data.len().is_multiple_of(2) {
                return None;
            }
            let units: Vec<_> = data
                .chunks_exact(2)
                .map(|bytes| u16::from_le_bytes([bytes[0], bytes[1]]))
                .collect();
            Some(make_item(Kind::Text, text_to_wire(&units)?.into_bytes()))
        }
        ClipFormat::Png if data.len() <= MAX_PNG_BYTES => Some(make_item(Kind::Png, data)),
        ClipFormat::Png => None,
        ClipFormat::DibV5 | ClipFormat::Dib => Some(make_item(Kind::Png, dib_to_png(&data).ok()?)),
    }
}

/// Validate and convert all representations before the native clipboard is emptied.
pub fn write_formats(item: &ClipboardItem) -> Result<Vec<(ClipFormat, Vec<u8>)>, PlatformError> {
    use pb::clipboard_header::Kind;

    match item.kind {
        Kind::Text => {
            if item.data.len() > MAX_TEXT_BYTES {
                return Err(PlatformError::Platform("clipboard text too large".into()));
            }
            let text = std::str::from_utf8(&item.data)
                .map_err(|_| PlatformError::Platform("clipboard text is not valid UTF-8".into()))?;
            let bytes = text_from_wire(text)
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect();
            Ok(vec![(ClipFormat::UnicodeText, bytes)])
        }
        Kind::Png => {
            let dib = png_to_dibv5(&item.data)
                .map_err(|err| PlatformError::Platform(format!("clipboard PNG: {err}")))?;
            Ok(vec![
                (ClipFormat::Png, item.data.clone()),
                (ClipFormat::DibV5, dib),
            ])
        }
        Kind::Unspecified => Err(PlatformError::Unsupported),
    }
}

#[derive(Debug, Default)]
pub struct OwnWrites {
    sequence: Option<u32>,
}

impl OwnWrites {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn note_written(&mut self, seq: u32) {
        self.sequence = Some(seq);
    }

    pub fn is_own(&self, seq: u32) -> bool {
        self.sequence == Some(seq)
    }
}

fn encode_png(width: u32, height: u32, rgba: &[u8]) -> Result<Vec<u8>, ClipError> {
    let mut encoded = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut encoded, width, height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|err| ClipError::Png(err.to_string()))?;
        writer
            .write_image_data(rgba)
            .map_err(|err| ClipError::Png(err.to_string()))?;
    }
    if encoded.len() > MAX_PNG_BYTES {
        Err(ClipError::TooLarge)
    } else {
        Ok(encoded)
    }
}

fn read_u16(data: &[u8], offset: usize) -> Option<u16> {
    Some(u16::from_le_bytes(
        data.get(offset..offset.checked_add(2)?)?.try_into().ok()?,
    ))
}

fn read_u32(data: &[u8], offset: usize) -> Option<u32> {
    Some(u32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn read_i32(data: &[u8], offset: usize) -> Option<i32> {
    Some(i32::from_le_bytes(
        data.get(offset..offset.checked_add(4)?)?.try_into().ok()?,
    ))
}

fn put_u16(data: &mut [u8], offset: usize, value: u16) {
    data[offset..offset + 2].copy_from_slice(&value.to_le_bytes());
}

fn put_u32(data: &mut [u8], offset: usize, value: u32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn put_i32(data: &mut [u8], offset: usize, value: i32) {
    data[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}
