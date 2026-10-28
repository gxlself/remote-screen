use rsp_core::{
    errors::ErrorCode,
    features, pb,
    video::{VideoHeader, FLAG_KEYFRAME, FLAG_STILL},
};
use std::time::Duration;
use tokio::time::{sleep, timeout, timeout_at, Instant};

mod wire_support;
use wire_support::{client_hello, WireClient, WireHost};

fn start() -> pb::StartStream {
    pb::StartStream {
        max_width: 640,
        fps: 60,
        bitrate_kbps: 500,
        ..Default::default()
    }
}

fn stable_start() -> pb::StartStream {
    // LAN clamps 500 kbps to its 4 Mbps floor. After two seconds at that
    // floor, rate control rebuilds the pipeline and emits an unrelated IDR.
    pb::StartStream {
        bitrate_kbps: 5_000,
        ..start()
    }
}

// Consume every shard, including parity, so quiet checks never see a tail of
// the previous frame. Wire tests use reliable loopback without a loss proxy.
async fn frame(client: &WireClient) -> VideoHeader {
    loop {
        let bytes = client.recv_datagram(0x10).await;
        let (header, _) = VideoHeader::parse(&bytes).unwrap();
        if header.shard_idx + 1 == header.data_shards + header.parity_shards {
            return header;
        }
    }
}

async fn warmup(client: &WireClient) -> VideoHeader {
    let mut last = frame(client).await;
    assert_eq!(last.flags, FLAG_KEYFRAME);
    for _ in 0..3 {
        last = frame(client).await;
        assert_eq!(last.flags, 0);
    }
    last
}

async fn refinement(client: &WireClient, mut last_capture: u64) -> (VideoHeader, Instant) {
    timeout(Duration::from_secs(1), async {
        loop {
            let header = frame(client).await;
            if header.flags & FLAG_STILL != 0 {
                assert_eq!(header.flags, FLAG_STILL, "refinement must be a P frame");
                assert_eq!(header.capture_ts_us, last_capture);
                return (header, Instant::now());
            }
            last_capture = header.capture_ts_us;
        }
    })
    .await
    .expect("missing still refinement within one second")
}

struct FrozenTime(tokio::task::JoinHandle<()>);

impl FrozenTime {
    fn new() -> Self {
        tokio::time::pause();
        Self(tokio::spawn(async {
            // Keep UDP I/O from advancing paused time automatically. Retain a
            // real one-second bound if the expected frame never arrives.
            let started = std::time::Instant::now();
            while started.elapsed() < Duration::from_secs(1) {
                tokio::task::yield_now().await;
            }
        }))
    }
}

impl Drop for FrozenTime {
    fn drop(&mut self) {
        self.0.abort();
        tokio::time::resume();
    }
}

async fn first_refinement(
    client: &WireClient,
    last_capture: u64,
    observation_delay: Duration,
) -> (VideoHeader, Instant) {
    let clock = FrozenTime::new();
    // Let the encoder consume frames already captured before capture paused.
    tokio::task::yield_now().await;
    // Cross the idle deadline including Tokio's millisecond timer rounding.
    tokio::time::advance(Duration::from_millis(301)).await;
    let first = refinement(client, last_capture).await;
    assert!(
        !clock.0.is_finished(),
        "first refinement exceeded one second"
    );
    // Encoding and receipt share this frozen instant. The caller may observe
    // the result later without moving the server's cooldown origin forward.
    tokio::time::advance(observation_delay).await;
    drop(clock);
    first
}

async fn no_video(client: &WireClient, duration: Duration) {
    assert!(
        timeout(duration, client.recv_datagram(0x10)).await.is_err(),
        "unexpected video datagram while the screen is unchanged"
    );
}

async fn observe(client: &WireClient, duration: Duration) -> Vec<VideoHeader> {
    let end = Instant::now() + duration;
    let mut frames = Vec::new();
    while let Ok(header) = timeout_at(end, frame(client)).await {
        frames.push(header);
    }
    frames
}

