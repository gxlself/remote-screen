//! ScreenCaptureKit system-audio capture for macOS 13 and later.

use std::ffi::{c_char, c_void, CStr};
use std::ptr::NonNull;
use std::slice;
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::platform::{
    AudioCaptureConfig, AudioChunk, AudioChunkSink, CaptureHandle, PlatformError, Result,
};

type AudioCb = extern "C" fn(*mut c_void, u64, u32, u32, *const f32);

extern "C" {
    fn rs_audio_capture_supported() -> bool;
    fn rs_audio_capture_start(
        sample_rate: u32,
        channels: u32,
        exclude_self: bool,
        ctx: *mut c_void,
        cb: AudioCb,
        err: *mut i32,
        msg: *mut c_char,
        msg_len: usize,
    ) -> *mut c_void;
    fn rs_audio_capture_stop(handle: *mut c_void);
}

/// Return whether ScreenCaptureKit audio capture is available on this host.
pub(crate) fn supported() -> bool {
    // SAFETY: the shim function has no arguments and only reads the OS version.
    unsafe { rs_audio_capture_supported() }
}

struct CallbackCtx {
    sink: AudioChunkSink,
    dropped: AtomicU64,
}

#[no_mangle]
extern "C" fn rs_audio_report_dropped(ctx: *mut c_void, count: u64) {
    if ctx.is_null() {
        return;
    }
    // SAFETY: native stop calls this after draining sample callbacks, while
    // MacAudioCapture still owns its boxed context.
    let ctx = unsafe { &*(ctx as *const CallbackCtx) };
    ctx.dropped.fetch_add(count, Ordering::Relaxed);
}

extern "C" fn on_audio(
    ctx: *mut c_void,
    capture_ts_us: u64,
    channels: u32,
    frames: u32,
    samples: *const f32,
) {
    if ctx.is_null() || samples.is_null() || channels == 0 || frames == 0 {
        return;
    }
    let Some(sample_count) = (channels as usize).checked_mul(frames as usize) else {
        return;
    };
    // SAFETY: the native callback lends exactly frames * channels f32 values
    // for the duration of this call, and stop drains this callback before the
    // boxed context is released.
    let samples = unsafe { slice::from_raw_parts(samples, sample_count) }.to_vec();
    // SAFETY: ctx points to the CallbackCtx owned by MacAudioCapture. Native
    // stop synchronizes the callback queue before that owner is dropped.
    let ctx = unsafe { &*(ctx as *const CallbackCtx) };
    let channels = match u8::try_from(channels) {
        Ok(value) => value,
        Err(_) => {
            ctx.dropped.fetch_add(1, Ordering::Relaxed);
            return;
        }
    };
    if ctx
        .sink
        .try_send(AudioChunk {
            capture_ts_us,
            channels,
            frames,
            samples,
        })
        .is_err()
    {
        ctx.dropped.fetch_add(1, Ordering::Relaxed);
    }
}

/// Independent system-audio stream. Drop stops and drains native callbacks.
pub struct MacAudioCapture {
    handle: Option<NonNull<c_void>>,
    ctx: Box<CallbackCtx>,
}

// SAFETY: the native handle serializes callbacks on its own queue, ctx has a
// stable Box allocation, and Drop drains that queue before freeing ctx.
unsafe impl Send for MacAudioCapture {}

impl CaptureHandle for MacAudioCapture {}

impl MacAudioCapture {
    /// Start a stream directly, allowing probes to read the final drop count.
    /// Normal host callers use `Platform::capture_audio` instead.
    pub fn start(cfg: AudioCaptureConfig, sink: AudioChunkSink) -> Result<Self> {
        if cfg.sample_rate != 48_000 || !matches!(cfg.channels, 1 | 2) || !supported() {
            return Err(PlatformError::Unsupported);
        }
        let mut ctx = Box::new(CallbackCtx {
            sink,
            dropped: AtomicU64::new(0),
        });
        let mut code = 3_i32;
        let mut message = [0 as c_char; 512];
        // SAFETY: ctx stays at a stable address in its Box until this capture
        // is dropped; the shim drains callbacks even when starting fails.
        let handle = unsafe {
            rs_audio_capture_start(
                cfg.sample_rate,
                u32::from(cfg.channels),
                cfg.exclude_self,
                ctx.as_mut() as *mut CallbackCtx as *mut c_void,
                on_audio,
                &mut code,
                message.as_mut_ptr(),
                message.len(),
            )
        };
        let handle =
            NonNull::new(handle).ok_or_else(|| map_start_error(code, error_message(&message)))?;
        Ok(Self {
            handle: Some(handle),
            ctx,
        })
    }

