use host_core::{
    pipeline::{viewport::expand_roi, viewport_geom},
    platform::{DisplayInfo, PixelRect},
};
use host_fake::FakePlatform;
use rsp_core::{features, pb};
use std::time::Duration;

mod wire_support;
use wire_support::{client_hello, OffsetClock, WireClient, WireHost, WireOptions};

const A: PixelRect = PixelRect {
    x: 800,
    y: 400,
    w: 480,
    h: 270,
};
const C: PixelRect = PixelRect {
    x: 1200,
    y: 600,
    w: 480,
    h: 270,
};

fn hello() -> pb::ClientHello {
    let mut hello = client_hello();
    hello.features |= features::VIEWPORT;
    hello
}

fn start() -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        max_width: 1920,
        fps: 30,
        ..Default::default()
    }
}

fn vp(seq: u32, display_id: u32, rect: Option<PixelRect>, settled: bool) -> pb::Viewport {
    pb::Viewport {
        seq,
        display_id,
        rect: rect.map(|r| pb::PixelRect {
            x: r.x,
            y: r.y,
            w: r.w,
            h: r.h,
        }),
        settled,
        view_width_px: 2532,
        view_height_px: 1170,
        crop_ok: true,
    }
}

async fn host(platform: Option<FakePlatform>) -> WireHost {
    WireHost::start_with(WireOptions {
        platform,
        clock: Some(std::sync::Arc::new(OffsetClock::manual())),
        ..Default::default()
    })
    .await
}

async fn ack(client: &mut WireClient) -> pb::ViewportAck {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::ViewportAck(ack) => Some(*ack),
            pb::envelope::Msg::Error(error) => panic!("viewport failed: {error:?}"),
            _ => None,
        })
        .await
}

async fn config(client: &mut WireClient) -> pb::StreamConfig {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            pb::envelope::Msg::Error(error) => panic!("stream failed: {error:?}"),
            _ => None,
        })
        .await
}

async fn no_viewport_or_config(client: &mut WireClient, duration: Duration) {
    client
        .expect_none_matching(duration, |msg| {
            matches!(
                msg,
                pb::envelope::Msg::ViewportAck(_)
                    | pb::envelope::Msg::StreamConfig(_)
                    | pb::envelope::Msg::Error(_)
            )
        })
        .await;
}

#[tokio::test]
async fn advertises_viewport() {
    let host = host(None).await;
    let mut client = host.connect().await;
    let (server, _) = client.hello().await;
    assert_ne!(server.features & features::VIEWPORT, 0);
}

#[tokio::test]
async fn stream_config_has_display_size() {
    let host = host(None).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let config = client.start_stream(start()).await;
    assert_eq!(
        (config.display_width_px, config.display_height_px),
        (1920, 1080)
    );
    assert_eq!(config.crop, None);
    assert_eq!(config.viewport_seq, 0);
}

#[tokio::test]
async fn ignored_without_client_feature() {
    let host = host(None).await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client.start_stream(start()).await;
    client.send(vp(1, config.display_id, Some(A), true)).await;
    no_viewport_or_config(&mut client, Duration::from_secs(1)).await;
}

#[tokio::test]
async fn unsupported_platform_acks_off() {
    let host = host(None).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let config = client.start_stream(start()).await;
    client.send(vp(1, config.display_id, Some(A), true)).await;
    let ack = ack(&mut client).await;
    assert_eq!((ack.seq, ack.mode), (1, pb::viewport_ack::Mode::Off as i32));
    host.clock.skip(Duration::from_millis(1_500));
    no_viewport_or_config(&mut client, Duration::from_millis(100)).await;
}

#[tokio::test]
async fn settled_zoom_crops() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_supported(true);
    let host = host(Some(platform.clone())).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let initial = client.start_stream(start()).await;
    client.send(vp(1, initial.display_id, Some(A), false)).await;
    let first = ack(&mut client).await;
    assert_eq!(first.mode, pb::viewport_ack::Mode::Off as i32);
    client.send(vp(2, initial.display_id, Some(A), true)).await;
    let changed = config(&mut client).await;
    let second = ack(&mut client).await;
    assert_eq!(
        (second.seq, second.mode),
        (2, pb::viewport_ack::Mode::Crop as i32)
    );
    let crop = changed.crop.expect("crop config");
    assert_eq!((crop.x, crop.y, crop.w, crop.h), (736, 352, 608, 352));
    assert_eq!(changed.viewport_seq, 2);
    assert_eq!(changed.display_width_px, 1920);
    assert_eq!(changed.display_height_px, 1080);
    assert!(changed.width % 2 == 0 && changed.height % 2 == 0);
    assert!(
        u64::from(changed.width) * u64::from(changed.height)
            <= u64::from(initial.width) * u64::from(initial.height)
    );
    assert!(changed.width <= 608);
    assert!(viewport_geom::aspect_matches(
        host_core::platform::PixelRect {
            x: crop.x,
            y: crop.y,
            w: crop.w,
            h: crop.h,
        },
        (changed.width, changed.height)
    ));
    host.wait_until(|| platform.viewport().region_captures().last().is_some())
        .await;
    let captures = platform.viewport().region_captures();
    let capture = captures.last().unwrap();
    assert_eq!(
        capture.rect,
        host_core::platform::PixelRect {
            x: crop.x,
            y: crop.y,
            w: crop.w,
            h: crop.h,
        }
    );
    assert_eq!(
        (capture.width, capture.height),
        (changed.width, changed.height)
    );
}

