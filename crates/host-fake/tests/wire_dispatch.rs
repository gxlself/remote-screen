use async_trait::async_trait;
use host_core::{
    auth::Role,
    platform::Platform,
    session::handshake::{ConnectionApprovalRequest, ConnectionApprover},
};
use rsp_core::{
    hid::{InputMsg, Mods},
    input::InputEvent,
    pb,
    pointer::PointerDatagram,
    wire::frame,
};
use std::{sync::Arc, time::Duration};
use tokio::{sync::Notify, time::timeout};

mod wire_support;
use wire_support::{write_record, WireClient, WireHost, WireOptions};

async fn ping(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 123 }).await;
    let pong = client
        .next_matching(|m| match m {
            pb::envelope::Msg::Pong(p) => Some(*p),
            _ => None,
        })
        .await;
    assert_eq!(pong.t1_us, 123);
}

async fn stopped(stream: &mut quinn::SendStream, code: u32) {
    assert_eq!(
        timeout(Duration::from_secs(3), stream.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(code.into())
    );
    assert!(
        matches!(stream.write_all(&[1]).await, Err(quinn::WriteError::Stopped(c)) if c == code.into())
    );
}

#[tokio::test]
async fn typed_mouse_and_key_input_preserve_the_session() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut input = client.open_typed(0x01).await;
    let events = [
        InputMsg::MouseMove { x: 0.25, y: 0.75 },
        InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::NONE,
        },
    ];
    for event in &events {
        write_record(&mut input, &event.encode().unwrap()).await;
    }
    host.wait_until(|| host.platform.injected_msgs().len() == 2)
        .await;
    assert_eq!(host.platform.injected_msgs(), events);
    ping(&mut client).await;
}

#[tokio::test]
async fn legacy_input_keeps_its_first_length_byte() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut input = client.open_legacy_input().await;
    let event = InputEvent::MouseMove { x: 0.2, y: 0.8 };
    input.write_all(&frame(&event.encode())).await.unwrap();
    host.wait_until(|| !host.platform.injected_events().is_empty())
        .await;
    assert_eq!(host.platform.injected_events(), [event]);
    ping(&mut client).await;
}

#[tokio::test]
async fn unknown_and_client_cursor_streams_are_stopped_without_disconnecting() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    for kind in [0x7f, 0x02] {
        let mut stream = client.open_typed(kind).await;
        // These bytes can race STOP_SENDING; either outcome is allowed.
        let _ = stream.write_all(&[1, 2, 3]).await;
        stopped(&mut stream, 1).await;
        ping(&mut client).await;
    }
}

#[tokio::test]
async fn oversized_and_invalid_input_only_stop_the_offending_stream() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    for bad in [
        0x0020_0000u32.to_be_bytes().to_vec(),
        (host_core::session::handlers::input::MAX_INPUT_RECORD as u32 + 1)
            .to_be_bytes()
            .to_vec(),
        frame(&[0xff]),
    ] {
        let mut input = client.open_typed(0x01).await;
        input.write_all(&bad).await.unwrap();
        stopped(&mut input, 1).await;
    }
    let mut input = client.open_typed(0x01).await;
    let event = InputMsg::MouseMove { x: 0.3, y: 0.7 };
    write_record(&mut input, &event.encode().unwrap()).await;
    host.wait_until(|| !host.platform.injected_msgs().is_empty())
        .await;
    assert_eq!(host.platform.injected_msgs(), [event]);
    ping(&mut client).await;
}

