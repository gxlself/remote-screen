use host_core::{
    auth::Role,
    platform::CursorUpdate,
    session::{cursor, handlers::cursor as cursor_handler},
};
use rsp_core::{cursor::CursorStreamDecoder, features, pb, pointer::PointerDatagram};
use std::time::Duration;
use tokio::time::{sleep, timeout};

mod wire_support;
use wire_support::{client_hello, read_record, Dev, WireClient, WireHost};

const QUIET: Duration = Duration::from_millis(500);

fn shape(id: u64) -> pb::CursorShape {
    pb::CursorShape {
        id,
        width: 16,
        height: 18,
        hot_x: 2,
        hot_y: 3,
        scale: 2.0,
        png: vec![id as u8, 0x50, 0x4e, 0x47],
    }
}

fn position(display_id: u32, x: f32, y: f32, visible: bool) -> CursorUpdate {
    CursorUpdate::Position {
        display_id,
        seq: 0,
        x,
        y,
        visible,
    }
}

fn start(fps: u32) -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        max_width: 640,
        fps,
        bitrate_kbps: 500,
        ..Default::default()
    }
}

async fn connect(host: &WireHost, features: u64) -> (WireClient, pb::ServerHello) {
    connect_as(host, Dev::A, features).await
}

async fn connect_as(host: &WireHost, dev: Dev, features: u64) -> (WireClient, pb::ServerHello) {
    let mut client = host.connect_as(dev, 0).await;
    let expected = host.conns().len() + 1;
    let (server, auth) = client
        .hello_with(pb::ClientHello {
            features,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    host.wait_until(|| host.conns().len() == expected).await;
    (client, server)
}

async fn cursor_stream(client: &WireClient) -> quinn::RecvStream {
    let (kind, recv) = client.accept_typed().await;
    assert_eq!(kind, cursor::CURSOR_STREAM_TYPE);
    recv
}

async fn read_cursor(recv: &mut quinn::RecvStream) -> pb::CursorMsg {
    let payload = read_record(recv).await.expect("cursor record");
    let mut decoder = CursorStreamDecoder::new();
    decoder.push(&(payload.len() as u32).to_be_bytes());
    decoder.push(&payload);
    decoder
        .next_msg()
        .expect("cursor message decodes")
        .expect("complete cursor message")
}

async fn next_position(client: &WireClient) -> PointerDatagram {
    PointerDatagram::decode(&client.recv_datagram(0x12).await).expect("pointer datagram decodes")
}

async fn no_datagram(client: &WireClient, within: Duration) {
    assert!(
        timeout(within, client.recv_datagram(0x12)).await.is_err(),
        "unexpected pointer datagram"
    );
}

async fn pause(host: &WireHost, client: &mut WireClient) {
    client.send(pb::PauseStream {}).await;
    host.wait_until(|| host.platform.active_captures() == 0)
        .await;
}

#[tokio::test]
async fn shape_changes_use_one_lazy_cursor_stream() {
    let host = WireHost::start().await;
    let (mut client, _) = connect(&host, u64::MAX).await;

    assert!(
        timeout(Duration::from_millis(200), client.conn.accept_uni())
            .await
            .is_err(),
        "the cursor stream must stay closed until the first record"
    );
    client.start_stream(start(20)).await;
    sleep(Duration::from_millis(50)).await;

    let shape = shape(7);
    host.platform
        .push_cursor(CursorUpdate::Shape(shape.clone()));
    let mut recv = cursor_stream(&client).await;
    assert_eq!(
        read_cursor(&mut recv).await,
        pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Shape(shape.clone())),
        }
    );

    host.platform.push_cursor(CursorUpdate::Hidden(true));
    assert_eq!(
        read_cursor(&mut recv).await,
        pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Hidden(true)),
        }
    );
    assert!(
        timeout(Duration::from_millis(100), client.conn.accept_uni())
            .await
            .is_err()
    );
}

#[tokio::test]
async fn pointer_positions_are_rate_limited_and_keep_the_latest_value() {
    let host = WireHost::start().await;
    let (mut client, _) = connect(&host, u64::MAX).await;
    client.start_stream(start(20)).await;
    sleep(Duration::from_millis(50)).await;

    host.platform.push_cursor(position(1, 0.25, 0.75, true));
    let first = next_position(&client).await;
    assert_eq!(first.display, 1);
    assert_eq!((first.x, first.y), (0.25, 0.75));
    assert!(first.visible);

    for index in 0..10 {
        let value = 0.1 + index as f32 * 0.05;
        host.platform
            .push_cursor(position(1, value, 1.0 - value, true));
    }
    let mut datagrams = Vec::new();
    let deadline = tokio::time::Instant::now() + Duration::from_millis(100);
    while tokio::time::Instant::now() < deadline {
        match timeout(
            deadline.saturating_duration_since(tokio::time::Instant::now()),
            client.recv_datagram(0x12),
        )
        .await
        {
            Ok(bytes) => datagrams.push(PointerDatagram::decode(&bytes).expect("pointer")),
            Err(_) => break,
        }
    }
    assert!(
        datagrams.len() <= 3,
        "received {} datagrams",
        datagrams.len()
    );
    let expected = 0.1 + 9.0 * 0.05;
    let last = datagrams.last().copied();
    let last = match last {
        Some(last) if (last.x - expected).abs() < 0.0001 => last,
        _ => next_position(&client).await,
    };
    assert_eq!(last.display, 1);
    assert!((last.x - expected).abs() < 0.0001);
    assert!((last.y - (1.0 - expected)).abs() < 0.0001);
}

