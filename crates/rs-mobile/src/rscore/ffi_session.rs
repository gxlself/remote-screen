//! Native session/video ABI for the mobile media pipeline.
//!
//! Video/cursor callbacks are serialized by VideoTap on rs-core workers. All
//! pointers are borrowed for the callback only. Native callbacks must not unwind
//! or synchronously wait for the main thread; detach waits for callbacks.

use std::{
    cell::Cell,
    ffi::{c_char, c_void, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr,
    sync::{
        atomic::{AtomicU8, Ordering},
        mpsc, Arc,
    },
};

use rsp_client::{session::SessionStateKind, CursorEvent, ReceivedFrame, SessionHandle};
use rsp_core::{
    hid::{Button, InputMsg, ScrollPhase, ScrollUnit, TouchPhase},
    nal::{hevc_to_avcc, split_annexb, HevcAccessUnit},
    pb,
};
use tracing::error;

use super::{
    adapters::{is_adapter_id, route as adapter_route},
    runtime::{core, Core},
    tables::{SessionTable, VideoConsumer},
};

pub const RS_EV_STREAM_CONFIG: i32 = 2;
pub const RS_EV_CLOCK_SYNC: i32 = 3;
pub const RS_EV_CLOSED: i32 = 5;
pub const RS_EV_CURSOR_POS: i32 = 6;
pub const RS_EV_CURSOR_SHAPE: i32 = 7;
pub const RS_EV_CURSOR_USE: i32 = 8;
pub const RS_EV_CURSOR_HIDDEN: i32 = 9;

/// Callback-only view of a frame; layout matches `include/rs_mobile.h`.
#[repr(C)]
pub struct RsFrame {
    pub frame_id: u32,
    pub keyframe: u8,
    pub clock_synced: u8,
    pub display_id: u8,
    pub epoch: u8,
    pub codec: u8,
    pub capture_ts_us: u64,
    pub capture_local_us: i64,
    pub received_us: u64,
    pub vps: *const u8,
    pub vps_len: usize,
    pub sps: *const u8,
    pub sps_len: usize,
    pub pps: *const u8,
    pub pps_len: usize,
    pub avcc: *const u8,
    pub avcc_len: usize,
}

/// Only fields documented for `kind` in the header are meaningful.
#[repr(C)]
pub struct RsEvent {
    pub kind: i32,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub display_id: u32,
    pub epoch: u32,
    pub codec: u8,
    pub cursor_visible: u8,
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub rtt_us: u64,
    pub offset_us: i64,
    pub code: i32,
    pub shape_id: u64,
    pub hot_x: u32,
    pub hot_y: u32,
    pub scale: f32,
    pub data: *const u8,
    pub data_len: usize,
    pub message: *const c_char,
    pub src_x: f32,
    pub src_y: f32,
    pub src_w: f32,
    pub src_h: f32,
    pub display_w: u32,
    pub display_h: u32,
}

pub type RsFrameCb = extern "C" fn(*mut c_void, *const RsFrame);
pub type RsEventCb = extern "C" fn(*mut c_void, *const RsEvent);

pub(super) fn guard<T: Default>(f: impl FnOnce() -> T) -> T {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        error!("panic contained at rs-mobile C ABI boundary");
        T::default()
    })
}

thread_local! {
    static IN_CALLBACK: Cell<bool> = const { Cell::new(false) };
}

pub(super) fn callback(f: impl FnOnce()) {
    struct Scope(bool);
    impl Drop for Scope {
        fn drop(&mut self) {
            IN_CALLBACK.set(self.0);
        }
    }
    let _scope = Scope(IN_CALLBACK.replace(true));
    guard(f);
}

// Taps replay synchronously. Wait for worker attachment to make ctx ownership
// and subsequent detach unambiguous. Reentrant attachment must stay on the
// callback thread to preserve the tap's reentrant attach/detach handling.
pub(super) fn attach_on_worker(c: &Core, f: impl FnOnce() -> bool + Send + 'static) -> bool {
    if IN_CALLBACK.get() {
        return guard(f);
    }
    let (tx, rx) = mpsc::sync_channel(1);
    c.handle().spawn_blocking(move || {
        let _ = tx.send(guard(f));
    });
    rx.recv().unwrap_or(false)
}

