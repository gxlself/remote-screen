//! Pixel format conversion to BGRA8.

use crate::types::PixelFormat;

fn mask_bits(max: u16) -> u8 {
    (u32::from(max).ilog2() as u8).saturating_add(1)
}

fn component_fits(max: u16, shift: u8, bits: u8) -> bool {
    max != 0
        && (u32::from(max) + 1).is_power_of_two()
        && u16::from(mask_bits(max)).saturating_add(u16::from(shift)) <= u16::from(bits)
}

fn components_fit_low(pf: &PixelFormat) -> bool {
    [
        (pf.red_max, pf.red_shift),
        (pf.green_max, pf.green_shift),
        (pf.blue_max, pf.blue_shift),
    ]
    .into_iter()
    .all(|(max, shift)| component_fits(max, shift, 24))
}

fn components_fit_high(pf: &PixelFormat) -> bool {
    [
        (pf.red_max, pf.red_shift),
        (pf.green_max, pf.green_shift),
        (pf.blue_max, pf.blue_shift),
    ]
    .into_iter()
    .all(|(max, shift)| shift >= 8 && component_fits(max, shift, 32))
}

/// Return whether `pf` is a supported true-colour RFB pixel format.
pub fn is_supported(pf: &PixelFormat) -> bool {
    matches!(pf.bits_per_pixel, 8 | 16 | 32)
        && (1..=pf.bits_per_pixel).contains(&pf.depth)
        && pf.true_colour
        && component_fits(pf.red_max, pf.red_shift, pf.bits_per_pixel)
        && component_fits(pf.green_max, pf.green_shift, pf.bits_per_pixel)
        && component_fits(pf.blue_max, pf.blue_shift, pf.bits_per_pixel)
}

/// Read one pixel from its wire representation.
pub fn read_pixel(pf: &PixelFormat, bytes: &[u8]) -> u32 {
    let count = pf.bytes_per_pixel().min(4);
    if pf.big_endian {
        bytes
            .iter()
            .take(count)
            .fold(0u32, |value, byte| (value << 8) | u32::from(*byte))
    } else {
        bytes
            .iter()
            .take(count)
            .enumerate()
            .fold(0u32, |value, (index, byte)| {
                value | (u32::from(*byte) << (index * 8))
            })
    }
}

fn component(px: u32, max: u16, shift: u8) -> u8 {
    if max == 0 || shift >= 32 {
        return 0;
    }
    let value = (px >> shift) & u32::from(max);
    ((value * 255 + u32::from(max) / 2) / u32::from(max)) as u8
}

/// Convert one packed pixel to BGRA8.
pub fn to_bgra(pf: &PixelFormat, px: u32) -> [u8; 4] {
    [
        component(px, pf.blue_max, pf.blue_shift),
        component(px, pf.green_max, pf.green_shift),
        component(px, pf.red_max, pf.red_shift),
        255,
    ]
}

/// Convert a row of wire pixels to BGRA8.
pub fn convert_row(pf: &PixelFormat, src: &[u8], dst: &mut [u8]) {
    let bpp = pf.bytes_per_pixel();
    if bpp == 0 {
        return;
    }
    let pixels = (src.len() / bpp).min(dst.len() / 4);
    if *pf == PixelFormat::BGRX32 {
        for index in 0..pixels {
            let source = index * 4;
            let target = index * 4;
            dst[target..target + 3].copy_from_slice(&src[source..source + 3]);
            dst[target + 3] = 255;
        }
        return;
    }

    for index in 0..pixels {
        let px = read_pixel(pf, &src[index * bpp..(index + 1) * bpp]);
        dst[index * 4..index * 4 + 4].copy_from_slice(&to_bgra(pf, px));
    }
}

/// Return the wire length of a ZRLE CPIXEL.
pub fn cpixel_len(pf: &PixelFormat) -> usize {
    if pf.true_colour
        && pf.bits_per_pixel == 32
        && pf.depth <= 24
        && (components_fit_low(pf) || components_fit_high(pf))
    {
        3
    } else {
        pf.bytes_per_pixel()
    }
}

/// Convert one ZRLE CPIXEL to BGRA8.
pub fn cpixel_to_bgra(pf: &PixelFormat, bytes: &[u8]) -> [u8; 4] {
    if cpixel_len(pf) != 3 {
        return to_bgra(pf, read_pixel(pf, bytes));
    }

    let low = components_fit_low(pf);
    let mut full = [0u8; 4];
    let destination = if (pf.big_endian && !low) || (!pf.big_endian && low) {
        &mut full[..3]
    } else {
        &mut full[1..]
    };
    for (target, source) in destination.iter_mut().zip(bytes.iter().take(3)) {
        *target = *source;
    }
    to_bgra(pf, read_pixel(pf, &full))
}
