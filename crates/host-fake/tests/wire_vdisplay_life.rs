use host_core::{
    session::{handlers::roster, takeover::Seat},
    vdisplay::VdError,
};
use host_fake::{
    vdisplay::{Appear, FakeVdOptions, FakeVirtualDisplays, VdCall},
    FakePlatform,
};
use rsp_core::pb;
use std::sync::Arc;
use std::time::Duration;

mod wire_support;
use wire_support::{client_hello, device_id, Dev, WireClient, WireHost, WireOptions};

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

fn create(req_id: u32, select: bool) -> pb::CreateVirtualDisplay {
    pb::CreateVirtualDisplay {
        request_id: req_id,
        width_px: 2732,
        height_px: 2048,
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

async fn list(client: &mut WireClient) -> pb::DisplayList {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::DisplayList(list) => Some(list.clone()),
            _ => None,
        })
        .await
}

async fn created(host: &WireHost, client: &mut WireClient) -> u32 {
    client.send(create(1, false)).await;
    let result = result(client).await;
    assert_eq!(result.request_id, 1);
    assert_eq!(result.status, pb::virtual_display_result::Status::Ok as i32);
    let display = result.display.expect("successful create has a display");
    let wire_id = display.id;
    host.wait_until(|| {
        host.mgr
            .features()
            .displays
            .registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .displays_pb()
            .iter()
            .any(|item| item.id == wire_id && item.is_virtual)
    })
    .await;
    wire_id
}

fn roster_contains(host: &WireHost, dev: Dev) -> bool {
    roster::with_roster(&host.mgr, |roster| {
        !roster.sessions_for_device(&device_id(dev)).is_empty()
    })
}

async fn goodbye_reason(client: &mut WireClient, reason: pb::goodbye::Reason) {
    let actual = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Goodbye(goodbye) => Some(goodbye.reason),
            _ => None,
        })
        .await;
    assert_eq!(actual, reason as i32);
}

#[tokio::test]
async fn linger_keeps_and_resume_sees_same_id() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    let (_, auth) = a.hello().await;
    let wire_id = created(&host, &mut a).await;
    let mut b = host.connect_as(Dev::B, 0).await;
    b.hello_with(pb::ClientHello {
        intent: pb::Intent::View as i32,
        ..client_hello()
    })
    .await;

    a.conn.close(0u32.into(), b"network disconnected");
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    host.clock.skip(Duration::from_secs(30));
    host.mgr.tick_all();
    b.expect_none_matching(Duration::from_millis(600), |msg| {
        matches!(msg, pb::envelope::Msg::DisplayList(_))
    })
    .await;
    assert!(vd.destroyed().is_empty());

    let mut resumed = host.connect().await;
    let (hello, resumed_auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert!(resumed_auth.resumed);
    assert!(hello
        .displays
        .iter()
        .any(|display| display.id == wire_id && display.is_virtual));

    resumed.send(create(5, false)).await;
    let second = result(&mut resumed).await;
    assert_eq!(second.status, pb::virtual_display_result::Status::Ok as i32);
    assert_eq!(second.request_id, 5);
    assert_eq!(second.display.expect("existing display").id, wire_id);
    assert_eq!(vd.create_count(), 1);
}

#[tokio::test]
async fn linger_expiry_destroys() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    a.hello().await;
    let wire_id = created(&host, &mut a).await;
    let mut b = host.connect_as(Dev::B, 0).await;
    b.hello_with(pb::ClientHello {
        intent: pb::Intent::View as i32,
        ..client_hello()
    })
    .await;

    a.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    host.clock.skip(Duration::from_secs(61));
    host.mgr.tick_all();
    host.wait_until(|| vd.live().is_empty()).await;
    let displays = b
        .next_matching(|msg| match msg {
            pb::envelope::Msg::DisplayList(list)
                if !list.displays.iter().any(|display| display.id == wire_id) =>
            {
                Some(list.clone())
            }
            _ => None,
        })
        .await;
    assert!(!displays
        .displays
        .iter()
        .any(|display| display.id == wire_id));
}

#[tokio::test]
async fn user_closed_destroys() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    a.hello().await;
    created(&host, &mut a).await;
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| vd.destroyed() == [1]).await;
}

