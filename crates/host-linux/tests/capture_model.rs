use std::io::Cursor;
use std::sync::{Arc, Mutex};

use host_core::capture_model::CursorModel;
use host_core::platform::{CaptureConfig, CapturedFrame, CursorUpdate};
use host_linux::capture_model::{
    argb32_to_bgra, cursor_pixels_from_spa, cursor_to_shape, display_id_for_name, BufferInfo,
    CaptureHub, CursorDeviation, CursorError, CursorPixels, FrameDecision, FrameGate, HubHandle,
    DEVIATION_PX, INJECT_SETTLE_US,
};
use rsp_core::pb;

fn decode_png(png: &[u8]) -> Vec<u8> {
    let decoder = png::Decoder::new(Cursor::new(png));
    let mut reader = decoder.read_info().expect("PNG header");
    let mut output = vec![0; reader.output_buffer_size().expect("PNG output size")];
    let info = reader.next_frame(&mut output).expect("PNG frame");
    output.truncate(info.buffer_size());
    output
}

fn info() -> BufferInfo {
    BufferInfo {
        chunk_size: 100,
        ..BufferInfo::default()
    }
}

#[test]
fn gate_first_frame_then_damage() {
    let mut gate = FrameGate::new();
    let mut first = info();
    first.damage_regions = Some(0);
    assert_eq!(gate.decide(&first), FrameDecision::Encode);
    assert_eq!(gate.decide(&first), FrameDecision::Skip);
    let mut cursor = first;
    cursor.cursor_changed = true;
    assert_eq!(gate.decide(&cursor), FrameDecision::CursorOnly);
    let mut damaged = first;
    damaged.damage_regions = Some(2);
    assert_eq!(gate.decide(&damaged), FrameDecision::Encode);
    let mut unknown_damage = first;
    unknown_damage.damage_regions = None;
    assert_eq!(gate.decide(&unknown_damage), FrameDecision::Encode);
}

#[test]
fn gate_corrupted_and_empty() {
    let mut gate = FrameGate::new();
    let mut corrupted = info();
    corrupted.header_flags = Some(1 << 1);
    assert_eq!(gate.decide(&corrupted), FrameDecision::Skip);
    assert_eq!(gate.decide(&info()), FrameDecision::Encode);

    let mut empty = info();
    empty.chunk_flags = 1 << 1;
    empty.cursor_changed = true;
    assert_eq!(gate.decide(&empty), FrameDecision::CursorOnly);
    let mut zero = info();
    zero.chunk_size = 0;
    assert_eq!(gate.decide(&zero), FrameDecision::Skip);

    gate.reset();
    let mut no_damage = info();
    no_damage.damage_regions = Some(0);
    assert_eq!(gate.decide(&no_damage), FrameDecision::Encode);
}

#[test]
fn cursor_shape_bgra_premultiplied() {
    let shape = cursor_to_shape(
        CursorPixels::Bgra,
        2,
        1,
        8,
        &[0, 0, 255, 255, 0, 0, 64, 128],
        5,
        -1,
        1.0,
        true,
    )
    .expect("cursor shape");
    assert_eq!(decode_png(&shape.png), [255, 0, 0, 255, 128, 0, 0, 128]);
    assert_eq!((shape.hot_x, shape.hot_y), (1, 0));
    assert_eq!(shape.id, rsp_core::cursor::shape_id(&shape.png));
}

#[test]
fn cursor_shape_formats() {
    let cases = [
        (CursorPixels::Rgba, [255, 0, 0, 255]),
        (CursorPixels::Argb, [255, 255, 0, 0]),
        (CursorPixels::Abgr, [255, 0, 0, 255]),
    ];
    for (format, pixels) in cases {
        let shape =
            cursor_to_shape(format, 1, 1, 4, &pixels, 0, 0, 1.0, false).expect("cursor shape");
        assert_eq!(decode_png(&shape.png), [255, 0, 0, 255]);
    }
    let shape = cursor_to_shape(
        CursorPixels::Bgra,
        1,
        1,
        4,
        &[0, 0, 64, 128],
        0,
        0,
        1.0,
        false,
    )
    .expect("cursor shape");
    assert_eq!(decode_png(&shape.png), [64, 0, 0, 128]);
    assert_eq!(cursor_pixels_from_spa(12), Some(CursorPixels::Bgra));
    assert_eq!(cursor_pixels_from_spa(11), Some(CursorPixels::Rgba));
    assert_eq!(cursor_pixels_from_spa(13), Some(CursorPixels::Argb));
    assert_eq!(cursor_pixels_from_spa(14), Some(CursorPixels::Abgr));
    assert_eq!(cursor_pixels_from_spa(99), None);
}

#[test]
fn cursor_shape_limits() {
    assert_eq!(
        cursor_to_shape(CursorPixels::Bgra, 0, 1, 0, &[], 0, 0, 1.0, false),
        Err(CursorError::Empty)
    );
    assert_eq!(
        cursor_to_shape(CursorPixels::Bgra, 257, 1, 1028, &[], 0, 0, 1.0, false),
        Err(CursorError::TooLarge)
    );
    assert_eq!(
        cursor_to_shape(CursorPixels::Bgra, 2, 1, 4, &[0; 8], 0, 0, 1.0, false),
        Err(CursorError::Truncated)
    );
    assert_eq!(
        cursor_to_shape(CursorPixels::Bgra, 2, 1, 8, &[0; 7], 0, 0, 1.0, false),
        Err(CursorError::Truncated)
    );
}

#[test]
fn argb32_conversion() {
    assert_eq!(argb32_to_bgra(&[0x80FF0000]), [0x00, 0x00, 0xFF, 0x80]);
}

