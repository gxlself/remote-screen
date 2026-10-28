use std::time::Duration;

use host_core::platform::{AudioCaptureConfig, AudioChunk, Platform, PlatformError};
use host_fake::audio::FakeAudioFailure;
use host_fake::FakePlatform;
use rs_audio::gen::{sample_index, Signal};
use tokio::sync::mpsc;
use tokio::time::{timeout, Instant};

const CHUNK_FRAMES: usize = 1024;

fn config(channels: u8) -> AudioCaptureConfig {
    AudioCaptureConfig {
        sample_rate: 48_000,
        channels,
        exclude_self: true,
    }
}

async fn next_chunk(rx: &mut mpsc::Receiver<AudioChunk>) -> AudioChunk {
    timeout(Duration::from_secs(1), rx.recv())
        .await
        .expect("audio chunk within one second")
        .expect("audio sink remains open")
}

async fn wait_until_active(audio: &host_fake::audio::FakeAudio, active: usize) {
    timeout(Duration::from_millis(100), async {
        while audio.active() != active {
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    })
    .await
    .expect("capture activity settles within 100 ms");
}

#[tokio::test]
async fn support_can_be_turned_off() {
    let platform = FakePlatform::new();
    let audio = platform.audio();
    assert!(platform.audio_capture_supported());

    audio.set_supported(false);
    assert!(!platform.audio_capture_supported());
    let (sink, _rx) = mpsc::channel(1);
    match platform.capture_audio(config(2), sink) {
        Err(PlatformError::Unsupported) => {}
        Err(error) => panic!("expected Unsupported, got {error}"),
        Ok(_) => panic!("unsupported platform started capture"),
    }
    assert_eq!(audio.starts(), 0);
}

#[tokio::test]
async fn one_second_has_continuous_48khz_stereo_chunks() {
    let platform = FakePlatform::new();
    let audio = platform.audio();
    let cfg = config(2);
    let (sink, mut rx) = mpsc::channel(64);
    let handle = platform
        .capture_audio(cfg, sink)
        .expect("start fake audio capture");

    assert_eq!(audio.starts(), 1);
    assert_eq!(audio.active(), 1);
    assert_eq!(audio.last_config(), Some(cfg));
    tokio::time::sleep(Duration::from_secs(1)).await;

    let mut chunks = Vec::new();
    while let Ok(chunk) = rx.try_recv() {
        chunks.push(chunk);
    }
    drop(handle);

    assert!(!chunks.is_empty());
    assert_eq!(audio.starts(), 1);
    assert_eq!(audio.last_config(), Some(cfg));
    let first_index = sample_index(chunks[0].capture_ts_us);
    let mut frames = 0u64;
    for (block, chunk) in chunks.iter().enumerate() {
        assert_eq!(chunk.frames, CHUNK_FRAMES as u32);
        assert_eq!(chunk.channels, 2);
        assert_eq!(chunk.samples.len(), CHUNK_FRAMES * 2);
        assert_eq!(
            sample_index(chunk.capture_ts_us),
            first_index + block as u64 * CHUNK_FRAMES as u64
        );
        frames += u64::from(chunk.frames);
    }

    let tolerance = 48_000u64 * 3 / 100;
    assert!(
        frames.abs_diff(48_000) <= tolerance,
        "captured {frames} frames in one second"
    );
}

#[tokio::test]
async fn samples_match_the_selected_signal() {
    let platform = FakePlatform::new();
    let signal = Signal::Noise { amp: 0.3, seed: 7 };
    platform.audio().set_signal(signal);
    let (sink, mut rx) = mpsc::channel(4);
    let handle = platform
        .capture_audio(config(2), sink)
        .expect("start fake audio capture");

    let chunk = next_chunk(&mut rx).await;
    let mut expected = vec![0.0; chunk.samples.len()];
    signal.render(chunk.capture_ts_us, 2, &mut expected);
    assert_eq!(chunk.samples, expected);
    drop(handle);
}

#[tokio::test]
async fn channel_and_sample_rate_validation() {
    let platform = FakePlatform::new();
    let (sink, mut rx) = mpsc::channel(4);
    let handle = platform
        .capture_audio(config(1), sink)
        .expect("start mono fake audio capture");
    let chunk = next_chunk(&mut rx).await;
    assert_eq!(chunk.channels, 1);
    assert_eq!(chunk.frames, CHUNK_FRAMES as u32);
    assert_eq!(chunk.samples.len(), CHUNK_FRAMES);
    drop(handle);

    let (sink, _rx) = mpsc::channel(1);
    match platform.capture_audio(config(3), sink) {
        Err(PlatformError::Platform(message)) => assert_eq!(message, "channels"),
        Err(error) => panic!("expected Platform(channels), got {error}"),
        Ok(_) => panic!("three-channel capture unexpectedly started"),
    }

    let (sink, _rx) = mpsc::channel(1);
    let mut unsupported = config(2);
    unsupported.sample_rate = 44_100;
    match platform.capture_audio(unsupported, sink) {
        Err(PlatformError::Unsupported) => {}
        Err(error) => panic!("expected Unsupported, got {error}"),
        Ok(_) => panic!("44.1 kHz capture unexpectedly started"),
    }
}

#[tokio::test]
async fn dropping_handle_stops_capture_and_closes_sink() {
    let platform = FakePlatform::new();
    let audio = platform.audio();
    let (sink, mut rx) = mpsc::channel(4);
    let handle = platform
        .capture_audio(config(2), sink)
        .expect("start fake audio capture");
    assert_eq!(audio.active(), 1);

    drop(handle);
    wait_until_active(&audio, 0).await;
    timeout(Duration::from_millis(100), async {
        while rx.recv().await.is_some() {}
    })
    .await
    .expect("sink closes within 100 ms");
}

#[tokio::test]
async fn fail_next_returns_one_error_then_recovers() {
    let platform = FakePlatform::new();
    let audio = platform.audio();

    for failure in [
        FakeAudioFailure::PermissionDenied,
        FakeAudioFailure::Platform,
    ] {
        audio.fail_next(failure);
        let (sink, _rx) = mpsc::channel(1);
        match platform.capture_audio(config(2), sink) {
            Err(PlatformError::PermissionDenied(_))
                if failure == FakeAudioFailure::PermissionDenied => {}
            Err(PlatformError::Platform(_)) if failure == FakeAudioFailure::Platform => {}
            Err(error) => panic!("unexpected injected error: {error}"),
            Ok(_) => panic!("injected failure unexpectedly started capture"),
        }
    }
    assert_eq!(audio.starts(), 0);

    let (sink, mut rx) = mpsc::channel(4);
    let handle = platform
        .capture_audio(config(2), sink)
        .expect("capture recovers after one failure");
    assert_eq!(audio.starts(), 1);
    next_chunk(&mut rx).await;
    drop(handle);
}

#[tokio::test]
async fn signal_switch_applies_to_following_chunks() {
    let platform = FakePlatform::new();
    let audio = platform.audio();
    audio.set_chunk_frames(480);
    let (sink, mut rx) = mpsc::channel(8);
    let handle = platform
        .capture_audio(config(2), sink)
        .expect("start fake audio capture");

    let first = next_chunk(&mut rx).await;
    assert!(first.samples.iter().any(|sample| *sample != 0.0));
    audio.set_signal(Signal::Silence);

    let mut silent_blocks = 0;
    while silent_blocks < 3 {
        let chunk = next_chunk(&mut rx).await;
        if chunk.samples.iter().all(|sample| *sample == 0.0) {
            silent_blocks += 1;
        } else {
            assert_eq!(
                silent_blocks, 0,
                "signal switched back to a non-silent block"
            );
        }
    }
    drop(handle);
}

#[tokio::test]
async fn full_sink_drops_without_blocking_capture() {
    let platform = FakePlatform::new();
    let audio = platform.audio();
    let (sink, _rx) = mpsc::channel(1);
    let handle = platform
        .capture_audio(config(2), sink)
        .expect("start fake audio capture");

    timeout(Duration::from_secs(1), async {
        while audio.dropped_chunks() == 0 {
            tokio::time::sleep(Duration::from_millis(2)).await;
        }
    })
    .await
    .expect("full sink eventually drops a chunk");
    let dropped = audio.dropped_chunks();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(audio.dropped_chunks() > dropped);
    assert_eq!(audio.active(), 1);

    let start = Instant::now();
    drop(handle);
    wait_until_active(&audio, 0).await;
    assert!(start.elapsed() <= Duration::from_millis(100));
}

#[tokio::test]
async fn paused_video_capture_does_not_pause_audio() {
    let platform = FakePlatform::new();
    platform.set_capture_paused(true);
    let (sink, mut rx) = mpsc::channel(4);
    let handle = platform
        .capture_audio(config(2), sink)
        .expect("start fake audio capture");

    let chunk = next_chunk(&mut rx).await;
    assert_eq!(chunk.frames, CHUNK_FRAMES as u32);
    drop(handle);
}

#[test]
fn capture_audio_without_runtime_is_a_platform_error() {
    let platform = FakePlatform::new();
    let (sink, _rx) = mpsc::channel(1);
    match platform.capture_audio(config(2), sink) {
        Err(PlatformError::Platform(_)) => {}
        Err(error) => panic!("expected Platform error without runtime, got {error}"),
        Ok(_) => panic!("capture started without a Tokio runtime"),
    }
}
