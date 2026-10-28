//! Capture the default PipeWire output device's monitor for the host audio pipeline.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use host_core::platform::{
    AudioCaptureConfig, AudioChunk, AudioChunkSink, CaptureHandle, PlatformError,
};
use rsp_core::clock::unix_micros;
use tokio::sync::mpsc::error::TrySendError;

use crate::pw::{Direction, PwLib, PwStream, Remote, StreamCtl, StreamHandler, StreamSpec};
use crate::pw_ffi::stream_flag;
use crate::spa_pod::{self, audio_format, param};

pub const MIN_RATE: u32 = 8_000;
pub const MAX_RATE: u32 = 192_000;
pub const MAX_CHANNELS: u8 = 8;

/// Decode F32LE, discarding incomplete samples, replacing non-finite values
/// with silence, and clamping the remaining samples to [-1, 1].
pub fn f32le_samples(bytes: &[u8]) -> Vec<f32> {
    bytes
        .chunks_exact(4)
        .map(|bytes| {
            let sample = f32::from_le_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]);
            if sample.is_finite() {
                sample.clamp(-1.0, 1.0)
            } else {
                0.0
            }
        })
        .collect()
}

/// Timestamp of the first sample, saturating at zero. A zero rate returns zero;
/// capture itself rejects zero rates before opening a stream.
pub fn chunk_start_us(now_us: u64, frames: u32, rate: u32) -> u64 {
    if rate == 0 {
        return 0;
    }
    now_us.saturating_sub(u64::from(frames) * 1_000_000 / u64::from(rate))
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct AudioStats {
    pub chunks: u64,
    pub dropped: u64,
    pub bad_buffers: u64,
}

#[derive(Default)]
struct Counters {
    chunks: AtomicU64,
    dropped: AtomicU64,
    bad_buffers: AtomicU64,
}

impl Counters {
    fn snapshot(&self) -> AudioStats {
        AudioStats {
            chunks: self.chunks.load(Ordering::Relaxed),
            dropped: self.dropped.load(Ordering::Relaxed),
            bad_buffers: self.bad_buffers.load(Ordering::Relaxed),
        }
    }
}

/// Owns the stream and its callback's only sink sender. Dropping the stream
/// drains callbacks and releases that sender before returning.
pub struct AudioHandle {
    _stream: PwStream,
    counters: Arc<Counters>,
}

impl AudioHandle {
    pub fn stats(&self) -> AudioStats {
        self.counters.snapshot()
    }
}

impl CaptureHandle for AudioHandle {}

struct Monitor {
    cfg: AudioCaptureConfig,
    sink: Option<AudioChunkSink>,
    counters: Arc<Counters>,
    format_ready: bool,
    format_rejected: bool,
}

impl Monitor {
    fn format_changed(&mut self, pod: Option<&[u8]>) {
        self.format_ready = false;
        // Removing a format during reconnection is not a negotiation failure.
        let Some(pod) = pod else { return };
        let parsed = spa_pod::parse_audio_format(pod);
        if parsed.as_ref().is_ok_and(|format| {
            format.format == audio_format::F32_LE
                && format.rate == self.cfg.sample_rate
                && format.channels == u32::from(self.cfg.channels)
        }) {
            self.format_ready = !self.format_rejected;
            return;
        }

        self.counters.bad_buffers.fetch_add(1, Ordering::Relaxed);
        if !self.format_rejected {
            // Format negotiation runs outside the real-time process callback.
            tracing::warn!(
                requested_rate = self.cfg.sample_rate,
                requested_channels = self.cfg.channels,
                negotiated = ?parsed,
                "PipeWire monitor format does not match the requested F32LE audio"
            );
        }
        self.format_rejected = true;
    }

    fn capture_bytes(&mut self, bytes: Option<&[u8]>) {
        if !self.format_ready {
            return;
        }
        let Some(sink) = &self.sink else { return };
        let Some(bytes) = bytes else {
            self.counters.bad_buffers.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let mut samples = f32le_samples(bytes);
        let channels = usize::from(self.cfg.channels);
        samples.truncate(samples.len() - samples.len() % channels);
        let Ok(frames) = u32::try_from(samples.len() / channels) else {
            self.counters.bad_buffers.fetch_add(1, Ordering::Relaxed);
            return;
        };
        let chunk = AudioChunk {
            capture_ts_us: chunk_start_us(unix_micros(), frames, self.cfg.sample_rate),
            channels: self.cfg.channels,
            frames,
            samples,
        };
        match sink.try_send(chunk) {
            Ok(()) => {
                self.counters.chunks.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Full(_)) => {
                self.counters.dropped.fetch_add(1, Ordering::Relaxed);
            }
            Err(TrySendError::Closed(_)) => self.sink = None,
        }
    }
}

impl StreamHandler for Monitor {
    fn param_changed(&mut self, _ctl: &mut StreamCtl<'_>, id: u32, pod: Option<&[u8]>) {
        if id == param::FORMAT {
            self.format_changed(pod);
        }
    }

    fn process(&mut self, ctl: &mut StreamCtl<'_>) {
        let Some(buffer) = ctl.dequeue() else {
            return;
        };
        if let Some(data) = buffer.data(0) {
            self.capture_bytes(data.valid());
        } else {
            self.capture_bytes(None);
        }
        // Every dequeued buffer is returned, including rejected/closed/full paths.
        ctl.queue(buffer);
    }
}

pub fn start_monitor_capture(
    lib: Arc<PwLib>,
    cfg: AudioCaptureConfig,
    sink: AudioChunkSink,
) -> Result<AudioHandle, PlatformError> {
    if !(MIN_RATE..=MAX_RATE).contains(&cfg.sample_rate)
        || !(1..=MAX_CHANNELS).contains(&cfg.channels)
    {
        return Err(PlatformError::Unsupported);
    }

    // The host produces no audio of its own, so exclude_self needs no filtering.
    let counters = Arc::new(Counters::default());
    let stream = PwStream::start(
        lib,
        Remote::Default,
        StreamSpec {
            name: "rs-host-audio".into(),
            props: [
                ("media.type", "Audio"),
                ("media.category", "Capture"),
                ("media.role", "Screen"),
                ("stream.capture.sink", "true"),
                ("node.name", "rs-host-audio"),
                ("node.dont-reconnect", "false"),
            ]
            .into_iter()
            .map(|(key, value)| (key.into(), value.into()))
            .collect(),
            direction: Direction::Input,
            target: None,
            flags: stream_flag::AUTOCONNECT | stream_flag::MAP_BUFFERS | stream_flag::RT_PROCESS,
            params: vec![spa_pod::audio_enum_format(
                audio_format::F32_LE,
                cfg.sample_rate,
                u32::from(cfg.channels),
            )],
        },
        Box::new(Monitor {
            cfg,
            sink: Some(sink),
            counters: counters.clone(),
            format_ready: false,
            format_rejected: false,
        }),
    )
    .map_err(|error| PlatformError::Platform(format!("PipeWire audio capture: {error}")))?;

    Ok(AudioHandle {
        _stream: stream,
        counters,
    })
}

#[cfg(test)]
mod tests {
    use super::*;
    use tokio::sync::mpsc;

    fn monitor(capacity: usize) -> (Monitor, mpsc::Receiver<AudioChunk>) {
        let (sink, receiver) = mpsc::channel(capacity);
        (
            Monitor {
                cfg: AudioCaptureConfig {
                    sample_rate: 48_000,
                    channels: 2,
                    exclude_self: true,
                },
                sink: Some(sink),
                counters: Arc::new(Counters::default()),
                format_ready: false,
                format_rejected: false,
            },
            receiver,
        )
    }

    fn format() -> Vec<u8> {
        spa_pod::audio_enum_format(audio_format::F32_LE, 48_000, 2)
    }

    #[test]
    fn requires_format_and_trims_incomplete_frames() {
        let (mut monitor, mut receiver) = monitor(2);
        let bytes: Vec<_> = [0.5_f32, -2.0, 0.25]
            .into_iter()
            .flat_map(f32::to_le_bytes)
            .chain([1, 2, 3])
            .collect();
        monitor.capture_bytes(Some(&bytes));
        assert!(receiver.try_recv().is_err());

        monitor.format_changed(Some(&format()));
        let before = unix_micros();
        monitor.capture_bytes(Some(&bytes));
        let chunk = receiver.try_recv().unwrap();
        assert_eq!(chunk.channels, 2);
        assert_eq!(chunk.frames, 1);
        assert_eq!(chunk.samples, [0.5, -1.0]);
        assert!(chunk.capture_ts_us >= chunk_start_us(before, 1, 48_000));
        assert!(chunk.capture_ts_us <= chunk_start_us(unix_micros(), 1, 48_000));

        monitor.format_changed(None);
        monitor.capture_bytes(Some(&bytes));
        assert!(receiver.try_recv().is_err());
        monitor.format_changed(Some(&format()));
        monitor.capture_bytes(Some(&bytes));
        assert!(receiver.try_recv().is_ok());
        assert_eq!(monitor.counters.snapshot().bad_buffers, 0);
    }

    #[test]
    fn rejects_mismatched_or_malformed_formats_permanently() {
        for pod in [
            spa_pod::audio_enum_format(audio_format::S16_LE, 48_000, 2),
            spa_pod::audio_enum_format(audio_format::F32_LE, 44_100, 2),
            spa_pod::audio_enum_format(audio_format::F32_LE, 48_000, 1),
            vec![0; 3],
        ] {
            let (mut monitor, mut receiver) = monitor(1);
            monitor.format_changed(Some(&format()));
            monitor.format_changed(Some(&pod));
            monitor.capture_bytes(Some(&[0; 8]));
            monitor.format_changed(None);
            monitor.format_changed(Some(&format()));
            monitor.capture_bytes(Some(&[0; 8]));
            assert!(receiver.try_recv().is_err());
            assert_eq!(monitor.counters.snapshot().bad_buffers, 1);
            assert_eq!(monitor.counters.snapshot().chunks, 0);
        }
    }

    #[test]
    fn full_sink_drops_and_closed_sink_stops_sending() {
        let (mut monitor, receiver) = monitor(1);
        monitor.format_changed(Some(&format()));
        for _ in 0..3 {
            monitor.capture_bytes(Some(&[0; 8]));
        }
        let stats = AudioStats {
            chunks: 1,
            dropped: 2,
            bad_buffers: 0,
        };
        assert_eq!(monitor.counters.snapshot(), stats);
        drop(receiver);
        monitor.capture_bytes(Some(&[0; 8]));
        assert!(monitor.sink.is_none());
        monitor.capture_bytes(Some(&[0; 8]));
        assert_eq!(monitor.counters.snapshot(), stats);
    }

    #[test]
    fn bad_buffer_does_not_poison_capture() {
        let (mut monitor, mut receiver) = monitor(1);
        monitor.format_changed(Some(&format()));
        monitor.capture_bytes(None);
        assert!(receiver.try_recv().is_err());
        monitor.capture_bytes(Some(&[0; 8]));
        assert!(receiver.try_recv().is_ok());
        assert_eq!(monitor.counters.snapshot().bad_buffers, 1);
        drop(monitor);
        assert!(receiver.is_closed());
    }
}
