//! Viewport-aware encoding end to end over host-fake (03 §5.5).
//!
//! These scenarios use the real rsp-client. `SessionHandle::set_viewport`
//! drives the reporter, the Host policy chooses crop or ROI, and the resulting
//! `StreamConfig` and `ViewportAck` return through the normal control stream.

use std::future::Future;
use std::time::Duration;

use host_core::pipeline::viewport::expand_crop;
use host_core::platform::PixelRect as HostRect;
use host_fake::harness::{
    app_identity, direct_params, open, wait_frames, FrameReport, TestHost, TestHostOptions,
};
use host_fake::FakePlatform;
use rsp_client::session::{
    EventStream, SessionEvent, SessionHandle, SessionParams, SessionStateKind,
};
use rsp_client::viewport::{to_pixel_rect, ViewportInput};
use rsp_core::trust::Permission;
use rsp_core::{features, pb};
use tokio::time::{timeout, Instant};

const BUDGET: Duration = Duration::from_secs(20);
const WAIT: Duration = Duration::from_secs(5);

const DISPLAY_W: u32 = 1920;
const DISPLAY_H: u32 = 1080;

/// The test viewport used for geometry and non-crop scenarios.
///
/// Its raw area is 960x560, so `zoom_of` is sqrt(1920*1080/(960*560)) ~= 1.964.
/// That is intentionally below the contract's CROP_ENTER_ZOOM of 2.0. Crop
/// scenarios use `crop_input` instead; see the task's final report.
fn zoom2() -> ViewportInput {
    ViewportInput {
        x: 0.25,
        y: 0.25,
        w: 0.5,
        h: 0.5,
        view_w_px: 2532,
        view_h_px: 1170,
        active: false,
        crop_ok: true,
    }
}

/// A slightly tighter viewport whose quantized area zoom is above 2.0, so it
/// exercises the crop path without changing the contract or Host policy.
fn crop_input() -> ViewportInput {
    ViewportInput {
        x: 0.26,
        y: 0.26,
        w: 0.48,
        h: 0.48,
        ..zoom2()
    }
}

fn full() -> ViewportInput {
    ViewportInput {
        x: 0.0,
        y: 0.0,
        w: 1.0,
        h: 1.0,
        ..zoom2()
    }
}

fn expected_crop() -> pb::PixelRect {
    let report = to_pixel_rect(&crop_input(), DISPLAY_W, DISPLAY_H)
        .expect("crop_input has a pixel rectangle");
    let task_report =
        to_pixel_rect(&zoom2(), DISPLAY_W, DISPLAY_H).expect("zoom2 has a pixel rect");
    assert_eq!(
        (task_report.x, task_report.y, task_report.w, task_report.h),
        (480, 256, 960, 560),
        "zoom2 converts to the documented display rectangle"
    );
    let expanded = expand_crop(
        HostRect {
            x: report.x,
            y: report.y,
            w: report.w,
            h: report.h,
        },
        DISPLAY_W,
        DISPLAY_H,
    );
    pb::PixelRect {
        x: expanded.x,
        y: expanded.y,
        w: expanded.w,
        h: expanded.h,
    }
}

fn same_rect(left: &pb::PixelRect, right: &pb::PixelRect) -> bool {
    (left.x, left.y, left.w, left.h) == (right.x, right.y, right.w, right.h)
}

fn crop_matches(config: &pb::StreamConfig, expected: &pb::PixelRect) -> bool {
    config
        .crop
        .as_ref()
        .is_some_and(|crop| same_rect(crop, expected))
}

async fn within_budget<F: Future<Output = ()>>(test: F) {
    timeout(BUDGET, test)
        .await
        .expect("viewport scenario, including setup and cleanup, finishes within 20 s");
}

async fn start_host(platform: FakePlatform) -> TestHost {
    TestHost::start_with_platform(platform, TestHostOptions::default()).await
}

async fn open_client(
    host: &TestHost,
    permission: Permission,
    adjust: impl FnOnce(&mut SessionParams),
) -> (SessionHandle, EventStream) {
    let app = app_identity();
    host.pair(&app, permission);
    let mut options = direct_params(host, app);
    adjust(&mut options);
    timeout(WAIT, open(options))
        .await
        .expect("session opens within five seconds")
        .expect("open real rsp-client session")
}

async fn open_control(host: &TestHost) -> (SessionHandle, EventStream) {
    open_client(host, Permission::Control, |_| {}).await
}

