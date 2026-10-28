#![cfg(windows)]
//! ：Windows 视频显示：d3d11va → Video Processor → DXGI 翻转链。

use std::{
    mem::ManuallyDrop,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, Receiver, Sender},
        Arc, Mutex, MutexGuard,
    },
    thread,
    time::{Duration, Instant, SystemTime, UNIX_EPOCH},
};

use tracing::{error, warn};
use windows::{
    core::{Interface, HRESULT},
    Win32::{
        Foundation::{
            CloseHandle, DXGI_STATUS_OCCLUDED, E_INVALIDARG, HANDLE, HWND, RECT, WAIT_OBJECT_0,
        },
        Graphics::{
            Direct3D11::*,
            Dxgi::{
                Common::{
                    DXGI_ALPHA_MODE_IGNORE, DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709, DXGI_FORMAT_B8G8R8A8_UNORM,
                    DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC,
                },
                IDXGIDevice, IDXGIFactory2, IDXGISwapChain2, DXGI_ERROR_DEVICE_REMOVED,
                DXGI_ERROR_DEVICE_RESET, DXGI_MWA_NO_ALT_ENTER, DXGI_PRESENT, DXGI_SCALING_STRETCH,
                DXGI_SWAP_CHAIN_DESC1, DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
                DXGI_SWAP_EFFECT_FLIP_DISCARD, DXGI_USAGE_RENDER_TARGET_OUTPUT,
            },
        },
        System::Threading::{CreateEventW, SetEvent, WaitForMultipleObjects},
    },
};

use crate::{
    platform::{BitstreamFormat, DecodeResult, DesktopFrame, FrameDone, FrameSink, StreamInfo},
    win_ffmpeg::{create_device, DecodeError, DecodedFrame, FfDecLib, FfDecoder},
    win_ffmpeg_abi::{Outcome, OutputMatcher},
    win_present_model::{
        target_rect, PresentPacer, PresentWhat, ResetBudget, ViewCache, OUTPUT_COLOR_SPACE,
        OUTPUT_LEGACY, STREAM_COLOR_SPACE, STREAM_LEGACY, VIEW_CACHE_MAX,
    },
};

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PresentStats {
    pub decoded: u64,
    pub presented: u64,
    pub superseded: u64,
    pub failed: u64,
    pub device_resets: u64,
}

enum Command {
    Configure(StreamInfo),
    Frame(DesktopFrame, FrameDone),
    Resize(u32, u32),
    Close,
    Shutdown,
}

struct Shared {
    sender: Mutex<Sender<Command>>,
    wake: HANDLE,
    video_size: Mutex<Option<(u32, u32)>>,
    stats: Mutex<PresentStats>,
    closed: AtomicBool,
}

// SAFETY: the event is an owned kernel handle and all Rust state is protected
// by synchronization primitives; the wrapper is shared only for wakeups.
unsafe impl Send for Shared {}
// SAFETY: see the Send implementation above.
unsafe impl Sync for Shared {}

/// HWND is represented by its stable integer value; the actual window and all
/// D3D calls are used only by the render thread.
pub struct WinVideoSink {
    shared: Arc<Shared>,
}

// SAFETY: `Shared` only sends the integer HWND through a command-owned render
// thread. The HWND is never dereferenced by callers of `FrameSink`.
unsafe impl Send for WinVideoSink {}
// SAFETY: all mutable state is behind Mutex/atomics and the HWND is used only
// on the render thread.
unsafe impl Sync for WinVideoSink {}

