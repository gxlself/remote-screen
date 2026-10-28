use host_core::inject::keymap_win::WinKey;
use host_core::inject::win_plan::{WinButton, WinInputOp};
use host_win::input_raw::{
    sas_policy_allows, to_raw, to_raw_all, RawInput, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, WAKE_NUDGE, XBUTTON1, XBUTTON2,
};

fn key(scan: u16, extended: bool, vk: u16, up: bool) -> WinInputOp {
    WinInputOp::Key {
        key: WinKey { scan, extended, vk },
        up,
    }
}

#[test]
fn scan_code_key() {
    assert_eq!(
        to_raw(&key(0x1E, false, 0, false)),
        RawInput::Key {
            vk: 0,
            scan: 0x1E,
            flags: KEYEVENTF_SCANCODE,
        }
    );
    assert_eq!(
        to_raw(&key(0x1E, false, 0, true)),
        RawInput::Key {
            vk: 0,
            scan: 0x1E,
            flags: KEYEVENTF_SCANCODE | KEYEVENTF_KEYUP,
        }
    );
}

#[test]
fn extended_and_virtual_keys() {
    assert_eq!(
        to_raw(&key(0x4D, true, 0, false)),
        RawInput::Key {
            vk: 0,
            scan: 0x4D,
            flags: KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY,
        }
    );
    assert_eq!(
        to_raw(&key(0x4D, true, 0, true)),
        RawInput::Key {
            vk: 0,
            scan: 0x4D,
            flags: KEYEVENTF_SCANCODE | KEYEVENTF_EXTENDEDKEY | KEYEVENTF_KEYUP,
        }
    );
    assert_eq!(
        to_raw(&key(0x45, false, 0x13, false)),
        RawInput::Key {
            vk: 0x13,
            scan: 0x45,
            flags: 0,
        }
    );
    assert_eq!(
        to_raw(&key(0x45, false, 0x13, true)),
        RawInput::Key {
            vk: 0x13,
            scan: 0x45,
            flags: KEYEVENTF_KEYUP,
        }
    );
}

#[test]
fn unicode_and_absolute_move() {
    assert_eq!(
        to_raw(&WinInputOp::Unicode {
            unit: 0x4F60,
            up: false,
        }),
        RawInput::Key {
            vk: 0,
            scan: 0x4F60,
            flags: KEYEVENTF_UNICODE,
        }
    );
    assert_eq!(
        to_raw(&WinInputOp::Unicode {
            unit: 0x4F60,
            up: true,
        }),
        RawInput::Key {
            vk: 0,
            scan: 0x4F60,
            flags: KEYEVENTF_UNICODE | KEYEVENTF_KEYUP,
        }
    );
    assert_eq!(
        to_raw(&WinInputOp::MoveAbs { x: 32785, y: 32798 }),
        RawInput::Mouse {
            dx: 32785,
            dy: 32798,
            mouse_data: 0,
            flags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        }
    );
}

#[test]
fn mouse_buttons_and_wheels() {
    let cases = [
        (WinButton::Left, false, MOUSEEVENTF_LEFTDOWN, 0),
        (WinButton::Left, true, MOUSEEVENTF_LEFTUP, 0),
        (WinButton::Right, false, MOUSEEVENTF_RIGHTDOWN, 0),
        (WinButton::Right, true, MOUSEEVENTF_RIGHTUP, 0),
        (WinButton::Middle, false, MOUSEEVENTF_MIDDLEDOWN, 0),
        (WinButton::Middle, true, MOUSEEVENTF_MIDDLEUP, 0),
        (WinButton::X1, false, MOUSEEVENTF_XDOWN, XBUTTON1),
        (WinButton::X2, true, MOUSEEVENTF_XUP, XBUTTON2),
    ];
    for (button, up, flags, mouse_data) in cases {
        assert_eq!(
            to_raw(&WinInputOp::Button { button, up }),
            RawInput::Mouse {
                dx: 0,
                dy: 0,
                mouse_data,
                flags,
            }
        );
    }
    assert_eq!(
        to_raw(&WinInputOp::Wheel { delta: -120 }),
        RawInput::Mouse {
            dx: 0,
            dy: 0,
            mouse_data: -120,
            flags: MOUSEEVENTF_WHEEL,
        }
    );
    assert_eq!(
        to_raw(&WinInputOp::HWheel { delta: 40 }),
        RawInput::Mouse {
            dx: 0,
            dy: 0,
            mouse_data: 40,
            flags: MOUSEEVENTF_HWHEEL,
        }
    );
}

#[test]
fn batch_and_policy_helpers() {
    let ops = [
        WinInputOp::MoveAbs { x: 1, y: 2 },
        WinInputOp::Wheel { delta: 3 },
        WinInputOp::Unicode { unit: 4, up: true },
    ];
    assert_eq!(to_raw_all(&ops), ops.iter().map(to_raw).collect::<Vec<_>>());

    assert!(sas_policy_allows(Some(1)));
    assert!(sas_policy_allows(Some(3)));
    for value in [None, Some(0), Some(2), Some(4)] {
        assert!(!sas_policy_allows(value));
    }
    assert_eq!(
        WAKE_NUDGE,
        RawInput::Mouse {
            dx: 0,
            dy: 0,
            mouse_data: 0,
            flags: MOUSEEVENTF_MOVE,
        }
    );
}
