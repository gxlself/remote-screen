//! Native handles stay in this module; host-core never reads image pixels.

use std::ffi::{c_char, c_void, CStr};
use std::path::Path;
use std::ptr::NonNull;
use std::sync::atomic::{AtomicBool, AtomicPtr, Ordering};
use std::sync::{Arc, Mutex};

use host_core::pipeline::capture::dimensions;
use host_core::platform::*;
use rsp_core::{clock::unix_micros, hid::InputMsg, pb};
use tokio::sync::mpsc;

use crate::encode::{EncoderInfo, EncoderStats, SyntheticFrame};
use crate::inject::MacInjector;
use crate::{activity, clipboard, console, cursor, displays, keystore, nics, perms, power, wol};

#[path = "thumbnail.rs"]
mod thumbnail;

type FrameCb = extern "C" fn(*mut c_void, *mut c_void, u32, u32, u64);

#[repr(C)]
#[derive(Default, Clone, Copy)]
struct DisplayRecord {
    id: u32,
    width: u32,
    height: u32,
    x: i32,
    y: i32,
    scale: f32,
    refresh_hz: u32,
    primary: u32,
    rotation: u32,
}

extern "C" {
    fn rs_list_displays(out: *mut DisplayRecord, capacity: u32) -> u32;
    fn rs_frame_capture_start(
        display: u32,
        fps: u32,
        width: u32,
        height: u32,
        ctx: *mut c_void,
        cb: FrameCb,
        handle: *mut *mut c_void,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
    fn rs_frame_capture_start_region(
        display: u32,
        fps: u32,
        width: u32,
        height: u32,
        src_x: f64,
        src_y: f64,
        src_w: f64,
        src_h: f64,
        ctx: *mut c_void,
        cb: FrameCb,
        handle: *mut *mut c_void,
        err: *mut c_char,
        errlen: usize,
    ) -> i32;
    fn rs_frame_capture_stop(handle: *mut c_void);
    fn rs_pixel_buffer_retain(pixel: *mut c_void);
    fn rs_pixel_buffer_release(pixel: *mut c_void);
    fn rs_encoder_start(
        codec: u32,
        width: u32,
        height: u32,
        fps: u32,
        kbps: u32,
        ctx: *mut c_void,
        cb: super::EncodedCb,
        err: *mut c_char,
        errlen: usize,
    ) -> *mut c_void;
    fn rs_encoder_encode(
        handle: *mut c_void,
        pixel: *mut c_void,
        capture_us: u64,
        force: i32,
        boost: i32,
    ) -> i32;
    fn rs_encoder_rate(handle: *mut c_void, kbps: u32, fps: u32);
    fn rs_encoder_rate_window(handle: *mut c_void, kbps: u32, fps: u32, window_seconds: f64);
    fn rs_encoder_keyframe(handle: *mut c_void);
    fn rs_encoder_quality(handle: *mut c_void, on: i32);
    fn rs_encoder_info(handle: *mut c_void, out: *mut NativeEncoderInfo);
    fn rs_encoder_stats(handle: *mut c_void, out: *mut NativeEncoderStats);
    fn rs_encoder_stop(handle: *mut c_void, out: *mut NativeEncoderStats);
    fn rs_input_select_display(display: u32);
    fn rs_input_release_all();
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeEncoderInfo {
    codec: u32,
    low_latency: i32,
    hardware: i32,
    quality_supported: i32,
}

#[repr(C)]
#[derive(Clone, Copy, Default)]
struct NativeEncoderStats {
    submitted: u64,
    output: u64,
    dropped: u64,
    callback_error: u64,
    submit_error: u64,
    callback_status_error: u64,
    unexpected_null: u64,
    conversion_error: u64,
    forced_key_retries: u64,
}

impl From<NativeEncoderStats> for EncoderStats {
    fn from(stats: NativeEncoderStats) -> Self {
        Self {
            submitted: stats.submitted,
            output: stats.output,
            dropped: stats.dropped,
            callback_error: stats.callback_error,
            submit_error: stats.submit_error,
            callback_status_error: stats.callback_status_error,
            unexpected_null: stats.unexpected_null,
            conversion_error: stats.conversion_error,
            forced_key_retries: stats.forced_key_retries,
        }
    }
}

static INPUT_LOCK: Mutex<()> = Mutex::new(());

#[derive(Clone)]
pub struct MacPlatform {
    keystore: Arc<keystore::MacKeyStore>,
    clipboard_own_change: Arc<std::sync::atomic::AtomicU64>,
    injector: Arc<Mutex<MacInjector>>,
}

impl MacPlatform {
    pub fn new() -> Self {
        let injector = Arc::new(Mutex::new(MacInjector::new()));
        let weak_injector = Arc::downgrade(&injector);
        if let Err(error) = std::thread::Builder::new()
            .name("rs-input-poll".into())
            .spawn(move || loop {
                std::thread::sleep(std::time::Duration::from_millis(100));
                let Some(injector) = weak_injector.upgrade() else {
                    break;
                };
                // All native input takes INPUT_LOCK before the injector lock.
                let _lock = INPUT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
                injector
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .poll(unix_micros());
            })
        {
            tracing::warn!(%error, "could not start scroll timeout polling");
        }
        Self {
            keystore: Arc::new(keystore::MacKeyStore::from_environment()),
            clipboard_own_change: Arc::new(std::sync::atomic::AtomicU64::new(0)),
            injector,
        }
    }

    /// Compatibility entry point forwarding to the shared v2 Platform method.
    pub fn inject_msg(&self, msg: &InputMsg, display: &DisplayInfo) {
        <Self as Platform>::inject_input(self, msg, display);
    }

    /// Call once when starting a session, after any previous controller is released.
    pub fn refresh_keyboard_profile(&self) {
        let _lock = INPUT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        self.injector
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .refresh_profile();
    }

    fn start_capture(
        &self,
        display: u32,
        fps: u32,
        width: u32,
        height: u32,
        src: Option<(f64, f64, f64, f64)>,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let mut ctx = Box::new(CaptureCtx {
            display_id: display,
            sink,
        });
        let mut handle = std::ptr::null_mut();
        let mut err = [0 as c_char; 512];
        // SAFETY: ctx's allocation is stable until Drop; start failure drains callbacks.
        let rc = unsafe {
            match src {
                Some((x, y, w, h)) => rs_frame_capture_start_region(
                    display,
                    fps,
                    width,
                    height,
                    x,
                    y,
                    w,
                    h,
                    ctx.as_mut() as *mut _ as *mut c_void,
                    on_frame,
                    &mut handle,
                    err.as_mut_ptr(),
                    err.len(),
                ),
                None => rs_frame_capture_start(
                    display,
                    fps,
                    width,
                    height,
                    ctx.as_mut() as *mut _ as *mut c_void,
                    on_frame,
                    &mut handle,
                    err.as_mut_ptr(),
                    err.len(),
                ),
            }
        };
        if rc != 0 {
            let message = error_message(&err);
            return Err(if rc == 2 {
                PlatformError::PermissionDenied(message)
            } else {
                PlatformError::Platform(message)
            });
        }
        let handle = NonNull::new(handle)
            .ok_or_else(|| PlatformError::Platform("capture returned no handle".into()))?;
        Ok(Box::new(MacCapture { handle, _ctx: ctx }))
    }
}

impl Default for MacPlatform {
    fn default() -> Self {
        Self::new()
    }
}

fn command(program: &str, args: &[&str], fallback: &str) -> String {
    std::process::Command::new(program)
        .args(args)
        .output()
        .ok()
        .filter(|output| output.status.success())
        .and_then(|output| String::from_utf8(output.stdout).ok())
        .map(|value| value.trim().to_owned())
        .filter(|value| !value.is_empty())
        .unwrap_or_else(|| fallback.to_owned())
}

/// Convert native pixels to SCK sourceRect points; invalid scales fall back to 1.
fn region_points(rect: PixelRect, scale: f32) -> (f64, f64, f64, f64) {
    let scale = if scale.is_finite() && scale > 0.0 {
        f64::from(scale)
    } else {
        1.0
    };
    (
        f64::from(rect.x) / scale,
        f64::from(rect.y) / scale,
        f64::from(rect.w) / scale,
        f64::from(rect.h) / scale,
    )
}

pub(crate) fn create_encoder(cfg: EncoderConfig) -> Result<(MacEncoder, EncoderInfo)> {
    let native_codec = match cfg.codec {
        pb::Codec::Hevc | pb::Codec::H264 => cfg.codec,
        _ => return Err(PlatformError::Unsupported),
    };
    if cfg.fps == 0
        || cfg.fps > 120
        || cfg.bitrate_kbps == 0
        || cfg.width < 2
        || cfg.height < 2
        || cfg.width > i32::MAX as u32
        || cfg.height > i32::MAX as u32
        || !cfg.width.is_multiple_of(2)
        || !cfg.height.is_multiple_of(2)
    {
        return Err(PlatformError::Platform(
            "invalid encoder configuration".into(),
        ));
    }
    if native_codec == pb::Codec::Hevc
        && std::env::var_os("RS_HOST_DISABLE_HEVC").is_some_and(|value| value == "1")
    {
        tracing::warn!("HEVC encoder disabled by RS_HOST_DISABLE_HEVC");
        return Err(PlatformError::Unsupported);
    }
    let mut ctx = Box::new(EncoderCtx {
        sink: cfg.sink,
        skip_until_key: AtomicBool::new(false),
        handle: AtomicPtr::new(std::ptr::null_mut()),
    });
    let mut err = [0 as c_char; 512];
    // SAFETY: the stable context is retained by MacEncoder until VT callbacks drain.
    let handle = unsafe {
        rs_encoder_start(
            native_codec as u32,
            cfg.width,
            cfg.height,
            cfg.fps,
            cfg.bitrate_kbps,
            ctx.as_mut() as *mut _ as *mut c_void,
            on_encoded,
            err.as_mut_ptr(),
            err.len(),
        )
    };
    let handle = match NonNull::new(handle) {
        Some(handle) => handle,
        None if native_codec == pb::Codec::Hevc => {
            tracing::warn!(error = %error_message(&err), "HEVC encoder unavailable");
            return Err(PlatformError::Unsupported);
        }
        None => return Err(PlatformError::Platform(error_message(&err))),
    };
    ctx.handle.store(handle.as_ptr(), Ordering::Release);
    let mut native_info = NativeEncoderInfo::default();
    // SAFETY: handle is live and native_info is a valid output structure.
    unsafe { rs_encoder_info(handle.as_ptr(), &mut native_info) };
    let info = EncoderInfo {
        codec: if native_info.codec == pb::Codec::H264 as u32 {
            pb::Codec::H264
        } else {
            pb::Codec::Hevc
        },
        low_latency: native_info.low_latency != 0,
        hardware: native_info.hardware != 0,
        quality_supported: native_info.quality_supported != 0,
    };
    tracing::info!(
        codec = ?info.codec,
        low_latency = info.low_latency,
        hardware = info.hardware,
        quality_supported = info.quality_supported,
        "VideoToolbox encoder ready"
    );
    Ok((
        MacEncoder {
            handle,
            width: cfg.width,
            height: cfg.height,
            stopped: false,
            final_stats: EncoderStats::default(),
            _ctx: ctx,
        },
        info,
    ))
}
impl Platform for MacPlatform {
    fn on_file_received(&self, path: &Path, device_name: &str) {
        if let Err(error) =
            crate::files::quarantine(path, "Remote Host", crate::files::unix_seconds())
        {
            tracing::warn!(%error, "quarantine failed");
        }
        let _ = device_name;
    }

    fn info(&self) -> HostInfo {
        HostInfo {
            name: command("scutil", &["--get", "ComputerName"], "mac"),
            os: pb::Platform::Macos,
            os_version: command("sw_vers", &["-productVersion"], "macOS"),
            model: command("sysctl", &["-n", "hw.model"], "Mac"),
            arch: std::env::consts::ARCH.into(),
        }
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        let mut records = [DisplayRecord::default(); 32];
        // SAFETY: out points to capacity initialized DisplayRecords with the C layout.
        let count = unsafe { rs_list_displays(records.as_mut_ptr(), records.len() as u32) };
        records
            .into_iter()
            .take(count.min(32) as usize)
            .map(|record| DisplayInfo {
                id: record.id,
                name: format!("Display {}", record.id),
                width_px: record.width,
                height_px: record.height,
                scale: record.scale,
                x: record.x,
                y: record.y,
                refresh_hz: record.refresh_hz,
                primary: record.primary != 0,
                rotation: record.rotation,
            })
            .collect()
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        displays::watch(self.clone(), tx);
    }

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let info = self
            .displays()
            .into_iter()
            .find(|info| info.id == display)
            .ok_or_else(|| PlatformError::Platform("unknown display".into()))?;
        if cfg.fps == 0 || cfg.fps > 120 {
            return Err(PlatformError::Platform("capture fps must be 1..120".into()));
        }
        let (width, height) = dimensions(&info, cfg.max_width, cfg.max_height)?;
        self.start_capture(display, cfg.fps, width, height, None, sink)
    }

    fn thumbnail_jpeg(&self, max_width: u32, max_bytes: usize) -> Option<Vec<u8>> {
        thumbnail::capture_jpeg(max_width, max_bytes)
    }

    fn capture_region_supported(&self) -> bool {
        true
    }

    fn capture_region(
        &self,
        display: u32,
        rect: PixelRect,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let info = self
            .displays()
            .into_iter()
            .find(|info| info.id == display)
            .ok_or_else(|| PlatformError::Platform("unknown display".into()))?;
        if cfg.fps == 0 || cfg.fps > 120 {
            return Err(PlatformError::Platform("capture fps must be 1..120".into()));
        }
        if rect.w < 2
            || rect.h < 2
            || u64::from(rect.x) + u64::from(rect.w) > u64::from(info.width_px)
            || u64::from(rect.y) + u64::from(rect.h) > u64::from(info.height_px)
        {
            return Err(PlatformError::Platform("crop outside display".into()));
        }
        let width = cfg.max_width & !1;
        let height = cfg.max_height & !1;
        if width < 2 || height < 2 {
            return Err(PlatformError::Platform("invalid crop output size".into()));
        }
        let src = region_points(rect, info.scale);
        self.start_capture(display, cfg.fps, width, height, Some(src), sink)
    }

    fn audio_capture_supported(&self) -> bool {
        crate::audio::supported()
    }

    fn capture_audio(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        crate::audio::start(cfg, sink)
    }

    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>> {
        create_encoder(cfg).map(|(encoder, _)| Box::new(encoder) as Box<dyn Encoder>)
    }

    fn inject(&self, ev: InputEvent, display: &DisplayInfo) {
        let _lock = INPUT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: the lock serializes the shim's display selection and button state.
        unsafe { rs_input_select_display(display.id) };
        super::inject(ev);
    }

    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        let _lock = INPUT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        // SAFETY: INPUT_LOCK serializes display selection and all native input.
        unsafe { rs_input_select_display(display.id) };
        self.injector
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .inject(msg, unix_micros());
        if matches!(msg, InputMsg::ReleaseAll) {
            // SAFETY: the shim owns shared M0/v2 mouse state, under INPUT_LOCK.
            unsafe { rs_input_release_all() };
        }
    }

    fn release_all(&self) {
        let _lock = INPUT_LOCK.lock().unwrap_or_else(|p| p.into_inner());
        self.injector
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .release_all();
        // SAFETY: serialized with inject; the shim releases only pressed buttons.
        unsafe { rs_input_release_all() };
    }

    fn session_action(&self, kind: SessionActionKind) -> Result<()> {
        match kind {
            SessionActionKind::LockScreen => power::lock_screen(),
            SessionActionKind::WakeDisplay => power::wake_display(),
            SessionActionKind::CtrlAltDel => Err(PlatformError::Unsupported),
            // Lock-on-end is a host-core policy: when the session ends it
            // invokes LockScreen, so this platform action is intentionally a
            // successful no-op.
            SessionActionKind::LockOnEnd => Ok(()),
            SessionActionKind::Unspecified => Err(PlatformError::Unsupported),
        }
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        let platform = self.clone();
        cursor::watch(tx, move || platform.displays());
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        activity::watch(tx);
    }
    fn clipboard_get(&self) -> Option<ClipboardItem> {
        clipboard::get()
    }
    fn clipboard_set(&self, item: ClipboardItem) -> Result<()> {
        let change_count = clipboard::set(&item)?;
        self.clipboard_own_change
            .store(change_count, std::sync::atomic::Ordering::Release);
        Ok(())
    }
    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        clipboard::watch(tx, Arc::clone(&self.clipboard_own_change));
    }
    fn power_keep_awake(&self, on: bool) {
        power::keep_awake(on);
    }
    fn console_state(&self) -> ConsoleState {
        console::state()
    }
    fn screen_sharing_enabled(&self) -> Option<bool> {
        crate::sysquery::screen_sharing_enabled()
    }
    fn filevault_enabled(&self) -> Option<bool> {
        crate::sysquery::filevault_enabled()
    }
    fn permissions(&self) -> Permissions {
        perms::permissions()
    }
    fn request_permission(&self, p: PermissionKind) {
        perms::request(p);
    }
    fn nics(&self) -> Vec<Nic> {
        nics::enumerate()
    }
    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        wol::send(mac, via)
    }
    fn keystore(&self) -> &dyn KeyStore {
        self.keystore.as_ref()
    }
    fn system_authenticator(&self) -> Option<Arc<dyn host_core::sysauth::SystemAuthenticator>> {
        Some(Arc::new(crate::sysauth::MacAuthenticator))
    }
    fn virtual_displays(&self) -> Option<Arc<dyn host_core::vdisplay::VirtualDisplayBackend>> {
        crate::vdisplay::backend()
    }
}

