use std::{cell::Cell, future::Future, time::Duration};

use host_core::pipeline::backlog::{
    set_watermark_bytes, wait_for_room, watermark_bytes, Backlog, Room, VIDEO_BACKLOG_MAX_WAIT,
    VIDEO_BACKLOG_POLL, VIDEO_BACKLOG_WATERMARK_BYTES,
};
use tokio::time::{sleep, Instant};

const VIDEO_PACKET_BYTES: usize = 1_200;

struct FakeBacklog {
    bytes: Cell<usize>,
}

impl FakeBacklog {
    fn new(bytes: usize) -> Self {
        Self {
            bytes: Cell::new(bytes),
        }
    }

    fn enqueue(&self, bytes: usize) {
        self.bytes.set(self.bytes.get() + bytes);
    }

    async fn while_draining<F: Future>(&self, bytes_per_ms: usize, work: F) -> F::Output {
        let drain = async {
            loop {
                sleep(VIDEO_BACKLOG_POLL).await;
                self.bytes
                    .set(self.bytes.get().saturating_sub(bytes_per_ms));
            }
        };
        tokio::select! {
            // Drain before polling producers when both timers fire on the same tick.
            biased;
            () = drain => unreachable!(),
            result = work => result,
        }
    }
}

impl Backlog for FakeBacklog {
    fn backlog_bytes(&self) -> usize {
        self.bytes.get()
    }
}

async fn send_video(queue: &FakeBacklog, mut bytes: usize, watermark: usize) {
    while bytes > 0 {
        if watermark > 0 {
            assert_ne!(
                wait_for_room(queue, watermark, VIDEO_BACKLOG_MAX_WAIT).await,
                Room::TimedOut
            );
        }
        let len = bytes.min(VIDEO_PACKET_BYTES);
        queue.enqueue(len);
        bytes -= len;
    }
}

