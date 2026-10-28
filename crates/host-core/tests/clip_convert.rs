use std::io::Cursor;

use host_core::clip_convert::{
    dib_to_png, make_item, pick_format, png_to_dibv5, read_item, text_from_wire, text_to_wire,
    write_formats, ClipError, ClipFormat, OwnWrites, MAX_IMAGE_SIDE, MAX_PNG_BYTES, MAX_TEXT_BYTES,
};
use host_core::platform::{ClipboardItem, PlatformError};
use png::{BitDepth, ColorType, Decoder, Encoder};
use rsp_core::pb::clipboard_header::Kind;
use sha2::{Digest, Sha256};

fn utf16(text: &str) -> Vec<u16> {
    text.encode_utf16().collect()
}

fn encode_png(width: u32, height: u32, color: ColorType, depth: BitDepth, data: &[u8]) -> Vec<u8> {
    let mut bytes = Vec::new();
    let mut encoder = Encoder::new(&mut bytes, width, height);
    encoder.set_color(color);
    encoder.set_depth(depth);
    let mut writer = encoder.write_header().expect("PNG header");
    writer.write_image_data(data).expect("PNG data");
    writer.finish().expect("PNG finish");
    bytes
}

fn decode_rgba(bytes: &[u8]) -> (u32, u32, Vec<u8>) {
    let decoder = Decoder::new(Cursor::new(bytes));
    let mut reader = decoder.read_info().expect("PNG info");
    let mut data = vec![0; reader.output_buffer_size().expect("PNG buffer")];
    let output = reader.next_frame(&mut data).expect("PNG frame");
    assert_eq!(output.color_type, ColorType::Rgba);
    assert_eq!(output.bit_depth, BitDepth::Eight);
    (
        output.width,
        output.height,
        data[..output.buffer_size()].to_vec(),
    )
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

fn dib_header(
    width: i32,
    height: i32,
    bit_count: u16,
    compression: u32,
    header_len: usize,
) -> Vec<u8> {
    let mut dib = vec![0; header_len];
    put_u32(&mut dib, 0, header_len as u32);
    put_i32(&mut dib, 4, width);
    put_i32(&mut dib, 8, height);
    put_u16(&mut dib, 12, 1);
    put_u16(&mut dib, 14, bit_count);
    put_u32(&mut dib, 16, compression);
    dib
}

#[test]
fn text_normalization_and_limits() {
    let mut input = utf16("a\r\nb\rc");
    input.push(0);
    input.extend(utf16("junk"));
    assert_eq!(text_to_wire(&input).as_deref(), Some("a\nb\rc"));
    assert_eq!(text_to_wire(&[0xd800]), Some("\u{fffd}".into()));
    assert_eq!(text_to_wire(&[]), None);
    assert_eq!(text_to_wire(&[0]), None);
    let long = vec![b'a' as u16; MAX_TEXT_BYTES + 1];
    assert_eq!(text_to_wire(&long), None);
    assert_eq!(text_from_wire("a\nb\r\nc"), utf16("a\r\nb\r\nc\0"));
}

#[test]
fn dib_24_bit_bottom_up_and_padding() {
    let mut dib = dib_header(2, 2, 24, 0, 40);
    // Bottom row: red, green; top row: blue, white. Each row has two padding bytes.
    dib.extend([
        0, 0, 255, 0, 255, 0, 0, 0, // bottom
        255, 0, 0, 255, 255, 255, 0, 0, // top
    ]);
    let png = dib_to_png(&dib).expect("DIB");
    let (width, height, rgba) = decode_rgba(&png);
    assert_eq!((width, height), (2, 2));
    assert_eq!(
        rgba,
        [
            0, 0, 255, 255, 255, 255, 255, 255, // top
            255, 0, 0, 255, 0, 255, 0, 255, // bottom
        ]
    );
}

#[test]
fn dib_32_bit_alpha_rules() {
    let mut opaque = dib_header(2, 1, 32, 0, 40);
    opaque.extend([0, 0, 255, 0, 0, 255, 0, 0]);
    let (_, _, rgba) = decode_rgba(&dib_to_png(&opaque).expect("opaque DIB"));
    assert_eq!([rgba[3], rgba[7]], [255, 255]);

    let mut alpha = dib_header(2, 1, 32, 0, 40);
    alpha.extend([0, 0, 255, 7, 0, 255, 0, 0]);
    let (_, _, rgba) = decode_rgba(&dib_to_png(&alpha).expect("alpha DIB"));
    assert_eq!([rgba[3], rgba[7]], [7, 0]);
}

#[test]
fn dib_bitfields_and_masks() {
    let mut dib = dib_header(1, 1, 32, 3, 40);
    dib.extend_from_slice(&0x00ff_0000u32.to_le_bytes());
    dib.extend_from_slice(&0x0000_ff00u32.to_le_bytes());
    dib.extend_from_slice(&0x0000_00ffu32.to_le_bytes());
    dib.extend([0x1e, 0x14, 0x0a, 0xff]);
    let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("bitfields DIB"));
    assert_eq!(rgba, [10, 20, 30, 255]);
    put_u32(&mut dib, 40, 0xf800);
    put_u32(&mut dib, 44, 0x07e0);
    put_u32(&mut dib, 48, 0x001f);
    assert_eq!(dib_to_png(&dib), Err(ClipError::Unsupported));
}

