use host_fake::{
    vdisplay::{FakeVdOptions, FakeVirtualDisplays},
    FakePlatform,
};
use rsp_core::{features, pb};
use std::sync::Arc;
use std::time::Duration;

mod wire_support;
use wire_support::{client_hello, Dev, WireClient, WireHost, WireOptions};

async fn host_vd(opts: FakeVdOptions) -> (WireHost, Arc<FakeVirtualDisplays>) {
    let platform = FakePlatform::new();
    let vd = FakeVirtualDisplays::install(&platform, opts);
    let host = WireHost::start_with(WireOptions {
        platform: Some(platform),
        ..Default::default()
    })
    .await;
    (host, vd)
}

fn create(req_id: u32, width_px: u32, height_px: u32, select: bool) -> pb::CreateVirtualDisplay {
    pb::CreateVirtualDisplay {
        request_id: req_id,
        width_px,
        height_px,
        scale: 2.0,
        refresh_hz: 60,
        select,
    }
}

async fn result(client: &mut WireClient) -> pb::VirtualDisplayResult {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::VirtualDisplayResult(result) => Some(result.clone()),
            _ => None,
        })
        .await
}

async fn until_result(client: &mut WireClient) -> (Vec<pb::DisplayList>, pb::VirtualDisplayResult) {
    let mut lists = Vec::new();
    let result = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::DisplayList(list) => {
                lists.push(list.clone());
                None
            }
            pb::envelope::Msg::VirtualDisplayResult(result) => Some(result.clone()),
            _ => None,
        })
        .await;
    (lists, result)
}