async fn wait_streaming(events: &mut EventStream) {
    timeout(WAIT, async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::StateChanged(SessionStateKind::Streaming) => return,
                SessionEvent::Ended { code, kind, reason } => {
                    panic!("session ended before Streaming: {code:?} {kind:?} {reason}")
                }
                _ => {}
            }
        }
        panic!("event stream closed before Streaming");
    })
    .await
    .expect("session reaches Streaming within five seconds");
}

/// Consume events until the next `StreamConfig`, discarding everything else.
async fn next_config(events: &mut EventStream) -> pb::StreamConfig {
    timeout(WAIT, async {
        loop {
            match events.recv().await {
                Some(SessionEvent::StreamConfig(config)) => return config,
                Some(SessionEvent::Ended { code, kind, reason }) => {
                    panic!(
                        "session ended while waiting for StreamConfig: {code:?} {kind:?} {reason}"
                    )
                }
                Some(_) => {}
                None => panic!("event stream closed while waiting for StreamConfig"),
            }
        }
    })
    .await
    .expect("next StreamConfig arrives within five seconds")
}

#[derive(Debug, Default)]
struct Observed {
    report: FrameReport,
    configs: Vec<pb::StreamConfig>,
}

impl Observed {
    fn assert_healthy(&self) {
        assert!(
            self.report.ended.is_none(),
            "session ended during observation window: {:?}",
            self.report
        );
        assert!(
            self.report.errors.is_empty(),
            "session errors during observation window: {:?}",
            self.report
        );
    }

    fn assert_no_crop(&self) {
        assert!(
            self.configs.iter().all(|config| config.crop.is_none()),
            "no cropped StreamConfig during observation window: {:?}",
            self.configs
        );
    }
}

/// Drain events for a bounded window while recording frames and configs.
async fn observe(events: &mut EventStream, window: Duration) -> Observed {
    let started = Instant::now();
    let mut observed = Observed::default();
    let mut last_frame = None;
    let _ = timeout(window, async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::StreamConfig(config) => observed.configs.push(config),
                SessionEvent::Frame(frame) => {
                    let now = started.elapsed();
                    observed.report.frames += 1;
                    observed.report.keyframes += u64::from(frame.frame.keyframe);
                    observed.report.first_frame.get_or_insert(now);
                    if let Some(previous) = last_frame {
                        observed.report.max_gap = observed.report.max_gap.max(now - previous);
                    }
                    last_frame = Some(now);
                }
                SessionEvent::Stats(stats) => observed.report.last_stats = Some(stats),
                SessionEvent::StateChanged(state) => observed.report.states.push(state),
                SessionEvent::Error { code, message } => {
                    observed.report.errors.push((code, message))
                }
                SessionEvent::Ended { code, kind, .. } => {
                    observed.report.ended = Some((code, kind));
                    break;
                }
                _ => {}
            }
        }
    })
    .await;
    observed
}

fn assert_frames(report: &FrameReport, count: u64) {
    assert_eq!(report.frames, count, "frames keep flowing: {report:?}");
    assert!(report.errors.is_empty(), "{report:?}");
    assert!(report.ended.is_none(), "{report:?}");
}

async fn expect_frames(events: &mut EventStream, count: u64) {
    let report = wait_frames(events, count, WAIT).await;
    assert_frames(&report, count);
}

/// Wait for the first frame of `epoch`, then require `count` consecutive frames
/// from that epoch. Frames already queued from the previous epoch are ignored
/// because control and media datagrams are independent streams.
async fn expect_epoch_frames(events: &mut EventStream, epoch: u32, count: u64) {
    let epoch = u8::try_from(epoch).expect("stream epoch fits in the video header");
    timeout(WAIT, async {
        let mut seen = 0_u64;
        let mut started = false;
        loop {
            match events.recv().await {
                Some(SessionEvent::Frame(frame)) => {
                    if !started {
                        if frame.frame.epoch != epoch {
                            continue;
                        }
                        started = true;
                    } else {
                        assert_eq!(
                            frame.frame.epoch, epoch,
                            "all frames after the switch use epoch {epoch}"
                        );
                    }
                    seen += 1;
                    if seen >= count {
                        return;
                    }
                }
                Some(SessionEvent::StreamConfig(config)) => {
                    panic!("unexpected StreamConfig while collecting epoch frames: {config:?}")
                }
                Some(SessionEvent::Ended { code, kind, reason }) => {
                    panic!("session ended before epoch frames: {code:?} {kind:?} {reason}")
                }
                Some(SessionEvent::Error { code, message }) => {
                    panic!("session error before epoch frames: {code:?} {message}")
                }
                Some(_) => {}
                None => panic!("event stream closed before epoch frames"),
            }
        }
    })
    .await
    .unwrap_or_else(|_| panic!("epoch {epoch} delivers {count} frames within five seconds"));
}

