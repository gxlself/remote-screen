use std::io;

use data_encoding::BASE64;
use host_core::platform::{CursorUpdate, DisplayInfo, PlatformError};
use host_win::pipe_proto::{
    check_hello, ctl_to_cursor, cursor_to_ctl, encode_frame, input_from_b64, input_to_b64,
    read_frame, session_pipe_name, write_frame, AudioPacket, BlobKind, BlobPacket, Ctl,
    DisplayWire, FrameReader, PipeError, PipeFrame, RegionWire, RoiWire, VideoPacket, WatchKind,
    WireError, KIND_AUDIO, KIND_BLOB, KIND_CTL, KIND_VIDEO, MAX_FRAME, PIPE_PROTO_VERSION,
    UI_PIPE_NAME,
};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit, TouchPhase};
use rsp_core::pb;
use tokio::io::duplex;
use tokio::io::AsyncWriteExt;

const AUDIO_HEADER: usize = 13;

fn frame_length(bytes: &[u8]) -> usize {
    u32::from_le_bytes(bytes[..4].try_into().expect("length field")) as usize
}

fn ctl_body(ctl: &Ctl) -> Vec<u8> {
    let bytes = encode_frame(&PipeFrame::Ctl(ctl.clone())).expect("encode ctl");
    assert_eq!(frame_length(&bytes), bytes.len() - 4);
    assert_eq!(bytes[4], KIND_CTL);
    bytes[5..].to_vec()
}

fn ctl_json(ctl: &Ctl) -> String {
    String::from_utf8(ctl_body(ctl)).expect("JSON is UTF-8")
}

fn roundtrip_frame(frame: PipeFrame) {
    let bytes = encode_frame(&frame).expect("encode frame");
    assert_eq!(frame_length(&bytes), bytes.len() - 4);
    let mut reader = FrameReader::new();
    reader.push(&bytes);
    assert_eq!(reader.next().expect("read frame"), Some(frame));
    assert_eq!(reader.next().expect("end of frame"), None);
}

fn ctl_samples() -> Vec<Ctl> {
    vec![
        Ctl::Hello {
            version: PIPE_PROTO_VERSION,
            session_id: 3,
            pid: 300,
        },
        Ctl::Welcome {
            version: PIPE_PROTO_VERSION,
        },
        Ctl::Ping { n: 1 },
        Ctl::Pong { n: 2 },
        Ctl::Inject {
            display_id: 1,
            input: "AA==".into(),
        },
        Ctl::ReleaseAll,
        Ctl::Action { req: 4, kind: 1 },
        Ctl::ClipboardGet { req: 5 },
        Ctl::Watch {
            what: WatchKind::Cursor,
        },
        Ctl::Unwatch {
            what: WatchKind::Clipboard,
        },
        Ctl::CaptureStart {
            req: 6,
            cap: 7,
            display: 8,
            fps: 60,
            max_width: 1920,
            max_height: 1080,
            region: None,
        },
        Ctl::CaptureStop { cap: 7 },
        Ctl::EncoderOpen {
            req: 8,
            enc: 9,
            codec: 2,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 15_000,
        },
        Ctl::Encode {
            enc: 9,
            cap: 7,
            seq: 10,
            force_keyframe: true,
            quality_boost: false,
        },
        Ctl::EncoderRate {
            enc: 9,
            kbps: 12_000,
            fps: 30,
        },
        Ctl::EncoderKeyframe { enc: 9 },
        Ctl::EncoderBoost { enc: 9, on: true },
        Ctl::EncoderClose { enc: 9 },
        Ctl::AudioStart {
            req: 10,
            sample_rate: 48_000,
            channels: 2,
            exclude_self: true,
        },
        Ctl::AudioStop,
        Ctl::KeepAwake { on: true },
        Ctl::Done {
            req: 11,
            error: Some(WireError::PermissionDenied {
                message: "closed".into(),
            }),
        },
        Ctl::Done {
            req: 12,
            error: None,
        },
        Ctl::Displays {
            list: vec![DisplayWire {
                id: 1,
                name: "Display 1".into(),
                width_px: 1920,
                height_px: 1080,
                scale: 1.5,
                x: -1920,
                y: 0,
                refresh_hz: 60,
                primary: true,
                rotation: 0,
            }],
        },
        Ctl::Frame {
            cap: 7,
            seq: 13,
            width: 1920,
            height: 1080,
            capture_ts_us: 1_700_000_000_000_000,
        },
        Ctl::CaptureEnded {
            cap: 7,
            error: Some(WireError::Platform {
                message: "access lost".into(),
            }),
        },
        Ctl::CaptureEnded {
            cap: 8,
            error: None,
        },
        Ctl::EncoderFailed {
            enc: 9,
            error: WireError::Unsupported,
        },
        Ctl::CursorShape {
            id: 14,
            width: 32,
            height: 32,
            hot_x: 1,
            hot_y: 2,
            scale: 1.25,
            png: BASE64.encode(&[1_u8, 2, 3]),
        },
        Ctl::CursorUse { id: 14 },
        Ctl::CursorHidden { hidden: true },
        Ctl::CursorPos {
            display_id: 1,
            seq: 15,
            x: 100.5,
            y: 200.25,
            visible: true,
        },
        Ctl::Activity { active: true },
        Ctl::ClipboardChanged,
        Ctl::Desktop {
            name: "Default".into(),
        },
    ]
}

