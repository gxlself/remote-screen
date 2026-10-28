//! WASAPI render-loopback capture for the interactive session process.
//!
//! All COM interfaces stay on one MTA thread. Format parsing, sample conversion
//! and packet timestamp conversion are implemented in `audio_convert`.

use std::ptr;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use host_core::platform::{
    AudioCaptureConfig, AudioChunk, AudioChunkSink, CaptureHandle, PlatformError,
};
use rsp_core::clock::unix_micros;
use tokio::sync::mpsc::error::TrySendError;
use windows::core::GUID;
use windows::Win32::Foundation::E_ACCESSDENIED;
use windows::Win32::Media::Audio::{
    eConsole, eRender, IAudioCaptureClient, IAudioClient, IMMDevice, IMMDeviceEnumerator,
    MMDeviceEnumerator, AUDCLNT_BUFFERFLAGS_SILENT, AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR,
    AUDCLNT_E_DEVICE_INVALIDATED, AUDCLNT_E_UNSUPPORTED_FORMAT, AUDCLNT_SHAREMODE_SHARED,
    AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM, AUDCLNT_STREAMFLAGS_LOOPBACK,
    AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY, WAVEFORMATEX, WAVEFORMATEXTENSIBLE,
    WAVEFORMATEXTENSIBLE_0,
};
use windows::Win32::System::Com::{
    CoCreateInstance, CoInitializeEx, CoTaskMemFree, CoUninitialize, CLSCTX_ALL,
    COINIT_MULTITHREADED,
};
use windows::Win32::System::Performance::{QueryPerformanceCounter, QueryPerformanceFrequency};

use crate::audio_convert::{
    parse_wave_format, plan_open, qpc_to_unix_us, to_stereo_f32, FormatError, OpenPlan,
    SampleFormat, WaveFormat, SUBTYPE_IEEE_FLOAT, WAVE_FORMAT_EXTENSIBLE,
};

const SAMPLE_RATE: u32 = 48_000;
const CHANNELS: u8 = 2;
const BUFFER_DURATION_100NS: i64 = 2_000_000;
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const DEVICE_CHECK_INTERVAL: Duration = Duration::from_millis(250);
const REOPEN_DELAY: Duration = Duration::from_millis(500);

type StartResult = Result<(), PlatformError>;

/// Capture the default playback endpoint as 48 kHz interleaved stereo f32.
///
/// The session process does not play audio, so `exclude_self` is naturally
/// satisfied. This endpoint-loopback API would include self audio if a caller
/// later adds playback to that process.
pub fn start_loopback(
    cfg: AudioCaptureConfig,
    sink: AudioChunkSink,
) -> Result<Box<dyn CaptureHandle>, PlatformError> {
    if cfg.sample_rate != SAMPLE_RATE || cfg.channels != CHANNELS {
        return Err(PlatformError::Unsupported);
    }
    let stop = Arc::new(AtomicBool::new(false));
    let thread_stop = Arc::clone(&stop);
    let (ready_tx, ready_rx) = mpsc::channel();
    let worker = thread::Builder::new()
        .name("rs-wasapi-loopback".into())
        .spawn(move || {
            let mut ready = Some(ready_tx);
            let mut dropped = 0_u64;
            if let Err(error) = capture_thread(&sink, &thread_stop, &mut dropped, &mut ready) {
                if let Some(ready) = ready.take() {
                    let _ = ready.send(Err(error.into_platform()));
                } else {
                    tracing::warn!(%error, "WASAPI loopback capture stopped");
                }
            }
            tracing::debug!(dropped, "WASAPI loopback worker stopped");
        })?;
    let error = match ready_rx.recv_timeout(Duration::from_secs(2)) {
        Ok(Ok(())) => {
            return Ok(Box::new(LoopbackHandle {
                stop,
                worker: Some(worker),
            }));
        }
        Ok(Err(error)) => error,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            PlatformError::Platform("WASAPI capture startup timed out".into())
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => {
            PlatformError::Platform("WASAPI worker stopped during startup".into())
        }
    };
    stop.store(true, Ordering::Release);
    // A synchronous driver/COM call cannot be cancelled. Do not extend the
    // two-second startup deadline by joining here; the cancelled worker owns
    // its resources and exits as soon as the native call returns.
    drop(worker);
    Err(error)
}

