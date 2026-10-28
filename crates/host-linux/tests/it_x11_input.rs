#![cfg(feature = "desktop-it")]

use std::time::{Duration, Instant};
use std::{env, thread};

use host_core::platform::DisplayInfo;
use host_linux::keymap::char_to_keysym;
use host_linux::x11_input::{X11Activity, X11Injector};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit};
use tokio::sync::mpsc;
use x11rb::connection::Connection;
use x11rb::protocol::xinput;
use x11rb::protocol::xproto;
use x11rb::protocol::xproto::ConnectionExt as XprotoConnectionExt;
use x11rb::protocol::xtest;
use x11rb::rust_connection::RustConnection;
use x11rb::COPY_DEPTH_FROM_PARENT;

const WAIT: Duration = Duration::from_secs(10);

fn assert_layer() {
    assert_eq!(
        env::var("RS_IT_LAYER").as_deref(),
        Ok("x11"),
        "run under crates/host-linux/tests/desktop/run.sh x11"
    );
}

fn display() -> DisplayInfo {
    DisplayInfo {
        id: 1,
        name: "x11-test".to_owned(),
        width_px: 1280,
        height_px: 720,
        scale: 1.0,
        x: 0,
        y: 0,
        refresh_hz: 60,
        primary: true,
        rotation: 0,
    }
}

fn test_window() -> (RustConnection, xproto::Window) {
    let (conn, screen_num) = x11rb::connect(None).expect("connect to X11");
    let screen = &conn.setup().roots[screen_num];
    let window = conn.generate_id().expect("allocate test window");
    let events = xproto::EventMask::KEY_PRESS
        | xproto::EventMask::KEY_RELEASE
        | xproto::EventMask::BUTTON_PRESS
        | xproto::EventMask::BUTTON_RELEASE
        | xproto::EventMask::POINTER_MOTION;
    conn.create_window(
        COPY_DEPTH_FROM_PARENT,
        window,
        screen.root,
        0,
        0,
        1280,
        720,
        0,
        xproto::WindowClass::INPUT_OUTPUT,
        screen.root_visual,
        &xproto::CreateWindowAux::new()
            .background_pixel(screen.white_pixel)
            .override_redirect(1_u32)
            .event_mask(events),
    )
    .expect("create test window")
    .check()
    .expect("create test window request");
    conn.map_window(window)
        .expect("map test window")
        .check()
        .expect("map test window request");
    conn.set_input_focus(xproto::InputFocus::PARENT, window, x11rb::CURRENT_TIME)
        .expect("focus test window")
        .check()
        .expect("focus test window request");
    conn.flush().expect("flush test window");
    (conn, window)
}

fn next_event(conn: &RustConnection) -> x11rb::protocol::Event {
    let deadline = Instant::now() + WAIT;
    loop {
        assert!(Instant::now() < deadline, "timed out waiting for X11 event");
        if let Some(event) = conn.poll_for_event().expect("poll X11 event") {
            return event;
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn next_key_event(conn: &RustConnection) -> (bool, u8, xproto::KeyButMask) {
    loop {
        match next_event(conn) {
            x11rb::protocol::Event::KeyPress(event) => return (true, event.detail, event.state),
            x11rb::protocol::Event::KeyRelease(event) => return (false, event.detail, event.state),
            _ => {}
        }
    }
}

fn next_button_event(conn: &RustConnection) -> (bool, u8, i16, i16) {
    loop {
        match next_event(conn) {
            x11rb::protocol::Event::ButtonPress(event) => {
                return (true, event.detail, event.root_x, event.root_y)
            }
            x11rb::protocol::Event::ButtonRelease(event) => {
                return (false, event.detail, event.root_x, event.root_y)
            }
            _ => {}
        }
    }
}

fn next_motion_event(conn: &RustConnection) {
    loop {
        if matches!(next_event(conn), x11rb::protocol::Event::MotionNotify(_)) {
            return;
        }
    }
}

#[test]
fn key_a() {
    assert_layer();
    let (conn, _window) = test_window();
    let injector = X11Injector::connect(None).expect("XTest connection");
    let info = display();
    injector.inject(
        &InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::NONE,
        },
        &info,
    );
    injector.inject(
        &InputMsg::Key {
            usage: 0x04,
            down: false,
            mods: Mods::NONE,
        },
        &info,
    );
    let (down, detail, _) = next_key_event(&conn);
    assert!(down);
    assert_eq!(detail, 38);
    let (down, detail, _) = next_key_event(&conn);
    assert!(!down);
    assert_eq!(detail, 38);
}

#[test]
fn text_with_shift_and_remap() {
    assert_layer();
    let (conn, _window) = test_window();
    let injector = X11Injector::connect(None).expect("XTest connection");
    let info = display();
    injector.inject(&InputMsg::Text("A€".to_owned()), &info);
    assert_eq!(injector.stats().remapped, 1);

    let mut key_events = Vec::new();
    while key_events.len() < 6 {
        let (down, detail, state) = next_key_event(&conn);
        key_events.push((down, detail, state));
    }
    assert!(key_events.iter().any(|(down, detail, state)| {
        *down && *detail == 38 && u16::from(*state) & u16::from(xproto::KeyButMask::SHIFT) != 0
    }));
    let euro = key_events
        .iter()
        .find(|(_, detail, _)| *detail != 38 && *detail != 50)
        .map(|(_, detail, _)| *detail)
        .expect("remapped key event");
    let min = conn.setup().min_keycode;
    let max = conn.setup().max_keycode;
    let mapping = xproto::get_keyboard_mapping(&conn, min, max - min + 1)
        .expect("read keyboard mapping")
        .reply()
        .expect("keyboard mapping reply");
    let width = usize::from(mapping.keysyms_per_keycode);
    let start = usize::from(euro - min) * width;
    assert_eq!(mapping.keysyms[start], char_to_keysym('€').unwrap());

    drop(injector);
    let mapping = xproto::get_keyboard_mapping(&conn, min, max - min + 1)
        .expect("read restored mapping")
        .reply()
        .expect("restored mapping reply");
    let start = usize::from(euro - min) * usize::from(mapping.keysyms_per_keycode);
    assert_eq!(mapping.keysyms[start], 0);
}

#[test]
fn click_position() {
    assert_layer();
    let (conn, _window) = test_window();
    let injector = X11Injector::connect(None).expect("XTest connection");
    injector.inject(
        &InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.5,
            y: 0.5,
            clicks: 1,
        },
        &display(),
    );
    next_motion_event(&conn);
    let (down, detail, x, y) = next_button_event(&conn);
    assert!(down);
    assert_eq!(detail, 1);
    assert_eq!((x, y), (640, 360));
    injector.release_all();
}

#[test]
fn wheel_buttons() {
    assert_layer();
    let (conn, _window) = test_window();
    let injector = X11Injector::connect(None).expect("XTest connection");
    injector.inject(
        &InputMsg::Scroll {
            dx: 0.0,
            dy: 3.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        },
        &display(),
    );
    let (down, detail, _, _) = next_button_event(&conn);
    assert!(down);
    assert_eq!(detail, 4);
    let (down, detail, _, _) = next_button_event(&conn);
    assert!(!down);
    assert_eq!(detail, 4);

    injector.inject(
        &InputMsg::Scroll {
            dx: -3.0,
            dy: 0.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        },
        &display(),
    );
    let (down, detail, _, _) = next_button_event(&conn);
    assert!(down);
    assert_eq!(detail, 7);
    let (down, detail, _, _) = next_button_event(&conn);
    assert!(!down);
    assert_eq!(detail, 7);
}

#[test]
fn release_all() {
    assert_layer();
    let (conn, _window) = test_window();
    let injector = X11Injector::connect(None).expect("XTest connection");
    let info = display();
    injector.inject(
        &InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::NONE,
        },
        &info,
    );
    injector.inject(
        &InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.5,
            y: 0.5,
            clicks: 1,
        },
        &info,
    );
    injector.release_all();
    let mut saw_key = false;
    let mut saw_button = false;
    let deadline = Instant::now() + WAIT;
    while !(saw_key && saw_button) {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for release events"
        );
        match next_event(&conn) {
            x11rb::protocol::Event::KeyRelease(event) if event.detail == 38 => saw_key = true,
            x11rb::protocol::Event::ButtonRelease(event) if event.detail == 1 => saw_button = true,
            _ => {}
        }
    }
}