#[tokio::test]
async fn stream_config_uses_the_cursor_position_on_the_streamed_display() {
    let host = WireHost::start().await;
    let (mut client, _) = connect(&host, u64::MAX).await;
    host.platform.push_cursor(position(1, 0.1, 0.2, true));
    sleep(Duration::from_millis(20)).await;

    let config = client.start_stream(start(20)).await;
    assert!((config.cursor_x - 0.1).abs() < 0.0001);
    assert!((config.cursor_y - 0.2).abs() < 0.0001);
    assert!(config.cursor_visible);
}

#[tokio::test]
async fn resuming_a_stream_resends_the_current_shape_snapshot() {
    let host = WireHost::start().await;
    let (mut client, _) = connect(&host, u64::MAX).await;
    client.start_stream(start(20)).await;

    let shape = shape(7);
    host.platform
        .push_cursor(CursorUpdate::Shape(shape.clone()));
    let mut recv = cursor_stream(&client).await;
    assert!(matches!(
        read_cursor(&mut recv).await.msg,
        Some(pb::cursor_msg::Msg::Shape(_))
    ));

    pause(&host, &mut client).await;
    client.send(pb::ResumeStream {}).await;
    client
        .next_matching(|message| {
            matches!(message, pb::envelope::Msg::StreamConfig(_)).then_some(())
        })
        .await;
    assert_eq!(
        read_cursor(&mut recv).await,
        pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Shape(shape)),
        }
    );
}

#[tokio::test]
async fn controller_injected_positions_are_suppressed_but_viewers_see_them() {
    let host = WireHost::start().await;
    host.platform.push_cursor(position(1, 0.2, 0.2, true));
    host.wait_until(|| {
        host.mgr
            .features()
            .cursor
            .hub
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .position()
            == Some((1, 0.2, 0.2, true))
    })
    .await;

    let (mut a, _) = connect(&host, u64::MAX).await;
    a.start_stream(start(20)).await;
    let a_key = host.conns()[0].key;
    assert_eq!(host.conns()[0].role, Role::Controller);

    let (mut b, _) = connect_as(&host, Dev::B, u64::MAX).await;
    assert_eq!(host.conns()[1].role, Role::Viewer);
    b.start_stream(start(20)).await;
    sleep(Duration::from_millis(50)).await;

    assert!(host.mgr.post(
        a_key,
        host_core::session::handlers::FeatureEvent::Cursor(cursor_handler::Event::Injected(
            0.5, 0.5
        ))
    ));
    host.platform.push_cursor(position(1, 0.5, 0.5, true));
    no_datagram(&a, Duration::from_millis(200)).await;
    let viewer = next_position(&b).await;
    assert_eq!((viewer.x, viewer.y), (0.5, 0.5));
}

#[tokio::test]
async fn feature_bits_gate_the_stream_and_datagrams_independently() {
    let host = WireHost::start().await;
    let (mut pointer_only, _) = connect(&host, features::POINTER_DGRAM).await;
    pointer_only.start_stream(start(20)).await;
    host.platform.push_cursor(CursorUpdate::Shape(shape(7)));
    host.platform.push_cursor(CursorUpdate::Hidden(true));
    sleep(Duration::from_millis(50)).await;
    assert!(
        timeout(QUIET, pointer_only.conn.accept_uni())
            .await
            .is_err(),
        "POINTER_DGRAM alone must not open the cursor stream"
    );
    host.platform.push_cursor(position(1, 0.25, 0.75, true));
    let pointer = next_position(&pointer_only).await;
    assert_eq!((pointer.x, pointer.y), (0.25, 0.75));

    let (mut none, _) = connect_as(&host, Dev::B, 0).await;
    none.start_stream(start(20)).await;
    host.platform.push_cursor(CursorUpdate::Shape(shape(8)));
    host.platform.push_cursor(position(1, 0.3, 0.6, true));
    assert!(timeout(QUIET, none.conn.accept_uni()).await.is_err());
    no_datagram(&none, QUIET).await;
}

#[tokio::test]
async fn server_hello_advertises_cursor_features() {
    let host = WireHost::start().await;
    let (_, server) = connect(&host, u64::MAX).await;
    let required = features::CURSOR_SHAPE | features::POINTER_DGRAM;
    assert_eq!(server.features & required, required);
}