    /// Stop capture and return chunks dropped by the sink or PCM conversion.
    /// This consumes the handle and releases its sender.
    pub fn finish(mut self) -> u64 {
        self.stop();
        self.ctx.dropped.load(Ordering::Relaxed)
    }

    fn stop(&mut self) {
        if let Some(handle) = self.handle.take() {
            // SAFETY: handle is owned by this value; stop synchronously drains
            // all native callbacks that can access ctx.
            unsafe { rs_audio_capture_stop(handle.as_ptr()) };
            let dropped = self.ctx.dropped.load(Ordering::Relaxed);
            tracing::debug!(dropped, "ScreenCaptureKit audio capture stopped");
        }
    }
}

impl Drop for MacAudioCapture {
    fn drop(&mut self) {
        self.stop();
    }
}

pub(crate) fn start(
    cfg: AudioCaptureConfig,
    sink: AudioChunkSink,
) -> Result<Box<dyn CaptureHandle>> {
    MacAudioCapture::start(cfg, sink).map(|capture| Box::new(capture) as Box<dyn CaptureHandle>)
}

fn error_message(message: &[c_char]) -> String {
    // SAFETY: the shim receives a zero-initialized buffer and always writes a
    // terminating NUL before returning.
    unsafe { CStr::from_ptr(message.as_ptr()) }
        .to_string_lossy()
        .into_owned()
}

fn map_start_error(code: i32, message: String) -> PlatformError {
    match code {
        1 => PlatformError::PermissionDenied(message),
        2 => PlatformError::Unsupported,
        _ => PlatformError::Platform(message),
    }
}

/// Convert a host-clock presentation timestamp to the unix-microsecond clock
/// used by the platform contract.
pub(crate) fn pts_to_unix_us(now_unix_us: u64, now_host_s: f64, pts_s: f64) -> u64 {
    if !now_host_s.is_finite() || !pts_s.is_finite() {
        return now_unix_us;
    }
    let delta_us = (now_host_s - pts_s) * 1_000_000.0;
    if !delta_us.is_finite() {
        return now_unix_us;
    }
    if delta_us >= 0.0 {
        let elapsed = delta_us.round();
        if elapsed >= u64::MAX as f64 {
            0
        } else {
            now_unix_us.saturating_sub(elapsed as u64)
        }
    } else {
        let ahead = (-delta_us).round();
        if ahead >= u64::MAX as f64 {
            u64::MAX
        } else {
            now_unix_us.saturating_add(ahead as u64)
        }
    }
}

#[no_mangle]
extern "C" fn rs_audio_pts_to_unix_us(now_unix_us: u64, now_host_s: f64, pts_s: f64) -> u64 {
    pts_to_unix_us(now_unix_us, now_host_s, pts_s)
}

/// Interleave into the native stream's reusable buffer without allocating.
fn interleave_channels(channels: &[&[f32]], frames: usize, out: &mut [f32]) -> bool {
    if channels.is_empty()
        || channels.iter().any(|channel| channel.len() < frames)
        || frames.checked_mul(channels.len()) != Some(out.len())
    {
        return false;
    }
    for frame in 0..frames {
        for (index, channel) in channels.iter().enumerate() {
            let sample = channel[frame];
            out[frame * channels.len() + index] = if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            };
        }
    }
    true
}

