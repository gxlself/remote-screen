use host_core::{
    platform::{CursorUpdate, Platform},
    session::cursor::ECHO_WINDOW_US,
};
use rsp_core::{
    features,
    hid::{InputMsg, Mods},
    pb,
    pointer::PointerDatagram,
};
use std::{sync::Arc, time::Duration};
use tokio::time::{sleep, timeout};

mod wire_support;
use wire_support::{
    client_hello, write_record, Dev, OffsetClock, WireClient, WireHost, WireOptions,
};

const QUIET: Duration = Duration::from_millis(200);
const WARN_WINDOW: Duration = Duration::from_millis(1500);
const IDLE_TIMEOUT_MIN: u16 = 2;

fn start_stream() -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        max_width: 640,
        fps: 1,
        bitrate_kbps: 500,
        ..Default::default()
    }
}

async fn connect(
    host: &WireHost,
    dev: Dev,
    intent: pb::Intent,
    client_features: u64,
) -> (WireClient, pb::ServerHello, pb::AuthResult) {
    let mut client = host.connect_as(dev, 0).await;
    let expected = host.conns().len() + 1;
    let (server, auth) = client
        .hello_with(pb::ClientHello {
            intent: intent as i32,
            features: client_features,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    host.wait_until(|| host.conns().len() == expected).await;
    (client, server, auth)
}

async fn controller(host: &WireHost) -> (WireClient, pb::ServerHello) {
    let (client, server, auth) = connect(
        host,
        Dev::A,
        pb::Intent::Control,
        features::POINTER_DGRAM | features::KEY_HID | features::TEXT_INPUT,
    )
    .await;
    assert_eq!(auth.role, pb::Role::Controller as i32);
    (client, server)
}

fn pointer(seq: u32, display: u8, x: f32, y: f32) -> PointerDatagram {
    PointerDatagram {
        seq,
        display,
        visible: true,
        x,
        y,
    }
}

async fn send_input(input: &mut quinn::SendStream, msg: &InputMsg) {
    write_record(input, &msg.encode().expect("input encodes")).await;
}

async fn ping(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 42 }).await;
    let pong = client
        .next_matching(|message| match message {
            pb::envelope::Msg::Pong(pong) => Some(*pong),
            _ => None,
        })
        .await;
    assert_eq!(pong.t1_us, 42);
}

#[tokio::test]
async fn pointer_datagram_injects_mouse_move_on_the_stream_display() {
    let host = WireHost::start().await;
    let (mut client, _) = controller(&host).await;
    let config = client.start_stream(start_stream()).await;
    let display_id = host.platform.displays()[0].id;

    client.send_datagram(&pointer(1, config.display_id as u8, 0.5, 0.25).encode());
    host.wait_until(|| host.platform.injected_with_display().len() == 1)
        .await;
    assert_eq!(
        host.platform.injected_with_display().last(),
        Some(&(InputMsg::MouseMove { x: 0.5, y: 0.25 }, display_id))
    );
}

#[tokio::test]
async fn stale_pointer_datagram_is_not_injected() {
    let host = WireHost::start().await;
    let (mut client, _) = controller(&host).await;
    let config = client.start_stream(start_stream()).await;
    let display = config.display_id as u8;

    client.send_datagram(&pointer(3, display, 0.3, 0.3).encode());
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    client.send_datagram(&pointer(2, display, 0.2, 0.2).encode());
    sleep(QUIET).await;

    assert_eq!(
        host.platform.injected_msgs(),
        [InputMsg::MouseMove { x: 0.3, y: 0.3 }]
    );
}

#[tokio::test]
async fn pointer_datagram_for_another_display_is_not_injected() {
    let host = WireHost::start().await;
    let (mut client, _) = controller(&host).await;
    let config = client.start_stream(start_stream()).await;

    client.send_datagram(&pointer(1, config.display_id as u8 + 1, 0.4, 0.6).encode());
    sleep(QUIET).await;
    assert!(host.platform.injected_msgs().is_empty());
}

#[tokio::test]
async fn viewer_pointer_datagram_is_not_injected() {
    let host = WireHost::start().await;
    let (mut owner, _) = controller(&host).await;
    let config = owner.start_stream(start_stream()).await;
    let (viewer, _, auth) = connect(&host, Dev::B, pb::Intent::View, features::POINTER_DGRAM).await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);

    viewer.send_datagram(&pointer(1, config.display_id as u8, 0.5, 0.5).encode());
    sleep(QUIET).await;
    assert!(host.platform.injected_msgs().is_empty());
}

#[tokio::test]
async fn malformed_pointer_datagram_does_not_close_the_connection() {
    let host = WireHost::start().await;
    let (mut client, _) = controller(&host).await;
    let config = client.start_stream(start_stream()).await;

    client.send_datagram(&[0x12; 10]);
    sleep(QUIET).await;
    assert!(host.platform.injected_msgs().is_empty());

    let event = InputMsg::MouseMove { x: 0.7, y: 0.2 };
    client.send_datagram(&pointer(1, config.display_id as u8, 0.7, 0.2).encode());
    host.wait_until(|| host.platform.injected_msgs() == [event.clone()])
        .await;
    ping(&mut client).await;
}