#[test]
fn dib_v5_top_down_alpha() {
    let mut dib = dib_header(1, -2, 32, 3, 124);
    put_u32(&mut dib, 40, 0x00ff_0000);
    put_u32(&mut dib, 44, 0x0000_ff00);
    put_u32(&mut dib, 48, 0x0000_00ff);
    put_u32(&mut dib, 52, 0xff00_0000);
    dib.extend([0, 0, 255, 10, 0, 255, 0, 20]);
    let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("top-down DIB"));
    assert_eq!(rgba, [255, 0, 0, 10, 0, 255, 0, 20]);
}

#[test]
fn dib_rejects_invalid_dimensions_depth_and_lengths() {
    let mut indexed = dib_header(1, 1, 8, 0, 40);
    indexed.resize(44, 0);
    assert_eq!(dib_to_png(&indexed), Err(ClipError::Unsupported));

    let mut short = dib_header(1, 1, 24, 0, 40);
    short.resize(short.len() + 3, 0);
    assert_eq!(dib_to_png(&short), Err(ClipError::Bad));
    assert_eq!(
        dib_to_png(&dib_header(0, 1, 24, 0, 40)),
        Err(ClipError::Bad)
    );
    assert_eq!(
        dib_to_png(&dib_header(MAX_IMAGE_SIDE as i32 + 1, 1, 24, 0, 40)),
        Err(ClipError::TooLarge)
    );
    assert_eq!(
        dib_to_png(&dib_header(1, 0, 24, 0, 40)),
        Err(ClipError::Bad)
    );
    assert_eq!(
        dib_to_png(&dib_header(-1, 1, 24, 0, 40)),
        Err(ClipError::Bad)
    );
    for height in [
        MAX_IMAGE_SIDE as i32 + 1,
        -(MAX_IMAGE_SIDE as i32 + 1),
        i32::MIN,
    ] {
        assert_eq!(
            dib_to_png(&dib_header(1, height, 24, 0, 40)),
            Err(ClipError::TooLarge)
        );
    }
    for depth in [0, 1, 4, 8, 16, 48, 64] {
        assert_eq!(
            dib_to_png(&dib_header(1, 1, depth, 0, 40)),
            Err(ClipError::Unsupported)
        );
    }
    for compression in [1, 2, 4, 5, 6] {
        assert_eq!(
            dib_to_png(&dib_header(1, 1, 32, compression, 40)),
            Err(ClipError::Unsupported)
        );
    }
}