impl WinVideoSink {
    pub fn new(hwnd: HWND, client_w_px: u32, client_h_px: u32) -> Arc<Self> {
        // SAFETY: an unnamed auto-reset event has no borrowed pointer or name.
        let wake = match unsafe { CreateEventW(None, false, false, None) } {
            Ok(handle) => handle,
            Err(error) => {
                error!(error = %error, "failed to create video render wake event");
                HANDLE::default()
            }
        };
        let (sender, receiver) = mpsc::channel();
        let shared = Arc::new(Shared {
            sender: Mutex::new(sender),
            wake,
            video_size: Mutex::new(None),
            stats: Mutex::new(PresentStats::default()),
            closed: AtomicBool::new(false),
        });
        let thread_shared = Arc::clone(&shared);
        let hwnd_value = hwnd.0 as isize;
        let spawn = thread::Builder::new()
            .name("rs-dt-video".into())
            .spawn(move || {
                render_thread(
                    thread_shared,
                    receiver,
                    hwnd_value,
                    client_w_px,
                    client_h_px,
                )
            });
        if spawn.is_err() {
            error!("failed to start Windows video render thread");
        }
        Arc::new(Self { shared })
    }

    pub fn resize(&self, client_w_px: u32, client_h_px: u32) {
        if !self.shared.closed.load(Ordering::Acquire) {
            send_command(&self.shared, Command::Resize(client_w_px, client_h_px));
        }
    }

    pub fn video_size(&self) -> Option<(u32, u32)> {
        lock(&self.shared.video_size).as_ref().copied()
    }

    pub fn shutdown(&self) {
        if !self.shared.closed.swap(true, Ordering::AcqRel) {
            send_command(&self.shared, Command::Shutdown);
        }
    }

    pub fn stats(&self) -> PresentStats {
        *lock(&self.shared.stats)
    }
}

impl FrameSink for WinVideoSink {
    fn format(&self) -> BitstreamFormat {
        BitstreamFormat::AnnexB
    }

    fn configure(&self, info: &StreamInfo) {
        if self.shared.closed.load(Ordering::Acquire) {
            return;
        }
        *lock(&self.shared.video_size) = Some((info.width, info.height));
        send_command(&self.shared, Command::Configure(info.clone()));
    }

    fn submit(&self, frame: DesktopFrame, done: FrameDone) {
        if self.shared.closed.load(Ordering::Acquire) {
            done.finish(DecodeResult::Failed);
            return;
        }
        send_command(&self.shared, Command::Frame(frame, done));
    }

    fn close(&self) {
        if !self.shared.closed.load(Ordering::Acquire) {
            send_command(&self.shared, Command::Close);
        }
    }
}

