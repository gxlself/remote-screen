#![cfg(feature = "desktop-it")]

use std::time::{Duration, Instant};

use host_core::platform::{CaptureConfig, CursorUpdate};
use host_linux::frame::LinuxFrame;
use host_linux::x11_capture::X11Capture;
use tokio::sync::mpsc;
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{ConnectionExt, CreateWindowAux, EventMask, WindowClass};
use x11rb::rust_connection::RustConnection;

fn check_layer() {
    assert_eq!(std::env::var("RS_IT_LAYER").as_deref(), Ok("x11"));
}

fn cfg(fps: u32) -> CaptureConfig {
    CaptureConfig {
        fps,
        max_width: 0,
        max_height: 0,
    }
}

async fn next_frame(
    rx: &mut mpsc::Receiver<host_core::platform::CapturedFrame>,
) -> host_core::platform::CapturedFrame {
    tokio::time::timeout(Duration::from_secs(10), rx.recv())
        .await
        .expect("frame timeout")
        .expect("capture sender closed")
}

#[tokio::test(flavor = "current_thread")]
async fn display_list() {
    check_layer();
    let capture = X11Capture::connect(None).expect("connect X11");
    let displays = capture.displays();
    assert_eq!(displays.len(), 1);
    assert_eq!((displays[0].width_px, displays[0].height_px), (1280, 720));
    assert!(displays[0].primary);
}

#[tokio::test(flavor = "current_thread")]
async fn first_frame_and_drawing() {
    check_layer();
    let capture = X11Capture::connect(None).expect("connect X11");
    let display = capture.displays().into_iter().next().expect("display");
    let (tx, mut rx) = mpsc::channel(4);
    let _handle = capture.capture(display.id, cfg(30), tx).expect("capture");
    let frame = next_frame(&mut rx).await;
    assert_eq!((frame.width, frame.height), (1280, 720));

    let (conn, screen_num) = RustConnection::connect(None).expect("test X11 connection");
    let screen = &conn.setup().roots[screen_num];
    let window = conn.generate_id().expect("window id");
    let gc = conn.generate_id().expect("gc id");
    conn.create_window(
        screen.root_depth,
        window,
        screen.root,
        50,
        50,
        0,
        0,
        0,
        WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &CreateWindowAux::new()
            .background_pixel(0x00ff0000)
            .event_mask(EventMask::EXPOSURE),
    )
    .expect("create window")
    .check()
    .expect("create window reply");
    conn.create_gc(gc, window, &Default::default())
        .expect("gc")
        .check()
        .expect("gc reply");
    conn.map_window(window)
        .expect("map")
        .check()
        .expect("map reply");
    conn.flush().expect("flush");

    let deadline = Instant::now() + Duration::from_secs(10);
    let mut found = false;
    while Instant::now() < deadline {
        let frame = next_frame(&mut rx).await;
        if let Some(linux) = LinuxFrame::of(&frame) {
            if let Some(row) = linux.row(125) {
                let pixel = &row[125 * 4..125 * 4 + 3];
                if pixel == [0x00, 0x00, 0xff] {
                    found = true;
                    break;
                }
            }
        }
    }
    assert!(found, "window background was not captured");
    let _ = conn.free_gc(gc);
    let _ = conn.destroy_window(window);
    let _ = conn.flush();
}

#[tokio::test(flavor = "current_thread")]
async fn idle_does_not_encode() {
    check_layer();
    let capture = X11Capture::connect(None).expect("connect X11");
    let display = capture.displays().into_iter().next().expect("display");
    let (tx, mut rx) = mpsc::channel(4);
    let _handle = capture.capture(display.id, cfg(30), tx).expect("capture");
    let _ = next_frame(&mut rx).await;
    let before = capture.stats(display.id).expect("stats");
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        let current = capture.stats(display.id).expect("stats");
        if current.idle_ticks >= before.idle_ticks + 3 {
            assert_eq!(current.encoded, before.encoded);
            assert!(rx.try_recv().is_err());
            return;
        }
        tokio::time::sleep(Duration::from_millis(20)).await;
    }
    panic!("idle ticks timeout");
}

