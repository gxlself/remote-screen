use rs_vnc::proto::{
    choose_security, client_init, client_version_bytes, decode_pixel_format, encode_bell,
    encode_fb_update_header, encode_pixel_format, encode_reason, encode_rect_header,
    encode_security_result, encode_security_type_33, encode_security_types, encode_server_cut_text,
    encode_server_init, from_latin1, needs_security_result, parse_server_version, read_client_msg,
    read_reason, read_rect_header, read_security_result, read_security_types, read_server_init,
    read_server_msg_header, read_version, skip, to_latin1, ClientMsg, RectHeader, ServerInit,
    ServerMsgHeader, Version,
};
use rs_vnc::types::{PixelFormat, Rect, VncError, MAX_CUT_TEXT, MAX_FB_SIDE, MAX_NAME_LEN};
use tokio::io::AsyncReadExt;

const BGRX32_BYTES: [u8; 16] = [32, 24, 0, 1, 0, 255, 0, 255, 0, 255, 16, 8, 0, 0, 0, 0];

fn studio() -> ServerInit {
    ServerInit {
        width: 1920,
        height: 1080,
        pixel_format: PixelFormat::BGRX32,
        name: "Studio".into(),
    }
}

fn client_vectors() -> [(ClientMsg, Vec<u8>); 6] {
    [
        (
            ClientMsg::SetEncodings(vec![16, 1, 0, -239]),
            vec![
                2, 0, 0, 4, 0, 0, 0, 16, 0, 0, 0, 1, 0, 0, 0, 0, 0xff, 0xff, 0xff, 0x11,
            ],
        ),
        (
            ClientMsg::FramebufferUpdateRequest {
                incremental: true,
                rect: Rect::new(0, 0, 1920, 1080),
            },
            vec![3, 1, 0, 0, 0, 0, 7, 0x80, 4, 0x38],
        ),
        (
            ClientMsg::KeyEvent {
                down: true,
                keysym: 0xff0d,
            },
            vec![4, 1, 0, 0, 0, 0, 0xff, 0x0d],
        ),
        (
            ClientMsg::PointerEvent {
                mask: 1,
                x: 100,
                y: 200,
            },
            vec![5, 1, 0, 100, 0, 200],
        ),
        (
            ClientMsg::ClientCutText(b"hi".to_vec()),
            vec![6, 0, 0, 0, 0, 0, 0, 2, b'h', b'i'],
        ),
        (
            ClientMsg::SetPixelFormat(PixelFormat::BGRX32),
            [vec![0, 0, 0, 0], BGRX32_BYTES.to_vec()].concat(),
        ),
    ]
}

#[test]
fn version_parse() {
    for (banner, version) in [
        (b"RFB 003.008\n", Version::V3_8),
        (b"RFB 003.007\n", Version::V3_7),
        (b"RFB 003.003\n", Version::V3_3),
        (b"RFB 003.004\n", Version::V3_3),
        (b"RFB 003.005\n", Version::V3_3),
        (b"RFB 003.006\n", Version::V3_3),
        (b"RFB 003.009\n", Version::V3_8),
        (b"RFB 003.889\n", Version::V3_8),
        (b"RFB 003.999\n", Version::V3_8),
        (b"RFB 004.001\n", Version::V3_8),
        (b"RFB 999.000\n", Version::V3_8),
    ] {
        assert_eq!(parse_server_version(banner), Ok(version));
    }
    for banner in [
        b"RFB 003.000\n",
        b"RFB 003.001\n",
        b"RFB 003.002\n",
        b"RFB 002.009\n",
        b"RFB 000.999\n",
    ] {
        assert_eq!(
            parse_server_version(banner),
            Err(VncError::UnsupportedVersion(
                String::from_utf8_lossy(&banner[..11]).into_owned()
            ))
        );
    }
    for banner in [
        b"RFX 003.008\n",
        b"RFB 003.008\r",
        b"RFB 0a3.008\n",
        b"RFB 003.a08\n",
        b"RFB 003,008\n",
        b"RFB\t003.008\n",
        b"RFB 003.00\xff\n",
    ] {
        assert!(matches!(
            parse_server_version(banner),
            Err(VncError::Protocol(reason)) if reason.starts_with("bad version ")
        ));
    }
}

