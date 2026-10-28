use flate2::{Compress, Compression, FlushCompress, Status};
use rs_vnc::encodings::zrle::ZrleDecoder;
use rs_vnc::{MemSurface, PixelFormat, Rect, Surface, VncError};

const A: [u8; 4] = [1, 2, 3, 255];
const B: [u8; 4] = [4, 5, 6, 255];
const C: [u8; 4] = [7, 8, 9, 255];
const D: [u8; 4] = [10, 11, 12, 255];
const E: [u8; 4] = [13, 14, 15, 255];

fn zlib_sync(compressor: &mut Compress, raw: &[u8]) -> Vec<u8> {
    let mut compressed = Vec::with_capacity(raw.len() + 64);
    let mut consumed = 0usize;
    loop {
        let before = compressor.total_in();
        let status = compressor
            .compress_vec(&raw[consumed..], &mut compressed, FlushCompress::Sync)
            .expect("compression should succeed");
        let added = usize::try_from(compressor.total_in() - before).expect("input count");
        consumed += added;
        if consumed == raw.len() {
            break;
        }
        if status != Status::BufError && added == 0 {
            compressed.reserve(raw.len().saturating_sub(consumed).max(128));
        } else if status == Status::BufError {
            compressed.reserve(128);
        }
    }
    compressed
}

fn decode(
    decoder: &mut ZrleDecoder,
    compressor: &mut Compress,
    pf: &PixelFormat,
    rect: Rect,
    raw: &[u8],
    surface: &mut MemSurface,
) -> Result<(), VncError> {
    let compressed = zlib_sync(compressor, raw);
    decoder.decode(pf, rect, &compressed, surface)
}

fn run(target: &mut Vec<u8>, len: usize) {
    let mut remaining = len.saturating_sub(1);
    while remaining >= 255 {
        target.push(255);
        remaining -= 255;
    }
    target.push(u8::try_from(remaining).expect("run length byte"));
}

fn new_surface() -> MemSurface {
    let mut surface = MemSurface::new();
    surface.resize(130, 130);
    surface
}

fn new_compressor() -> Compress {
    Compress::new(Compression::default(), true)
}

#[test]
fn solid() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 4, 4),
        &[1, 1, 2, 3],
        &mut surface,
    )
    .unwrap();
    for y in 0..4 {
        for x in 0..4 {
            assert_eq!(surface.pixel(x, y), A);
        }
    }
}

#[test]
fn raw() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(1, 1, 2, 2),
        &[0, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12],
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.pixel(1, 1), A);
    assert_eq!(surface.pixel(2, 1), B);
    assert_eq!(surface.pixel(1, 2), C);
    assert_eq!(surface.pixel(2, 2), D);
    assert_eq!(surface.pixel(0, 0), [0, 0, 0, 255]);
}

#[test]
fn packed_1bit() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 3, 2),
        &[2, 1, 2, 3, 4, 5, 6, 0b1010_0000, 0b0100_0000],
        &mut surface,
    )
    .unwrap();
    assert_eq!(
        [
            surface.pixel(0, 0),
            surface.pixel(1, 0),
            surface.pixel(2, 0)
        ],
        [B, A, B]
    );
    assert_eq!(
        [
            surface.pixel(0, 1),
            surface.pixel(1, 1),
            surface.pixel(2, 1)
        ],
        [A, B, A]
    );
}

#[test]
fn packed_2bit() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 5, 1),
        &[3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0b0001_1001, 0],
        &mut surface,
    )
    .unwrap();
    assert_eq!(
        [
            surface.pixel(0, 0),
            surface.pixel(1, 0),
            surface.pixel(2, 0),
            surface.pixel(3, 0),
            surface.pixel(4, 0),
        ],
        [A, B, C, B, A]
    );
}

#[test]
fn packed_4bit() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 3, 1),
        &[
            5, 1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, 13, 14, 15, 0x40, 0x30,
        ],
        &mut surface,
    )
    .unwrap();
    assert_eq!(
        [
            surface.pixel(0, 0),
            surface.pixel(1, 0),
            surface.pixel(2, 0)
        ],
        [E, A, D]
    );
}

#[test]
fn plain_rle() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 4, 4),
        &{
            let mut raw = vec![128, 1, 2, 3];
            run(&mut raw, 10);
            raw.extend([4, 5, 6]);
            run(&mut raw, 6);
            raw
        },
        &mut surface,
    )
    .unwrap();
    for index in 0..16 {
        let expected = if index < 10 { A } else { B };
        assert_eq!(surface.pixel(index % 4, index / 4), expected);
    }
}

