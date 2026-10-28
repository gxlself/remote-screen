#![cfg(windows)]
//! One Desktop Duplication worker per display, shared by all subscribers.
//!
//! Call `displays::ensure_dpi_awareness()` at session-process startup.
//! `Platform::capture` delegates to `capture::start(..).map(|c| Box::new(c) as
//! Box<dyn CaptureHandle>)`; `Platform::watch_cursor` delegates to `watch_cursor`.
//! First start can block up to 1 s; Drop normally takes at most about 60 ms.
//! Tokio handlers call synchronously; session startup may use `block_in_place`.
//!
//! Encoders downcast `frame.native.downcast_ref::<WinFrame>()` inside `Encoder::encode`.
//! Encoders are constructed before capture: initialize D3D11 lazily from the
//! first frame's device and rebuild on `device_gen` changes. Frames have native
//! duplication dimensions and unrotated scan orientation. The encoder performs scaling
//! to EncoderConfig, BGRA -> NV12 and `rotation` with the D3D11 Video Processor.
//! Pool textures can be reused after all CapturedFrame references are dropped.
//! Submit GPU reads on this device's immediate context before releasing a frame;
//! ordered commands on that context suffice. Other devices/contexts must finish
//! their own synchronization before releasing it.
//!
//! Capture policy: 50 ms acquire timeout, desktop polling every 100 ms,
//! duplication pointer shapes (no GetCursorInfo fallback), and no scaling.
//! DesktopImageInSystemMemory follows the ordinary resource-copy path; that
//! configuration has not been validated on hardware.

use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        mpsc, Arc, Mutex, MutexGuard, OnceLock,
    },
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use host_core::platform::{
    CaptureConfig, CaptureHandle, CapturedFrame, CursorUpdate, FrameSink, PlatformError,
};
use tokio::sync::mpsc::{error::TrySendError, Sender};
use tracing::{debug, info, trace, warn};
use windows::{
    core::{Error, Interface, HRESULT},
    Win32::{
        Foundation::{E_FAIL, HMODULE},
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
            },
            Direct3D11::*,
            Dxgi::{
                Common::*, CreateDXGIFactory1, IDXGIAdapter1, IDXGIFactory1, IDXGIOutput,
                IDXGIOutput1, IDXGIOutput5, IDXGIOutputDuplication, DXGI_ERROR_MORE_DATA,
                DXGI_OUTDUPL_DESC, DXGI_OUTDUPL_FRAME_INFO, DXGI_OUTDUPL_POINTER_SHAPE_INFO,
                DXGI_OUTPUT_DESC,
            },
        },
        System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency},
    },
};

use crate::{
    desktop, displays,
    frame_gate::{FrameAction, FrameGate, FrameInfoLite},
    winproc_model::DesktopTracker,
};
use host_core::{
    capture_model::*,
    cursor_shape::{to_cursor_shape, PointerShape},
    display_model::{display_id, RectPx},
};

/// Owned BGRA desktop copy; no handwritten Send/Sync implementation is needed.
pub struct WinFrame {
    /// B8G8R8A8_UNORM, DEFAULT, SHADER_RESOURCE | RENDER_TARGET, MiscFlags = 0.
    pub texture: ID3D11Texture2D,
    /// Its immediate context has multithread protection enabled.
    pub device: ID3D11Device,
    pub device_gen: u64,
    pub vendor_id: u32,
    pub adapter_luid: i64,
    /// Native, unrotated scan texture; clockwise display rotation in degrees.
    pub rotation: u32,
}

pub struct WinCapture {
    screen_id: u32,
    sub_id: u64,
    worker: Arc<Worker>,
}
impl CaptureHandle for WinCapture {}
impl WinCapture {
    pub fn stats(&self) -> CaptureStats {
        self.worker.stats()
    }
}

struct Shared {
    fanout: Fanout<CapturedFrame, FrameSink>,
    stats: CaptureStats,
}
struct Worker {
    shared: Mutex<Shared>,
    stop: AtomicBool,
    alive: AtomicBool,
    thread: Mutex<Option<JoinHandle<()>>>,
}
static WORKERS: OnceLock<Mutex<HashMap<u32, Arc<Worker>>>> = OnceLock::new();
static NEXT_SUB: AtomicU64 = AtomicU64::new(1);
static DEVICE_GEN: AtomicU64 = AtomicU64::new(1);
fn registry() -> &'static Mutex<HashMap<u32, Arc<Worker>>> {
    WORKERS.get_or_init(Mutex::default)
}
fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}
fn platform_error(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Platform(error.to_string())
}
fn absent() -> Error {
    Error::from_hresult(E_FAIL)
}