pub(super) fn empty_event(kind: i32) -> RsEvent {
    RsEvent {
        kind,
        width: 0,
        height: 0,
        fps: 0,
        display_id: 0,
        epoch: 0,
        codec: 0,
        cursor_visible: 0,
        cursor_x: 0.0,
        cursor_y: 0.0,
        rtt_us: 0,
        offset_us: 0,
        code: 0,
        shape_id: 0,
        hot_x: 0,
        hot_y: 0,
        scale: 0.0,
        data: ptr::null(),
        data_len: 0,
        message: ptr::null(),
        src_x: 0.0,
        src_y: 0.0,
        src_w: 0.0,
        src_h: 0.0,
        display_w: 0,
        display_h: 0,
    }
}

fn opt_slice(v: &Option<Vec<u8>>) -> (*const u8, usize) {
    match v.as_deref().filter(|v| !v.is_empty()) {
        Some(v) => (v.as_ptr(), v.len()),
        None => (ptr::null(), 0),
    }
}

/// Annex B H.264 → AVCC（长度前缀）+ SPS/PPS；电脑端 Mac 解码也用。
pub fn h264_to_avcc(data: &[u8]) -> HevcAccessUnit {
    let mut au = HevcAccessUnit::default();
    for nal in split_annexb(data) {
        match nal[0] & 0x1f {
            7 => au.sps = Some(nal.to_vec()),
            8 => au.pps = Some(nal.to_vec()),
            _ => {
                au.avcc.extend_from_slice(&(nal.len() as u32).to_be_bytes());
                au.avcc.extend_from_slice(nal);
                au.nal_count += 1;
            }
        }
    }
    au
}

struct FfiVideo {
    ctx: *mut c_void,
    on_frame: RsFrameCb,
    on_event: RsEventCb,
    codec: AtomicU8,
}

// SAFETY: Native callers keep ctx/callbacks valid through detach and permit
// worker-thread access. VideoTap serializes every callback for this consumer.
unsafe impl Send for FfiVideo {}
// SAFETY: Same lifetime/thread contract; codec is atomic and callbacks serialized.
unsafe impl Sync for FfiVideo {}

impl FfiVideo {
    fn event(&self, event: &RsEvent) {
        callback(|| (self.on_event)(self.ctx, event));
    }
}

impl VideoConsumer for FfiVideo {
    fn frame(&self, f: &ReceivedFrame) {
        guard(|| {
            let codec = self.codec.load(Ordering::Relaxed);
            let mut au = match pb::Codec::try_from(i32::from(codec)) {
                Ok(pb::Codec::Hevc) => hevc_to_avcc(&f.frame.data),
                Ok(pb::Codec::H264) => h264_to_avcc(&f.frame.data),
                _ => return,
            };
            if !f.frame.keyframe {
                au.vps = None;
                au.sps = None;
                au.pps = None;
            }
            let (vps, vps_len) = opt_slice(&au.vps);
            let (sps, sps_len) = opt_slice(&au.sps);
            let (pps, pps_len) = opt_slice(&au.pps);
            let frame = RsFrame {
                frame_id: f.frame.frame_id,
                keyframe: u8::from(f.frame.keyframe),
                clock_synced: u8::from(f.capture_local_us.is_some()),
                display_id: f.frame.display_id,
                epoch: f.frame.epoch,
                codec,
                capture_ts_us: f.frame.capture_ts_us,
                capture_local_us: f.capture_local_us.unwrap_or(0),
                received_us: f.received_us,
                vps,
                vps_len,
                sps,
                sps_len,
                pps,
                pps_len,
                avcc: au.avcc.as_ptr(),
                avcc_len: au.avcc.len(),
            };
            callback(|| (self.on_frame)(self.ctx, &frame));
        });
    }

    fn stream_config(&self, c: &pb::StreamConfig) {
        self.codec
            .store(u8::try_from(c.codec).unwrap_or(0), Ordering::Relaxed);
        let mut e = empty_event(RS_EV_STREAM_CONFIG);
        e.width = c.width;
        e.height = c.height;
        e.fps = c.fps;
        e.display_id = c.display_id;
        e.epoch = c.epoch;
        e.codec = self.codec.load(Ordering::Relaxed);
        e.cursor_x = c.cursor_x;
        e.cursor_y = c.cursor_y;
        e.cursor_visible = u8::from(c.cursor_visible);
        let [sx, sy, sw, sh] = super::ffi_viewport::crop_src(c);
        e.src_x = sx;
        e.src_y = sy;
        e.src_w = sw;
        e.src_h = sh;
        e.display_w = c.display_width_px;
        e.display_h = c.display_height_px;
        self.event(&e);
    }

