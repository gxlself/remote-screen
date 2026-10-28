use host_core::{
    auth::Role,
    file_send,
    platform::{CursorUpdate, Platform},
    session::manager::EndReason,
};
use rsp_core::{
    features,
    file::{build_header, encode_stream_prefix},
    hid::{InputMsg, Mods},
    pb,
    pointer::PointerDatagram,
    video::{self, FrameMeta, VideoHeader},
};
use rsp_transport::{CloseCode, DatagramKind, StreamKind};
use std::time::Duration;
use tokio::time::{sleep, timeout};

mod wire_support;
use wire_support::{client_hello, device_id, Dev, WireHost, WireOptions, WireTcpClient};

fn start() -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        quality: pb::Quality::Auto as i32,
        ..Default::default()
    }
}

async fn send_input(client: &WireTcpClient, msg: &InputMsg) -> rsp_transport::UniSend {
    let mut input = client.conn.open_uni(StreamKind::Input).await.unwrap();
    input.send_record(&msg.encode().unwrap()).await.unwrap();
    input
}

#[tokio::test]
async fn tcp_handshake_and_sixty_whole_frames() {
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    let (server, auth) = client.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.session_token.len(), 16);
    assert_ne!(server.features & features::TCP_TRANSPORT, 0);
    host.wait_until(|| host.conns().len() == 1).await;
    let config = client.start_stream(start()).await;
    assert_eq!(config.bitrate_kbps, 4_000);
    assert!(config.fps <= 30);
    assert!(config.width <= 1920 && config.height <= 1080);
    let mut previous = None;
    for index in 0..60 {
        let packet = client.video().await;
        let (header, payload) = VideoHeader::parse(&packet).unwrap();
        assert_eq!(
            (header.data_shards, header.parity_shards, header.shard_len),
            (1, 0, 0)
        );
        assert_eq!(header.epoch, config.epoch as u8);
        assert_eq!(header.frame_len as usize, payload.len());
        if index == 0 {
            assert!(header.is_keyframe());
        }
        if let Some(previous) = previous {
            assert!(header.frame_id > previous);
        }
        previous = Some(header.frame_id);
        // Comparing the full record also catches an extra 0x10 in the payload.
        let expected = video::packetize_tcp(
            payload,
            FrameMeta {
                flags: header.flags,
                display_id: header.display_id,
                epoch: header.epoch,
                frame_id: header.frame_id,
                capture_ts_us: header.capture_ts_us,
            },
        )
        .unwrap();
        assert_eq!(packet, expected[0]);
    }
    client.ping().await;
}

#[tokio::test]
async fn tcp_input_tokens_resume_and_close() {
    let host = WireHost::start_with(WireOptions {
        listeners: 2,
        ..Default::default()
    })
    .await;
    let client = host.connect_tcp(Dev::A, 0).await;
    let (_, auth) = client.hello().await;
    let config = client.start_stream(start()).await;
    let mouse = InputMsg::MouseMove { x: 0.25, y: 0.75 };
    let _input = send_input(&client, &mouse).await;
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    assert_eq!(host.platform.injected_msgs(), std::slice::from_ref(&mouse));
    host.reject_tcp_attach(Dev::A, 0, vec![0x55; 16]).await;
    host.reject_tcp_attach(Dev::B, 0, auth.session_token.clone())
        .await;
    client.ping().await;
    drop(client);
    host.wait_until(|| host.conns().is_empty()).await;
    host.reject_tcp_attach(Dev::A, 0, auth.session_token.clone())
        .await;
    let resumed = host.connect_tcp(Dev::A, 1).await;
    let (_, fresh) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token.clone(),
            ..client_hello()
        })
        .await;
    assert!(fresh.resumed);
    assert_ne!(fresh.session_token, auth.session_token);
    let restored = resumed
        .next_matching(|m| match m {
            pb::envelope::Msg::StreamConfig(c) => Some(*c),
            _ => None,
        })
        .await;
    assert_eq!(restored.epoch, config.epoch + 1);
    assert!(VideoHeader::parse(&resumed.video().await)
        .unwrap()
        .0
        .is_keyframe());
    host.reject_tcp_attach(Dev::A, 0, auth.session_token.clone())
        .await;
    host.reject_tcp_attach(Dev::A, 1, auth.session_token).await;
    let _new_input = send_input(&resumed, &mouse).await;
    host.wait_until(|| host.platform.injected_msgs().len() == 2)
        .await;
    let key = host.conns()[0].key;
    host.mgr.end(key, EndReason::LocalUser).await;
    let goodbye = resumed
        .next_matching(|m| match m {
            pb::envelope::Msg::Goodbye(g) => Some(g.clone()),
            _ => None,
        })
        .await;
    assert_eq!(
        goodbye.reason,
        pb::goodbye::Reason::HostClosedByLocalUser as i32
    );
    host.reject_tcp_attach(Dev::A, 1, fresh.session_token).await;
}

