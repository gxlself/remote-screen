use std::sync::Arc;
use std::time::Duration;

use data_encoding::BASE64;
use host_core::platform::{ClipboardItem, CursorUpdate, DisplayInfo};
use host_fake::FakePlatform;
use host_win::pipe_proto::{
    input_to_b64, BlobKind, BlobPacket, Ctl, PipeFrame, WatchKind, WireError,
};
use host_win::session_server::SessionServer;
use rsp_core::hid::{InputMsg, Mods};
use rsp_core::pb;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

fn display(id: u32, primary: bool) -> DisplayInfo {
    DisplayInfo {
        id,
        name: format!("Display {id}"),
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        x: if id == 1 { 0 } else { 1920 },
        y: 0,
        refresh_hz: 60,
        primary,
        rotation: 0,
    }
}

async fn recv_frame(rx: &mut mpsc::UnboundedReceiver<PipeFrame>) -> PipeFrame {
    tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("session server output timeout")
        .expect("session server output closed")
}

async fn recv_ctl(rx: &mut mpsc::UnboundedReceiver<PipeFrame>) -> Ctl {
    match recv_frame(rx).await {
        PipeFrame::Ctl(ctl) => ctl,
        other => panic!("expected control frame, got {other:?}"),
    }
}

#[tokio::test]
async fn displays_inject_actions_and_clipboard() {
    let d1 = display(1, true);
    let d2 = display(2, false);
    let fake = FakePlatform::with_displays(vec![d1.clone(), d2.clone()]);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut server = SessionServer::new(Arc::new(fake.clone()), out_tx);

    server.start();
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Displays {
            list: vec![(&d1).into(), (&d2).into()]
        }
    );
    fake.set_displays(vec![d1.clone()]);
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Displays {
            list: vec![(&d1).into()]
        }
    );

    let key = InputMsg::Key {
        usage: 0x04,
        down: true,
        mods: Mods::from_bits(0),
    };
    server.handle(PipeFrame::Ctl(Ctl::Inject {
        display_id: 2,
        input: input_to_b64(&key).expect("test input encodes"),
    }));
    assert_eq!(fake.injected_with_display().last(), Some(&(key.clone(), 1)));
    server.handle(PipeFrame::Ctl(Ctl::Inject {
        display_id: 9,
        input: input_to_b64(&key).expect("test input encodes"),
    }));
    assert_eq!(fake.injected_with_display().last(), Some(&(key.clone(), 1)));
    server.handle(PipeFrame::Ctl(Ctl::Inject {
        display_id: 1,
        input: "!!".into(),
    }));
    assert_eq!(fake.injected_with_display().len(), 2);

    server.handle(PipeFrame::Ctl(Ctl::Action {
        req: 1,
        kind: pb::session_action::Kind::LockScreen as i32,
    }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 1,
            error: None
        }
    );
    assert_eq!(
        fake.session_actions(),
        vec![pb::session_action::Kind::LockScreen]
    );
    fake.fail_session_actions(true);
    server.handle(PipeFrame::Ctl(Ctl::Action {
        req: 2,
        kind: pb::session_action::Kind::LockScreen as i32,
    }));
    assert!(matches!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 2,
            error: Some(_)
        }
    ));
    server.handle(PipeFrame::Ctl(Ctl::Action { req: 3, kind: 99 }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 3,
            error: Some(WireError::Unsupported)
        }
    );

    let text = b"hi".to_vec();
    let mut text_sha = [0_u8; 32];
    text_sha.copy_from_slice(&Sha256::digest(&text));
    fake.set_clipboard(Some(ClipboardItem {
        kind: pb::clipboard_header::Kind::Text,
        data: text.clone(),
        sha256: text_sha,
    }));
    server.handle(PipeFrame::Ctl(Ctl::ClipboardGet { req: 4 }));
    assert_eq!(
        recv_frame(&mut out_rx).await,
        PipeFrame::Blob(BlobPacket {
            req: 4,
            kind: BlobKind::Text,
            data: text,
        })
    );
    fake.set_clipboard(None);
    server.handle(PipeFrame::Ctl(Ctl::ClipboardGet { req: 5 }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 5,
            error: None
        }
    );

    server.handle(PipeFrame::Blob(BlobPacket {
        req: 6,
        kind: BlobKind::Png,
        data: vec![1, 2, 3],
    }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 6,
            error: None
        }
    );
    let writes = fake.clipboard_writes();
    assert_eq!(
        writes.last().map(|item| item.kind),
        Some(pb::clipboard_header::Kind::Png)
    );
    assert_eq!(
        writes.last().map(|item| item.data.as_slice()),
        Some([1, 2, 3].as_slice())
    );
    assert_eq!(
        writes.last().map(|item| item.sha256),
        Some(Sha256::digest([1, 2, 3]).into())
    );
}

