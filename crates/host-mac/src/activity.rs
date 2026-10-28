//! Local user activity event tap and its throttle/silence state machine.

use std::ffi::c_void;
use std::sync::mpsc::{self, RecvTimeoutError, SyncSender};
use std::thread;
use std::time::Duration;

use tokio::sync::mpsc::{error::TrySendError, Sender};
use tracing::warn;

pub const ACTIVE_THROTTLE: Duration = Duration::from_millis(200);
pub const SILENCE: Duration = Duration::from_secs(2);

type ActivityCallback = extern "C" fn(i32, *mut c_void);

extern "C" {
    fn rs_activity_watch_create(callback: ActivityCallback, ctx: *mut c_void) -> *mut c_void;
    fn rs_activity_watch_run(handle: *mut c_void) -> i32;
    fn rs_activity_watch_stop(handle: *mut c_void);
    fn rs_activity_watch_destroy(handle: *mut c_void);
}

struct CallbackContext {
    events: SyncSender<i32>,
}

extern "C" fn on_activity(value: i32, ctx: *mut c_void) {
    if ctx.is_null() {
        return;
    }
    // Native filtering removes Remote Screen's 0x5253-marked events.  This
    // callback only transfers a state marker and never logs event contents.
    // SAFETY: the native watcher retains this context until after its run loop
    // exits and no callback can run after that point.
    let context = unsafe { &*(ctx as *const CallbackContext) };
    let _ = context.events.try_send(value);
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct ActivityState {
    active: bool,
    last_event: Option<Duration>,
    last_emit: Option<Duration>,
}

impl ActivityState {
    pub fn on_event(&mut self, now: Duration) -> Option<bool> {
        let emit = if !self.active {
            self.active = true;
            true
        } else {
            self.last_emit
                .map(|last| now.saturating_sub(last) >= ACTIVE_THROTTLE)
                .unwrap_or(true)
        };
        self.last_event = Some(now);
        if emit {
            self.last_emit = Some(now);
            Some(true)
        } else {
            None
        }
    }

    pub fn on_tick(&mut self, now: Duration) -> Option<bool> {
        if self.active
            && self
                .last_event
                .map(|last| now.saturating_sub(last) >= SILENCE)
                .unwrap_or(false)
        {
            self.active = false;
            self.last_emit = None;
            return Some(false);
        }
        None
    }
}

pub fn watch(tx: Sender<bool>) {
    let (event_tx, event_rx) = mpsc::sync_channel(128);
    let context = Box::new(CallbackContext { events: event_tx });
    let context_addr = Box::into_raw(context) as usize;
    // SAFETY: the callback and context remain valid until the native watcher
    // has unregistered and returned from its run loop.
    let handle = unsafe { rs_activity_watch_create(on_activity, context_addr as *mut c_void) };
    let Some(handle) = (!handle.is_null()).then_some(handle) else {
        // SAFETY: creation failed, so native code did not retain the context.
        unsafe { drop(Box::from_raw(context_addr as *mut CallbackContext)) };
        warn!("failed to create macOS local activity watcher");
        return;
    };
    let handle_addr = handle as usize;
    let native = thread::spawn(move || {
        // SAFETY: this thread owns the native run-loop handle until run returns.
        let rc = unsafe { rs_activity_watch_run(handle_addr as *mut c_void) };
        unsafe {
            // SAFETY: run returned after the tap and its run-loop source ended.
            rs_activity_watch_destroy(handle_addr as *mut c_void);
            // SAFETY: native callbacks have drained before the run call returns.
            drop(Box::from_raw(context_addr as *mut CallbackContext));
        }
        rc
    });

    thread::spawn(move || {
        let origin = std::time::Instant::now();
        let mut state = ActivityState::default();
        let mut unavailable = false;
        let mut native_finished = false;
        let mut stop_requested = false;
        loop {
            if tx.is_closed() {
                // SAFETY: the handle stays alive until the native thread joins.
                unsafe { rs_activity_watch_stop(handle_addr as *mut c_void) };
                stop_requested = true;
                break;
            }
            let timeout = state
                .last_event
                .map(|last| SILENCE.saturating_sub(origin.elapsed().saturating_sub(last)))
                .unwrap_or(Duration::from_millis(200))
                .min(Duration::from_millis(200));
            match event_rx.recv_timeout(timeout) {
                Ok(-1) => {
                    unavailable = true;
                    warn!("local activity event tap unavailable; accessibility permission is required");
                    break;
                }
                Ok(_) => {
                    if let Some(active) = state.on_event(origin.elapsed()) {
                        if let Err(TrySendError::Closed(_)) = tx.try_send(active) {
                            // SAFETY: the handle stays alive until the native thread joins.
                            unsafe { rs_activity_watch_stop(handle_addr as *mut c_void) };
                            stop_requested = true;
                            break;
                        }
                    }
                }
                Err(RecvTimeoutError::Timeout) => {
                    if let Some(active) = state.on_tick(origin.elapsed()) {
                        if let Err(TrySendError::Closed(_)) = tx.try_send(active) {
                            // SAFETY: the handle stays alive until the native thread joins.
                            unsafe { rs_activity_watch_stop(handle_addr as *mut c_void) };
                            stop_requested = true;
                            break;
                        }
                    }
                }
                Err(RecvTimeoutError::Disconnected) => {
                    native_finished = true;
                    break;
                }
            }
        }
        if !unavailable && !native_finished && !stop_requested {
            // SAFETY: the handle stays alive until the native thread joins.
            unsafe { rs_activity_watch_stop(handle_addr as *mut c_void) };
        }
        let _ = native.join();
    });
}
