use host_core::inject::keymap_win::hid_to_win;
use rs_rdp::desktop::{desktop_size, landscape_desktop_size};
use rs_rdp::input::InputMapper;
use rs_rdp::scancode::{ctrl_alt_end, hid_to_scancode};
use rs_rdp::types::{MouseButton, RdpInput};

#[test]
fn scancode_matches_host_core() {
    for usage in 0_u16..=u16::MAX {
        let expected = hid_to_win(usage)
            .and_then(|key| (key.vk == 0).then_some((key.scan as u8, key.extended)));
        assert_eq!(hid_to_scancode(usage), expected, "usage {usage:#06x}");
    }
}

#[test]
fn scancode_spots() {
    assert_eq!(hid_to_scancode(0x04), Some((0x1E, false)));
    assert_eq!(hid_to_scancode(0x28), Some((0x1C, false)));
    assert_eq!(hid_to_scancode(0x4F), Some((0x4D, true)));
    assert_eq!(hid_to_scancode(0x52), Some((0x48, true)));
    assert_eq!(hid_to_scancode(0x58), Some((0x1C, true)));
    assert_eq!(hid_to_scancode(0xE3), Some((0x5B, true)));
    assert_eq!(hid_to_scancode(0x48), None);
}

#[test]
fn ctrl_alt_end_sequence() {
    assert_eq!(
        ctrl_alt_end(),
        vec![
            RdpInput::Key {
                scancode: 0x1D,
                extended: false,
                down: true,
            },
            RdpInput::Key {
                scancode: 0x38,
                extended: false,
                down: true,
            },
            RdpInput::Key {
                scancode: 0x4F,
                extended: true,
                down: true,
            },
            RdpInput::Key {
                scancode: 0x4F,
                extended: true,
                down: false,
            },
            RdpInput::Key {
                scancode: 0x38,
                extended: false,
                down: false,
            },
            RdpInput::Key {
                scancode: 0x1D,
                extended: false,
                down: false,
            },
        ]
    );
}

#[test]
fn to_pixels() {
    let mapper = InputMapper::new(1920, 1080);
    assert_eq!(mapper.to_pixels(0.5, 0.5), (960, 540));
    assert_eq!(mapper.to_pixels(2.0, -1.0), (1919, 0));
}

#[test]
fn buttons() {
    let mut mapper = InputMapper::new(100, 100);
    assert_eq!(
        mapper.button(0, true, 0.5, 0.5),
        vec![
            RdpInput::MouseMove { x: 50, y: 50 },
            RdpInput::MouseButton {
                button: MouseButton::Left,
                down: true,
                x: 50,
                y: 50,
            },
        ]
    );
    assert_eq!(
        mapper.button(0, true, 0.25, 0.25),
        vec![RdpInput::MouseMove { x: 25, y: 25 }]
    );
    assert_eq!(
        mapper.button(3, true, 0.25, 0.25),
        vec![
            RdpInput::MouseMove { x: 25, y: 25 },
            RdpInput::MouseButton {
                button: MouseButton::X1,
                down: true,
                x: 25,
                y: 25,
            },
        ]
    );
    assert!(mapper.button(7, true, 0.5, 0.5).is_empty());
}

#[test]
fn scroll_pixels() {
    let mut mapper = InputMapper::new(100, 100);
    assert_eq!(
        mapper.scroll(0.0, 10.0, false),
        vec![RdpInput::Wheel {
            horizontal: false,
            units: 12,
        }]
    );
    assert!(mapper.scroll(0.0, 0.5, false).is_empty());
    assert_eq!(
        mapper.scroll(0.0, 0.5, false),
        vec![RdpInput::Wheel {
            horizontal: false,
            units: 1,
        }]
    );
    assert_eq!(
        mapper.scroll(10.0, 0.0, false),
        vec![RdpInput::Wheel {
            horizontal: true,
            units: -12,
        }]
    );
}

#[test]
fn scroll_lines_and_split() {
    let mut mapper = InputMapper::new(100, 100);
    assert_eq!(
        mapper.scroll(0.0, 10.0, true),
        vec![
            RdpInput::Wheel {
                horizontal: false,
                units: 255,
            },
            RdpInput::Wheel {
                horizontal: false,
                units: 145,
            },
        ]
    );
    let events = mapper.scroll(0.0, -100.0, true);
    assert_eq!(events.len(), 10);
    assert!(events.iter().all(|event| {
        *event
            == RdpInput::Wheel {
                horizontal: false,
                units: -255,
            }
    }));
}

#[test]
fn key_with_shift() {
    let mut mapper = InputMapper::new(100, 100);
    assert_eq!(
        mapper.key(0x04, true, 0b10),
        vec![
            RdpInput::Key {
                scancode: 0x2A,
                extended: false,
                down: true,
            },
            RdpInput::Key {
                scancode: 0x1E,
                extended: false,
                down: true,
            },
        ]
    );
    assert_eq!(
        mapper.key(0x04, false, 0),
        vec![
            RdpInput::Key {
                scancode: 0x2A,
                extended: false,
                down: false,
            },
            RdpInput::Key {
                scancode: 0x1E,
                extended: false,
                down: false,
            },
        ]
    );
}

