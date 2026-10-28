//! Conversion of DXGI pointer shapes into RGBA pixels and RSP cursor shapes.

use rsp_core::cursor::shape_id;
use rsp_core::pb;

/// The shape format reported by `DXGI_OUTDUPL_POINTER_SHAPE_TYPE_*`.
#[repr(u32)]
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PointerKind {
    Monochrome = 1,
    Color = 2,
    MaskedColor = 4,
}

/// The raw pointer shape returned by `IDXGIOutputDuplication::GetFramePointerShape`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PointerShape {
    pub kind: PointerKind,
    pub width: u32,
    pub height: u32,
    pub pitch: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub data: Vec<u8>,
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum ShapeError {
    #[error("empty shape")]
    Empty,
    #[error("shape too large")]
    TooLarge,
    #[error("shape buffer too short")]
    Short,
    #[error("png encode: {0}")]
    Png(String),
}

/// Cursor shapes larger than this are rejected before allocating an image.
pub const MAX_CURSOR_SIDE: u32 = 256;

/// Tightly packed RGBA8 pixels.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Rgba {
    pub width: u32,
    pub height: u32,
    pub pixels: Vec<u8>,
}

/// Decode a DXGI pointer shape into tightly packed RGBA8 pixels.
pub fn to_rgba(shape: &PointerShape) -> Result<Rgba, ShapeError> {
    let (width, height) = output_size(shape)?;
    let stride = usize::try_from(shape.pitch).map_err(|_| ShapeError::Short)?;
    let pixels_per_row = match shape.kind {
        PointerKind::Monochrome => {
            usize::try_from(width.div_ceil(8)).map_err(|_| ShapeError::Short)?
        }
        PointerKind::Color | PointerKind::MaskedColor => {
            usize::try_from(width).map_err(|_| ShapeError::Short)? * 4
        }
    };
    let stored_height = match shape.kind {
        PointerKind::Monochrome => usize::try_from(shape.height).map_err(|_| ShapeError::Short)?,
        PointerKind::Color | PointerKind::MaskedColor => {
            usize::try_from(height).map_err(|_| ShapeError::Short)?
        }
    };
    let required = stride.checked_mul(stored_height).ok_or(ShapeError::Short)?;
    if stride < pixels_per_row || shape.data.len() < required {
        return Err(ShapeError::Short);
    }

    let pixel_count = usize::try_from(width)
        .map_err(|_| ShapeError::Short)?
        .checked_mul(usize::try_from(height).map_err(|_| ShapeError::Short)?)
        .ok_or(ShapeError::Short)?;
    let mut pixels = Vec::with_capacity(pixel_count * 4);
    match shape.kind {
        PointerKind::Monochrome => {
            let xor_offset = usize::try_from(height).map_err(|_| ShapeError::Short)? * stride;
            for y in 0..usize::try_from(height).map_err(|_| ShapeError::Short)? {
                let and_row = &shape.data[y * stride..(y + 1) * stride];
                let xor_row = &shape.data[xor_offset + y * stride..xor_offset + (y + 1) * stride];
                for x in 0..usize::try_from(width).map_err(|_| ShapeError::Short)? {
                    let bit = 0x80 >> (x % 8);
                    let byte = x / 8;
                    let and = and_row[byte] & bit != 0;
                    let xor = xor_row[byte] & bit != 0;
                    let pixel = match (and, xor) {
                        (false, false) | (true, true) => [0, 0, 0, 255],
                        (false, true) => [255, 255, 255, 255],
                        (true, false) => [0, 0, 0, 0],
                    };
                    pixels.extend_from_slice(&pixel);
                }
            }
        }
        PointerKind::Color => {
            for y in 0..usize::try_from(height).map_err(|_| ShapeError::Short)? {
                let row = &shape.data[y * stride..(y + 1) * stride];
                for pixel in row[..usize::try_from(width).map_err(|_| ShapeError::Short)? * 4]
                    .chunks_exact(4)
                {
                    pixels.extend_from_slice(&[pixel[2], pixel[1], pixel[0], pixel[3]]);
                }
            }
        }
        PointerKind::MaskedColor => {
            for y in 0..usize::try_from(height).map_err(|_| ShapeError::Short)? {
                let row = &shape.data[y * stride..(y + 1) * stride];
                for pixel in row[..usize::try_from(width).map_err(|_| ShapeError::Short)? * 4]
                    .chunks_exact(4)
                {
                    let [blue, green, red, mask] = [pixel[0], pixel[1], pixel[2], pixel[3]];
                    let rgba = if mask == 0 {
                        [red, green, blue, 255]
                    } else if blue == 0 && green == 0 && red == 0 {
                        [0, 0, 0, 0]
                    } else {
                        [red, green, blue, 255]
                    };
                    pixels.extend_from_slice(&rgba);
                }
            }
        }
    }
    Ok(Rgba {
        width,
        height,
        pixels,
    })
}

/// Encode a pointer shape as an RGBA8 PNG and wrap it in the RSP cursor shape.
pub fn to_cursor_shape(shape: &PointerShape, scale: f32) -> Result<pb::CursorShape, ShapeError> {
    let rgba = to_rgba(shape)?;
    let mut png_bytes = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png_bytes, rgba.width, rgba.height);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder
            .write_header()
            .map_err(|error| ShapeError::Png(error.to_string()))?;
        writer
            .write_image_data(&rgba.pixels)
            .map_err(|error| ShapeError::Png(error.to_string()))?;
        writer
            .finish()
            .map_err(|error| ShapeError::Png(error.to_string()))?;
    }
    Ok(pb::CursorShape {
        id: shape_id(&png_bytes),
        width: rgba.width,
        height: rgba.height,
        hot_x: shape.hot_x.min(rgba.width - 1),
        hot_y: shape.hot_y.min(rgba.height - 1),
        scale,
        png: png_bytes,
    })
}

fn output_size(shape: &PointerShape) -> Result<(u32, u32), ShapeError> {
    let height = match shape.kind {
        PointerKind::Monochrome => shape.height / 2,
        PointerKind::Color | PointerKind::MaskedColor => shape.height,
    };
    if shape.width == 0 || height == 0 {
        return Err(ShapeError::Empty);
    }
    if shape.width > MAX_CURSOR_SIDE || height > MAX_CURSOR_SIDE {
        return Err(ShapeError::TooLarge);
    }
    Ok((shape.width, height))
}
