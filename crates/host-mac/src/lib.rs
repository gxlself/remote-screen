#![cfg(target_os = "macos")]

//! Safe wrappers over the Objective-C shim (`shim.m`).

pub mod activity;
pub mod audio;
pub mod clipboard;
pub mod console;
pub mod cursor;
pub mod displays;
pub mod encode;
pub mod inject;
pub mod keystore;
pub mod nics;
pub mod perms;
pub mod power;
pub mod sysauth;
pub mod sysquery;
pub mod update_install;
pub mod update_verify;
pub mod vdisplay;
pub mod wol;

mod platform;
pub use platform::MacPlatform;

use std::ffi::{c_char, c_void, CStr};
use std::sync::atomic::{AtomicBool, Ordering};

use rsp_core::clock::unix_micros;
use rsp_core::input::{InputEvent, MouseButton};
use tokio::sync::mpsc;

type EncodedCb =
    extern "C" fn(ctx: *mut c_void, data: *const u8, len: usize, flags: i32, capture_ts_us: u64);

extern "C" {
    fn rs_capture_start(
        fps: u32,
        bitrate_kbps: u32,
        max_width: u32,
        ctx: *mut c_void,
        cb: EncodedCb,
        out_w: *mut u32,
        out_h: *mut u32,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
    fn rs_capture_request_keyframe();
    fn rs_capture_stop();
    fn rs_display_pixel_size(w: *mut u32, h: *mut u32);
    fn rs_input_mouse_move(x: f64, y: f64);
    fn rs_input_mouse_button(button: i32, down: i32, x: f64, y: f64, clicks: i32);
    fn rs_input_scroll(dx: f64, dy: f64);
    fn rs_cursor_position(x: *mut f64, y: *mut f64);
}

#[derive(Debug)]
pub struct EncodedFrame {
    /// HEVC access unit, Annex-B; parameter sets prepended on keyframes.
    pub data: Vec<u8>,
    pub keyframe: bool,
    /// Host wall clock when the frame was on screen.
    pub capture_ts_us: u64,
    /// Host wall clock when the encoder handed the frame back.
    pub encoded_us: u64,
}

struct CallbackCtx {
    tx: mpsc::Sender<EncodedFrame>,
    /// Set after a frame had to be dropped: later delta frames reference it,
    /// so drop them too until the requested keyframe comes out.
    skip_until_key: AtomicBool,
}

extern "C" fn on_encoded(
    ctx: *mut c_void,
    data: *const u8,
    len: usize,
    flags: i32,
    capture_ts_us: u64,
) {
    // SAFETY: ctx is the CallbackCtx owned by the live Capture; the shim stops
    // calling back before Capture::drop frees it.
    let ctx = unsafe { &*(ctx as *const CallbackCtx) };
    let keyframe = flags & 1 != 0;
    if ctx.skip_until_key.load(Ordering::Relaxed) {
        if !keyframe {
            return;
        }
        ctx.skip_until_key.store(false, Ordering::Relaxed);
    }
    // SAFETY: the shim lends len readable bytes for the duration of this callback.
    let data = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    let frame = EncodedFrame {
        data,
        keyframe,
        capture_ts_us,
        encoded_us: unix_micros(),
    };
    if ctx.tx.try_send(frame).is_err() {
        ctx.skip_until_key.store(true, Ordering::Relaxed);
        // SAFETY: this function only schedules work on the live shim capture.
        unsafe { rs_capture_request_keyframe() };
    }
}

#[derive(Debug)]
pub struct CaptureError {
    pub permission_denied: bool,
    pub message: String,
}

impl std::fmt::Display for CaptureError {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.write_str(&self.message)
    }
}

impl std::error::Error for CaptureError {}

/// A running capture of the main display. Dropping it stops capture.
pub struct Capture {
    ctx: *mut CallbackCtx,
    pub width: u32,
    pub height: u32,
}

// SAFETY: the shim serializes callbacks and Drop drains them before freeing ctx.
unsafe impl Send for Capture {}

impl Capture {
    pub fn start(
        fps: u32,
        bitrate_kbps: u32,
        max_width: u32,
        tx: mpsc::Sender<EncodedFrame>,
    ) -> Result<Self, CaptureError> {
        let ctx = Box::into_raw(Box::new(CallbackCtx {
            tx,
            skip_until_key: AtomicBool::new(false),
        }));
        let (mut w, mut h) = (0u32, 0u32);
        let mut err = [0 as c_char; 512];
        // SAFETY: all output buffers and the boxed context live through this call.
        let rc = unsafe {
            rs_capture_start(
                fps,
                bitrate_kbps,
                max_width,
                ctx as *mut c_void,
                on_encoded,
                &mut w,
                &mut h,
                err.as_mut_ptr(),
                err.len(),
            )
        };
        if rc != 0 {
            // SAFETY: a failed start leaves no callbacks holding this context.
            drop(unsafe { Box::from_raw(ctx) });
            // SAFETY: the shim always NUL-terminates the error buffer.
            let message = unsafe { CStr::from_ptr(err.as_ptr()) }
                .to_string_lossy()
                .into_owned();
            return Err(CaptureError {
                permission_denied: rc == 2,
                message,
            });
        }
        Ok(Capture {
            ctx,
            width: w,
            height: h,
        })
    }
}

impl Drop for Capture {
    fn drop(&mut self) {
        // SAFETY: stop drains the callback queue before the context is freed.
        unsafe {
            rs_capture_stop();
            drop(Box::from_raw(self.ctx));
        }
    }
}

pub fn request_keyframe() {
    // SAFETY: the shim handles an absent capture and synchronizes its queue.
    unsafe { rs_capture_request_keyframe() }
}

pub fn display_pixel_size() -> (u32, u32) {
    let (mut w, mut h) = (0, 0);
    // SAFETY: both pointers reference valid u32 outputs.
    unsafe { rs_display_pixel_size(&mut w, &mut h) };
    (w, h)
}

pub fn screen_capture_allowed(request: bool) -> bool {
    perms::screen_capture_allowed(request)
}

pub fn accessibility_trusted(prompt: bool) -> bool {
    perms::accessibility_trusted(prompt)
}

pub fn cursor_position() -> (f32, f32) {
    let (mut x, mut y) = (0.5, 0.5);
    // SAFETY: both pointers reference valid f64 outputs.
    unsafe { rs_cursor_position(&mut x, &mut y) };
    (x.clamp(0.0, 1.0) as f32, y.clamp(0.0, 1.0) as f32)
}

pub fn inject(ev: InputEvent) {
    // SAFETY: all arguments are primitive input event fields; the shim owns CGEvents.
    unsafe {
        match ev {
            InputEvent::MouseMove { x, y } => rs_input_mouse_move(x as f64, y as f64),
            InputEvent::MouseButton {
                button,
                down,
                x,
                y,
                clicks,
            } => {
                let b = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                };
                rs_input_mouse_button(b, down as i32, x as f64, y as f64, clicks as i32)
            }
            InputEvent::Scroll { dx, dy } => rs_input_scroll(dx as f64, dy as f64),
        }
    }
}

pub mod files;