#[tokio::test]
async fn viewer_watching_falls_back_to_primary() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    a.hello().await;
    let wire_id = created(&host, &mut a).await;
    let mut b = host.connect_as(Dev::B, 0).await;
    b.hello_with(pb::ClientHello {
        intent: pb::Intent::View as i32,
        ..client_hello()
    })
    .await;
    b.start_stream(pb::StartStream {
        display_id: 0,
        ..Default::default()
    })
    .await;
    b.send(pb::SelectDisplay {
        display_id: wire_id,
    })
    .await;
    let selected = b
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            _ => None,
        })
        .await;
    assert_eq!(selected.display_id, wire_id);

    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    let displays = b
        .next_matching(|msg| match msg {
            pb::envelope::Msg::DisplayList(list)
                if !list.displays.iter().any(|display| display.id == wire_id) =>
            {
                Some(list.clone())
            }
            _ => None,
        })
        .await;
    assert!(!displays
        .displays
        .iter()
        .any(|display| display.id == wire_id));
    let fallback = b
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            _ => None,
        })
        .await;
    assert!(displays
        .displays
        .iter()
        .find(|display| display.primary)
        .is_some_and(|primary| primary.id == fallback.display_id));
    host.wait_until(|| vd.destroyed() == [1]).await;
}

#[tokio::test]
async fn appear_timeout_fails_and_destroys() {
    let (host, vd) = host_vd(FakeVdOptions {
        appear: Appear::Never,
        ..FakeVdOptions::default()
    })
    .await;
    let mut a = host.connect().await;
    a.hello().await;
    a.send(create(1, false)).await;
    a.expect_none_matching(Duration::from_millis(600), |msg| {
        matches!(msg, pb::envelope::Msg::VirtualDisplayResult(_))
    })
    .await;
    host.clock.skip(Duration::from_secs(11));
    let failed = result(&mut a).await;
    assert_eq!(failed.request_id, 1);
    assert_eq!(
        failed.status,
        pb::virtual_display_result::Status::Failed as i32
    );
    host.wait_until(|| vd.destroyed() == [1]).await;
}

#[tokio::test]
async fn manual_appear_completes() {
    let (host, vd) = host_vd(FakeVdOptions {
        appear: Appear::Manual,
        ..FakeVdOptions::default()
    })
    .await;
    let mut a = host.connect().await;
    a.hello().await;
    a.send(create(1, false)).await;
    a.expect_none_matching(Duration::from_millis(600), |msg| {
        matches!(msg, pb::envelope::Msg::VirtualDisplayResult(_))
    })
    .await;
    vd.show_pending();
    let displays = list(&mut a).await;
    let display = displays
        .displays
        .iter()
        .find(|display| display.is_virtual)
        .expect("manual display appears in list before result");
    let completed = result(&mut a).await;
    assert_eq!(
        completed.status,
        pb::virtual_display_result::Status::Ok as i32
    );
    assert_eq!(
        completed.display.as_ref().map(|item| item.id),
        Some(display.id)
    );
}

#[tokio::test]
async fn second_request_while_creating_is_limit() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    vd.hold_creates(true);
    let mut a = host.connect().await;
    a.hello().await;
    a.send(create(1, false)).await;
    host.wait_until(|| vd.waiting_creates() == 1).await;
    a.send(create(2, false)).await;
    let limited = result(&mut a).await;
    assert_eq!(limited.request_id, 2);
    assert_eq!(
        limited.status,
        pb::virtual_display_result::Status::Limit as i32
    );
    vd.release_creates();
    let completed = result(&mut a).await;
    assert_eq!(completed.request_id, 1);
    assert_eq!(
        completed.status,
        pb::virtual_display_result::Status::Ok as i32
    );
    assert_eq!(vd.create_count(), 1);
}

#[tokio::test]
async fn owner_leaves_while_creating() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    vd.hold_creates(true);
    let mut a = host.connect().await;
    a.hello().await;
    a.send(create(1, false)).await;
    host.wait_until(|| vd.waiting_creates() == 1).await;
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| !roster_contains(&host, Dev::A)).await;
    host.clock.skip(Duration::from_secs(1));
    vd.release_creates();
    host.wait_until(|| vd.destroyed() == [1] && vd.live().is_empty())
        .await;

    let mut b = host.connect_as(Dev::B, 0).await;
    let (hello, _) = b.hello().await;
    assert!(!hello.displays.iter().any(|display| display.is_virtual));
}

