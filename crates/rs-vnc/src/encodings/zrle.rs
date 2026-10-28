//! ZRLE decoder.

use flate2::{Decompress, FlushDecompress};

use crate::pixel::{cpixel_len, cpixel_to_bgra, is_supported};
use crate::surface::Surface;
use crate::types::{PixelFormat, Rect, VncError};

/// The width and height of a ZRLE tile.
pub const TILE: u16 = 64;

const DECOMPRESS_BUFFER: usize = 8192;

/// Decoder state for one RFB connection.
///
/// The zlib stream is deliberately kept across calls: RFC 6143 specifies one
/// stream for the whole connection, rather than one stream per rectangle.
pub struct ZrleDecoder {
    decompress: Decompress,
    output: Vec<u8>,
    tile_bgra: Vec<u8>,
}

impl Default for ZrleDecoder {
    fn default() -> Self {
        Self::new()
    }
}

impl ZrleDecoder {
    /// Create a decoder with a fresh zlib stream.
    pub fn new() -> Self {
        Self {
            decompress: Decompress::new(true),
            output: Vec::new(),
            tile_bgra: Vec::with_capacity(usize::from(TILE) * usize::from(TILE) * 4),
        }
    }

    /// Decode one compressed ZRLE rectangle into `s`.
    pub fn decode(
        &mut self,
        pf: &PixelFormat,
        r: Rect,
        data: &[u8],
        s: &mut dyn Surface,
    ) -> Result<(), VncError> {
        check_fits(r, s)?;
        if r.is_empty() && data.is_empty() {
            return Ok(());
        }
        if !is_supported(pf) {
            return Err(protocol("unsupported pixel format"));
        }

        let cpix = cpixel_len(pf);
        let limit = output_limit(r, cpix)?;
        self.inflate(data, limit)?;

        let mut cursor = Cursor::new(&self.output);
        let width = usize::from(r.w);
        let height = usize::from(r.h);
        let tile_width = usize::from(TILE);
        let tile_height = usize::from(TILE);
        let tile_bgra = &mut self.tile_bgra;

        let mut tile_y = 0usize;
        while tile_y < height {
            let th = (height - tile_y).min(tile_height);
            let mut tile_x = 0usize;
            while tile_x < width {
                let tw = (width - tile_x).min(tile_width);
                let tile_x_u16 =
                    u16::try_from(tile_x).map_err(|_| protocol("zrle tile coordinate overflow"))?;
                let tile_y_u16 =
                    u16::try_from(tile_y).map_err(|_| protocol("zrle tile coordinate overflow"))?;
                let x =
                    r.x.checked_add(tile_x_u16)
                        .ok_or_else(|| protocol("zrle tile coordinate overflow"))?;
                let y =
                    r.y.checked_add(tile_y_u16)
                        .ok_or_else(|| protocol("zrle tile coordinate overflow"))?;
                let tile_rect = Rect::new(
                    x,
                    y,
                    u16::try_from(tw).map_err(|_| protocol("zrle tile size overflow"))?,
                    u16::try_from(th).map_err(|_| protocol("zrle tile size overflow"))?,
                );
                let tile_pixels = tw
                    .checked_mul(th)
                    .and_then(|pixels| pixels.checked_mul(4))
                    .ok_or_else(|| protocol("zrle tile size overflow"))?;
                tile_bgra.resize(tile_pixels, 0);

                match decode_tile(pf, cpix, tw, th, &mut cursor, tile_bgra)? {
                    Some(color) => s.fill(tile_rect, color),
                    None => s.put_bgra(tile_rect, tile_bgra, tw * 4),
                }

                tile_x += tw;
            }
            tile_y += th;
        }

        if cursor.remaining() != 0 {
            tracing::warn!(
                bytes = cursor.remaining(),
                "ZRLE payload has trailing decoded bytes"
            );
        }
        Ok(())
    }