impl Drop for WinVideoSink {
    fn drop(&mut self) {
        self.shutdown();
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    match mutex.lock() {
        Ok(value) => value,
        Err(poisoned) => poisoned.into_inner(),
    }
}

fn send_command(shared: &Shared, command: Command) {
    let result = lock(&shared.sender).send(command);
    if result.is_ok() && !shared.wake.is_invalid() {
        // SAFETY: `wake` is the live event created in `new`; only the render
        // thread closes it after the receiver is gone.
        unsafe {
            let _ = SetEvent(shared.wake);
        }
    }
}

struct RenderState {
    device: ID3D11Device,
    context: ID3D11DeviceContext,
    video_device: ID3D11VideoDevice,
    video_context: ID3D11VideoContext,
    video_context1: Option<ID3D11VideoContext1>,
    swap_chain: IDXGISwapChain2,
    waitable: HANDLE,
    width: u32,
    height: u32,
    processor: Option<VideoProcessor>,
}

struct VideoProcessor {
    key: (u32, u32, u32, u32),
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input_views: ViewCache<(usize, u32), (ID3D11Texture2D, ID3D11VideoProcessorInputView)>,
    output_views: ViewCache<usize, (ID3D11Texture2D, ID3D11VideoProcessorOutputView)>,
    fallback_warned: bool,
}

struct PendingFrame {
    frame: DecodedFrame,
}

fn render_thread(
    shared: Arc<Shared>,
    receiver: Receiver<Command>,
    hwnd_value: isize,
    mut client_w: u32,
    mut client_h: u32,
) {
    let hwnd = HWND(hwnd_value as *mut _);
    let mut render = match RenderState::new(hwnd, client_w, client_h) {
        Ok(value) => Some(value),
        Err(error) => {
            error!(error = %error, "Windows video renderer initialization failed");
            None
        }
    };
    let ff_lib = FfDecLib::shared().ok();
    let mut decoder: Option<FfDecoder> = None;
    let mut stream: Option<StreamInfo> = None;
    let mut matcher = OutputMatcher::new(32);
    let mut pacer = PresentPacer::new();
    let mut pending: Option<PendingFrame> = None;
    let mut last: Option<DecodedFrame> = None;
    let mut reset_budget = ResetBudget::new();
    let mut reset_disabled = false;
    let mut running = true;

    while running {
        if wait_for_commands(&shared, render.as_ref().map(|state| state.waitable), &pacer) {
            pacer.on_swapchain_ready();
        }
        while let Ok(command) = receiver.try_recv() {
            match command {
                Command::Configure(info) => {
                    stream = Some(info.clone());
                    reset_disabled = false;
                    if decoder.as_ref().is_none_or(|old| old.codec() != info.codec) {
                        finish_all(&mut matcher, &shared);
                        decoder = ff_lib.as_ref().and_then(|lib| {
                            render.as_ref().and_then(|state| {
                                FfDecoder::open(lib, info.codec, &state.device)
                                    .map_err(|error| {
                                        warn!(error = %error, "opening FFmpeg decoder failed");
                                    })
                                    .ok()
                            })
                        });
                        if decoder.is_none() {
                            warn!("Windows video decoder unavailable; frames will fail");
                        }
                    }
                }
                Command::Frame(frame, done) => {
                    let device_lost = process_frame(
                        &shared,
                        decoder.as_mut(),
                        &mut matcher,
                        &mut pacer,
                        &mut pending,
                        frame,
                        done,
                    );
                    if device_lost {
                        finish_all(&mut matcher, &shared);
                        pending = None;
                        last = None;
                        render = None;
                        decoder = None;
                        if reset_disabled || !reset_budget.try_take(now_us()) {
                            reset_disabled = true;
                            error!("D3D11 device reset budget exhausted");
                        } else {
                            lock(&shared.stats).device_resets += 1;
                            render = RenderState::new(hwnd, client_w, client_h).ok();
                            decoder = stream.as_ref().and_then(|info| {
                                ff_lib.as_ref().and_then(|lib| {
                                    render.as_ref().and_then(|state| {
                                        FfDecoder::open(lib, info.codec, &state.device).ok()
                                    })
                                })
                            });
                        }
                    }
                }
                Command::Resize(width, height) => {
                    client_w = width;
                    client_h = height;
                    if width != 0 && height != 0 {
                        if let Some(state) = render.as_mut() {
                            if let Err(error) = state.resize(width, height) {
                                warn!(error = %error, "DXGI swapchain resize failed");
                            } else {
                                pacer.request_redraw();
                            }
                        }
                    }
                }
                Command::Close => {
                    decoder = None;
                    finish_all(&mut matcher, &shared);
                    pending = None;
                    pacer = PresentPacer::new();
                    pacer.request_redraw();
                }
                Command::Shutdown => {
                    finish_all(&mut matcher, &shared);
                    running = false;
                    break;
                }
            }
        }

        if let Some(state) = render.as_mut() {
            while let Some(what) = pacer.next() {
                let result = match what {
                    PresentWhat::Pending(_) => pending.as_ref().map(|item| &item.frame),
                    PresentWhat::Redraw => last.as_ref(),
                };
                let result = match result {
                    Some(frame) => state.present(frame),
                    None => state.present_black(),
                };
                match result {
                    Ok(()) => {
                        if matches!(what, PresentWhat::Pending(_)) {
                            if let Some(item) = pending.take() {
                                last = Some(item.frame);
                            }
                        }
                        lock(&shared.stats).presented += 1;
                    }
                    Err(RenderError::DeviceLost(code)) => {
                        error!(reason = code, "D3D11 device lost while presenting");
                        finish_all(&mut matcher, &shared);
                        pending = None;
                        last = None;
                        render = None;
                        if reset_disabled || !reset_budget.try_take(now_us()) {
                            reset_disabled = true;
                            error!("D3D11 device reset budget exhausted");
                            decoder = None;
                        } else {
                            lock(&shared.stats).device_resets += 1;
                            render = RenderState::new(hwnd, client_w, client_h).ok();
                            decoder = stream.as_ref().and_then(|info| {
                                ff_lib.as_ref().and_then(|lib| {
                                    render.as_ref().and_then(|state| {
                                        FfDecoder::open(lib, info.codec, &state.device).ok()
                                    })
                                })
                            });
                        }
                        break;
                    }
                    Err(RenderError::Failed(error)) => {
                        warn!(error = %error, "video presentation failed");
                        break;
                    }
                }
            }
        }
    }

    finish_all(&mut matcher, &shared);
    drop(decoder);
    drop(render);
    if !shared.wake.is_invalid() {
        // SAFETY: render thread owns the event's final close operation.
        unsafe {
            let _ = CloseHandle(shared.wake);
        }
    }
}

fn wait_for_commands(shared: &Shared, waitable: Option<HANDLE>, pacer: &PresentPacer) -> bool {
    if shared.wake.is_invalid() {
        thread::sleep(Duration::from_millis(1));
        return false;
    }
    let mut handles = [shared.wake, HANDLE::default()];
    let count = if pacer.wants_waitable() {
        if let Some(waitable) = waitable {
            handles[1] = waitable;
            2
        } else {
            1
        }
    } else {
        1
    };
    // SAFETY: all handles are live kernel handles and the array length matches
    // the count passed to the wait API.
    let result = unsafe { WaitForMultipleObjects(&handles[..count], false, 250) };
    count == 2 && result.0 == WAIT_OBJECT_0.0 + 1
}

#[allow(clippy::too_many_arguments)]
fn process_frame(
    shared: &Shared,
    decoder: Option<&mut FfDecoder>,
    matcher: &mut OutputMatcher<(FrameDone, Instant)>,
    pacer: &mut PresentPacer,
    pending: &mut Option<PendingFrame>,
    frame: DesktopFrame,
    done: FrameDone,
) -> bool {
    let Some(decoder) = decoder else {
        done.finish(DecodeResult::Failed);
        lock(&shared.stats).failed += 1;
        return false;
    };
    let submitted = Instant::now();
    for ((done, submitted_at), outcome) in matcher.push(frame.frame_id, (done, submitted)) {
        settle(shared, done, outcome, submitted_at);
    }
    let outputs = match decoder.decode(frame.frame_id, &frame.data, frame.keyframe) {
        Ok(outputs) => outputs,
        Err(error) => {
            for ((done, submitted_at), outcome) in matcher.fail(frame.frame_id) {
                settle(shared, done, outcome, submitted_at);
            }
            return matches!(error, DecodeError::DeviceRemoved(_));
        }
    };
    for output in outputs {
        for ((done, submitted_at), outcome) in matcher.output(output.frame_id) {
            settle(shared, done, outcome, submitted_at);
        }
        if pacer.on_decoded(output.frame_id).is_some() && pending.take().is_some() {
            lock(&shared.stats).superseded += 1;
        }
        *pending = Some(PendingFrame { frame: output });
        lock(&shared.stats).decoded += 1;
    }
    false
}

fn settle(shared: &Shared, done: FrameDone, outcome: Outcome, submitted: Instant) {
    match outcome {
        Outcome::Shown => done.finish(DecodeResult::Shown {
            decode_us: submitted.elapsed().as_micros().min(u64::MAX as u128) as u64,
        }),
        Outcome::Failed => {
            done.finish(DecodeResult::Failed);
            lock(&shared.stats).failed += 1;
        }
    }
}

fn finish_all(matcher: &mut OutputMatcher<(FrameDone, Instant)>, shared: &Shared) {
    for ((done, _), _) in matcher.fail_all() {
        done.finish(DecodeResult::Failed);
        lock(&shared.stats).failed += 1;
    }
}

fn now_us() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_micros().min(u64::MAX as u128) as u64,
        Err(_) => 0,
    }
}

