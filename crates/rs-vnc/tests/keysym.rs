use rs_vnc::keysym::{
    char_to_keysym, ctrl_alt_del, hid_to_keysym, text_events, to_pixels, KeyTracker, PointerState,
};

#[test]
fn letters_and_digits() {
    assert_eq!(hid_to_keysym(0x04, false), Some(0x61));
    assert_eq!(hid_to_keysym(0x04, true), Some(0x41));
    assert_eq!(hid_to_keysym(0x1D, false), Some(0x7a));
    assert_eq!(hid_to_keysym(0x1E, true), Some(0x21));
    assert_eq!(hid_to_keysym(0x1F, true), Some(0x40));
    assert_eq!(hid_to_keysym(0x27, false), Some(0x30));
    assert_eq!(hid_to_keysym(0x27, true), Some(0x29));
}

#[test]
fn table_spot_checks() {
    let checks = [
        (0x04, false, 0x61),
        (0x1D, true, 0x5a),
        (0x1E, false, 0x31),
        (0x26, true, 0x28),
        (0x27, false, 0x30),
        (0x28, false, 0xff0d),
        (0x29, false, 0xff1b),
        (0x2A, false, 0xff08),
        (0x2B, false, 0xff09),
        (0x2C, false, 0x20),
        (0x2C, true, 0x20),
        (0x2D, false, 0x2d),
        (0x2D, true, 0x5f),
        (0x2E, false, 0x3d),
        (0x2E, true, 0x2b),
        (0x2F, false, 0x5b),
        (0x2F, true, 0x7b),
        (0x30, false, 0x5d),
        (0x30, true, 0x7d),
        (0x31, false, 0x5c),
        (0x31, true, 0x7c),
        (0x32, false, 0x23),
        (0x32, true, 0x7e),
        (0x33, false, 0x3b),
        (0x33, true, 0x3a),
        (0x34, false, 0x27),
        (0x34, true, 0x22),
        (0x35, false, 0x60),
        (0x35, true, 0x7e),
        (0x36, false, 0x2c),
        (0x36, true, 0x3c),
        (0x37, false, 0x2e),
        (0x37, true, 0x3e),
        (0x38, false, 0x2f),
        (0x38, true, 0x3f),
        (0x39, false, 0xffe5),
        (0x3A, false, 0xffbe),
        (0x45, false, 0xffc9),
        (0x46, false, 0xff61),
        (0x47, false, 0xff14),
        (0x48, false, 0xff13),
        (0x49, false, 0xff63),
        (0x4A, false, 0xff50),
        (0x4B, false, 0xff55),
        (0x4C, false, 0xffff),
        (0x4D, false, 0xff57),
        (0x4E, false, 0xff56),
        (0x4F, false, 0xff53),
        (0x50, false, 0xff51),
        (0x51, false, 0xff54),
        (0x52, false, 0xff52),
        (0x53, false, 0xff7f),
        (0x54, false, 0xffaf),
        (0x55, false, 0xffaa),
        (0x56, false, 0xffad),
        (0x57, false, 0xffab),
        (0x58, false, 0xff8d),
        (0x59, false, 0xffb1),
        (0x62, false, 0xffb0),
        (0x63, false, 0xffae),
        (0x64, false, 0x3c),
        (0x64, true, 0x3e),
        (0x65, false, 0xff67),
        (0x67, false, 0xffbd),
        (0x68, false, 0xffca),
        (0x73, false, 0xffd5),
        (0xE0, false, 0xffe3),
        (0xE1, false, 0xffe1),
        (0xE2, false, 0xffe9),
        (0xE3, false, 0xffeb),
        (0xE4, false, 0xffe4),
        (0xE5, false, 0xffe2),
        (0xE6, false, 0xffea),
        (0xE7, false, 0xffec),
    ];
    for (usage, shift, expected) in checks {
        assert_eq!(
            hid_to_keysym(usage, shift),
            Some(expected),
            "usage {usage:#04x}, shift {shift}"
        );
    }
}

#[test]
fn unmapped() {
    for usage in [0x00, 0x03, 0x66, 0x74, 0x86, 0xE8, 0x1234] {
        assert_eq!(hid_to_keysym(usage, false), None, "usage {usage:#04x}");
    }
}

#[test]
fn char_mapping() {
    assert_eq!(char_to_keysym('a'), Some(0x61));
    assert_eq!(char_to_keysym('é'), Some(0xe9));
    assert_eq!(char_to_keysym('中'), Some(0x0100_4e2d));
    assert_eq!(char_to_keysym('\n'), Some(0xff0d));
    assert_eq!(char_to_keysym('\u{1}'), None);
    assert_eq!(char_to_keysym('\u{85}'), None);
}

#[test]
fn text_events_crlf() {
    assert_eq!(
        text_events("a\r\nb"),
        [
            (true, 0x61),
            (false, 0x61),
            (true, 0xff0d),
            (false, 0xff0d),
            (true, 0x62),
            (false, 0x62)
        ]
    );
    assert_eq!(text_events("\u{1}x"), [(true, 0x78), (false, 0x78)]);
}

#[test]
fn ctrl_alt_del_sequence() {
    assert_eq!(
        ctrl_alt_del(),
        vec![
            (true, 0xffe3),
            (true, 0xffe9),
            (true, 0xffff),
            (false, 0xffff),
            (false, 0xffe9),
            (false, 0xffe3),
        ]
    );
}