#[test]
fn pipe_names_and_version_match_the_contract() {
    assert_eq!(session_pipe_name(3), r"\\.\pipe\RemoteHost.session.3");
    assert_eq!(UI_PIPE_NAME, r"\\.\pipe\RemoteHost.ui");
    assert_eq!(PIPE_PROTO_VERSION, 1);
}

#[test]
fn every_ctl_variant_roundtrips_through_a_frame() {
    for ctl in ctl_samples() {
        roundtrip_frame(PipeFrame::Ctl(ctl));
    }
}

#[test]
fn capture_start_region_roundtrip_and_legacy_json() {
    let ctl = Ctl::CaptureStart {
        req: 1,
        cap: 2,
        display: 3,
        fps: 60,
        max_width: 1920,
        max_height: 1080,
        region: Some(RegionWire {
            x: 736,
            y: 352,
            w: 608,
            h: 352,
        }),
    };
    roundtrip_frame(PipeFrame::Ctl(ctl.clone()));
    let legacy = r#"{"t":"capture_start","req":1,"cap":2,"display":3,"fps":60,"max_width":1920,"max_height":1080}"#;
    let decoded: Ctl = serde_json::from_str(legacy).expect("legacy capture start decodes");
    assert_eq!(
        decoded,
        Ctl::CaptureStart {
            req: 1,
            cap: 2,
            display: 3,
            fps: 60,
            max_width: 1920,
            max_height: 1080,
            region: None,
        }
    );
}

#[test]
fn encoder_roi_and_info_roundtrip() {
    roundtrip_frame(PipeFrame::Ctl(Ctl::EncoderRoi {
        enc: 7,
        regions: vec![
            RoiWire {
                x: 0,
                y: 0,
                w: 64,
                h: 64,
                qp: -6,
            },
            RoiWire {
                x: 64,
                y: 64,
                w: 128,
                h: 128,
                qp: 2,
            },
        ],
    }));
    roundtrip_frame(PipeFrame::Ctl(Ctl::EncoderInfo { enc: 7, roi: true }));
}

#[test]
fn ctl_json_field_names_are_stable() {
    assert_eq!(
        ctl_json(&Ctl::Inject {
            display_id: 1,
            input: "AA==".into(),
        }),
        r#"{"t":"inject","display_id":1,"input":"AA=="}"#
    );
    assert_eq!(
        ctl_json(&Ctl::Done {
            req: 5,
            error: Some(WireError::Platform {
                message: "x".into(),
            }),
        }),
        r#"{"t":"done","req":5,"error":{"code":"platform","message":"x"}}"#
    );
    assert_eq!(
        ctl_json(&Ctl::Watch {
            what: WatchKind::Cursor,
        }),
        r#"{"t":"watch","what":"cursor"}"#
    );
}

#[test]
fn video_flags_header_and_payload_roundtrip() {
    let packet = VideoPacket {
        enc: 4,
        keyframe: true,
        still_refine: true,
        capture_ts_us: 100,
        encoded_ts_us: 101,
        data: vec![0xaa, 0xbb, 0xcc],
    };
    let bytes = encode_frame(&PipeFrame::Video(packet.clone())).expect("encode video");
    assert_eq!(bytes[4], KIND_VIDEO);
    assert_eq!(frame_length(&bytes) - 1, 21 + packet.data.len());
    assert_eq!(bytes[9], 0x03);
    roundtrip_frame(PipeFrame::Video(packet));

    roundtrip_frame(PipeFrame::Video(VideoPacket {
        enc: 5,
        keyframe: false,
        still_refine: false,
        capture_ts_us: 102,
        encoded_ts_us: 103,
        data: Vec::new(),
    }));
}