#[derive(Debug)]
enum RenderError {
    DeviceLost(i32),
    Failed(String),
}

impl std::fmt::Display for RenderError {
    fn fmt(&self, formatter: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::DeviceLost(code) => write!(formatter, "device lost {code:#x}"),
            Self::Failed(error) => formatter.write_str(error),
        }
    }
}

impl RenderState {
    fn new(hwnd: HWND, width: u32, height: u32) -> Result<Self, String> {
        let device = create_device().map_err(|error| error.to_string())?;
        // SAFETY: the device owns its immediate context for this render thread.
        let context = unsafe { device.GetImmediateContext() }.map_err(|error| error.to_string())?;
        let video_device: ID3D11VideoDevice = device.cast().map_err(|error| error.to_string())?;
        let video_context: ID3D11VideoContext =
            context.cast().map_err(|error| error.to_string())?;
        let video_context1 = context.cast::<ID3D11VideoContext1>().ok();
        let dxgi_device: IDXGIDevice = device.cast().map_err(|error| error.to_string())?;
        // SAFETY: `dxgi_device` is a live interface returned by the device.
        let adapter = unsafe { dxgi_device.GetAdapter() }.map_err(|error| error.to_string())?;
        let factory: IDXGIFactory2 =
            // SAFETY: the adapter is a live DXGI interface.
            unsafe { adapter.GetParent() }.map_err(|error| error.to_string())?;
        let desc = DXGI_SWAP_CHAIN_DESC1 {
            Width: width.max(1),
            Height: height.max(1),
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            Stereo: false.into(),
            SampleDesc: DXGI_SAMPLE_DESC {
                Count: 1,
                Quality: 0,
            },
            BufferUsage: DXGI_USAGE_RENDER_TARGET_OUTPUT,
            BufferCount: 2,
            Scaling: DXGI_SCALING_STRETCH,
            SwapEffect: DXGI_SWAP_EFFECT_FLIP_DISCARD,
            AlphaMode: DXGI_ALPHA_MODE_IGNORE,
            Flags: DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT.0 as u32,
        };
        // SAFETY: the descriptor and HWND remain valid for this synchronous call.
        let swap_chain1 =
            unsafe { factory.CreateSwapChainForHwnd(&device, hwnd, &desc, None, None) }
                .map_err(|error| error.to_string())?;
        // SAFETY: the factory and HWND are live and owned by this render thread.
        unsafe {
            factory
                .MakeWindowAssociation(hwnd, DXGI_MWA_NO_ALT_ENTER)
                .map_err(|error| error.to_string())?;
        }
        let swap_chain: IDXGISwapChain2 = swap_chain1.cast().map_err(|error| error.to_string())?;
        // SAFETY: the swapchain is a live waitable flip chain.
        unsafe { swap_chain.SetMaximumFrameLatency(1) }.map_err(|error| error.to_string())?;
        // SAFETY: the swapchain returns its owned frame-latency event handle.
        let waitable = unsafe { swap_chain.GetFrameLatencyWaitableObject() };
        if waitable.is_invalid() {
            return Err("DXGI did not return a frame-latency waitable object".into());
        }
        Ok(Self {
            device,
            context,
            video_device,
            video_context,
            video_context1,
            swap_chain,
            waitable,
            width,
            height,
            processor: None,
        })
    }