    fn inflate(&mut self, data: &[u8], limit: usize) -> Result<(), VncError> {
        self.output.clear();
        // A previous, larger rectangle may have left a large allocation behind.
        // Drop it before a small decode so this call never grows the output
        // buffer beyond the current rectangle's bound.
        if self.output.capacity() > limit {
            self.output = Vec::new();
        }

        let mut consumed = 0usize;
        let mut scratch = [0u8; DECOMPRESS_BUFFER];
        loop {
            let input = &data[consumed..];
            let before_in = self.decompress.total_in();
            let before_out = self.decompress.total_out();
            self.decompress
                .decompress(input, &mut scratch, FlushDecompress::Sync)
                .map_err(|error| protocol(format!("zlib: {error}")))?;
            let consumed_now = self.decompress.total_in().saturating_sub(before_in);
            let produced = self.decompress.total_out().saturating_sub(before_out);
            let consumed_now = usize::try_from(consumed_now)
                .map_err(|_| protocol("zlib input counter overflow"))?;
            let produced =
                usize::try_from(produced).map_err(|_| protocol("zlib output counter overflow"))?;
            if consumed_now > input.len() || produced > scratch.len() {
                return Err(protocol("zlib counter out of range"));
            }
            if produced != 0 {
                let new_len = self
                    .output
                    .len()
                    .checked_add(produced)
                    .ok_or_else(|| protocol("zrle output too large"))?;
                if new_len > limit {
                    return Err(protocol("zrle output too large"));
                }
                if self.output.capacity() < new_len {
                    self.output
                        .try_reserve_exact(new_len - self.output.capacity())
                        .map_err(|_| protocol("zrle output too large"))?;
                }
                self.output.extend_from_slice(&scratch[..produced]);
            }
            consumed = consumed
                .checked_add(consumed_now)
                .ok_or_else(|| protocol("zlib input counter overflow"))?;

            if consumed == data.len() {
                // A full scratch buffer can leave output pending inside zlib;
                // make another call with no input to drain it.  With no output
                // produced, Sync means the current rectangle is complete.
                if produced == scratch.len() {
                    continue;
                }
                break;
            }

            if consumed_now == 0 && produced == 0 {
                return Err(protocol("zlib: no progress"));
            }

            // A short output with input remaining is normal: the next call
            // consumes the rest of the compressed segment.
        }
        Ok(())
    }
}

fn protocol(message: impl Into<String>) -> VncError {
    VncError::Protocol(message.into())
}

fn check_fits(r: Rect, s: &dyn Surface) -> Result<(), VncError> {
    let (width, height) = s.size();
    if r.fits(width, height) {
        Ok(())
    } else {
        Err(protocol("rect out of bounds"))
    }
}

fn output_limit(r: Rect, cpix: usize) -> Result<usize, VncError> {
    let width = usize::from(r.w);
    let height = usize::from(r.h);
    let tile_columns = width.div_ceil(usize::from(TILE));
    let tile_rows = height.div_ceil(usize::from(TILE));
    let tiles = tile_columns
        .checked_mul(tile_rows)
        .ok_or_else(|| protocol("zrle output too large"))?;
    let pixel_bytes = width
        .checked_mul(height)
        .and_then(|pixels| pixels.checked_mul(cpix.checked_add(1)?))
        .ok_or_else(|| protocol("zrle output too large"))?;
    let tile_bytes = tiles
        .checked_mul(
            1usize
                .checked_add(
                    cpix.checked_mul(16)
                        .ok_or_else(|| protocol("zrle output too large"))?,
                )
                .ok_or_else(|| protocol("zrle output too large"))?,
        )
        .ok_or_else(|| protocol("zrle output too large"))?;
    pixel_bytes
        .checked_add(tile_bytes)
        .and_then(|value| value.checked_add(64))
        .ok_or_else(|| protocol("zrle output too large"))
}

struct Cursor<'a> {
    bytes: &'a [u8],
    pos: usize,
}

impl<'a> Cursor<'a> {
    fn new(bytes: &'a [u8]) -> Self {
        Self { bytes, pos: 0 }
    }

    fn read_u8(&mut self) -> Result<u8, VncError> {
        let byte = self
            .bytes
            .get(self.pos)
            .copied()
            .ok_or_else(|| protocol("zrle truncated"))?;
        self.pos += 1;
        Ok(byte)
    }

    fn take(&mut self, len: usize) -> Result<&'a [u8], VncError> {
        let end = self
            .pos
            .checked_add(len)
            .ok_or_else(|| protocol("zrle truncated"))?;
        if end > self.bytes.len() {
            return Err(protocol("zrle truncated"));
        }
        let result = &self.bytes[self.pos..end];
        self.pos = end;
        Ok(result)
    }

    fn remaining(&self) -> usize {
        self.bytes.len() - self.pos
    }
}

