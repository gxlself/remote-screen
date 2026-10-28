use std::time::Duration;

use flate2::{Decompress, FlushDecompress};
use rs_vnc::{
    auth::{
        ard::{ard_response, read_ard_params},
        vnc::vnc_auth_response,
    },
    proto::{self, ClientMsg, ServerMsgHeader, Version},
    testkit::{
        listen_tcp, pair, ArdTestParams, AuthCheck, FakeHandle, SecurityPlan, ServerScript, Step,
        TestRect,
    },
    types::{encoding, security, PixelFormat, Rect, VncError},
    Screen,
};
use tokio::{
    io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpStream,
};

const WAIT: Duration = Duration::from_secs(5);

async fn wait<F: std::future::Future>(future: F) -> F::Output {
    tokio::time::timeout(WAIT, future)
        .await
        .expect("test timed out")
}

async fn read_banner<S: AsyncRead + Unpin>(stream: &mut S) -> Version {
    let mut banner = [0u8; 12];
    wait(stream.read_exact(&mut banner)).await.unwrap();
    proto::parse_server_version(&banner).unwrap()
}

async fn handshake<S: AsyncRead + AsyncWrite + Unpin>(
    stream: &mut S,
    choice: Option<u8>,
    shared: bool,
) -> proto::ServerInit {
    let version = read_banner(stream).await;
    wait(stream.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    let offered = wait(proto::read_security_types(stream, version))
        .await
        .unwrap();
    if version != Version::V3_3 {
        wait(stream.write_all(&[choice.unwrap_or(offered[0])]))
            .await
            .unwrap();
    } else if let Some(expected) = choice {
        assert_eq!(offered, vec![expected]);
    }
    if proto::needs_security_result(version, offered[0]) {
        wait(proto::read_security_result(stream, version))
            .await
            .unwrap();
    }
    wait(stream.write_all(&[u8::from(shared)])).await.unwrap();
    wait(proto::read_server_init(stream)).await.unwrap()
}

async fn none_pair(
    script: ServerScript,
) -> (tokio::io::DuplexStream, FakeHandle, proto::ServerInit) {
    let (mut client, handle) = pair(script);
    let init = handshake(&mut client, Some(security::NONE), true).await;
    (client, handle, init)
}

async fn send_message<S: AsyncWrite + Unpin>(stream: &mut S, message: ClientMsg) {
    wait(stream.write_all(&message.encode().unwrap()))
        .await
        .unwrap();
}

#[tokio::test]
async fn none_38() {
    let (client, handle, init) = none_pair(ServerScript::new(800, 600)).await;
    assert_eq!(init.width, 800);
    assert_eq!(init.height, 600);
    assert_eq!(init.pixel_format, PixelFormat::BGRX32);
    assert_eq!(init.name, "fake");
    let log = handle.log();
    assert_eq!(log.shared, Some(true));
    assert_eq!(log.chosen_security, Some(1));
    drop(client);
    assert!(handle.finished().await.closed);
}

#[tokio::test]
async fn apple_version() {
    let mut script = ServerScript::new(10, 10);
    script.version = *b"RFB 003.889\n";
    let (mut client, handle) = pair(script);
    let version = read_banner(&mut client).await;
    assert_eq!(version, Version::V3_8);
    wait(client.write_all(b"RFB 003.008\n")).await.unwrap();
    let _ = wait(proto::read_security_types(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[1])).await.unwrap();
    wait(proto::read_security_result(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[1])).await.unwrap();
    let _ = wait(proto::read_server_init(&mut client)).await.unwrap();
    assert_eq!(handle.log().client_version, Some(*b"RFB 003.008\n"));
    drop(client);
    handle.finished().await;
}

#[tokio::test]
async fn vnc_auth() {
    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Offer {
        types: vec![security::VNC_AUTH],
        check: AuthCheck::VncPassword("secret".to_owned()),
    };
    let (mut client, handle) = pair(script.clone());
    let version = read_banner(&mut client).await;
    wait(client.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    assert_eq!(
        wait(proto::read_security_types(&mut client, version))
            .await
            .unwrap(),
        vec![2]
    );
    wait(client.write_all(&[2])).await.unwrap();
    let mut challenge = [0; 16];
    wait(client.read_exact(&mut challenge)).await.unwrap();
    wait(client.write_all(&vnc_auth_response("secret", &challenge)))
        .await
        .unwrap();
    wait(proto::read_security_result(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[1])).await.unwrap();
    let _ = wait(proto::read_server_init(&mut client)).await.unwrap();
    assert_eq!(handle.log().auth_ok, Some(true));
    drop(client);
    handle.finished().await;

    let (mut client, handle) = pair(script);
    let version = read_banner(&mut client).await;
    wait(client.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    let _ = wait(proto::read_security_types(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[2])).await.unwrap();
    wait(client.read_exact(&mut challenge)).await.unwrap();
    wait(client.write_all(&vnc_auth_response("wrong", &challenge)))
        .await
        .unwrap();
    let error = wait(proto::read_security_result(&mut client, version))
        .await
        .unwrap_err();
    assert_eq!(
        error,
        VncError::AuthFailed("Authentication failed".to_owned())
    );
    let mut byte = [0u8; 1];
    assert!(wait(client.read_exact(&mut byte)).await.is_err());
    assert_eq!(handle.finished().await.auth_ok, Some(false));
}

#[tokio::test]
async fn ard_ok_and_wrong() {
    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Offer {
        types: vec![security::ARD],
        check: AuthCheck::Ard {
            user: "alice".to_owned(),
            password: "s3cret!".to_owned(),
            params: ArdTestParams::small(),
        },
    };
    let (mut client, handle) = pair(script.clone());
    let version = read_banner(&mut client).await;
    wait(client.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    wait(proto::read_security_types(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[30])).await.unwrap();
    let params = wait(read_ard_params(&mut client)).await.unwrap();
    let response = ard_response(&params, "alice", "s3cret!").unwrap();
    wait(client.write_all(&response)).await.unwrap();
    wait(proto::read_security_result(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[1])).await.unwrap();
    let _ = wait(proto::read_server_init(&mut client)).await.unwrap();
    let log = handle.log();
    assert_eq!(
        log.ard_credentials,
        Some(("alice".to_owned(), "s3cret!".to_owned()))
    );
    assert_eq!(log.auth_ok, Some(true));
    drop(client);
    handle.finished().await;

    let (mut client, handle) = pair(script);
    let version = read_banner(&mut client).await;
    wait(client.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    wait(proto::read_security_types(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[30])).await.unwrap();
    let params = wait(read_ard_params(&mut client)).await.unwrap();
    wait(client.write_all(&ard_response(&params, "alice", "nope").unwrap()))
        .await
        .unwrap();
    assert!(wait(proto::read_security_result(&mut client, version))
        .await
        .is_err());
    assert_eq!(handle.finished().await.auth_ok, Some(false));
}

#[tokio::test]
async fn ard_oakley2() {
    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Offer {
        types: vec![security::ARD],
        check: AuthCheck::Ard {
            user: "alice".to_owned(),
            password: "s3cret!".to_owned(),
            params: ArdTestParams::oakley2(),
        },
    };
    let (mut client, handle) = pair(script);
    let version = read_banner(&mut client).await;
    wait(client.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    wait(proto::read_security_types(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[30])).await.unwrap();
    let params = wait(read_ard_params(&mut client)).await.unwrap();
    assert_eq!(params.key_len, 128);
    wait(client.write_all(&ard_response(&params, "alice", "s3cret!").unwrap()))
        .await
        .unwrap();
    wait(proto::read_security_result(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[1])).await.unwrap();
    let _ = wait(proto::read_server_init(&mut client)).await.unwrap();
    assert_eq!(handle.log().auth_ok, Some(true));
    drop(client);
    handle.finished().await;
}

#[tokio::test]
async fn refuse_and_v33_none() {
    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Refuse("busy".to_owned());
    let (mut client, handle) = pair(script.clone());
    let version = read_banner(&mut client).await;
    wait(client.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    assert_eq!(
        wait(proto::read_security_types(&mut client, version))
            .await
            .unwrap_err(),
        VncError::Refused("busy".to_owned())
    );
    handle.finished().await;
    drop(client);

    let (mut client, handle) = pair(script);
    let _ = read_banner(&mut client).await;
    wait(client.write_all(b"RFB 003.003\n")).await.unwrap();
    assert_eq!(
        wait(proto::read_security_types(&mut client, Version::V3_3))
            .await
            .unwrap_err(),
        VncError::Refused("busy".to_owned())
    );
    handle.finished().await;

    let mut script = ServerScript::new(2, 2);
    script.version = *b"RFB 003.008\n";
    script.security = SecurityPlan::Offer {
        types: vec![1],
        check: AuthCheck::Any,
    };
    let (client, handle, _) = none_pair(script).await;
    drop(client);
    handle.finished().await;

    let mut script = ServerScript::new(2, 2);
    script.version = *b"RFB 003.003\n";
    let (mut client, handle) = pair(script);
    let _ = read_banner(&mut client).await;
    wait(client.write_all(b"RFB 003.003\n")).await.unwrap();
    assert_eq!(
        wait(proto::read_security_types(&mut client, Version::V3_3))
            .await
            .unwrap(),
        vec![1]
    );
    wait(client.write_all(&[1])).await.unwrap();
    let _ = wait(proto::read_server_init(&mut client)).await.unwrap();
    assert_eq!(handle.log().auth_ok, Some(true));
    drop(client);
    handle.finished().await;
}

#[tokio::test]
async fn bad_choice() {
    let mut script = ServerScript::new(2, 2);
    script.security = SecurityPlan::Offer {
        types: vec![2],
        check: AuthCheck::Any,
    };
    let (mut client, handle) = pair(script);
    let version = read_banner(&mut client).await;
    wait(client.write_all(&proto::client_version_bytes(version)))
        .await
        .unwrap();
    wait(proto::read_security_types(&mut client, version))
        .await
        .unwrap();
    wait(client.write_all(&[1])).await.unwrap();
    let mut byte = [0u8; 1];
    assert!(wait(client.read_exact(&mut byte)).await.is_err());
    let log = handle.finished().await;
    assert!(log.error.unwrap_or_default().contains("client chose 1"));
}

#[tokio::test]
async fn fbur_raw_and_rgb565() {
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::Solid {
            rect: Rect::new(0, 0, 2, 2),
            bgra: [1, 2, 3, 255],
        }])],
        ..ServerScript::new(2, 2)
    };
    let (mut client, handle, _) = none_pair(script).await;
    send_message(&mut client, ClientMsg::SetPixelFormat(PixelFormat::BGRX32)).await;
    send_message(&mut client, ClientMsg::SetEncodings(vec![encoding::RAW])).await;
    send_message(
        &mut client,
        ClientMsg::FramebufferUpdateRequest {
            incremental: false,
            rect: Rect::new(0, 0, 2, 2),
        },
    )
    .await;
    assert_eq!(
        wait(proto::read_server_msg_header(&mut client))
            .await
            .unwrap(),
        ServerMsgHeader::FramebufferUpdate { rects: 1 }
    );
    assert_eq!(
        wait(proto::read_rect_header(&mut client))
            .await
            .unwrap()
            .encoding,
        0
    );
    let mut raw = [0u8; 16];
    wait(client.read_exact(&mut raw)).await.unwrap();
    assert_eq!(raw, [1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0, 1, 2, 3, 0]);
    assert_eq!(handle.log().msgs.len(), 3);
    drop(client);
    handle.finished().await;

    let mut pf = PixelFormat::BGRX32;
    pf.bits_per_pixel = 16;
    pf.depth = 16;
    pf.red_max = 31;
    pf.green_max = 63;
    pf.blue_max = 31;
    pf.red_shift = 11;
    pf.green_shift = 5;
    pf.blue_shift = 0;
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::Solid {
            rect: Rect::new(0, 0, 1, 1),
            bgra: [0, 0, 255, 255],
        }])],
        ..ServerScript::new(1, 1)
    };
    let (mut client, handle, _) = none_pair(script).await;
    send_message(&mut client, ClientMsg::SetPixelFormat(pf)).await;
    send_message(
        &mut client,
        ClientMsg::FramebufferUpdateRequest {
            incremental: false,
            rect: Rect::new(0, 0, 1, 1),
        },
    )
    .await;
    let _ = wait(proto::read_server_msg_header(&mut client))
        .await
        .unwrap();
    let _ = wait(proto::read_rect_header(&mut client)).await.unwrap();
    let mut pixel = [0u8; 2];
    wait(client.read_exact(&mut pixel)).await.unwrap();
    assert_eq!(pixel, [0, 0xf8]);
    drop(client);
    handle.finished().await;
}