#[tokio::test(start_paused = true)]
async fn low_backlog_is_ready_without_sleeping() {
    let start = Instant::now();
    for bytes in [4 * 1024, VIDEO_BACKLOG_WATERMARK_BYTES] {
        let queue = FakeBacklog::new(bytes);
        assert_eq!(
            wait_for_room(
                &queue,
                VIDEO_BACKLOG_WATERMARK_BYTES,
                VIDEO_BACKLOG_MAX_WAIT
            )
            .await,
            Room::Ready
        );
    }
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn draining_backlog_is_ready_after_six_polls() {
    let queue = FakeBacklog::new(20 * 1024);
    let room = queue
        .while_draining(
            2 * 1024,
            wait_for_room(
                &queue,
                VIDEO_BACKLOG_WATERMARK_BYTES,
                VIDEO_BACKLOG_MAX_WAIT,
            ),
        )
        .await;
    assert_eq!(room, Room::Waited(Duration::from_millis(6)));
    assert_eq!(queue.backlog_bytes(), VIDEO_BACKLOG_WATERMARK_BYTES);
}

#[tokio::test(start_paused = true)]
async fn stalled_backlog_times_out_after_fifty_ms() {
    let queue = FakeBacklog::new(100 * 1024);
    let start = Instant::now();
    assert_eq!(
        wait_for_room(
            &queue,
            VIDEO_BACKLOG_WATERMARK_BYTES,
            VIDEO_BACKLOG_MAX_WAIT
        )
        .await,
        Room::TimedOut
    );
    assert_eq!(start.elapsed(), Duration::from_millis(50));
    assert_eq!(queue.backlog_bytes(), 100 * 1024);
}

#[tokio::test(start_paused = true)]
async fn watermark_override_can_disable_and_restore_waiting() {
    assert_eq!(watermark_bytes(), VIDEO_BACKLOG_WATERMARK_BYTES);
    set_watermark_bytes(0);
    let disabled = watermark_bytes();
    let queue = FakeBacklog::new(100 * 1024);
    let start = Instant::now();
    send_video(&queue, VIDEO_PACKET_BYTES, disabled).await;
    set_watermark_bytes(VIDEO_BACKLOG_WATERMARK_BYTES);

    assert_eq!(disabled, 0);
    assert_eq!(start.elapsed(), Duration::ZERO);
    assert_eq!(queue.backlog_bytes(), 100 * 1024 + VIDEO_PACKET_BYTES);
    assert_eq!(watermark_bytes(), 8192);
}

#[tokio::test(start_paused = true)]
async fn watermark_preserves_twenty_mbit_throughput() {
    let queue = FakeBacklog::new(0);
    let start = Instant::now();
    queue
        .while_draining(2_500, async {
            send_video(&queue, 1_000_000, VIDEO_BACKLOG_WATERMARK_BYTES).await;
            assert!(queue.backlog_bytes() <= VIDEO_BACKLOG_WATERMARK_BYTES + VIDEO_PACKET_BYTES);
            // Include draining the final packets, not just accepting them into memory.
            assert_ne!(
                wait_for_room(&queue, 0, VIDEO_BACKLOG_MAX_WAIT).await,
                Room::TimedOut
            );
        })
        .await;
    let elapsed = start.elapsed();
    println!("1 MB at 20 Mbit/s: {elapsed:?}");
    assert!(elapsed >= Duration::from_millis(400));
    assert!(elapsed <= Duration::from_millis(420));
    assert_eq!(queue.backlog_bytes(), 0);
}

async fn audio_backlogs_during_video_burst(watermark: usize) -> Vec<usize> {
    let queue = FakeBacklog::new(0);
    queue
        .while_draining(2_500, async {
            let audio = async {
                let mut backlogs = Vec::new();
                for _ in 0..13 {
                    sleep(Duration::from_millis(10)).await;
                    backlogs.push(queue.backlog_bytes());
                    // Audio always enters the same FIFO immediately, without a watermark wait.
                    queue.enqueue(120);
                }
                backlogs
            };
            let ((), backlogs) = tokio::join!(send_video(&queue, 300_000, watermark), audio);
            backlogs
        })
        .await
}

#[tokio::test(start_paused = true)]
async fn watermark_keeps_audio_ahead_of_a_video_burst() {
    let enabled = audio_backlogs_during_video_burst(VIDEO_BACKLOG_WATERMARK_BYTES).await;
    let disabled = audio_backlogs_during_video_burst(0).await;
    println!("audio backlog bytes: enabled={enabled:?}, disabled={disabled:?}");
    assert_eq!(enabled.len(), 13);
    assert!(enabled
        .iter()
        .all(|&bytes| bytes <= VIDEO_BACKLOG_WATERMARK_BYTES + VIDEO_PACKET_BYTES));
    assert!(disabled.iter().any(|&bytes| bytes > 200 * 1024));
}

#[tokio::test(start_paused = true)]
async fn runnable_transport_drains_before_arming_a_poll_timer() {
    let queue = FakeBacklog::new(20 * 1024);
    let start = Instant::now();
    let (room, ()) = tokio::join!(
        wait_for_room(
            &queue,
            VIDEO_BACKLOG_WATERMARK_BYTES,
            VIDEO_BACKLOG_MAX_WAIT
        ),
        async {
            queue.bytes.set(0);
        },
    );
    assert_eq!(room, Room::Waited(Duration::ZERO));
    assert_eq!(start.elapsed(), Duration::ZERO);
}

#[tokio::test(start_paused = true)]
async fn source_drops_leave_callback_headroom_without_discarding_references() {
    use host_core::pipeline::backlog::source_ready;
    let (output, mut rx) = tokio::sync::mpsc::channel(4);
    let now = 1_000_000;
    let mut encoded = 0;
    for capture in [now, now + 33_333, now + 66_666, now + 99_999] {
        if source_ready(output.max_capacity() - output.capacity(), capture, capture) {
            encoded += 1;
            output.try_send(encoded).unwrap();
        }
    }
    assert_eq!(encoded, 1);
    assert_eq!(output.capacity(), 3, "native callbacks retain headroom");
    assert_eq!(rx.recv().await, Some(1), "keep the encoded reference");
    assert!(source_ready(0, now + 133_332, now + 133_332));
    output.try_send(2).unwrap();
    assert_eq!(
        rx.recv().await,
        Some(2),
        "the next delta keeps its reference"
    );
}

#[test]
fn stale_source_frames_are_dropped_before_encoding() {
    use host_core::pipeline::backlog::{source_ready, SOURCE_MAX_AGE_US};
    let capture = 1_000_000;
    assert!(source_ready(0, capture, capture + SOURCE_MAX_AGE_US));
    assert!(!source_ready(0, capture, capture + SOURCE_MAX_AGE_US + 1));
    assert!(!source_ready(1, capture, capture));
    assert!(
        source_ready(0, capture + 1, capture),
        "clock rollback saturates"
    );
}
