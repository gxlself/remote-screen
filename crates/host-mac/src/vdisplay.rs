//! macOS 虚拟显示器（私有 CGVirtualDisplay，运行时探测）。M3 · V1。

use std::collections::HashMap;
use std::ffi::{c_char, c_void, CStr, CString};
use std::sync::atomic::{AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use host_core::platform::{DisplayInfo, Platform};
use host_core::vdisplay::arrange::{mac_rect, place_beside};
use host_core::vdisplay::mode::physical_size_mm;
use host_core::vdisplay::{VdCaps, VdCreated, VdError, VdMode, VirtualDisplayBackend};

use crate::MacPlatform;

extern "C" {
    fn rs_vd_supported() -> i32;
    fn rs_vd_create(
        name: *const c_char,
        width_px: u32,
        height_px: u32,
        hidpi: i32,
        refresh_hz: u32,
        width_mm: u32,
        height_mm: u32,
        serial: u32,
        mode_variant: u32,
        out_display_id: *mut u32,
        err: *mut c_char,
        errlen: usize,
    ) -> *mut c_void;
    fn rs_vd_destroy(handle: *mut c_void) -> i32;
    fn rs_vd_modes(display_id: u32, out: *mut DisplayMode, capacity: usize) -> usize;
    fn rs_vd_current_mode(display_id: u32, out: *mut DisplayMode) -> i32;
    fn rs_vd_set_mode(display_id: u32, mode_id: u32) -> i32;
    fn rs_vd_set_origin(display_id: u32, x: i32, y: i32) -> i32;
    fn rs_vd_wait_online(display_id: u32, timeout_ms: u32) -> i32;
    fn rs_vd_online_identities(out: *mut DisplayIdentity, capacity: usize) -> usize;
    fn rs_vd_queue_selectors() -> u32;
    fn rs_vd_diag_introspect();
    fn rs_vd_appkit_initialize() -> i32;
    fn rs_vd_run_main_loop(timeout_ms: u32) -> i32;
    fn rs_vd_diag_begin(out_error: *mut i32) -> *mut c_void;
    fn rs_vd_diag_end(handle: *mut c_void) -> i32;
    fn rs_vd_diag_pump(timeout_ms: u32) -> i32;
    fn rs_vd_diag_snapshot(out: *mut diagnostics::Events);
}

/// Owns the displays made by this process. The lock also serializes placement
/// and the display limit across concurrent create calls.
pub struct MacVirtualDisplays {
    handles: Mutex<HashMap<u64, VdHandle>>,
    next_token: AtomicU64,
}

struct VdHandle {
    ptr: *mut c_void,
    display_id: u32,
}

// SAFETY: the pointer is owned by this wrapper and is used only by the shim,
// which dispatches every private API operation to its single serial queue.
unsafe impl Send for VdHandle {}

impl Drop for VdHandle {
    fn drop(&mut self) {
        if self.ptr.is_null() {
            return;
        }
        // SAFETY: rs_vd_create retained this object once; this is its sole owner.
        let error = unsafe { rs_vd_destroy(self.ptr) };
        self.ptr = std::ptr::null_mut();
        if error != 0 {
            tracing::error!(
                display_id = self.display_id,
                cg_error = error,
                "virtual display did not disappear during drop"
            );
        }
    }
}

impl VdHandle {
    fn destroy(mut self) -> Result<(), VdError> {
        if self.ptr.is_null() {
            return Ok(());
        }
        // SAFETY: the handle owns this native pointer and consumes it exactly once.
        let error = unsafe { rs_vd_destroy(self.ptr) };
        self.ptr = std::ptr::null_mut();
        if error == 0 {
            Ok(())
        } else {
            Err(failed(format!(
                "virtual display {} did not disappear within 3 seconds (CGError {error})",
                self.display_id
            )))
        }
    }
}

/// 进程内单例；私有 API 不可用时为 None，不声明虚拟屏能力。
pub fn backend() -> Option<Arc<dyn VirtualDisplayBackend>> {
    static BACKEND: OnceLock<Option<Arc<dyn VirtualDisplayBackend>>> = OnceLock::new();
    BACKEND
        .get_or_init(|| {
            supported().then(|| {
                Arc::new(MacVirtualDisplays {
                    handles: Mutex::new(HashMap::new()),
                    next_token: AtomicU64::new(1),
                }) as Arc<dyn VirtualDisplayBackend>
            })
        })
        .clone()
}

/// Mac 的虚拟屏能力，尺寸上限和下限按长边 / 短边给。
pub fn mac_caps() -> VdCaps {
    VdCaps {
        max_long_px: 7680,
        max_short_px: 4320,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        scales: vec![1.0, 2.0],
        align_px: 4,
        max_displays: 2,
    }
}

/// HiDPI 使用 2 倍倍率；1.5 及以上的请求选 HiDPI。
pub fn is_hidpi(mode: &VdMode) -> bool {
    mode.scale >= 1.5
}

/// 只探测类和选择子，不创建显示器。结果在 native 层缓存。
pub fn supported() -> bool {
    // SAFETY: this takes no pointers and probes only Objective-C class metadata.
    unsafe { rs_vd_supported() != 0 }
}

/// Return the online display identities without creating or changing a mode.
pub fn online_display_identities() -> Vec<DisplayIdentity> {
    // SAFETY: a null output with zero capacity asks only for the count.
    let mut count = unsafe { rs_vd_online_identities(std::ptr::null_mut(), 0) };
    for _ in 0..3 {
        let mut displays = vec![DisplayIdentity::default(); count];
        // SAFETY: the shim writes at most displays.len() POD records during
        // this synchronous call and retains no Rust pointers.
        let available = unsafe { rs_vd_online_identities(displays.as_mut_ptr(), displays.len()) };
        if available <= displays.len() {
            displays.truncate(available);
            return displays;
        }
        count = available;
    }
    Vec::new()
}

fn next_serial() -> u32 {
    static SEQUENCE: OnceLock<AtomicU32> = OnceLock::new();
    let sequence = SEQUENCE.get_or_init(|| AtomicU32::new(0));
    let pid_prefix = std::process::id().wrapping_shl(8);
    loop {
        let serial = pid_prefix | (sequence.fetch_add(1, Ordering::Relaxed) & 0xff);
        if !online_display_identities()
            .iter()
            .any(|display| display.serial == serial)
        {
            return serial;
        }
    }
}

/// Initialize AppKit on the process main thread. A Host that chooses the
/// main-queue descriptor variant must call this once before creating a screen.
pub fn initialize_appkit() -> Result<(), VdError> {
    // SAFETY: native checks NSThread::isMainThread and catches AppKit
    // exceptions before returning a CoreGraphics error code.
    let error = unsafe { rs_vd_appkit_initialize() };
    if error == 0 {
        Ok(())
    } else {
        Err(failed(format!(
            "AppKit initialization failed: CGError {error}"
        )))
    }
}

/// Service AppKit/CoreGraphics sources for at most 50 ms. This is a scheduler
/// hook for a Host entry point that uses a main-queue descriptor callback;
/// callers must invoke it on the process main thread between async turns.
pub fn pump_main_run_loop(timeout: Duration) -> bool {
    let timeout_ms = timeout.as_millis().clamp(1, 50) as u32;
    // SAFETY: native rejects calls off the main thread and bounds the run-loop
    // interval. It retains no Rust pointers.
    unsafe { rs_vd_run_main_loop(timeout_ms) != 0 }
}

/// A system mode snapshot. Point and backing-pixel sizes are deliberately
/// separate: equal point sizes can describe either a 1x or a 2x backing store.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq)]
pub struct DisplayMode {
    pub mode_id: u32,
    pub width_px: u32,
    pub height_px: u32,
    pub width_pt: u32,
    pub height_pt: u32,
    /// Whether CoreGraphics allows this mode for the desktop GUI.
    pub usable: bool,
    /// Raw CoreGraphics value; zero means unknown, not 60 Hz.
    pub refresh_hz: f64,
}

/// Identity fields exposed by CoreGraphics for online displays. Probe-only;
/// production uses the same allocator to avoid colliding with orphaned RS
/// displays from an earlier process.
#[repr(C)]
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DisplayIdentity {
    pub id: u32,
    pub vendor: u32,
    pub model: u32,
    pub serial: u32,
}

impl DisplayMode {
    fn scale(self) -> Option<f32> {
        if self.width_px == 0 || self.height_px == 0 || self.width_pt == 0 || self.height_pt == 0 {
            return None;
        }
        let scale = self.width_px as f32 / self.width_pt as f32;
        let scale_y = self.height_px as f32 / self.height_pt as f32;
        if (scale - scale_y).abs() > 0.001 {
            return None;
        }
        Some(scale)
    }

    fn refresh(self) -> Option<u32> {
        let refresh_hz = self.refresh_hz.round();
        (1.0..=f64::from(u32::MAX))
            .contains(&refresh_hz)
            .then_some(refresh_hz as u32)
    }

    /// Convert measured dimensions/timing to the existing backend contract.
    /// An unknown refresh rate cannot truthfully populate an actual mode.
    pub fn actual(self) -> Option<VdMode> {
        Some(VdMode {
            width_px: self.width_px,
            height_px: self.height_px,
            scale: self.scale()?,
            refresh_hz: self.refresh()?,
        })
    }
}

/// Best available report when mode confirmation times out. The requested
/// geometry and scale remain authoritative because macOS 26 can expose stale
/// 1x data to the creating process while a fresh process sees the configured
/// 2x mode. A timeout also keeps the requested refresh explicitly unconfirmed;
/// only `configure_mode`'s successful mode-list path may report a measurement.
pub fn reported_mode(
    requested: VdMode,
    _observed: Option<DisplayMode>,
    _enumerated: Option<&DisplayInfo>,
) -> VdMode {
    requested
}

/// List all system modes, including duplicate low-resolution variants.
/// This reads display metadata only; it never allocates a virtual display.
/// CoreGraphics runs on the calling thread, not the descriptor callback queue.
pub fn display_modes(display_id: u32) -> Vec<DisplayMode> {
    // SAFETY: a null output with zero capacity asks only for the array length.
    let mut count = unsafe { rs_vd_modes(display_id, std::ptr::null_mut(), 0) };
    for _ in 0..3 {
        let mut modes = vec![DisplayMode::default(); count];
        // SAFETY: the shim writes at most modes.len() POD records and retains
        // no pointers after this synchronous call.
        let available = unsafe { rs_vd_modes(display_id, modes.as_mut_ptr(), modes.len()) };
        if available <= modes.len() {
            modes.truncate(available);
            return modes;
        }
        // applySettings can still be publishing modes between the two reads.
        count = available;
    }
    Vec::new()
}

/// Read points, backing pixels and timing from one CGDisplayMode snapshot.
pub fn current_mode(display_id: u32) -> Option<DisplayMode> {
    let mut mode = DisplayMode::default();
    // SAFETY: mode is a writable POD output that lives through the call.
    (unsafe { rs_vd_current_mode(display_id, &mut mode) } != 0).then_some(mode)
}

/// Select the nearest known refresh rate at exactly the requested pixels and
/// logical points. A 1x mode must never substitute for a requested 2x mode.
/// Unknown timings are excluded; ties choose the lower rate.
pub fn closest_mode(modes: &[DisplayMode], requested: &VdMode) -> Option<DisplayMode> {
    let divisor = if is_hidpi(requested) { 2 } else { 1 };
    let target_hz = f64::from(requested.refresh_hz);
    modes
        .iter()
        .filter(|candidate| {
            candidate.usable
                && candidate.width_px == requested.width_px
                && candidate.height_px == requested.height_px
                && candidate.width_pt.checked_mul(divisor) == Some(requested.width_px)
                && candidate.height_pt.checked_mul(divisor) == Some(requested.height_px)
                && candidate.actual().is_some()
        })
        .min_by(|a, b| {
            (a.refresh_hz - target_hz)
                .abs()
                .total_cmp(&(b.refresh_hz - target_hz).abs())
                .then_with(|| a.refresh_hz.total_cmp(&b.refresh_hz))
                .then_with(|| a.mode_id.cmp(&b.mode_id))
        })
        .copied()
}

fn same_mode(a: DisplayMode, b: DisplayMode) -> bool {
    a.width_px == b.width_px
        && a.height_px == b.height_px
        && a.width_pt == b.width_pt
        && a.height_pt == b.height_pt
        && (a.refresh_hz - b.refresh_hz).abs() < 0.5
}

// Called only after rs_vd_create has returned. Polling/sleeping never occupies
// either native queue. Mode confirmation is best effort and cannot destroy a
// successfully created display just because CoreGraphics metadata is late.
fn configure_mode(platform: &MacPlatform, display_id: u32, requested: &VdMode) -> (VdMode, bool) {
    let deadline = Instant::now() + Duration::from_secs(2);
    let mut applied = None;
    let mut previous = None;
    let mut last_observed = None;
    let mut last_error = 0;
    loop {
        let modes = display_modes(display_id);
        let preferred = closest_mode(&modes, requested);
        let current = current_mode(display_id);
        if current.is_some() {
            last_observed = current;
        }
        let now = Instant::now();
        if let Some((preferred, current)) = preferred.zip(current) {
            // Require two matching observations, allowing WindowServer to
            // publish the new backing size and points after a mode switch.
            // A fallback rate is accepted only after the full discovery
            // window, so an early 60 Hz mode cannot hide a late 30 Hz mode.
            if same_mode(current, preferred)
                && previous == Some(current)
                && ((preferred.refresh_hz - f64::from(requested.refresh_hz)).abs() < 0.5
                    || now >= deadline)
            {
                if let Some(actual) = current.actual() {
                    return (actual, true);
                }
            }
        }
        if now >= deadline {
            let enumerated = platform
                .displays()
                .into_iter()
                .find(|display| display.id == display_id);
            let reported = reported_mode(*requested, last_observed, enumerated.as_ref());
            let refresh_source = "request (unmeasured; no confirmed mode list)";
            tracing::warn!(
                event = "vdisplay.mac.mode_unconfirmed",
                display_id,
                mode_confirmed = false,
                requested = ?requested,
                reported = ?reported,
                refresh_source,
                current = ?current,
                last_observed = ?last_observed,
                available = ?modes,
                applied_mode = ?applied,
                enumerated = ?enumerated,
                cg_error = last_error,
                "virtual display mode unconfirmed (未确认); keeping created display"
            );
            return (reported, false);
        }
        if let Some(preferred) = preferred {
            if !current.is_some_and(|current| same_mode(current, preferred))
                && applied != Some(preferred)
            {
                // SAFETY: the display is owned by create's guard; the shim
                // looks up the mode and keeps its CFArray alive until the
                // synchronous CoreGraphics call on this worker returns.
                last_error = unsafe { rs_vd_set_mode(display_id, preferred.mode_id) };
                if last_error == 0 {
                    applied = Some(preferred);
                }
            }
        }
        previous = current;
        // create runs on a blocking worker. Never hold rs_vd_queue while
        // sleeping: descriptor callbacks must be able to finish configuration.
        std::thread::sleep(
            deadline
                .saturating_duration_since(Instant::now())
                .min(Duration::from_millis(50)),
        );
    }
}

fn failed(message: impl Into<String>) -> VdError {
    let message = message.into();
    tracing::warn!(error = %message, "virtual display creation failed");
    VdError::Failed(message)
}

fn create_handle(
    mode: VdMode,
    label: &str,
    serial: u32,
    mode_variant: u32,
) -> Result<VdHandle, VdError> {
    let (width_mm, height_mm) = physical_size_mm(&mode);
    let label = CString::new(label.replace('\0', " "))
        .map_err(|_| failed("virtual display label contains NUL"))?;
    let mut display_id = 0;
    let mut error = [0 as c_char; 512];
    // SAFETY: label and both output buffers live through this synchronous
    // call; the shim retains the returned object for the Rust handle.
    let ptr = unsafe {
        rs_vd_create(
            label.as_ptr(),
            mode.width_px,
            mode.height_px,
            i32::from(is_hidpi(&mode)),
            mode.refresh_hz,
            width_mm,
            height_mm,
            serial,
            mode_variant,
            &mut display_id,
            error.as_mut_ptr(),
            error.len(),
        )
    };
    if ptr.is_null() {
        // SAFETY: the zero-initialized buffer is NUL-terminated, and the
        // native error writer always preserves that terminator.
        let message = unsafe { CStr::from_ptr(error.as_ptr()) }.to_string_lossy();
        return Err(failed(if message.is_empty() {
            "virtual display creation failed".to_owned()
        } else {
            message.into_owned()
        }));
    }
    Ok(VdHandle { ptr, display_id })
}

/// Explicit real-display diagnostics for vdisplay_probe; never used by Host
/// startup or unit tests. Production always uses the V1F2 A sequence; these
/// variants remain isolated to the probe for regression evidence.
#[doc(hidden)]
pub mod diagnostics {
    use std::marker::PhantomData;
    use std::rc::Rc;

    use super::*;

    #[derive(Clone, Copy, Debug)]
    #[repr(u32)]
    pub enum Variant {
        Current = 0,
        Pixels = 1,
        Points = 2,
        PixelsAndPoints = 3,
        AppKit = 4,
        DeskPad = 5,
        DelayedApply = 6,
        RetainedObjects = 7,
    }

    pub struct Display {
        handle: VdHandle,
    }

    impl Display {
        pub fn create(mode: VdMode, variant: Variant) -> Result<Self, VdError> {
            Ok(Self {
                handle: create_handle(
                    mode,
                    "Remote Screen Diagnostic",
                    next_serial(),
                    variant as u32,
                )?,
            })
        }

        pub fn id(&self) -> u32 {
            self.handle.display_id
        }

        pub fn destroy(self) -> Result<(), VdError> {
            self.handle.destroy()
        }

        pub fn select_mode(&self, mode_id: u32) -> i32 {
            // SAFETY: this guard owns the display. Native retains the mode
            // array until CGDisplaySetDisplayMode returns on this caller.
            unsafe { rs_vd_set_mode(self.id(), mode_id) }
        }
    }

    #[derive(Clone, Copy, Debug, Default)]
    #[repr(C)]
    pub struct Events {
        pub callbacks: u64,
        pub main_callbacks: u64,
        pub last_display_id: u32,
        pub flags: u32,
    }

    // !Send + !Sync: pump and Drop must stay on the original process main
    // thread. A Tokio blocking worker cannot pump that thread's CFRunLoop.
    pub struct MainRunLoop {
        ptr: *mut c_void,
        _main_thread: PhantomData<Rc<()>>,
    }

    impl MainRunLoop {
        pub fn register() -> Result<Self, VdError> {
            let mut error = 0;
            // SAFETY: error lives through the call; native rejects calls off
            // the main thread and retains a source only for a non-null result.
            let ptr = unsafe { rs_vd_diag_begin(&mut error) };
            if ptr.is_null() {
                return Err(failed(format!(
                    "diagnostic main run loop registration failed: CGError {error}"
                )));
            }
            Ok(Self {
                ptr,
                _main_thread: PhantomData,
            })
        }

        pub fn pump(&self, timeout: Duration) {
            let timeout_ms = timeout.as_millis().clamp(1, 50) as u32;
            // SAFETY: this !Send owner was constructed on the main thread;
            // native bounds RunInMode to at most 50 ms and retains no pointers.
            unsafe { rs_vd_diag_pump(timeout_ms) };
        }

        pub fn events(&self) -> Events {
            let mut events = Events::default();
            // SAFETY: events is a writable POD output, retained only here.
            unsafe { rs_vd_diag_snapshot(&mut events) };
            events
        }
    }

    impl Drop for MainRunLoop {
        fn drop(&mut self) {
            // SAFETY: native retained this source for this sole main-thread
            // owner. Callback data is static, including for in-flight events.
            let error = unsafe { rs_vd_diag_end(self.ptr) };
            if error != 0 {
                tracing::warn!(
                    cg_error = error,
                    "diagnostic display observer removal failed"
                );
            }
        }
    }

    pub fn queue_selectors() -> (bool, bool) {
        // SAFETY: metadata-only Objective-C selector probes take no pointers.
        let selectors = unsafe { rs_vd_queue_selectors() };
        (selectors & 1 != 0, selectors & 2 != 0)
    }

    /// Print all runtime methods/properties for the four private classes.
    /// This is intentionally probe-only because the output is OS-version
    /// evidence rather than a stable application API.
    pub fn introspect() {
        // SAFETY: native reads Objective-C class metadata and writes only to
        // stderr; it retains no pointers and has no Rust-visible output.
        unsafe { rs_vd_diag_introspect() };
    }

    /// Initialize AppKit on the process main thread before E-H. The caller
    /// must continue pumping that thread's run loop while using main-queue
    /// descriptor callbacks.
    pub fn initialize_appkit() -> Result<(), VdError> {
        super::initialize_appkit()
    }
}

impl VirtualDisplayBackend for MacVirtualDisplays {
    fn caps(&self) -> Option<VdCaps> {
        Some(mac_caps())
    }

    fn create(&self, mode: VdMode, label: &str) -> Result<VdCreated, VdError> {
        let mut handles = self.handles.lock().unwrap_or_else(|p| p.into_inner());
        if handles.len() >= mac_caps().max_displays as usize {
            return Err(VdError::Limit);
        }
        let hidpi = is_hidpi(&mode);
        let serial = next_serial();
        // The temporary owner also releases the screen on a timeout or unwind.
        let handle = create_handle(mode, label, serial, 0)?;
        let display_id = handle.display_id;
        // SAFETY: display_id was produced by the live display handle above.
        if unsafe { rs_vd_wait_online(handle.display_id, 2000) } == 0 {
            return Err(failed("virtual display did not come online"));
        }

        let platform = MacPlatform::new();
        let existing = platform
            .displays()
            .into_iter()
            .filter(|display| display.id != handle.display_id)
            .map(|display| (mac_rect(&display), display.primary))
            .collect::<Vec<_>>();
        let (x, y) = place_beside(&existing);
        // SAFETY: the display remains owned by handle until it is stored below.
        let error = unsafe { rs_vd_set_origin(handle.display_id, x, y) };
        if error != 0 {
            tracing::warn!("could not arrange virtual display: CGError {error}");
        }
        // Placement can itself reconfigure the display. Select the mode after
        // that transaction; unreadable fields remain explicitly unconfirmed.
        let (reported, mode_confirmed) = configure_mode(&platform, display_id, &mode);

        let token = self.next_token.fetch_add(1, Ordering::Relaxed);
        handles.insert(token, handle);
        tracing::info!(
            event = "vdisplay.mac.created",
            display_id,
            width = reported.width_px,
            height = reported.height_px,
            scale = reported.scale,
            requested_refresh_hz = mode.refresh_hz,
            reported_refresh_hz = reported.refresh_hz,
            mode_confirmed,
            hidpi
        );
        Ok(VdCreated {
            token,
            platform_id: Some(display_id),
            match_name: None,
            mode: reported,
        })
    }

    fn destroy(&self, token: u64) -> Result<(), VdError> {
        let handle = self
            .handles
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(&token);
        match handle {
            Some(handle) => handle.destroy(),
            None => Ok(()),
        }
    }

    fn cleanup_stale(&self) -> usize {
        // WindowServer owns process-exit cleanup, including SIGKILL. The probe
        // exercises this separately because ordinary unit tests never build screens.
        0
    }
}

impl Drop for MacVirtualDisplays {
    fn drop(&mut self) {
        self.handles
            .get_mut()
            .unwrap_or_else(|p| p.into_inner())
            .clear();
    }
}
