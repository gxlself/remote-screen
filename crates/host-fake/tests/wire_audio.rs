use host_fake::{audio::FakeAudioFailure, FakePlatform};
use rs_audio::{codec::AudioDecoder, gen::Signal, FRAME_LEN};
use rsp_core::{
    audio::{decode, AudioHeader, AUDIO_TYPE},
    features, pb,
};
use rsp_transport::DatagramKind;
use std::time::Duration;
use tokio::time::{sleep, timeout, timeout_at, Instant};

mod wire_support;
use wire_support::{client_hello, device_id, Dev, WireClient, WireHost, WireOptions};

fn hello(audio: bool) -> pb::ClientHello {
    let mut hello = client_hello();
    hello.caps = Some(pb::ClientCaps {
        audio_opus: audio,
        ..Default::default()
    });
    hello
}

fn start(fps: u32) -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        max_width: 320,
        fps,
        bitrate_kbps: 1_000,
        ..Default::default()
    }
}

fn unlimited_start() -> pb::StartStream {
    pb::StartStream {
        bitrate_kbps: 0,
        ..start(10)
    }
}

async fn audio_for(client: &WireClient, duration: Duration) -> Vec<Vec<u8>> {
    let deadline = Instant::now() + duration;
    let mut packets = Vec::new();
    while let Ok(packet) = timeout_at(deadline, client.conn.read_datagram()).await {
        let packet = packet.expect("audio session must stay connected");
        if packet.first() == Some(&AUDIO_TYPE) {
            packets.push(packet.to_vec());
        }
    }
    packets
}

async fn no_audio(client: &WireClient, duration: Duration) {
    assert!(
        audio_for(client, duration).await.is_empty(),
        "received audio while disabled"
    );
}

async fn next_audio(client: &WireClient, within: Duration) -> Vec<u8> {
    timeout(within, client.recv_datagram(AUDIO_TYPE))
        .await
        .expect("audio did not start within the deadline")
}

// Control messages are handled in stream order. A matching Pong proves that
// the preceding PauseStream/SetAudio/ResumeStream has been handled by the host.
async fn control_barrier(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 42 }).await;
    client
        .next_matching(|message| match message {
            pb::envelope::Msg::Pong(pong) if pong.t1_us == 42 => Some(()),
            _ => None,
        })
        .await;
}

async fn drain_arrived_audio(client: &WireClient) -> Vec<Vec<u8>> {
    let mut packets = Vec::new();
    // On this loopback path Quinn 0.11.19 writes DATAGRAM before STREAM frames,
    // so audio queued before the pause arrives no later than its barrier Pong.
    while let Ok(packet) = timeout(Duration::ZERO, client.conn.read_datagram()).await {
        let packet = packet.expect("audio session must stay connected");
        if packet.first() == Some(&AUDIO_TYPE) {
            packets.push(packet.to_vec());
        }
    }
    packets
}

fn decode_packets(packets: &[Vec<u8>]) -> Vec<AudioHeader> {
    assert!(!packets.is_empty(), "expected audio packets");
    let mut decoder = AudioDecoder::new().unwrap();
    let mut pcm = [0.0; FRAME_LEN];
    packets
        .iter()
        .map(|packet| {
            let (header, opus) = decode(packet).unwrap();
            assert_eq!(decoder.decode(opus, &mut pcm).unwrap(), 480);
            header
        })
        .collect()
}

fn check_packets(packets: &[Vec<u8>]) -> Vec<AudioHeader> {
    let headers = decode_packets(packets);
    for pair in headers.windows(2) {
        assert_eq!(pair[1].seq, pair[0].seq.wrapping_add(1));
    }
    headers
}

async fn no_errors(client: &mut WireClient, duration: Duration) {
    client
        .expect_none_matching(duration, |message| {
            matches!(
                message,
                pb::envelope::Msg::Error(_) | pb::envelope::Msg::Goodbye(_)
            )
        })
        .await;
}

#[tokio::test]
async fn quic_audio_advertises_capability_and_sends_ten_ms_opus_packets() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let (server, auth) = client.hello_with(hello(true)).await;
    assert_ne!(server.features & (1 << 15), 0);
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    no_audio(&client, Duration::from_millis(100)).await;
    assert_eq!(host.platform.audio().starts(), 0);

    client.start_stream(start(10)).await;
    let packets = audio_for(&client, Duration::from_secs(1)).await;
    assert!(packets.len() >= 90, "only {} audio packets", packets.len());
    let headers = check_packets(&packets);
    assert_eq!(headers[0].seq, 0);
    for pair in headers.windows(2) {
        let delta = pair[1].capture_ts_us - pair[0].capture_ts_us;
        assert!((9_998..=10_002).contains(&delta), "timestamp delta {delta}");
    }
}