impl Worker {
    fn stats(&self) -> CaptureStats {
        let shared = lock(&self.shared);
        CaptureStats {
            subscribers: shared.fanout.len().min(u32::MAX as usize) as u32,
            ..shared.stats
        }
    }
    fn update(&self, f: impl FnOnce(&mut CaptureStats)) {
        f(&mut lock(&self.shared).stats);
    }
    fn wake(&self) {
        if let Some(t) = lock(&self.thread).as_ref() {
            t.thread().unpark();
        }
    }
    fn join(&self) {
        let handle = lock(&self.thread).take();
        if let Some(handle) = handle {
            if handle.join().is_err() {
                warn!("capture worker panicked");
            }
        }
    }
    fn poll(&self, now: Duration) {
        let mut shared = lock(&self.shared);
        let report = shared
            .fanout
            .poll(now, |sink, frame| match sink.try_send(frame) {
                Ok(()) => SendResult::Sent,
                Err(TrySendError::Full(frame)) => SendResult::Full(frame),
                Err(TrySendError::Closed(_)) => SendResult::Closed,
            });
        shared.stats.sent += u64::from(report.sent);
        shared.stats.sink_full += u64::from(report.full);
    }
}

pub fn start(
    screen_id: u32,
    cfg: CaptureConfig,
    sink: FrameSink,
) -> Result<WinCapture, PlatformError> {
    displays::ensure_dpi_awareness();
    if !(MIN_FPS..=MAX_FPS).contains(&cfg.fps) {
        return Err(platform_error("capture fps must be 1..120"));
    }
    if !displays::enumerate()
        .iter()
        .any(|r| display_id(&r.device_name) == screen_id)
    {
        return Err(platform_error("unknown display"));
    }
    debug!(
        screen_id,
        max_width = cfg.max_width,
        max_height = cfg.max_height,
        "capture uses native size; encoder handles scaling"
    );
    let sub_id = NEXT_SUB.fetch_add(1, Ordering::Relaxed);
    let mut workers = lock(registry());
    if let Some(worker) = workers.get(&screen_id) {
        let mut shared = lock(&worker.shared);
        // Give-up marks alive while holding this same lock, so registration
        // cannot add a sink after the terminating worker drains its subscribers.
        if worker.alive.load(Ordering::Acquire) {
            shared.fanout.add(sub_id, cfg.fps, sink);
            drop(shared);
            worker.wake();
            return Ok(WinCapture {
                screen_id,
                sub_id,
                worker: Arc::clone(worker),
            });
        }
    }
    if let Some(old) = workers.remove(&screen_id) {
        old.join();
    }
    let mut fanout = Fanout::new();
    fanout.add(sub_id, cfg.fps, sink);
    let worker = Arc::new(Worker {
        shared: Mutex::new(Shared {
            fanout,
            stats: CaptureStats::default(),
        }),
        stop: AtomicBool::new(false),
        alive: AtomicBool::new(true),
        thread: Mutex::new(None),
    });
    let (ready_tx, ready_rx) = mpsc::sync_channel(1);
    let thread_worker = Arc::clone(&worker);
    let handle = thread::Builder::new()
        .name(format!("rs-capture-{screen_id}"))
        .spawn(move || run(screen_id, &thread_worker, ready_tx))
        .map_err(platform_error)?;
    *lock(&worker.thread) = Some(handle);
    workers.insert(screen_id, Arc::clone(&worker));
    drop(workers); // Never wait for readiness while holding the registry.
    let capture = WinCapture {
        screen_id,
        sub_id,
        worker,
    };
    match ready_rx.recv_timeout(START_WAIT) {
        Ok(Ok(())) | Err(mpsc::RecvTimeoutError::Timeout) => Ok(capture),
        Ok(Err(reason)) => {
            drop(capture);
            Err(platform_error(reason))
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            drop(capture);
            Err(platform_error("capture worker ended before ready"))
        }
    }
}

