#![cfg(windows)]

//! Thin Windows adapter for the platform-independent input planner.

use std::mem::size_of;
use std::sync::Mutex;

use host_core::inject::win_plan::{ScreenRect, WinPlanner, RS_EXTRA_INFO};
use host_core::platform::{DisplayInfo, InputMsg, PlatformError};
use tracing::{debug, warn};
use windows::core::{w, BOOL};
use windows::Win32::Foundation::GetLastError;
use windows::Win32::System::Power::{SetThreadExecutionState, ES_DISPLAY_REQUIRED};
use windows::Win32::System::Registry::{RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD};
use windows::Win32::System::Shutdown::LockWorkStation;
use windows::Win32::UI::Input::KeyboardAndMouse::{
    SendInput, INPUT, INPUT_0, INPUT_KEYBOARD, INPUT_MOUSE, KEYBDINPUT, KEYBD_EVENT_FLAGS,
    KEYEVENTF_EXTENDEDKEY as WIN_KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP as WIN_KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE as WIN_KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE as WIN_KEYEVENTF_UNICODE,
    MOUSEEVENTF_ABSOLUTE as WIN_MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL as WIN_MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN as WIN_MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP as WIN_MOUSEEVENTF_LEFTUP,
    MOUSEEVENTF_MIDDLEDOWN as WIN_MOUSEEVENTF_MIDDLEDOWN,
    MOUSEEVENTF_MIDDLEUP as WIN_MOUSEEVENTF_MIDDLEUP, MOUSEEVENTF_MOVE as WIN_MOUSEEVENTF_MOVE,
    MOUSEEVENTF_RIGHTDOWN as WIN_MOUSEEVENTF_RIGHTDOWN,
    MOUSEEVENTF_RIGHTUP as WIN_MOUSEEVENTF_RIGHTUP,
    MOUSEEVENTF_VIRTUALDESK as WIN_MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL as WIN_MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN as WIN_MOUSEEVENTF_XDOWN,
    MOUSEEVENTF_XUP as WIN_MOUSEEVENTF_XUP, MOUSEINPUT, MOUSE_EVENT_FLAGS, VIRTUAL_KEY,
};
use windows::Win32::UI::WindowsAndMessaging::{XBUTTON1 as WIN_XBUTTON1, XBUTTON2 as WIN_XBUTTON2};

use crate::displays::enumerate;
use crate::input_raw::{
    sas_policy_allows, to_raw_all, RawInput, KEYEVENTF_EXTENDEDKEY, KEYEVENTF_KEYUP,
    KEYEVENTF_SCANCODE, KEYEVENTF_UNICODE, MOUSEEVENTF_ABSOLUTE, MOUSEEVENTF_HWHEEL,
    MOUSEEVENTF_LEFTDOWN, MOUSEEVENTF_LEFTUP, MOUSEEVENTF_MIDDLEDOWN, MOUSEEVENTF_MIDDLEUP,
    MOUSEEVENTF_MOVE, MOUSEEVENTF_RIGHTDOWN, MOUSEEVENTF_RIGHTUP, MOUSEEVENTF_VIRTUALDESK,
    MOUSEEVENTF_WHEEL, MOUSEEVENTF_XDOWN, MOUSEEVENTF_XUP, WAKE_NUDGE, XBUTTON1, XBUTTON2,
};
use host_core::display_model::{rect_for, virtual_desk, MonitorRecord, RectPx};