#[test]
fn long_run() {
    let pf = PixelFormat::BGRX32;
    let mut raw = vec![128, 1, 2, 3];
    run(&mut raw, 4096);
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 64, 64),
        &raw,
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.pixel(63, 63), A);
}

#[test]
fn palette_rle() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 2, 2),
        &{
            let mut raw = vec![130, 1, 2, 3, 4, 5, 6, 0x80];
            run(&mut raw, 3);
            raw.push(0x01);
            raw
        },
        &mut surface,
    )
    .unwrap();
    assert_eq!(
        [
            surface.pixel(0, 0),
            surface.pixel(1, 0),
            surface.pixel(0, 1),
            surface.pixel(1, 1)
        ],
        [A, A, A, B]
    );
}

#[test]
fn multi_tile() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 65, 65),
        &[1, 1, 2, 3, 1, 4, 5, 6, 1, 7, 8, 9, 1, 10, 11, 12],
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.pixel(0, 0), A);
    assert_eq!(surface.pixel(64, 0), B);
    assert_eq!(surface.pixel(0, 64), C);
    assert_eq!(surface.pixel(64, 64), D);
    assert_eq!(surface.pixel(63, 63), A);
}

#[test]
fn stream_persists() {
    let pf = PixelFormat::BGRX32;
    let mut compressor = new_compressor();
    let mut decoder = ZrleDecoder::new();
    let mut surface = new_surface();
    let first = zlib_sync(&mut compressor, &[1, 1, 2, 3]);
    decoder
        .decode(&pf, Rect::new(0, 0, 1, 1), &first, &mut surface)
        .unwrap();
    let second = zlib_sync(&mut compressor, &[1, 4, 5, 6]);
    decoder
        .decode(&pf, Rect::new(1, 1, 1, 1), &second, &mut surface)
        .unwrap();
    assert_eq!(surface.pixel(0, 0), A);
    assert_eq!(surface.pixel(1, 1), B);

    let mut fresh = ZrleDecoder::new();
    assert!(matches!(
        fresh.decode(&pf, Rect::new(1, 1, 1, 1), &second, &mut surface),
        Err(VncError::Protocol(_))
    ));
}

#[test]
fn rgb565_cpixel() {
    let pf = PixelFormat {
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
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 1, 1),
        &[1, 0x10, 0x84],
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.pixel(0, 0), [132, 130, 132, 255]);
}

#[test]
fn errors() {
    let pf = PixelFormat::BGRX32;
    let cases: &[&[u8]] = &[
        &[17],
        &[129],
        &[0, 1, 2, 3],
        &[130, 1, 2, 3, 4, 5, 6, 2, 0],
        &[128, 1, 2, 3, 16],
        &[3, 1, 2, 3, 4, 5, 6, 7, 8, 9, 0b1100_0000],
    ];
    for raw in cases {
        let mut decoder = ZrleDecoder::new();
        let mut compressor = new_compressor();
        let mut surface = new_surface();
        let result = decode(
            &mut decoder,
            &mut compressor,
            &pf,
            Rect::new(0, 0, 4, 4),
            raw,
            &mut surface,
        );
        assert!(matches!(result, Err(VncError::Protocol(_))), "raw={raw:?}");
    }

    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    assert!(matches!(
        decode(
            &mut decoder,
            &mut compressor,
            &pf,
            Rect::new(100, 100, 64, 64),
            &[1, 1, 2, 3],
            &mut surface,
        ),
        Err(VncError::Protocol(_))
    ));

    let mut decoder = ZrleDecoder::new();
    assert!(matches!(
        decoder.decode(&pf, Rect::new(0, 0, 1, 1), b"not zlib", &mut surface),
        Err(VncError::Protocol(_))
    ));
}

#[test]
fn trailing_bytes_ignored() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut compressor = new_compressor();
    let mut surface = new_surface();
    decode(
        &mut decoder,
        &mut compressor,
        &pf,
        Rect::new(0, 0, 1, 1),
        &[1, 1, 2, 3, 9, 8, 7],
        &mut surface,
    )
    .unwrap();
    assert_eq!(surface.pixel(0, 0), A);
}

#[test]
fn empty_rect() {
    let pf = PixelFormat::BGRX32;
    let mut decoder = ZrleDecoder::new();
    let mut surface = new_surface();
    decoder
        .decode(&pf, Rect::new(0, 0, 0, 0), &[], &mut surface)
        .unwrap();
    assert_eq!(surface.pixel(0, 0), [0, 0, 0, 255]);
}