    fn resize(&mut self, width: u32, height: u32) -> Result<(), String> {
        self.processor = None;
        // SAFETY: all outstanding views have been dropped before ResizeBuffers.
        unsafe {
            self.swap_chain
                .ResizeBuffers(
                    0,
                    width,
                    height,
                    windows::Win32::Graphics::Dxgi::Common::DXGI_FORMAT_UNKNOWN,
                    DXGI_SWAP_CHAIN_FLAG_FRAME_LATENCY_WAITABLE_OBJECT,
                )
                .map_err(|error| error.to_string())?;
        }
        self.width = width;
        self.height = height;
        Ok(())
    }

    fn ensure_processor(
        &mut self,
        frame_width: u32,
        frame_height: u32,
    ) -> Result<&mut VideoProcessor, String> {
        let key = (
            frame_width,
            frame_height,
            self.width.max(1),
            self.height.max(1),
        );
        if self
            .processor
            .as_ref()
            .is_none_or(|processor| processor.key != key)
        {
            self.processor = Some(VideoProcessor::new(
                &self.video_device,
                &self.video_context,
                self.video_context1.as_ref(),
                key,
            )?);
        }
        self.processor
            .as_mut()
            .ok_or_else(|| "video processor unavailable".into())
    }

    fn present(&mut self, frame: &DecodedFrame) -> Result<(), RenderError> {
        // SAFETY: buffer 0 is an owned back-buffer interface of the live chain.
        let output_texture: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0) }
            .map_err(|error| RenderError::Failed(error.to_string()))?;
        let device = self.device.clone();
        let video_device = self.video_device.clone();
        let context = self.context.clone();
        let video_context = self.video_context.clone();
        let processor = self
            .ensure_processor(frame.width, frame.height)
            .map_err(RenderError::Failed)?;
        let output_view = processor
            .output_view(&video_device, &output_texture)
            .map_err(RenderError::Failed)?;
        let (input_view, fallback) = processor
            .input_view(
                &device,
                &video_device,
                &context,
                &frame.texture,
                frame.array_index,
            )
            .map_err(RenderError::Failed)?;
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ..Default::default()
        };
        // SAFETY: processor, views, and stream are live for this synchronous blit.
        let blt = unsafe {
            video_context.VideoProcessorBlt(
                &processor.processor,
                &output_view,
                0,
                std::slice::from_ref(&stream),
            )
        };
        // SAFETY: VideoProcessorBlt synchronously consumed the ManuallyDrop view.
        unsafe {
            ManuallyDrop::drop(&mut stream.pInputSurface);
        }
        drop(fallback);
        if let Err(error) = blt {
            let code = error.code();
            if code == DXGI_ERROR_DEVICE_REMOVED || code == DXGI_ERROR_DEVICE_RESET {
                return Err(RenderError::DeviceLost(self.device_lost_code(code)));
            }
            return Err(RenderError::Failed(format!("VideoProcessorBlt: {error}")));
        }
        // SAFETY: all GPU commands were submitted on this render thread.
        let present = unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) };
        if present == DXGI_ERROR_DEVICE_REMOVED || present == DXGI_ERROR_DEVICE_RESET {
            return Err(RenderError::DeviceLost(self.device_lost_code(present)));
        }
        if present != HRESULT(0) && present != DXGI_STATUS_OCCLUDED {
            return Err(RenderError::Failed(format!("Present: {present:?}")));
        }
        Ok(())
    }

    fn present_black(&mut self) -> Result<(), RenderError> {
        // SAFETY: buffer 0 is an owned back-buffer interface of the live chain.
        let output_texture: ID3D11Texture2D = unsafe { self.swap_chain.GetBuffer(0) }
            .map_err(|error| RenderError::Failed(error.to_string()))?;
        let mut view = None;
        let desc = D3D11_RENDER_TARGET_VIEW_DESC {
            Format: DXGI_FORMAT_B8G8R8A8_UNORM,
            ViewDimension: D3D11_RTV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_RENDER_TARGET_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_RTV { MipSlice: 0 },
            },
        };
        // SAFETY: the back buffer and descriptor are live for this call.
        unsafe {
            self.device
                .CreateRenderTargetView(&output_texture, Some(&desc), Some(&mut view))
                .map_err(|error| RenderError::Failed(error.to_string()))?;
            if let Some(view) = view {
                self.context
                    .ClearRenderTargetView(&view, &[0.0, 0.0, 0.0, 1.0]);
            }
        }
        // SAFETY: the clear and present execute on the render thread.
        let present = unsafe { self.swap_chain.Present(0, DXGI_PRESENT(0)) };
        if present == DXGI_ERROR_DEVICE_REMOVED || present == DXGI_ERROR_DEVICE_RESET {
            return Err(RenderError::DeviceLost(self.device_lost_code(present)));
        }
        if present != HRESULT(0) && present != DXGI_STATUS_OCCLUDED {
            return Err(RenderError::Failed(format!("Present black: {present:?}")));
        }
        Ok(())
    }

    fn device_lost_code(&self, fallback: HRESULT) -> i32 {
        // SAFETY: the D3D11 device remains live while this render state is
        // handling the loss notification.
        match unsafe { self.device.GetDeviceRemovedReason() } {
            Ok(()) => fallback.0,
            Err(error) => error.code().0,
        }
    }
}

