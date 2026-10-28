//! macOS ScreenCaptureKit 窗口采集。
use std::ffi::{c_char, c_void, CStr};
use std::panic::catch_unwind;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::mpsc::{self, Receiver, RecvTimeoutError, SyncSender, TrySendError};
use std::sync::Mutex;
use std::time::Duration;

use crate::capture::{CapturePlatform, FrameSource, TimedFrame, WindowInfo};

#[repr(C)]
#[derive(Clone, Copy)]
struct NativeWindow {
    id: u64,
    pid: i32,
    width: u32,
    height: u32,
    on_screen: i32,
    app: [c_char; 256],
    title: [c_char; 512],
}

impl Default for NativeWindow {
    fn default() -> Self {
        Self {
            id: 0,
            pid: 0,
            width: 0,
            height: 0,
            on_screen: 0,
            app: [0; 256],
            title: [0; 512],
        }
    }
}

type FrameCallback = extern "C" fn(*mut c_void, u64, u32, u32, *const u8, usize);
type ErrorCallback = extern "C" fn(*mut c_void, *const c_char);

extern "C" {
    fn rsb_list_windows(out: *mut NativeWindow, cap: i32, err: *mut c_char, err_len: i32) -> i32;
    fn rsb_permission(request: i32) -> i32;
    fn rsb_now_unix_us() -> u64;
    fn rsb_capture_start(
        window_id: u64,
        fps: u32,
        ctx: *mut c_void,
        on_frame: FrameCallback,
        on_error: ErrorCallback,
        err: *mut c_char,
        err_len: i32,
    ) -> *mut c_void;
    fn rsb_capture_stop(handle: *mut c_void);
}

fn native_string(value: &[c_char]) -> String {
    let bytes: Vec<u8> = value
        .iter()
        .take_while(|&&byte| byte != 0)
        .map(|&byte| byte as u8)
        .collect();
    String::from_utf8_lossy(&bytes).into_owned()
}

fn list_into(windows: &mut [NativeWindow]) -> Result<usize, String> {
    let cap = i32::try_from(windows.len()).map_err(|_| "too many windows".to_owned())?;
    let mut error = [0; 1024];
    // SAFETY: both buffers are initialized and writable for their declared
    // capacities. The shim writes at most cap entries and NUL-terminates error.
    let count = unsafe {
        rsb_list_windows(
            windows.as_mut_ptr(),
            cap,
            error.as_mut_ptr(),
            error.len() as i32,
        )
    };
    if count < 0 {
        let message = native_string(&error);
        return Err(if message.is_empty() {
            "cannot list windows".to_owned()
        } else {
            message
        });
    }
    Ok(count as usize)
}

/// ScreenCaptureKit 的单窗口采集平台。
pub struct MacPlatform;

impl CapturePlatform for MacPlatform {
    fn name(&self) -> &'static str {
        "mac-sck"
    }

    fn check_permission(&self, request: bool) -> bool {
        // SAFETY: this only checks TCC, optionally asking the system to prompt.
        unsafe { rsb_permission(i32::from(request)) != 0 }
    }

    fn list_windows(&self) -> Result<Vec<WindowInfo>, String> {
        let mut windows = vec![NativeWindow::default(); 512];
        let mut count = list_into(&mut windows)?;
        if count > windows.len() {
            windows.resize(count, NativeWindow::default());
            count = list_into(&mut windows)?;
        }
        // Windows may be created between the two snapshots; only the entries
        // actually written into this snapshot's buffer are initialized records.
        windows.truncate(count.min(windows.len()));
        Ok(windows
            .into_iter()
            .map(|window| WindowInfo {
                id: window.id,
                pid: window.pid,
                app: native_string(&window.app),
                title: native_string(&window.title),
                width: window.width,
                height: window.height,
                on_screen: window.on_screen != 0,
            })
            .collect())
    }

    fn open(&self, window: &WindowInfo, fps: u32) -> Result<Box<dyn FrameSource>, String> {
        if fps == 0 || fps > i32::MAX as u32 {
            return Err("fps must be in 1..=2147483647".to_owned());
        }
        let (sender, receiver) = mpsc::sync_channel(8);
        let shared = Box::into_raw(Box::new(Shared {
            sender: Mutex::new(Some(sender)),
            dropped: AtomicU64::new(0),
            error: Mutex::new(None),
        }));
        let mut error = [0; 1024];
        // SAFETY: shared has a stable Box allocation. The shim drains callbacks
        // on failed start as well as on stop, before we reclaim that allocation.
        let handle = unsafe {
            rsb_capture_start(
                window.id,
                fps,
                shared.cast(),
                on_frame,
                on_error,
                error.as_mut_ptr(),
                error.len() as i32,
            )
        };
        let Some(handle) = NonNull::new(handle) else {
            // SAFETY: a failed start has stopped and drained every callback.
            unsafe { drop(Box::from_raw(shared)) };
            let message = native_string(&error);
            return Err(if message.is_empty() {
                "cannot start capture".to_owned()
            } else {
                message
            });
        };
        Ok(Box::new(MacSource {
            handle,
            shared,
            receiver,
        }))
    }
}

