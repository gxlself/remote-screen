//! ：Mac 视频 FrameSink（VideoToolbox + AVSampleBufferDisplayLayer）。

use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    ptr::NonNull,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use crate::platform::{
    BitstreamFormat, DecodeResult, DesktopFrame, FrameDone, FrameSink, StreamInfo, VideoCodec,
};

type RsDtFrameDone = extern "C" fn(*mut c_void, bool, u64);

/// Snapshot counters and the last cursor hotspot in view coordinates.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct VideoDebug {
    pub decoded: u64,
    pub failed: u64,
    pub last_width: i32,
    pub last_height: i32,
    pub cursor_x: f64,
    pub cursor_y: f64,
}

// SAFETY: build.rs links these C ABI symbols from the macOS-only Swift module.
unsafe extern "C" {
    fn rs_dt_video_hw_supported(codec: i32) -> bool;
    fn rs_dt_video_create_headless(width: i32, height: i32) -> *mut c_void;
    fn rs_dt_video_configure(pointer: *mut c_void, codec: i32, width: u32, height: u32);
    fn rs_dt_video_submit(
        pointer: *mut c_void,
        frame_id: u32,
        keyframe: bool,
        data: *const u8,
        data_len: usize,
        vps: *const u8,
        vps_len: usize,
        sps: *const u8,
        sps_len: usize,
        pps: *const u8,
        pps_len: usize,
        capture_ts_us: u64,
        ctx: *mut c_void,
        done: RsDtFrameDone,
    );
    fn rs_dt_video_close(pointer: *mut c_void);
    fn rs_dt_video_release(pointer: *mut c_void);
    fn rs_dt_video_debug_state(pointer: *mut c_void, output: *mut VideoDebug);
    fn rs_dt_video_debug_move_cursor(pointer: *mut c_void, nx: f64, ny: f64);
}

/// Mac hardware decoder and display sink.  The Swift pipeline owns the
/// retained object behind `pointer`; decoding runs on its private serial queue.
pub struct MacVideoSink {
    pointer: NonNull<c_void>,
    closed: AtomicBool,
}

// SAFETY: Swift copies frame bytes synchronously, serializes decoding on its
// queue, and protects ingress and debug state with locks.  Window view updates
// are sent to the main queue; headless updates use the same ingress lock.
unsafe impl Send for MacVideoSink {}
// SAFETY: see the `Send` implementation above; no Rust references are kept by
// the Swift object and all mutable state is serialized there.
unsafe impl Sync for MacVideoSink {}

impl MacVideoSink {
    /// Take ownership of a +1 retained Swift pipeline pointer.
    ///
    /// # Safety
    /// `pointer` must be a non-null +1 retained pipeline returned by
    /// `rs_dt_video_create_headless` or `rs_dt_window_video`. Ownership of that
    /// retain is transferred here exactly once; it must not already be released.
    // SAFETY: pointer validity and transfer of the retain are caller obligations.
    pub unsafe fn from_raw(pointer: *mut c_void) -> Arc<Self> {
        let pointer = match NonNull::new(pointer) {
            Some(pointer) => pointer,
            None => panic!("MacVideoSink requires a non-null Swift pipeline"),
        };
        Arc::new(Self {
            pointer,
            closed: AtomicBool::new(false),
        })
    }

    /// Construct a retained headless pipeline for tests.
    pub fn headless(width: u32, height: u32) -> Arc<Self> {
        let width = i32::try_from(width).unwrap_or(i32::MAX);
        let height = i32::try_from(height).unwrap_or(i32::MAX);
        // SAFETY: the Swift function returns a +1 retained pipeline and takes
        // only value arguments.
        let pointer = unsafe { rs_dt_video_create_headless(width, height) };
        // SAFETY: the pointer was returned by the Swift constructor above.
        unsafe { Self::from_raw(pointer) }
    }

    pub fn debug_state(&self) -> VideoDebug {
        let mut output = VideoDebug::default();
        // SAFETY: `pointer` remains retained for the lifetime of `self`, and
        // `output` is a valid writable C-layout value.
        unsafe { rs_dt_video_debug_state(self.pointer.as_ptr(), &mut output) };
        output
    }