impl Drop for WinCapture {
    fn drop(&mut self) {
        let mut workers = lock(registry());
        let empty = {
            let mut shared = lock(&self.worker.shared);
            shared.fanout.remove(self.sub_id);
            shared.fanout.is_empty()
        };
        if empty {
            self.worker.stop.store(true, Ordering::Release);
            self.worker.wake();
            // No worker code locks the registry, or holds shared while joining.
            self.worker.join();
            if workers
                .get(&self.screen_id)
                .is_some_and(|w| Arc::ptr_eq(w, &self.worker))
            {
                workers.remove(&self.screen_id);
            }
        }
    }
}

#[derive(Debug)]
pub struct OutputInfo {
    pub adapter: String,
    pub vendor_id: u32,
    pub device_name: String,
    pub display_id: u32,
    pub desktop: RectPx,
    pub attached: bool,
    pub rotation: u32,
}
struct Output {
    adapter: IDXGIAdapter1,
    output: IDXGIOutput,
    desc: DXGI_OUTPUT_DESC,
}
fn rect(desc: &DXGI_OUTPUT_DESC) -> RectPx {
    let r = desc.DesktopCoordinates;
    RectPx {
        left: r.left,
        top: r.top,
        width: (i64::from(r.right) - i64::from(r.left)).clamp(0, i64::from(u32::MAX)) as u32,
        height: (i64::from(r.bottom) - i64::from(r.top)).clamp(0, i64::from(u32::MAX)) as u32,
    }
}
fn outputs() -> windows::core::Result<Vec<Output>> {
    // SAFETY: The factory's COM reference is owned by this function.
    let factory: IDXGIFactory1 = unsafe { CreateDXGIFactory1()? };
    let mut outputs = Vec::new();
    for i in 0..u32::MAX {
        // SAFETY: factory is live; the API validates enumeration indices.
        let adapter = match unsafe { factory.EnumAdapters1(i) } {
            Ok(a) => a,
            Err(e) if classify_hr(e.code().0) == HrClass::NotFound => break,
            Err(e) => return Err(e),
        };
        for j in 0..u32::MAX {
            // SAFETY: adapter is live; the API validates enumeration indices.
            let output = match unsafe { adapter.EnumOutputs(j) } {
                Ok(o) => o,
                Err(e) if classify_hr(e.code().0) == HrClass::NotFound => break,
                Err(e) => return Err(e),
            };
            // SAFETY: live output; GetDesc returns an owned value.
            let desc = unsafe { output.GetDesc()? };
            outputs.push(Output {
                adapter: adapter.clone(),
                output,
                desc,
            });
        }
    }
    Ok(outputs)
}
pub fn list_outputs() -> Result<Vec<OutputInfo>, PlatformError> {
    displays::ensure_dpi_awareness();
    outputs()
        .and_then(|outputs| {
            outputs
                .into_iter()
                .map(|o| {
                    // SAFETY: the output record owns a live adapter reference.
                    let adapter = unsafe { o.adapter.GetDesc1()? };
                    let device_name = device_name_from_wide(&o.desc.DeviceName);
                    Ok(OutputInfo {
                        adapter: device_name_from_wide(&adapter.Description),
                        vendor_id: adapter.VendorId,
                        display_id: display_id(&device_name),
                        device_name,
                        desktop: rect(&o.desc),
                        attached: o.desc.AttachedToDesktop.as_bool(),
                        rotation: dxgi_rotation_degrees(o.desc.Rotation.0),
                    })
                })
                .collect()
        })
        .map_err(platform_error)
}

