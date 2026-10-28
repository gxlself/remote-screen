use rs_vnc::encodings::basic::{
    apply_desktop_size, apply_extended_desktop_size, apply_pointer_pos, cursor_len,
    decode_copy_rect, decode_cursor, decode_raw, extended_desktop_size_len,
};
use rs_vnc::pixel::{convert_row, cpixel_len, cpixel_to_bgra, is_supported, read_pixel, to_bgra};
use rs_vnc::{MemSurface, PixelFormat, Rect, Screen, Surface, VncError};

const RGB565: PixelFormat = PixelFormat {
    bits_per_pixel: 16,
    depth: 16,
    big_endian: false,
    true_colour: true,
    red_max: 31,
    green_max: 63,
    blue_max: 31,
    red_shift: 11,
    green_shift: 5,
    blue_shift: 0,
};

const BGR233: PixelFormat = PixelFormat {
    bits_per_pixel: 8,
    depth: 8,
    big_endian: false,
    true_colour: true,
    red_max: 7,
    green_max: 7,
    blue_max: 3,
    red_shift: 0,
    green_shift: 3,
    blue_shift: 6,
};

const RGBX_BE: PixelFormat = PixelFormat {
    bits_per_pixel: 32,
    depth: 24,
    big_endian: true,
    true_colour: true,
    red_max: 255,
    green_max: 255,
    blue_max: 255,
    red_shift: 16,
    green_shift: 8,
    blue_shift: 0,
};

fn surface() -> MemSurface {
    let mut surface = MemSurface::new();
    surface.resize(8, 8);
    surface
}

fn assert_protocol(result: Result<(), VncError>) {
    assert!(matches!(result, Err(VncError::Protocol(_))));
}

#[test]
fn is_supported_matrix() {
    assert!(is_supported(&PixelFormat::BGRX32));
    assert!(is_supported(&RGB565));
    assert!(is_supported(&BGR233));
    assert!(is_supported(&RGBX_BE));

    let mut invalid = PixelFormat::BGRX32;
    invalid.true_colour = false;
    assert!(!is_supported(&invalid));
    invalid = PixelFormat::BGRX32;
    invalid.bits_per_pixel = 24;
    assert!(!is_supported(&invalid));
    invalid = PixelFormat::BGRX32;
    invalid.red_max = 0;
    assert!(!is_supported(&invalid));
    invalid = PixelFormat::BGRX32;
    invalid.red_max = 5;
    assert!(!is_supported(&invalid));
    invalid = PixelFormat::BGRX32;
    invalid.red_shift = 30;
    assert!(!is_supported(&invalid));
}

#[test]
fn to_bgra_values() {
    assert_eq!(
        to_bgra(&PixelFormat::BGRX32, 0x0011_2233),
        [0x33, 0x22, 0x11, 255]
    );
    assert_eq!(to_bgra(&RGB565, 0x8410), [132, 130, 132, 255]);
    assert_eq!(to_bgra(&RGB565, 0xffff), [255, 255, 255, 255]);
    assert_eq!(to_bgra(&BGR233, 0xff), [255, 255, 255, 255]);
    assert_eq!(to_bgra(&BGR233, 0x07), [0, 0, 255, 255]);
}

#[test]
fn read_pixel_endianness() {
    assert_eq!(read_pixel(&RGBX_BE, &[0x00, 0x11, 0x22, 0x33]), 0x0011_2233);
    assert_eq!(
        read_pixel(&PixelFormat::BGRX32, &[0x00, 0x11, 0x22, 0x33]),
        0x3322_1100
    );
}

#[test]
fn convert_row_fast_path() {
    let mut output = [0u8; 8];
    convert_row(&PixelFormat::BGRX32, &[1, 2, 3, 9, 4, 5, 6, 9], &mut output);
    assert_eq!(output, [1, 2, 3, 255, 4, 5, 6, 255]);

    convert_row(&RGB565, &[0x10, 0x84, 0xff, 0xff], &mut output);
    assert_eq!(output, [132, 130, 132, 255, 255, 255, 255, 255]);
}

