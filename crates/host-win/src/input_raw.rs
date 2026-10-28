//! Platform-independent translation of planned Windows input operations.
//!
//! The values in this module mirror the Win32 `SendInput` flags, but the
//! module deliberately does not depend on the `windows` crate.  That keeps the
//! conversion logic testable on Linux and leaves the Windows layer as a thin
//! FFI adapter.

use host_core::inject::keymap_win::WinKey;
use host_core::inject::win_plan::{WinButton, WinInputOp};

pub const KEYEVENTF_EXTENDEDKEY: u32 = 0x0001;
pub const KEYEVENTF_KEYUP: u32 = 0x0002;
pub const KEYEVENTF_UNICODE: u32 = 0x0004;
pub const KEYEVENTF_SCANCODE: u32 = 0x0008;

pub const MOUSEEVENTF_MOVE: u32 = 0x0001;
pub const MOUSEEVENTF_LEFTDOWN: u32 = 0x0002;
pub const MOUSEEVENTF_LEFTUP: u32 = 0x0004;
pub const MOUSEEVENTF_RIGHTDOWN: u32 = 0x0008;
pub const MOUSEEVENTF_RIGHTUP: u32 = 0x0010;
pub const MOUSEEVENTF_MIDDLEDOWN: u32 = 0x0020;
pub const MOUSEEVENTF_MIDDLEUP: u32 = 0x0040;
pub const MOUSEEVENTF_XDOWN: u32 = 0x0080;
pub const MOUSEEVENTF_XUP: u32 = 0x0100;
pub const MOUSEEVENTF_WHEEL: u32 = 0x0800;
pub const MOUSEEVENTF_HWHEEL: u32 = 0x1000;
pub const MOUSEEVENTF_VIRTUALDESK: u32 = 0x4000;
pub const MOUSEEVENTF_ABSOLUTE: u32 = 0x8000;

pub const XBUTTON1: i32 = 1;
pub const XBUTTON2: i32 = 2;

/// A Win32 keyboard or mouse input with the fields needed by the injector.
///
/// The Windows adapter fills `time` with zero and `dwExtraInfo` with
/// [`host_core::inject::win_plan::RS_EXTRA_INFO`] for every value.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RawInput {
    Key {
        vk: u16,
        scan: u16,
        flags: u32,
    },
    Mouse {
        dx: i32,
        dy: i32,
        mouse_data: i32,
        flags: u32,
    },
}

pub const WAKE_NUDGE: RawInput = RawInput::Mouse {
    dx: 0,
    dy: 0,
    mouse_data: 0,
    flags: MOUSEEVENTF_MOVE,
};

/// Convert one planned operation to the platform-independent raw shape.
pub fn to_raw(op: &WinInputOp) -> RawInput {
    match *op {
        WinInputOp::Key { key, up } => raw_key(key, up),
        WinInputOp::Unicode { unit, up } => RawInput::Key {
            vk: 0,
            scan: unit,
            flags: KEYEVENTF_UNICODE | key_up(up),
        },
        WinInputOp::MoveAbs { x, y } => RawInput::Mouse {
            dx: i32::from(x),
            dy: i32::from(y),
            mouse_data: 0,
            flags: MOUSEEVENTF_MOVE | MOUSEEVENTF_ABSOLUTE | MOUSEEVENTF_VIRTUALDESK,
        },
        WinInputOp::Button { button, up } => raw_button(button, up),
        WinInputOp::Wheel { delta } => RawInput::Mouse {
            dx: 0,
            dy: 0,
            mouse_data: delta,
            flags: MOUSEEVENTF_WHEEL,
        },
        WinInputOp::HWheel { delta } => RawInput::Mouse {
            dx: 0,
            dy: 0,
            mouse_data: delta,
            flags: MOUSEEVENTF_HWHEEL,
        },
    }
}

/// Convert a planned operation sequence without changing its order.
pub fn to_raw_all(ops: &[WinInputOp]) -> Vec<RawInput> {
    ops.iter().map(to_raw).collect()
}

fn raw_key(key: WinKey, up: bool) -> RawInput {
    let mut flags = key_up(up);
    if key.extended {
        flags |= KEYEVENTF_EXTENDEDKEY;
    }

    if key.vk == 0 {
        flags |= KEYEVENTF_SCANCODE;
    }

    RawInput::Key {
        vk: key.vk,
        scan: key.scan,
        flags,
    }
}

fn raw_button(button: WinButton, up: bool) -> RawInput {
    let (flags, mouse_data) = match button {
        WinButton::Left => (
            if up {
                MOUSEEVENTF_LEFTUP
            } else {
                MOUSEEVENTF_LEFTDOWN
            },
            0,
        ),
        WinButton::Right => (
            if up {
                MOUSEEVENTF_RIGHTUP
            } else {
                MOUSEEVENTF_RIGHTDOWN
            },
            0,
        ),
        WinButton::Middle => (
            if up {
                MOUSEEVENTF_MIDDLEUP
            } else {
                MOUSEEVENTF_MIDDLEDOWN
            },
            0,
        ),
        WinButton::X1 => (
            if up {
                MOUSEEVENTF_XUP
            } else {
                MOUSEEVENTF_XDOWN
            },
            XBUTTON1,
        ),
        WinButton::X2 => (
            if up {
                MOUSEEVENTF_XUP
            } else {
                MOUSEEVENTF_XDOWN
            },
            XBUTTON2,
        ),
    };
    RawInput::Mouse {
        dx: 0,
        dy: 0,
        mouse_data,
        flags,
    }
}

const fn key_up(up: bool) -> u32 {
    if up {
        KEYEVENTF_KEYUP
    } else {
        0
    }
}

/// Return whether the SoftwareSASGeneration value permits a service to emit
/// Secure Attention Sequences.
pub const fn sas_policy_allows(value: Option<u32>) -> bool {
    matches!(value, Some(1 | 3))
}
