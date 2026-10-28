//! ：Mac 会话窗口与输入采集。

use std::{
    ffi::{c_char, c_void, CString},
    panic::{catch_unwind, AssertUnwindSafe},
    ptr::NonNull,
    sync::Arc,
};

use crate::{
    mac_video::{self, MacVideoSink},
    platform::{
        FrameSink, KeyInput, PointerCapture, PointerInput, ScrollInput, SessionWindow,
        SessionWindowFactory, VideoCodec, WindowChange, WindowCommand, WindowEvents, WindowSpec,
    },
};

/// Callback table passed to the Swift window. The context is a boxed Arc and
/// is released by the Swift window exactly once when the window is closed.
#[repr(C)]
pub struct RsDtWindowCallbacks {
    pub ctx: *mut c_void,
    pub key: extern "C" fn(*mut c_void, u8, u16, bool, u64),
    pub pointer: extern "C" fn(*mut c_void, u8, f64, f64, f64, f64, u8, bool, u8),
    pub scroll: extern "C" fn(*mut c_void, u8, f64, f64, u64, u64),
    pub window: extern "C" fn(*mut c_void, u8, f64, f64, f64),
    pub release: extern "C" fn(*mut c_void),
}

// SAFETY: These are C functions exported by the Swift module built by
// `build.rs`; the arguments are copied by Swift before asynchronous work.
unsafe extern "C" {
    fn rs_dt_window_create(
        session_id: u64,
        title: *const c_char,
        width: u32,
        height: u32,
        callbacks: *const RsDtWindowCallbacks,
    ) -> *mut c_void;
    fn rs_dt_window_video(window: *mut c_void) -> *mut c_void;
    fn rs_dt_window_set_title(window: *mut c_void, title: *const c_char);
    fn rs_dt_window_set_fullscreen(window: *mut c_void, fullscreen: bool);
    fn rs_dt_window_set_keyboard_capture(window: *mut c_void, captured: bool);
    fn rs_dt_window_set_pointer_capture(window: *mut c_void, capture: u8);
    fn rs_dt_window_set_cursor_image(
        window: *mut c_void,
        has_image: bool,
        png: *const u8,
        png_len: usize,
        width: u32,
        height: u32,
        hot_x: u32,
        hot_y: u32,
        scale: f32,
    );
    fn rs_dt_window_set_cursor_hidden(window: *mut c_void, hidden: bool);
    fn rs_dt_window_show_remote_cursor(window: *mut c_void, shown: bool);
    fn rs_dt_window_move_remote_cursor(window: *mut c_void, x: f32, y: f32);
    fn rs_dt_window_set_overlay(window: *mut c_void, text: *const c_char);
    fn rs_dt_window_focus(window: *mut c_void);
    fn rs_dt_window_close(window: *mut c_void);
    fn rs_dt_window_release(window: *mut c_void);
    fn rs_dt_main_screen_pixel_width() -> u32;
}

/// Concrete Mac session-window factory.
pub struct MacFactory;

impl SessionWindowFactory for MacFactory {
    fn create(
        &self,
        spec: &WindowSpec,
        events: Arc<dyn WindowEvents>,
    ) -> Result<Arc<dyn SessionWindow>, crate::platform::PlatformError> {
        let title = CString::new(spec.title.as_str()).map_err(|_| {
            crate::platform::PlatformError::Failed("window title contains NUL".into())
        })?;
        let context = Box::into_raw(Box::new(events)).cast::<c_void>();
        let callbacks = RsDtWindowCallbacks {
            ctx: context,
            key: on_key,
            pointer: on_pointer,
            scroll: on_scroll,
            window: on_window,
            release: on_release,
        };
        // SAFETY: Swift copies the title and callback table synchronously;
        // `context` remains owned by the window until its release callback.
        let handle = unsafe {
            rs_dt_window_create(
                spec.session_id,
                title.as_ptr(),
                spec.width,
                spec.height,
                &callbacks as *const RsDtWindowCallbacks,
            )
        };
        let handle = match NonNull::new(handle) {
            Some(handle) => handle,
            None => {
                // SAFETY: `context` was allocated immediately above and has
                // not been transferred to a live window.
                unsafe { drop(Box::from_raw(context.cast::<Arc<dyn WindowEvents>>())) };
                return Err(crate::platform::PlatformError::Failed(
                    "failed to create Mac session window".into(),
                ));
            }
        };

        // SAFETY: the handle is a retained Swift object; this function returns
        // another +1 retained video pipeline.
        let pipeline = unsafe { rs_dt_window_video(handle.as_ptr()) };
        let pipeline = match NonNull::new(pipeline) {
            Some(pipeline) => pipeline,
            None => {
                // SAFETY: both calls accept the retained handle.
                unsafe {
                    rs_dt_window_close(handle.as_ptr());
                    rs_dt_window_release(handle.as_ptr());
                }
                return Err(crate::platform::PlatformError::Failed(
                    "failed to create Mac video pipeline".into(),
                ));
            }
        };
        // SAFETY: Swift returned a non-null +1 retained pipeline.
        let sink = unsafe { MacVideoSink::from_raw(pipeline.as_ptr()) };
        Ok(Arc::new(MacWindow { handle, sink }))
    }

