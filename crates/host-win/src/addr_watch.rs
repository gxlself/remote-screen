#![cfg(windows)]

//! Thin Win32 address notification source. Keep the returned guard alive while
//! following NICs; service assembly supplies the rebind and firewall callbacks:
//!
//! ```text
//! let (tx, rx) = mpsc::channel(8);
//! let _watch = addr_watch::watch_addresses(tx)?;
//! tokio::spawn(follow_nics(rx, nics::nics, |nics| {
//!     /* HostListener::update(policy, nics);
//!        WinFirewall::sync_intranet (spawn_blocking) */
//! }, DEBOUNCE, MAX_DELAY, RESYNC));
//! ```

use std::ffi::c_void;

use host_core::platform::PlatformError;
use tokio::sync::mpsc::Sender;
use windows::Win32::Foundation::{HANDLE, NO_ERROR};
use windows::Win32::NetworkManagement::IpHelper::{
    CancelMibChangeNotify2, NotifyUnicastIpAddressChange, MIB_NOTIFICATION_TYPE,
    MIB_UNICASTIPADDRESS_ROW,
};
use windows::Win32::Networking::WinSock::AF_UNSPEC;

struct Ctx {
    tx: Sender<()>,
}

/// Owns an IP Helper registration and its stable callback context.
pub struct AddrWatch {
    handle: HANDLE,
    ctx: *mut Ctx,
}

// SAFETY: the registration can be cancelled from any non-callback thread.
// Ctx stays at a stable address, contains a thread-safe Sender, and is freed
// only after cancellation has waited for all callbacks to finish.
unsafe impl Send for AddrWatch {}

/// Subscribe to both IPv4 and IPv6 unicast address changes.
pub fn watch_addresses(tx: Sender<()>) -> Result<AddrWatch, PlatformError> {
    let ctx = Box::into_raw(Box::new(Ctx { tx }));
    let mut handle = HANDLE::default();
    // SAFETY: ctx remains valid until cancellation; the callback has the
    // required ABI, and handle is valid writable output storage.
    let status = unsafe {
        NotifyUnicastIpAddressChange(
            AF_UNSPEC,
            Some(callback),
            Some(ctx.cast::<c_void>()),
            false,
            &mut handle,
        )
    };
    if status != NO_ERROR {
        // SAFETY: a failed registration retains no callback context, so this
        // uniquely owned allocation can be reclaimed immediately.
        unsafe { drop(Box::from_raw(ctx)) };
        return Err(PlatformError::Platform(format!(
            "NotifyUnicastIpAddressChange: {}",
            status.0
        )));
    }
    Ok(AddrWatch { handle, ctx })
}

unsafe extern "system" fn callback(
    callercontext: *const c_void,
    _row: *const MIB_UNICASTIPADDRESS_ROW,
    _notificationtype: MIB_NOTIFICATION_TYPE,
) {
    // SAFETY: registration supplies the non-null Ctx allocation, which lives
    // until CancelMibChangeNotify2 has finished all callbacks.
    let ctx = unsafe { &*callercontext.cast::<Ctx>() };
    let _ = ctx.tx.try_send(());
}

impl Drop for AddrWatch {
    fn drop(&mut self) {
        // SAFETY: this guard uniquely owns the active registration. Drop never
        // runs within its callback, so waiting for callbacks cannot deadlock.
        let status = unsafe { CancelMibChangeNotify2(self.handle) };
        if status != NO_ERROR {
            tracing::warn!(
                code = status.0,
                "CancelMibChangeNotify2 failed; leaking callback context"
            );
            return;
        }
        // SAFETY: cancellation completed and guarantees no further callbacks;
        // the context is still the uniquely owned Box allocated at registration.
        unsafe { drop(Box::from_raw(self.ctx)) };
    }
}