#[tokio::test]
async fn tcp_live_resume_rotates_token_and_drains_reason_text() {
    let host = WireHost::start().await;
    let old = host.connect_tcp(Dev::A, 0).await;
    let (_, auth) = old.hello().await;
    let mut input = send_input(
        &old,
        &InputMsg::Key {
            usage: 4,
            down: true,
            mods: Mods::NONE,
        },
    )
    .await;
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    let new = host.connect_tcp(Dev::A, 0).await;
    assert_ne!(old.conn.stable_id(), new.conn.stable_id());
    assert_eq!(old.conn.stable_id(), old.conn.clone().stable_id());
    let (_, fresh) = new
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token.clone(),
            ..client_hello()
        })
        .await;
    assert!(fresh.resumed);
    let goodbye = old
        .next_matching(|m| match m {
            pb::envelope::Msg::Goodbye(g) => Some(g.clone()),
            _ => None,
        })
        .await;
    assert_eq!(goodbye.reason, pb::goodbye::Reason::Replaced as i32);
    assert_eq!(goodbye.message, "replaced by a new connection");
    host.reject_tcp_attach(Dev::A, 0, auth.session_token).await;
    let before = host.platform.injected_msgs().len();
    let _ = input
        .send_record(&InputMsg::MouseMove { x: 0.1, y: 0.1 }.encode().unwrap())
        .await;
    let _input = send_input(&new, &InputMsg::MouseMove { x: 0.8, y: 0.2 }).await;
    host.wait_until(|| host.platform.injected_msgs().len() == before + 1)
        .await;
    new.ping().await;
}

#[tokio::test]
async fn quic_controller_tcp_viewer_receives_roster_video_and_drops_input() {
    let host = WireHost::start().await;
    let mut controller = host.connect().await;
    controller.hello().await;
    let viewer = host.connect_tcp(Dev::B, 0).await;
    let (_, auth) = viewer
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    let state = viewer
        .next_matching(|m| match m {
            pb::envelope::Msg::SessionState(s) => Some(s.clone()),
            _ => None,
        })
        .await;
    assert_eq!(state.controller.unwrap().device_id, device_id(Dev::A));
    viewer.start_stream(start()).await;
    assert!(VideoHeader::parse(&viewer.video().await)
        .unwrap()
        .0
        .is_keyframe());
    let mut input = send_input(&viewer, &InputMsg::MouseMove { x: 0.2, y: 0.3 }).await;
    input
        .send_record(
            &InputMsg::Key {
                usage: 4,
                down: true,
                mods: Mods::NONE,
            }
            .encode()
            .unwrap(),
        )
        .await
        .unwrap();
    viewer
        .conn
        .send_datagram(
            DatagramKind::Pointer,
            PointerDatagram {
                seq: 1,
                display: 0,
                visible: true,
                x: 0.4,
                y: 0.6,
            }
            .encode()[1..]
                .to_vec()
                .into(),
        )
        .unwrap();
    let key = host
        .conns()
        .iter()
        .find(|c| c.role == Role::Viewer)
        .unwrap()
        .key;
    let counts = host.mgr.features().input.counters(key).unwrap();
    host.wait_until(|| counts.dropped.load(std::sync::atomic::Ordering::Relaxed) >= 3)
        .await;
    assert!(host.platform.injected_msgs().is_empty());
    viewer.ping().await;
}

