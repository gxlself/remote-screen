use rsp_client::viewport::{to_pixel_rect, ViewportInput, ViewportReporter};
use rsp_core::pb;

fn q(x: f32, y: f32, w: f32, h: f32, active: bool) -> ViewportInput {
    ViewportInput {
        x,
        y,
        w,
        h,
        view_w_px: 2532,
        view_h_px: 1170,
        active,
        crop_ok: true,
    }
}

fn rect(x: u32, y: u32, w: u32, h: u32) -> pb::PixelRect {
    pb::PixelRect { x, y, w, h }
}

#[test]
fn pixel_rect_quarter() {
    assert_eq!(
        to_pixel_rect(&q(0.25, 0.25, 0.5, 0.5, false), 1920, 1080),
        Some(rect(480, 256, 960, 560))
    );
}

#[test]
fn pixel_rect_full() {
    assert_eq!(
        to_pixel_rect(&q(0.0, 0.0, 1.0, 1.0, false), 1920, 1080),
        None
    );
    assert_eq!(
        to_pixel_rect(&q(0.01, 0.0, 0.985, 0.99, false), 1920, 1080),
        None
    );
}

#[test]
fn pixel_rect_clamped() {
    let input = q(0.9, 0.9, 0.5, 0.5, false)
        .sanitized()
        .expect("valid input");
    assert_eq!(
        to_pixel_rect(&input, 1920, 1080),
        Some(rect(1728, 960, 192, 120))
    );
}

#[test]
fn pixel_rect_other_display() {
    assert_eq!(
        to_pixel_rect(&q(0.25, 0.25, 0.5, 0.5, false), 2560, 1440),
        Some(rect(640, 352, 1280, 736))
    );
}

#[test]
fn sanitize_rejects() {
    assert!(q(f32::NAN, 0.0, 0.5, 0.5, false).sanitized().is_none());
    assert!(q(0.0, 0.0, 0.0, 0.5, false).sanitized().is_none());
    assert!(q(0.0, 0.0, 0.5, -1.0, false).sanitized().is_none());
    assert!(q(1.0, 0.0, 0.5, 0.5, false).sanitized().is_none());
}

#[test]
fn throttle_and_settle() {
    let mut reporter = ViewportReporter::new();
    reporter.on_stream_config(1, 1920, 1080);
    reporter.on_input(0, q(0.25, 0.25, 0.5, 0.5, true));

    let first = reporter.poll(0).expect("first viewport");
    assert_eq!(first.seq, 1);
    assert_eq!(first.rect, Some(rect(480, 256, 960, 560)));
    assert!(!first.settled);
    assert_eq!(first.view_width_px, 2528);
    assert_eq!(first.view_height_px, 1168);

    reporter.on_input(50_000, q(0.26, 0.25, 0.5, 0.5, true));
    assert!(reporter.poll(50_000).is_none());
    assert_eq!(reporter.deadline(), Some(100_000));

    let second = reporter.poll(100_000).expect("throttled viewport");
    assert_eq!(second.seq, 2);
    assert_eq!(second.rect, Some(rect(496, 256, 976, 560)));

    reporter.on_input(150_000, q(0.26, 0.25, 0.5, 0.5, false));
    assert!(reporter.poll(150_000).is_none());
    assert_eq!(reporter.deadline(), Some(400_000));
    assert!(reporter.poll(399_999).is_none());

    let settled = reporter.poll(400_000).expect("settled viewport");
    assert_eq!(settled.seq, 3);
    assert_eq!(settled.rect, Some(rect(496, 256, 976, 560)));
    assert!(settled.settled);
    assert!(reporter.poll(1_000_000).is_none());
    assert_eq!(reporter.deadline(), None);
}

#[test]
fn quantized_duplicates_suppressed() {
    let mut reporter = ViewportReporter::new();
    reporter.on_stream_config(1, 1920, 1080);
    reporter.on_input(0, q(0.25, 0.25, 0.5, 0.5, false));
    let first = reporter.poll(0).expect("first viewport");
    reporter.on_input(200_000, q(0.2501, 0.25, 0.5, 0.5, false));
    let settled = reporter.poll(5_000_000).expect("settled viewport");
    assert_eq!(settled.seq, first.seq + 1);
    assert!(settled.settled);
    assert_eq!(settled.rect, first.rect);
    assert!(reporter.poll(5_000_001).is_none());
}