struct LoopbackHandle {
    stop: Arc<AtomicBool>,
    worker: Option<JoinHandle<()>>,
}

impl CaptureHandle for LoopbackHandle {}

impl Drop for LoopbackHandle {
    fn drop(&mut self) {
        self.stop.store(true, Ordering::Release);
        if let Some(worker) = self.worker.take() {
            if worker.join().is_err() {
                tracing::warn!("WASAPI loopback worker panicked");
            }
        }
    }
}

#[derive(Debug, thiserror::Error)]
enum CaptureError {
    #[error(transparent)]
    Windows(#[from] windows::core::Error),
    #[error(transparent)]
    Format(#[from] FormatError),
    #[error("default playback endpoint changed or disappeared")]
    EndpointChanged,
}

impl CaptureError {
    fn needs_reopen(&self) -> bool {
        matches!(self, Self::EndpointChanged)
            || matches!(self, Self::Windows(error) if error.code() == AUDCLNT_E_DEVICE_INVALIDATED)
    }

    fn into_platform(self) -> PlatformError {
        match self {
            Self::Format(FormatError::Unsupported) => PlatformError::Unsupported,
            Self::Windows(error) if error.code() == AUDCLNT_E_UNSUPPORTED_FORMAT => {
                PlatformError::Unsupported
            }
            Self::Windows(error) if error.code() == E_ACCESSDENIED => {
                PlatformError::PermissionDenied(error.to_string())
            }
            error => PlatformError::Platform(error.to_string()),
        }
    }
}

struct ComGuard;

impl ComGuard {
    fn new() -> Result<Self, windows::core::Error> {
        // SAFETY: this new, dedicated thread has no prior COM apartment. The
        // guard balances every successful result, including S_FALSE.
        unsafe { CoInitializeEx(None, COINIT_MULTITHREADED).ok()? };
        Ok(Self)
    }
}

impl Drop for ComGuard {
    fn drop(&mut self) {
        // SAFETY: the guard stays on its creating thread and all audio
        // interfaces are released before the guard leaves capture_thread.
        unsafe { CoUninitialize() };
    }
}

fn capture_thread(
    sink: &AudioChunkSink,
    stop: &AtomicBool,
    dropped: &mut u64,
    ready: &mut Option<mpsc::Sender<StartResult>>,
) -> Result<(), CaptureError> {
    let _com = ComGuard::new()?;
    let clock = QpcClock::new()?;
    let mut opened = loop {
        match OpenCapture::start() {
            Ok(opened) => break opened,
            Err(error) if error.needs_reopen() => {
                if wait_or_stop(stop, sink, REOPEN_DELAY) {
                    if let Some(ready) = ready.take() {
                        let _ = ready.send(Ok(()));
                    }
                    return Ok(());
                }
            }
            Err(error) => return Err(error),
        }
    };
    if cancelled(stop, sink) {
        if let Some(ready) = ready.take() {
            let _ = ready.send(Ok(()));
        }
        return Ok(());
    }
    if let Some(ready) = ready.take() {
        if ready.send(Ok(())).is_err() {
            return Ok(());
        }
    }
    loop {
        let result = opened.capture_packets(sink, stop, dropped, &clock);
        // Stop and release the old audio client before waiting or reopening.
        drop(opened);
        match result {
            Ok(()) => return Ok(()),
            Err(error) if error.needs_reopen() => {
                tracing::debug!(%error, "rebuilding WASAPI loopback capture");
            }
            Err(error) => return Err(error),
        }
        loop {
            if wait_or_stop(stop, sink, REOPEN_DELAY) {
                return Ok(());
            }
            match OpenCapture::start() {
                Ok(capture) => {
                    opened = capture;
                    break;
                }
                Err(error) => {
                    // During unplug/replug there may briefly be no default
                    // endpoint. Keep retrying until capture is cancelled.
                    tracing::debug!(%error, "WASAPI loopback rebuild will retry");
                }
            }
        }
    }
}

struct OpenCapture {
    capture: IAudioCaptureClient,
    client: IAudioClient,
    enumerator: IMMDeviceEnumerator,
    endpoint_id: Vec<u16>,
    format: WaveFormat,
}

impl OpenCapture {
    fn start() -> Result<Self, CaptureError> {
        // SAFETY: capture_thread has initialized COM on this thread; no
        // aggregation is requested and the bindings own the returned reference.
        let enumerator: IMMDeviceEnumerator =
            unsafe { CoCreateInstance(&MMDeviceEnumerator, None, CLSCTX_ALL)? };
        // SAFETY: enumerator belongs to this initialized MTA thread.
        let device = unsafe { enumerator.GetDefaultAudioEndpoint(eRender, eConsole)? };
        let endpoint_id = endpoint_id(&device)?;
        // SAFETY: device is a valid endpoint on the same MTA; activation has no
        // extra parameters and the binding requests IAudioClient's IID.
        let mut client: IAudioClient = unsafe { device.Activate(CLSCTX_ALL, None)? };
        let requested = WAVEFORMATEXTENSIBLE {
            Format: WAVEFORMATEX {
                wFormatTag: WAVE_FORMAT_EXTENSIBLE,
                nChannels: u16::from(CHANNELS),
                nSamplesPerSec: SAMPLE_RATE,
                nAvgBytesPerSec: SAMPLE_RATE * u32::from(CHANNELS) * 4,
                nBlockAlign: u16::from(CHANNELS) * 4,
                wBitsPerSample: 32,
                cbSize: 22,
            },
            Samples: WAVEFORMATEXTENSIBLE_0 {
                wValidBitsPerSample: 32,
            },
            dwChannelMask: 3, // SPEAKER_FRONT_LEFT | SPEAKER_FRONT_RIGHT
            SubFormat: GUID::from_u128(SUBTYPE_IEEE_FLOAT),
        };
        // SAFETY: requested contains the complete extensible format and stays
        // alive for the synchronous call. Periodicity must be zero in shared mode.
        let automatic = unsafe {
            client.Initialize(
                AUDCLNT_SHAREMODE_SHARED,
                AUDCLNT_STREAMFLAGS_LOOPBACK
                    | AUDCLNT_STREAMFLAGS_AUTOCONVERTPCM
                    | AUDCLNT_STREAMFLAGS_SRC_DEFAULT_QUALITY,
                BUFFER_DURATION_100NS,
                0,
                ptr::addr_of!(requested.Format),
                None,
            )
        };
        let format = match automatic {
            Ok(()) => WaveFormat {
                sample_rate: SAMPLE_RATE,
                channels: u16::from(CHANNELS),
                block_align: u16::from(CHANNELS) * 4,
                format: SampleFormat::F32,
            },
            Err(error) if error.code() == AUDCLNT_E_DEVICE_INVALIDATED => {
                return Err(error.into());
            }
            Err(_) => {
                // SAFETY: client is valid on this thread. GetMixFormat lends
                // ownership of a COM-task-allocated format, including cbSize bytes.
                let mix = TaskMem(unsafe { client.GetMixFormat()? });
                let parsed = parse_native_format(&mix)?;
                let OpenPlan::UseMix(format) = plan_open(SAMPLE_RATE, false, Some(parsed)) else {
                    // No application-side resampling: a different-rate mix is
                    // unsupported when WASAPI auto-conversion is unavailable.
                    return Err(FormatError::Unsupported.into());
                };
                // SAFETY: use a fresh IAudioClient after a failed Initialize;
                // the same endpoint remains owned on this MTA thread.
                client = unsafe { device.Activate(CLSCTX_ALL, None)? };
                // SAFETY: mix owns the complete format allocation throughout
                // Initialize. The engine copies it before returning.
                unsafe {
                    client.Initialize(
                        AUDCLNT_SHAREMODE_SHARED,
                        AUDCLNT_STREAMFLAGS_LOOPBACK,
                        BUFFER_DURATION_100NS,
                        0,
                        mix.0,
                        None,
                    )?;
                }
                format
            }
        };
        // SAFETY: the initialized loopback client exposes IAudioCaptureClient;
        // both interfaces remain on this thread.
        let capture = unsafe { client.GetService::<IAudioCaptureClient>()? };
        let opened = Self {
            capture,
            client,
            enumerator,
            endpoint_id,
            format,
        };
        // SAFETY: Initialize succeeded; Drop stops the client before releasing it.
        unsafe { opened.client.Start()? };
        Ok(opened)
    }

    fn default_endpoint_changed(&self) -> bool {
        // SAFETY: enumerator and returned endpoint are used on their owning MTA.
        let current = unsafe { self.enumerator.GetDefaultAudioEndpoint(eRender, eConsole) };
        match current.and_then(|device| endpoint_id(&device)) {
            Ok(id) => id != self.endpoint_id,
            Err(_) => true,
        }
    }

    fn capture_packets(
        &self,
        sink: &AudioChunkSink,
        stop: &AtomicBool,
        dropped: &mut u64,
        clock: &QpcClock,
    ) -> Result<(), CaptureError> {
        let mut last_device_check = Instant::now();
        loop {
            if cancelled(stop, sink) {
                return Ok(());
            }
            // An endpoint can remain valid after a user selects another
            // default output. Poll its ID as well as handling DEVICE_INVALIDATED.
            if last_device_check.elapsed() >= DEVICE_CHECK_INTERVAL {
                if self.default_endpoint_changed() {
                    return Err(CaptureError::EndpointChanged);
                }
                last_device_check = Instant::now();
            }
            // SAFETY: Start succeeded and all capture calls run on this thread.
            let frames = unsafe { self.capture.GetNextPacketSize()? };
            if frames == 0 {
                // Inactive render endpoints produce no packets; host-core's
                // framer handles the timestamp gap without synthetic silence.
                thread::sleep(POLL_INTERVAL);
                continue;
            }
            let mut data = ptr::null_mut();
            let mut frames = 0;
            let mut flags = 0;
            let mut qpc = 0;
            // SAFETY: all output pointers refer to valid locals. WASAPI lends
            // the returned data until the matching ReleaseBuffer below.
            unsafe {
                self.capture
                    .GetBuffer(&mut data, &mut frames, &mut flags, None, Some(&mut qpc))?;
            }
            if frames == 0 {
                // GetBuffer can return AUDCLNT_S_BUFFER_EMPTY without a buffer
                // to release. Check cancellation again before the next poll.
                thread::sleep(POLL_INTERVAL);
                continue;
            }
            let samples = copy_packet(data, frames, flags, &self.format);
            // SAFETY: a nonempty buffer was acquired on this thread. Release
            // exactly its frame count, including on conversion failure.
            unsafe { self.capture.ReleaseBuffer(frames)? };
            let samples = match samples {
                Ok(samples) => samples,
                Err(_) => {
                    *dropped = dropped.saturating_add(1);
                    continue;
                }
            };
            let now_qpc = clock.now_100ns()?;
            let now_unix_us = unix_micros();
            let capture_ts_us = if flags & AUDCLNT_BUFFERFLAGS_TIMESTAMP_ERROR.0 as u32 != 0 {
                now_unix_us
            } else {
                qpc_to_unix_us(qpc, now_qpc, now_unix_us)
            };
            match sink.try_send(AudioChunk {
                capture_ts_us,
                channels: CHANNELS,
                frames,
                samples,
            }) {
                Ok(()) => {}
                Err(TrySendError::Full(_)) => *dropped = dropped.saturating_add(1),
                Err(TrySendError::Closed(_)) => return Ok(()),
            }
        }
    }
}

impl Drop for OpenCapture {
    fn drop(&mut self) {
        // SAFETY: this client is only used on its initialized MTA thread.
        // Stop can fail when the endpoint was removed, but references must
        // still be released. No buffer remains acquired here.
        let _ = unsafe { self.client.Stop() };
    }
}

struct TaskMem<T>(*mut T);

impl<T> Drop for TaskMem<T> {
    fn drop(&mut self) {
        // SAFETY: private callers create this guard only for allocations
        // returned by GetMixFormat/GetId; it owns their sole matching free.
        unsafe { CoTaskMemFree(Some(self.0.cast())) };
    }
}

fn endpoint_id(device: &IMMDevice) -> Result<Vec<u16>, windows::core::Error> {
    // SAFETY: device is a valid COM endpoint on the calling MTA thread.
    let id = unsafe { device.GetId()? };
    let _allocation = TaskMem(id.0);
    // SAFETY: GetId returns a NUL-terminated UTF-16 allocation. The guard keeps
    // it alive while as_wide reads it and the Vec copies it.
    Ok(unsafe { id.as_wide() }.to_vec())
}

fn parse_native_format(mix: &TaskMem<WAVEFORMATEX>) -> Result<WaveFormat, FormatError> {
    if mix.0.is_null() {
        return Err(FormatError::Bad);
    }
    // SAFETY: GetMixFormat allocated at least a WAVEFORMATEX. An unaligned copy
    // also handles the SDK's packed wave-format structures.
    let header = unsafe { mix.0.read_unaligned() };
    let ext = if header.wFormatTag == WAVE_FORMAT_EXTENSIBLE {
        if header.cbSize < 22 {
            return Err(FormatError::Bad);
        }
        // SAFETY: cbSize advertises the full 22-byte extensible suffix and the
        // TaskMem guard still owns that GetMixFormat allocation.
        let ext = unsafe { mix.0.cast::<WAVEFORMATEXTENSIBLE>().read_unaligned() };
        // SAFETY: WAVE_FORMAT_EXTENSIBLE PCM/float uses wValidBitsPerSample.
        let valid_bits = unsafe { ext.Samples.wValidBitsPerSample };
        let subtype = ext.SubFormat;
        Some((valid_bits, subtype.to_u128()))
    } else {
        None
    };
    parse_wave_format(
        header.wFormatTag,
        header.nChannels,
        header.nSamplesPerSec,
        header.wBitsPerSample,
        header.nBlockAlign,
        ext,
    )
}

fn copy_packet(
    data: *const u8,
    frames: u32,
    flags: u32,
    format: &WaveFormat,
) -> Result<Vec<f32>, FormatError> {
    let frames_len = usize::try_from(frames).map_err(|_| FormatError::Bad)?;
    let sample_count = frames_len.checked_mul(2).ok_or(FormatError::Bad)?;
    if flags & AUDCLNT_BUFFERFLAGS_SILENT.0 as u32 != 0 {
        return Ok(vec![0.0; sample_count]);
    }
    let byte_len = frames_len
        .checked_mul(usize::from(format.block_align))
        .filter(|&len| len <= isize::MAX as usize)
        .ok_or(FormatError::Bad)?;
    if data.is_null() {
        return Err(FormatError::Bad);
    }
    // SAFETY: the only caller passes a live WASAPI buffer containing frames *
    // block_align bytes, exclusively borrowed until ReleaseBuffer.
    let bytes = unsafe { std::slice::from_raw_parts(data, byte_len) };
    let mut samples = Vec::with_capacity(sample_count);
    to_stereo_f32(bytes, format, frames, &mut samples)?;
    Ok(samples)
}

struct QpcClock(u64);

impl QpcClock {
    fn new() -> Result<Self, windows::core::Error> {
        let mut frequency = 0_i64;
        // SAFETY: the API writes one i64 into a valid local. Windows guarantees
        // a positive frequency that remains fixed during system uptime.
        unsafe { QueryPerformanceFrequency(&mut frequency)? };
        Ok(Self(frequency as u64))
    }

    fn now_100ns(&self) -> Result<u64, windows::core::Error> {
        let mut counter = 0_i64;
        // SAFETY: the API writes one counter value into a valid local pointer.
        unsafe { QueryPerformanceCounter(&mut counter)? };
        let units = u128::from(counter as u64) * 10_000_000 / u128::from(self.0);
        Ok(units.min(u128::from(u64::MAX)) as u64)
    }
}

fn cancelled(stop: &AtomicBool, sink: &AudioChunkSink) -> bool {
    stop.load(Ordering::Acquire) || sink.is_closed()
}

fn wait_or_stop(stop: &AtomicBool, sink: &AudioChunkSink, duration: Duration) -> bool {
    let deadline = Instant::now() + duration;
    while !cancelled(stop, sink) {
        let remaining = deadline.saturating_duration_since(Instant::now());
        if remaining.is_zero() {
            return false;
        }
        thread::sleep(remaining.min(POLL_INTERVAL));
    }
    true
}
