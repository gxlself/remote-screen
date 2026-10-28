//! Single-frame JPEG snapshots of a display.

use std::ffi::c_void;

extern "C" {
    fn rs_thumbnail_jpeg(
        max_width: u32,
        max_bytes: usize,
        out_data: *mut *mut u8,
        out_len: *mut usize,
    ) -> i32;
    fn rs_thumbnail_free(data: *mut c_void);
}

pub fn capture_jpeg(max_width: u32, max_bytes: usize) -> Option<Vec<u8>> {
    if max_width == 0 || max_bytes == 0 {
        return None;
    }
    let mut data = std::ptr::null_mut();
    let mut len = 0usize;
    // SAFETY: the shim writes the two output values and allocates the returned
    // buffer with malloc; ownership is transferred to this function on success.
    let status = unsafe { rs_thumbnail_jpeg(max_width, max_bytes, &mut data, &mut len) };
    if status == 0 || data.is_null() || len == 0 || len > max_bytes {
        if !data.is_null() {
            // SAFETY: failed calls may only return a buffer owned by the shim.
            unsafe { rs_thumbnail_free(data.cast()) };
        }
        return None;
    }
    // SAFETY: the shim owns a malloc-allocated buffer of exactly `len` bytes.
    let image = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    // SAFETY: successful calls transfer the malloc buffer to this wrapper.
    unsafe { rs_thumbnail_free(data.cast()) };
    Some(image)
}

#[cfg(test)]
mod tests {
    use super::capture_jpeg;

    #[test]
    #[ignore = "requires a macOS screen-recording permission and a live display"]
    fn real_thumbnail_is_320px_jpeg_under_budget() {
        let image = capture_jpeg(320, 60 * 1024).expect("thumbnail capture");
        assert!(image.starts_with(&[0xff, 0xd8]));
        assert_eq!(jpeg_dimensions(&image).map(|(width, _)| width), Some(320));
        assert!(image.len() <= 60 * 1024);
    }

    fn jpeg_dimensions(data: &[u8]) -> Option<(u16, u16)> {
        if data.get(..2) != Some(&[0xff, 0xd8]) {
            return None;
        }
        let mut offset = 2;
        while offset + 3 < data.len() {
            if data[offset] != 0xff {
                offset += 1;
                continue;
            }
            while offset < data.len() && data[offset] == 0xff {
                offset += 1;
            }
            let marker = *data.get(offset)?;
            offset += 1;
            if marker == 0xd9 || marker == 0xda {
                return None;
            }
            if marker == 0xd8 || (0xd0..=0xd7).contains(&marker) {
                continue;
            }
            let length = u16::from_be_bytes([*data.get(offset)?, *data.get(offset + 1)?]);
            let length = usize::from(length);
            if length < 2 || offset + length > data.len() {
                return None;
            }
            if matches!(marker, 0xc0..=0xc3 | 0xc5..=0xc7 | 0xc9..=0xcb | 0xcd..=0xcf) {
                if length < 7 {
                    return None;
                }
                let height = u16::from_be_bytes([data[offset + 3], data[offset + 4]]);
                let width = u16::from_be_bytes([data[offset + 5], data[offset + 6]]);
                return Some((width, height));
            }
            offset += length;
        }
        None
    }
}