impl VideoProcessor {
    fn new(
        device: &ID3D11VideoDevice,
        context: &ID3D11VideoContext,
        context1: Option<&ID3D11VideoContext1>,
        key: (u32, u32, u32, u32),
    ) -> Result<Self, String> {
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL {
                Numerator: 30,
                Denominator: 1,
            },
            InputWidth: key.0,
            InputHeight: key.1,
            OutputFrameRate: windows::Win32::Graphics::Dxgi::Common::DXGI_RATIONAL {
                Numerator: 30,
                Denominator: 1,
            },
            OutputWidth: key.2,
            OutputHeight: key.3,
            Usage: D3D11_VIDEO_USAGE_PLAYBACK_NORMAL,
        };
        // SAFETY: the content descriptor is fully initialized for this call.
        let enumerator = unsafe { device.CreateVideoProcessorEnumerator(&content) }
            .map_err(|error| error.to_string())?;
        // SAFETY: enumerator is live and processor index zero is valid after creation.
        let processor = unsafe { device.CreateVideoProcessor(&enumerator, 0) }
            .map_err(|error| error.to_string())?;
        let source = RECT {
            left: 0,
            top: 0,
            right: key.0.min(i32::MAX as u32) as i32,
            bottom: key.1.min(i32::MAX as u32) as i32,
        };
        let target = target_rect(key.2, key.3, key.0, key.1);
        let dest = RECT {
            left: target.left.min(i32::MAX as u32) as i32,
            top: target.top.min(i32::MAX as u32) as i32,
            right: target.right.min(i32::MAX as u32) as i32,
            bottom: target.bottom.min(i32::MAX as u32) as i32,
        };
        let output = RECT {
            left: 0,
            top: 0,
            right: key.2.min(i32::MAX as u32) as i32,
            bottom: key.3.min(i32::MAX as u32) as i32,
        };
        let color = D3D11_VIDEO_COLOR {
            Anonymous: D3D11_VIDEO_COLOR_0 {
                RGBA: D3D11_VIDEO_COLOR_RGBA {
                    R: 0.0,
                    G: 0.0,
                    B: 0.0,
                    A: 1.0,
                },
            },
        };
        // SAFETY: all setter pointers refer to stack values valid for each call.
        unsafe {
            context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&source));
            context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&dest));
            context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&output));
            context.VideoProcessorSetOutputBackgroundColor(&processor, false, &color);
            if let Some(context1) = context1 {
                context1.VideoProcessorSetStreamColorSpace1(
                    &processor,
                    0,
                    DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                );
                context1.VideoProcessorSetOutputColorSpace1(
                    &processor,
                    DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                );
            } else {
                let input = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                    _bitfield: STREAM_LEGACY,
                };
                let output = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                    _bitfield: OUTPUT_LEGACY,
                };
                context.VideoProcessorSetStreamColorSpace(&processor, 0, &input);
                context.VideoProcessorSetOutputColorSpace(&processor, &output);
            }
        }
        let _ = (STREAM_COLOR_SPACE, OUTPUT_COLOR_SPACE);
        Ok(Self {
            key,
            enumerator,
            processor,
            input_views: ViewCache::new(VIEW_CACHE_MAX),
            output_views: ViewCache::new(VIEW_CACHE_MAX),
            fallback_warned: false,
        })
    }

    fn input_view(
        &mut self,
        d3d_device: &ID3D11Device,
        device: &ID3D11VideoDevice,
        context: &ID3D11DeviceContext,
        source: &ID3D11Texture2D,
        array_index: u32,
    ) -> Result<(ID3D11VideoProcessorInputView, Option<ID3D11Texture2D>), String> {
        let key = (source.as_raw() as usize, array_index);
        let desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
            FourCC: 0,
            ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPIV {
                    MipSlice: 0,
                    ArraySlice: array_index,
                },
            },
        };
        if let Some((_, cached)) = self.input_views.get(&key) {
            return Ok((cached.clone(), None));
        }
        // SAFETY: source and enumerator are live D3D11 interfaces.
        let direct = unsafe {
            let mut view = None;
            device
                .CreateVideoProcessorInputView(source, &self.enumerator, &desc, Some(&mut view))
                .map(|_| view)
        };
        match direct {
            Ok(Some(view)) => self
                .input_views
                .get_or_insert_with(key, || Ok::<_, String>((source.clone(), view.clone())))
                .map(|(_, cached)| (cached.clone(), None)),
            Ok(None) => Err("missing Video Processor input view".into()),
            Err(error) if error.code() == E_INVALIDARG => {
                if !self.fallback_warned {
                    warn!("decoder texture input view rejected; using NV12 copy fallback");
                    self.fallback_warned = true;
                }
                let desc_tex = D3D11_TEXTURE2D_DESC {
                    Width: self.key.0,
                    Height: self.key.1,
                    MipLevels: 1,
                    ArraySize: 1,
                    Format: DXGI_FORMAT_NV12,
                    SampleDesc: DXGI_SAMPLE_DESC {
                        Count: 1,
                        Quality: 0,
                    },
                    Usage: D3D11_USAGE_DEFAULT,
                    BindFlags: D3D11_BIND_SHADER_RESOURCE.0 as u32,
                    CPUAccessFlags: 0,
                    MiscFlags: 0,
                };
                let mut texture = None;
                // SAFETY: the device owns the newly allocated texture descriptor.
                unsafe {
                    d3d_device
                        .CreateTexture2D(&desc_tex, None, Some(&mut texture))
                        .map_err(|error| error.to_string())?;
                }
                let texture = texture.ok_or_else(|| "copy texture not returned".to_owned())?;
                // SAFETY: both source and destination textures are live on this context.
                unsafe {
                    context.CopySubresourceRegion(&texture, 0, 0, 0, 0, source, array_index, None);
                }
                let mut view = None;
                let copy_desc = D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
                    FourCC: 0,
                    ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
                    Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
                        Texture2D: D3D11_TEX2D_VPIV {
                            MipSlice: 0,
                            ArraySlice: 0,
                        },
                    },
                };
                // SAFETY: the fallback texture and enumerator are live interfaces.
                unsafe {
                    device
                        .CreateVideoProcessorInputView(
                            &texture,
                            &self.enumerator,
                            &copy_desc,
                            Some(&mut view),
                        )
                        .map_err(|error| error.to_string())?;
                }
                Ok((
                    view.ok_or_else(|| "missing fallback input view".to_owned())?,
                    Some(texture),
                ))
            }
            Err(error) => Err(error.to_string()),
        }
    }

    fn output_view(
        &mut self,
        device: &ID3D11VideoDevice,
        texture: &ID3D11Texture2D,
    ) -> Result<ID3D11VideoProcessorOutputView, String> {
        let key = texture.as_raw() as usize;
        let desc = D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        };
        self.output_views
            .get_or_insert_with(key, || {
                let mut view = None;
                // SAFETY: texture and enumerator are live for this view creation.
                unsafe {
                    device
                        .CreateVideoProcessorOutputView(
                            texture,
                            &self.enumerator,
                            &desc,
                            Some(&mut view),
                        )
                        .map_err(|error| error.to_string())?;
                }
                Ok::<_, String>((
                    texture.clone(),
                    view.ok_or_else(|| "missing output view".to_owned())?,
                ))
            })
            .map(|(_, view)| view.clone())
    }
}