    /// Test-only cursor update on the calling thread. Window pipelines ignore it.
    pub fn debug_move_cursor(&self, nx: f64, ny: f64) {
        // SAFETY: the retained Swift pipeline accepts this test-only call on
        // the caller's thread and does not borrow the arguments.
        unsafe { rs_dt_video_debug_move_cursor(self.pointer.as_ptr(), nx, ny) };
    }
}

impl FrameSink for MacVideoSink {
    fn format(&self) -> BitstreamFormat {
        BitstreamFormat::Avcc
    }

    fn configure(&self, info: &StreamInfo) {
        if self.closed.load(Ordering::Acquire) {
            return;
        }
        // SAFETY: the pipeline is retained, and Swift copies these value
        // arguments before enqueuing the configuration reset.
        unsafe {
            rs_dt_video_configure(
                self.pointer.as_ptr(),
                codec_number(info.codec),
                info.width,
                info.height,
            )
        }
    }

    fn submit(&self, frame: DesktopFrame, done: FrameDone) {
        if self.closed.load(Ordering::Acquire) {
            done.finish(DecodeResult::Failed);
            return;
        }

        let data = frame.data;
        let vps = frame.vps;
        let sps = frame.sps;
        let pps = frame.pps;
        let (vps_ptr, vps_len) = bytes_arg(vps.as_deref());
        let (sps_ptr, sps_len) = bytes_arg(sps.as_deref());
        let (pps_ptr, pps_len) = bytes_arg(pps.as_deref());
        let (data_ptr, data_len) = bytes_arg(Some(data.as_slice()));
        let context = Box::into_raw(Box::new(done)).cast::<c_void>();
        // SAFETY: all pointers refer to `frame`'s owned vectors, which stay
        // alive through this synchronous call while Swift copies them.  The
        // callback takes ownership of `context` exactly once.
        unsafe {
            rs_dt_video_submit(
                self.pointer.as_ptr(),
                frame.frame_id,
                frame.keyframe,
                data_ptr,
                data_len,
                vps_ptr,
                vps_len,
                sps_ptr,
                sps_len,
                pps_ptr,
                pps_len,
                frame.capture_ts_us,
                context,
                on_done,
            )
        }
    }

    fn close(&self) {
        if self.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        // SAFETY: `pointer` is retained until Drop and the Swift close call is
        // idempotent.
        unsafe { rs_dt_video_close(self.pointer.as_ptr()) };
    }
}

impl Drop for MacVideoSink {
    fn drop(&mut self) {
        self.close();
        // SAFETY: this is the +1 retained pointer consumed by `from_raw`, and
        // Drop releases it exactly once.
        unsafe { rs_dt_video_release(self.pointer.as_ptr()) };
    }
}

pub fn can_decode(codec: VideoCodec) -> bool {
    // SAFETY: the argument is a small enum mapping and the Swift probe has no
    // pointer or lifetime requirements.
    unsafe { rs_dt_video_hw_supported(codec_number(codec)) }
}

fn codec_number(codec: VideoCodec) -> i32 {
    match codec {
        VideoCodec::Hevc => 0,
        VideoCodec::H264 => 1,
    }
}

fn bytes_arg(bytes: Option<&[u8]>) -> (*const u8, usize) {
    match bytes {
        Some(bytes) if !bytes.is_empty() => (bytes.as_ptr(), bytes.len()),
        _ => (std::ptr::null(), 0),
    }
}

extern "C" fn on_done(context: *mut c_void, ok: bool, decode_us: u64) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if context.is_null() {
            return;
        }
        // SAFETY: `context` is a Box<FrameDone> allocated by `submit`; the
        // Swift callback is required to invoke it exactly once.
        let done = unsafe { Box::from_raw(context.cast::<FrameDone>()) };
        if ok {
            done.finish(DecodeResult::Shown { decode_us });
        } else {
            done.finish(DecodeResult::Failed);
        }
    }));
}