struct Device {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    output: IDXGIOutput1,
    output5: Option<IDXGIOutput5>,
    generation: u64,
    vendor: u32,
    luid: i64,
}
impl Device {
    fn create(screen_id: u32) -> windows::core::Result<Self> {
        let mut outputs = outputs()?;
        let names: Vec<_> = outputs
            .iter()
            .map(|o| {
                (
                    device_name_from_wide(&o.desc.DeviceName),
                    o.desc.AttachedToDesktop.as_bool(),
                )
            })
            .collect();
        let index = find_output(&names, screen_id)
            .ok_or_else(|| Error::from_hresult(HRESULT(DXGI_ERROR_NOT_FOUND)))?;
        let o = outputs.swap_remove(index);
        let adapter = o
            .adapter
            .cast::<windows::Win32::Graphics::Dxgi::IDXGIAdapter>()?;
        let create = |flags| {
            let mut device = None;
            let mut context = None;
            // SAFETY: adapter owns the target output; all output pointers and
            // feature-level entries are live for the synchronous call.
            unsafe {
                D3D11CreateDevice(
                    Some(&adapter),
                    D3D_DRIVER_TYPE_UNKNOWN,
                    HMODULE::default(),
                    flags,
                    Some(&[
                        D3D_FEATURE_LEVEL_11_1,
                        D3D_FEATURE_LEVEL_11_0,
                        D3D_FEATURE_LEVEL_10_1,
                        D3D_FEATURE_LEVEL_10_0,
                    ]),
                    D3D11_SDK_VERSION,
                    Some(&mut device),
                    None,
                    Some(&mut context),
                )?;
            }
            Ok::<_, Error>((device.ok_or_else(absent)?, context.ok_or_else(absent)?))
        };
        let (device, context) = create(D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT).or_else(|e| {
            warn!(screen_id, error = %e, "D3D11 video support unavailable; retrying without VIDEO_SUPPORT");
            create(D3D11_CREATE_DEVICE_BGRA_SUPPORT)
        })?;
        let multithread: ID3D11Multithread = device.cast()?;
        // SAFETY: live interface; enables serialization for downstream encoders.
        unsafe {
            let _ = multithread.SetMultithreadProtected(true);
        }
        // SAFETY: live adapter, returning an owned descriptor.
        let desc = unsafe { o.adapter.GetDesc1()? };
        Ok(Self {
            device,
            context,
            output: o.output.cast()?,
            output5: o.output.cast().ok(),
            generation: DEVICE_GEN.fetch_add(1, Ordering::Relaxed),
            vendor: desc.VendorId,
            luid: (i64::from(desc.AdapterLuid.HighPart) << 32)
                | i64::from(desc.AdapterLuid.LowPart),
        })
    }
    fn duplicate(&mut self, screen_id: u32) -> windows::core::Result<Duplication> {
        // A factory/output can outlive the topology it described. Refresh only
        // on reconstruction so unplugged displays reach the NotFound grace
        // period, and a moved output gets a device on its new adapter.
        let mut current = outputs()?;
        let names: Vec<_> = current
            .iter()
            .map(|o| {
                (
                    device_name_from_wide(&o.desc.DeviceName),
                    o.desc.AttachedToDesktop.as_bool(),
                )
            })
            .collect();
        let index = find_output(&names, screen_id)
            .ok_or_else(|| Error::from_hresult(HRESULT(DXGI_ERROR_NOT_FOUND)))?;
        let output = current.swap_remove(index);
        // SAFETY: the refreshed output owns a live adapter reference.
        let adapter = unsafe { output.adapter.GetDesc1()? };
        let luid = (i64::from(adapter.AdapterLuid.HighPart) << 32)
            | i64::from(adapter.AdapterLuid.LowPart);
        if luid != self.luid {
            return Err(Error::from_hresult(HRESULT(DXGI_ERROR_DEVICE_REMOVED)));
        }
        self.output = output.output.cast()?;
        self.output5 = output.output.cast().ok();
        // SAFETY: the output and device remain live and belong to one adapter.
        let (desc, dup) = unsafe {
            let desc = self.output.GetDesc()?;
            if !desc.AttachedToDesktop.as_bool() {
                return Err(Error::from_hresult(HRESULT(DXGI_ERROR_NOT_FOUND)));
            }
            let dup = if let Some(output) = &self.output5 {
                output.DuplicateOutput1(&self.device, 0, &[DXGI_FORMAT_B8G8R8A8_UNORM])?
            } else {
                self.output.DuplicateOutput(&self.device)?
            };
            (desc, dup)
        };
        // SAFETY: newly created, live duplication; descriptor is copied out.
        let mode = unsafe { dup.GetDesc() };
        let dpi = displays::enumerate()
            .iter()
            .find(|r| display_id(&r.device_name) == screen_id)
            .map_or(96, |r| r.dpi);
        Ok(Duplication {
            dup,
            mode,
            desktop: rect(&desc),
            scale: if dpi == 0 { 1.0 } else { dpi as f32 / 96.0 },
        })
    }
    fn allocate(&self, dup: &Duplication) -> windows::core::Result<WinFrame> {
        let desc = texture_desc(dup.mode.ModeDesc.Width, dup.mode.ModeDesc.Height);
        let texture = create_texture(&self.device, &desc)?;
        Ok(WinFrame {
            texture,
            device: self.device.clone(),
            device_gen: self.generation,
            vendor_id: self.vendor,
            adapter_luid: self.luid,
            rotation: dxgi_rotation_degrees(dup.mode.Rotation.0),
        })
    }
}
struct Duplication {
    dup: IDXGIOutputDuplication,
    mode: DXGI_OUTDUPL_DESC,
    desktop: RectPx,
    scale: f32,
}
struct Acquired<'a>(&'a IDXGIOutputDuplication);
impl Drop for Acquired<'_> {
    fn drop(&mut self) {
        // SAFETY: constructed only after successful AcquireNextFrame; exactly
        // one guard owns the release, including every error/unwind path.
        if let Err(error) = unsafe { self.0.ReleaseFrame() } {
            trace!(%error, "ReleaseFrame failed");
        }
    }
}
fn texture_desc(width: u32, height: u32) -> D3D11_TEXTURE2D_DESC {
    D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_B8G8R8A8_UNORM,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_SHADER_RESOURCE | D3D11_BIND_RENDER_TARGET).0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    }
}
fn create_texture(
    device: &ID3D11Device,
    desc: &D3D11_TEXTURE2D_DESC,
) -> windows::core::Result<ID3D11Texture2D> {
    let mut texture = None;
    // SAFETY: valid descriptor and writable COM output; no initial data pointers.
    unsafe {
        device.CreateTexture2D(desc, None, Some(&mut texture))?;
    }
    texture.ok_or_else(absent)
}