#[test]
fn cpixel_len_rules() {
    assert_eq!(cpixel_len(&PixelFormat::BGRX32), 3);
    let mut depth_32 = PixelFormat::BGRX32;
    depth_32.depth = 32;
    assert_eq!(cpixel_len(&depth_32), 4);
    assert_eq!(cpixel_len(&RGB565), 2);
    assert_eq!(cpixel_len(&RGBX_BE), 3);

    let high = PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 24,
        green_shift: 16,
        blue_shift: 8,
    };
    assert_eq!(cpixel_len(&high), 3);
    let crossing = PixelFormat {
        red_shift: 20,
        green_shift: 8,
        blue_shift: 0,
        ..high
    };
    assert_eq!(cpixel_len(&crossing), 4);
}

#[test]
fn cpixel_to_bgra_variants() {
    assert_eq!(
        cpixel_to_bgra(&PixelFormat::BGRX32, &[0x33, 0x22, 0x11]),
        [0x33, 0x22, 0x11, 255]
    );
    let high_le = PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 24,
        green_shift: 16,
        blue_shift: 8,
    };
    assert_eq!(
        cpixel_to_bgra(&high_le, &[0x33, 0x22, 0x11]),
        [0x33, 0x22, 0x11, 255]
    );
    assert_eq!(
        cpixel_to_bgra(&RGBX_BE, &[0x11, 0x22, 0x33]),
        [0x33, 0x22, 0x11, 255]
    );
    assert_eq!(cpixel_to_bgra(&RGB565, &[0x10, 0x84]), [132, 130, 132, 255]);
}

#[test]
fn raw_bgrx() {
    let mut surface = surface();
    let data = [1, 2, 3, 9, 4, 5, 6, 9, 7, 8, 9, 9, 10, 11, 12, 9];
    decode_raw(
        &PixelFormat::BGRX32,
        Rect::new(1, 1, 2, 2),
        &data,
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.pixel(1, 1), [1, 2, 3, 255]);
    assert_eq!(surface.pixel(2, 1), [4, 5, 6, 255]);
    assert_eq!(surface.pixel(1, 2), [7, 8, 9, 255]);
    assert_eq!(surface.pixel(2, 2), [10, 11, 12, 255]);
    assert_eq!(surface.pixel(0, 0), [0, 0, 0, 255]);
}

#[test]
fn raw_rgb565() {
    let mut surface = surface();
    decode_raw(&RGB565, Rect::new(0, 0, 1, 1), &[0x10, 0x84], &mut surface).unwrap();
    assert_eq!(surface.pixel(0, 0), [132, 130, 132, 255]);
}

#[test]
fn raw_errors() {
    let mut surface = surface();
    assert_protocol(decode_raw(
        &PixelFormat::BGRX32,
        Rect::new(0, 0, 1, 1),
        &[0; 3],
        &mut surface,
    ));
    assert_protocol(decode_raw(
        &PixelFormat::BGRX32,
        Rect::new(7, 7, 2, 2),
        &[0; 16],
        &mut surface,
    ));
    let before = surface.pixels.clone();
    decode_raw(
        &PixelFormat::BGRX32,
        Rect::new(2, 2, 0, 2),
        &[],
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.pixels, before);
}

#[test]
fn copy_rect_overlap() {
    let mut surface = surface();
    for (x, color) in [
        [1, 2, 3, 255],
        [4, 5, 6, 255],
        [7, 8, 9, 255],
        [10, 11, 12, 255],
    ]
    .into_iter()
    .enumerate()
    {
        surface.fill(Rect::new(x as u16, 0, 1, 1), color);
    }
    decode_copy_rect(Rect::new(1, 0, 3, 1), &[0, 0, 0, 0], &mut surface).unwrap();
    assert_eq!(surface.pixel(1, 0), [1, 2, 3, 255]);
    assert_eq!(surface.pixel(2, 0), [4, 5, 6, 255]);
    assert_eq!(surface.pixel(3, 0), [7, 8, 9, 255]);
    assert_protocol(decode_copy_rect(
        Rect::new(0, 0, 2, 2),
        &[7, 7, 7, 7],
        &mut surface,
    ));
    assert_protocol(decode_copy_rect(
        Rect::new(0, 0, 2, 2),
        &[0, 0, 0],
        &mut surface,
    ));
}