#[tokio::test]
async fn backend_errors_map() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    a.hello().await;

    vd.fail_next(VdError::Limit);
    a.send(create(1, false)).await;
    let limit = result(&mut a).await;
    assert_eq!(limit.request_id, 1);
    assert_eq!(
        limit.status,
        pb::virtual_display_result::Status::Limit as i32
    );

    vd.fail_next(VdError::Failed("boom".into()));
    a.send(create(2, false)).await;
    let failed = result(&mut a).await;
    assert_eq!(failed.request_id, 2);
    assert_eq!(
        failed.status,
        pb::virtual_display_result::Status::Failed as i32
    );
    assert!(failed.message.contains("boom"));

    vd.fail_next(VdError::Unsupported("gone".into()));
    a.send(create(3, false)).await;
    let unsupported = result(&mut a).await;
    assert_eq!(unsupported.request_id, 3);
    assert_eq!(
        unsupported.status,
        pb::virtual_display_result::Status::Unsupported as i32
    );
    assert!(vd.live().is_empty());

    a.send(create(4, false)).await;
    let completed = result(&mut a).await;
    assert_eq!(completed.request_id, 4);
    assert_eq!(
        completed.status,
        pb::virtual_display_result::Status::Ok as i32
    );
}

#[tokio::test]
async fn unexpected_vanish_destroys_after_grace() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    a.hello().await;
    let wire_id = created(&host, &mut a).await;
    vd.vanish(1);
    let displays = list(&mut a).await;
    assert!(!displays
        .displays
        .iter()
        .any(|display| display.id == wire_id));
    host.clock.skip(Duration::from_secs(2));
    a.expect_none_matching(Duration::from_millis(600), |msg| {
        matches!(msg, pb::envelope::Msg::DisplayList(_))
    })
    .await;
    assert!(vd.destroyed().is_empty());
    host.clock.skip(Duration::from_secs(2));
    host.wait_until(|| vd.destroyed() == [1]).await;
}

#[tokio::test]
async fn vanish_and_return_within_grace_keeps() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    a.hello().await;
    let wire_id = created(&host, &mut a).await;
    vd.vanish(1);
    let removed = list(&mut a).await;
    assert!(!removed.displays.iter().any(|display| display.id == wire_id));
    host.clock.skip(Duration::from_secs(1));
    vd.reappear(1);
    let restored = list(&mut a).await;
    assert!(restored
        .displays
        .iter()
        .any(|display| display.id == wire_id && display.is_virtual));
    host.clock.skip(Duration::from_secs(5));
    a.expect_none_matching(Duration::from_millis(600), |msg| {
        matches!(msg, pb::envelope::Msg::DisplayList(_))
    })
    .await;
    assert!(vd.destroyed().is_empty());
}

#[tokio::test]
async fn identify_by_name() {
    let (host, _) = host_vd(FakeVdOptions {
        identify_by_name: true,
        ..FakeVdOptions::default()
    })
    .await;
    let mut a = host.connect().await;
    a.hello().await;
    let wire_id = created(&host, &mut a).await;
    let display = host
        .mgr
        .features()
        .displays
        .registry
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .displays_pb()
        .into_iter()
        .find(|display| display.id == wire_id)
        .expect("created display is registered");
    assert!(display.is_virtual);
    assert!(display.name.starts_with("虚拟屏（"));
}

#[tokio::test]
async fn replaced_session_destroys() {
    let (host, vd) = host_vd(FakeVdOptions::default()).await;
    let mut a = host.connect().await;
    a.hello().await;
    created(&host, &mut a).await;

    let mut replacement = host.connect_as(Dev::A, 0).await;
    replacement.hello().await;
    goodbye_reason(&mut a, pb::goodbye::Reason::Replaced).await;
    host.wait_until(|| vd.destroyed() == [1]).await;

    replacement.send(create(2, false)).await;
    let completed = result(&mut replacement).await;
    assert_eq!(completed.request_id, 2);
    assert_eq!(
        completed.status,
        pb::virtual_display_result::Status::Ok as i32
    );
    assert_eq!(vd.create_count(), 2);
}

#[tokio::test]
async fn cleanup_stale_called_once_at_start() {
    let (host, vd) = host_vd(FakeVdOptions {
        stale: 3,
        ..FakeVdOptions::default()
    })
    .await;
    let mut a = host.connect().await;
    a.hello().await;
    created(&host, &mut a).await;
    let calls = vd.calls();
    assert_eq!(
        calls
            .iter()
            .filter(|call| matches!(call, VdCall::CleanupStale))
            .count(),
        1
    );
    let cleanup = calls
        .iter()
        .position(|call| matches!(call, VdCall::CleanupStale))
        .expect("startup cleanup call");
    let first_create = calls
        .iter()
        .position(|call| matches!(call, VdCall::Create { .. }))
        .expect("create call");
    assert!(cleanup < first_create);
}
