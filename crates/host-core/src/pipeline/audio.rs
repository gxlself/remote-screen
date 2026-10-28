//! Shared system-audio capture/encoding and per-connection datagram delivery.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use bytes::Bytes;
use rs_audio::codec::{AudioEncoder, EncoderConfig};
use rs_audio::framer::Framer;
use rs_audio::silence::{Gate, SilenceGate};
use rsp_core::audio::{self, AudioHeader};
use rsp_core::ratectl::Decision;
use rsp_transport::{conn::SendDatagramError, Conn, ConnError, DatagramKind};
use tokio::sync::{broadcast, mpsc};
use tokio::task::JoinHandle;

use crate::platform::{AudioCaptureConfig, AudioChunkSink, CaptureHandle, Platform, PlatformError};

/// 96 kbps Opus plus packet headers and transport overhead.
pub const AUDIO_RESERVE_KBPS: u32 = 128;
/// Bounded platform-to-encoder queue.
pub const AUDIO_CHUNK_QUEUE: usize = 32;
/// Sixteen 10 ms packets of fanout history.
pub const AUDIO_FANOUT: usize = 16;
pub const AUDIO_MIN_VIDEO_KBPS: u32 = 500;

#[derive(Debug, Clone)]
pub struct EncodedAudio {
    pub capture_ts_us: u64,
    pub payload: Bytes,
    pub after_gap: bool,
}

struct RunningCapture {
    _handle: Box<dyn CaptureHandle>,
    // Only the encoder keeps the channel open. A capture source that stops
    // producing must let existing receivers observe its termination.
    fanout: broadcast::WeakSender<EncodedAudio>,
}

#[derive(Default)]
struct HubState {
    capture: Option<RunningCapture>,
    subscribers: usize,
}

/// A Host shares one capture source and one encoder across all subscriptions.
/// Subscriptions keep this state alive until the last connection stops audio.
pub struct AudioHub {
    cfg: EncoderConfig,
    state: Arc<Mutex<HubState>>,
}

impl AudioHub {
    pub fn new(cfg: EncoderConfig) -> Self {
        Self {
            cfg,
            state: Arc::new(Mutex::new(HubState::default())),
        }
    }

    pub fn subscribe<P: Platform>(&self, platform: &P) -> Result<AudioSubscription, PlatformError> {
        if !platform.audio_capture_supported() {
            return Err(PlatformError::Unsupported);
        }
        self.subscribe_with(|cfg, sink| platform.capture_audio(cfg, sink))
    }

    /// Platform-independent entry point; `start` owns capture startup.
    pub fn subscribe_with(
        &self,
        start: impl FnOnce(
            AudioCaptureConfig,
            AudioChunkSink,
        ) -> Result<Box<dyn CaptureHandle>, PlatformError>,
    ) -> Result<AudioSubscription, PlatformError> {
        // Serialize startup with the last subscription's capture teardown.
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        let receiver = if let Some(capture) = &state.capture {
            capture
                .fanout
                .upgrade()
                .ok_or_else(|| PlatformError::Platform("audio capture has stopped".into()))?
                .subscribe()
        } else {
            let encoder = AudioEncoder::new(&self.cfg)
                .map_err(|e| PlatformError::Platform(format!("audio encoder: {e}")))?;
            let (chunks, receiver) = mpsc::channel(AUDIO_CHUNK_QUEUE);
            let (fanout, subscription) = broadcast::channel(AUDIO_FANOUT);
            let handle = start(
                AudioCaptureConfig {
                    sample_rate: 48_000,
                    channels: 2,
                    exclude_self: true,
                },
                chunks,
            )?;
            let output = fanout.downgrade();
            // Dropping the JoinHandle detaches the thread. Capture teardown
            // releases its sink, so blocking_recv eventually returns None.
            std::thread::Builder::new()
                .name("rs-audio-enc".into())
                .spawn(move || encode_chunks(encoder, receiver, fanout))?;
            state.capture = Some(RunningCapture {
                _handle: handle,
                fanout: output,
            });
            subscription
        };
        state.subscribers += 1;
        Ok(AudioSubscription {
            state: Arc::clone(&self.state),
            receiver,
            lagged: Arc::new(AtomicU64::new(0)),
        })
    }

    pub fn running(&self) -> bool {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .capture
            .as_ref()
            .is_some_and(|capture| capture.fanout.strong_count() > 0)
    }

    pub fn subscribers(&self) -> usize {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .subscribers
    }
}

