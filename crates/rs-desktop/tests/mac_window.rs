#![cfg(target_os = "macos")]

use rs_desktop::{
    mac_window::{
        decode_key, decode_pointer, decode_scroll, decode_window, factory, pointer_capture_code,
    },
    platform::{KeyInput, PointerCapture, PointerInput, ScrollInput, VideoCodec, WindowChange},
};

#[test]
fn decode_key_kinds() {
    assert_eq!(
        decode_key(0, 0x00, false, 0),
        Some(KeyInput::MacKey {
            vk: 0x00,
            down: true,
            repeat: false
        })
    );
    assert_eq!(
        decode_key(1, 0x00, true, 0),
        Some(KeyInput::MacKey {
            vk: 0x00,
            down: false,
            repeat: true
        })
    );
    assert_eq!(
        decode_key(2, 0x38, false, 0x20102),
        Some(KeyInput::MacFlags {
            vk: 0x38,
            flags: 0x20102
        })
    );
    assert_eq!(decode_key(9, 0, false, 0), None);
}

#[test]
fn decode_pointer_kinds() {
    assert_eq!(
        decode_pointer(0, 10.0, 20.0, 1.5, -2.0, 0, false, 0),
        Some(PointerInput::Move {
            x: 10.0,
            y: 20.0,
            dx: 1.5,
            dy: -2.0
        })
    );
    assert_eq!(
        decode_pointer(1, 10.0, 20.0, 0.0, 0.0, 4, true, 1),
        Some(PointerInput::Button {
            button: 4,
            down: true,
            x: 10.0,
            y: 20.0,
            clicks: 1
        })
    );
    assert_eq!(decode_pointer(1, 0.0, 0.0, 0.0, 0.0, 7, true, 1), None);
    assert_eq!(
        decode_pointer(2, 0.0, 0.0, 0.0, 0.0, 0, false, 0),
        Some(PointerInput::Leave)
    );
}

#[test]
fn decode_scroll_kinds() {
    match decode_scroll(0, f64::NAN, 2.0, 4, 8) {
        Some(ScrollInput::MacPrecise {
            dx,
            dy,
            phase,
            momentum,
        }) => {
            assert!(dx.is_nan());
            assert_eq!((dy, phase, momentum), (2.0, 4, 8));
        }
        other => panic!("unexpected decoded scroll: {other:?}"),
    }
    assert_eq!(
        decode_scroll(1, -1.0, 3.0, 99, 100),
        Some(ScrollInput::MacLines { dx: -1.0, dy: 3.0 })
    );
    assert_eq!(decode_scroll(9, 0.0, 0.0, 0, 0), None);
}

#[test]
fn decode_window_kinds() {
    assert_eq!(
        decode_window(0, 800.0, 600.0, 2.0),
        Some(WindowChange::Resized {
            width: 800.0,
            height: 600.0,
            scale: 2.0
        })
    );
    assert_eq!(
        decode_window(1, 1.0, 0.0, 0.0),
        Some(WindowChange::Focus(true))
    );
    assert_eq!(
        decode_window(4, 0.0, 0.0, 0.0),
        Some(WindowChange::CloseRequested)
    );
    assert_eq!(decode_window(9, 0.0, 0.0, 0.0), None);
}

#[test]
fn capture_codes() {
    assert_eq!(pointer_capture_code(PointerCapture::Free), 0);
    assert_eq!(pointer_capture_code(PointerCapture::Confine), 1);
    assert_eq!(pointer_capture_code(PointerCapture::Virtual), 2);
}

#[test]
fn factory_present() {
    let factory = factory().expect("Mac factory is present");
    assert!(factory.can_decode(VideoCodec::Hevc));
}