#[test]
fn png_to_dibv5_header_pixels_and_round_trip() {
    let original = vec![
        255, 0, 0, 1, 0, 255, 0, 2, 0, 0, 255, 3, // top
        10, 20, 30, 4, 40, 50, 60, 5, 70, 80, 90, 6, // bottom
    ];
    let png = encode_png(3, 2, ColorType::Rgba, BitDepth::Eight, &original);
    let dib = png_to_dibv5(&png).expect("PNG to DIBV5");
    assert_eq!(u32::from_le_bytes(dib[0..4].try_into().unwrap()), 124);
    assert_eq!(i32::from_le_bytes(dib[4..8].try_into().unwrap()), 3);
    assert_eq!(i32::from_le_bytes(dib[8..12].try_into().unwrap()), 2);
    assert_eq!(u16::from_le_bytes(dib[12..14].try_into().unwrap()), 1);
    assert_eq!(u16::from_le_bytes(dib[14..16].try_into().unwrap()), 32);
    assert_eq!(u32::from_le_bytes(dib[16..20].try_into().unwrap()), 3);
    assert_eq!(u32::from_le_bytes(dib[20..24].try_into().unwrap()), 24);
    assert_eq!(
        u32::from_le_bytes(dib[40..44].try_into().unwrap()),
        0x00ff_0000
    );
    assert_eq!(
        u32::from_le_bytes(dib[44..48].try_into().unwrap()),
        0x0000_ff00
    );
    assert_eq!(
        u32::from_le_bytes(dib[48..52].try_into().unwrap()),
        0x0000_00ff
    );
    assert_eq!(
        u32::from_le_bytes(dib[52..56].try_into().unwrap()),
        0xff00_0000
    );
    assert_eq!(
        u32::from_le_bytes(dib[56..60].try_into().unwrap()),
        0x7352_4742
    );
    assert_eq!(u32::from_le_bytes(dib[108..112].try_into().unwrap()), 4);
    assert!(dib[24..40].iter().all(|&byte| byte == 0));
    assert!(dib[60..108].iter().all(|&byte| byte == 0));
    assert!(dib[112..124].iter().all(|&byte| byte == 0));
    assert_eq!(
        &dib[124..136],
        &[30, 20, 10, 4, 60, 50, 40, 5, 90, 80, 70, 6]
    );
    let round_trip = dib_to_png(&dib).expect("DIB to PNG");
    let (_, _, rgba) = decode_rgba(&round_trip);
    assert_eq!(rgba, original);
}

#[test]
fn png_grayscale_palette_and_16bit_rgb_convert() {
    let grayscale = encode_png(2, 1, ColorType::Grayscale, BitDepth::Eight, &[7, 200]);
    let dib = png_to_dibv5(&grayscale).expect("grayscale");
    let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("grayscale round trip"));
    assert_eq!(rgba, [7, 7, 7, 255, 200, 200, 200, 255]);

    let mut palette_png = Vec::new();
    {
        let mut encoder = Encoder::new(&mut palette_png, 2, 1);
        encoder.set_color(ColorType::Indexed);
        encoder.set_depth(BitDepth::Eight);
        encoder.set_palette(vec![255, 0, 0, 0, 255, 0]);
        let mut writer = encoder.write_header().expect("palette header");
        writer.write_image_data(&[0, 1]).expect("palette data");
    }
    let dib = png_to_dibv5(&palette_png).expect("palette");
    let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("palette round trip"));
    assert_eq!(rgba, [255, 0, 0, 255, 0, 255, 0, 255]);

    let rgb16 = encode_png(
        1,
        1,
        ColorType::Rgb,
        BitDepth::Sixteen,
        &[0x12, 0x34, 0xab, 0xcd, 0xef, 0x01],
    );
    let dib = png_to_dibv5(&rgb16).expect("16-bit RGB");
    let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("16-bit round trip"));
    assert_eq!(rgba, [0x12, 0xab, 0xef, 255]);
}

#[test]
fn format_priority_item_hash_and_own_writes() {
    assert_eq!(
        pick_format(&[ClipFormat::Dib, ClipFormat::UnicodeText]),
        Some(ClipFormat::UnicodeText)
    );
    assert_eq!(
        pick_format(&[ClipFormat::Dib, ClipFormat::DibV5]),
        Some(ClipFormat::DibV5)
    );
    assert_eq!(
        pick_format(&[ClipFormat::Dib, ClipFormat::Png]),
        Some(ClipFormat::Png)
    );
    assert_eq!(pick_format(&[]), None);

    let item: ClipboardItem = make_item(Kind::Text, b"x".to_vec());
    assert_eq!(item.sha256.as_slice(), Sha256::digest(b"x").as_slice());

    let mut own = OwnWrites::new();
    assert!(!own.is_own(0));
    own.note_written(7);
    assert!(own.is_own(7));
    assert!(!own.is_own(8));
    own.note_written(u32::MAX);
    assert!(own.is_own(u32::MAX));
    own.note_written(0);
    assert!(own.is_own(0));
    assert!(!own.is_own(u32::MAX));
}

