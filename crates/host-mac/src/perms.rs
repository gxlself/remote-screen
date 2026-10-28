//! Screen-recording and accessibility permission queries/requests.

use host_core::platform::{PermissionKind, Permissions};

extern "C" {
    fn rs_screen_capture_allowed(request: i32) -> i32;
    fn rs_accessibility_trusted(prompt: i32) -> i32;
}

pub fn screen_capture_allowed(request: bool) -> bool {
    // SAFETY: the shim only queries or requests this process's permission.
    unsafe { rs_screen_capture_allowed(i32::from(request)) != 0 }
}

pub fn accessibility_trusted(prompt: bool) -> bool {
    // SAFETY: the shim only queries or requests this process's permission.
    unsafe { rs_accessibility_trusted(i32::from(prompt)) != 0 }
}

pub fn permissions() -> Permissions {
    Permissions {
        screen_recording: screen_capture_allowed(false),
        accessibility: accessibility_trusted(false),
    }
}

pub fn request(kind: PermissionKind) {
    match kind {
        PermissionKind::ScreenRecording => {
            let _ = screen_capture_allowed(true);
        }
        PermissionKind::Accessibility => {
            let _ = accessibility_trusted(true);
        }
    }
}
