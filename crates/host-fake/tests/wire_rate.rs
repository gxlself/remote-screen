use host_core::config::HostConfig;
use rsp_core::{
    clock::unix_micros,
    features,
    fec::{KEYFRAME_MIN_REDUNDANCY, MAX_PARITY_SHARDS},
    pb,
    ratectl::{Controller, Path},
};
use std::time::Duration;
use tokio::time::timeout;

mod wire_support;
use wire_support::{client_hello, WireClient, WireHost, WireOptions};

fn start() -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        quality: pb::Quality::Auto as i32,
        ..Default::default()
    }
}

async fn rate_update(client: &mut WireClient) -> pb::RateUpdate {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::RateUpdate(update) => Some(update.clone()),
            pb::envelope::Msg::Error(error) => panic!("stream failed: {error:?}"),
            _ => None,
        })
        .await
}

// A Pong is a control-stream barrier: Feedback/SetQuality sent before the Ping
// has reached the host before advancing its test clock again.
async fn sync_rates(client: &mut WireClient) -> Vec<pb::RateUpdate> {
    let t1_us = unix_micros();
    client.send(pb::Ping { t1_us }).await;
    let mut rates = Vec::new();
    loop {
        match client.next().await {
            pb::envelope::Msg::Pong(pong) if pong.t1_us == t1_us => return rates,
            pb::envelope::Msg::RateUpdate(update) => rates.push(update),
            pb::envelope::Msg::Error(error) => panic!("stream failed: {error:?}"),
            _ => {}
        }
    }
}

async fn no_rate_update(client: &mut WireClient) {
    client
        .expect_none_matching(Duration::from_millis(500), |msg| {
            matches!(msg, pb::envelope::Msg::RateUpdate(_))
        })
        .await;
}

#[tokio::test]
async fn path_start_and_explicit_bitrate_follow_the_controller() {
    for requested in [0, 8_000, 1, 100_000] {
        let host = WireHost::start().await;
        let mut client = host.connect().await;
        let (hello, _) = client.hello().await;
        assert_ne!(hello.features & features::RATE_FEEDBACK, 0);
        assert_ne!(hello.features & features::FEC_RS, 0);
        let t = unix_micros();
        let expected = Controller::new(Path::Lan, pb::Quality::Auto, requested, (1920, 1080), t)
            .tick(t)
            .encoder_kbps;
        let config = client
            .start_stream(pb::StartStream {
                bitrate_kbps: requested,
                ..start()
            })
            .await;
        println!("LAN requested={requested} encoder_kbps={expected}");
        assert_eq!(host.platform.encoders_started().last().unwrap().3, expected);
        assert_eq!(config.bitrate_kbps, expected);
        assert_ne!(expected, 500);
    }
}

#[tokio::test]
async fn client_max_height_limits_stream_dimensions() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client
        .start_stream(pb::StartStream {
            max_width: 4_000,
            max_height: 720,
            ..start()
        })
        .await;
    assert_eq!((config.width, config.height), (1280, 720));
    let encoder = *host.platform.encoders_started().last().unwrap();
    assert_eq!((encoder.0, encoder.1), (1280, 720));
}

#[tokio::test]
async fn initial_quality_eco_limits_rate_fps_and_size() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client
        .start_stream(pb::StartStream {
            quality: pb::Quality::Eco as i32,
            ..start()
        })
        .await;
    let t = unix_micros();
    let mut ctl = Controller::new(Path::Lan, pb::Quality::Eco, 0, (1920, 1080), t);
    let expected = ctl.tick(t);
    assert_eq!(config.bitrate_kbps, expected.encoder_kbps);
    assert_eq!(config.fps, 30);
    assert_eq!((config.width, config.height), ctl.stream_size());
}

fn parity(datagram: &[u8]) -> u16 {
    u16::from_be_bytes([datagram[12], datagram[13]])
}

fn expected_parity(k: u16, ratio_pct: u32, keyframe: bool) -> f32 {
    if ratio_pct == 0 {
        return 0.0;
    }
    let ratio = ratio_pct as f32 / 100.0;
    let effective_ratio = if keyframe {
        ratio.max(KEYFRAME_MIN_REDUNDANCY)
    } else {
        ratio
    };
    (f32::from(k) * effective_ratio)
        .ceil()
        .clamp(1.0, MAX_PARITY_SHARDS as f32)
}