struct Exit<'a> {
    screen_id: u32,
    worker: &'a Worker,
}
impl Drop for Exit<'_> {
    fn drop(&mut self) {
        let sinks = {
            let mut shared = lock(&self.worker.shared);
            self.worker.alive.store(false, Ordering::Release);
            shared.fanout.drain()
        };
        drop(sinks);
        lock(cursor_bus()).model.display_stopped(self.screen_id);
        debug!(screen_id = self.screen_id, stats = ?self.worker.stats(), "capture worker stopped");
    }
}
fn run(screen_id: u32, worker: &Worker, ready: mpsc::SyncSender<Result<(), String>>) {
    // Declared first: native objects below are released before exit cleanup.
    // A concurrent start always joins this thread before replacing its worker.
    let _exit = Exit { screen_id, worker };
    let mut ready = Some(ready);
    let clock = Instant::now();
    let mut recovery = Recovery::new();
    let mut gate = FrameGate::new();
    let mut pool = FramePool::new(POOL_MAX);
    let mut desktop_poll = Periodic::new(DESKTOP_POLL);
    let mut tracker = DesktopTracker::new();
    let mut pool_warning = Periodic::new(Duration::from_secs(10));
    let mut device: Option<Device> = None;
    let mut duplication: Option<Duplication> = None;
    let mut reason = String::from("capture initialization failed");
    let mut qpc_freq = 0;
    // SAFETY: pointer is a live writable i64; failure falls back to Unix now.
    if unsafe { QueryPerformanceFrequency(&mut qpc_freq) }.is_err() {
        qpc_freq = 0;
    }
    while !worker.stop.load(Ordering::Acquire) {
        let now = clock.elapsed();
        match recovery.step(now) {
            Step::GiveUp => {
                let sinks = {
                    let mut shared = lock(&worker.shared);
                    worker.alive.store(false, Ordering::Release);
                    shared.fanout.drain()
                };
                drop(sinks);
                warn!(screen_id, %reason, "capture giving up");
                if let Some(tx) = ready.take() {
                    let _ = tx.send(Err(reason));
                }
                break;
            }
            Step::Wait(d) => thread::park_timeout(d),
            Step::CreateDevice => {
                duplication = None;
                pool.clear();
                lock(&worker.shared).fanout.clear();
                gate.reset();
                device = None;
                match Device::create(screen_id) {
                    Ok(d) => {
                        device = Some(d);
                        recovery.device_result(clock.elapsed(), Ok(()));
                        worker.update(|s| s.device_creates += 1);
                    }
                    Err(e) => {
                        reason = format!("create device: {e}");
                        debug!(screen_id, %reason, "capture retry");
                        recovery.device_result(clock.elapsed(), Err(classify_hr(e.code().0)));
                    }
                }
            }
            Step::CreateDuplication => {
                duplication = None;
                pool.clear();
                lock(&worker.shared).fanout.clear();
                gate.reset();
                match desktop::attach_thread_to_input_desktop() {
                    Ok(name) => {
                        // ACCESS_LOST can arrive before the periodic poll. Still
                        // account for that switch when attachment observes it.
                        if tracker.observe(Some(&name)).is_some() && worker.stats().dup_creates > 0
                        {
                            worker.update(|s| s.desktop_switches += 1);
                            info!(screen_id, desktop = %name, "capture input desktop changed");
                        }
                    }
                    Err(PlatformError::Platform(message))
                        if message.starts_with("SetThreadDesktop") =>
                    {
                        reason = message;
                        device = None;
                        recovery.desktop_attach_failed(clock.elapsed());
                        continue;
                    }
                    Err(error) => {
                        debug!(screen_id, %error, "input desktop inaccessible; trying duplication on current desktop")
                    }
                }
                let Some(d) = device.as_mut() else {
                    recovery.desktop_attach_failed(clock.elapsed());
                    continue;
                };
                match d.duplicate(screen_id) {
                    Ok(dup) => {
                        duplication = Some(dup);
                        recovery.duplication_result(clock.elapsed(), Ok(()));
                        worker.update(|s| s.dup_creates += 1);
                        if let Some(tx) = ready.take() {
                            let _ = tx.send(Ok(()));
                        }
                    }
                    Err(e) => {
                        reason = format!("create duplication: {e}");
                        debug!(screen_id, %reason, "capture retry");
                        recovery.duplication_result(clock.elapsed(), Err(classify_hr(e.code().0)));
                    }
                }
            }
            Step::Acquire => {
                if desktop_poll.due(now) {
                    if let Some(name) = tracker.observe(desktop::input_desktop_name().as_deref()) {
                        recovery.desktop_switched();
                        worker.update(|s| s.desktop_switches += 1);
                        info!(screen_id, desktop=%name, "capture input desktop changed");
                        continue;
                    }
                }
                worker.poll(now);
                let timeout = lock(&worker.shared)
                    .fanout
                    .wait_hint(clock.elapsed(), ACQUIRE_TIMEOUT);
                if let (Some(d), Some(dup)) = (&device, &duplication) {
                    let mut capture = AcquireContext {
                        screen_id,
                        device: d,
                        duplication: dup,
                        worker,
                        gate: &mut gate,
                        pool: &mut pool,
                        pool_warning: &mut pool_warning,
                        clock: &clock,
                        qpc_freq,
                    };
                    let class = match capture.acquire(timeout) {
                        Ok(()) => HrClass::Ok,
                        Err(e) => {
                            let class = classify_hr(e.code().0);
                            if class != HrClass::Timeout {
                                reason = format!("acquire: {e}");
                                debug!(screen_id, ?class, %reason, "capture recovering");
                            }
                            class
                        }
                    };
                    recovery.acquire_result(clock.elapsed(), class);
                    worker.poll(clock.elapsed());
                } else {
                    recovery.desktop_attach_failed(clock.elapsed());
                }
            }
        }
    }
}

