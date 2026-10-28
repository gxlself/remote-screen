//! Windows power requests, independent of the lifetime of the calling thread.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::Mutex;

use tracing::warn;
use windows::core::PWSTR;
use windows::Win32::Foundation::HANDLE;
use windows::Win32::System::Power::{
    PowerClearRequest, PowerCreateRequest, PowerRequestDisplayRequired, PowerRequestSystemRequired,
    PowerSetRequest,
};
use windows::Win32::System::SystemServices::POWER_REQUEST_CONTEXT_VERSION;
use windows::Win32::System::Threading::{
    POWER_REQUEST_CONTEXT_SIMPLE_STRING, REASON_CONTEXT, REASON_CONTEXT_0,
};

use host_core::activity_model::KeepAwakeLatch;

#[derive(Default)]
struct PowerState {
    latch: KeepAwakeLatch,
    request: Option<PowerRequest>,
}

/// Owns one shared display/system idle-sleep inhibitor.
#[derive(Default)]
pub struct WinPower {
    state: Mutex<PowerState>,
}

impl WinPower {
    pub fn new() -> Self {
        Self::default()
    }

    /// Set the sleep inhibitor; repeated successful on/off calls are ignored.
    pub fn set_keep_awake(&self, on: bool) {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.latch.set(on).is_none() {
            return;
        }
        if on {
            match PowerRequest::create() {
                Ok(request) => state.request = Some(request),
                Err(error) => {
                    let _ = state.latch.set(false);
                    warn!(%error, "failed to enable Windows keep-awake requests");
                }
            }
        } else {
            drop(state.request.take());
        }
    }
}

impl Drop for WinPower {
    fn drop(&mut self) {
        let state = self
            .state
            .get_mut()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let _ = state.latch.set(false);
        drop(state.request.take());
    }
}

struct PowerRequest {
    handle: OwnedHandle,
    display_set: bool,
    system_set: bool,
}

impl PowerRequest {
    fn create() -> windows::core::Result<Self> {
        let mut text: Vec<u16> = "Remote Host session".encode_utf16().chain([0]).collect();
        let reason = REASON_CONTEXT {
            Version: POWER_REQUEST_CONTEXT_VERSION,
            Flags: POWER_REQUEST_CONTEXT_SIMPLE_STRING,
            Reason: REASON_CONTEXT_0 {
                SimpleReasonString: PWSTR(text.as_mut_ptr()),
            },
        };
        // SAFETY: the simple-string discriminator matches the initialized
        // union member; Windows copies the terminated string during the call.
        let handle = unsafe { PowerCreateRequest(&reason)? };
        // SAFETY: PowerCreateRequest returned a uniquely owned, valid kernel
        // handle. OwnedHandle closes it and may safely move between threads.
        let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
        let mut request = Self {
            handle,
            display_set: false,
            system_set: false,
        };
        // SAFETY: request retains a valid power request handle throughout the
        // call. Drop releases it, including on a partial initialization error.
        unsafe { PowerSetRequest(request.raw(), PowerRequestDisplayRequired)? };
        request.display_set = true;
        // SAFETY: the same uniquely owned handle is still alive.
        unsafe { PowerSetRequest(request.raw(), PowerRequestSystemRequired)? };
        request.system_set = true;
        Ok(request)
    }

    fn raw(&self) -> HANDLE {
        HANDLE(self.handle.as_raw_handle())
    }
}

impl Drop for PowerRequest {
    fn drop(&mut self) {
        if self.display_set {
            // SAFETY: the handle remains alive and this request was set once.
            if let Err(error) =
                unsafe { PowerClearRequest(self.raw(), PowerRequestDisplayRequired) }
            {
                warn!(%error, "failed to clear the Windows display power request");
            }
        }
        if self.system_set {
            // SAFETY: the handle remains alive and this request was set once.
            if let Err(error) = unsafe { PowerClearRequest(self.raw(), PowerRequestSystemRequired) }
            {
                warn!(%error, "failed to clear the Windows system power request");
            }
        }
        // OwnedHandle closes the request after both requests are cleared.
    }
}