    fn cursor(&self, c: &CursorEvent) {
        let mut e = empty_event(RS_EV_CURSOR_POS);
        e.cursor_x = c.x;
        e.cursor_y = c.y;
        e.cursor_visible = u8::from(c.visible);
        self.event(&e);
    }

    fn cursor_msg(&self, m: &pb::CursorMsg) {
        let mut e;
        match &m.msg {
            Some(pb::cursor_msg::Msg::Shape(s)) => {
                e = empty_event(RS_EV_CURSOR_SHAPE);
                e.shape_id = s.id;
                e.width = s.width;
                e.height = s.height;
                e.hot_x = s.hot_x;
                e.hot_y = s.hot_y;
                e.scale = s.scale;
                e.data = s.png.as_ptr();
                e.data_len = s.png.len();
            }
            Some(pb::cursor_msg::Msg::Use(id)) => {
                e = empty_event(RS_EV_CURSOR_USE);
                e.shape_id = *id;
            }
            Some(pb::cursor_msg::Msg::Hidden(hidden)) => {
                e = empty_event(RS_EV_CURSOR_HIDDEN);
                e.cursor_visible = u8::from(!hidden);
            }
            None => return,
        }
        self.event(&e);
    }

    fn clock(&self, rtt_us: u64, offset_us: i64) {
        let mut e = empty_event(RS_EV_CLOCK_SYNC);
        e.rtt_us = rtt_us;
        e.offset_us = offset_us;
        self.event(&e);
    }

    fn closed(&self, reason: &str) {
        guard(|| {
            let message = CString::new(reason.replace('\0', "?")).unwrap_or_default();
            let mut e = empty_event(RS_EV_CLOSED);
            e.message = message.as_ptr();
            self.event(&e);
        });
    }
}

/// Attach native video/cursor callbacks, replaying cached state on a core worker.
/// # Safety
/// ctx and callbacks must allow worker-thread access and remain valid until
/// detach/replacement returns. Callback pointers are borrowed and must not unwind
/// or synchronously wait for the main thread. NULL callbacks are rejected.
#[no_mangle]
pub unsafe extern "C" fn rs_session_attach(
    id: u64,
    ctx: *mut c_void,
    on_frame: Option<RsFrameCb>,
    on_event: Option<RsEventCb>,
) -> bool {
    guard(|| {
        let (Some(on_frame), Some(on_event)) = (on_frame, on_event) else {
            return false;
        };
        let Ok(c) = core() else {
            return false;
        };
        let video = Arc::new(FfiVideo {
            ctx,
            on_frame,
            on_event,
            codec: AtomicU8::new(0),
        });
        let Some(entry) = c.state::<SessionTable>().get(id) else {
            // M3 V5：录制回放的「假会话」走录制模块。
            return super::recording::playback::attach_native(&c, id, video);
        };
        attach_on_worker(&c, move || {
            if entry.is_cancelled()
                || entry
                    .handle()
                    .is_none_or(|h| h.state() != SessionStateKind::Streaming)
            {
                return false;
            }
            let attached = guard(|| entry.video.attach(video));
            // A panicking Rust attach hook must not leave ctx registered after
            // returning false: the native caller may immediately release it.
            if !attached {
                entry.video.detach();
            }
            attached
        })
    })
}

/// Detach and wait for in-flight callbacks; no later callback uses the old ctx.
/// # Safety
/// No pointer arguments. When called inside a callback, its borrowed pointers
/// remain valid only until that callback returns.
#[no_mangle]
pub extern "C" fn rs_session_detach(id: u64) {
    guard(|| {
        if let Ok(c) = core() {
            if let Some(e) = c.state::<SessionTable>().get(id) {
                e.video.detach();
            } else {
                super::recording::playback::detach_native(&c, id);
            }
        }
    });
}

pub(super) fn with_handle(id: u64, f: impl FnOnce(&SessionHandle)) {
    if let Ok(c) = core() {
        if let Some(e) = c.state::<SessionTable>().get(id) {
            if !e.video.is_closed() && !e.is_cancelled() {
                if let Some(h) = e
                    .handle()
                    .filter(|h| h.state() == SessionStateKind::Streaming)
                {
                    f(&h);
                }
            }
        }
    }
}

/// Send normalized pointer coordinates; non-finite values are ignored.
/// # Safety
/// No pointer arguments or additional caller requirements.
#[no_mangle]
pub extern "C" fn rs_input_pointer(id: u64, x: f32, y: f32) {
    guard(|| {
        if x.is_finite() && y.is_finite() {
            if is_adapter_id(id) {
                adapter_route::input(id, InputMsg::MouseMove { x, y });
                return;
            }
            with_handle(id, |h| h.send_pointer(x, y));
        }
    });
}