#[tokio::test]
async fn tcp_rate_bounds_and_feedback_have_no_fec() {
    for requested in [0, 1, 5_000, 100_000] {
        let platform = host_fake::FakePlatform::new();
        let mut displays = platform.displays();
        displays[0].width_px = 3840;
        displays[0].height_px = 2160;
        platform.set_displays(displays);
        let host = WireHost::start_with(WireOptions {
            platform: Some(platform),
            ..Default::default()
        })
        .await;
        let client = host.connect_tcp(Dev::A, 0).await;
        client.hello().await;
        let config = client
            .start_stream(pb::StartStream {
                fps: 120,
                max_width: 4000,
                bitrate_kbps: requested,
                ..start()
            })
            .await;
        assert_eq!(
            config.bitrate_kbps,
            if requested == 0 {
                4000
            } else {
                requested.clamp(2000, 6000)
            }
        );
        assert!(config.fps <= 30 && config.height <= 1080);
        client
            .send(pb::Feedback {
                epoch: config.epoch,
                shards_received: 100,
                shards_lost: 25,
                interval_ms: 200,
                ..Default::default()
            })
            .await;
        let rate = client
            .next_matching(|m| match m {
                pb::envelope::Msg::RateUpdate(r) => Some(r.clone()),
                _ => None,
            })
            .await;
        assert_eq!(rate.fec_ratio_pct, 0);
        assert!((2000..=6000).contains(&rate.target_kbps));
        assert!(rate.fps <= 30);
        host.wait_until(|| !host.platform.encoder_rates().is_empty())
            .await;
        for (kbps, fps) in host.platform.encoder_rates() {
            assert!((2000..=6000).contains(&kbps));
            assert!(fps <= 30);
        }
    }
}

#[tokio::test]
async fn tcp_file_upload_succeeds_and_connection_keeps_pinging() {
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    client.ping().await;
    let body = b"TCP file upload with Ping/Pong during the transfer";
    let source = host.state_dir.join("tcp-file-source.bin");
    tokio::fs::write(&source, body).await.unwrap();
    let prepared = file_send::prepare(&source, Some("11111111111111111111111111111111".into()))
        .await
        .unwrap();
    let header = build_header(
        prepared.transfer_id,
        "tcp-file.bin",
        body.len() as u64,
        0,
        prepared.sha256,
        prepared.mtime_us,
    )
    .unwrap();
    let mut file = client.conn.open_uni(StreamKind::File).await.unwrap();
    file.write_raw(&encode_stream_prefix(&header).unwrap())
        .await
        .unwrap();
    let split = body.len() / 2;
    file.write_raw(&body[..split]).await.unwrap();
    // Ping/Pong must also work while this file remains unfinished.
    client.ping().await;
    file.write_raw(&body[split..]).await.unwrap();
    file.finish().await.unwrap();
    let status = client
        .next_matching(|message| match message {
            pb::envelope::Msg::FileStatus(status) if status.transfer_id == header.transfer_id => {
                Some(status.clone())
            }
            _ => None,
        })
        .await;
    assert_eq!(status.request_id, 0);
    assert_eq!(status.state, pb::file_status::State::Completed as i32);
    assert_eq!(status.error, pb::FileError::None as i32);
    assert_eq!(status.received, body.len() as u64);
    assert_eq!(status.saved_name, header.name);
    assert_eq!(
        tokio::fs::read(host.downloads.join(&status.saved_name))
            .await
            .unwrap(),
        body
    );
    client.ping().await;
}