#[test]
fn client_version_bytes_exact() {
    assert_eq!(client_version_bytes(Version::V3_3), *b"RFB 003.003\n");
    assert_eq!(client_version_bytes(Version::V3_7), *b"RFB 003.007\n");
    assert_eq!(client_version_bytes(Version::V3_8), *b"RFB 003.008\n");
    assert!(Version::V3_3 < Version::V3_7);
    assert!(Version::V3_7 < Version::V3_8);
}

#[tokio::test]
async fn read_version_eof() {
    let mut input = b"RFB 003".as_slice();
    assert_eq!(
        read_version(&mut input).await,
        Err(VncError::Closed("eof".into()))
    );
}

#[tokio::test]
async fn read_version_framing() -> Result<(), VncError> {
    let mut input = b"RFB 003.889\n\x02".as_slice();
    assert_eq!(read_version(&mut input).await?, Version::V3_8);
    assert_eq!(input, &[2]);
    Ok(())
}

#[tokio::test]
async fn security_types_38() -> Result<(), VncError> {
    for version in [Version::V3_7, Version::V3_8] {
        let mut input = [2, 1, 30].as_slice();
        assert_eq!(read_security_types(&mut input, version).await?, vec![1, 30]);
        assert!(input.is_empty());

        let mut input = [0, 0, 0, 0, 5, b'b', b'u', b's', b'y', b'!'].as_slice();
        assert_eq!(
            read_security_types(&mut input, version).await,
            Err(VncError::Refused("busy!".into()))
        );
    }
    Ok(())
}

#[tokio::test]
async fn security_types_33() -> Result<(), VncError> {
    let mut input = [0, 0, 0, 2].as_slice();
    assert_eq!(
        read_security_types(&mut input, Version::V3_3).await?,
        vec![2]
    );
    let mut input = [0, 0, 0, 0, 0, 0, 0, 2, b'n', b'o'].as_slice();
    assert_eq!(
        read_security_types(&mut input, Version::V3_3).await,
        Err(VncError::Refused("no".into()))
    );
    for bytes in [[0, 0, 1, 0], [255, 255, 255, 255]] {
        let mut input = bytes.as_slice();
        assert!(matches!(
            read_security_types(&mut input, Version::V3_3).await,
            Err(VncError::Protocol(_))
        ));
    }
    for t in [1, 30, 255] {
        let bytes = [0, 0, 0, t];
        let mut input = bytes.as_slice();
        assert_eq!(
            read_security_types(&mut input, Version::V3_3).await?,
            vec![t]
        );
    }
    Ok(())
}

#[test]
fn choose_security_order() {
    assert_eq!(choose_security(&[2, 30]), Some(30));
    assert_eq!(choose_security(&[2]), Some(2));
    assert_eq!(choose_security(&[30, 1]), Some(1));
    assert_eq!(choose_security(&[2, 30, 1, 1]), Some(1));
    assert_eq!(choose_security(&[16, 18]), None);
    assert_eq!(choose_security(&[0]), None);
    assert_eq!(choose_security(&[]), None);
}

#[test]
fn needs_security_result_table() {
    for (version, sec, expected) in [
        (Version::V3_8, 1, true),
        (Version::V3_7, 1, false),
        (Version::V3_3, 1, false),
        (Version::V3_7, 2, true),
        (Version::V3_3, 2, true),
        (Version::V3_8, 30, true),
        (Version::V3_7, 30, true),
        (Version::V3_3, 30, true),
    ] {
        assert_eq!(needs_security_result(version, sec), expected);
    }
}