#[tokio::test]
async fn view_only_host_discards_typed_input() {
    let mut options = WireOptions::default();
    options.options.view_only = true;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    let (_, auth) = client.hello().await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    let mut input = client.open_typed(0x01).await;
    write_record(
        &mut input,
        &InputMsg::MouseMove { x: 0.5, y: 0.5 }.encode().unwrap(),
    )
    .await;
    input.finish().unwrap();
    // Wait for delivery, then give the input reader time to discard it.
    assert_eq!(
        timeout(Duration::from_secs(3), input.stopped())
            .await
            .unwrap()
            .unwrap(),
        None
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(host.platform.injected_msgs().is_empty());
    assert_eq!(host.platform.release_count(), 0);
    ping(&mut client).await;
}

struct DelayedApproval(Notify);
#[async_trait]
impl ConnectionApprover for DelayedApproval {
    async fn request(&self, _: ConnectionApprovalRequest) -> bool {
        self.0.notified().await;
        true
    }
}

#[tokio::test]
async fn pending_complete_and_partial_typed_records_are_discarded() {
    let approver = Arc::new(DelayedApproval(Notify::new()));
    let mut options = WireOptions {
        connection_approver: Some(approver.clone()),
        ..Default::default()
    };
    options.options.auth_policy.attended_mode = true;
    let host = WireHost::start_with(options).await;
    let mut client = host.connect().await;
    let (_, pending) = client.hello().await;
    assert_eq!(
        pending.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let mut input = client.open_typed(0x01).await;
    let before = frame(&InputMsg::MouseMove { x: 0.1, y: 0.2 }.encode().unwrap());
    input.write_all(&before).await.unwrap();
    // Split inside the length prefix so even a partial header must retain
    // the authorization observed before admission.
    input.write_all(&before[..2]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(host.platform.injected_msgs().is_empty());
    approver.0.notify_one();
    client
        .next_matching(|m| match m {
            pb::envelope::Msg::AuthResult(a) => {
                assert_eq!(a.status, pb::auth_result::Status::Ok as i32);
                Some(())
            }
            _ => None,
        })
        .await;
    input.write_all(&before[2..]).await.unwrap();
    let after = InputMsg::MouseMove { x: 0.3, y: 0.4 };
    write_record(&mut input, &after.encode().unwrap()).await;
    host.wait_until(|| !host.platform.injected_msgs().is_empty())
        .await;
    assert_eq!(host.platform.injected_msgs(), [after]);
    ping(&mut client).await;
}

#[tokio::test]
async fn role_changes_close_and_reopen_the_input_gate() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut input = client.open_typed(0x01).await;
    let event = InputMsg::MouseMove { x: 0.5, y: 0.5 };
    write_record(&mut input, &event.encode().unwrap()).await;
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    let key = host.conns()[0].key;
    let releases = host.platform.release_count();
    assert!(host.mgr.set_role(key, Role::Viewer));
    host.wait_until(|| host.platform.release_count() == releases + 1)
        .await;
    let mut blocked = client.open_typed(0x01).await;
    write_record(&mut blocked, &event.encode().unwrap()).await;
    blocked.finish().unwrap();
    assert_eq!(
        timeout(Duration::from_secs(3), blocked.stopped())
            .await
            .unwrap()
            .unwrap(),
        None
    );
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(
        host.platform.injected_msgs().as_slice(),
        std::slice::from_ref(&event)
    );
    // Repeating the closed state must not release twice.
    assert!(host.mgr.set_role(key, Role::Viewer));
    ping(&mut client).await;
    assert_eq!(host.platform.release_count(), releases + 1);
    assert!(host.mgr.set_role(key, Role::Controller));
    host.wait_until(|| host.conns()[0].role == Role::Controller)
        .await;
    write_record(&mut input, &event.encode().unwrap()).await;
    host.wait_until(|| host.platform.injected_msgs().len() == 2)
        .await;
    assert_eq!(host.platform.injected_msgs(), [event.clone(), event]);
    assert_eq!(host.platform.release_count(), releases + 1);
}

#[tokio::test]
async fn closing_releases_all_regardless_of_input_gate() {
    for view_only in [false, true] {
        let mut options = WireOptions::default();
        options.options.view_only = view_only;
        let host = WireHost::start_with(options).await;
        let mut client = host.connect().await;
        client.hello().await;
        let releases = host.platform.release_count();
        client.goodbye(pb::goodbye::Reason::UserClosed).await;
        assert_eq!(client.close_code().await, 0);
        host.wait_until(|| host.platform.release_count() == releases + 1)
            .await;
    }
}

#[tokio::test]
async fn view_only_session_end_still_releases_all() {
    let mut options = WireOptions::default();
    options.options.view_only = true;
    let host = WireHost::start_with(options).await;
    let mut first = host.connect().await;
    let (_, auth) = first.hello().await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    let releases = host.platform.release_count();

    let mut second = host.connect().await;
    let (_, auth) = second.hello().await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    host.wait_until(|| host.platform.release_count() > releases)
        .await;
}

#[tokio::test]
async fn partial_records_check_the_gate_at_both_ends() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut input = client.open_typed(0x01).await;
    let bytes = frame(&InputMsg::MouseMove { x: 0.1, y: 0.2 }.encode().unwrap());
    input.write_all(&bytes[..5]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    let key = host.conns()[0].key;
    assert!(host.mgr.set_role(key, Role::Viewer));
    host.wait_until(|| host.platform.release_count() == 1).await;
    input.write_all(&bytes[5..]).await.unwrap();
    input.write_all(&bytes[..5]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(host.mgr.set_role(key, Role::Controller));
    host.wait_until(|| host.conns()[0].role == Role::Controller)
        .await;
    input.write_all(&bytes[5..]).await.unwrap();
    let after = InputMsg::MouseMove { x: 0.7, y: 0.8 };
    write_record(&mut input, &after.encode().unwrap()).await;
    host.wait_until(|| !host.platform.injected_msgs().is_empty())
        .await;
    assert_eq!(host.platform.injected_msgs(), [after]);
}

#[tokio::test]
async fn unknown_empty_and_pointer_datagrams_preserve_the_session() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send_datagram(&[0x55, 1, 2, 3]);
    client.send_datagram(&[]);
    let pointer = PointerDatagram {
        seq: 1,
        display: host.platform.displays()[0].id as u8,
        visible: true,
        x: 0.25,
        y: 0.75,
    };
    client.send_datagram(&pointer.encode());
    host.wait_until(|| host.platform.injected_msgs() == [InputMsg::MouseMove { x: 0.25, y: 0.75 }])
        .await;
    ping(&mut client).await;
}

#[tokio::test]
async fn video_header_uses_the_stream_config_display_id() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client
        .start_stream(pb::StartStream {
            codec: pb::Codec::Hevc as i32,
            fps: 30,
            bitrate_kbps: 2000,
            max_width: 640,
            ..Default::default()
        })
        .await;
    let video = client.recv_datagram(0x10).await;
    assert_eq!(video[2], config.display_id as u8);
    let mut input = client.open_typed(0x01).await;
    let event = InputMsg::MouseMove { x: 0.5, y: 0.5 };
    write_record(&mut input, &event.encode().unwrap()).await;
    host.wait_until(|| !host.platform.injected_msgs().is_empty())
        .await;
    assert_eq!(
        host.platform.injected_with_display(),
        [(event, host.platform.displays()[0].id)]
    );
}

#[tokio::test]
async fn stalled_type_reader_does_not_block_input_or_control() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    // The dispatcher must give every accepted stream its own type reader.
    let mut stalled = client.conn.open_uni().await.unwrap();
    // Sending an empty write does not announce a stream; a later stream ID
    // makes the earlier stream visible to accept_uni on the server.
    let mut input = client.open_typed(0x01).await;
    write_record(
        &mut input,
        &InputMsg::MouseMove { x: 0.4, y: 0.6 }.encode().unwrap(),
    )
    .await;
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    ping(&mut client).await;
    stalled.finish().unwrap();
}
