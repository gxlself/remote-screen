//! Bound queued QUIC video so audio can enter the send queue promptly.
//!
//! [`set_watermark_bytes`] is a process-wide test override for e2e comparisons.
//! Production code must not call it; zero disables waiting in the packetizer.

use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use tokio::time::{sleep, Instant};

pub const VIDEO_BACKLOG_WATERMARK_BYTES: usize = 8 * 1024;
pub const VIDEO_BACKLOG_MAX_WAIT: Duration = Duration::from_millis(50);
pub const VIDEO_BACKLOG_POLL: Duration = Duration::from_millis(1);

static WATERMARK_BYTES: AtomicUsize = AtomicUsize::new(VIDEO_BACKLOG_WATERMARK_BYTES);

pub trait Backlog {
    fn backlog_bytes(&self) -> usize;
}

impl Backlog for rsp_transport::Conn {
    fn backlog_bytes(&self) -> usize {
        self.datagram_backlog_bytes()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Room {
    Ready,
    Waited(Duration),
    TimedOut,
}

/// Return immediately at or below the watermark, otherwise poll until room or timeout.
/// A timeout lets the caller send anyway so a stalled queue cannot block forever.
pub async fn wait_for_room(b: &impl Backlog, watermark: usize, max_wait: Duration) -> Room {
    if b.backlog_bytes() <= watermark {
        return Room::Ready;
    }
    let start = Instant::now();
    // Quinn's connection driver often only needs a turn to drain this batch.
    // A timer for every 8 KiB amplifies scheduler latency into frame latency.
    tokio::task::yield_now().await;
    if b.backlog_bytes() <= watermark {
        return Room::Waited(start.elapsed());
    }
    loop {
        let elapsed = start.elapsed();
        if elapsed >= max_wait {
            return Room::TimedOut;
        }
        sleep(VIDEO_BACKLOG_POLL.min(max_wait - elapsed)).await;
        if b.backlog_bytes() <= watermark {
            return Room::Waited(start.elapsed());
        }
    }
}

/// Override the process-wide watermark for tests; zero disables waiting.
pub fn set_watermark_bytes(bytes: usize) {
    WATERMARK_BYTES.store(bytes, Ordering::Relaxed);
}

pub fn watermark_bytes() -> usize {
    WATERMARK_BYTES.load(Ordering::Relaxed)
}

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct BacklogStats {
    /// All waits, including those that timed out.
    pub waits: u64,
    pub timeouts: u64,
    /// Completed wait durations plus the wait budget for each timeout.
    pub waited_us: u64,
}

// Audio priority is scoped to a connection, including overlapping sender tasks
// during a restart. Video-only streams keep their normal transport pacing.
static AUDIO_CONNECTIONS: std::sync::Mutex<std::collections::BTreeMap<String, usize>> =
    std::sync::Mutex::new(std::collections::BTreeMap::new());

/// Lifetime guard for an audio sender on one transport connection.
pub struct AudioPriority(String);

impl AudioPriority {
    pub fn new(id: String) -> Self {
        *AUDIO_CONNECTIONS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .entry(id.clone())
            .or_default() += 1;
        Self(id)
    }

    pub fn active(id: &str) -> bool {
        AUDIO_CONNECTIONS
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .contains_key(id)
    }
}

impl Drop for AudioPriority {
    fn drop(&mut self) {
        let mut connections = AUDIO_CONNECTIONS.lock().unwrap_or_else(|e| e.into_inner());
        if let Some(count) = connections.get_mut(&self.0) {
            *count -= 1;
            if *count == 0 {
                connections.remove(&self.0);
            }
        }
    }
}

/// Pace video in small batches, leaving audio free to enter the transport.
/// A queue watermark alone cannot see bytes already in flight: a large QUIC
/// window can move a whole keyframe into the bottleneck's FIFO immediately.
#[derive(Default)]
pub struct VideoPacer {
    bytes: usize,
    next: Option<Instant>,
}

impl VideoPacer {
    pub async fn wait(
        &mut self,
        bytes: usize,
        burst_bytes: usize,
        rtt: Duration,
        min_rtt: Duration,
        cwnd: u64,
    ) {
        if self.bytes >= burst_bytes {
            if let Some(next) = self.next {
                tokio::time::sleep_until(next).await;
            }
            self.bytes = 0;
            self.next = None;
        }
        // Preserve the propagation flight, but drain an inflated RTT faster
        // than QUIC's loss-based controller. This is send pacing, not a change
        // to the encoder bitrate or the transport congestion window. At high
        // queueing delay, use one quarter of cwnd/RTT; on a path with no RTT
        // inflation keep cwnd/RTT, so propagation delay is not penalized.
        let interval = rtt + rtt.saturating_sub(min_rtt) * 3;
        let cost = interval.mul_f64(bytes as f64 / cwnd.max(1) as f64);
        let now = Instant::now();
        self.next = Some(self.next.unwrap_or(now).max(now) + cost);
        self.bytes += bytes;
    }
}

/// Keep accepting encoded frames while pacing a keyframe. This moves one
/// bounded encoder-queue worth of video out of the shared transport FIFO,
/// rather than blocking the encoder and provoking replacement keyframes.
/// No frames are discarded, reordered, or accepted beyond the extra capacity.
pub async fn prefetch_while_waiting<T, F: std::future::Future>(
    wait: F,
    rx: &mut tokio::sync::mpsc::Receiver<T>,
    pending: &mut std::collections::VecDeque<T>,
) -> F::Output {
    tokio::pin!(wait);
    loop {
        tokio::select! {
            biased;
            result = &mut wait => return result,
            item = rx.recv(), if pending.len() < rx.max_capacity() => {
                match item {
                    Some(item) => pending.push_back(item),
                    None => return wait.await,
                }
            }
        }
    }
}

/// Source frames older than this are discarded before they enter the codec.
/// Still refinements deliberately retain the original capture time and bypass
/// this age check. Encoded references are never discarded by this admission rule.
pub const SOURCE_MAX_AGE_US: u64 = 150_000;

/// Keep one waiting encoded frame, leaving the other channel slots available
/// for asynchronous native callbacks. Source drops do not break the GOP.
pub fn source_ready(queued_frames: usize, capture_us: u64, now_us: u64) -> bool {
    queued_frames == 0 && now_us.saturating_sub(capture_us) <= SOURCE_MAX_AGE_US
}
