use host_core::config::VideoEncoder;
use host_core::cursor_shape::{
    to_cursor_shape, to_rgba, PointerKind, PointerShape, ShapeError, MAX_CURSOR_SIDE,
};
use host_core::encoder_pick::{
    candidates, ffmpeg_encoder_name, supports, vendor_encoder, EncoderKind, VENDOR_AMD,
    VENDOR_INTEL, VENDOR_NVIDIA,
};
use host_win::frame_gate::{FrameAction, FrameGate, FrameInfoLite};
use rsp_core::cursor::shape_id;
use rsp_core::pb;

#[test]
fn monochrome_shape_converts_all_four_mask_combinations() {
    let shape = PointerShape {
        kind: PointerKind::Monochrome,
        width: 2,
        height: 4,
        pitch: 1,
        hot_x: 0,
        hot_y: 0,
        data: vec![0x00, 0b1100_0000, 0b0100_0000, 0b0100_0000],
    };

    let rgba = to_rgba(&shape).expect("decode monochrome pointer");
    assert_eq!(rgba.width, 2);
    assert_eq!(rgba.height, 2);
    assert_eq!(
        rgba.pixels,
        [
            0, 0, 0, 255, // AND=0 XOR=0: opaque black
            255, 255, 255, 255, // AND=0 XOR=1: opaque white
            0, 0, 0, 0, // AND=1 XOR=0: transparent
            0, 0, 0, 255, // AND=1 XOR=1: inverted, represented as black
        ]
    );
}

#[test]
fn color_shape_swaps_bgra_to_rgba_and_keeps_alpha() {
    let shape = PointerShape {
        kind: PointerKind::Color,
        width: 1,
        height: 2,
        pitch: 4,
        hot_x: 0,
        hot_y: 0,
        data: vec![1, 2, 3, 4, 5, 6, 7, 8],
    };

    let rgba = to_rgba(&shape).expect("decode color pointer");
    assert_eq!(rgba.pixels, [3, 2, 1, 4, 7, 6, 5, 8]);
}

#[test]
fn masked_color_shape_uses_mask_byte_instead_of_alpha() {
    let shape = PointerShape {
        kind: PointerKind::MaskedColor,
        width: 1,
        height: 3,
        pitch: 4,
        hot_x: 0,
        hot_y: 0,
        data: vec![10, 20, 30, 0x00, 0, 0, 0, 0xFF, 1, 2, 3, 0xFF],
    };

    let rgba = to_rgba(&shape).expect("decode masked color pointer");
    assert_eq!(rgba.pixels[0..4], [30, 20, 10, 255]);
    assert_eq!(rgba.pixels[4..8], [0, 0, 0, 0]);
    assert_eq!(rgba.pixels[8..12], [3, 2, 1, 255]);
}

#[test]
fn invalid_shapes_return_specific_errors() {
    assert_eq!(
        to_rgba(&PointerShape {
            kind: PointerKind::Color,
            width: 0,
            height: 1,
            pitch: 4,
            hot_x: 0,
            hot_y: 0,
            data: vec![0; 4],
        }),
        Err(ShapeError::Empty)
    );
    assert_eq!(
        to_rgba(&PointerShape {
            kind: PointerKind::Monochrome,
            width: 1,
            height: 1,
            pitch: 1,
            hot_x: 0,
            hot_y: 0,
            data: vec![0; 1],
        }),
        Err(ShapeError::Empty)
    );
    assert_eq!(
        to_rgba(&PointerShape {
            kind: PointerKind::Color,
            width: MAX_CURSOR_SIDE + 1,
            height: 1,
            pitch: 4 * (MAX_CURSOR_SIDE + 1),
            hot_x: 0,
            hot_y: 0,
            data: vec![0; 4 * (MAX_CURSOR_SIDE as usize + 1)],
        }),
        Err(ShapeError::TooLarge)
    );
    assert_eq!(
        to_rgba(&PointerShape {
            kind: PointerKind::Color,
            width: 1,
            height: 1,
            pitch: 4,
            hot_x: 0,
            hot_y: 0,
            data: vec![0; 3],
        }),
        Err(ShapeError::Short)
    );
    assert_eq!(
        to_rgba(&PointerShape {
            kind: PointerKind::Color,
            width: 2,
            height: 1,
            pitch: 4,
            hot_x: 0,
            hot_y: 0,
            data: vec![0; 8],
        }),
        Err(ShapeError::Short)
    );
}

