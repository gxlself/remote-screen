//! ：Mac FFI，见 。

use std::{
    ffi::{c_char, c_void},
    slice,
};

/// A byte buffer allocated by Swift and owned by the Rust caller until `take`.
#[repr(C)]
pub(crate) struct RsDtBuf {
    pub(crate) ptr: *mut u8,
    pub(crate) len: usize,
}

impl RsDtBuf {
    pub(crate) const fn empty() -> Self {
        Self {
            ptr: std::ptr::null_mut(),
            len: 0,
        }
    }

    /// Copy the Swift-owned bytes and release the original allocation.
    pub(crate) fn take(self) -> Vec<u8> {
        let bytes = if self.ptr.is_null() || self.len == 0 {
            Vec::new()
        } else {
            // SAFETY: Swift returns a valid allocation for `len` bytes and the
            // pointer remains valid until `rs_dt_buf_free` below.
            unsafe { slice::from_raw_parts(self.ptr, self.len).to_vec() }
        };
        // SAFETY: `self` came from a Swift FFI output; its allocation pointer
        // is passed to Swift's matching free function exactly once after the
        // bytes have been copied.
        unsafe { rs_dt_buf_free(self.ptr) };
        bytes
    }
}

pub(crate) fn err_buf() -> [c_char; 256] {
    [0; 256]
}

pub(crate) fn err_string(buffer: &[c_char]) -> String {
    let bytes = buffer
        .iter()
        .take_while(|byte| **byte != 0)
        .map(|byte| *byte as u8)
        .collect::<Vec<_>>();
    String::from_utf8_lossy(&bytes).into_owned()
}

unsafe extern "C" {
    pub(crate) fn rs_dt_buf_free(buffer: *mut u8);
    pub(crate) fn rs_dt_se_available() -> bool;
    pub(crate) fn rs_dt_se_create(
        blob: *mut c_void,
        x963: *mut c_void,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    pub(crate) fn rs_dt_se_open(
        blob: *const u8,
        length: usize,
        x963: *mut c_void,
        error: *mut c_char,
        capacity: usize,
    ) -> *mut c_void;
    pub(crate) fn rs_dt_se_sign(
        handle: *mut c_void,
        message: *const u8,
        length: usize,
        signature: *mut c_void,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    pub(crate) fn rs_dt_se_close(handle: *mut c_void);
    pub(crate) fn rs_dt_kc_get(
        service: *const c_char,
        account: *const c_char,
        output: *mut c_void,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    pub(crate) fn rs_dt_kc_set(
        service: *const c_char,
        account: *const c_char,
        value: *const u8,
        length: usize,
        error: *mut c_char,
        capacity: usize,
    ) -> i32;
    #[allow(dead_code)]
    pub(crate) fn rs_dt_kc_delete(service: *const c_char, account: *const c_char) -> i32;
}