#[test]
fn cursor_mask() {
    let mut surface = surface();
    let mut data = [10, 20, 30, 0].repeat(4);
    data.extend_from_slice(&[0b1000_0000, 0b0100_0000]);
    decode_cursor(
        &PixelFormat::BGRX32,
        Rect::new(1, 0, 2, 2),
        &data,
        &mut surface,
    )
    .unwrap();
    assert_eq!(
        surface.cursor,
        Some((
            2,
            2,
            1,
            0,
            vec![10, 20, 30, 255, 0, 0, 0, 0, 0, 0, 0, 0, 10, 20, 30, 255]
        ))
    );
}

#[test]
fn cursor_hide_and_hotspot() {
    let mut surface = surface();
    decode_cursor(
        &PixelFormat::BGRX32,
        Rect::new(5, 9, 0, 0),
        &[],
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.cursor, Some((0, 0, 0, 0, vec![])));

    let data = vec![0; cursor_len(&PixelFormat::BGRX32, 2, 2)];
    decode_cursor(
        &PixelFormat::BGRX32,
        Rect::new(5, 9, 2, 2),
        &data,
        &mut surface,
    )
    .unwrap();
    assert_eq!(
        surface.cursor.as_ref().map(|cursor| (cursor.2, cursor.3)),
        Some((1, 1))
    );
    assert_protocol(decode_cursor(
        &PixelFormat::BGRX32,
        Rect::new(0, 0, 2, 2),
        &data[..data.len() - 1],
        &mut surface,
    ));
}

#[test]
fn cursor_len_padding() {
    assert_eq!(cursor_len(&PixelFormat::BGRX32, 9, 2), 76);
}

#[test]
fn desktop_size() {
    let mut surface = surface();
    apply_desktop_size(Rect::new(0, 0, 1024, 768), &mut surface).unwrap();
    assert_eq!((surface.w, surface.h), (1024, 768));
    assert_eq!(surface.pixel(1023, 767), [0, 0, 0, 255]);
    assert_protocol(apply_desktop_size(Rect::new(0, 0, 0, 768), &mut surface));
    assert_protocol(apply_desktop_size(Rect::new(0, 0, 9000, 1), &mut surface));
}

#[test]
fn extended_desktop_size() {
    assert_eq!(extended_desktop_size_len(1), 20);
    let mut surface = surface();
    let mut payload = vec![1, 0, 0, 0];
    payload.extend_from_slice(&[0; 16]);
    apply_extended_desktop_size(Rect::new(0, 0, 800, 600), &payload, &mut surface).unwrap();
    assert_eq!((surface.w, surface.h), (800, 600));
    apply_extended_desktop_size(Rect::new(0, 1, 320, 240), &payload, &mut surface).unwrap();
    assert_eq!((surface.w, surface.h), (800, 600));
    assert_protocol(apply_extended_desktop_size(
        Rect::new(0, 0, 800, 600),
        &payload[..19],
        &mut surface,
    ));
}

fn screen(id: u32, rect: Rect, flags: u32) -> Screen {
    Screen { id, rect, flags }
}

/// ExtendedDesktopSize payload: count, padding, then 16-byte SCREEN entries.
fn screens_payload(screens: &[Screen]) -> Vec<u8> {
    let mut payload = vec![screens.len() as u8, 0, 0, 0];
    for screen in screens {
        payload.extend_from_slice(&screen.id.to_be_bytes());
        for value in [screen.rect.x, screen.rect.y, screen.rect.w, screen.rect.h] {
            payload.extend_from_slice(&value.to_be_bytes());
        }
        payload.extend_from_slice(&screen.flags.to_be_bytes());
    }
    payload
}