#[tokio::test]
async fn set_audio_overrides_caps_and_preserves_sequence_when_reenabled() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello_with(hello(false)).await;
    client.start_stream(start(10)).await;
    no_audio(&client, Duration::from_millis(500)).await;
    assert_eq!(host.platform.audio().starts(), 0);

    client.send(pb::SetAudio { enabled: true }).await;
    let mut packets = vec![next_audio(&client, Duration::from_millis(300)).await];
    client.send(pb::SetAudio { enabled: false }).await;
    // Consume packets already in flight while the disable reaches the host.
    packets.extend(audio_for(&client, Duration::from_millis(100)).await);
    let headers = check_packets(&packets);
    no_audio(&client, Duration::from_millis(500)).await;
    assert_eq!(host.platform.audio().active(), 0);

    client.send(pb::SetAudio { enabled: true }).await;
    let resumed = next_audio(&client, Duration::from_millis(300)).await;
    assert_eq!(
        decode(&resumed).unwrap().0.seq,
        headers.last().unwrap().seq.wrapping_add(1)
    );
}

#[tokio::test]
async fn missing_feature_ignores_caps_and_set_audio() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let mut hello = hello(true);
    hello.features &= !features::AUDIO_OPUS;
    client.hello_with(hello).await;
    client.start_stream(start(10)).await;
    no_audio(&client, Duration::from_millis(500)).await;
    client.send(pb::SetAudio { enabled: true }).await;
    no_audio(&client, Duration::from_millis(500)).await;
    assert_eq!(host.platform.audio().starts(), 0);
    client.recv_datagram(0x10).await;
}

#[tokio::test]
async fn set_audio_before_stream_is_remembered_without_starting_capture() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello_with(hello(true)).await;
    client.send(pb::SetAudio { enabled: false }).await;
    client.start_stream(start(10)).await;
    no_audio(&client, Duration::from_millis(150)).await;
    client.send(pb::PauseStream {}).await;
    client.send(pb::SetAudio { enabled: true }).await;
    no_audio(&client, Duration::from_millis(100)).await;
    assert_eq!(host.platform.audio().starts(), 0);
    client.send(pb::ResumeStream {}).await;
    let packet = next_audio(&client, Duration::from_millis(300)).await;
    assert_eq!(decode(&packet).unwrap().0.seq, 0);
}

#[tokio::test]
async fn pause_resume_keeps_connection_sequence() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello_with(hello(true)).await;
    client.start_stream(start(10)).await;
    let mut packets = vec![client.recv_datagram(AUDIO_TYPE).await];
    packets.extend(audio_for(&client, Duration::from_millis(150)).await);
    client.send(pb::PauseStream {}).await;
    control_barrier(&mut client).await;
    packets.extend(drain_arrived_audio(&client).await);
    let headers = check_packets(&packets);
    no_audio(&client, Duration::from_millis(300)).await;
    assert_eq!(host.platform.audio().active(), 0);

    client.send(pb::ResumeStream {}).await;
    control_barrier(&mut client).await;
    let packet = next_audio(&client, Duration::from_millis(300)).await;
    assert_eq!(
        decode(&packet).unwrap().0.seq,
        headers.last().unwrap().seq.wrapping_add(1)
    );
}

#[tokio::test]
async fn unsupported_audio_keeps_static_capability_and_video_session() {
    let platform = FakePlatform::new();
    platform.audio().set_supported(false);
    let host = WireHost::start_with(WireOptions {
        platform: Some(platform),
        ..Default::default()
    })
    .await;
    let mut client = host.connect().await;
    let (server, _) = client.hello_with(hello(true)).await;
    assert_ne!(server.features & features::AUDIO_OPUS, 0);
    client.start_stream(start(10)).await;
    no_audio(&client, Duration::from_millis(500)).await;
    client.send(pb::SetAudio { enabled: true }).await;
    no_audio(&client, Duration::from_millis(100)).await;
    client.recv_datagram(0x10).await;
    no_errors(&mut client, Duration::from_millis(100)).await;
    assert_eq!(host.platform.audio().starts(), 0);
}