async fn wait_until<T>(description: &str, mut check: impl FnMut() -> Option<T>) -> T {
    timeout(WAIT, async {
        loop {
            if let Some(value) = check() {
                return value;
            }
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{description} within five seconds"))
}

async fn wait_mode(handle: &SessionHandle, mode: pb::viewport_ack::Mode) {
    wait_until(&format!("viewport_mode becomes {mode:?}"), || {
        (handle.viewport_mode() == mode).then_some(())
    })
    .await;
}

async fn enter_crop(
    session: &SessionHandle,
    events: &mut EventStream,
    initial: &pb::StreamConfig,
) -> pb::StreamConfig {
    session.set_viewport(crop_input());
    let cropped = next_config(events).await;
    let expected = expected_crop();
    assert!(
        crop_matches(&cropped, &expected),
        "zoom in produces the expanded crop: {cropped:?}, expected={expected:?}"
    );
    assert_eq!(cropped.epoch, initial.epoch + 1);
    assert!(cropped.viewport_seq >= 1, "{cropped:?}");
    wait_mode(session, pb::viewport_ack::Mode::Crop).await;
    cropped
}

async fn stop(host: TestHost, session: SessionHandle) {
    session.close();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crop_round_trip() {
    within_budget(async {
        let platform = FakePlatform::new();
        platform.viewport().set_region_supported(true);
        let host = start_host(platform.clone()).await;
        let (session, mut events) = open_control(&host).await;
        wait_streaming(&mut events).await;

        let initial = next_config(&mut events).await;
        assert_eq!(initial.display_width_px, DISPLAY_W);
        assert_eq!(initial.display_height_px, DISPLAY_H);
        assert_eq!(initial.crop, None);
        expect_frames(&mut events, 5).await;

        let cropped = enter_crop(&session, &mut events, &initial).await;
        assert_eq!(session.viewport_mode(), pb::viewport_ack::Mode::Crop);
        expect_epoch_frames(&mut events, cropped.epoch, 10).await;

        let captures = platform.viewport().region_captures();
        assert_eq!(
            captures.len(),
            1,
            "one region capture for the cropped stream"
        );
        let capture = captures[0];
        let expected = expected_crop();
        assert!(
            same_rect(
                &pb::PixelRect {
                    x: capture.rect.x,
                    y: capture.rect.y,
                    w: capture.rect.w,
                    h: capture.rect.h,
                },
                &expected
            ),
            "region capture rectangle matches StreamConfig.crop: {capture:?} vs {expected:?}"
        );
        assert_eq!(capture.width, cropped.width);
        assert_eq!(capture.height, cropped.height);

        stop(host, session).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn zoom_out_restores_full() {
    within_budget(async {
        let platform = FakePlatform::new();
        platform.viewport().set_region_supported(true);
        let host = start_host(platform.clone()).await;
        let (session, mut events) = open_control(&host).await;
        wait_streaming(&mut events).await;

        let initial = next_config(&mut events).await;
        assert_eq!(initial.crop, None);
        expect_frames(&mut events, 5).await;

        let cropped = enter_crop(&session, &mut events, &initial).await;
        expect_epoch_frames(&mut events, cropped.epoch, 10).await;

        session.set_viewport(full());
        let restored = next_config(&mut events).await;
        assert_eq!(restored.crop, None, "{restored:?}");
        assert_eq!(restored.epoch, initial.epoch + 2);
        assert_eq!(restored.viewport_seq, 0);

        expect_epoch_frames(&mut events, restored.epoch, 10).await;
        wait_mode(&session, pb::viewport_ack::Mode::Off).await;

        stop(host, session).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn roi_only_platform() {
    within_budget(async {
        let platform = FakePlatform::new();
        platform.viewport().set_roi_supported(true);
        let host = start_host(platform.clone()).await;
        let (session, mut events) = open_control(&host).await;
        wait_streaming(&mut events).await;
        let initial = next_config(&mut events).await;
        assert_eq!(initial.crop, None);
        expect_frames(&mut events, 5).await;

        session.set_viewport(zoom2());
        wait_mode(&session, pb::viewport_ack::Mode::Roi).await;
        let calls = wait_until("ROI region is applied", || {
            let calls = platform.viewport().roi_calls();
            (!calls.is_empty()).then_some(calls)
        })
        .await;
        let last = calls.last().expect("ROI call was observed");
        assert_eq!(last[0].qp_delta, -6, "focus region uses QP -6: {last:?}");

        let observed = observe(&mut events, Duration::from_millis(1_500)).await;
        observed.assert_healthy();
        observed.assert_no_crop();
        assert!(
            observed.report.frames >= 10,
            "frames keep flowing without crop: {:?}",
            observed.report
        );
        assert_eq!(session.viewport_mode(), pb::viewport_ack::Mode::Roi);
        assert!(platform.viewport().region_captures().is_empty());

        stop(host, session).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn client_without_feature() {
    within_budget(async {
        let platform = FakePlatform::new();
        platform.viewport().set_region_supported(true);
        let host = start_host(platform.clone()).await;
        let (session, mut events) = open_client(&host, Permission::Control, |options| {
            options.features &= !features::VIEWPORT;
        })
        .await;
        wait_streaming(&mut events).await;
        let initial = next_config(&mut events).await;
        assert_eq!(initial.crop, None);
        expect_frames(&mut events, 5).await;

        session.set_viewport(zoom2());
        let observed = observe(&mut events, Duration::from_secs(2)).await;
        observed.assert_healthy();
        assert!(
            observed.configs.is_empty(),
            "legacy client receives no StreamConfig: {:?}",
            observed.configs
        );
        assert_eq!(session.viewport_mode(), pb::viewport_ack::Mode::Off);
        assert!(platform.viewport().region_captures().is_empty());
        assert!(
            observed.report.frames >= 10,
            "legacy client still receives frames: {:?}",
            observed.report
        );

        stop(host, session).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crop_ok_false_stays_full() {
    within_budget(async {
        let platform = FakePlatform::new();
        platform.viewport().set_region_supported(true);
        let host = start_host(platform.clone()).await;
        let (session, mut events) = open_control(&host).await;
        wait_streaming(&mut events).await;
        let initial = next_config(&mut events).await;
        assert_eq!(initial.crop, None);
        expect_frames(&mut events, 5).await;

        // Use the crop-capable geometry so a false `crop_ok` is the only
        // reason this viewport stays full-screen.
        let mut input = crop_input();
        input.crop_ok = false;
        session.set_viewport(input);
        let observed = observe(&mut events, Duration::from_secs(2)).await;
        observed.assert_healthy();
        observed.assert_no_crop();
        assert_eq!(session.viewport_mode(), pb::viewport_ack::Mode::Off);
        assert!(platform.viewport().region_captures().is_empty());
        assert!(
            observed.report.frames >= 10,
            "full stream continues: {:?}",
            observed.report
        );

        stop(host, session).await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn second_viewer_unaffected() {
    within_budget(async {
        let platform = FakePlatform::new();
        platform.viewport().set_region_supported(true);
        let host = start_host(platform.clone()).await;

        let (controller, mut controller_events) = open_control(&host).await;
        wait_streaming(&mut controller_events).await;
        let controller_initial = next_config(&mut controller_events).await;
        assert_eq!(controller_initial.crop, None);
        expect_frames(&mut controller_events, 5).await;

        let (viewer, mut viewer_events) = open_client(&host, Permission::Control, |options| {
            options.intent = pb::Intent::View;
            options.start.fps = 30;
        })
        .await;
        wait_streaming(&mut viewer_events).await;
        let viewer_initial = next_config(&mut viewer_events).await;
        assert_eq!(viewer_initial.crop, None);
        expect_frames(&mut viewer_events, 5).await;

        controller.set_viewport(crop_input());
        let (cropped, observed) = tokio::join!(
            next_config(&mut controller_events),
            observe(&mut viewer_events, Duration::from_millis(1_500)),
        );
        let expected = expected_crop();
        assert!(
            crop_matches(&cropped, &expected),
            "controller enters crop: {cropped:?}, expected={expected:?}"
        );
        assert_eq!(cropped.epoch, controller_initial.epoch + 1);
        wait_mode(&controller, pb::viewport_ack::Mode::Crop).await;

        observed.assert_healthy();
        observed.assert_no_crop();
        assert!(
            observed.report.frames >= 10,
            "viewer keeps receiving an uninterrupted full stream: {:?}",
            observed.report
        );
        assert_eq!(viewer.viewport_mode(), pb::viewport_ack::Mode::Off);

        viewer.close();
        stop(host, controller).await;
    })
    .await;
}