#[tokio::test]
async fn security_result() -> Result<(), VncError> {
    for version in [Version::V3_3, Version::V3_7, Version::V3_8] {
        let mut input = [0, 0, 0, 0, 2].as_slice();
        read_security_result(&mut input, version).await?;
        assert_eq!(input, &[2]);
    }
    let mut input = [0, 0, 0, 1, 0, 0, 0, 3, b'b', b'a', b'd'].as_slice();
    assert_eq!(
        read_security_result(&mut input, Version::V3_8).await,
        Err(VncError::AuthFailed("bad".into()))
    );
    for version in [Version::V3_3, Version::V3_7] {
        let mut input = [0, 0, 0, 1, 2].as_slice();
        assert_eq!(
            read_security_result(&mut input, version).await,
            Err(VncError::AuthFailed(String::new()))
        );
        assert_eq!(input, &[2]);
    }
    let mut input = [0, 0, 0, 2, 0, 0, 0, 0].as_slice();
    assert_eq!(
        read_security_result(&mut input, Version::V3_8).await,
        Err(VncError::AuthFailed(String::new()))
    );
    Ok(())
}

#[tokio::test]
async fn reason_limits_and_lossy_utf8() -> Result<(), VncError> {
    for len in [MAX_NAME_LEN + 1, u32::MAX] {
        let bytes = len.to_be_bytes();
        let mut input = bytes.as_slice();
        assert!(matches!(
            read_reason(&mut input).await,
            Err(VncError::Protocol(_))
        ));
    }
    let bytes = encode_reason(&"x".repeat(MAX_NAME_LEN as usize));
    let mut input = bytes.as_slice();
    assert_eq!(read_reason(&mut input).await?.len(), MAX_NAME_LEN as usize);

    let mut input = [0, 0, 0, 3, b'f', 0xff, b'o', 2].as_slice();
    assert_eq!(read_reason(&mut input).await?, "f\u{fffd}o");
    assert_eq!(input, &[2]);

    let mut input = [0, 0, 0, 0].as_slice();
    assert_eq!(read_reason(&mut input).await?, "");
    Ok(())
}

#[tokio::test]
async fn security_rejects_oversized_reasons() {
    for version in [Version::V3_3, Version::V3_7, Version::V3_8] {
        let mut bytes = if version == Version::V3_3 {
            vec![0, 0, 0, 0]
        } else {
            vec![0]
        };
        bytes.extend_from_slice(&(MAX_NAME_LEN + 1).to_be_bytes());
        let mut input = bytes.as_slice();
        assert!(matches!(
            read_security_types(&mut input, version).await,
            Err(VncError::Protocol(_))
        ));
    }
    let mut bytes = vec![0, 0, 0, 1];
    bytes.extend_from_slice(&(MAX_NAME_LEN + 1).to_be_bytes());
    let mut input = bytes.as_slice();
    assert!(matches!(
        read_security_result(&mut input, Version::V3_8).await,
        Err(VncError::Protocol(_))
    ));
}