#[tokio::test]
async fn capture_failures_are_nonfatal_and_retry_only_on_explicit_trigger() {
    for failure in [
        FakeAudioFailure::PermissionDenied,
        FakeAudioFailure::Platform,
    ] {
        let host = WireHost::start().await;
        host.platform.audio().fail_next(failure);
        let mut client = host.connect().await;
        client.hello_with(hello(true)).await;
        // Keep the controller above its resolution floor so this rate tick
        // does not rebuild video (a rebuild legitimately retries audio).
        client.start_stream(unlimited_start()).await;
        no_audio(&client, Duration::from_millis(100)).await;
        // Even a rate tick must not retry the failed optional capture.
        host.clock.skip(Duration::from_secs(2));
        host.mgr.tick_all();
        no_audio(&client, Duration::from_millis(100)).await;
        client.recv_datagram(0x10).await;
        no_errors(&mut client, Duration::from_millis(100)).await;
        assert_eq!(host.platform.audio().starts(), 0);

        client.send(pb::SetAudio { enabled: true }).await;
        next_audio(&client, Duration::from_millis(300)).await;
        assert_eq!(host.platform.audio().starts(), 1);
    }
}

#[tokio::test]
async fn controller_and_viewer_share_capture_until_both_disconnect() {
    let host = WireHost::start().await;
    let mut controller = host.connect().await;
    let (_, auth) = controller.hello_with(hello(true)).await;
    assert_eq!(auth.role, pb::Role::Controller as i32);
    controller.start_stream(start(10)).await;
    // Anchor each start deadline after a control round trip through the host.
    control_barrier(&mut controller).await;
    next_audio(&controller, Duration::from_millis(300)).await;

    let mut viewer = host.connect_as(Dev::B, 0).await;
    let mut hello = hello(true);
    hello.intent = pb::Intent::View as i32;
    let (_, auth) = viewer.hello_with(hello).await;
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    viewer.start_stream(start(10)).await;
    control_barrier(&mut viewer).await;
    let packet = next_audio(&viewer, Duration::from_millis(300)).await;
    assert_eq!(decode(&packet).unwrap().0.seq, 0);
    assert_eq!(host.platform.audio().starts(), 1);
    assert_eq!(host.platform.audio().active(), 1);

    drop(controller);
    host.wait_until(|| {
        !host
            .conns()
            .iter()
            .any(|conn| conn.device_id == device_id(Dev::A))
    })
    .await;
    let packets = vec![
        viewer.recv_datagram(AUDIO_TYPE).await,
        viewer.recv_datagram(AUDIO_TYPE).await,
    ];
    check_packets(&packets);
    assert_eq!(host.platform.audio().active(), 1);
    drop(viewer);
    // Start the release deadline after the host has observed the disconnect.
    host.wait_until(|| {
        !host
            .conns()
            .iter()
            .any(|conn| conn.device_id == device_id(Dev::B))
    })
    .await;
    timeout(Duration::from_millis(300), async {
        while host.platform.audio().active() != 0 {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("last disconnect must release shared audio capture");
}

#[tokio::test]
async fn silence_suppression_resumes_without_advancing_sequence() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello_with(hello(true)).await;
    client.start_stream(start(10)).await;
    let mut packets = audio_for(&client, Duration::from_millis(100)).await;
    host.platform.audio().set_signal(Signal::Silence);
    packets.extend(audio_for(&client, Duration::from_millis(260)).await);
    let headers = check_packets(&packets);
    no_audio(&client, Duration::from_millis(500)).await;

    host.platform.audio().set_signal(Signal::Sine {
        freq_hz: 440,
        amp: 0.25,
    });
    let packet = next_audio(&client, Duration::from_millis(100)).await;
    let header = decode(&packet).unwrap().0;
    let previous = headers.last().unwrap();
    assert_eq!(header.seq, previous.seq.wrapping_add(1));
    assert!(header.capture_ts_us > previous.capture_ts_us + 500_000);
    assert_eq!(host.platform.audio().starts(), 1);
}

#[tokio::test]
async fn initial_video_bitrate_reserves_audio_budget() {
    let host = WireHost::start().await;
    let mut quiet = host.connect().await;
    quiet.hello_with(hello(false)).await;
    let video_only = quiet.start_stream(unlimited_start()).await;
    let mut audible = host.connect_as(Dev::B, 0).await;
    audible.hello_with(hello(true)).await;
    let with_audio = audible.start_stream(unlimited_start()).await;
    let difference = video_only.bitrate_kbps - with_audio.bitrate_kbps;
    assert!(
        (96..=128).contains(&difference),
        "reserved {difference} kbps"
    );
}

#[tokio::test]
async fn set_audio_updates_video_budget_on_the_next_rate_tick() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello_with(hello(false)).await;
    let config = client.start_stream(unlimited_start()).await;
    client.send(pb::SetAudio { enabled: true }).await;
    control_barrier(&mut client).await;
    next_audio(&client, Duration::from_millis(300)).await;
    host.clock.skip(Duration::from_secs(1));
    host.mgr.tick_all();
    timeout(Duration::from_millis(300), async {
        loop {
            if let Some(&(kbps, _)) = host.platform.encoder_rates().last() {
                if kbps < config.bitrate_kbps {
                    assert!((96..=128).contains(&(config.bitrate_kbps - kbps)));
                    break;
                }
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("audio enable must reserve video bitrate at the next tick");

    client.send(pb::SetAudio { enabled: false }).await;
    // Tick commands have priority over control reads; order the disable before
    // tick_all rather than assuming a sampling window has let it arrive.
    control_barrier(&mut client).await;
    host.clock.skip(Duration::from_secs(1));
    host.mgr.tick_all();
    timeout(Duration::from_millis(300), async {
        while host.platform.encoder_rates().last().map(|rate| rate.0) != Some(config.bitrate_kbps) {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("audio disable must restore video bitrate at the next tick");
}

#[tokio::test]
async fn tcp_audio_sends_at_least_ninety_decodable_records_per_second() {
    let host = WireHost::start().await;
    let client = host.connect_tcp(Dev::A, 0).await;
    client.hello_with(hello(true)).await;
    client.start_stream(start(10)).await;
    let deadline = Instant::now() + Duration::from_secs(1);
    let mut packets = Vec::new();
    while let Ok(record) = timeout_at(deadline, client.conn.recv_datagram()).await {
        let (kind, payload) = record.unwrap();
        if kind == DatagramKind::Audio {
            let mut packet = vec![AUDIO_TYPE];
            packet.extend_from_slice(&payload);
            packets.push(packet);
        }
    }
    assert!(packets.len() >= 90, "only {} audio records", packets.len());
    let headers = decode_packets(&packets);
    assert_eq!(headers[0].seq, 0);
}

#[tokio::test]
async fn video_rebuild_does_not_restart_or_interrupt_audio() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello_with(hello(true)).await;
    let initial = client.start_stream(start(10)).await;
    let mut packets = vec![client.recv_datagram(AUDIO_TYPE).await];
    let before = audio_for(&client, Duration::from_millis(200)).await;
    assert!(!before.is_empty());
    packets.extend(before);
    let rebuilt = client.start_stream(start(20)).await;
    assert_eq!(rebuilt.epoch, initial.epoch + 1);
    let after = audio_for(&client, Duration::from_millis(200)).await;
    assert!(!after.is_empty());
    packets.extend(after);
    let headers = check_packets(&packets);
    for pair in headers.windows(2) {
        assert_eq!(pair[1].capture_ts_us - pair[0].capture_ts_us, 10_000);
    }
    assert_eq!(host.platform.audio().starts(), 1);
}

#[tokio::test]
async fn files_intent_never_starts_audio_even_with_capability_and_set_audio() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let mut hello = hello(true);
    hello.intent = pb::Intent::Files as i32;
    hello.features |= features::FILE_BROWSE;
    // The current integration may not advertise FILE_BROWSE yet. Exercise
    // FILES isolation without requiring that separate browsing feature.
    hello.required_features |=
        features::FILE_BROWSE & host_core::session::handlers::host_features();
    // FILES intentionally has no SessionState, so read the raw handshake.
    client.send(hello).await;
    let auth = client
        .next_matching(|message| match message {
            pb::envelope::Msg::AuthResult(auth) => Some(auth.clone()),
            _ => None,
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    client.send(pb::SetAudio { enabled: true }).await;
    client.send(start(10)).await;
    let error = client
        .next_matching(|message| match message {
            pb::envelope::Msg::Error(error) => Some(error.clone()),
            _ => None,
        })
        .await;
    assert_eq!(
        error.code,
        u32::from(rsp_core::errors::ErrorCode::PolicyDenied.code())
    );
    client.send(pb::ResumeStream {}).await;
    no_audio(&client, Duration::from_millis(500)).await;
    assert_eq!(host.platform.audio().starts(), 0);
    assert_eq!(host.platform.active_captures(), 0);
}