struct AcquireContext<'a> {
    screen_id: u32,
    device: &'a Device,
    duplication: &'a Duplication,
    worker: &'a Worker,
    gate: &'a mut FrameGate,
    pool: &'a mut FramePool<WinFrame>,
    pool_warning: &'a mut Periodic,
    clock: &'a Instant,
    qpc_freq: i64,
}
impl AcquireContext<'_> {
    fn acquire(&mut self, timeout: Duration) -> windows::core::Result<()> {
        let dup = self.duplication;
        let mut info = DXGI_OUTDUPL_FRAME_INFO::default();
        let mut resource = None;
        // Round upward to avoid a sub-ms due time turning into a busy spin.
        let timeout_ms = timeout
            .as_nanos()
            .div_ceil(1_000_000)
            .min(u128::from(u32::MAX)) as u32;
        // SAFETY: outputs are live writable locals; no previous frame is held.
        unsafe {
            dup.dup
                .AcquireNextFrame(timeout_ms, &mut info, &mut resource)?;
        }
        let _frame = Acquired(&dup.dup);
        self.worker.update(|s| s.acquired += 1);
        update_pointer(self.screen_id, dup, &info);
        let action = self.gate.decide(&FrameInfoLite {
            last_present_time: info.LastPresentTime,
            accumulated_frames: info.AccumulatedFrames,
            total_metadata_buffer_size: info.TotalMetadataBufferSize,
            last_mouse_update_time: info.LastMouseUpdateTime,
        });
        if action != FrameAction::Encode {
            self.worker.update(|s| s.skipped += 1);
            return Ok(());
        }
        let desktop: ID3D11Texture2D = resource.ok_or_else(absent)?.cast()?;
        let slot = match self.pool.checkout(|| self.device.allocate(dup)) {
            Ok(Some(slot)) => slot,
            Err(e) if classify_hr(e.code().0) == HrClass::DeviceLost => return Err(e),
            result => {
                self.worker.update(|s| s.pool_exhausted += 1);
                self.gate.reset();
                if self.pool_warning.due(self.clock.elapsed()) {
                    let error = result.err().map(|e| e.to_string());
                    warn!(
                        screen_id = self.screen_id,
                        ?error,
                        "capture texture pool unavailable; dropping frame"
                    );
                }
                return Ok(());
            }
        };
        // SAFETY: both textures have matching dimensions/format on this device;
        // slot has no downstream owners until publish and the desktop is acquired.
        unsafe {
            self.device.context.CopyResource(&slot.texture, &desktop);
        }
        let mut qpc_now = 0;
        // SAFETY: writable i64 output; zero falls back to the Unix timestamp.
        let _ = unsafe { QueryPerformanceCounter(&mut qpc_now) };
        let frame = CapturedFrame {
            display_id: self.screen_id,
            width: dup.mode.ModeDesc.Width,
            height: dup.mode.ModeDesc.Height,
            capture_ts_us: present_to_unix_us(
                info.LastPresentTime,
                qpc_now,
                self.qpc_freq,
                rsp_core::clock::unix_micros(),
            ),
            native: slot,
        };
        let mut shared = lock(&self.worker.shared);
        shared.fanout.publish(frame);
        shared.stats.published += 1;
        trace!(screen_id = self.screen_id, "captured desktop frame");
        Ok(())
    }
}