#[test]
fn audio_checks_the_sample_count() {
    let packet = AudioPacket {
        capture_ts_us: 200,
        channels: 2,
        frames: 3,
        samples: vec![-1.0, -0.5, 0.0, 0.25, 0.5, 1.0],
    };
    roundtrip_frame(PipeFrame::Audio(packet));

    let mut bytes = Vec::new();
    let samples = [0.0_f32; 5];
    let frame_len = 1 + AUDIO_HEADER + samples.len() * 4;
    bytes.extend_from_slice(&(frame_len as u32).to_le_bytes());
    bytes.push(KIND_AUDIO);
    bytes.extend_from_slice(&200_u64.to_le_bytes());
    bytes.push(2);
    bytes.extend_from_slice(&3_u32.to_le_bytes());
    for sample in samples {
        bytes.extend_from_slice(&sample.to_le_bytes());
    }

    let mut reader = FrameReader::new();
    reader.push(&bytes);
    assert!(matches!(
        reader.next(),
        Err(PipeError::BadFrame("audio samples"))
    ));
}

#[test]
fn blob_kinds_are_checked() {
    for kind in [BlobKind::Text, BlobKind::Png] {
        roundtrip_frame(PipeFrame::Blob(BlobPacket {
            req: 7,
            kind,
            data: vec![1, 2, 3],
        }));
    }

    let mut bytes = Vec::new();
    bytes.extend_from_slice(&(1_u32 + 5).to_le_bytes());
    bytes.push(KIND_BLOB);
    bytes.extend_from_slice(&7_u32.to_le_bytes());
    bytes.push(9);
    let mut reader = FrameReader::new();
    reader.push(&bytes);
    assert!(matches!(reader.next(), Err(PipeError::BadFrame(_))));
}

#[test]
fn fragmented_frames_are_read_in_order() {
    let frames = [
        PipeFrame::Ctl(Ctl::Ping { n: 31 }),
        PipeFrame::Video(VideoPacket {
            enc: 1,
            keyframe: true,
            still_refine: false,
            capture_ts_us: 32,
            encoded_ts_us: 33,
            data: vec![1, 2, 3, 4],
        }),
        PipeFrame::Blob(BlobPacket {
            req: 34,
            kind: BlobKind::Text,
            data: b"hello".to_vec(),
        }),
    ];
    let mut stream = Vec::new();
    for frame in &frames {
        stream.extend_from_slice(&encode_frame(frame).expect("encode frame"));
    }

    let mut reader = FrameReader::new();
    let mut read = Vec::new();
    for byte in stream {
        reader.push(&[byte]);
        if let Some(frame) = reader.next().expect("incremental read") {
            read.push(frame);
        }
    }
    assert_eq!(read, frames);
    assert_eq!(reader.next().expect("end of stream"), None);
}

#[test]
fn malformed_lengths_and_kinds_are_rejected_immediately() {
    let mut reader = FrameReader::new();
    reader.push(&((MAX_FRAME as u32) + 1).to_le_bytes());
    assert!(matches!(reader.next(), Err(PipeError::TooLarge(_))));

    let mut reader = FrameReader::new();
    reader.push(&0_u32.to_le_bytes());
    assert!(matches!(reader.next(), Err(PipeError::BadFrame(_))));

    let mut reader = FrameReader::new();
    reader.push(&1_u32.to_le_bytes());
    reader.push(&[9]);
    assert_eq!(
        reader.next().unwrap_err().to_string(),
        "unknown frame kind 0x09"
    );

    let body = br#"{"t":"nope"}"#;
    let mut bytes = Vec::new();
    bytes.extend_from_slice(&((body.len() + 1) as u32).to_le_bytes());
    bytes.push(KIND_CTL);
    bytes.extend_from_slice(body);
    let mut reader = FrameReader::new();
    reader.push(&bytes);
    assert!(matches!(reader.next(), Err(PipeError::BadJson(_))));
}

#[test]
fn oversized_video_encoding_is_rejected() {
    let packet = PipeFrame::Video(VideoPacket {
        enc: 1,
        keyframe: true,
        still_refine: false,
        capture_ts_us: 0,
        encoded_ts_us: 0,
        data: vec![0; MAX_FRAME],
    });
    assert!(matches!(encode_frame(&packet), Err(PipeError::TooLarge(_))));
}

#[tokio::test]
async fn async_frames_roundtrip_and_truncation_is_reported() {
    let frames = [
        PipeFrame::Ctl(Ctl::Ping { n: 41 }),
        PipeFrame::Blob(BlobPacket {
            req: 42,
            kind: BlobKind::Png,
            data: vec![3, 4, 5],
        }),
    ];
    let (mut writer, mut reader) = duplex(4096);
    for frame in &frames {
        write_frame(&mut writer, frame).await.expect("write frame");
    }
    drop(writer);

    assert_eq!(
        read_frame(&mut reader).await.expect("first frame"),
        Some(frames[0].clone())
    );
    assert_eq!(
        read_frame(&mut reader).await.expect("second frame"),
        Some(frames[1].clone())
    );
    assert_eq!(read_frame(&mut reader).await.expect("end of stream"), None);

    let full = encode_frame(&frames[0]).expect("encode frame");
    let (mut writer, mut reader) = duplex(64);
    writer
        .write_all(&full[..full.len() / 2])
        .await
        .expect("write partial frame");
    drop(writer);
    match read_frame(&mut reader).await {
        Err(PipeError::Io(error)) => assert_eq!(error.kind(), io::ErrorKind::UnexpectedEof),
        other => panic!("expected unexpected EOF, got {other:?}"),
    }
}