#[test]
fn text_limit_applies_after_utf16_decoding_and_newline_normalization() {
    let windows = "\r\n".repeat(MAX_TEXT_BYTES);
    let units = utf16(&windows);
    let text = text_to_wire(&units).expect("normalized text at byte limit");
    assert_eq!(text.len(), MAX_TEXT_BYTES);
    assert!(text.bytes().all(|byte| byte == b'\n'));

    let bytes = units.into_iter().flat_map(u16::to_le_bytes).collect();
    let item = read_item(ClipFormat::UnicodeText, bytes).expect("large UTF-16 allocation");
    assert_eq!(item.data, text.as_bytes());
    let too_large = utf16(&"\u{4e2d}".repeat(MAX_TEXT_BYTES / 3 + 1));
    assert!(text_to_wire(&too_large).is_none());
    assert_eq!(text_from_wire(""), [0]);
    assert_eq!(text_from_wire("\r\n\n\r"), utf16("\r\n\r\n\r\0"));
    assert_eq!(
        text_to_wire(&utf16("\u{4e2d}\u{1d11e}")),
        Some("\u{4e2d}\u{1d11e}".into())
    );
}

#[test]
fn dib_all_supported_headers_depths_and_compressions() {
    for header_len in [40, 108, 124] {
        for depth in [24, 32] {
            for compression in [0, 3] {
                let mut dib = dib_header(1, 1, depth, compression, header_len);
                if compression == 3 {
                    dib.resize(header_len.max(52), 0);
                    put_u32(&mut dib, 40, 0x00ff_0000);
                    put_u32(&mut dib, 44, 0x0000_ff00);
                    put_u32(&mut dib, 48, 0x0000_00ff);
                }
                dib.extend([30, 20, 10, 0]);
                let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("supported DIB"));
                assert_eq!(rgba, [10, 20, 30, 255]);
                for length in 0..dib.len() {
                    assert_eq!(dib_to_png(&dib[..length]), Err(ClipError::Bad),
                        "truncated header {header_len}, depth {depth}, compression {compression}, len {length}");
                }
            }
        }
    }
}

#[test]
fn dib_v5_explicit_zero_alpha_remains_transparent() {
    for compression in [0, 3] {
        let mut dib = dib_header(1, 1, 32, compression, 124);
        put_u32(&mut dib, 40, 0x00ff_0000);
        put_u32(&mut dib, 44, 0x0000_ff00);
        put_u32(&mut dib, 48, 0x0000_00ff);
        put_u32(&mut dib, 52, 0xff00_0000);
        dib.extend([30, 20, 10, 0]);
        let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("explicit alpha"));
        assert_eq!(rgba, [10, 20, 30, 0]);
    }
}

#[test]
fn png_low_bit_depth_and_alpha_color_types_convert() {
    type ColorCase<'a> = (u32, ColorType, BitDepth, &'a [u8], &'a [u8]);
    let cases: &[ColorCase<'_>] = &[
        (
            4,
            ColorType::Grayscale,
            BitDepth::One,
            &[0b0101_0000],
            &[
                0, 0, 0, 255, 255, 255, 255, 255, 0, 0, 0, 255, 255, 255, 255, 255,
            ],
        ),
        (
            4,
            ColorType::Grayscale,
            BitDepth::Two,
            &[0b0001_1011],
            &[
                0, 0, 0, 255, 85, 85, 85, 255, 170, 170, 170, 255, 255, 255, 255, 255,
            ],
        ),
        (
            2,
            ColorType::Grayscale,
            BitDepth::Four,
            &[0x2a],
            &[34, 34, 34, 255, 170, 170, 170, 255],
        ),
        (
            1,
            ColorType::Grayscale,
            BitDepth::Sixteen,
            &[0x56, 0x78],
            &[86, 86, 86, 255],
        ),
        (
            1,
            ColorType::GrayscaleAlpha,
            BitDepth::Eight,
            &[42, 0],
            &[42, 42, 42, 0],
        ),
        (
            1,
            ColorType::GrayscaleAlpha,
            BitDepth::Sixteen,
            &[0x20, 0xff, 0x40, 0x80],
            &[32, 32, 32, 64],
        ),
        (
            1,
            ColorType::Rgb,
            BitDepth::Eight,
            &[3, 7, 8],
            &[3, 7, 8, 255],
        ),
        (
            1,
            ColorType::Rgba,
            BitDepth::Sixteen,
            &[0x11, 0xff, 0x22, 0xff, 0x33, 0xff, 0x44, 0xff],
            &[17, 34, 51, 68],
        ),
    ];
    for &(width, color, depth, data, expected) in cases {
        let png = encode_png(width, 1, color, depth, data);
        let dib = png_to_dibv5(&png).expect("color normalization");
        let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("normalized round trip"));
        assert_eq!(rgba, expected, "color {color:?}, depth {depth:?}");
    }
}