    fn can_decode(&self, codec: VideoCodec) -> bool {
        mac_video::can_decode(codec)
    }

    fn screen_pixel_width(&self) -> u32 {
        // SAFETY: Swift performs this query synchronously on the main queue.
        unsafe { rs_dt_main_screen_pixel_width() }
    }
}

/// Return the real Mac factory when this module is linked into a macOS build.
pub fn factory() -> Option<Arc<dyn SessionWindowFactory>> {
    Some(Arc::new(MacFactory))
}

pub struct MacWindow {
    handle: NonNull<c_void>,
    sink: Arc<MacVideoSink>,
}

// SAFETY: the retained handle is an opaque Swift object; operations copy
// values and schedule work on AppKit's main queue.
unsafe impl Send for MacWindow {}
// SAFETY: mutable state is owned and synchronized by Swift/AppKit.
unsafe impl Sync for MacWindow {}

impl SessionWindow for MacWindow {
    fn frame_sink(&self) -> Arc<dyn FrameSink> {
        self.sink.clone()
    }

    fn apply(&self, command: WindowCommand) {
        let handle = self.handle.as_ptr();
        match command {
            WindowCommand::SetTitle(title) => {
                let Ok(title) = CString::new(title) else {
                    return;
                };
                // SAFETY: Swift copies the title before queueing the update.
                unsafe { rs_dt_window_set_title(handle, title.as_ptr()) };
            }
            WindowCommand::SetFullscreen(fullscreen) => {
                // SAFETY: value-only asynchronous FFI call.
                unsafe { rs_dt_window_set_fullscreen(handle, fullscreen) };
            }
            WindowCommand::SetKeyboardCapture(captured) => {
                // SAFETY: value-only asynchronous FFI call.
                unsafe { rs_dt_window_set_keyboard_capture(handle, captured) };
            }
            WindowCommand::SetSystemKeyGrab(_) | WindowCommand::SetSessionMenu(_) => {}
            WindowCommand::SetPointerCapture(capture) => {
                // SAFETY: value-only asynchronous FFI call.
                unsafe { rs_dt_window_set_pointer_capture(handle, pointer_capture_code(capture)) };
            }
            WindowCommand::SetCursorImage(image) => match image {
                Some(image) => {
                    let (ptr, len) = if image.png.is_empty() {
                        (std::ptr::null(), 0)
                    } else {
                        (image.png.as_ptr(), image.png.len())
                    };
                    // SAFETY: Swift copies the PNG synchronously.
                    unsafe {
                        rs_dt_window_set_cursor_image(
                            handle,
                            true,
                            ptr,
                            len,
                            image.width,
                            image.height,
                            image.hot_x,
                            image.hot_y,
                            image.scale,
                        )
                    };
                }
                None => {
                    // SAFETY: value-only asynchronous FFI call.
                    unsafe {
                        rs_dt_window_set_cursor_image(
                            handle,
                            false,
                            std::ptr::null(),
                            0,
                            0,
                            0,
                            0,
                            0,
                            1.0,
                        )
                    };
                }
            },
            WindowCommand::SetCursorHidden(hidden) => {
                // SAFETY: value-only asynchronous FFI call.
                unsafe { rs_dt_window_set_cursor_hidden(handle, hidden) };
            }
            WindowCommand::ShowRemoteCursor(shown) => {
                // SAFETY: value-only asynchronous FFI call.
                unsafe { rs_dt_window_show_remote_cursor(handle, shown) };
            }
            WindowCommand::MoveVirtualCursor { x, y } => {
                // SAFETY: value-only asynchronous FFI call.
                unsafe { rs_dt_window_move_remote_cursor(handle, x, y) };
            }
            WindowCommand::SetOverlayText(text) => match text {
                Some(text) => {
                    let Ok(text) = CString::new(text) else { return };
                    // SAFETY: Swift copies the text before queueing the update.
                    unsafe { rs_dt_window_set_overlay(handle, text.as_ptr()) };
                }
                None => {
                    // SAFETY: NULL asks Swift to clear the overlay.
                    unsafe { rs_dt_window_set_overlay(handle, std::ptr::null()) };
                }
            },
            WindowCommand::Focus => {
                // SAFETY: value-only asynchronous FFI call.
                unsafe { rs_dt_window_focus(handle) };
            }
            WindowCommand::Close => {
                // SAFETY: Swift makes close idempotent and releases the event
                // context exactly once on the main queue.
                unsafe { rs_dt_window_close(handle) };
            }
        }
    }
}

