#![cfg(target_os = "macos")]

use std::env;
use std::time::{Duration, Instant};

use host_core::platform::{AudioCaptureConfig, Platform, PlatformError};
use host_mac::MacPlatform;
use tokio::sync::mpsc;

fn require_sck() -> bool {
    if env::var_os("RS_REQUIRE_SCK").is_some_and(|value| value == "1") {
        true
    } else {
        eprintln!("SKIP sck-audio: set RS_REQUIRE_SCK=1 to require ScreenCaptureKit");
        false
    }
}

#[tokio::test]
async fn screen_capture_kit_audio_stream_is_monotonic_and_releases_sink() {
    if !require_sck() {
        return;
    }
    let platform = MacPlatform::new();
    assert!(
        platform.audio_capture_supported(),
        "RS_REQUIRE_SCK=1 but audio is unavailable"
    );
    let (tx, mut rx) = mpsc::channel(256);
    let handle = platform
        .capture_audio(
            AudioCaptureConfig {
                sample_rate: 48_000,
                channels: 2,
                exclude_self: true,
            },
            tx,
        )
        .unwrap_or_else(|error| panic!("RS_REQUIRE_SCK=1: audio start failed: {error}"));
    // Start the measurement at the first block, so startup and TCC latency
    // cannot shorten the two-second PCM window.
    let first = tokio::time::timeout(Duration::from_secs(5), rx.recv())
        .await
        .expect("no first audio chunk within five seconds")
        .expect("audio sink closed before the first chunk");
    let mut chunks = vec![first];
    let deadline = tokio::time::Instant::now() + Duration::from_secs(2);
    while tokio::time::Instant::now() < deadline {
        let remaining = deadline.saturating_duration_since(tokio::time::Instant::now());
        match tokio::time::timeout(remaining.min(Duration::from_millis(250)), rx.recv()).await {
            Ok(Some(chunk)) => chunks.push(chunk),
            Ok(None) => panic!("audio sink closed before capture deadline"),
            Err(_) => {}
        }
    }
    let stopping = Instant::now();
    drop(handle);
    let remaining = Duration::from_millis(200)
        .checked_sub(stopping.elapsed())
        .expect("dropping the audio handle took more than 200 ms");
    tokio::time::timeout(remaining, async {
        while let Some(chunk) = rx.recv().await {
            chunks.push(chunk);
        }
    })
    .await
    .expect("audio sink did not close within 200 ms of dropping the handle");
    assert!(chunks.len() >= 50, "only received {} chunks", chunks.len());
    let frames: u64 = chunks.iter().map(|chunk| u64::from(chunk.frames)).sum();
    assert!(
        (91_200..=100_800).contains(&frames),
        "unexpected frame count: {frames}"
    );
    assert!(chunks.iter().all(|chunk| {
        chunk.channels == 2
            && chunk.samples.len() == chunk.frames as usize * 2
            && chunk
                .samples
                .iter()
                .all(|sample| sample.is_finite() && (-1.0..=1.0).contains(sample))
    }));
    assert!(chunks
        .windows(2)
        .all(|pair| pair[0].capture_ts_us < pair[1].capture_ts_us));
    assert!(chunks.windows(2).all(|pair| {
        let expected = u64::from(pair[0].frames) * 1_000_000 / 48_000;
        pair[1]
            .capture_ts_us
            .saturating_sub(pair[0].capture_ts_us)
            .abs_diff(expected)
            <= 5_000
    }));
}

#[test]
fn unsupported_audio_configuration_is_rejected() {
    let platform = MacPlatform::new();
    let (tx, _rx) = mpsc::channel(1);
    let error = match platform.capture_audio(
        AudioCaptureConfig {
            sample_rate: 44_100,
            channels: 2,
            exclude_self: true,
        },
        tx,
    ) {
        Ok(_) => panic!("44.1 kHz audio must be unsupported"),
        Err(error) => error,
    };
    assert!(matches!(error, PlatformError::Unsupported));
}