#[derive(Default)]
struct CursorBus {
    model: CursorModel,
    watchers: Vec<Sender<CursorUpdate>>,
}
static CURSOR: OnceLock<Mutex<CursorBus>> = OnceLock::new();
fn cursor_bus() -> &'static Mutex<CursorBus> {
    CURSOR.get_or_init(Mutex::default)
}
impl CursorBus {
    fn broadcast(&mut self, update: CursorUpdate) {
        let mut forgotten = None;
        self.watchers
            .retain(|tx| match tx.try_send(update.clone()) {
                Ok(()) => true,
                Err(TrySendError::Closed(_)) => false,
                Err(TrySendError::Full(CursorUpdate::Shape(shape))) => {
                    forgotten = Some(shape.id);
                    true
                }
                Err(TrySendError::Full(_)) => true,
            });
        if let Some(id) = forgotten {
            self.model.forget_shape(id);
        }
    }
}
/// Registers a cursor watcher and sends the current snapshot without blocking.
/// Cursor updates exist only while at least one display is being captured.
pub fn watch_cursor(tx: Sender<CursorUpdate>) {
    let mut bus = lock(cursor_bus());
    bus.model.reset_shapes();
    for update in bus.model.snapshot() {
        match tx.try_send(update) {
            Ok(()) => {}
            Err(TrySendError::Closed(_)) => return,
            Err(TrySendError::Full(CursorUpdate::Shape(shape))) => bus.model.forget_shape(shape.id),
            Err(TrySendError::Full(_)) => {}
        }
    }
    bus.watchers.push(tx);
}
fn pointer_shape(
    dup: &IDXGIOutputDuplication,
    size: u32,
) -> windows::core::Result<Option<PointerShape>> {
    let mut data = vec![0; size as usize];
    let mut required = 0;
    let mut shape = DXGI_OUTDUPL_POINTER_SHAPE_INFO::default();
    for attempt in 0..2 {
        // SAFETY: data has exactly the advertised writable capacity; both
        // output structs remain live and a frame is acquired by the caller.
        let result = unsafe {
            dup.GetFramePointerShape(
                data.len() as u32,
                data.as_mut_ptr().cast(),
                &mut required,
                &mut shape,
            )
        };
        match result {
            Err(e) if e.code() == DXGI_ERROR_MORE_DATA && attempt == 0 => {
                data.resize(required as usize, 0)
            }
            Err(e) => return Err(e),
            Ok(()) => {
                data.truncate(required as usize);
                return Ok(pointer_kind(shape.Type).map(|kind| PointerShape {
                    kind,
                    width: shape.Width,
                    height: shape.Height,
                    pitch: shape.Pitch,
                    hot_x: shape.HotSpot.x.max(0) as u32,
                    hot_y: shape.HotSpot.y.max(0) as u32,
                    data,
                }));
            }
        }
    }
    Ok(None)
}
fn update_pointer(screen_id: u32, dup: &Duplication, info: &DXGI_OUTDUPL_FRAME_INFO) {
    let shape = if info.PointerShapeBufferSize > 0 {
        match pointer_shape(&dup.dup, info.PointerShapeBufferSize) {
            Ok(Some(shape)) => match to_cursor_shape(&shape, dup.scale) {
                Ok(shape) => Some(shape),
                Err(error) => {
                    debug!(screen_id,%error,"cursor shape conversion failed");
                    None
                }
            },
            Ok(None) => None,
            Err(error) => {
                debug!(screen_id,%error,"GetFramePointerShape failed");
                None
            }
        }
    } else {
        None
    };
    if shape.is_none() && info.LastMouseUpdateTime == 0 {
        return;
    }
    let mut bus = lock(cursor_bus());
    if let Some(shape) = shape {
        if let Some(update) = bus.model.on_shape(shape) {
            bus.broadcast(update);
        }
    }
    let report = PointerReport {
        display_id: screen_id,
        update_time: info.LastMouseUpdateTime,
        visible: info.PointerPosition.Visible.as_bool(),
        x: info.PointerPosition.Position.x,
        y: info.PointerPosition.Position.y,
        width: dup.desktop.width,
        height: dup.desktop.height,
    };
    for update in bus.model.on_report(&report) {
        bus.broadcast(update);
    }
}