#[test]
fn tracker_shift_symbol() {
    let mut tracker = KeyTracker::new();
    assert_eq!(
        tracker.key(0x1E, true, 0b10),
        [(true, 0xffe1), (true, 0x21)]
    );
    assert_eq!(
        tracker.key(0x1E, false, 0),
        [(false, 0xffe1), (false, 0x21)]
    );
}

#[test]
fn tracker_modifier_usage() {
    let mut tracker = KeyTracker::new();
    assert_eq!(tracker.key(0xE0, true, 0), [(true, 0xffe3)]);
    assert_eq!(tracker.key(0xE0, false, 0b1), [(false, 0xffe3)]);
}

#[test]
fn tracker_autorepeat() {
    let mut tracker = KeyTracker::new();
    assert_eq!(tracker.key(0x04, true, 0), [(true, 0x61)]);
    assert_eq!(tracker.key(0x04, true, 0), [(true, 0x61)]);
    assert_eq!(tracker.key(0x04, false, 0), [(false, 0x61)]);
    assert_eq!(tracker.key(0x04, false, 0), []);
}

#[test]
fn tracker_release_all() {
    let mut tracker = KeyTracker::new();
    tracker.key(0x05, true, 0b1001);
    tracker.key(0x04, true, 0b1001);
    assert_eq!(
        tracker.release_all(),
        [
            (false, 0x61),
            (false, 0x62),
            (false, 0xffe3),
            (false, 0xffeb)
        ]
    );
    assert_eq!(tracker.release_all(), []);
}

#[test]
fn to_pixels_rounding() {
    assert_eq!(to_pixels(0.5, 0.5, 1920, 1080), (960, 540));
    assert_eq!(to_pixels(-1.0, 2.0, 100, 100), (0, 99));
    assert_eq!(to_pixels(f32::NAN, 0.0, 10, 10), (0, 0));
    assert_eq!(to_pixels(0.3, 0.3, 0, 10), (0, 0));
}

#[test]
fn buttons_mask() {
    let mut pointer = PointerState::new();
    assert_eq!(pointer.button(0, true, 0.5, 0.5, 10, 10), [(1, 5, 5)]);
    assert_eq!(pointer.button(1, true, 0.5, 0.5, 10, 10), [(5, 5, 5)]);
    assert_eq!(pointer.button(0, false, 0.5, 0.5, 10, 10), [(4, 5, 5)]);
    assert_eq!(pointer.button(2, true, 0.5, 0.5, 10, 10), [(6, 5, 5)]);
    assert_eq!(pointer.button(3, true, 0.5, 0.5, 10, 10), []);
}

#[test]
fn scroll_pixels() {
    let mut pointer = PointerState::new();
    pointer.move_to(0.5, 0.5, 10, 10);
    assert_eq!(
        pointer.scroll(0.0, 25.0, false),
        [(8, 5, 5), (0, 5, 5), (8, 5, 5), (0, 5, 5)]
    );
    assert_eq!(pointer.scroll(0.0, 5.0, false), [(8, 5, 5), (0, 5, 5)]);
    assert_eq!(pointer.scroll(0.0, -10.0, false), [(16, 5, 5), (0, 5, 5)]);
    assert_eq!(pointer.scroll(10.0, 0.0, false), [(32, 5, 5), (0, 5, 5)]);
    assert_eq!(pointer.scroll(-10.0, 0.0, false), [(64, 5, 5), (0, 5, 5)]);
}

#[test]
fn scroll_lines_and_cap() {
    let mut pointer = PointerState::new();
    assert_eq!(pointer.scroll(0.0, 2.0, true).len(), 4);
    assert_eq!(pointer.scroll(0.0, 50.0, true).len(), 20);
    assert_eq!(pointer.scroll(0.0, 0.5, true), []);
}

#[test]
fn scroll_keeps_buttons() {
    let mut pointer = PointerState::new();
    pointer.button(0, true, 0.5, 0.5, 10, 10);
    assert_eq!(pointer.scroll(0.0, 10.0, false), [(9, 5, 5), (1, 5, 5)]);
}

#[test]
fn scroll_non_finite() {
    let mut pointer = PointerState::new();
    assert_eq!(pointer.scroll(f32::NAN, 1.0, false), []);
    assert_eq!(
        pointer.scroll(0.0, 25.0, false),
        [(8, 0, 0), (0, 0, 0), (8, 0, 0), (0, 0, 0)]
    );
    assert_eq!(pointer.scroll(0.0, f32::NAN, false), []);
    assert_eq!(
        pointer.scroll(0.0, 5.0, false),
        [(8, 0, 0), (0, 0, 0)],
        "non-finite input must preserve the existing remainder"
    );
    pointer.reset_scroll();
    assert_eq!(pointer.scroll(0.0, 5.0, false), []);
}

#[test]
fn pointer_release_all() {
    let mut pointer = PointerState::new();
    pointer.button(0, true, 0.5, 0.5, 10, 10);
    pointer.button(1, true, 0.5, 0.5, 10, 10);
    assert_eq!(pointer.release_all(), Some((0, 5, 5)));
    assert_eq!(pointer.release_all(), None);
}
