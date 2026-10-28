//! Input desktop queries and per-thread attachment for session workers.

use std::cell::RefCell;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use host_core::platform::PlatformError;
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, GENERIC_ALL, HANDLE};
use windows::Win32::System::StationsAndDesktops::{
    CloseDesktop, GetThreadDesktop, GetUserObjectInformationW, OpenInputDesktop, SetThreadDesktop,
    DESKTOP_ACCESS_FLAGS, DESKTOP_CONTROL_FLAGS, DESKTOP_READOBJECTS, HDESK, UOI_NAME,
};
use windows::Win32::System::Threading::GetCurrentThreadId;

use crate::winproc_model::{desktop_name_from_utf16, DesktopTracker};

struct OwnedDesktop(HDESK);

impl Drop for OwnedDesktop {
    fn drop(&mut self) {
        // SAFETY: This handle was returned by OpenInputDesktop and has one owner.
        // An attached handle is detached before replacement/normal TLS cleanup.
        if let Err(error) = unsafe { CloseDesktop(self.0) } {
            tracing::warn!(%error, "CloseDesktop failed");
        }
    }
}

struct AttachedDesktop {
    // Borrowed from GetThreadDesktop; never pass this handle to CloseDesktop.
    original: HDESK,
    current: OwnedDesktop,
}

impl Drop for AttachedDesktop {
    fn drop(&mut self) {
        // SAFETY: TLS destruction runs on the same thread. The borrowed original
        // desktop remains valid for that thread; it is restored before current drops.
        if let Err(error) = unsafe { SetThreadDesktop(self.original) } {
            tracing::warn!(%error, "SetThreadDesktop during thread cleanup failed; remove windows/hooks before thread exit");
        }
    }
}

thread_local! {
    static ATTACHED: RefCell<Option<AttachedDesktop>> = const { RefCell::new(None) };
}

fn platform_error(api: &str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Platform(format!("{api}: {error}"))
}

fn open_input(access: DESKTOP_ACCESS_FLAGS) -> Result<OwnedDesktop, PlatformError> {
    // SAFETY: No pointers are passed; a successful handle is adopted immediately.
    unsafe { OpenInputDesktop(DESKTOP_CONTROL_FLAGS(0), false, access) }
        .map(OwnedDesktop)
        .map_err(|error| platform_error("OpenInputDesktop", error))
}

fn desktop_name(desktop: &OwnedDesktop) -> Result<String, PlatformError> {
    let mut bytes = 0;
    // SAFETY: This is the documented size query: a null buffer, length zero and
    // a live writable DWORD, against a desktop owned throughout both calls.
    let query = unsafe {
        GetUserObjectInformationW(HANDLE(desktop.0 .0), UOI_NAME, None, 0, Some(&mut bytes))
    };
    if let Err(error) = query {
        if error.code() != ERROR_INSUFFICIENT_BUFFER.to_hresult() {
            return Err(platform_error("GetUserObjectInformationW", error));
        }
    }
    let mut name = vec![0_u16; (bytes as usize).div_ceil(size_of::<u16>())];
    // SAFETY: name is aligned for UTF-16 and contains at least bytes writable
    // bytes. The owned desktop and buffer remain live throughout the call.
    unsafe {
        GetUserObjectInformationW(
            HANDLE(desktop.0 .0),
            UOI_NAME,
            Some(name.as_mut_ptr().cast()),
            bytes,
            None,
        )
    }
    .map_err(|error| platform_error("GetUserObjectInformationW", error))?;
    desktop_name_from_utf16(&name)
        .ok_or_else(|| platform_error("GetUserObjectInformationW", "invalid UTF-16 desktop name"))
}

pub fn input_desktop_name() -> Option<String> {
    desktop_name(&open_input(DESKTOP_READOBJECTS).ok()?).ok()
}

/// Attaches the calling OS thread, retaining the handle until its next switch.
/// SetThreadDesktop fails if this thread already owns windows or hooks: tear
/// those down before switching, then rebuild clipboard/hook threads.
/// Call from a dedicated OS thread, not a migrating Tokio task. Remove any
/// windows/hooks before thread exit so TLS cleanup can restore its original desktop.
pub fn attach_thread_to_input_desktop() -> Result<String, PlatformError> {
    let desktop = open_input(DESKTOP_ACCESS_FLAGS(GENERIC_ALL.0))?;
    let name = desktop_name(&desktop)?;
    ATTACHED.with(|slot| {
        let mut slot = slot.borrow_mut();
        let original = if let Some(attached) = slot.as_ref() {
            attached.original
        } else {
            // SAFETY: The query is for the calling thread; the result is borrowed,
            // stays on that thread, and is never closed by this module.
            unsafe { GetThreadDesktop(GetCurrentThreadId()) }
                .map_err(|error| platform_error("GetThreadDesktop", error))?
        };
        // SAFETY: desktop owns a live desktop handle. It is retained in TLS on
        // success and the previous handle is closed only after this switch.
        unsafe { SetThreadDesktop(desktop.0) }
            .map_err(|error| platform_error("SetThreadDesktop", error))?;
        if let Some(attached) = slot.as_mut() {
            attached.current = desktop;
        } else {
            *slot = Some(AttachedDesktop {
                original,
                current: desktop,
            });
        }
        Ok(name)
    })
}

/// Stops and joins the watcher on drop. Callbacks run on the watcher thread and
/// should return promptly; drop waits for an in-progress callback to finish.
pub struct DesktopWatch {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Drop for DesktopWatch {
    fn drop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            // A callback may release the last owner of its own watch. Do not
            // join the current thread; the stop message still ends its loop.
            if thread.thread().id() != thread::current().id() && thread.join().is_err() {
                tracing::warn!("desktop watcher panicked");
            }
        }
    }
}

pub fn spawn_desktop_watch(
    interval: Duration,
    on_change: impl Fn(String) + Send + 'static,
) -> DesktopWatch {
    let (stop, stopped) = mpsc::channel();
    let thread = thread::spawn(move || {
        let mut tracker = DesktopTracker::new();
        loop {
            if let Some(name) = tracker.observe(input_desktop_name().as_deref()) {
                on_change(name);
            }
            match stopped.recv_timeout(interval.max(Duration::from_millis(1))) {
                Err(mpsc::RecvTimeoutError::Timeout) => {}
                Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            }
        }
    });
    DesktopWatch {
        stop,
        thread: Some(thread),
    }
}
