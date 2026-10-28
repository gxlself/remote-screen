use host_core::platform::DisplayInfo;
use host_fake::FakePlatform;
use rsp_core::{features, hid::InputMsg, pb};
use std::time::Duration;
use tokio::time::{sleep, timeout};

mod wire_support;
use wire_support::{client_hello, write_record, Dev, WireClient, WireHost, WireOptions};

const QUIET: Duration = Duration::from_millis(200);

fn display(id: u32, primary: bool, width_px: u32, height_px: u32) -> DisplayInfo {
    DisplayInfo {
        id,
        name: format!("display-{id}"),
        width_px,
        height_px,
        scale: 1.0,
        x: if primary { 0 } else { 1920 },
        y: 0,
        refresh_hz: 60,
        primary,
        rotation: 0,
    }
}

fn d10() -> DisplayInfo {
    display(10, true, 1920, 1080)
}

fn d20() -> DisplayInfo {
    display(20, false, 2560, 1440)
}

fn d30() -> DisplayInfo {
    display(30, false, 1600, 900)
}

async fn host_with(displays: Vec<DisplayInfo>) -> WireHost {
    WireHost::start_with(WireOptions {
        platform: Some(FakePlatform::with_displays(displays)),
        ..Default::default()
    })
    .await
}

async fn two_displays() -> WireHost {
    // The primary must come first on the wire even when the platform lists it last.
    host_with(vec![d20(), d10()]).await
}

fn start(display_id: u32) -> pb::StartStream {
    pb::StartStream {
        display_id,
        codec: pb::Codec::Hevc as i32,
        fps: 20,
        bitrate_kbps: 500,
        ..Default::default()
    }
}

fn ids(displays: &[pb::Display]) -> Vec<u32> {
    displays.iter().map(|d| d.id).collect()
}

async fn stream_config(client: &mut WireClient) -> pb::StreamConfig {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            pb::envelope::Msg::Error(error) => panic!("stream failed: {error:?}"),
            pb::envelope::Msg::Goodbye(goodbye) => panic!("stream closed: {goodbye:?}"),
            _ => None,
        })
        .await
}

async fn display_list(client: &mut WireClient) -> pb::DisplayList {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::DisplayList(list) => Some(list.clone()),
            pb::envelope::Msg::StreamConfig(config) => {
                panic!("DisplayList must precede the rebuild: {config:?}")
            }
            pb::envelope::Msg::Error(error) => panic!("display update failed: {error:?}"),
            pb::envelope::Msg::Goodbye(goodbye) => panic!("session closed: {goodbye:?}"),
            _ => None,
        })
        .await
}

async fn error(client: &mut WireClient, key: &str, value: &str) {
    let error = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Error(error) => Some(error.clone()),
            pb::envelope::Msg::StreamConfig(config) => {
                panic!("invalid selection reconfigured the stream: {config:?}")
            }
            _ => None,
        })
        .await;
    assert_eq!(error.code, 199);
    assert_eq!(error.detail.get(key).map(String::as_str), Some(value));
}

async fn no_reconfigure(client: &mut WireClient) {
    client
        .expect_none_matching(QUIET, |msg| {
            matches!(
                msg,
                pb::envelope::Msg::StreamConfig(_)
                    | pb::envelope::Msg::Error(_)
                    | pb::envelope::Msg::Goodbye(_)
            )
        })
        .await;
}

async fn video(client: &WireClient, config: &pb::StreamConfig) {
    timeout(Duration::from_secs(3), async {
        loop {
            let bytes = client.recv_datagram(0x10).await;
            assert!(bytes.len() >= 28);
            // Control messages and datagrams are independent; old epoch packets
            // may already be queued when the new StreamConfig arrives.
            if bytes[3] == config.epoch as u8 {
                assert_eq!(bytes[2], config.display_id as u8);
                return;
            }
        }
    })
    .await
    .expect("no video for the current epoch");
}

#[tokio::test]
async fn hello_and_two_connections_use_wire_ids_and_capture_the_requested_displays() {
    let host = two_displays().await;
    let mut primary = host.connect().await;
    let (hello, auth) = primary.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_ne!(hello.features & features::MULTI_DISPLAY, 0);
    assert_eq!(ids(&hello.displays), [1, 2]);
    assert!(hello.displays[0].primary);
    assert_eq!(hello.displays[0].name, "display-10");
    assert_eq!(hello.displays[1].name, "display-20");

    let first = primary.start_stream(start(0)).await;
    assert_eq!((first.display_id, first.epoch), (1, 1));
    assert_eq!((first.width, first.height), (1920, 1080));
    let bytes = primary.recv_datagram(0x10).await;
    assert!(bytes.len() >= 28);
    assert_eq!((bytes[2], bytes[3]), (1, 1));
    assert_eq!(host.platform.capture_displays().last(), Some(&10));

    let mut second = host.connect_as(Dev::B, 0).await;
    let (hello, auth) = second
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    assert_eq!(ids(&hello.displays), [1, 2]);
    let config = second.start_stream(start(2)).await;
    assert_eq!((config.display_id, config.epoch), (2, 1));
    assert_eq!((config.width, config.height), (2560, 1440));
    assert_eq!(host.platform.capture_displays().last(), Some(&20));
    video(&second, &config).await;
    assert_eq!(host.platform.active_captures(), 2);
    assert_eq!(host.conns().len(), 2);
}

