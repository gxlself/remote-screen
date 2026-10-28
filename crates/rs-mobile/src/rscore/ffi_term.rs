//! Native terminal bytes; callbacks are serialized on rs-core workers.
//! Buffers are borrowed only during callbacks. NULL/0 signals terminal closure.

use std::{ffi::c_void, ptr, slice, sync::Arc};

use super::{
    ffi_session::{attach_on_worker, callback, guard},
    runtime::core,
    tables::{SshTable, TermOutput},
};

pub type RsTermOutputCb = extern "C" fn(*mut c_void, *const u8, usize);

struct FfiTerm {
    ctx: *mut c_void,
    on_output: RsTermOutputCb,
}

// SAFETY: Native callers promise ctx/callback validity until detach/replacement
// returns and allow access on library workers. TermTap serializes callbacks.
unsafe impl Send for FfiTerm {}
// SAFETY: Same native lifetime/thread contract; TermTap serializes callbacks.
unsafe impl Sync for FfiTerm {}

impl TermOutput for FfiTerm {
    fn output(&self, bytes: &[u8]) {
        callback(|| (self.on_output)(self.ctx, bytes.as_ptr(), bytes.len()));
    }
    fn closed(&self) {
        callback(|| (self.on_output)(self.ctx, ptr::null(), 0));
    }
}

/// Attach terminal output and replay buffered bytes on a core worker.
/// # Safety
/// ctx/callback must permit worker-thread access, remain valid until detach or
/// replacement returns, and not unwind or synchronously wait for the main thread.
/// Buffers are callback-only borrows; NULL/0 is the single close signal.
#[no_mangle]
pub unsafe extern "C" fn rs_term_attach(
    id: u64,
    ctx: *mut c_void,
    on_output: Option<RsTermOutputCb>,
) -> bool {
    guard(|| {
        let Some(on_output) = on_output else {
            return false;
        };
        let Ok(c) = core() else {
            return false;
        };
        let Some(entry) = c.state::<SshTable>().get(id) else {
            return false;
        };
        let out = Arc::new(FfiTerm { ctx, on_output });
        attach_on_worker(&c, move || entry.term.attach(out))
    })
}

/// Detach output; no later callback uses the previous ctx.
/// # Safety
/// No pointer arguments. A callback that calls detach still owns its current
/// borrowed buffer only until it returns.
#[no_mangle]
pub extern "C" fn rs_term_detach(id: u64) {
    guard(|| {
        if let Ok(c) = core() {
            if let Some(e) = c.state::<SshTable>().get(id) {
                e.term.detach();
            }
        }
    });
}

/// Copy terminal input; NULL or zero-length input is ignored.
/// # Safety
/// Non-NULL buf with nonzero len must point to len readable bytes in one
/// allocation, with len <= isize::MAX, for this call only.
#[no_mangle]
pub unsafe extern "C" fn rs_term_input(id: u64, buf: *const u8, len: usize) {
    guard(|| {
        if buf.is_null() || len == 0 || len > isize::MAX as usize {
            return;
        }
        if let Ok(c) = core() {
            if let Some(e) = c.state::<SshTable>().get(id) {
                // SAFETY: The caller guarantees the readable slice for this call.
                let bytes = unsafe { slice::from_raw_parts(buf, len) };
                e.term.input(bytes);
            }
        }
    });
}

/// Resize the PTY; zero dimensions and missing IDs are ignored.
/// # Safety
/// No pointer arguments or additional caller requirements.
#[no_mangle]
pub extern "C" fn rs_term_resize(id: u64, cols: u16, rows: u16) {
    guard(|| {
        if let Ok(c) = core() {
            if let Some(e) = c.state::<SshTable>().get(id) {
                e.term.resize(cols, rows);
            }
        }
    });
}
