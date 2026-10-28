//! Windows active-console, username, and lock-state query.

use std::mem::size_of;

use host_core::platform::ConsoleState;
use windows::core::PWSTR;
use windows::Win32::System::RemoteDesktop::{
    WTSFreeMemory, WTSGetActiveConsoleSessionId, WTSQuerySessionInformationW, WTSSessionInfoEx,
    WTSUserName, WTSINFOEXW, WTS_CURRENT_SERVER_HANDLE, WTS_INFO_CLASS,
};

use crate::console_model::{
    active_session_id, classify, parse_wts_user, wts_lock_flag, wts_utf16_len, ConsoleFacts,
    LockFlag,
};

/// Query the active console from the service in session 0, using WTS rather
/// than the service's own username or session.
pub fn console_facts() -> ConsoleFacts {
    // SAFETY: this query has no arguments or caller-owned resources.
    let session_id = active_session_id(unsafe { WTSGetActiveConsoleSessionId() });
    let Some(session) = session_id else {
        return ConsoleFacts {
            session_id,
            user: None,
            lock: LockFlag::Unknown,
        };
    };
    ConsoleFacts {
        session_id,
        user: query_user(session),
        lock: query_lock(session),
    }
}

/// Return the mapped host-core state for the active console.
pub fn console_state() -> ConsoleState {
    classify(&console_facts())
}

struct WtsBuffer {
    pointer: PWSTR,
    bytes: u32,
}

impl WtsBuffer {
    fn query(session: u32, class: WTS_INFO_CLASS) -> Option<Self> {
        let mut buffer = Self {
            pointer: PWSTR::null(),
            bytes: 0,
        };
        // SAFETY: both outputs are valid. The local server pseudo-handle must
        // not be closed; any output allocation is owned by WtsBuffer.
        unsafe {
            WTSQuerySessionInformationW(
                Some(WTS_CURRENT_SERVER_HANDLE),
                session,
                class,
                &mut buffer.pointer,
                &mut buffer.bytes,
            )
        }
        .ok()?;
        (!buffer.pointer.is_null()).then_some(buffer)
    }
}

impl Drop for WtsBuffer {
    fn drop(&mut self) {
        if !self.pointer.is_null() {
            // SAFETY: WTS allocated this buffer and ownership was never copied.
            unsafe { WTSFreeMemory(self.pointer.0.cast()) };
        }
    }
}

fn query_user(session: u32) -> Option<String> {
    let buffer = WtsBuffer::query(session, WTSUserName)?;
    let units = wts_utf16_len(buffer.bytes)?;
    // SAFETY: WTS returned a nonnull UTF-16 buffer of the reported size; the
    // RAII guard remains alive while the bounded slice is decoded.
    let units = unsafe { std::slice::from_raw_parts(buffer.pointer.0, units) };
    parse_wts_user(units)
}

fn query_lock(session: u32) -> LockFlag {
    let Some(buffer) = WtsBuffer::query(session, WTSSessionInfoEx) else {
        return LockFlag::Unknown;
    };
    if (buffer.bytes as usize) < size_of::<WTSINFOEXW>() {
        return LockFlag::Unknown;
    }
    // SAFETY: WTS returned a complete WTSINFOEXW header/union. Copy without
    // alignment assumptions; the union's contents are read only at level 1.
    let info = unsafe { buffer.pointer.0.cast::<WTSINFOEXW>().read_unaligned() };
    let flags = if info.Level == 1 {
        // SAFETY: Level 1 selects WTSInfoExLevel1 and the buffer contains the
        // full structure. Do not read the union for an unsupported level.
        unsafe { info.Data.WTSInfoExLevel1.SessionFlags }
    } else {
        -1
    };
    wts_lock_flag(info.Level, flags)
}
