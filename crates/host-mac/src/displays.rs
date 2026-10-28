//! Display reconfiguration notifications and their 300 ms debounce policy.

use std::ffi::c_void;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::{Duration, Instant};

use host_core::platform::{DisplayInfo, Platform};
use tokio::sync::mpsc::{error::TrySendError, Sender};
use tracing::warn;

use crate::MacPlatform;

/// macOS sends this flag before a reconfiguration transaction starts.  The
/// corresponding end notification is the one that should wake the debounce
/// worker.  This value is `kCGDisplayBeginConfigurationFlag` from CoreGraphics.
pub const BEGIN_CONFIGURATION_FLAG: u32 = 1;
pub const DEBOUNCE: Duration = Duration::from_millis(300);

type DisplayCallback = extern "C" fn(u32, u32, *mut c_void);

extern "C" {
    fn rs_display_watch_create(callback: DisplayCallback, ctx: *mut c_void) -> *mut c_void;
    fn rs_display_watch_run(handle: *mut c_void) -> i32;
    fn rs_display_watch_stop(handle: *mut c_void);
    fn rs_display_watch_destroy(handle: *mut c_void);
}

struct CallbackContext {
    events: SyncSender<u32>,
}

extern "C" fn on_display_change(_display: u32, flags: u32, ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    // The native callback only performs a bounded, non-blocking send.  Listing
    // displays happens on the debounce worker below.
    // SAFETY: the native watcher retains this context until after its run loop
    // exits and no callback can run after that point.
    let context = unsafe { &*(ctx as *const CallbackContext) };
    let _ = context.events.try_send(flags);
}

/// Return the send times after applying the display watcher debounce policy.
/// `events` must be non-decreasing timestamps relative to one arbitrary clock.
pub fn debounced_send_times(events: &[Duration]) -> Vec<Duration> {
    let mut sends = Vec::new();
    let mut deadline = None;
    for &event in events {
        if let Some(previous) = deadline {
            if event >= previous {
                sends.push(previous);
            }
        }
        deadline = Some(event.saturating_add(DEBOUNCE));
    }
    if let Some(last) = deadline {
        sends.push(last);
    }
    sends
}

pub fn watch(platform: MacPlatform, tx: Sender<Vec<DisplayInfo>>) {
    let initial = platform.displays();
    if tx.try_send(initial).is_err() {
        return;
    }

    let (event_tx, event_rx) = mpsc::sync_channel(128);
    let context = Box::new(CallbackContext { events: event_tx });
    let context_addr = Box::into_raw(context) as usize;
    // SAFETY: the callback and context remain valid until the native watcher
    // has unregistered and returned from its run loop.
    let handle = unsafe { rs_display_watch_create(on_display_change, context_addr as *mut c_void) };
    let Some(handle) = (!handle.is_null()).then_some(handle) else {
        // The allocation is ours when native watcher creation fails.
        // SAFETY: creation failed, so native code did not retain the context.
        unsafe { drop(Box::from_raw(context_addr as *mut CallbackContext)) };
        warn!("failed to create macOS display reconfiguration watcher");
        return;
    };
    let handle_addr = handle as usize;

    let native = thread::spawn(move || {
        // SAFETY: this thread owns the native run-loop handle until run returns.
        let rc = unsafe { rs_display_watch_run(handle_addr as *mut c_void) };
        unsafe {
            // SAFETY: run returned only after unregistering the callback.
            rs_display_watch_destroy(handle_addr as *mut c_void);
            // SAFETY: native callbacks have drained before the run call returns.
            drop(Box::from_raw(context_addr as *mut CallbackContext));
        }
        rc
    });

    thread::spawn(move || {
        let mut deadline: Option<Instant> = None;
        loop {
            if tx.is_closed() {
                // SAFETY: the handle stays alive until the native thread joins.
                unsafe { rs_display_watch_stop(handle_addr as *mut c_void) };
                break;
            }
            let timeout = deadline
                .map(|at| at.saturating_duration_since(Instant::now()))
                .unwrap_or(Duration::from_millis(200));
            match event_rx.recv_timeout(timeout) {
                Ok(flags) => {
                    if flags & BEGIN_CONFIGURATION_FLAG == 0 {
                        deadline = Some(Instant::now() + DEBOUNCE);
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(at) = deadline {
                        if Instant::now() >= at {
                            deadline = None;
                            match tx.try_send(platform.displays()) {
                                Ok(()) | Err(TrySendError::Full(_)) => {}
                                Err(TrySendError::Closed(_)) => {
                                    // SAFETY: the handle stays alive until the native thread joins.
                                    unsafe { rs_display_watch_stop(handle_addr as *mut c_void) };
                                    break;
                                }
                            }
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    break;
                }
            }
        }
        let _ = native.join();
    });
}