pub struct AudioSubscription {
    state: Arc<Mutex<HubState>>,
    receiver: broadcast::Receiver<EncodedAudio>,
    lagged: Arc<AtomicU64>,
}

impl AudioSubscription {
    /// Skip overwritten packets, counting them, until a packet or shutdown.
    pub async fn recv(&mut self) -> Option<EncodedAudio> {
        loop {
            match self.receiver.recv().await {
                Ok(frame) => return Some(frame),
                Err(broadcast::error::RecvError::Lagged(count)) => {
                    self.lagged.fetch_add(count, Ordering::Relaxed);
                }
                Err(broadcast::error::RecvError::Closed) => return None,
            }
        }
    }

    /// Total packets overwritten before this subscription could receive them.
    pub fn lagged(&self) -> u64 {
        self.lagged.load(Ordering::Relaxed)
    }
}

impl Drop for AudioSubscription {
    fn drop(&mut self) {
        let mut state = self.state.lock().unwrap_or_else(|e| e.into_inner());
        state.subscribers -= 1;
        if state.subscribers == 0 {
            // Do not join here: this often runs on a Tokio worker. Drop the
            // handle under the startup lock so a replacement cannot race it.
            state.capture.take();
        }
    }
}

fn encode_chunks(
    mut encoder: AudioEncoder,
    mut chunks: mpsc::Receiver<crate::platform::AudioChunk>,
    fanout: broadcast::Sender<EncodedAudio>,
) {
    let mut framer = Framer::new();
    let mut gate = SilenceGate::new();
    let mut output = [0; rs_audio::MAX_PACKET];
    let mut last_warning = None;
    while let Some(chunk) = chunks.blocking_recv() {
        timing::received(chunk.capture_ts_us);
        let frames = match framer.push(chunk.capture_ts_us, chunk.channels, &chunk.samples) {
            Ok(frames) => frames,
            Err(error) => {
                warn_audio(&mut last_warning, &error);
                continue;
            }
        };
        for frame in frames {
            let after_gap = match gate.push(frame.pcm.as_slice()) {
                Gate::Skip => continue,
                Gate::Resume => {
                    if let Err(error) = encoder.reset() {
                        warn_audio(&mut last_warning, &error);
                        continue;
                    }
                    true
                }
                Gate::Send => false,
            };
            match encoder.encode(frame.pcm.as_slice(), &mut output) {
                Ok(len) => {
                    let _ = fanout.send(EncodedAudio {
                        capture_ts_us: frame.capture_ts_us,
                        payload: Bytes::copy_from_slice(&output[..len]),
                        after_gap,
                    });
                }
                Err(error) => warn_audio(&mut last_warning, &error),
            }
        }
    }
}