#[test]
fn cursor_shape_png_matches_pixels_and_hash_metadata() {
    let data: Vec<u8> = (0..32 * 2 * 4)
        .map(|index| u8::try_from((index * 37) % 256).expect("byte value"))
        .collect();
    let shape = PointerShape {
        kind: PointerKind::Color,
        width: 32,
        height: 2,
        pitch: 32 * 4,
        hot_x: 99,
        hot_y: 99,
        data,
    };
    let rgba = to_rgba(&shape).expect("decode pointer");
    let cursor = to_cursor_shape(&shape, 1.5).expect("encode pointer");
    assert_eq!(cursor.width, rgba.width);
    assert_eq!(cursor.height, rgba.height);
    assert_eq!(cursor.hot_x, 31);
    assert_eq!(cursor.hot_y, 1);
    assert_eq!(cursor.scale, 1.5);
    assert_eq!(cursor.id, shape_id(&cursor.png));
    assert_eq!(
        cursor.id,
        to_cursor_shape(&shape, 1.5)
            .expect("encode pointer again")
            .id
    );

    let decoder = png::Decoder::new(std::io::Cursor::new(cursor.png.as_slice()));
    let mut reader = decoder.read_info().expect("read png header");
    let mut pixels = vec![0; reader.output_buffer_size().expect("png output size")];
    let info = reader.next_frame(&mut pixels).expect("decode png");
    pixels.truncate(info.buffer_size());
    assert_eq!((info.width, info.height), (rgba.width, rgba.height));
    assert_eq!(pixels, rgba.pixels);
}

#[test]
fn frame_gate_tracks_fresh_presents_and_metadata() {
    let mut gate = FrameGate::new();
    assert_eq!(
        gate.decide(&FrameInfoLite {
            last_present_time: 1,
            total_metadata_buffer_size: 0,
            ..FrameInfoLite::default()
        }),
        FrameAction::Encode
    );
    assert_eq!(
        gate.decide(&FrameInfoLite {
            last_present_time: 1,
            total_metadata_buffer_size: 0,
            ..FrameInfoLite::default()
        }),
        FrameAction::Skip
    );
    assert_eq!(
        gate.decide(&FrameInfoLite {
            last_present_time: 1,
            total_metadata_buffer_size: 64,
            ..FrameInfoLite::default()
        }),
        FrameAction::Encode
    );
    assert_eq!(
        gate.decide(&FrameInfoLite {
            last_mouse_update_time: 5,
            ..FrameInfoLite::default()
        }),
        FrameAction::PointerOnly
    );
    assert_eq!(gate.decide(&FrameInfoLite::default()), FrameAction::Skip);
    gate.reset();
    assert_eq!(
        gate.decide(&FrameInfoLite {
            last_present_time: 1,
            total_metadata_buffer_size: 0,
            ..FrameInfoLite::default()
        }),
        FrameAction::Encode
    );
}

#[test]
fn candidates_follow_vendor_forced_and_codec_rules() {
    assert_eq!(vendor_encoder(VENDOR_NVIDIA), Some(EncoderKind::Nvenc));
    assert_eq!(vendor_encoder(VENDOR_AMD), Some(EncoderKind::Amf));
    assert_eq!(vendor_encoder(VENDOR_INTEL), Some(EncoderKind::Qsv));
    assert!(supports(EncoderKind::Nvenc, pb::Codec::Hevc));
    assert!(supports(EncoderKind::Nvenc, pb::Codec::H264));
    assert!(!supports(EncoderKind::OpenH264, pb::Codec::Hevc));
    assert!(supports(EncoderKind::OpenH264, pb::Codec::H264));

    assert_eq!(
        candidates(Some(VENDOR_NVIDIA), VideoEncoder::Auto, pb::Codec::Hevc),
        [EncoderKind::Nvenc, EncoderKind::MediaFoundation]
    );
    assert_eq!(
        candidates(Some(VENDOR_NVIDIA), VideoEncoder::Auto, pb::Codec::H264),
        [
            EncoderKind::Nvenc,
            EncoderKind::MediaFoundation,
            EncoderKind::OpenH264,
        ]
    );
    assert_eq!(
        candidates(Some(0x1234), VideoEncoder::Auto, pb::Codec::Hevc),
        [EncoderKind::MediaFoundation]
    );
    assert_eq!(
        candidates(None, VideoEncoder::Auto, pb::Codec::H264),
        [EncoderKind::MediaFoundation, EncoderKind::OpenH264]
    );
    assert_eq!(
        candidates(Some(VENDOR_INTEL), VideoEncoder::Amf, pb::Codec::Hevc),
        [EncoderKind::Amf]
    );
    assert!(candidates(None, VideoEncoder::OpenH264, pb::Codec::Hevc).is_empty());
    assert_eq!(
        candidates(None, VideoEncoder::OpenH264, pb::Codec::H264),
        [EncoderKind::OpenH264]
    );
    assert_eq!(
        candidates(None, VideoEncoder::Auto, pb::Codec::Unspecified),
        [EncoderKind::MediaFoundation]
    );
}

#[test]
fn ffmpeg_names_cover_hardware_encoders_only() {
    assert_eq!(
        ffmpeg_encoder_name(EncoderKind::Qsv, pb::Codec::H264),
        Some("h264_qsv")
    );
    assert_eq!(
        ffmpeg_encoder_name(EncoderKind::MediaFoundation, pb::Codec::Hevc),
        None
    );
    assert_eq!(
        ffmpeg_encoder_name(EncoderKind::Nvenc, pb::Codec::Hevc),
        Some("hevc_nvenc")
    );
    assert_eq!(
        ffmpeg_encoder_name(EncoderKind::Amf, pb::Codec::H264),
        Some("h264_amf")
    );
    assert_eq!(
        ffmpeg_encoder_name(EncoderKind::OpenH264, pb::Codec::H264),
        None
    );
}