impl Drop for MacWindow {
    fn drop(&mut self) {
        // SAFETY: this is the one retained window handle owned by `MacWindow`.
        unsafe {
            rs_dt_window_close(self.handle.as_ptr());
            rs_dt_window_release(self.handle.as_ptr());
        }
    }
}

/// Decode a Swift key callback into the platform-neutral input enum.
pub fn decode_key(kind: u8, vk: u16, repeat: bool, flags: u64) -> Option<KeyInput> {
    match kind {
        0 => Some(KeyInput::MacKey {
            vk,
            down: true,
            repeat,
        }),
        1 => Some(KeyInput::MacKey {
            vk,
            down: false,
            repeat,
        }),
        2 => Some(KeyInput::MacFlags { vk, flags }),
        _ => None,
    }
}

/// Decode a Swift pointer callback into the platform-neutral input enum.
/// The parameters mirror the Swift callback's C ABI one to one.
#[allow(clippy::too_many_arguments)]
pub fn decode_pointer(
    kind: u8,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    button: u8,
    down: bool,
    clicks: u8,
) -> Option<PointerInput> {
    match kind {
        0 => Some(PointerInput::Move { x, y, dx, dy }),
        1 if button <= 4 => Some(PointerInput::Button {
            button,
            down,
            x,
            y,
            clicks,
        }),
        2 => Some(PointerInput::Leave),
        _ => None,
    }
}

/// Decode a Swift scroll callback into the platform-neutral input enum.
pub fn decode_scroll(kind: u8, dx: f64, dy: f64, phase: u64, momentum: u64) -> Option<ScrollInput> {
    match kind {
        0 => Some(ScrollInput::MacPrecise {
            dx,
            dy,
            phase,
            momentum,
        }),
        1 => Some(ScrollInput::MacLines { dx, dy }),
        _ => None,
    }
}

/// Decode a Swift window callback into the platform-neutral window enum.
pub fn decode_window(kind: u8, a: f64, b: f64, c: f64) -> Option<WindowChange> {
    match kind {
        0 => Some(WindowChange::Resized {
            width: a,
            height: b,
            scale: c,
        }),
        1 => Some(WindowChange::Focus(a != 0.0)),
        2 => Some(WindowChange::Fullscreen(a != 0.0)),
        3 => Some(WindowChange::Minimized(a != 0.0)),
        4 => Some(WindowChange::CloseRequested),
        _ => None,
    }
}

/// Encode the platform-neutral pointer-capture mode for Swift.
pub const fn pointer_capture_code(capture: PointerCapture) -> u8 {
    match capture {
        PointerCapture::Free => 0,
        PointerCapture::Confine => 1,
        PointerCapture::Virtual => 2,
    }
}

extern "C" fn on_key(ctx: *mut c_void, kind: u8, vk: u16, repeat: bool, flags: u64) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(event) = decode_key(kind, vk, repeat, flags) else {
            return;
        };
        with_events(ctx, |events| events.key(event));
    }));
}

extern "C" fn on_pointer(
    ctx: *mut c_void,
    kind: u8,
    x: f64,
    y: f64,
    dx: f64,
    dy: f64,
    button: u8,
    down: bool,
    clicks: u8,
) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(event) = decode_pointer(kind, x, y, dx, dy, button, down, clicks) else {
            return;
        };
        with_events(ctx, |events| events.pointer(event));
    }));
}

extern "C" fn on_scroll(ctx: *mut c_void, kind: u8, dx: f64, dy: f64, phase: u64, momentum: u64) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(event) = decode_scroll(kind, dx, dy, phase, momentum) else {
            return;
        };
        with_events(ctx, |events| events.scroll(event));
    }));
}

extern "C" fn on_window(ctx: *mut c_void, kind: u8, a: f64, b: f64, c: f64) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        let Some(event) = decode_window(kind, a, b, c) else {
            return;
        };
        with_events(ctx, |events| events.window(event));
    }));
}

extern "C" fn on_release(ctx: *mut c_void) {
    let _ = catch_unwind(AssertUnwindSafe(|| {
        if ctx.is_null() {
            return;
        }
        // SAFETY: Swift calls release exactly once for the Box allocated by
        // `MacFactory::create`, after all other callbacks have stopped.
        unsafe { drop(Box::from_raw(ctx.cast::<Arc<dyn WindowEvents>>())) };
    }));
}

fn with_events(ctx: *mut c_void, f: impl FnOnce(&Arc<dyn WindowEvents>)) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: Swift retains the callback context until release; event callbacks
    // run before that callback.
    let events = unsafe { &*ctx.cast::<Arc<dyn WindowEvents>>() };
    f(events);
}