#[tokio::test]
async fn fec_is_enabled_from_the_first_frame() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(start()).await;
    for _ in 0..20 {
        let datagram = client.recv_datagram(0x10).await;
        assert!(parity(&datagram) >= 1);
    }
}

#[tokio::test]
async fn delay_feedback_reduces_wire_target_and_encoder_rate() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client.start_stream(start()).await;
    let t = unix_micros();
    let initial_target = Controller::new(Path::Lan, pb::Quality::Auto, 0, (1920, 1080), t)
        .tick(t)
        .target_kbps;
    let mut updates = Vec::new();
    for _ in 0..3 {
        host.clock.skip(Duration::from_millis(500));
        client
            .send(pb::Feedback {
                epoch: config.epoch,
                delay_gradient_us_per_s: 20_000,
                interval_ms: 200,
                ..Default::default()
            })
            .await;
        updates.extend(sync_rates(&mut client).await);
    }
    assert!(updates.iter().any(|u| u.target_kbps < initial_target));
    host.wait_until(|| {
        host.platform
            .encoder_rates()
            .last()
            .is_some_and(|&(kbps, _)| kbps < config.bitrate_kbps)
    })
    .await;
    assert_eq!(host.platform.encoders_started().len(), 1);
}

async fn frame_after(client: &WireClient, after_us: u64, keyframe: bool) -> Vec<u8> {
    timeout(Duration::from_secs(3), async {
        loop {
            let datagram = client.recv_datagram(0x10).await;
            let captured_us = u64::from_be_bytes(datagram[20..28].try_into().unwrap());
            if (datagram[1] & 1 != 0) == keyframe && captured_us >= after_us {
                return datagram;
            }
        }
    })
    .await
    .expect("no fresh frame of the requested type")
}

#[tokio::test]
async fn shard_loss_increases_fec_update_and_keyframe_parity() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client.start_stream(start()).await;
    let baseline_delta = frame_after(&client, 0, false).await;
    let mut updates = Vec::new();
    for _ in 0..4 {
        client
            .send(pb::Feedback {
                epoch: config.epoch,
                shards_received: 100,
                shards_lost: 20,
                interval_ms: 200,
                ..Default::default()
            })
            .await;
        updates.extend(sync_rates(&mut client).await);
    }
    let update = updates.last().expect("loss feedback must update FEC");
    assert!(update.fec_ratio_pct > 5);
    let after_us = unix_micros();
    client.send(pb::KeyframeRequest::default()).await;
    let protected = frame_after(&client, after_us, true).await;
    let data_shards = u16::from_be_bytes([protected[10], protected[11]]);
    let expected = expected_parity(data_shards, update.fec_ratio_pct, true);
    // RateUpdate rounds the ratio to whole percent, so allow one shard of rounding.
    assert!((f32::from(parity(&protected)) - expected).abs() <= 1.0);
    // Delta frames have no keyframe floor, so the increased ratio must add parity.
    let protected_delta = frame_after(&client, after_us, false).await;
    assert!(parity(&protected_delta) > parity(&baseline_delta));
}

#[tokio::test]
async fn set_quality_eco_updates_encoder_and_rebuilds_with_a_new_epoch() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let initial = client.start_stream(start()).await;
    client
        .send(pb::SetQuality {
            quality: pb::Quality::Eco as i32,
        })
        .await;
    let update = rate_update(&mut client).await;
    assert_eq!(update.fps, 30);
    let rebuilt = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            _ => None,
        })
        .await;
    assert_eq!(rebuilt.epoch, initial.epoch + 1);
    assert_eq!(rebuilt.fps, 30);
    let t = unix_micros();
    let mut ctl = Controller::new(Path::Lan, pb::Quality::Auto, 0, (1920, 1080), t);
    ctl.tick(t);
    ctl.set_quality(pb::Quality::Eco, t);
    assert_eq!((rebuilt.width, rebuilt.height), ctl.stream_size());
    host.wait_until(|| {
        host.platform
            .encoder_rates()
            .last()
            .is_some_and(|r| r.1 == 30)
    })
    .await;
}

#[tokio::test]
async fn old_epoch_feedback_does_not_change_the_controller() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let config = client.start_stream(start()).await;
    for _ in 0..3 {
        host.clock.skip(Duration::from_millis(500));
        client
            .send(pb::Feedback {
                epoch: config.epoch - 1,
                delay_gradient_us_per_s: 20_000,
                interval_ms: 200,
                ..Default::default()
            })
            .await;
        assert!(sync_rates(&mut client).await.is_empty());
    }
    no_rate_update(&mut client).await;
    assert_eq!(host.platform.encoders_started().len(), 1);
    assert!(host
        .platform
        .encoder_rates()
        .iter()
        .all(|&(kbps, _)| kbps == config.bitrate_kbps));
}