#[tokio::test]
async fn still_screen_gets_one_p_frame_with_original_timestamp_then_no_video() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let mut hello = client_hello();
    hello.required_features |= features::HQ_REFRESH;
    let (server, _) = client.hello_with(hello).await;
    assert_ne!(server.features & features::HQ_REFRESH, 0);
    client.start_stream(start()).await;
    let last = warmup(&client).await;

    host.platform.set_capture_paused(true);
    refinement(&client, last.capture_ts_us).await;
    // Includes the remainder of the first second: a second refinement, normal
    // frame or stray parity shard would all fail this check.
    no_video(&client, Duration::from_secs(2)).await;
}

#[tokio::test]
async fn renewed_motion_keeps_the_one_second_refinement_cooldown() {
    renewed_motion_cooldown(Duration::ZERO).await;
}

#[tokio::test]
async fn delayed_first_refinement_observation_keeps_the_server_cooldown() {
    renewed_motion_cooldown(Duration::from_millis(100)).await;
}

async fn renewed_motion_cooldown(observation_delay: Duration) {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(stable_start()).await;
    let last = warmup(&client).await;
    host.platform.set_capture_paused(true);
    let (_, first_at) = first_refinement(&client, last.capture_ts_us, observation_delay).await;
    assert!(first_at.elapsed() >= observation_delay);

    host.platform.set_capture_paused(false);
    sleep(Duration::from_millis(100)).await;
    // A timer waking the test does not prove the capture task ran as well.
    let moving = frame(&client).await;
    assert!(moving.capture_ts_us > last.capture_ts_us);
    host.platform.set_capture_paused(true);
    let (second, second_at) = refinement(&client, moving.capture_ts_us).await;
    assert!(second.capture_ts_us > last.capture_ts_us);
    assert!(second_at.duration_since(first_at) >= Duration::from_secs(1));
    no_video(&client, Duration::from_secs(1)).await;
}

#[tokio::test]
async fn still_refinement_requires_the_client_feature() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let mut hello = client_hello();
    hello.features &= !features::HQ_REFRESH;
    client.hello_with(hello).await;
    client.start_stream(start()).await;
    warmup(&client).await;

    host.platform.set_capture_paused(true);
    let frames = observe(&client, Duration::from_secs(2)).await;
    assert!(frames.iter().all(|frame| frame.flags & FLAG_STILL == 0));
    no_video(&client, Duration::from_millis(100)).await;
}

async fn keyframe(client: &WireClient) -> VideoHeader {
    loop {
        let header = frame(client).await;
        if header.flags & FLAG_KEYFRAME != 0 {
            return header;
        }
    }
}

const KEYFRAME_COOLDOWN: Duration = Duration::from_millis(300);

/// Simulate the keyframe gate from when each request was actually sent. A
/// request inside the current cooldown joins one trailing response; a request
/// after the window starts a new response. This keeps scheduler load from
/// deciding which scenario the test exercised.
fn expected_keyframes(sent_at: &[Instant]) -> u64 {
    let Some((&first, rest)) = sent_at.split_first() else {
        return 0;
    };
    let mut last_response = first;
    let mut pending = false;
    let mut keyframes = 1;
    for &sent in rest {
        let trailing_at = last_response + KEYFRAME_COOLDOWN;
        if pending && sent > trailing_at {
            keyframes += 1;
            last_response = trailing_at;
        }
        if sent.duration_since(last_response) >= KEYFRAME_COOLDOWN {
            keyframes += 1;
            last_response = sent;
            pending = false;
        } else {
            pending = true;
        }
    }
    keyframes + u64::from(pending)
}

async fn keyframe_burst(spacing: Duration, advance_rate_clock: Duration) {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(stable_start()).await;
    warmup(&client).await;
    // The opening keyframe starts the same cooldown as a requested keyframe.
    observe(&client, Duration::from_millis(350)).await;

    let mut sent_at = Vec::with_capacity(5);
    for _ in 0..5 {
        client.send(pb::KeyframeRequest::default()).await;
        sent_at.push(Instant::now());
        if !spacing.is_zero() {
            sleep(spacing).await;
        }
    }
    let expected = expected_keyframes(&sent_at);
    let first = timeout(Duration::from_secs(1), keyframe(&client))
        .await
        .expect("the first request must be answered immediately");
    let mut keyframes = vec![first];
    for _ in 1..expected {
        keyframes.push(
            timeout(Duration::from_secs(1), keyframe(&client))
                .await
                .expect("excess requests must produce one merged keyframe"),
        );
    }
    for pair in keyframes.windows(2) {
        assert_ne!(pair[0].frame_id, pair[1].frame_id);
        assert!(pair[1].capture_ts_us - pair[0].capture_ts_us >= 300_000);
    }
    assert_eq!(host.platform.forced_keyframes(), expected);

    host.clock.skip(advance_rate_clock);
    host.mgr.tick_all();
    let later = observe(&client, Duration::from_millis(600)).await;
    assert!(later.iter().all(|frame| frame.flags & FLAG_KEYFRAME == 0));
    assert_eq!(host.platform.forced_keyframes(), expected);
    assert_eq!(host.platform.encoders_started().len(), 1);
    assert!(later.iter().all(|frame| frame.epoch == first.epoch));
}

