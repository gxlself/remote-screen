//! Session queries used by the service and its UI pipe listener.

use windows::Win32::System::RemoteDesktop::{ProcessIdToSessionId, WTSGetActiveConsoleSessionId};

pub fn active_console_session() -> Option<u32> {
    // SAFETY: This query takes no pointers or handles.
    crate::winproc_model::console_session_id(unsafe { WTSGetActiveConsoleSessionId() })
}

pub fn process_session(pid: u32) -> Option<u32> {
    let mut session = 0;
    // SAFETY: The output points to a live, writable u32 for the duration of the call.
    unsafe { ProcessIdToSessionId(pid, &mut session) }.ok()?;
    Some(session)
}