#[tokio::test(flavor = "current_thread")]
async fn cursor_shape_and_position() {
    check_layer();
    let capture = X11Capture::connect(None).expect("connect X11");
    let display = capture.displays().into_iter().next().expect("display");
    let (tx, mut rx) = mpsc::channel(8);
    capture.watch_cursor(tx);
    let mut shape_seen = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline && !shape_seen {
        if let Ok(Some(CursorUpdate::Shape(shape))) =
            tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            shape_seen = shape.width > 0 && shape.height > 0;
        }
    }
    assert!(shape_seen, "cursor shape timeout");

    let (conn, screen_num) = RustConnection::connect(None).expect("pointer X11 connection");
    let root = conn.setup().roots[screen_num].root;
    conn.warp_pointer(x11rb::NONE, root, 0, 0, 0, 0, 640, 360)
        .expect("warp")
        .check()
        .expect("warp reply");
    conn.flush().expect("flush");
    let mut position_seen = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(CursorUpdate::Position {
            display_id,
            x,
            y,
            visible,
            ..
        })) = tokio::time::timeout(Duration::from_millis(100), rx.recv()).await
        {
            if display_id == display.id
                && visible
                && (x - 0.5).abs() < 0.03
                && (y - 0.5).abs() < 0.03
            {
                position_seen = true;
                break;
            }
        }
    }
    assert!(position_seen, "cursor position timeout");
}

#[tokio::test(flavor = "current_thread")]
async fn monitor_change() {
    check_layer();
    let capture = X11Capture::connect(None).expect("connect X11");
    let (tx, mut rx) = mpsc::channel(8);
    capture.watch_displays(tx);

    let create = std::process::Command::new("xrandr")
        .args(["--setmonitor", "rs-left", "640/0x720/0+0+0", "none"])
        .status()
        .expect("run xrandr");
    assert!(create.success(), "xrandr setmonitor failed: {create}");
    let mut found = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(list)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            if list
                .iter()
                .any(|info| info.name == "rs-left" && info.width_px == 640 && info.height_px == 720)
            {
                found = true;
                break;
            }
        }
    }
    let delete = std::process::Command::new("xrandr")
        .args(["--delmonitor", "rs-left"])
        .status()
        .expect("run xrandr delete");
    assert!(delete.success(), "xrandr delmonitor failed: {delete}");
    let mut restored = false;
    let deadline = Instant::now() + Duration::from_secs(10);
    while Instant::now() < deadline {
        if let Ok(Some(list)) = tokio::time::timeout(Duration::from_millis(200), rx.recv()).await {
            if !list.iter().any(|info| info.name == "rs-left") {
                restored = true;
                break;
            }
        }
    }
    assert!(found, "monitor add notification timeout");
    assert!(restored, "monitor delete notification timeout");
}

#[tokio::test(flavor = "current_thread")]
async fn no_shm_fallback() {
    check_layer();
    let executable = std::env::current_exe().expect("test executable");
    let status = std::process::Command::new(executable)
        .args(["--exact", "no_shm_child", "--nocapture"])
        .env("RS_X11_NO_SHM", "1")
        .status()
        .expect("run no-shm child");
    assert!(status.success(), "no-shm child failed: {status}");
}

#[tokio::test(flavor = "current_thread")]
async fn no_shm_child() {
    if std::env::var_os("RS_X11_NO_SHM").is_none() {
        return;
    }
    check_layer();
    let capture = X11Capture::connect(None).expect("connect X11");
    let display = capture.displays().into_iter().next().expect("display");
    assert!(!capture.stats(display.id).expect("stats").shm);
    let (tx, mut rx) = mpsc::channel(2);
    let _handle = capture.capture(display.id, cfg(30), tx).expect("capture");
    let frame = next_frame(&mut rx).await;
    assert_eq!(frame.width, 1280);
}

#[tokio::test(flavor = "current_thread")]
async fn drop_is_clean() {
    check_layer();
    let (tx, mut rx) = mpsc::channel(2);
    let capture = X11Capture::connect(None).expect("connect X11");
    let display = capture.displays().into_iter().next().expect("display");
    let handle = capture.capture(display.id, cfg(30), tx).expect("capture");
    let _ = next_frame(&mut rx).await;
    drop(handle);
    drop(capture);
    assert!(tokio::time::timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("receiver timeout")
        .is_none());
}