#[tokio::test]
async fn pending_request_zrle() {
    let script = ServerScript::new(65, 1);
    let (mut client, handle, _) = none_pair(script).await;
    send_message(
        &mut client,
        ClientMsg::FramebufferUpdateRequest {
            incremental: false,
            rect: Rect::new(0, 0, 65, 1),
        },
    )
    .await;
    handle.push(Step::Update(vec![TestRect::Solid {
        rect: Rect::new(0, 0, 2, 1),
        bgra: [4, 5, 6, 255],
    }]));
    assert_eq!(
        wait(proto::read_server_msg_header(&mut client))
            .await
            .unwrap(),
        ServerMsgHeader::FramebufferUpdate { rects: 1 }
    );
    let _ = wait(proto::read_rect_header(&mut client)).await.unwrap();
    let mut bytes = [0u8; 8];
    wait(client.read_exact(&mut bytes)).await.unwrap();
    assert_eq!(&bytes[..4], &[4, 5, 6, 0]);
    drop(client);
    handle.finished().await;

    let bgra = [9, 8, 7, 255].repeat(65);
    let script = ServerScript {
        steps: vec![
            Step::Update(vec![TestRect::Zrle {
                rect: Rect::new(0, 0, 65, 1),
                bgra: bgra.clone(),
            }]),
            Step::Update(vec![TestRect::Zrle {
                rect: Rect::new(0, 0, 65, 1),
                bgra,
            }]),
        ],
        ..ServerScript::new(65, 1)
    };
    let (mut client, handle, _) = none_pair(script).await;
    let mut decompress = Decompress::new(true);
    for _ in 0..2 {
        send_message(
            &mut client,
            ClientMsg::FramebufferUpdateRequest {
                incremental: false,
                rect: Rect::new(0, 0, 65, 1),
            },
        )
        .await;
        let _ = wait(proto::read_server_msg_header(&mut client))
            .await
            .unwrap();
        let _ = wait(proto::read_rect_header(&mut client)).await.unwrap();
        let len = wait(client.read_u32()).await.unwrap() as usize;
        let mut compressed = vec![0; len];
        wait(client.read_exact(&mut compressed)).await.unwrap();
        let mut plain = Vec::with_capacity(128);
        decompress
            .decompress_vec(&compressed, &mut plain, FlushDecompress::Sync)
            .unwrap();
        assert_eq!(plain, vec![1, 9, 8, 7, 1, 9, 8, 7]);
    }
    drop(client);
    handle.finished().await;
}