fn warn_audio(last_warning: &mut Option<Instant>, error: &rs_audio::AudioError) {
    let now = Instant::now();
    if last_warning.is_none_or(|last| now.duration_since(last) >= Duration::from_secs(10)) {
        tracing::warn!(%error, "audio encoding failed; dropping frame");
        *last_warning = Some(now);
    }
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct AudioSendStats {
    pub sent: u64,
    pub dropped_full: u64,
    pub dropped_too_large: u64,
    pub lagged: u64,
}

struct SenderState {
    next_seq: u16,
    stats: AudioSendStats,
    stopped: bool,
}

/// Per-connection sequence numbers and nonblocking audio delivery.
pub struct AudioSender {
    task: JoinHandle<()>,
    state: Arc<Mutex<SenderState>>,
    lagged: Arc<AtomicU64>,
}

impl AudioSender {
    pub fn spawn(conn: Conn, mut sub: AudioSubscription, first_seq: u16) -> Self {
        let state = Arc::new(Mutex::new(SenderState {
            next_seq: first_seq,
            stats: AudioSendStats::default(),
            stopped: false,
        }));
        let task_state = Arc::clone(&state);
        let lagged = Arc::clone(&sub.lagged);
        let priority = super::backlog::AudioPriority::new(conn.stable_id());
        let task = tokio::spawn(async move {
            let _priority = priority;
            while let Some(frame) = sub.recv().await {
                let mut state = task_state.lock().unwrap_or_else(|e| e.into_inner());
                if state.stopped {
                    break;
                }
                let header = AudioHeader {
                    seq: state.next_seq,
                    capture_ts_us: frame.capture_ts_us,
                };
                state.next_seq = state.next_seq.wrapping_add(1);
                let datagram = match audio::encode(header, &frame.payload) {
                    Ok(datagram) => datagram,
                    Err(audio::AudioEncodeError::PayloadTooLarge(_)) => {
                        state.stats.dropped_too_large += 1;
                        continue;
                    }
                    Err(audio::AudioEncodeError::EmptyPayload) => continue,
                };
                timing::submitted(frame.capture_ts_us, &conn);
                match conn.send_datagram(DatagramKind::Audio, datagram.slice(1..)) {
                    Ok(()) => state.stats.sent += 1,
                    Err(ConnError::WouldBlock) => state.stats.dropped_full += 1,
                    Err(ConnError::SendDatagram(SendDatagramError::TooLarge)) => {
                        state.stats.dropped_too_large += 1;
                    }
                    Err(_) => break,
                }
            }
        });
        Self {
            task,
            state,
            lagged,
        }
    }

    /// Next sequence number to preserve when restarting this connection's audio.
    pub fn next_seq(&self) -> u16 {
        self.state
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .next_seq
    }

    pub fn stats(&self) -> AudioSendStats {
        let mut stats = self.state.lock().unwrap_or_else(|e| e.into_inner()).stats;
        stats.lagged = self.lagged.load(Ordering::Relaxed);
        stats
    }
}

impl Drop for AudioSender {
    fn drop(&mut self) {
        // Serialize with send_datagram so no new attempt starts after drop.
        self.state.lock().unwrap_or_else(|e| e.into_inner()).stopped = true;
        self.task.abort();
    }
}

/// Video encoder bitrate after reserving audio and accounting for video FEC.
pub fn video_encoder_kbps(d: &Decision, audio_active: bool) -> u32 {
    if !audio_active {
        return d.encoder_kbps;
    }
    let video_budget = d.target_kbps.saturating_sub(AUDIO_RESERVE_KBPS);
    let encoder = (video_budget as f32 / (1.0 + d.fec_ratio)) as u32;
    encoder.max(AUDIO_MIN_VIDEO_KBPS).min(d.encoder_kbps)
}

/// Bounded, opt-in timing recorder for in-process end-to-end tests. Timestamps
/// are Unix microseconds; no audio samples or payloads are retained.
#[doc(hidden)]
pub mod timing {
    use std::collections::BTreeMap;
    use std::sync::atomic::{AtomicBool, Ordering};
    use std::sync::Mutex;

    #[derive(Clone, Copy, Debug, Default)]
    pub struct Sample {
        pub capture_ts_us: u64,
        pub received_us: u64,
        pub submitted_us: u64,
        pub backlog_bytes: usize,
        pub rtt_us: u64,
    }

    static ENABLED: AtomicBool = AtomicBool::new(false);
    static SAMPLES: Mutex<BTreeMap<u64, Sample>> = Mutex::new(BTreeMap::new());

    pub struct Probe {
        _private: (),
    }

    impl Default for Probe {
        fn default() -> Self {
            SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).clear();
            ENABLED.store(true, Ordering::Relaxed);
            Self { _private: () }
        }
    }

    impl Probe {
        pub fn samples(&self) -> Vec<Sample> {
            SAMPLES
                .lock()
                .unwrap_or_else(|e| e.into_inner())
                .values()
                .copied()
                .collect()
        }
    }

    impl Drop for Probe {
        fn drop(&mut self) {
            ENABLED.store(false, Ordering::Relaxed);
            SAMPLES.lock().unwrap_or_else(|e| e.into_inner()).clear();
        }
    }

    pub(super) fn received(capture_ts_us: u64) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let received_us = rsp_core::clock::unix_micros();
        let mut samples = SAMPLES.lock().unwrap_or_else(|e| e.into_inner());
        if samples.len() == 2048 {
            samples.pop_first();
        }
        samples.insert(
            capture_ts_us,
            Sample {
                capture_ts_us,
                received_us,
                ..Sample::default()
            },
        );
    }

    pub(super) fn submitted(capture_ts_us: u64, conn: &rsp_transport::Conn) {
        if !ENABLED.load(Ordering::Relaxed) {
            return;
        }
        let submitted_us = rsp_core::clock::unix_micros();
        if let Some(sample) = SAMPLES
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .get_mut(&capture_ts_us)
        {
            sample.submitted_us = submitted_us;
            sample.backlog_bytes = conn.datagram_backlog_bytes();
            sample.rtt_us = conn.as_quic().map_or(0, |q| q.rtt().as_micros() as u64);
        }
    }
}
