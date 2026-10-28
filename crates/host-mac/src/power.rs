//! Display wake and the single shared no-idle-display-sleep assertion.

use host_core::platform::{PlatformError, Result};

extern "C" {
    fn rs_power_keep_awake(on: i32);
    fn rs_wake_display() -> i32;
    fn rs_lock_screen() -> i32;
}

/// The Objective-C shim combines capture references and this explicit platform
/// reference into one assertion, so either path can release without leaking a
/// second `IOPMAssertionID`.
pub fn keep_awake(on: bool) {
    // SAFETY: the shim owns the shared assertion state and accepts a scalar.
    unsafe { rs_power_keep_awake(i32::from(on)) }
}

pub fn wake_display() -> Result<()> {
    // SAFETY: the shim owns the IOPM assertion lifetime.
    let status = unsafe { rs_wake_display() };
    if status == 0 {
        Ok(())
    } else {
        Err(PlatformError::Platform(format!(
            "IOPMAssertionDeclareUserActivity failed ({status})"
        )))
    }
}

pub fn lock_screen() -> Result<()> {
    // SAFETY: the shim creates, marks, posts, and releases the CGEvents.
    let status = unsafe { rs_lock_screen() };
    if status == 0 {
        Ok(())
    } else {
        Err(PlatformError::Platform(format!(
            "failed to inject lock-screen shortcut ({status})"
        )))
    }
}