#[tokio::test]
async fn exit_after_hold() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_supported(true);
    let host = host(Some(platform)).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let initial = client.start_stream(start()).await;
    client.send(vp(1, initial.display_id, Some(A), true)).await;
    let initial_crop = config(&mut client).await;
    assert_eq!(initial_crop.viewport_seq, 1);
    assert_eq!(
        ack(&mut client).await.mode,
        pb::viewport_ack::Mode::Crop as i32
    );
    client.send(vp(2, initial.display_id, None, true)).await;
    let held = ack(&mut client).await;
    assert_eq!(held.mode, pb::viewport_ack::Mode::Crop as i32);
    host.clock.skip(Duration::from_millis(400));
    host.mgr.tick_all();
    let exited = config(&mut client).await;
    assert_eq!(exited.crop, None);
    assert_eq!(exited.epoch, initial.epoch + 2);
}

#[tokio::test]
async fn roi_without_crop() {
    let platform = FakePlatform::new();
    platform.viewport().set_roi_supported(true);
    let host = host(Some(platform.clone())).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let initial = client.start_stream(start()).await;
    client.send(vp(1, initial.display_id, Some(A), false)).await;
    assert_eq!(
        ack(&mut client).await.mode,
        pb::viewport_ack::Mode::Roi as i32
    );
    host.wait_until(|| platform.viewport().roi_calls().len() == 1)
        .await;
    let expected = viewport_geom::focus_regions(
        expand_roi(A, 1920, 1080),
        A,
        (1920, 1080),
        (initial.width, initial.height),
    );
    assert_eq!(platform.viewport().roi_calls().last(), Some(&expected));
    client.send(vp(2, initial.display_id, None, false)).await;
    assert_eq!(
        ack(&mut client).await.mode,
        pb::viewport_ack::Mode::Off as i32
    );
    host.wait_until(|| {
        platform
            .viewport()
            .roi_calls()
            .last()
            .is_some_and(Vec::is_empty)
    })
    .await;
}

#[tokio::test]
async fn stale_seq_dropped() {
    let host = host(None).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let config = client.start_stream(start()).await;
    client.send(vp(2, config.display_id, Some(A), false)).await;
    assert_eq!(ack(&mut client).await.seq, 2);
    client.send(vp(1, config.display_id, Some(A), true)).await;
    client
        .expect_none_matching(
            Duration::from_secs(1),
            |msg| matches!(msg, pb::envelope::Msg::ViewportAck(ack) if ack.seq == 1),
        )
        .await;
}

#[tokio::test]
async fn other_display_only_acks() {
    let host = host(None).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let config = client.start_stream(start()).await;
    client
        .send(vp(1, config.display_id + 99, Some(A), true))
        .await;
    assert_eq!(
        ack(&mut client).await.mode,
        pb::viewport_ack::Mode::Off as i32
    );
    no_viewport_or_config(&mut client, Duration::from_millis(200)).await;
}

#[tokio::test]
async fn crop_ok_false_never_crops() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_supported(true);
    let host = host(Some(platform)).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let config = client.start_stream(start()).await;
    let mut report = vp(1, config.display_id, Some(A), true);
    report.crop_ok = false;
    client.send(report).await;
    assert_eq!(
        ack(&mut client).await.mode,
        pb::viewport_ack::Mode::Off as i32
    );
    no_viewport_or_config(&mut client, Duration::from_millis(200)).await;
}

fn display(id: u32, primary: bool) -> DisplayInfo {
    DisplayInfo {
        id,
        name: format!("display-{id}"),
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        x: if primary { 0 } else { 1920 },
        y: 0,
        refresh_hz: 60,
        primary,
        rotation: 0,
    }
}

#[tokio::test]
async fn display_switch_resets() {
    let platform = FakePlatform::with_displays(vec![display(1, true), display(2, false)]);
    platform.viewport().set_region_supported(true);
    let host = host(Some(platform)).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let initial = client.start_stream(start()).await;
    client.send(vp(1, initial.display_id, Some(A), true)).await;
    let _ = config(&mut client).await;
    assert_eq!(
        ack(&mut client).await.mode,
        pb::viewport_ack::Mode::Crop as i32
    );
    client.send(pb::SelectDisplay { display_id: 2 }).await;
    let selected = config(&mut client).await;
    assert_eq!(selected.display_id, 2);
    assert_eq!(selected.crop, None);
}

#[tokio::test]
async fn region_failure_falls_back() {
    let platform = FakePlatform::new();
    platform.viewport().set_region_supported(true);
    platform.viewport().set_region_fails(true);
    let host = host(Some(platform)).await;
    let mut client = host.connect().await;
    client.hello_with(hello()).await;
    let initial = client.start_stream(start()).await;
    client.send(vp(1, initial.display_id, Some(A), true)).await;
    let fallback = config(&mut client).await;
    assert_eq!(
        ack(&mut client).await.mode,
        pb::viewport_ack::Mode::Off as i32
    );
    assert_eq!(fallback.crop, None);
    assert_eq!(fallback.epoch, initial.epoch + 1);
    client.send(vp(2, initial.display_id, Some(C), true)).await;
    let second = ack(&mut client).await;
    assert_ne!(second.mode, pb::viewport_ack::Mode::Crop as i32);
    host.clock.skip(Duration::from_secs(2));
    host.mgr.tick_all();
    no_viewport_or_config(&mut client, Duration::from_millis(100)).await;
}