// Keep the pure conversion constants synchronized with the SDK bindings.  A
// mismatch here would otherwise silently produce malformed INPUT structures.
const _: () = assert!(KEYEVENTF_EXTENDEDKEY == WIN_KEYEVENTF_EXTENDEDKEY.0);
const _: () = assert!(KEYEVENTF_KEYUP == WIN_KEYEVENTF_KEYUP.0);
const _: () = assert!(KEYEVENTF_UNICODE == WIN_KEYEVENTF_UNICODE.0);
const _: () = assert!(KEYEVENTF_SCANCODE == WIN_KEYEVENTF_SCANCODE.0);
const _: () = assert!(MOUSEEVENTF_MOVE == WIN_MOUSEEVENTF_MOVE.0);
const _: () = assert!(MOUSEEVENTF_LEFTDOWN == WIN_MOUSEEVENTF_LEFTDOWN.0);
const _: () = assert!(MOUSEEVENTF_LEFTUP == WIN_MOUSEEVENTF_LEFTUP.0);
const _: () = assert!(MOUSEEVENTF_RIGHTDOWN == WIN_MOUSEEVENTF_RIGHTDOWN.0);
const _: () = assert!(MOUSEEVENTF_RIGHTUP == WIN_MOUSEEVENTF_RIGHTUP.0);
const _: () = assert!(MOUSEEVENTF_MIDDLEDOWN == WIN_MOUSEEVENTF_MIDDLEDOWN.0);
const _: () = assert!(MOUSEEVENTF_MIDDLEUP == WIN_MOUSEEVENTF_MIDDLEUP.0);
const _: () = assert!(MOUSEEVENTF_XDOWN == WIN_MOUSEEVENTF_XDOWN.0);
const _: () = assert!(MOUSEEVENTF_XUP == WIN_MOUSEEVENTF_XUP.0);
const _: () = assert!(MOUSEEVENTF_WHEEL == WIN_MOUSEEVENTF_WHEEL.0);
const _: () = assert!(MOUSEEVENTF_HWHEEL == WIN_MOUSEEVENTF_HWHEEL.0);
const _: () = assert!(MOUSEEVENTF_VIRTUALDESK == WIN_MOUSEEVENTF_VIRTUALDESK.0);
const _: () = assert!(MOUSEEVENTF_ABSOLUTE == WIN_MOUSEEVENTF_ABSOLUTE.0);
const _: () = assert!(XBUTTON1 as u16 == WIN_XBUTTON1);
const _: () = assert!(XBUTTON2 as u16 == WIN_XBUTTON2);

/// Stateful Windows input injector.  The monitor cache is refreshed lazily if
/// a display id is not present, which handles hot-plug and topology changes
/// without making every input event enumerate monitors.
pub struct WinInjector {
    planner: Mutex<WinPlanner>,
    monitors: Mutex<Vec<MonitorRecord>>,
}

impl WinInjector {
    pub fn new() -> Self {
        Self {
            planner: Mutex::new(WinPlanner::for_new_process()),
            monitors: Mutex::new(enumerate()),
        }
    }

    /// Plan and inject one RSP input message as one `SendInput` call.
    pub fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        let (monitor, desk) = self.rects_for(display.id);
        let ops = {
            let mut planner = self
                .planner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            planner.plan(msg, monitor, desk)
        };
        send_raw(&to_raw_all(&ops));
    }

    /// Release all keys, modifiers and buttons tracked by the planner.
    pub fn release_all(&self) {
        let ops = {
            let mut planner = self
                .planner
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            planner.release_all()
        };
        send_raw(&to_raw_all(&ops));
    }

    pub fn lock_workstation(&self) -> Result<(), PlatformError> {
        // SAFETY: LockWorkStation has no pointer arguments and is safe to call
        // for the current interactive session; Windows performs permission
        // checks for the caller.
        unsafe { LockWorkStation() }
            .map_err(|error| PlatformError::Platform(format!("LockWorkStation: {error}")))
    }

    pub fn wake_display(&self) -> Result<(), PlatformError> {
        send_raw(&[WAKE_NUDGE]);

        // SAFETY: ES_DISPLAY_REQUIRED is a documented flag and does not carry
        // any pointers or borrowed handles.
        let previous = unsafe { SetThreadExecutionState(ES_DISPLAY_REQUIRED) };
        if previous.0 == 0 {
            // SAFETY: GetLastError reads the thread-local error from the
            // immediately preceding failed Win32 call.
            let error = unsafe { GetLastError() };
            return Err(PlatformError::Platform(format!(
                "SetThreadExecutionState: {:?}",
                error
            )));
        }
        Ok(())
    }

    fn rects_for(&self, display_id: u32) -> (ScreenRect, ScreenRect) {
        let mut records = self
            .monitors
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let mut monitor = rect_for(&records, display_id);
        if monitor.is_none() {
            *records = enumerate();
            monitor = rect_for(&records, display_id);
        }

        let desk = virtual_desk(&records);
        let monitor = match monitor {
            Some(rect) => rect,
            None => {
                debug!(display_id, "display id not found; using virtual desktop");
                desk
            }
        };
        (screen_rect(monitor), screen_rect(desk))
    }
}

