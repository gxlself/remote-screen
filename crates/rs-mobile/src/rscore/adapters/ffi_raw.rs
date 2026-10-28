//! rs_raw_* C ABI (rs_mobile.h, ).

use std::{ffi::c_void, slice, sync::Arc};

use crate::rscore::{
    ffi_session::{attach_on_worker, callback, guard, RsEvent, RsEventCb},
    runtime::core,
};

use super::{
    is_adapter_id,
    rawframe::{RawConsumer, RawRegistry},
};

#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct RsRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

pub type RsRawReadyCb = extern "C" fn(*mut c_void, u64, u32, u32);

pub const RS_PIX_BGRA8: u8 = 1;
pub const RS_PIX_RGBA8: u8 = 2;

struct FfiRaw {
    ctx: *mut c_void,
    on_ready: RsRawReadyCb,
    on_event: RsEventCb,
}

// SAFETY: As with FfiVideo, native callers keep ctx/callbacks valid through
// detach and permit worker-thread access. RawTap serializes every callback.
unsafe impl Send for FfiRaw {}
// SAFETY: The same native lifetime/thread contract applies, and callbacks are
// serialized by RawTap; this wrapper has no mutable state.
unsafe impl Sync for FfiRaw {}

impl RawConsumer for FfiRaw {
    fn ready(&self, seq: u64, width: u32, height: u32) {
        callback(|| (self.on_ready)(self.ctx, seq, width, height));
    }

    fn event(&self, event: &RsEvent) {
        callback(|| (self.on_event)(self.ctx, event));
    }
}

/// # Safety
/// `ctx` and the callbacks must stay valid until rs_raw_detach returns.
#[no_mangle]
pub unsafe extern "C" fn rs_raw_attach(
    id: u64,
    ctx: *mut c_void,
    on_ready: Option<RsRawReadyCb>,
    on_event: Option<RsEventCb>,
) -> bool {
    guard(|| {
        let (Some(on_ready), Some(on_event)) = (on_ready, on_event) else {
            return false;
        };
        if !is_adapter_id(id) {
            return false;
        }
        let Ok(core) = core() else {
            return false;
        };
        let Some(frame) = core.state::<RawRegistry>().get(id) else {
            return false;
        };
        if frame.is_closed() {
            return false;
        }
        let consumer = Arc::new(FfiRaw {
            ctx,
            on_ready,
            on_event,
        });
        attach_on_worker(&core, move || frame.attach(consumer))
    })
}

#[no_mangle]
pub extern "C" fn rs_raw_detach(id: u64) {
    guard(|| {
        if !is_adapter_id(id) {
            return;
        }
        if let Ok(core) = core() {
            if let Some(frame) = core.state::<RawRegistry>().get(id) {
                frame.detach();
            }
        }
    });
}

/// # Safety
/// `dst` must be writable for `dst_stride * (dst_h - 1) + dst_w * 4` bytes,
/// exclusively borrowed for this call. `copied` may be NULL; otherwise it must
/// be aligned, writable and not overlap `dst`.
#[no_mangle]
pub unsafe extern "C" fn rs_raw_copy(
    id: u64,
    since_seq: u64,
    dst: *mut u8,
    dst_stride: usize,
    dst_w: u32,
    dst_h: u32,
    pix: u8,
    copied: *mut RsRect,
) -> u64 {
    guard(|| {
        if dst.is_null() || dst_w == 0 || dst_h == 0 || !is_adapter_id(id) {
            return 0;
        }
        let Some(row_bytes) = (dst_w as usize).checked_mul(4) else {
            return 0;
        };
        if dst_stride < row_bytes {
            return 0;
        }
        let Some(len) = dst_stride
            .checked_mul(dst_h as usize - 1)
            .and_then(|bytes| bytes.checked_add(row_bytes))
            .filter(|len| *len <= isize::MAX as usize)
        else {
            return 0;
        };
        let Ok(core) = core() else {
            return 0;
        };
        let Some(frame) = core.state::<RawRegistry>().get(id) else {
            return 0;
        };
        // SAFETY: The caller guarantees exclusive writable storage of this
        // length. Arithmetic and the slice's isize::MAX limit were checked.
        let dst = unsafe { slice::from_raw_parts_mut(dst, len) };
        let (seq, rect) = frame.copy_into(since_seq, dst, dst_stride, dst_w, dst_h, pix);
        if !copied.is_null() {
            // SAFETY: A non-null copied is a writable, aligned RsRect, disjoint
            // from dst, by the caller's contract.
            unsafe { copied.write(rect) };
        }
        seq
    })
}