#[tokio::test]
async fn pseudo_rects_and_immediate_steps() {
    let script = ServerScript {
        steps: vec![Step::Update(vec![
            TestRect::Cursor {
                hot_x: 1,
                hot_y: 0,
                w: 2,
                h: 1,
                bgra: vec![1, 2, 3, 255, 0, 0, 0, 0],
            },
            TestRect::PointerPos { x: 3, y: 4 },
            TestRect::DesktopSize { w: 1024, h: 768 },
            TestRect::LastRect,
        ])],
        ..ServerScript::new(2, 2)
    };
    let (mut client, handle, _) = none_pair(script).await;
    send_message(
        &mut client,
        ClientMsg::FramebufferUpdateRequest {
            incremental: false,
            rect: Rect::new(0, 0, 2, 2),
        },
    )
    .await;
    assert_eq!(
        wait(proto::read_server_msg_header(&mut client))
            .await
            .unwrap(),
        ServerMsgHeader::FramebufferUpdate { rects: 0xffff }
    );
    let cursor = wait(proto::read_rect_header(&mut client)).await.unwrap();
    assert_eq!(cursor.rect, Rect::new(1, 0, 2, 1));
    assert_eq!(cursor.encoding, encoding::CURSOR);
    let mut cursor_body = [0u8; 9];
    wait(client.read_exact(&mut cursor_body)).await.unwrap();
    assert_eq!(cursor_body, [1, 2, 3, 0, 0, 0, 0, 0, 0b1000_0000]);
    assert_eq!(
        wait(proto::read_rect_header(&mut client))
            .await
            .unwrap()
            .encoding,
        encoding::POINTER_POS
    );
    assert_eq!(
        wait(proto::read_rect_header(&mut client))
            .await
            .unwrap()
            .rect,
        Rect::new(0, 0, 1024, 768)
    );
    assert_eq!(
        wait(proto::read_rect_header(&mut client))
            .await
            .unwrap()
            .encoding,
        encoding::LAST_RECT
    );
    drop(client);
    handle.finished().await;

    let script = ServerScript {
        steps: vec![Step::Bell, Step::CutText(b"hi".to_vec()), Step::Close],
        ..ServerScript::new(2, 2)
    };
    let (mut client, handle, _) = none_pair(script).await;
    assert_eq!(wait(client.read_u8()).await.unwrap(), 2);
    let mut cut = [0u8; 10];
    wait(client.read_exact(&mut cut[..8])).await.unwrap();
    assert_eq!(&cut[..8], &[3, 0, 0, 0, 0, 0, 0, 2]);
    wait(client.read_exact(&mut cut[..2])).await.unwrap();
    assert_eq!(&cut[..2], b"hi");
    let mut eof = [0u8; 1];
    assert!(wait(client.read_exact(&mut eof)).await.is_err());
    assert!(handle.finished().await.closed);
}