#[tokio::test]
async fn select_rebuilds_with_the_stream_epoch_and_routes_typed_input() {
    let host = two_displays().await;
    let mut client = host.connect().await;
    client.hello().await;
    let mut input = client.open_typed(0x01).await;
    let initial = client.start_stream(start(0)).await;

    // Pause/resume advances only the stream epoch. Selection must continue it,
    // rather than publishing StreamDisplay's separate internal epoch.
    client.send(pb::PauseStream {}).await;
    host.wait_until(|| host.platform.active_captures() == 0)
        .await;
    client.send(pb::ResumeStream {}).await;
    let resumed = stream_config(&mut client).await;
    assert_eq!(resumed.epoch, initial.epoch + 1);

    client.send(pb::SelectDisplay { display_id: 2 }).await;
    let selected = stream_config(&mut client).await;
    assert_eq!(selected.display_id, 2);
    assert_eq!(selected.epoch, resumed.epoch + 1);
    assert_eq!(host.platform.capture_displays().last(), Some(&20));
    video(&client, &selected).await;
    let event = InputMsg::MouseMove { x: 0.25, y: 0.75 };
    write_record(&mut input, &event.encode().unwrap()).await;
    host.wait_until(|| !host.platform.injected_with_display().is_empty())
        .await;
    assert_eq!(
        host.platform.injected_with_display().last(),
        Some(&(event, 20))
    );

    client.send(pb::SelectDisplay { display_id: 2 }).await;
    no_reconfigure(&mut client).await;
    client.send(pb::SelectDisplay { display_id: 0 }).await;
    let primary = stream_config(&mut client).await;
    assert_eq!(primary.display_id, 1);
    assert_eq!(primary.epoch, selected.epoch + 1);
    assert_eq!(host.platform.capture_displays().last(), Some(&10));
    video(&client, &primary).await;
}