fn error_message(err: &[c_char]) -> String {
    // SAFETY: every caller supplies a zero-initialized, shim NUL-terminated buffer.
    unsafe { CStr::from_ptr(err.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

struct NativeFrame(NonNull<c_void>);
// SAFETY: the retained CVPixelBuffer is immutable here; only VT reads its pixels.
unsafe impl Send for NativeFrame {}
// SAFETY: shared references only lend the immutable buffer to native encoders.
unsafe impl Sync for NativeFrame {}
impl Drop for NativeFrame {
    fn drop(&mut self) {
        // SAFETY: on_frame retained this buffer once for this owning wrapper.
        unsafe { rs_pixel_buffer_release(self.0.as_ptr()) };
    }
}

struct CaptureCtx {
    display_id: u32,
    sink: FrameSink,
}

extern "C" fn on_frame(
    ctx: *mut c_void,
    pixel: *mut c_void,
    width: u32,
    height: u32,
    capture_ts_us: u64,
) {
    let Some(pixel) = NonNull::new(pixel) else {
        return;
    };
    // SAFETY: capture stop drains the callback queue before freeing this context.
    let ctx = unsafe { &*(ctx as *const CaptureCtx) };
    // SAFETY: the callback lends a live CVPixelBuffer; retain extends its lifetime.
    unsafe { rs_pixel_buffer_retain(pixel.as_ptr()) };
    let _ = ctx.sink.try_send(CapturedFrame {
        display_id: ctx.display_id,
        width,
        height,
        capture_ts_us,
        native: Arc::new(NativeFrame(pixel)),
    });
}

struct MacCapture {
    handle: NonNull<c_void>,
    _ctx: Box<CaptureCtx>,
}
// SAFETY: the shim serializes capture callbacks and synchronously stops on Drop.
unsafe impl Send for MacCapture {}
impl CaptureHandle for MacCapture {}
impl Drop for MacCapture {
    fn drop(&mut self) {
        // SAFETY: handle is owned by this capture; stop drains callbacks using _ctx.
        unsafe { rs_frame_capture_stop(self.handle.as_ptr()) };
    }
}

struct EncoderCtx {
    sink: mpsc::Sender<EncodedFrame>,
    skip_until_key: AtomicBool,
    handle: AtomicPtr<c_void>,
}

extern "C" fn on_encoded(
    ctx: *mut c_void,
    data: *const u8,
    len: usize,
    flags: i32,
    capture_ts_us: u64,
) {
    // SAFETY: encoder stop completes all frames before freeing its context.
    let ctx = unsafe { &*(ctx as *const EncoderCtx) };
    let keyframe = flags & 1 != 0;
    let still_refine = flags & 2 != 0 && !keyframe;
    if ctx.skip_until_key.load(Ordering::Relaxed) && !keyframe {
        return;
    }
    if keyframe {
        ctx.skip_until_key.store(false, Ordering::Relaxed);
    }
    // SAFETY: VT lends len readable Annex-B bytes until this callback returns.
    let data = unsafe { std::slice::from_raw_parts(data, len) }.to_vec();
    if ctx
        .sink
        .try_send(EncodedFrame {
            data,
            keyframe,
            still_refine,
            capture_ts_us,
            encoded_ts_us: unix_micros(),
        })
        .is_err()
    {
        ctx.skip_until_key.store(true, Ordering::Relaxed);
        let handle = ctx.handle.load(Ordering::Acquire);
        if !handle.is_null() {
            // SAFETY: stop drains this callback before releasing handle; the shim
            // schedules a keyframe without blocking VT, including on a still screen.
            unsafe { rs_encoder_keyframe(handle) };
        }
    }
}

/// A VideoToolbox encoder on its own serial native queue.
pub struct MacEncoder {
    handle: NonNull<c_void>,
    width: u32,
    height: u32,
    stopped: bool,
    final_stats: EncoderStats,
    _ctx: Box<EncoderCtx>,
}
// SAFETY: all VT operations are dispatched to the encoder's serial native queue.
unsafe impl Send for MacEncoder {}
impl Encoder for MacEncoder {
    fn encode(&mut self, frame: CapturedFrame, opts: EncodeOpts) -> Result<()> {
        if self.stopped {
            return Err(PlatformError::Platform("encoder already finished".into()));
        }
        if frame.width != self.width || frame.height != self.height {
            return Err(PlatformError::Platform(
                "frame dimensions differ from encoder configuration".into(),
            ));
        }
        let pixel = frame
            .native
            .downcast_ref::<NativeFrame>()
            .map(|native| native.0)
            .or_else(|| {
                frame
                    .native
                    .downcast_ref::<SyntheticFrame>()
                    .map(|native| native.0)
            })
            .ok_or_else(|| PlatformError::Platform("not a macOS native frame".into()))?;
        // SAFETY: both owned native handles remain live; VT retains the submitted image.
        let status = unsafe {
            rs_encoder_encode(
                self.handle.as_ptr(),
                pixel.as_ptr(),
                frame.capture_ts_us,
                i32::from(opts.force_keyframe),
                i32::from(opts.quality_boost),
            )
        };
        if status == 0 {
            Ok(())
        } else {
            Err(PlatformError::Platform(format!(
                "VideoToolbox encode failed ({status})"
            )))
        }
    }
    fn set_rate(&mut self, kbps: u32, fps: u32) {
        if self.stopped {
            return;
        }
        // SAFETY: the encoder is live and the shim serializes property updates.
        unsafe {
            rs_encoder_rate(
                self.handle.as_ptr(),
                kbps.clamp(1, 150_000),
                fps.clamp(1, 120),
            )
        };
    }
    fn force_keyframe(&mut self) {
        if self.stopped {
            return;
        }
        // SAFETY: the shim also schedules a retained still frame if capture is idle.
        unsafe { rs_encoder_keyframe(self.handle.as_ptr()) };
    }
    fn set_quality_boost(&mut self, on: bool) {
        if self.stopped {
            return;
        }
        // SAFETY: the encoder is live and the shim serializes property updates.
        unsafe { rs_encoder_quality(self.handle.as_ptr(), i32::from(on)) };
    }
}

impl MacEncoder {
    /// Snapshot counters without stopping the encoder.
    pub fn stats(&self) -> EncoderStats {
        if self.stopped {
            return self.final_stats;
        }
        let mut native = NativeEncoderStats::default();
        // SAFETY: the native handle is live while this encoder is borrowed.
        unsafe { rs_encoder_stats(self.handle.as_ptr(), &mut native) };
        native.into()
    }

    /// Complete the session and return final counters. This is useful for
    /// probes because `VTCompressionSessionCompleteFrames` drains callbacks
    /// before the counters are copied.
    #[doc(hidden)]
    pub fn finish(&mut self) -> EncoderStats {
        if self.stopped {
            return self.final_stats;
        }
        let mut native = NativeEncoderStats::default();
        // SAFETY: stop completes all callbacks before releasing the native handle.
        unsafe { rs_encoder_stop(self.handle.as_ptr(), &mut native) };
        self.stopped = true;
        self._ctx
            .handle
            .store(std::ptr::null_mut(), Ordering::Release);
        self.final_stats = native.into();
        tracing::debug!(
            submitted = self.final_stats.submitted,
            output = self.final_stats.output,
            dropped = self.final_stats.dropped,
            callback_error = self.final_stats.callback_error,
            submit_error = self.final_stats.submit_error,
            callback_status_error = self.final_stats.callback_status_error,
            unexpected_null = self.final_stats.unexpected_null,
            conversion_error = self.final_stats.conversion_error,
            forced_key_retries = self.final_stats.forced_key_retries,
            "VideoToolbox encoder stats"
        );
        self.final_stats
    }

    /// Apply an alternate data-rate window for the step-response probe.
    #[doc(hidden)]
    pub fn set_rate_window(&mut self, kbps: u32, fps: u32, window_seconds: f64) {
        if self.stopped {
            return;
        }
        let window_seconds = if window_seconds.is_finite() {
            window_seconds.clamp(0.1, 2.0)
        } else {
            1.0
        };
        // SAFETY: the encoder is live and the shim serializes property updates.
        unsafe {
            rs_encoder_rate_window(
                self.handle.as_ptr(),
                kbps.clamp(1, 150_000),
                fps.clamp(1, 120),
                window_seconds,
            )
        };
    }
}

impl Drop for MacEncoder {
    fn drop(&mut self) {
        self.finish();
    }
}

#[cfg(test)]
mod tests {
    use super::region_points;
    use host_core::platform::PixelRect;

    #[test]
    fn region_points_divides_by_scale() {
        let rect = PixelRect {
            x: 736,
            y: 352,
            w: 608,
            h: 352,
        };
        assert_eq!(region_points(rect, 2.0), (368.0, 176.0, 304.0, 176.0));
        for scale in [1.0, 0.0, -1.0, f32::NAN, f32::INFINITY, f32::NEG_INFINITY] {
            assert_eq!(
                region_points(rect, scale),
                (736.0, 352.0, 608.0, 352.0),
                "scale {scale}"
            );
        }
    }
}