#[test]
fn key_modifier_usage() {
    let mut mapper = InputMapper::new(100, 100);
    assert_eq!(
        mapper.key(0xE3, true, 0),
        vec![RdpInput::Key {
            scancode: 0x5B,
            extended: true,
            down: true,
        }]
    );
}

#[test]
fn text_utf16() {
    let mut mapper = InputMapper::new(100, 100);
    assert_eq!(
        mapper.text("aé"),
        vec![
            RdpInput::Unicode {
                code: 0x61,
                down: true,
            },
            RdpInput::Unicode {
                code: 0x61,
                down: false,
            },
            RdpInput::Unicode {
                code: 0xE9,
                down: true,
            },
            RdpInput::Unicode {
                code: 0xE9,
                down: false,
            },
        ]
    );
    assert_eq!(
        mapper.text("😀"),
        vec![
            RdpInput::Unicode {
                code: 0xD83D,
                down: true,
            },
            RdpInput::Unicode {
                code: 0xD83D,
                down: false,
            },
            RdpInput::Unicode {
                code: 0xDE00,
                down: true,
            },
            RdpInput::Unicode {
                code: 0xDE00,
                down: false,
            },
        ]
    );
    let line = mapper.text("a\r\nb");
    assert_eq!(
        line,
        vec![
            RdpInput::Unicode {
                code: 0x61,
                down: true,
            },
            RdpInput::Unicode {
                code: 0x61,
                down: false,
            },
            RdpInput::Key {
                scancode: 0x1C,
                extended: false,
                down: true,
            },
            RdpInput::Key {
                scancode: 0x1C,
                extended: false,
                down: false,
            },
            RdpInput::Unicode {
                code: 0x62,
                down: true,
            },
            RdpInput::Unicode {
                code: 0x62,
                down: false,
            },
        ]
    );
    assert!(mapper.text("\u{1}").is_empty());
}

#[test]
fn release_all_order() {
    let mut mapper = InputMapper::new(100, 100);
    mapper.key(0x05, true, 0);
    mapper.key(0x04, true, 0);
    mapper.key(0xE0, true, 0);
    mapper.button(0, true, 0.5, 0.5);
    mapper.button(1, true, 0.5, 0.5);
    let events = mapper.release_all();
    assert_eq!(
        events,
        vec![
            RdpInput::Key {
                scancode: 0x1E,
                extended: false,
                down: false,
            },
            RdpInput::Key {
                scancode: 0x30,
                extended: false,
                down: false,
            },
            RdpInput::Key {
                scancode: 0x1D,
                extended: false,
                down: false,
            },
            RdpInput::MouseButton {
                button: MouseButton::Left,
                down: false,
                x: 50,
                y: 50,
            },
            RdpInput::MouseButton {
                button: MouseButton::Right,
                down: false,
                x: 50,
                y: 50,
            },
            RdpInput::ReleaseAll,
        ]
    );
    assert_eq!(mapper.release_all(), vec![RdpInput::ReleaseAll]);
}

#[test]
fn desktop_examples() {
    // 852*3 = 2556, 393*3 = 1179; 1179*1920/2556 = 885, then the
    // aligned even height is 884.
    assert_eq!(desktop_size(852.0, 393.0, 3.0), (1920, 884));
    assert_eq!(desktop_size(1366.0, 1024.0, 2.0), (1920, 1438));
    assert_eq!(desktop_size(915.0, 412.0, 2.625), (1920, 864));
    assert_eq!(desktop_size(400.0, 300.0, 1.0), (800, 600));
    assert_eq!(desktop_size(393.0, 852.0, 3.0), (880, 1920));
    assert_eq!(desktop_size(1024.0, 1366.0, 2.0), (1432, 1920));
    assert_eq!(desktop_size(375.0, 667.0, 2.0), (744, 1334));
}

#[test]
fn desktop_bounds() {
    assert_eq!(desktop_size(10.0, 10.0, 1.0), (600, 600));
    assert_eq!(desktop_size(5000.0, 100.0, 1.0), (1920, 600));
    assert_eq!(desktop_size(0.0, 100.0, 2.0), (1280, 720));
    assert_eq!(desktop_size(f32::NAN, 100.0, 2.0), (1280, 720));
    assert_eq!(desktop_size(100.0, 100.0, -1.0), (1280, 720));
}

#[test]
fn landscape_desktop_examples() {
    // An upright phone asks for the same landscape desktop as a rotated one.
    assert_eq!(landscape_desktop_size(393.0, 852.0, 3.0), (1920, 884));
    assert_eq!(landscape_desktop_size(852.0, 393.0, 3.0), (1920, 884));
    assert_eq!(landscape_desktop_size(1024.0, 1366.0, 2.0), (1920, 1438));
    assert_eq!(landscape_desktop_size(375.0, 667.0, 2.0), (1328, 750));
    assert_eq!(landscape_desktop_size(10.0, 10.0, 1.0), (600, 600));
    assert_eq!(landscape_desktop_size(f32::NAN, 100.0, 2.0), (1280, 720));
}