#[tokio::test]
async fn feature_bit_follows_backend() {
    let host = WireHost::start_with(WireOptions {
        platform: Some(FakePlatform::new()),
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    assert_eq!(
        client.hello().await.0.features & features::VIRTUAL_DISPLAY,
        0
    );

    let (host, _) = host_vd(FakeVdOptions::default()).await;
    let mut client = host.connect().await;
    assert_ne!(
        client.hello().await.0.features & features::VIRTUAL_DISPLAY,
        0
    );

    let (host, _) = host_vd(FakeVdOptions {
        caps: None,
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    assert_eq!(
        client.hello().await.0.features & features::VIRTUAL_DISPLAY,
        0
    );
}

#[tokio::test]
async fn create_ok_lists_then_results() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(create(1, 2732, 2048, false)).await;
    let (lists, result) = until_result(&mut client).await;
    assert_eq!(result.status, pb::virtual_display_result::Status::Ok as i32);
    assert_eq!(result.request_id, 1);
    let display = result.display.expect("successful create has a display");
    assert!(display.is_virtual);
    assert_eq!(display.width_px, 2732);
    assert_eq!(display.scale, 2.0);
    assert!(display.name.starts_with("虚拟屏（"));
    assert!(!lists.is_empty());
    let last = lists.last().expect("at least one display list");
    assert!(last
        .displays
        .iter()
        .any(|item| item.id == display.id && item.is_virtual));
    assert!(last
        .displays
        .iter()
        .any(|item| item.id == 1 && !item.is_virtual));
    assert_eq!(vd.create_count(), 1);
}

#[tokio::test]
async fn create_select_while_streaming_switches() {
    let (host, _) = host_vd(FakeVdOptions::default()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client
        .start_stream(pb::StartStream {
            display_id: 0,
            ..Default::default()
        })
        .await;
    client.send(create(2, 2732, 2048, true)).await;
    let result = result(&mut client).await;
    assert_eq!(result.status, pb::virtual_display_result::Status::Ok as i32);
    let display_id = result.display.expect("successful create").id;
    let config = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            _ => None,
        })
        .await;
    assert_eq!(config.display_id, display_id);
}

#[tokio::test]
async fn create_select_when_idle_is_noop() {
    let (host, _) = host_vd(FakeVdOptions::default()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(create(3, 2732, 2048, true)).await;
    let result = result(&mut client).await;
    assert_eq!(result.status, pb::virtual_display_result::Status::Ok as i32);
    client
        .expect_none_matching(Duration::from_millis(200), |msg| {
            matches!(msg, pb::envelope::Msg::StreamConfig(_))
        })
        .await;
}

#[tokio::test]
async fn viewer_not_allowed() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut controller = host.connect().await;
    controller.hello().await;
    let mut viewer = host.connect_as(Dev::B, 0).await;
    viewer
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    viewer.send(create(4, 2732, 2048, false)).await;
    let result = result(&mut viewer).await;
    assert_eq!(
        result.status,
        pb::virtual_display_result::Status::NotAllowed as i32
    );
    assert_eq!(vd.create_count(), 0);
}

#[tokio::test]
async fn unsupported_without_backend() {
    let host = WireHost::start_with(WireOptions {
        platform: Some(FakePlatform::new()),
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(create(5, 2732, 2048, false)).await;
    let result = result(&mut client).await;
    assert_eq!(
        result.status,
        pb::virtual_display_result::Status::Unsupported as i32
    );
}

#[tokio::test]
async fn bad_request() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(create(6, 0, 2048, false)).await;
    assert_eq!(
        result(&mut client).await.status,
        pb::virtual_display_result::Status::BadRequest as i32
    );
    client.send(create(7, 100, 100, false)).await;
    assert_eq!(
        result(&mut client).await.status,
        pb::virtual_display_result::Status::BadRequest as i32
    );
    assert_eq!(vd.create_count(), 0);
}

#[tokio::test]
async fn idempotent_second_request() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(create(8, 2732, 2048, false)).await;
    let first = result(&mut client).await;
    client.send(create(9, 2732, 2048, false)).await;
    let second = result(&mut client).await;
    assert_eq!(first.status, pb::virtual_display_result::Status::Ok as i32);
    assert_eq!(second.status, pb::virtual_display_result::Status::Ok as i32);
    assert_eq!(second.request_id, 9);
    assert_eq!(
        first.display.as_ref().map(|display| display.id),
        second.display.as_ref().map(|display| display.id)
    );
    assert_eq!(vd.create_count(), 1);
}

#[tokio::test]
async fn release_removes() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut client = host.connect().await;
    client.hello().await;
    client.send(create(10, 2732, 2048, false)).await;
    let created = result(&mut client).await;
    let display_id = created.display.expect("successful create").id;
    client.send(pb::ReleaseVirtualDisplay { display_id }).await;
    let removed = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::DisplayList(list)
                if !list.displays.iter().any(|d| d.id == display_id) =>
            {
                Some(list.clone())
            }
            _ => None,
        })
        .await;
    assert!(!removed.displays.iter().any(|d| d.id == display_id));
    host.wait_until(|| vd.destroyed() == [1]).await;
    assert_eq!(vd.destroyed(), [1]);
    client.send(pb::ReleaseVirtualDisplay { display_id }).await;
    client
        .expect_none_matching(Duration::from_millis(300), |msg| {
            matches!(msg, pb::envelope::Msg::DisplayList(_))
        })
        .await;
    assert_eq!(vd.destroyed(), [1]);
}

#[tokio::test]
async fn release_by_other_session_ignored() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut controller = host.connect().await;
    controller.hello().await;
    controller.send(create(11, 2732, 2048, false)).await;
    let created = result(&mut controller).await;
    let display_id = created.display.expect("successful create").id;

    let mut viewer = host.connect_as(Dev::B, 0).await;
    viewer
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    viewer.send(pb::ReleaseVirtualDisplay { display_id }).await;
    viewer
        .expect_none_matching(Duration::from_millis(300), |msg| {
            matches!(msg, pb::envelope::Msg::DisplayList(_))
        })
        .await;
    assert!(vd.destroyed().is_empty());
    assert_eq!(vd.live(), [1]);
}

#[tokio::test]
async fn hello_lists_virtual_after_reconnect() {
    let (host, _) = host_vd(FakeVdOptions::default()).await;
    let mut controller = host.connect().await;
    controller.hello().await;
    controller.send(create(12, 2732, 2048, false)).await;
    let created = result(&mut controller).await;
    let display_id = created.display.expect("successful create").id;

    let mut viewer = host.connect_as(Dev::C, 0).await;
    let (hello, _) = viewer
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    let display = hello
        .displays
        .iter()
        .find(|display| display.id == display_id)
        .expect("reconnected viewer sees the virtual display");
    assert!(display.is_virtual);
}