#[tokio::test]
async fn unknown_selection_reports_199_and_keeps_the_current_stream() {
    let host = two_displays().await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client.start_stream(start(2)).await;
    client.send(pb::SelectDisplay { display_id: 9 }).await;
    error(&mut client, "display_id", "9").await;
    no_reconfigure(&mut client).await;
    video(&client, &config).await;
    assert_eq!(host.platform.capture_displays(), [20]);
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn select_before_start_is_ignored_and_start_has_its_own_selection() {
    let host = two_displays().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(pb::SelectDisplay { display_id: 2 }).await;
    client.send(pb::SelectDisplay { display_id: 9 }).await;
    no_reconfigure(&mut client).await;
    assert!(host.platform.capture_displays().is_empty());
    let config = client.start_stream(start(0)).await;
    assert_eq!((config.display_id, config.epoch), (1, 1));
    assert_eq!(host.platform.capture_displays(), [10]);
}

#[tokio::test]
async fn hotplug_broadcasts_to_idle_connections_falls_back_and_preserves_wire_ids() {
    let host = two_displays().await;
    let mut client = host.connect().await;
    client.hello().await;
    let initial = client.start_stream(start(2)).await;
    let mut observer = host.connect_as(Dev::B, 0).await;
    observer
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;

    host.platform.set_displays(vec![d10(), d30()]);
    assert_eq!(ids(&display_list(&mut client).await.displays), [1, 3]);
    assert_eq!(ids(&display_list(&mut observer).await.displays), [1, 3]);
    let fallback = stream_config(&mut client).await;
    assert_eq!(fallback.display_id, 1);
    assert_eq!(fallback.epoch, initial.epoch + 1);
    assert_eq!(host.platform.capture_displays().last(), Some(&10));
    video(&client, &fallback).await;
    no_reconfigure(&mut observer).await;

    host.platform.set_displays(vec![d10(), d20(), d30()]);
    let list = display_list(&mut client).await;
    assert_eq!(ids(&list.displays), [1, 2, 3]);
    assert_eq!(list.displays[1].name, "display-20");
    assert_eq!(ids(&display_list(&mut observer).await.displays), [1, 2, 3]);
    no_reconfigure(&mut client).await;
    assert_eq!(host.platform.active_captures(), 1);
    assert_eq!(host.conns().len(), 2);

    host.platform.set_displays(vec![d10(), d20(), d30()]);
    client
        .expect_none_matching(QUIET, |msg| {
            matches!(
                msg,
                pb::envelope::Msg::DisplayList(_) | pb::envelope::Msg::StreamConfig(_)
            )
        })
        .await;
}

#[tokio::test]
async fn selected_size_and_rotation_changes_rebuild_but_unselected_changes_do_not() {
    let host = two_displays().await;
    let mut client = host.connect().await;
    client.hello().await;
    let initial = client.start_stream(start(1)).await;

    let mut other = d20();
    other.width_px = 1440;
    other.height_px = 2560;
    other.rotation = 90;
    host.platform.set_displays(vec![d10(), other.clone()]);
    let list = display_list(&mut client).await;
    assert_eq!(list.displays[1].rotation, 90);
    no_reconfigure(&mut client).await;

    let mut primary = d10();
    primary.width_px = 1280;
    primary.height_px = 720;
    host.platform
        .set_displays(vec![primary.clone(), other.clone()]);
    let list = display_list(&mut client).await;
    assert_eq!(list.displays[0].width_px, 1280);
    let resized = stream_config(&mut client).await;
    assert_eq!(resized.display_id, 1);
    assert_eq!((resized.width, resized.height), (1280, 720));
    assert_eq!(resized.epoch, initial.epoch + 1);
    video(&client, &resized).await;

    // A rotation-only notification must rebuild even if pixel dimensions match.
    primary.rotation = 180;
    host.platform.set_displays(vec![primary, other]);
    assert_eq!(display_list(&mut client).await.displays[0].rotation, 180);
    let rotated = stream_config(&mut client).await;
    assert_eq!(rotated.display_id, 1);
    assert_eq!((rotated.width, rotated.height), (1280, 720));
    assert_eq!(rotated.epoch, resized.epoch + 1);
    video(&client, &rotated).await;
    assert_eq!(host.platform.capture_displays(), [10, 10, 10]);
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn removing_all_displays_suspends_then_reappearance_resumes_the_connection() {
    let host = two_displays().await;
    let mut client = host.connect().await;
    client.hello().await;
    let initial = client.start_stream(start(2)).await;
    video(&client, &initial).await;

    host.platform.set_displays(vec![]);
    assert!(display_list(&mut client).await.displays.is_empty());
    sleep(Duration::from_millis(500)).await;
    assert_eq!(host.platform.active_captures(), 0);
    assert_eq!(host.conns().len(), 1);
    assert!(client.conn.close_reason().is_none());
    no_reconfigure(&mut client).await;
    // Drain packets sent before suspension, then prove no new video is produced.
    while let Ok(bytes) = timeout(Duration::from_millis(10), client.conn.read_datagram()).await {
        bytes.expect("the suspended connection must remain open");
    }
    assert!(timeout(QUIET, client.recv_datagram(0x10)).await.is_err());
    client.send(pb::SelectDisplay { display_id: 1 }).await;
    error(&mut client, "display", "none").await;

    host.platform.set_displays(vec![d10()]);
    assert_eq!(ids(&display_list(&mut client).await.displays), [1]);
    let restored = stream_config(&mut client).await;
    assert_eq!(restored.display_id, 1);
    assert_eq!(restored.epoch, initial.epoch + 1);
    assert_eq!(host.platform.capture_displays().last(), Some(&10));
    assert_eq!(host.platform.active_captures(), 1);
    video(&client, &restored).await;
}

#[tokio::test]
async fn invalid_start_reports_the_missing_display_without_capturing() {
    let host = two_displays().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(start(9)).await;
    error(&mut client, "display_id", "9").await;
    no_reconfigure(&mut client).await;
    assert_eq!(host.platform.active_captures(), 0);
    let config = client.start_stream(start(2)).await;
    assert_eq!((config.display_id, config.epoch), (2, 1));
    assert_eq!(host.platform.capture_displays(), [20]);
}

#[tokio::test]
async fn start_with_no_displays_reports_none_without_capturing() {
    let host = host_with(vec![]).await;
    let mut client = host.connect().await;
    assert!(client.hello().await.0.displays.is_empty());
    client.send(start(0)).await;
    error(&mut client, "display", "none").await;
    no_reconfigure(&mut client).await;
    assert_eq!(host.platform.active_captures(), 0);
    assert!(host.platform.capture_displays().is_empty());
    assert!(client.conn.close_reason().is_none());
}