#[test]
fn pointer_report_offsets() {
    let report =
        host_linux::capture_model::pointer_report(7, 0, true, (100, 50), (3, 4), (1920, 1080));
    assert_eq!(report.x, 97);
    assert_eq!(report.y, 46);
    assert_eq!(report.update_time, 1);
    assert_eq!(report.width, 1920);

    let shape = pb::CursorShape {
        id: 1,
        width: 8,
        height: 8,
        hot_x: 3,
        hot_y: 4,
        scale: 1.0,
        png: Vec::new(),
    };
    let mut model = CursorModel::new();
    model.on_shape(shape);
    let updates = model.on_report(&report);
    assert!(updates.iter().any(|update| {
        matches!(
            update,
            CursorUpdate::Position {
                display_id: 7,
                x,
                y,
                ..
            } if (*x - 100.0 / 1920.0).abs() < f32::EPSILON
                && (*y - 50.0 / 1080.0).abs() < f32::EPSILON
        )
    }));
}

#[test]
fn display_ids() {
    let dp = display_id_for_name("DP-1");
    assert_eq!(dp, display_id_for_name("DP-1"));
    assert_ne!(dp, display_id_for_name("HDMI-1"));
    assert!(dp <= 0x7FFF_FFFF);
    assert_ne!(dp, 0);
    assert_eq!(display_id_for_name(""), 0x011C9DC5);
}

fn frame() -> CapturedFrame {
    CapturedFrame {
        display_id: 1,
        width: 1,
        height: 1,
        capture_ts_us: 1,
        native: Arc::new(1_u32),
    }
}

fn config(fps: u32, max_width: u32, max_height: u32) -> CaptureConfig {
    CaptureConfig {
        fps,
        max_width,
        max_height,
    }
}

#[tokio::test]
async fn hub_fanout_and_remove() {
    let (tx1, mut rx1) = tokio::sync::mpsc::channel(1);
    let (tx2, mut rx2) = tokio::sync::mpsc::channel(1);
    let mut hub = CaptureHub::new();
    let (id1, first) = hub.add(1_u32, tx1, config(30, 0, 0));
    let (id2, second) = hub.add(1_u32, tx2, config(60, 0, 0));
    assert!(first);
    assert!(!second);
    assert_eq!(hub.deliver(&1, &frame()), 2);
    assert_eq!(hub.deliver(&1, &frame()), 2);
    assert_eq!(hub.dropped(), 2);
    assert!(rx1.try_recv().is_ok());
    assert!(rx2.try_recv().is_ok());
    drop(rx1);
    assert_eq!(hub.deliver(&1, &frame()), 1);
    assert!(hub.remove(&1, id2));
    assert!(!hub.remove(&1, id1));
}

#[test]
fn hub_effective_config() {
    let (tx1, _rx1) = tokio::sync::mpsc::channel(1);
    let (tx2, _rx2) = tokio::sync::mpsc::channel(1);
    let mut hub = CaptureHub::new();
    hub.add(1_u32, tx1, config(30, 1280, 720));
    hub.add(1_u32, tx2, config(60, 0, 0));
    assert_eq!(hub.effective(&1), Some(config(60, 0, 0)));

    let (tx3, _rx3) = tokio::sync::mpsc::channel(1);
    let (tx4, _rx4) = tokio::sync::mpsc::channel(1);
    let mut sized = CaptureHub::new();
    sized.add(1_u32, tx3, config(30, 1280, 720));
    sized.add(1_u32, tx4, config(60, 1920, 1080));
    assert_eq!(sized.effective(&1), Some(config(60, 1920, 1080)));
    assert_eq!(sized.effective(&2), None);
}

#[test]
fn hub_handle_runs_once() {
    let count = Arc::new(Mutex::new(0_u32));
    let first = Arc::clone(&count);
    let handle = HubHandle::new(move || *first.lock().expect("counter") += 1);
    drop(handle);
    assert_eq!(*count.lock().expect("counter"), 1);

    let second = Arc::clone(&count);
    let handle: Box<dyn host_core::platform::CaptureHandle> = Box::new(HubHandle::new(move || {
        *second.lock().expect("counter") += 1
    }));
    drop(handle);
    assert_eq!(*count.lock().expect("counter"), 2);
}

#[test]
fn deviation_ignores_injected_motion() {
    let t0 = 1_000_000;
    let mut deviation = CursorDeviation::new();
    deviation.on_injected(1, 100.0, 100.0, t0);
    assert!(!deviation.on_cursor(1, 100.0, 100.0, t0 + 10_000));
    assert!(!deviation.on_cursor(1, 101.0, 100.0, t0 + 200_000));
    assert!(deviation.on_cursor(1, 300.0, 300.0, t0 + 400_000));
    assert_eq!(DEVIATION_PX, 3.0);
}

#[test]
fn deviation_settle_window() {
    let t0 = 1_000_000;
    let mut deviation = CursorDeviation::new();
    deviation.on_injected(1, 0.0, 0.0, t0);
    assert!(!deviation.on_cursor(1, 500.0, 500.0, t0 + 100_000));
    assert_eq!(INJECT_SETTLE_US, 150_000);
}

#[test]
fn deviation_without_injection() {
    let mut deviation = CursorDeviation::new();
    assert!(!deviation.on_cursor(1, 0.0, 0.0, 1));
    assert!(!deviation.on_cursor(1, 2.0, 0.0, 2));
    assert!(deviation.on_cursor(1, 50.0, 0.0, 3));
    assert!(deviation.on_cursor(2, 50.0, 0.0, 4));
}