fn decode_tile(
    pf: &PixelFormat,
    cpix: usize,
    tw: usize,
    th: usize,
    cursor: &mut Cursor<'_>,
    tile_bgra: &mut [u8],
) -> Result<Option<[u8; 4]>, VncError> {
    let subencoding = cursor.read_u8()?;
    let pixels = tw
        .checked_mul(th)
        .ok_or_else(|| protocol("zrle tile size overflow"))?;
    match subencoding {
        0 => {
            for pixel in 0..pixels {
                let color = read_color(pf, cpix, cursor)?;
                write_color(tile_bgra, pixel, color);
            }
            Ok(None)
        }
        1 => Ok(Some(read_color(pf, cpix, cursor)?)),
        2..=16 => {
            let palette_len = usize::from(subencoding);
            let mut palette = [[0u8; 4]; 16];
            for color in palette.iter_mut().take(palette_len) {
                *color = read_color(pf, cpix, cursor)?;
            }
            let bits = if palette_len == 2 {
                1usize
            } else if palette_len <= 4 {
                2
            } else {
                4
            };
            let row_bytes = tw
                .checked_mul(bits)
                .and_then(|value| value.checked_add(7))
                .map(|value| value / 8)
                .ok_or_else(|| protocol("zrle tile size overflow"))?;
            let mask = (1u8 << bits) - 1;
            for row in 0..th {
                let packed = cursor.take(row_bytes)?;
                for column in 0..tw {
                    let bit = column * bits;
                    let byte = packed[bit / 8];
                    let shift = 8 - bits - bit % 8;
                    let index = usize::from((byte >> shift) & mask);
                    if index >= palette_len {
                        return Err(protocol("zrle palette index out of range"));
                    }
                    write_color(tile_bgra, row * tw + column, palette[index]);
                }
            }
            Ok(None)
        }
        128 => {
            decode_rle(pf, cpix, pixels, cursor, tile_bgra, None)?;
            Ok(None)
        }
        130..=255 => {
            let palette_len = usize::from(subencoding - 128);
            let mut palette = [[0u8; 4]; 127];
            for color in palette.iter_mut().take(palette_len) {
                *color = read_color(pf, cpix, cursor)?;
            }
            decode_rle(
                pf,
                cpix,
                pixels,
                cursor,
                tile_bgra,
                Some((&palette, palette_len)),
            )?;
            Ok(None)
        }
        value => Err(protocol(format!("zrle subencoding {value}"))),
    }
}

fn decode_rle(
    pf: &PixelFormat,
    cpix: usize,
    pixels: usize,
    cursor: &mut Cursor<'_>,
    tile_bgra: &mut [u8],
    palette: Option<(&[[u8; 4]; 127], usize)>,
) -> Result<(), VncError> {
    let mut filled = 0usize;
    while filled < pixels {
        let (color, run) = if let Some((colors, palette_len)) = palette {
            let first = cursor.read_u8()?;
            let index = usize::from(first & 0x7f);
            if index >= palette_len {
                return Err(protocol("zrle palette index out of range"));
            }
            let run = if first & 0x80 == 0 {
                1
            } else {
                read_run_length(cursor)?
            };
            (colors[index], run)
        } else {
            let pixel = read_color(pf, cpix, cursor)?;
            (pixel, read_run_length(cursor)?)
        };
        let remaining = pixels - filled;
        if run > remaining {
            return Err(protocol("zrle run exceeds tile"));
        }
        for index in filled..filled + run {
            write_color(tile_bgra, index, color);
        }
        filled += run;
    }
    Ok(())
}

fn read_run_length(cursor: &mut Cursor<'_>) -> Result<usize, VncError> {
    let mut length = 1usize;
    loop {
        let byte = cursor.read_u8()?;
        length = length
            .checked_add(usize::from(byte))
            .ok_or_else(|| protocol("zrle run length overflow"))?;
        if byte != 255 {
            return Ok(length);
        }
    }
}

fn read_color(pf: &PixelFormat, cpix: usize, cursor: &mut Cursor<'_>) -> Result<[u8; 4], VncError> {
    let bytes = cursor.take(cpix)?;
    Ok(cpixel_to_bgra(pf, bytes))
}

fn write_color(tile_bgra: &mut [u8], pixel: usize, color: [u8; 4]) {
    let offset = pixel * 4;
    tile_bgra[offset..offset + 4].copy_from_slice(&color);
}