#[tokio::test]
async fn hid_keys_and_text_are_injected_in_order() {
    let host = WireHost::start().await;
    let (client, _) = controller(&host).await;
    let mut input = client.open_typed(0x01).await;
    let events = [
        InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::NONE,
        },
        InputMsg::Key {
            usage: 0x04,
            down: false,
            mods: Mods::NONE,
        },
        InputMsg::Text("你好".into()),
    ];
    for event in &events {
        send_input(&mut input, event).await;
    }
    host.wait_until(|| host.platform.injected_msgs().len() == events.len())
        .await;
    assert_eq!(host.platform.injected_msgs(), events);
}

#[tokio::test]
async fn injected_input_resets_the_idle_timer() {
    let mut options = WireOptions::default();
    options.config.access.idle_timeout_min = IDLE_TIMEOUT_MIN;
    let host = WireHost::start_with(options).await;
    let (mut client, _) = controller(&host).await;

    host.clock.skip(Duration::from_secs(61));
    let warning = timeout(
        WARN_WINDOW,
        client.next_matching(|message| match message {
            pb::envelope::Msg::IdleWarning(warning) => Some(*warning),
            _ => None,
        }),
    )
    .await
    .expect("initial idle warning arrives");
    assert!((58..=60).contains(&warning.seconds_left), "{warning:?}");

    let mut input = client.open_typed(0x01).await;
    send_input(&mut input, &InputMsg::MouseMove { x: 0.1, y: 0.1 }).await;
    host.wait_until(|| !host.platform.injected_msgs().is_empty())
        .await;
    ping(&mut client).await;

    // HW7 warns 60 seconds before expiry. A 58-second jump leaves two seconds
    // of margin inside the 1.5-second quiet window below.
    host.clock.skip(Duration::from_secs(58));
    client
        .expect_none_matching(WARN_WINDOW, |message| {
            matches!(
                message,
                pb::envelope::Msg::IdleWarning(_) | pb::envelope::Msg::Goodbye(_)
            )
        })
        .await;
    assert_eq!(host.conns().len(), 1);
}

#[tokio::test]
async fn injected_pointer_suppresses_the_matching_cursor_echo() {
    // Executor delays must not move this test out of the echo window.
    let host = WireHost::start_with(WireOptions {
        clock: Some(Arc::new(OffsetClock::manual())),
        ..Default::default()
    })
    .await;
    let (mut client, _) = controller(&host).await;
    client.start_stream(start_stream()).await;
    let display_id = host.platform.displays()[0].id;
    let mut input = client.open_typed(0x01).await;

    send_input(&mut input, &InputMsg::MouseMove { x: 0.3, y: 0.3 }).await;
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;
    wait_for_injection_report(&host).await;

    host.platform.push_cursor(CursorUpdate::Position {
        display_id,
        seq: 0,
        x: 0.3,
        y: 0.3,
        visible: true,
    });
    match timeout(QUIET, client.recv_datagram(0x12)).await {
        Err(_) => {}
        Ok(bytes) => panic!(
            "injected pointer position must not be echoed: {:?}",
            PointerDatagram::decode(&bytes)
        ),
    }

    // A different position must still be sent while the echo window is open.
    host.platform.push_cursor(CursorUpdate::Position {
        display_id,
        seq: 1,
        x: 0.8,
        y: 0.8,
        visible: true,
    });
    let position = PointerDatagram::decode(&client.recv_datagram(0x12).await)
        .expect("pointer datagram decodes");
    assert_eq!((position.x, position.y), (0.8, 0.8));

    // At the window boundary, the injected coordinates must be sent again.
    host.clock.skip(Duration::from_micros(ECHO_WINDOW_US));
    host.platform.push_cursor(CursorUpdate::Position {
        display_id,
        seq: 2,
        x: 0.3,
        y: 0.3,
        visible: true,
    });
    let position = PointerDatagram::decode(&client.recv_datagram(0x12).await)
        .expect("pointer datagram decodes");
    assert_eq!((position.x, position.y), (0.3, 0.3));
}

async fn wait_for_injection_report(host: &WireHost) {
    let key = host
        .conns()
        .into_iter()
        .find(|info| info.role == host_core::auth::Role::Controller)
        .expect("a controller is connected")
        .key;
    let counters: Arc<host_core::session::handlers::input::InputCounters> = host
        .mgr
        .features()
        .input
        .counters(key)
        .expect("input counters are registered");
    host.wait_until(|| counters.reported() >= 1).await;
}

#[tokio::test]
async fn server_hello_advertises_hid_and_text_input() {
    let host = WireHost::start().await;
    let (_, server) = controller(&host).await;
    let required = features::KEY_HID | features::TEXT_INPUT;
    assert_eq!(server.features & required, required);
}