#[test]
fn png_palette_transparency_and_packed_indices_convert() {
    let mut bytes = Vec::new();
    let mut encoder = Encoder::new(&mut bytes, 4, 1);
    encoder.set_color(ColorType::Indexed);
    encoder.set_depth(BitDepth::Two);
    encoder.set_palette(vec![255, 0, 0, 0, 255, 0, 0, 0, 255, 9, 8, 7]);
    encoder.set_trns(vec![255, 40, 0]);
    let mut writer = encoder.write_header().unwrap();
    writer.write_image_data(&[0b0001_1011]).unwrap();
    writer.finish().unwrap();
    let dib = png_to_dibv5(&bytes).expect("palette alpha");
    let (_, _, rgba) = decode_rgba(&dib_to_png(&dib).expect("palette alpha round trip"));
    assert_eq!(
        rgba,
        [255, 0, 0, 255, 0, 255, 0, 40, 0, 0, 255, 0, 9, 8, 7, 255]
    );
}

#[test]
fn png_limits_and_invalid_input() {
    assert_eq!(
        png_to_dibv5(&vec![0; MAX_PNG_BYTES + 1]),
        Err(ClipError::TooLarge)
    );
    for (width, height) in [(MAX_IMAGE_SIDE + 1, 1), (1, MAX_IMAGE_SIDE + 1)] {
        let png = encode_png(
            width,
            height,
            ColorType::Rgba,
            BitDepth::Eight,
            &vec![0; width as usize * height as usize * 4],
        );
        assert_eq!(png_to_dibv5(&png), Err(ClipError::TooLarge));
    }
    assert!(matches!(png_to_dibv5(b"not a PNG"), Err(ClipError::Png(_))));
    let png = encode_png(1, 1, ColorType::Rgba, BitDepth::Eight, &[1, 2, 3, 4]);
    assert!(matches!(png_to_dibv5(&png[..40]), Err(ClipError::Png(_))));
}

#[test]
fn dib_encoded_png_limit() {
    let mut dib = dib_header(2048, 1400, 32, 0, 40);
    let mut random = 0x1357_2468u32;
    for _ in 0..2048 * 1400 {
        random ^= random << 13;
        random ^= random >> 17;
        random ^= random << 5;
        dib.extend(random.to_le_bytes());
    }
    assert_eq!(dib_to_png(&dib), Err(ClipError::TooLarge));
}

#[test]
fn format_adapters_validate_items_and_preserve_registered_png_bytes() {
    assert!(read_item(ClipFormat::UnicodeText, vec![0]).is_none());
    let item = make_item(Kind::Text, b"a\nb\r\nc".to_vec());
    let formats = write_formats(&item).expect("text formats");
    assert_eq!(
        formats,
        vec![(
            ClipFormat::UnicodeText,
            utf16("a\r\nb\r\nc\0")
                .into_iter()
                .flat_map(u16::to_le_bytes)
                .collect()
        )]
    );
    let item = read_item(formats[0].0, formats[0].1.clone()).expect("read text");
    assert_eq!(item.kind, Kind::Text);
    assert_eq!(item.data, b"a\nb\nc");
    assert_eq!(
        item.sha256.as_slice(),
        Sha256::digest(&item.data).as_slice()
    );
    let too_large = make_item(Kind::Text, vec![b'a'; MAX_TEXT_BYTES + 1]);
    assert!(write_formats(&too_large).is_err());
    assert!(write_formats(&make_item(Kind::Text, vec![0xff])).is_err());
    assert!(matches!(
        write_formats(&make_item(Kind::Unspecified, vec![])),
        Err(PlatformError::Unsupported)
    ));

    let png = encode_png(1, 1, ColorType::Rgba, BitDepth::Eight, &[1, 2, 3, 4]);
    let item = make_item(Kind::Png, png.clone());
    let formats = write_formats(&item).expect("image formats");
    assert_eq!(formats.len(), 2);
    assert_eq!(formats[0], (ClipFormat::Png, png.clone()));
    assert_eq!(formats[1].0, ClipFormat::DibV5);
    for (format, data) in formats {
        let item = read_item(format, data).expect("read image format");
        assert_eq!(item.kind, Kind::Png);
        assert_eq!(decode_rgba(&item.data).2, [1, 2, 3, 4]);
    }
    assert_eq!(read_item(ClipFormat::Png, png.clone()).unwrap().data, png);
    assert!(read_item(ClipFormat::Png, vec![0; MAX_PNG_BYTES + 1]).is_none());
    assert!(write_formats(&make_item(Kind::Png, b"bad PNG".to_vec())).is_err());
}
