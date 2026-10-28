use std::time::Duration;

use host_core::pipeline::backlog::{prefetch_while_waiting, AudioPriority, VideoPacer};
use tokio::time::{sleep, Instant};

const BURST: usize = 8_000;
const PACKET: usize = 1_000;
const WINDOW: u64 = 240_000;

#[test]
fn priority_is_per_connection_and_survives_overlapping_senders() {
    let first = AudioPriority::new("audio-priority-test".into());
    let replacement = AudioPriority::new("audio-priority-test".into());
    assert!(AudioPriority::active("audio-priority-test"));
    assert!(!AudioPriority::active("video-only-test"));
    drop(first);
    assert!(AudioPriority::active("audio-priority-test"));
    drop(replacement);
    assert!(!AudioPriority::active("audio-priority-test"));
}

async fn burst(pacer: &mut VideoPacer, rtt: Duration, min_rtt: Duration) {
    for _ in 0..BURST / PACKET {
        pacer.wait(PACKET, BURST, rtt, min_rtt, WINDOW).await;
    }
}

#[tokio::test(start_paused = true)]
async fn inflated_rtt_paces_video_even_when_the_local_queue_is_empty() {
    let mut pacer = VideoPacer::default();
    let rtt = Duration::from_millis(200);
    let start = Instant::now();
    burst(&mut pacer, rtt, Duration::ZERO).await;
    assert_eq!(
        start.elapsed(),
        Duration::ZERO,
        "one small burst is allowed"
    );
    burst(&mut pacer, rtt, Duration::ZERO).await;
    // 8 KB / (240 KB / (4 * 200 ms)) = 26.67 ms. A watermark
    // alone would allow both bursts immediately into the bottleneck FIFO.
    assert!(start.elapsed() >= Duration::from_millis(26));
    assert!(start.elapsed() <= Duration::from_millis(28));
}

#[tokio::test(start_paused = true)]
async fn propagation_rtt_does_not_get_the_queueing_penalty() {
    let mut pacer = VideoPacer::default();
    let rtt = Duration::from_millis(200);
    let start = Instant::now();
    burst(&mut pacer, rtt, rtt).await;
    burst(&mut pacer, rtt, rtt).await;
    assert!(start.elapsed() >= Duration::from_millis(6));
    assert!(start.elapsed() <= Duration::from_millis(8));
}

#[tokio::test(start_paused = true)]
async fn idle_time_cannot_accumulate_an_unbounded_keyframe_burst() {
    let mut pacer = VideoPacer::default();
    let rtt = Duration::from_millis(200);
    burst(&mut pacer, rtt, Duration::ZERO).await;
    sleep(Duration::from_secs(2)).await;
    let resumed = Instant::now();
    burst(&mut pacer, rtt, Duration::ZERO).await;
    assert!(resumed.elapsed() < Duration::from_millis(2));
    burst(&mut pacer, rtt, Duration::ZERO).await;
    assert!(resumed.elapsed() >= Duration::from_millis(26));
}

#[tokio::test(start_paused = true)]
async fn audio_ticks_continue_while_a_whole_keyframe_is_paced() {
    let start = Instant::now();
    let video = async {
        let mut pacer = VideoPacer::default();
        for _ in 0..30 {
            burst(&mut pacer, Duration::from_millis(200), Duration::ZERO).await;
        }
    };
    let audio = async {
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        for i in 0..60 {
            tick.tick().await;
            assert!(
                start.elapsed().abs_diff(Duration::from_millis(i * 10)) < Duration::from_millis(2)
            );
        }
    };
    tokio::join!(video, audio);
}

#[tokio::test(start_paused = true)]
async fn pacing_prefetch_is_bounded_and_keeps_every_frame_in_order() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    let encoder = tokio::spawn(async move {
        for frame in 0..12 {
            tx.send(frame).await.unwrap();
        }
    });
    let mut pending = std::collections::VecDeque::new();
    prefetch_while_waiting(sleep(Duration::from_millis(50)), &mut rx, &mut pending).await;
    assert_eq!(pending.len(), 4);
    assert_eq!(rx.len(), 4);
    assert!(!encoder.is_finished(), "extra capacity must stay bounded");
    let mut frames: Vec<_> = pending.drain(..).collect();
    while let Some(frame) = rx.recv().await {
        frames.push(frame);
    }
    encoder.await.unwrap();
    assert_eq!(frames, (0..12).collect::<Vec<_>>());
}

#[tokio::test(start_paused = true)]
async fn closing_the_encoder_does_not_cancel_the_pacing_deadline() {
    let (tx, mut rx) = tokio::sync::mpsc::channel(4);
    tx.send(1).await.unwrap();
    drop(tx);
    let mut pending = std::collections::VecDeque::new();
    let start = Instant::now();
    prefetch_while_waiting(sleep(Duration::from_millis(50)), &mut rx, &mut pending).await;
    assert_eq!(start.elapsed(), Duration::from_millis(50));
    assert_eq!(pending.pop_front(), Some(1));
}