#[test]
fn activity_excludes_ours() {
    assert_layer();
    let (tx, mut rx) = mpsc::channel(16);
    let activity = X11Activity::start(None, tx).expect("XI2 activity listener");
    let injector = X11Injector::connect(None).expect("XTest connection");
    let info = display();
    let (sync, _) = test_window();
    for position in 0..5 {
        injector.inject(
            &InputMsg::MouseMove {
                x: 0.1 + position as f32 * 0.1,
                y: 0.2,
            },
            &info,
        );
        sync.get_input_focus()
            .expect("XSync request")
            .reply()
            .expect("XSync reply");
    }
    let deadline = Instant::now() + WAIT;
    while activity.stats().ours < 5 {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for XTEST events"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert_eq!(activity.stats().local, 0);
    assert!(
        rx.try_recv().is_err(),
        "XTEST events must not emit activity"
    );

    let (conn, screen_num) = x11rb::connect(None).expect("connect for hierarchy");
    let change = xinput::HierarchyChange {
        len: 4,
        data: xinput::HierarchyChangeData::AddMaster(xinput::HierarchyChangeDataAddMaster {
            send_core: false,
            enable: true,
            name: b"rs-test".to_vec(),
        }),
    };
    xinput::xi_change_hierarchy(&conn, &[change])
        .expect("add test master")
        .check()
        .expect("add test master request");
    conn.flush().expect("flush hierarchy");
    let deadline = Instant::now() + WAIT;
    let mut pointer = None;
    while pointer.is_none() {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for test master"
        );
        let devices = xinput::xi_query_device(&conn, xinput::Device::ALL)
            .expect("query devices")
            .reply()
            .expect("query devices reply");
        pointer = devices.infos.into_iter().find_map(|device| {
            (device.name.starts_with(b"rs-test")
                && device.type_ == xinput::DeviceType::MASTER_POINTER)
                .then_some(device.deviceid)
        });
        if pointer.is_none() {
            thread::sleep(Duration::from_millis(10));
        }
    }
    let pointer = pointer.expect("test pointer device");
    let root = conn.setup().roots[screen_num].root;
    xtest::fake_input(
        &conn,
        xproto::MOTION_NOTIFY_EVENT,
        0,
        0,
        root,
        17,
        19,
        pointer as u8,
    )
    .expect("test-device motion")
    .ignore_error();
    conn.flush().expect("flush test-device motion");
    let deadline = Instant::now() + WAIT;
    while activity.stats().local < 1 {
        assert!(
            Instant::now() < deadline,
            "timed out waiting for local event"
        );
        thread::sleep(Duration::from_millis(10));
    }
    assert!(rx.try_recv().expect("local activity marker"));
}

#[test]
fn wake_ok() {
    assert_layer();
    let injector = X11Injector::connect(None).expect("XTest connection");
    let before = injector.stats().errors;
    injector.wake();
    assert_eq!(injector.stats().errors, before);
}