#[tokio::test]
async fn five_requests_in_one_burst_keep_one_trailing_keyframe() {
    keyframe_burst(Duration::ZERO, Duration::ZERO).await;
}

#[tokio::test]
async fn requests_around_the_encoder_callback_keep_one_trailing_keyframe() {
    keyframe_burst(Duration::from_millis(7), Duration::ZERO).await;
}

#[tokio::test]
async fn keyframe_burst_crossing_the_rate_floor_deadline_keeps_the_same_pipeline() {
    keyframe_burst(Duration::ZERO, Duration::from_secs(3)).await;
}

fn hello_with_codecs(codecs: &[pb::Codec]) -> pb::ClientHello {
    pb::ClientHello {
        caps: Some(pb::ClientCaps {
            decode: codecs.iter().map(|&codec| codec as i32).collect(),
            ..Default::default()
        }),
        ..client_hello()
    }
}

#[tokio::test]
async fn unsupported_hevc_falls_back_to_h264_and_survives_a_rebuild() {
    let host = WireHost::start().await;
    host.platform.set_hevc_unsupported(true);
    let mut client = host.connect().await;
    client
        .hello_with(hello_with_codecs(&[pb::Codec::Hevc, pb::Codec::H264]))
        .await;
    let config = client.start_stream(start()).await;
    assert_eq!(config.codec, pb::Codec::H264 as i32);
    assert_eq!(
        host.platform
            .encoders_started_with_codecs()
            .last()
            .unwrap()
            .codec,
        pb::Codec::H264
    );
    assert_eq!(frame(&client).await.flags, FLAG_KEYFRAME);

    client.send(pb::PauseStream {}).await;
    host.wait_until(|| host.platform.active_captures() == 0)
        .await;
    client.send(pb::ResumeStream {}).await;
    let resumed = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            pb::envelope::Msg::Error(error) => panic!("resume failed: {error:?}"),
            _ => None,
        })
        .await;
    assert_eq!(resumed.epoch, config.epoch + 1);
    assert_eq!(resumed.codec, pb::Codec::H264 as i32);
    assert_eq!(
        host.platform
            .encoders_started_with_codecs()
            .last()
            .unwrap()
            .codec,
        pb::Codec::H264
    );
}

#[tokio::test]
async fn unavailable_only_advertised_codec_uses_existing_stream_failure() {
    let host = WireHost::start().await;
    host.platform.set_hevc_unsupported(true);
    let mut client = host.connect().await;
    client
        .hello_with(hello_with_codecs(&[pb::Codec::Hevc]))
        .await;
    client.send(start()).await;
    let error = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Error(error) => Some(error.clone()),
            pb::envelope::Msg::StreamConfig(config) => panic!("unexpected stream: {config:?}"),
            _ => None,
        })
        .await;
    assert_eq!(error.code, u32::from(ErrorCode::Other.code()));
    assert_eq!(error.message, "unsupported platform operation");
    assert_eq!(host.platform.active_captures(), 0);
    assert!(host.platform.encoders_started().is_empty());
    no_video(&client, Duration::from_millis(350)).await;
}

#[tokio::test]
async fn codec_selection_preserves_the_client_preference_order() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client
        .hello_with(hello_with_codecs(&[pb::Codec::H264, pb::Codec::Hevc]))
        .await;
    assert_eq!(
        client.start_stream(start()).await.codec,
        pb::Codec::H264 as i32
    );
}