#[tokio::test]
async fn tcp_cursor_records_and_bidirectional_pointer() {
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    let config = client.start_stream(start()).await;
    let shape = pb::CursorShape {
        id: 7,
        width: 16,
        height: 16,
        png: vec![1, 2, 3],
        ..Default::default()
    };
    let display_id = host.platform.displays()[0].id;
    host.platform
        .push_cursor(CursorUpdate::Shape(shape.clone()));
    let (kind, mut cursor) = timeout(Duration::from_secs(3), client.conn.accept_uni())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(kind, StreamKind::Cursor);
    timeout(Duration::from_secs(3), async {
        loop {
            let record = cursor.recv_record().await.unwrap().unwrap();
            // Decode exactly one CursorMsg from this TCP record.
            let mut decoder = rsp_core::cursor::CursorStreamDecoder::default();
            decoder.push(&(record.len() as u32).to_be_bytes());
            decoder.push(&record);
            if let Some(pb::cursor_msg::Msg::Shape(got)) =
                decoder.next_msg().unwrap().and_then(|m| m.msg)
            {
                assert_eq!(got.png, shape.png);
                break;
            }
        }
    })
    .await
    .unwrap();
    let pointer = PointerDatagram {
        seq: 1,
        display: config.display_id as u8,
        visible: true,
        x: 0.3,
        y: 0.7,
    };
    client
        .conn
        .send_datagram(DatagramKind::Pointer, pointer.encode()[1..].to_vec().into())
        .unwrap();
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    host.platform.push_cursor(CursorUpdate::Position {
        display_id,
        seq: 2,
        x: 0.6,
        y: 0.4,
        visible: true,
    });
    timeout(Duration::from_secs(3), async {
        loop {
            let (kind, _) = client.conn.recv_datagram().await.unwrap();
            if kind == DatagramKind::Pointer {
                break;
            }
        }
    })
    .await
    .unwrap();
}