struct Mapped<'a> {
    context: &'a ID3D11DeviceContext,
    texture: &'a ID3D11Texture2D,
}
impl Drop for Mapped<'_> {
    fn drop(&mut self) {
        // SAFETY: constructed only after Map(READ) succeeds for subresource 0.
        unsafe {
            self.context.Unmap(self.texture, 0);
        }
    }
}
/// Debug-only GPU readback, tightly packed BGRA; desktop alpha is unspecified.
pub fn read_back_bgra(frame: &WinFrame, width: u32, height: u32) -> Result<Vec<u8>, PlatformError> {
    let read = || -> windows::core::Result<Vec<u8>> {
        let mut desc = D3D11_TEXTURE2D_DESC::default();
        // SAFETY: live texture and writable descriptor.
        unsafe {
            frame.texture.GetDesc(&mut desc);
        }
        if width != desc.Width
            || height != desc.Height
            || desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM
            || width == 0
            || height == 0
        {
            return Err(Error::from_hresult(HRESULT(E_INVALIDARG)));
        }
        desc.Usage = D3D11_USAGE_STAGING;
        desc.BindFlags = 0;
        desc.CPUAccessFlags = D3D11_CPU_ACCESS_READ.0 as u32;
        desc.MiscFlags = 0;
        let staging = create_texture(&frame.device, &desc)?;
        // SAFETY: live device; its immediate context is multithread protected.
        let context = unsafe { frame.device.GetImmediateContext()? };
        let mut mapped = D3D11_MAPPED_SUBRESOURCE::default();
        // SAFETY: textures match on the same device; staging is private to this
        // readback. Map synchronizes the preceding copy and writes a live output.
        unsafe {
            context.CopyResource(&staging, &frame.texture);
            context.Map(&staging, 0, D3D11_MAP_READ, 0, Some(&mut mapped))?;
        }
        let _mapping = Mapped {
            context: &context,
            texture: &staging,
        };
        let row = (width as usize).checked_mul(4).ok_or_else(absent)?;
        let length = row.checked_mul(height as usize).ok_or_else(absent)?;
        let pitch = mapped.RowPitch as usize;
        let mapped_length = (height as usize - 1)
            .checked_mul(pitch)
            .and_then(|v| v.checked_add(row))
            .ok_or_else(absent)?;
        if mapped.pData.is_null() || pitch < row || mapped_length > isize::MAX as usize {
            return Err(absent());
        }
        let mut bytes = vec![0; length];
        // SAFETY: the mapped allocation contains height rows of at least row
        // bytes. Overflow/bounds were checked above; _mapping keeps it mapped.
        let source =
            unsafe { std::slice::from_raw_parts(mapped.pData.cast::<u8>(), mapped_length) };
        for y in 0..height as usize {
            bytes[y * row..(y + 1) * row].copy_from_slice(&source[y * pitch..y * pitch + row]);
        }
        Ok(bytes)
    };
    read().map_err(platform_error)
}