#[test]
fn hello_checks_version_and_variant() {
    assert!(matches!(
        check_hello(&Ctl::Hello {
            version: PIPE_PROTO_VERSION,
            session_id: 2,
            pid: 300,
        }),
        Ok((2, 300))
    ));
    assert!(matches!(
        check_hello(&Ctl::Hello {
            version: 2,
            session_id: 2,
            pid: 300,
        }),
        Err(PipeError::Version { got: 2 })
    ));
    assert!(matches!(
        check_hello(&Ctl::Ping { n: 1 }),
        Err(PipeError::BadFrame("hello"))
    ));
}

#[test]
fn input_base64_roundtrips_every_variant() {
    let inputs = [
        InputMsg::MouseMove { x: 0.25, y: 0.75 },
        InputMsg::MouseButton {
            button: Button::Back,
            down: true,
            x: 0.5,
            y: 0.625,
            clicks: 2,
        },
        InputMsg::Scroll {
            dx: 1.5,
            dy: -2.25,
            phase: ScrollPhase::Began,
            momentum: ScrollPhase::Ended,
            unit: ScrollUnit::Pixel,
        },
        InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::LCTRL | Mods::LSHIFT,
        },
        InputMsg::Text("hello".into()),
        InputMsg::ReleaseAll,
        InputMsg::Touch {
            id: 2,
            phase: TouchPhase::Moved,
            x: 0.1,
            y: 0.2,
            pressure: 0.5,
        },
    ];
    for input in inputs {
        let encoded = input_to_b64(&input).expect("encode input");
        assert_eq!(input_from_b64(&encoded).expect("decode input"), input);
    }
    assert!(matches!(
        input_from_b64("!!"),
        Err(PipeError::BadFrame("input"))
    ));
}

#[test]
fn display_wire_roundtrips_display_info() {
    let display = DisplayInfo {
        id: 2,
        name: "Secondary".into(),
        width_px: 2560,
        height_px: 1440,
        scale: 1.25,
        x: 1920,
        y: -200,
        refresh_hz: 144,
        primary: false,
        rotation: 90,
    };
    let wire = DisplayWire::from(&display);
    assert_eq!(DisplayInfo::from(wire), display);
}

#[test]
fn cursor_updates_roundtrip_and_ignore_non_cursor_messages() {
    let updates = [
        CursorUpdate::Shape(pb::CursorShape {
            id: 1,
            width: 32,
            height: 64,
            hot_x: 3,
            hot_y: 4,
            scale: 2.0,
            png: vec![0x89, b'P', b'N', b'G'],
        }),
        CursorUpdate::Use(1),
        CursorUpdate::Hidden(true),
        CursorUpdate::Position {
            display_id: 2,
            seq: 3,
            x: 12.5,
            y: 34.75,
            visible: false,
        },
    ];
    for update in updates {
        assert_eq!(ctl_to_cursor(&cursor_to_ctl(&update)), Some(update));
    }
    assert_eq!(ctl_to_cursor(&Ctl::Activity { active: true }), None);

    let invalid_shape = Ctl::CursorShape {
        id: 1,
        width: 1,
        height: 1,
        hot_x: 0,
        hot_y: 0,
        scale: 1.0,
        png: "!!".into(),
    };
    assert_eq!(ctl_to_cursor(&invalid_shape), None);
}

#[test]
fn platform_errors_roundtrip_through_wire_errors() {
    let unsupported = PlatformError::Unsupported;
    assert_eq!(WireError::from(&unsupported), WireError::Unsupported);
    assert!(matches!(
        PlatformError::from(WireError::Unsupported),
        PlatformError::Unsupported
    ));

    let denied = PlatformError::PermissionDenied("a".into());
    let wire = WireError::from(&denied);
    assert_eq!(
        wire,
        WireError::PermissionDenied {
            message: "a".into()
        }
    );
    assert!(matches!(
        PlatformError::from(wire),
        PlatformError::PermissionDenied(message) if message == "a"
    ));

    let platform = PlatformError::Platform("b".into());
    assert_eq!(
        WireError::from(&platform),
        WireError::Platform {
            message: "b".into()
        }
    );
    let io_error = PlatformError::Io(io::Error::other("disk"));
    assert_eq!(
        WireError::from(&io_error),
        WireError::Platform {
            message: "disk".into()
        }
    );
}