#[tokio::test]
async fn tcp_video_queue_requests_encoder_keyframes_with_throttle() {
    // Exercise the real pipeline callback while keeping queue replacement
    // deterministic: each burst runs without yielding to the TCP writer.
    let platform = host_fake::FakePlatform::new();
    let host_id = rsp_transport::identity::LocalIdentity::generate_software(
        rsp_transport::identity::CertKind::Host,
    )
    .unwrap();
    let cfg = rsp_transport::tls::server_config(&host_id).unwrap();
    let listener = rsp_transport::TcpListener::bind("127.0.0.1:0".parse().unwrap(), cfg)
        .await
        .unwrap();
    let tls = rsp_transport::tls::client_config(
        wire_support::identity(Dev::A),
        rsp_transport::tls::ServerPin::Fingerprint(host_id.fingerprint()),
    )
    .unwrap();
    let ((client, _control), server) = tokio::join!(
        async {
            let client = rsp_transport::connect_tcp(listener.local_addr().unwrap(), tls.into())
                .await
                .unwrap();
            let control = client.open_control().await.unwrap();
            control.send(&[]).await.unwrap();
            (client, control)
        },
        async {
            listener
                .accept()
                .await
                .unwrap()
                .accept()
                .await
                .unwrap()
                .unwrap()
        },
    );
    let _pipeline = host_core::pipeline::VideoPipeline::start(
        &platform,
        &platform.displays()[0],
        0,
        server.clone(),
        1,
        640,
        1,
        4000,
        &[],
        false,
        None,
    )
    .unwrap();
    let burst = || {
        for _ in 0..20 {
            server
                .send_datagram(DatagramKind::Video, vec![0; 128].into())
                .unwrap();
        }
    };
    burst();
    assert_eq!(server.dropped_video_frames(), 19);
    timeout(Duration::from_secs(3), async {
        while platform.forced_keyframes() < 1 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    let first = platform.forced_keyframes();
    burst();
    sleep(Duration::from_millis(100)).await;
    assert_eq!(platform.forced_keyframes(), first, "500 ms throttle");
    sleep(Duration::from_millis(520)).await;
    burst();
    timeout(Duration::from_secs(3), async {
        while platform.forced_keyframes() <= first {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    eprintln!(
        "TCP dropped_frames={} encoder_forced_keyframes={}",
        server.dropped_video_frames(),
        platform.forced_keyframes()
    );
    assert_eq!(platform.forced_keyframes(), 2);
    server.close(CloseCode::Normal, b"done");
    client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn tcp_clipboard_chunks_round_trip_without_file_framing() {
    use rsp_core::clipboard::{ClipboardItem, ClipboardKind};
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    let text = vec![b'x'; 150_000];
    let upload = ClipboardItem::new(ClipboardKind::Text, text.clone()).unwrap();
    let mut stream = client.conn.open_uni(StreamKind::Clipboard).await.unwrap();
    host_core::session::clipboard::write_uni_stream(&mut stream, &upload)
        .await
        .unwrap();
    host.wait_until(|| {
        host.platform
            .clipboard_writes()
            .iter()
            .any(|i| i.data == text)
    })
    .await;
    // A distinct local item avoids the clipboard echo guard.
    let download = b"host clipboard over TCP".to_vec();
    host.platform
        .set_clipboard(Some(host_core::platform::ClipboardItem {
            kind: pb::clipboard_header::Kind::Text,
            sha256: host_core::session::clipboard::sha256(&download),
            data: download.clone(),
        }));
    let received = timeout(Duration::from_secs(3), async {
        loop {
            let (kind, mut recv) = client.conn.accept_uni().await.unwrap();
            assert_eq!(kind, StreamKind::Clipboard);
            let item = host_core::session::clipboard::read_uni_stream(&mut recv)
                .await
                .unwrap();
            if item.data.as_ref() == download {
                return item;
            }
        }
    })
    .await
    .unwrap();
    assert_eq!(received.kind, ClipboardKind::Text);
    client.ping().await;
}

#[tokio::test]
async fn tcp_token_can_resume_on_quic_then_back_to_tcp() {
    let host = WireHost::start().await;
    let tcp = host.connect_tcp(Dev::A, 0).await;
    let (_, first) = tcp.hello().await;
    tcp.start_stream(start()).await;
    let mut quic = host.connect().await;
    let (_, second) = quic
        .hello_with(pb::ClientHello {
            resume_token: first.session_token.clone(),
            ..client_hello()
        })
        .await;
    assert!(second.resumed);
    host.reject_tcp_attach(Dev::A, 0, first.session_token).await;
    let again = host.connect_tcp(Dev::A, 0).await;
    let (_, third) = again
        .hello_with(pb::ClientHello {
            resume_token: second.session_token,
            ..client_hello()
        })
        .await;
    assert!(third.resumed);
    let config = again
        .next_matching(|m| match m {
            pb::envelope::Msg::StreamConfig(c) => Some(*c),
            _ => None,
        })
        .await;
    assert_eq!(config.epoch, 3);
    let _input = send_input(&again, &InputMsg::MouseMove { x: 0.4, y: 0.5 }).await;
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    again.ping().await;
}

#[tokio::test]
async fn tcp_malformed_control_drains_error_before_close() {
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello().await;
    // Invalid protobuf wire type: same Protocol end reason as the QUIC path.
    client.control.send(&[0xff]).await.unwrap();
    let message = client
        .next_matching(|m| match m {
            pb::envelope::Msg::Error(e) => Some(e.message.clone()),
            _ => None,
        })
        .await;
    assert!(message.contains("invalid protobuf control frame"));
    timeout(Duration::from_secs(3), client.conn.closed())
        .await
        .unwrap();
    host.wait_until(|| host.conns().is_empty()).await;
}