impl Default for WinInjector {
    fn default() -> Self {
        Self::new()
    }
}

/// Send a Ctrl+Alt+Del equivalent from a service permitted by policy.
pub fn send_sas() -> Result<(), PlatformError> {
    if !sas_allowed() {
        return Err(PlatformError::PermissionDenied(
            "SoftwareSASGeneration policy does not allow services".to_owned(),
        ));
    }

    // SAFETY: SendSAS is a system API with a BOOL argument and no borrowed
    // pointers.  FALSE requests generation on behalf of the service account.
    unsafe { SendSAS(BOOL(0)) };
    Ok(())
}

/// Read SoftwareSASGeneration from the machine policy hive.
pub fn sas_allowed() -> bool {
    sas_policy_allows(software_sas_generation())
}

fn software_sas_generation() -> Option<u32> {
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: all strings are static NUL-terminated UTF-16 values generated by
    // `w!`; the output buffer is a valid writable DWORD and its byte size is
    // supplied to the API.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            w!("SOFTWARE\\Microsoft\\Windows\\CurrentVersion\\Policies\\System"),
            w!("SoftwareSASGeneration"),
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    if status == windows::Win32::Foundation::ERROR_SUCCESS && size == size_of::<u32>() as u32 {
        Some(value)
    } else {
        None
    }
}

fn screen_rect(rect: RectPx) -> ScreenRect {
    ScreenRect {
        left: rect.left,
        top: rect.top,
        width: rect.width,
        height: rect.height,
    }
}

fn send_raw(raw: &[RawInput]) {
    if raw.is_empty() {
        return;
    }
    let inputs: Vec<INPUT> = raw.iter().map(raw_to_input).collect();
    // SAFETY: each INPUT is fully initialized, the slice remains alive for the
    // duration of the synchronous call, and the size matches the SDK struct.
    let sent = unsafe { SendInput(&inputs, size_of::<INPUT>() as i32) };
    if sent < inputs.len() as u32 {
        // SAFETY: GetLastError reads the status of the immediately preceding
        // SendInput call on this thread.
        let error = unsafe { GetLastError() };
        warn!(sent, total = inputs.len(), err = ?error, "SendInput injected fewer events than requested");
    }
}

fn raw_to_input(raw: &RawInput) -> INPUT {
    match *raw {
        RawInput::Key { vk, scan, flags } => INPUT {
            r#type: INPUT_KEYBOARD,
            Anonymous: INPUT_0 {
                ki: KEYBDINPUT {
                    wVk: VIRTUAL_KEY(vk),
                    wScan: scan,
                    dwFlags: KEYBD_EVENT_FLAGS(flags),
                    time: 0,
                    dwExtraInfo: RS_EXTRA_INFO,
                },
            },
        },
        RawInput::Mouse {
            dx,
            dy,
            mouse_data,
            flags,
        } => INPUT {
            r#type: INPUT_MOUSE,
            Anonymous: INPUT_0 {
                mi: MOUSEINPUT {
                    dx,
                    dy,
                    mouseData: mouse_data as u32,
                    dwFlags: MOUSE_EVENT_FLAGS(flags),
                    time: 0,
                    dwExtraInfo: RS_EXTRA_INFO,
                },
            },
        },
    }
}

#[link(name = "sas", kind = "raw-dylib")]
unsafe extern "system" {
    fn SendSAS(as_user: BOOL);
}
