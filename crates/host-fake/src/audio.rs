//! Controllable system-audio capture for Linux end-to-end tests.

use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_core::platform::{
    AudioCaptureConfig, AudioChunk, AudioChunkSink, CaptureHandle, PlatformError, Result,
};
use rs_audio::gen::{sample_index, sample_ts_us, Signal};
use rs_audio::SAMPLE_RATE;
use rsp_core::clock::unix_micros;
use tokio::task::JoinHandle;
use tokio::time::{interval, Instant, MissedTickBehavior};

const DEFAULT_CHUNK_FRAMES: u32 = 1024;
const TICK: Duration = Duration::from_millis(5);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum FakeAudioFailure {
    PermissionDenied,
    Platform,
}

struct AudioState {
    signal: Mutex<Signal>,
    supported: AtomicBool,
    failure: Mutex<Option<FakeAudioFailure>>,
    chunk_frames: AtomicU32,
    active: AtomicUsize,
    starts: AtomicU64,
    last_config: Mutex<Option<AudioCaptureConfig>>,
    dropped_chunks: AtomicU64,
}

impl Default for AudioState {
    fn default() -> Self {
        Self {
            signal: Mutex::new(Signal::Sine {
                freq_hz: 440,
                amp: 0.25,
            }),
            supported: AtomicBool::new(true),
            failure: Mutex::new(None),
            chunk_frames: AtomicU32::new(DEFAULT_CHUNK_FRAMES),
            active: AtomicUsize::new(0),
            starts: AtomicU64::new(0),
            last_config: Mutex::new(None),
            dropped_chunks: AtomicU64::new(0),
        }
    }
}

/// Clones share signal selection, controls and counters.
#[derive(Clone, Default)]
pub struct FakeAudio {
    state: Arc<AudioState>,
}

impl FakeAudio {
    pub fn set_signal(&self, signal: Signal) {
        *self
            .state
            .signal
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = signal;
    }

    pub fn set_supported(&self, on: bool) {
        self.state.supported.store(on, Ordering::Relaxed);
    }

    pub fn fail_next(&self, failure: FakeAudioFailure) {
        *self
            .state
            .failure
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(failure);
    }

    pub fn set_chunk_frames(&self, frames: u32) {
        self.state
            .chunk_frames
            .store(frames.max(1), Ordering::Relaxed);
    }

    pub fn active(&self) -> usize {
        self.state.active.load(Ordering::Relaxed)
    }

    pub fn starts(&self) -> u64 {
        self.state.starts.load(Ordering::Relaxed)
    }

    pub fn last_config(&self) -> Option<AudioCaptureConfig> {
        *self
            .state
            .last_config
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    pub fn dropped_chunks(&self) -> u64 {
        self.state.dropped_chunks.load(Ordering::Relaxed)
    }
}

pub(crate) fn supported(audio: &FakeAudio) -> bool {
    audio.state.supported.load(Ordering::Relaxed)
}

pub(crate) fn start(
    audio: &FakeAudio,
    cfg: AudioCaptureConfig,
    sink: AudioChunkSink,
) -> Result<Box<dyn CaptureHandle>> {
    if !supported(audio) {
        return Err(PlatformError::Unsupported);
    }

    let failure = audio
        .state
        .failure
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .take();
    if let Some(failure) = failure {
        return Err(match failure {
            FakeAudioFailure::PermissionDenied => {
                PlatformError::PermissionDenied("audio capture".into())
            }
            FakeAudioFailure::Platform => PlatformError::Platform("audio capture".into()),
        });
    }

    if cfg.sample_rate != SAMPLE_RATE {
        return Err(PlatformError::Unsupported);
    }
    if cfg.channels != 1 && cfg.channels != 2 {
        return Err(PlatformError::Platform("channels".into()));
    }

    let runtime = tokio::runtime::Handle::try_current()
        .map_err(|error| PlatformError::Platform(error.to_string()))?;
    let chunk_frames = audio.state.chunk_frames.load(Ordering::Relaxed).max(1);
    let channels = cfg.channels;

    audio.state.starts.fetch_add(1, Ordering::Relaxed);
    *audio
        .state
        .last_config
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner()) = Some(cfg);
    audio.state.active.fetch_add(1, Ordering::Relaxed);

    let active = ActiveCapture {
        audio: audio.clone(),
        live: Arc::new(AtomicBool::new(true)),
    };
    let handle_active = active.clone();
    let audio = audio.clone();
    let task = runtime.spawn(async move {
        let _active = active;
        let started = Instant::now();
        let base = sample_index(unix_micros());
        let mut next_index = base;
        let mut produced_frames = 0u64;
        let mut ticker = interval(TICK);
        ticker.set_missed_tick_behavior(MissedTickBehavior::Delay);

        loop {
            ticker.tick().await;
            let due_frames =
                u64::try_from(started.elapsed().as_micros() * u128::from(SAMPLE_RATE) / 1_000_000)
                    .unwrap_or(u64::MAX);

            while produced_frames.saturating_add(u64::from(chunk_frames)) <= due_frames {
                let signal = *audio
                    .state
                    .signal
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner());
                let mut samples = vec![0.0; chunk_frames as usize * usize::from(channels)];
                signal.render_index(next_index, usize::from(channels), &mut samples);
                let chunk = AudioChunk {
                    capture_ts_us: sample_ts_us(next_index),
                    channels,
                    frames: chunk_frames,
                    samples,
                };
                match sink.try_send(chunk) {
                    Ok(()) => {}
                    Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => {
                        audio.state.dropped_chunks.fetch_add(1, Ordering::Relaxed);
                    }
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => return,
                }

                next_index = next_index.saturating_add(u64::from(chunk_frames));
                produced_frames = produced_frames.saturating_add(u64::from(chunk_frames));
            }
        }
    });

    Ok(Box::new(FakeAudioCapture {
        task,
        active: handle_active,
    }))
}

#[derive(Clone)]
struct ActiveCapture {
    audio: FakeAudio,
    live: Arc<AtomicBool>,
}

impl ActiveCapture {
    fn finish(&self) {
        if self.live.swap(false, Ordering::AcqRel) {
            self.audio.state.active.fetch_sub(1, Ordering::Relaxed);
        }
    }
}

impl Drop for ActiveCapture {
    fn drop(&mut self) {
        self.finish();
    }
}

struct FakeAudioCapture {
    task: JoinHandle<()>,
    active: ActiveCapture,
}

impl CaptureHandle for FakeAudioCapture {}

impl Drop for FakeAudioCapture {
    fn drop(&mut self) {
        self.task.abort();
        self.active.finish();
    }
}