#[test]
fn pixel_format_bgrx32_bytes() {
    assert_eq!(encode_pixel_format(&PixelFormat::BGRX32), BGRX32_BYTES);
    assert_eq!(decode_pixel_format(&BGRX32_BYTES), PixelFormat::BGRX32);

    let rgb565 = PixelFormat {
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
    assert_eq!(
        encode_pixel_format(&rgb565),
        [16, 16, 0, 1, 0, 31, 0, 63, 0, 31, 11, 5, 0, 0, 0, 0]
    );
    assert_eq!(decode_pixel_format(&encode_pixel_format(&rgb565)), rgb565);
}

#[test]
fn pixel_format_flags_and_padding() {
    let b = [
        7, 255, 2, 255, 0x12, 0x34, 0x56, 0x78, 0x9a, 0xbc, 31, 32, 33, 1, 2, 3,
    ];
    let pf = decode_pixel_format(&b);
    assert_eq!(
        pf,
        PixelFormat {
            bits_per_pixel: 7,
            depth: 255,
            big_endian: true,
            true_colour: true,
            red_max: 0x1234,
            green_max: 0x5678,
            blue_max: 0x9abc,
            red_shift: 31,
            green_shift: 32,
            blue_shift: 33,
        }
    );
    let encoded = encode_pixel_format(&pf);
    assert_eq!(&encoded[2..4], &[1, 1]);
    assert_eq!(&encoded[13..], &[0, 0, 0]);
    assert!(!decode_pixel_format(&[0; 16]).big_endian);
    assert!(!decode_pixel_format(&[0; 16]).true_colour);
}

#[tokio::test]
async fn server_init_roundtrip() -> Result<(), VncError> {
    let si = studio();
    let mut bytes = encode_server_init(&si);
    assert_eq!(&bytes[..4], &[7, 0x80, 4, 0x38]);
    assert_eq!(&bytes[4..20], &BGRX32_BYTES);
    assert_eq!(
        &bytes[20..],
        &[0, 0, 0, 6, b'S', b't', b'u', b'd', b'i', b'o']
    );
    bytes.push(2);
    let mut input = bytes.as_slice();
    assert_eq!(read_server_init(&mut input).await?, si);
    assert_eq!(input, &[2]);
    assert_eq!(client_init(true), [1]);
    assert_eq!(client_init(false), [0]);
    Ok(())
}

#[tokio::test]
async fn server_init_limits() -> Result<(), VncError> {
    for (width, height) in [(0u16, 1u16), (9000, 1), (1, 0), (1, 9000), (8193, 1)] {
        let bytes = [width.to_be_bytes(), height.to_be_bytes()].concat();
        let mut input = bytes.as_slice();
        assert!(matches!(
            read_server_init(&mut input).await,
            Err(VncError::Protocol(_))
        ));
    }
    for len in [MAX_NAME_LEN + 1, u32::MAX] {
        let mut bytes = encode_server_init(&studio());
        bytes.truncate(20);
        bytes.extend_from_slice(&len.to_be_bytes());
        let mut input = bytes.as_slice();
        assert!(matches!(
            read_server_init(&mut input).await,
            Err(VncError::Protocol(_))
        ));
    }
    let si = ServerInit {
        width: MAX_FB_SIDE,
        height: MAX_FB_SIDE,
        name: "x".repeat(MAX_NAME_LEN as usize),
        ..studio()
    };
    let bytes = encode_server_init(&si);
    let mut input = bytes.as_slice();
    assert_eq!(read_server_init(&mut input).await?, si);
    Ok(())
}

#[tokio::test]
async fn server_init_lossy_name() -> Result<(), VncError> {
    let mut bytes = encode_server_init(&studio());
    bytes.truncate(20);
    bytes.extend_from_slice(&[0, 0, 0, 2, 0xff, b'A']);
    let mut input = bytes.as_slice();
    assert_eq!(read_server_init(&mut input).await?.name, "\u{fffd}A");
    Ok(())
}

#[test]
fn client_msg_vectors() -> Result<(), VncError> {
    for (message, expected) in client_vectors() {
        assert_eq!(message.encode()?, expected);
    }
    Ok(())
}

#[test]
fn client_msg_limits() {
    assert!(matches!(
        ClientMsg::SetEncodings(vec![0; 65]).encode(),
        Err(VncError::Protocol(_))
    ));
    assert!(matches!(
        ClientMsg::ClientCutText(vec![0; MAX_CUT_TEXT as usize + 1]).encode(),
        Err(VncError::Protocol(_))
    ));
}

#[tokio::test]
async fn read_client_msg_roundtrip() -> Result<(), VncError> {
    for (message, _) in client_vectors() {
        let bytes = message.encode()?;
        let mut input = bytes.as_slice();
        assert_eq!(read_client_msg(&mut input).await?, message);
        assert!(input.is_empty());
    }
    let mut input = [9].as_slice();
    assert_eq!(
        read_client_msg(&mut input).await,
        Err(VncError::Protocol("unknown client message type 9".into()))
    );
    Ok(())
}

#[tokio::test]
async fn read_client_msg_limits() -> Result<(), VncError> {
    for bytes in [[2, 0, 0, 65], [2, 0, 255, 255]] {
        let mut input = bytes.as_slice();
        assert!(matches!(
            read_client_msg(&mut input).await,
            Err(VncError::Protocol(_))
        ));
    }
    for len in [MAX_CUT_TEXT + 1, u32::MAX] {
        let mut bytes = vec![6, 0, 0, 0];
        bytes.extend_from_slice(&len.to_be_bytes());
        let mut input = bytes.as_slice();
        assert!(matches!(
            read_client_msg(&mut input).await,
            Err(VncError::Protocol(_))
        ));
    }
    for message in [
        ClientMsg::SetEncodings(vec![i32::MIN; 64]),
        ClientMsg::SetEncodings(Vec::new()),
        ClientMsg::ClientCutText(vec![0xff; MAX_CUT_TEXT as usize]),
        ClientMsg::ClientCutText(Vec::new()),
    ] {
        let bytes = message.encode()?;
        let mut input = bytes.as_slice();
        assert_eq!(read_client_msg(&mut input).await?, message);
        assert!(input.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn client_msg_stream_and_flags() -> Result<(), VncError> {
    let messages = client_vectors();
    let bytes: Vec<u8> = messages
        .iter()
        .flat_map(|(_, bytes)| bytes.clone())
        .collect();
    let mut input = bytes.as_slice();
    for (message, _) in messages {
        assert_eq!(read_client_msg(&mut input).await?, message);
    }
    assert!(input.is_empty());

    for (mut message, mut bytes) in [client_vectors()[1].clone(), client_vectors()[2].clone()] {
        bytes[1] = 255;
        let mut input = bytes.as_slice();
        assert_eq!(read_client_msg(&mut input).await?, message);
        match &mut message {
            ClientMsg::FramebufferUpdateRequest { incremental, .. } => *incremental = false,
            ClientMsg::KeyEvent { down, .. } => *down = false,
            _ => {}
        }
        bytes[1] = 0;
        let mut input = bytes.as_slice();
        assert_eq!(read_client_msg(&mut input).await?, message);
        assert_eq!(message.encode()?, bytes);
    }
    Ok(())
}

#[tokio::test]
async fn server_msg_headers() -> Result<(), VncError> {
    for (bytes, expected) in [
        (
            vec![0, 0, 0, 3],
            ServerMsgHeader::FramebufferUpdate { rects: 3 },
        ),
        (
            vec![1, 0, 0, 0, 0, 2],
            ServerMsgHeader::SetColourMapEntries { first: 0, count: 2 },
        ),
        (vec![2], ServerMsgHeader::Bell),
        (
            vec![3, 0, 0, 0, 0, 0, 0, 5],
            ServerMsgHeader::ServerCutText { len: 5 },
        ),
    ] {
        let mut input = bytes.as_slice();
        assert_eq!(read_server_msg_header(&mut input).await?, expected);
        assert!(input.is_empty());
    }
    let mut input = [9].as_slice();
    assert_eq!(
        read_server_msg_header(&mut input).await,
        Err(VncError::Protocol("unknown server message type 9".into()))
    );
    Ok(())
}

#[tokio::test]
async fn server_headers_leave_payloads() -> Result<(), VncError> {
    let mut input = [1, 255, 0x12, 0x34, 0, 1, 0, 1, 0, 2, 0, 3, 2].as_slice();
    assert_eq!(
        read_server_msg_header(&mut input).await?,
        ServerMsgHeader::SetColourMapEntries {
            first: 0x1234,
            count: 1
        }
    );
    assert_eq!(input, &[0, 1, 0, 2, 0, 3, 2]);
    skip(&mut input, 6).await?;
    assert_eq!(
        read_server_msg_header(&mut input).await?,
        ServerMsgHeader::Bell
    );

    let mut input = [3, 1, 2, 3, 255, 255, 255, 255].as_slice();
    assert_eq!(
        read_server_msg_header(&mut input).await?,
        ServerMsgHeader::ServerCutText { len: u32::MAX }
    );
    assert!(input.is_empty());
    Ok(())
}

#[tokio::test]
async fn rect_header() -> Result<(), VncError> {
    for (bytes, encoding) in [
        ([0, 10, 0, 20, 0, 30, 0, 40, 0, 0, 0, 16], 16),
        ([0, 10, 0, 20, 0, 30, 0, 40, 255, 255, 255, 0x11], -239),
    ] {
        let mut input = bytes.as_slice();
        let header = read_rect_header(&mut input).await?;
        assert_eq!(
            header,
            RectHeader {
                rect: Rect::new(10, 20, 30, 40),
                encoding
            }
        );
        assert_eq!(encode_rect_header(&header), bytes);
        assert!(input.is_empty());
    }
    Ok(())
}

#[tokio::test]
async fn skip_exact() -> Result<(), VncError> {
    let mut input = [0, 1, 2, 3, 4, 5, 6, 7, 8, 9].as_slice();
    skip(&mut input, 5).await?;
    let mut remaining = [0; 5];
    input.read_exact(&mut remaining).await?;
    assert_eq!(remaining, [5, 6, 7, 8, 9]);

    let mut input = [0; 10].as_slice();
    assert_eq!(
        skip(&mut input, 11).await,
        Err(VncError::Closed("eof".into()))
    );

    let bytes = vec![0; 200_000];
    let mut input = bytes.as_slice();
    skip(&mut input, 200_000).await?;
    assert!(input.is_empty());

    let mut input = [2].as_slice();
    skip(&mut input, 0).await?;
    assert_eq!(input, &[2]);
    assert_eq!(
        skip(&mut input, u64::MAX).await,
        Err(VncError::Closed("eof".into()))
    );
    Ok(())
}

#[tokio::test]
async fn server_encoders() -> Result<(), VncError> {
    assert_eq!(encode_security_types(&[1, 30]), [2, 1, 30]);
    assert_eq!(encode_security_types(&[]), [0]);
    assert_eq!(encode_security_type_33(0x1234), [0, 0, 0x12, 0x34]);
    assert_eq!(encode_reason(""), [0, 0, 0, 0]);
    assert_eq!(
        encode_reason("café"),
        [0, 0, 0, 5, b'c', b'a', b'f', 0xc3, 0xa9]
    );
    assert_eq!(encode_security_result(true, None), [0, 0, 0, 0]);
    assert_eq!(encode_security_result(true, Some("ignored")), [0, 0, 0, 0]);
    assert_eq!(encode_security_result(false, None), [0, 0, 0, 1]);
    assert_eq!(
        encode_security_result(false, Some("")),
        [0, 0, 0, 1, 0, 0, 0, 0]
    );

    let bytes = encode_security_result(false, Some("bad"));
    let mut input = bytes.as_slice();
    assert_eq!(
        read_security_result(&mut input, Version::V3_8).await,
        Err(VncError::AuthFailed("bad".into()))
    );

    assert_eq!(encode_fb_update_header(0x1234), [0, 0, 0x12, 0x34]);
    assert_eq!(encode_bell(), [2]);
    let bytes = encode_server_cut_text(b"ab");
    assert_eq!(bytes, [3, 0, 0, 0, 0, 0, 0, 2, b'a', b'b']);
    let mut input = bytes.as_slice();
    assert_eq!(
        read_server_msg_header(&mut input).await?,
        ServerMsgHeader::ServerCutText { len: 2 }
    );
    assert_eq!(input, b"ab");
    Ok(())
}

#[tokio::test]
async fn security_type_count_wire_limit() -> Result<(), VncError> {
    let bytes = encode_security_types(&[1; 256]);
    assert_eq!(bytes.len(), 256);
    assert_eq!(bytes[0], 255);
    let mut input = bytes.as_slice();
    assert_eq!(
        read_security_types(&mut input, Version::V3_8).await?,
        vec![1; 255]
    );
    assert!(input.is_empty());
    Ok(())
}

#[tokio::test]
async fn readers_report_truncated_inputs() {
    for (_, bytes) in client_vectors() {
        for len in 0..bytes.len() {
            let mut input = &bytes[..len];
            assert_eq!(
                read_client_msg(&mut input).await,
                Err(VncError::Closed("eof".into())),
                "client message type {} truncated at {len}",
                bytes[0]
            );
        }
    }
    let bytes = encode_server_init(&studio());
    for len in 0..bytes.len() {
        let mut input = &bytes[..len];
        assert_eq!(
            read_server_init(&mut input).await,
            Err(VncError::Closed("eof".into()))
        );
    }
    let bytes = encode_security_result(false, Some("bad"));
    for len in 0..bytes.len() {
        let mut input = &bytes[..len];
        assert_eq!(
            read_security_result(&mut input, Version::V3_8).await,
            Err(VncError::Closed("eof".into()))
        );
    }
    for bytes in [
        encode_fb_update_header(3).to_vec(),
        vec![1, 0, 0, 0, 0, 2],
        encode_bell().to_vec(),
        encode_server_cut_text(b"ab")[..8].to_vec(),
    ] {
        for len in 0..bytes.len() {
            let mut input = &bytes[..len];
            assert_eq!(
                read_server_msg_header(&mut input).await,
                Err(VncError::Closed("eof".into()))
            );
        }
    }
    let bytes = encode_rect_header(&RectHeader {
        rect: Rect::new(1, 2, 3, 4),
        encoding: -239,
    });
    for len in 0..bytes.len() {
        let mut input = &bytes[..len];
        assert_eq!(
            read_rect_header(&mut input).await,
            Err(VncError::Closed("eof".into()))
        );
    }
    for (bytes, version) in [
        (vec![2, 1, 30], Version::V3_8),
        (vec![2, 1, 30], Version::V3_7),
        (vec![0, 0, 0, 30], Version::V3_3),
        (vec![0, 0, 0, 0, 0, 0, 0, 2, b'n', b'o'], Version::V3_3),
    ] {
        for len in 0..bytes.len() {
            let mut input = &bytes[..len];
            assert_eq!(
                read_security_types(&mut input, version).await,
                Err(VncError::Closed("eof".into()))
            );
        }
    }
}

#[test]
fn latin1() {
    assert_eq!(to_latin1("café"), Some(vec![0x63, 0x61, 0x66, 0xe9]));
    assert_eq!(to_latin1("a\r\nb"), Some(vec![0x61, 0x0a, 0x62]));
    assert_eq!(to_latin1("中"), None);
    assert_eq!(from_latin1(&[0xe9]), "é");
    assert_eq!(to_latin1("\u{100}"), None);
    assert_eq!(to_latin1("😀"), None);
    assert_eq!(to_latin1("\r\r\n\r"), Some(vec![b'\r', b'\n', b'\r']));
    assert_eq!(to_latin1(""), Some(Vec::new()));
    assert_eq!(from_latin1(&[]), "");

    let bytes: Vec<u8> = (0..=u8::MAX).collect();
    let text = from_latin1(&bytes);
    assert_eq!(text.chars().count(), 256);
    assert_eq!(to_latin1(&text), Some(bytes));
}