#[test]
fn waits_for_stream_config() {
    let mut reporter = ViewportReporter::new();
    reporter.on_input(0, q(0.25, 0.25, 0.5, 0.5, false));
    assert!(reporter.poll(0).is_none());
    assert_eq!(reporter.deadline(), None);

    reporter.on_stream_config(3, 2560, 1440);
    let report = reporter.poll(0).expect("viewport after stream config");
    assert_eq!(report.display_id, 3);
    assert_eq!(report.rect, Some(rect(640, 352, 1280, 736)));
}

#[test]
fn display_switch_resends() {
    let mut reporter = ViewportReporter::new();
    reporter.on_stream_config(1, 1920, 1080);
    reporter.on_input(0, q(0.25, 0.25, 0.5, 0.5, true));
    assert_eq!(reporter.poll(0).expect("first").seq, 1);
    reporter.on_input(50_000, q(0.26, 0.25, 0.5, 0.5, true));
    assert_eq!(reporter.poll(100_000).expect("second").seq, 2);
    reporter.on_input(150_000, q(0.26, 0.25, 0.5, 0.5, false));
    assert_eq!(reporter.poll(400_000).expect("settled").seq, 3);

    reporter.on_stream_config(2, 3840, 2160);
    let report = reporter.poll(1_100_000).expect("display switch");
    assert_eq!(report.seq, 4);
    assert_eq!(report.display_id, 2);
    assert_eq!(report.rect, Some(rect(992, 528, 1936, 1104)));
    assert!(report.settled);
}

#[test]
fn crop_ok_change_sends() {
    let mut reporter = ViewportReporter::new();
    reporter.on_stream_config(1, 1920, 1080);
    reporter.on_input(0, q(0.25, 0.25, 0.5, 0.5, true));
    assert_eq!(reporter.poll(0).expect("first").seq, 1);

    let mut input = q(0.25, 0.25, 0.5, 0.5, true);
    input.crop_ok = false;
    reporter.on_input(100_000, input);
    let report = reporter.poll(100_000).expect("crop mode change");
    assert_eq!(report.seq, 2);
    assert!(!report.crop_ok);
}

#[test]
fn full_rect_has_no_rect() {
    let mut reporter = ViewportReporter::new();
    reporter.on_stream_config(1, 1920, 1080);
    reporter.on_input(0, q(0.0, 0.0, 1.0, 1.0, false));
    assert_eq!(reporter.poll(0).expect("full viewport").rect, None);
}

#[test]
fn ack_tracking() {
    let mut reporter = ViewportReporter::new();
    reporter.on_stream_config(1, 1920, 1080);
    reporter.on_input(0, q(0.25, 0.25, 0.5, 0.5, true));
    assert_eq!(reporter.poll(0).expect("first").seq, 1);
    reporter.on_input(100_000, q(0.26, 0.25, 0.5, 0.5, true));
    assert_eq!(reporter.poll(100_000).expect("second").seq, 2);
    reporter.on_input(200_000, q(0.27, 0.25, 0.5, 0.5, true));
    assert_eq!(reporter.poll(200_000).expect("third").seq, 3);

    reporter.on_ack(&pb::ViewportAck {
        seq: 3,
        mode: pb::viewport_ack::Mode::Crop as i32,
    });
    assert_eq!(reporter.host_mode(), pb::viewport_ack::Mode::Crop);
    reporter.on_ack(&pb::ViewportAck {
        seq: 2,
        mode: pb::viewport_ack::Mode::Roi as i32,
    });
    assert_eq!(reporter.host_mode(), pb::viewport_ack::Mode::Crop);
    reporter.on_ack(&pb::ViewportAck {
        seq: 99,
        mode: pb::viewport_ack::Mode::Off as i32,
    });
    assert_eq!(reporter.host_mode(), pb::viewport_ack::Mode::Crop);
    reporter.on_ack(&pb::ViewportAck { seq: 3, mode: 77 });
    assert_eq!(reporter.host_mode(), pb::viewport_ack::Mode::Off);
}

#[test]
fn same_rect_active_toggle_no_send() {
    let mut reporter = ViewportReporter::new();
    reporter.on_stream_config(1, 1920, 1080);
    reporter.on_input(0, q(0.25, 0.25, 0.5, 0.5, true));
    assert!(!reporter.poll(0).expect("first").settled);
    reporter.on_input(100_000, q(0.25, 0.25, 0.5, 0.5, true));
    assert!(reporter.poll(100_000).is_none());
    reporter.on_input(200_000, q(0.25, 0.25, 0.5, 0.5, true));
    assert!(reporter.poll(200_000).is_none());
    reporter.on_input(300_000, q(0.25, 0.25, 0.5, 0.5, false));
    assert!(reporter.poll(300_000).is_none());
    let settled = reporter.poll(550_000).expect("settled viewport");
    assert!(settled.settled);
}