#[tokio::test]
async fn quality_still_applies_without_negotiating_rate_feedback() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    let mut hello = client_hello();
    hello.features &= !features::RATE_FEEDBACK;
    client.hello_with(hello).await;
    client.start_stream(start()).await;
    client
        .send(pb::SetQuality {
            quality: pb::Quality::Eco as i32,
        })
        .await;
    assert!(sync_rates(&mut client).await.is_empty());
    host.wait_until(|| {
        host.platform
            .encoder_rates()
            .last()
            .is_some_and(|r| r.1 == 30)
    })
    .await;
    no_rate_update(&mut client).await;
}

#[tokio::test]
async fn host_stats_report_lan_traffic_using_the_system_clock() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(start()).await;
    // A jumped session clock must not divide capture/send counts by 60 s.
    host.clock.skip(Duration::from_secs(60));
    host.mgr.tick_all();
    let stats = timeout(Duration::from_millis(2500), async {
        client
            .next_matching(|msg| match msg {
                pb::envelope::Msg::HostStats(stats) => Some(stats.clone()),
                _ => None,
            })
            .await
    })
    .await
    .expect("HostStats must arrive within 2.5 seconds");
    assert_eq!(stats.path, pb::PathKind::Lan as i32);
    assert_eq!(stats.cpu_pct, 0);
    assert!(stats.send_kbps > 0);
    assert!(stats.capture_fps >= 15, "unexpected capture FPS: {stats:?}");
}

#[tokio::test]
async fn configured_and_requested_fps_caps_apply_to_start_and_runtime_updates() {
    for (configured, requested, expected) in [(15, 0, 15), (60, 10, 10), (0, 0, 60)] {
        let mut config = HostConfig::default();
        config.video.max_fps = configured;
        let host = WireHost::start_with(WireOptions {
            config,
            ..Default::default()
        })
        .await;
        let mut client = host.connect().await;
        client.hello().await;
        let config = client
            .start_stream(pb::StartStream {
                fps: requested,
                ..start()
            })
            .await;
        assert_eq!(config.fps, expected);
        client
            .send(pb::Feedback {
                epoch: config.epoch,
                frames_lost: 1,
                ..Default::default()
            })
            .await;
        let update = rate_update(&mut client).await;
        assert_eq!(update.fps, expected);
        host.wait_until(|| {
            host.platform
                .encoder_rates()
                .last()
                .is_some_and(|&(kbps, fps)| kbps < config.bitrate_kbps && fps == expected)
        })
        .await;
    }
}

#[tokio::test]
async fn pause_preserves_rate_and_fec_until_resume() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let initial = client.start_stream(start()).await;
    client
        .send(pb::Feedback {
            epoch: initial.epoch,
            frames_lost: 1,
            shards_received: 100,
            shards_lost: 20,
            ..Default::default()
        })
        .await;
    let update = rate_update(&mut client).await;
    host.wait_until(|| {
        host.platform
            .encoder_rates()
            .last()
            .is_some_and(|r| r.0 < initial.bitrate_kbps)
    })
    .await;
    let reduced = host.platform.encoder_rates().last().unwrap().0;
    client.send(pb::PauseStream {}).await;
    host.wait_until(|| host.platform.active_captures() == 0)
        .await;
    sync_rates(&mut client).await;
    host.clock.skip(Duration::from_secs(3));
    host.mgr.tick_all();
    client
        .expect_none_matching(Duration::from_millis(300), |msg| {
            matches!(msg, pb::envelope::Msg::HostStats(_))
        })
        .await;
    let resumed_at = unix_micros();
    client.send(pb::ResumeStream {}).await;
    let resumed = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            _ => None,
        })
        .await;
    assert_eq!(resumed.epoch, initial.epoch + 1);
    assert_eq!(resumed.bitrate_kbps, reduced);
    let frame = frame_after(&client, resumed_at, true).await;
    let data_shards = u16::from_be_bytes([frame[10], frame[11]]);
    let expected = expected_parity(data_shards, update.fec_ratio_pct, true);
    // RateUpdate rounds the ratio to whole percent, so allow one shard of rounding.
    assert!((f32::from(parity(&frame)) - expected).abs() <= 1.0);
    assert_eq!(host.platform.encoders_started().len(), 2);
}