#[tokio::test]
async fn extended_desktop_size_rect() {
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::ExtendedDesktopSize {
            reason: 1,
            status: 3,
            w: 300,
            h: 200,
            screens: vec![Screen {
                id: 0x0a0b_0c0d,
                rect: Rect::new(1, 2, 3, 4),
                flags: 5,
            }],
        }])],
        ..ServerScript::new(2, 2)
    };
    let (mut client, handle, _) = none_pair(script).await;
    send_message(
        &mut client,
        ClientMsg::FramebufferUpdateRequest {
            incremental: false,
            rect: Rect::new(0, 0, 2, 2),
        },
    )
    .await;
    assert_eq!(
        wait(proto::read_server_msg_header(&mut client))
            .await
            .unwrap(),
        ServerMsgHeader::FramebufferUpdate { rects: 1 }
    );
    let header = wait(proto::read_rect_header(&mut client)).await.unwrap();
    assert_eq!(header.rect, Rect::new(1, 3, 300, 200));
    assert_eq!(header.encoding, encoding::EXTENDED_DESKTOP_SIZE);
    let mut body = [0u8; 20];
    wait(client.read_exact(&mut body)).await.unwrap();
    assert_eq!(
        body,
        [1, 0, 0, 0, 0x0a, 0x0b, 0x0c, 0x0d, 0, 1, 0, 2, 0, 3, 0, 4, 0, 0, 0, 5]
    );
    drop(client);
    handle.finished().await;
}