fn two_screens() -> [Screen; 2] {
    [
        screen(7, Rect::new(0, 0, 2880, 1800), 0),
        screen(9, Rect::new(2880, 300, 1920, 1200), 0x0102_0304),
    ]
}

#[test]
fn extended_desktop_size_two_screens() {
    let mut surface = surface();
    let screens = two_screens();
    let payload = screens_payload(&screens);
    assert_eq!(payload.len(), extended_desktop_size_len(2));
    apply_extended_desktop_size(Rect::new(0, 0, 4800, 1800), &payload, &mut surface).unwrap();
    assert_eq!((surface.w, surface.h), (4800, 1800));
    assert_eq!(surface.screens, Some(screens.to_vec()));
}

#[test]
fn extended_desktop_size_refused_keeps_size_and_reports_screens() {
    let mut surface = MemSurface::new();
    surface.resize(4800, 1800);
    let screens = two_screens();
    // Reason 1 (this client asked), status 3 (invalid layout).
    apply_extended_desktop_size(
        Rect::new(1, 3, 640, 480),
        &screens_payload(&screens),
        &mut surface,
    )
    .unwrap();
    assert_eq!((surface.w, surface.h), (4800, 1800));
    assert_eq!(surface.screens, Some(screens.to_vec()));
}

#[test]
fn extended_desktop_size_two_screens_bad_length() {
    let mut surface = surface();
    let payload = screens_payload(&two_screens());
    assert_protocol(apply_extended_desktop_size(
        Rect::new(0, 0, 4800, 1800),
        &payload[..payload.len() - 1],
        &mut surface,
    ));
    let mut longer = payload.clone();
    longer.push(0);
    assert_protocol(apply_extended_desktop_size(
        Rect::new(0, 0, 4800, 1800),
        &longer,
        &mut surface,
    ));
    assert_protocol(apply_extended_desktop_size(
        Rect::new(0, 0, 4800, 1800),
        &[],
        &mut surface,
    ));
    assert_eq!((surface.w, surface.h), (8, 8));
    assert_eq!(surface.screens, None);
}

#[test]
fn extended_desktop_size_unusable_layouts_not_reported() {
    let mut surface = surface();
    apply_extended_desktop_size(Rect::new(0, 0, 640, 480), &[0, 0, 0, 0], &mut surface).unwrap();
    assert_eq!((surface.w, surface.h), (640, 480));
    assert_eq!(surface.screens, None);

    let outside = [
        screen(1, Rect::new(0, 0, 320, 480), 0),
        screen(2, Rect::new(320, 0, 321, 480), 0),
    ];
    apply_extended_desktop_size(
        Rect::new(0, 0, 640, 480),
        &screens_payload(&outside),
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.screens, None);

    let empty = [
        screen(1, Rect::new(0, 0, 320, 480), 0),
        screen(2, Rect::new(320, 0, 0, 480), 0),
    ];
    apply_extended_desktop_size(
        Rect::new(0, 0, 640, 480),
        &screens_payload(&empty),
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.screens, None);

    // A refused request is checked against the size the framebuffer kept.
    let fits_requested_only = [screen(1, Rect::new(0, 0, 800, 600), 0)];
    apply_extended_desktop_size(
        Rect::new(1, 1, 800, 600),
        &screens_payload(&fits_requested_only),
        &mut surface,
    )
    .unwrap();
    assert_eq!((surface.w, surface.h), (640, 480));
    assert_eq!(surface.screens, None);
}

#[test]
fn pointer_pos_clamped() {
    let mut surface = surface();
    apply_pointer_pos(Rect::new(3, 4, 0, 0), &mut surface);
    assert_eq!(surface.pointer, Some((3, 4)));
    apply_pointer_pos(Rect::new(100, 100, 0, 0), &mut surface);
    assert_eq!(surface.pointer, Some((7, 7)));
}