/// 本平台采集实现。
pub fn platform() -> Option<Box<dyn CapturePlatform>> {
    Some(Box::new(MacPlatform))
}

fn plane_len(width: usize, height: usize, stride: usize) -> Option<usize> {
    if stride < width {
        return None;
    }
    if width == 0 || height == 0 {
        return Some(0);
    }
    stride.checked_mul(height - 1)?.checked_add(width)
}

/// 把亮度平面紧排成 width × height；最后一行不需要尾部填充。
/// 行跨度过小、源缓冲不足或尺寸溢出时返回 None。
pub fn pack_plane(width: u32, height: u32, stride: usize, src: &[u8]) -> Option<Vec<u8>> {
    let width = width as usize;
    let height = height as usize;
    let needed = plane_len(width, height, stride)?;
    if src.len() < needed {
        return None;
    }
    if needed == 0 {
        return Some(Vec::new());
    }
    let mut packed = Vec::with_capacity(width.checked_mul(height)?);
    for row in 0..height {
        let start = row * stride;
        packed.extend_from_slice(&src[start..start + width]);
    }
    Some(packed)
}

/// 当前 mach 时钟换算到 unix 微秒的时刻。
pub fn mach_now_unix_us() -> u64 {
    // SAFETY: the shim takes no pointers and only reads system clocks.
    unsafe { rsb_now_unix_us() }
}

struct Shared {
    sender: Mutex<Option<SyncSender<TimedFrame>>>,
    dropped: AtomicU64,
    error: Mutex<Option<String>>,
}

extern "C" fn on_frame(
    ctx: *mut c_void,
    t_us: u64,
    width: u32,
    height: u32,
    y: *const u8,
    stride: usize,
) {
    let _ = catch_unwind(|| {
        if ctx.is_null() || y.is_null() || width == 0 || height == 0 {
            return;
        }
        let Some(len) = plane_len(width as usize, height as usize, stride) else {
            return;
        };
        if len > isize::MAX as usize {
            return;
        }
        // SAFETY: the shim locks the pixel buffer and lends these plane bytes
        // until this callback returns, including the last row's visible pixels.
        let plane = unsafe { std::slice::from_raw_parts(y, len) };
        let Some(luma) = pack_plane(width, height, stride, plane) else {
            return;
        };
        // SAFETY: stop (and failed start) drains callbacks before freeing Shared.
        let shared = unsafe { &*ctx.cast::<Shared>() };
        let sender = shared.sender.lock().unwrap_or_else(|p| p.into_inner());
        if let Some(sender) = sender.as_ref() {
            match sender.try_send(TimedFrame {
                t_us,
                width,
                height,
                luma,
            }) {
                Err(TrySendError::Full(_)) => {
                    shared.dropped.fetch_add(1, Ordering::Relaxed);
                }
                Ok(()) | Err(TrySendError::Disconnected(_)) => {}
            }
        }
    });
}

extern "C" fn on_error(ctx: *mut c_void, message: *const c_char) {
    let _ = catch_unwind(|| {
        if ctx.is_null() {
            return;
        }
        let message = if message.is_null() {
            "capture stopped".to_owned()
        } else {
            // SAFETY: the shim lends a NUL-terminated UTF-8 description for this call.
            unsafe { CStr::from_ptr(message) }
                .to_string_lossy()
                .into_owned()
        };
        // SAFETY: this uses the same drained, boxed context as on_frame.
        let shared = unsafe { &*ctx.cast::<Shared>() };
        *shared.error.lock().unwrap_or_else(|p| p.into_inner()) = Some(message);
        shared
            .sender
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .take();
    });
}

struct MacSource {
    handle: NonNull<c_void>,
    shared: *mut Shared,
    receiver: Receiver<TimedFrame>,
}

// SAFETY: the native handle has no thread affinity. Until synchronous stop
// drains callbacks, they only access Shared through its mutexes and atomics;
// moving MacSource does not move Shared's Box allocation.
unsafe impl Send for MacSource {}

impl MacSource {
    fn shared(&self) -> &Shared {
        // SAFETY: this Box is owned by MacSource until Drop stops the stream.
        unsafe { &*self.shared }
    }
}

impl FrameSource for MacSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<TimedFrame>, String> {
        match self.receiver.recv_timeout(timeout) {
            Ok(frame) => Ok(Some(frame)),
            Err(RecvTimeoutError::Timeout) => Ok(None),
            Err(RecvTimeoutError::Disconnected) => Err(self
                .shared()
                .error
                .lock()
                .unwrap_or_else(|p| p.into_inner())
                .clone()
                .unwrap_or_else(|| "capture stopped".to_owned())),
        }
    }

    fn dropped(&self) -> u64 {
        self.shared().dropped.load(Ordering::Relaxed)
    }
}

impl Drop for MacSource {
    fn drop(&mut self) {
        // SAFETY: we own this handle; stop synchronously drains all callbacks.
        unsafe { rsb_capture_stop(self.handle.as_ptr()) };
        // SAFETY: no native callback can access Shared after stop has returned.
        unsafe { drop(Box::from_raw(self.shared)) };
    }
}