#[no_mangle]
extern "C" fn rs_audio_interleave_f32(
    planes: *const *const f32,
    channels: u32,
    frames: u32,
    out: *mut f32,
) -> bool {
    if planes.is_null() || out.is_null() || !matches!(channels, 1 | 2) || frames == 0 {
        return false;
    }
    let channels = channels as usize;
    let frames = frames as usize;
    // SAFETY: the shim lends channels plane pointers, each validated to contain
    // frames f32 samples, until this function returns.
    let pointers = unsafe { slice::from_raw_parts(planes, channels) };
    let mut slices: [&[f32]; 2] = [&[], &[]];
    for (channel, &pointer) in pointers.iter().enumerate() {
        if pointer.is_null() {
            return false;
        }
        // SAFETY: the shim checks every plane's size before calling this
        // function, and its retained CMBlockBuffer keeps the memory readable.
        slices[channel] = unsafe { slice::from_raw_parts(pointer, frames) };
    }
    // SAFETY: the shim supplies a separately allocated f32 destination with
    // frames * channels elements, exclusively borrowed until this call ends.
    let out = unsafe { slice::from_raw_parts_mut(out, frames * channels) };
    interleave_channels(&slices[..channels], frames, out)
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn interleaves_two_channel_planes() {
        let left = [0.0, 0.1, 0.2, 0.3];
        let right = [-0.4, -0.5, -0.6, -0.7];
        let mut out = [0.0; 8];
        assert!(interleave_channels(&[&left, &right], 4, &mut out));
        assert_eq!(out, [0.0, -0.4, 0.1, -0.5, 0.2, -0.6, 0.3, -0.7]);
    }

    #[test]
    fn leaves_mono_samples_unchanged() {
        let mono = [0.25, -0.5, 0.75];
        let mut out = [0.0; 3];
        assert!(interleave_channels(&[&mono], 3, &mut out));
        assert_eq!(out, mono);
    }

    #[test]
    fn converts_pts_to_unix_microseconds() {
        assert_eq!(
            pts_to_unix_us(1_700_000_000_000_000, 100.0, 99.95),
            1_699_999_999_950_000
        );
    }

    #[test]
    fn pts_conversion_saturates_and_handles_invalid_times() {
        assert_eq!(pts_to_unix_us(1, 100.0, 99.0), 0);
        assert_eq!(pts_to_unix_us(u64::MAX - 1, 99.0, 100.0), u64::MAX);
        assert_eq!(pts_to_unix_us(123, f64::NAN, 99.0), 123);
        assert_eq!(pts_to_unix_us(123, 100.0, f64::INFINITY), 123);
    }

    #[test]
    fn callback_copies_pcm_and_counts_full_and_closed_sinks() {
        let (sink, mut rx) = tokio::sync::mpsc::channel(1);
        let mut ctx = Box::new(CallbackCtx {
            sink,
            dropped: AtomicU64::new(0),
        });
        let pointer = ctx.as_mut() as *mut CallbackCtx as *mut c_void;
        let mut samples = [0.25, -0.5, 0.75, -1.0];
        on_audio(pointer, 42, 2, 2, samples.as_ptr());
        samples[0] = 1.0;
        on_audio(pointer, 84, 2, 2, samples.as_ptr());
        assert_eq!(ctx.dropped.load(Ordering::Relaxed), 1);
        let chunk = rx.try_recv().expect("first chunk was sent");
        assert_eq!(chunk.capture_ts_us, 42);
        assert_eq!(chunk.channels, 2);
        assert_eq!(chunk.frames, 2);
        assert_eq!(chunk.samples, [0.25, -0.5, 0.75, -1.0]);
        drop(rx);
        on_audio(pointer, 126, 2, 2, samples.as_ptr());
        assert_eq!(ctx.dropped.load(Ordering::Relaxed), 2);
        rs_audio_report_dropped(pointer, 3);
        assert_eq!(ctx.dropped.load(Ordering::Relaxed), 5);
    }

    #[test]
    fn maps_native_errors() {
        assert!(matches!(
            map_start_error(1, "permission".into()),
            PlatformError::PermissionDenied(_)
        ));
        assert!(matches!(
            map_start_error(2, "unsupported".into()),
            PlatformError::Unsupported
        ));
        assert!(matches!(
            map_start_error(3, "failed".into()),
            PlatformError::Platform(_)
        ));
    }
}