/// Send a mouse button (0 left, 1 right, 2 middle, 3 back, 4 forward).
/// # Safety
/// No pointer arguments; invalid buttons/non-finite coordinates are ignored.
#[no_mangle]
pub extern "C" fn rs_input_button(id: u64, button: i32, down: bool, x: f32, y: f32, clicks: u8) {
    guard(|| {
        if !x.is_finite() || !y.is_finite() {
            return;
        }
        let Ok(button) = u8::try_from(button)
            .map_err(|_| ())
            .and_then(|b| Button::from_u8(b).map_err(|_| ()))
        else {
            return;
        };
        if is_adapter_id(id) {
            adapter_route::input(
                id,
                InputMsg::MouseButton {
                    button,
                    down,
                    x,
                    y,
                    clicks,
                },
            );
            return;
        }
        with_handle(id, |h| {
            let _ = h.send_input(InputMsg::MouseButton {
                button,
                down,
                x,
                y,
                clicks,
            });
        });
    });
}

/// Send pixel scroll deltas. phase/momentum: 0 none, 1 began, 2 changed, 3 ended.
/// # Safety
/// No pointer arguments; invalid phases/non-finite deltas are ignored.
#[no_mangle]
pub extern "C" fn rs_input_scroll(id: u64, dx: f32, dy: f32, phase: u8, momentum: u8) {
    guard(|| {
        if !dx.is_finite() || !dy.is_finite() {
            return;
        }
        let (Ok(phase), Ok(momentum)) =
            (ScrollPhase::from_u8(phase), ScrollPhase::from_u8(momentum))
        else {
            return;
        };
        if is_adapter_id(id) {
            adapter_route::input(
                id,
                InputMsg::Scroll {
                    dx,
                    dy,
                    phase,
                    momentum,
                    unit: ScrollUnit::Pixel,
                },
            );
            return;
        }
        with_handle(id, |h| {
            let _ = h.send_input(InputMsg::Scroll {
                dx,
                dy,
                phase,
                momentum,
                unit: ScrollUnit::Pixel,
            });
        });
    });
}

/// Send a touch: phase 0 began, 1 moved, 2 ended, 3 cancelled.
/// # Safety
/// No pointer arguments; invalid phases/non-finite coordinates or pressure are ignored.
#[no_mangle]
pub extern "C" fn rs_input_touch(id: u64, touch_id: u8, phase: u8, x: f32, y: f32, pressure: f32) {
    guard(|| {
        if !x.is_finite() || !y.is_finite() || !pressure.is_finite() {
            return;
        }
        let Ok(phase) = TouchPhase::from_u8(phase) else {
            return;
        };
        if is_adapter_id(id) {
            adapter_route::input(
                id,
                InputMsg::Touch {
                    id: touch_id,
                    phase,
                    x,
                    y,
                    pressure,
                },
            );
            return;
        }
        with_handle(id, |h| {
            let _ = h.send_input(InputMsg::Touch {
                id: touch_id,
                phase,
                x,
                y,
                pressure,
            });
        });
    });
}

/// Release all held inputs.
/// # Safety
/// No pointer arguments or additional caller requirements.
#[no_mangle]
pub extern "C" fn rs_input_release_all(id: u64) {
    guard(|| {
        if is_adapter_id(id) {
            adapter_route::input(id, InputMsg::ReleaseAll);
            return;
        }
        with_handle(id, |h| {
            let _ = h.send_input(InputMsg::ReleaseAll);
        })
    });
}

/// Request a keyframe for a decode failure/rebuild or decoder backlog.
/// # Safety
/// No pointer arguments or additional caller requirements.
#[no_mangle]
pub extern "C" fn rs_session_request_keyframe(id: u64, reason: u8) {
    guard(|| {
        let reason = match reason {
            2 => pb::keyframe_request::Reason::DecoderReset,
            4 => pb::keyframe_request::Reason::Backlog,
            _ => return,
        };
        with_handle(id, |h| h.request_keyframe_for(reason));
    });
}

/// Report one decoded frame for Feedback aggregation.
/// # Safety
/// No pointer arguments or additional caller requirements.
#[no_mangle]
pub extern "C" fn rs_session_report_decoded(id: u64, decode_us: u64, latency_us: u64, queue: u32) {
    guard(|| {
        with_handle(id, |h| h.report_decoded(decode_us, latency_us, queue));
    });
}