#[tokio::test]
async fn watches_capture_encoder_audio_and_shutdown() {
    let d1 = display(1, true);
    let fake = FakePlatform::with_displays(vec![d1]);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let fake_ref = Arc::new(fake.clone());
    let mut server = SessionServer::new(fake_ref, out_tx);
    server.start();
    let _ = recv_ctl(&mut out_rx).await;

    server.handle(PipeFrame::Ctl(Ctl::Watch {
        what: WatchKind::Cursor,
    }));
    assert!(matches!(
        recv_ctl(&mut out_rx).await,
        Ctl::CursorPos { display_id: 1, .. }
    ));
    let update = CursorUpdate::Position {
        display_id: 1,
        seq: 4,
        x: 0.25,
        y: 0.75,
        visible: true,
    };
    assert_eq!(fake.push_cursor(update.clone()), 1);
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::CursorPos {
            display_id: 1,
            seq: 4,
            x: 0.25,
            y: 0.75,
            visible: true,
        }
    );
    server.handle(PipeFrame::Ctl(Ctl::Unwatch {
        what: WatchKind::Cursor,
    }));
    assert_eq!(fake.push_cursor(update.clone()), 1);
    assert!(tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .is_err());
    server.handle(PipeFrame::Ctl(Ctl::Watch {
        what: WatchKind::Cursor,
    }));
    assert_eq!(fake.push_cursor(update), 1);
    assert!(matches!(
        recv_ctl(&mut out_rx).await,
        Ctl::CursorPos { seq: 4, .. }
    ));

    server.handle(PipeFrame::Ctl(Ctl::CaptureStart {
        req: 7,
        cap: 1,
        display: 1,
        fps: 30,
        max_width: 0,
        max_height: 0,
        region: None,
    }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 7,
            error: None
        }
    );
    let frame = recv_ctl(&mut out_rx).await;
    assert!(matches!(frame, Ctl::Frame { cap: 1, seq: 1, .. }));
    server.handle(PipeFrame::Ctl(Ctl::EncoderOpen {
        req: 8,
        enc: 1,
        codec: pb::Codec::Hevc as i32,
        width: 1920,
        height: 1080,
        fps: 30,
        bitrate_kbps: 4_000,
    }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::EncoderInfo { enc: 1, roi: false }
    );
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 8,
            error: None
        }
    );
    server.handle(PipeFrame::Ctl(Ctl::Encode {
        enc: 1,
        cap: 1,
        seq: 1,
        force_keyframe: true,
        quality_boost: false,
    }));
    assert!(matches!(
        recv_frame(&mut out_rx).await,
        PipeFrame::Video(packet) if packet.enc == 1 && packet.keyframe
    ));
    fake.set_capture_paused(true);
    server.handle(PipeFrame::Ctl(Ctl::Encode {
        enc: 1,
        cap: 1,
        seq: 999,
        force_keyframe: false,
        quality_boost: false,
    }));
    loop {
        match tokio::time::timeout(Duration::from_millis(50), out_rx.recv()).await {
            Err(_) => break,
            Ok(Some(PipeFrame::Ctl(Ctl::Frame { .. }))) => {}
            Ok(Some(other)) => panic!("unexpected output for stale encode: {other:?}"),
            Ok(None) => break,
        }
    }
    server.handle(PipeFrame::Ctl(Ctl::EncoderRate {
        enc: 1,
        kbps: 4_000,
        fps: 30,
    }));
    server.handle(PipeFrame::Ctl(Ctl::EncoderKeyframe { enc: 1 }));
    assert_eq!(fake.encoder_rates(), vec![(4_000, 30)]);
    assert_eq!(fake.forced_keyframes(), 1);
    server.handle(PipeFrame::Ctl(Ctl::CaptureStop { cap: 1 }));
    assert_eq!(fake.active_captures(), 0);

    server.handle(PipeFrame::Ctl(Ctl::AudioStart {
        req: 9,
        sample_rate: 48_000,
        channels: 2,
        exclude_self: true,
    }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 9,
            error: None
        }
    );
    let audio = loop {
        match recv_frame(&mut out_rx).await {
            PipeFrame::Audio(packet) => break packet,
            PipeFrame::Ctl(_) => continue,
            other => panic!("unexpected frame {other:?}"),
        }
    };
    assert_eq!(audio.channels, 2);
    assert_eq!(audio.samples.len(), audio.frames as usize * 2);
    assert_eq!(
        fake.audio().last_config(),
        Some(host_core::platform::AudioCaptureConfig {
            sample_rate: 48_000,
            channels: 2,
            exclude_self: true,
        })
    );
    server.handle(PipeFrame::Ctl(Ctl::AudioStop));
    assert_eq!(fake.audio().active(), 0);

    server.handle(PipeFrame::Ctl(Ctl::KeepAwake { on: true }));
    server.handle(PipeFrame::Ctl(Ctl::Ping { n: 7 }));
    assert_eq!(recv_ctl(&mut out_rx).await, Ctl::Pong { n: 7 });
    server.notify_desktop("Winlogon");
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Desktop {
            name: "Winlogon".into()
        }
    );
    let (input_tx, input_rx) = mpsc::unbounded_channel();
    let release_before = fake.release_count();
    let task = tokio::spawn(server.run(input_rx));
    drop(input_tx);
    task.await.expect("session server run task");
    assert_eq!(fake.keep_awake_calls(), vec![true, false]);
    assert_eq!(fake.release_count(), release_before + 1);
    assert_eq!(fake.active_captures(), 0);
}

#[tokio::test]
async fn unsupported_hevc_is_returned_verbatim() {
    let fake = FakePlatform::new();
    fake.set_hevc_unsupported(true);
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut server = SessionServer::new(Arc::new(fake), out_tx);
    server.handle(PipeFrame::Ctl(Ctl::EncoderOpen {
        req: 1,
        enc: 1,
        codec: pb::Codec::Hevc as i32,
        width: 1920,
        height: 1080,
        fps: 30,
        bitrate_kbps: 4_000,
    }));
    assert_eq!(
        recv_ctl(&mut out_rx).await,
        Ctl::Done {
            req: 1,
            error: Some(WireError::Unsupported)
        }
    );
}

#[test]
fn input_base64_helper_uses_wire_encoding() {
    let msg = InputMsg::Key {
        usage: 0x04,
        down: true,
        mods: Mods::from_bits(0),
    };
    assert!(!BASE64
        .decode(input_to_b64(&msg).expect("test input encodes").as_bytes())
        .expect("base64 output decodes")
        .is_empty());
}