#[tokio::test]
async fn records_input_and_listen_tcp_scripts() {
    let (mut client, handle, _) = none_pair(ServerScript::new(2, 2)).await;
    send_message(
        &mut client,
        ClientMsg::KeyEvent {
            down: true,
            keysym: 0x61,
        },
    )
    .await;
    send_message(
        &mut client,
        ClientMsg::PointerEvent {
            mask: 1,
            x: 2,
            y: 3,
        },
    )
    .await;
    send_message(&mut client, ClientMsg::ClientCutText(b"x".to_vec())).await;
    assert!(handle.wait_for(|log| log.msgs.len() == 3, WAIT).await);
    drop(client);
    assert!(handle.finished().await.closed);

    let s0 = ServerScript {
        name: "a".to_owned(),
        ..ServerScript::new(2, 2)
    };
    let s1 = ServerScript {
        name: "b".to_owned(),
        ..ServerScript::new(2, 2)
    };
    let (address, mut handles) = listen_tcp(vec![s0, s1]).await.unwrap();
    for expected in ["a", "b", "b"] {
        let mut client = TcpStream::connect(address).await.unwrap();
        let init = handshake(&mut client, Some(1), true).await;
        assert_eq!(init.name, expected);
        drop(client);
        let handle = wait(handles.recv()).await.unwrap();
        assert!(handle.finished().await.closed);
    }
}
