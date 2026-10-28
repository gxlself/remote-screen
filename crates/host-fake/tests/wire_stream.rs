use rsp_core::pb;
use std::time::Duration;
use tokio::time::{sleep, timeout};

mod wire_support;
use wire_support::{client_hello, WireClient, WireHost};

const QUIET: Duration = Duration::from_millis(300);

fn start(fps: u32) -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        max_width: 640,
        fps,
        bitrate_kbps: 500,
        ..Default::default()
    }
}

async fn stream_config(client: &mut WireClient) -> pb::StreamConfig {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            pb::envelope::Msg::Error(error) => panic!("stream failed: {error:?}"),
            pb::envelope::Msg::Goodbye(goodbye) => panic!("stream closed: {goodbye:?}"),
            _ => None,
        })
        .await
}

async fn first_keyframe(client: &WireClient, config: &pb::StreamConfig) {
    let datagram = client.recv_datagram(0x10).await;
    assert!(datagram.len() >= 28);
    assert_eq!(
        datagram[1] & 1,
        1,
        "the first video frame must be a keyframe"
    );
    assert_eq!(datagram[2], config.display_id as u8);
    assert_eq!(datagram[3], config.epoch as u8);
}

async fn no_stream_config(client: &mut WireClient, duration: Duration) {
    client
        .expect_none_matching(duration, |msg| {
            matches!(
                msg,
                pb::envelope::Msg::StreamConfig(_)
                    | pb::envelope::Msg::Goodbye(_)
                    | pb::envelope::Msg::Error(_)
            )
        })
        .await;
}

async fn no_video(client: &WireClient) {
    assert!(timeout(QUIET, client.recv_datagram(0x10)).await.is_err());
}

async fn pause(host: &WireHost, client: &mut WireClient) {
    client.send(pb::PauseStream {}).await;
    sleep(Duration::from_millis(500)).await;
    assert_eq!(host.platform.active_captures(), 0);
    // Discard packets sent before PauseStream reached the host. The following
    // quiet check observes only packets arriving after the pipeline stopped.
    while let Ok(datagram) = timeout(Duration::from_millis(10), client.conn.read_datagram()).await {
        datagram.expect("the paused connection must stay open");
    }
    no_video(client).await;
}

#[tokio::test]
async fn pause_then_resume_restarts_capture_with_a_new_epoch_and_keyframe() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    let initial = client.start_stream(start(20)).await;
    assert_eq!(initial.epoch, 1);
    first_keyframe(&client, &initial).await;

    pause(&host, &mut client).await;
    client.send(pb::ResumeStream {}).await;
    let resumed = stream_config(&mut client).await;
    assert_eq!(resumed.epoch, 2);
    assert_eq!(resumed.display_id, initial.display_id);
    first_keyframe(&client, &resumed).await;
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn pause_fallback_resumes_after_45_seconds_without_closing_the_connection() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    assert_eq!(client.start_stream(start(20)).await.epoch, 1);
    pause(&host, &mut client).await;

    host.clock.skip(Duration::from_secs(30));
    host.mgr.tick_all();
    no_stream_config(&mut client, QUIET).await;
    assert_eq!(host.platform.active_captures(), 0);
    no_video(&client).await;

    host.clock.skip(Duration::from_secs(16));
    host.mgr.tick_all();
    let resumed = stream_config(&mut client).await;
    assert_eq!(resumed.epoch, 2);
    first_keyframe(&client, &resumed).await;
    assert_eq!(host.platform.active_captures(), 1);
    no_stream_config(&mut client, QUIET).await;
    assert_eq!(host.conns().len(), 1);
    assert!(client.conn.close_reason().is_none());
}

#[tokio::test]
async fn resume_while_live_only_requests_a_keyframe() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(start(20)).await;
    let forced = host.platform.forced_keyframes();
    client.send(pb::ResumeStream {}).await;
    host.wait_until(|| host.platform.forced_keyframes() == forced + 1)
        .await;
    no_stream_config(&mut client, QUIET).await;
    assert_eq!(host.platform.forced_keyframes(), forced + 1);
    assert_eq!(host.platform.encoders_started().len(), 1);
}

#[tokio::test]
async fn start_while_paused_defers_new_parameters_until_resume() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(start(20)).await;
    pause(&host, &mut client).await;

    client.send(start(15)).await;
    no_stream_config(&mut client, QUIET).await;
    no_video(&client).await;
    assert_eq!(host.platform.active_captures(), 0);
    assert_eq!(host.platform.encoders_started().len(), 1);

    client.send(pb::ResumeStream {}).await;
    let resumed = stream_config(&mut client).await;
    assert_eq!(resumed.epoch, 2);
    assert_eq!(resumed.fps, 15);
    first_keyframe(&client, &resumed).await;
}

#[tokio::test]
async fn repeated_start_requests_a_keyframe_or_rebuilds_changed_parameters() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    assert_eq!(client.start_stream(start(20)).await.epoch, 1);

    let forced = host.platform.forced_keyframes();
    client.send(start(20)).await;
    host.wait_until(|| host.platform.forced_keyframes() == forced + 1)
        .await;
    no_stream_config(&mut client, QUIET).await;
    assert_eq!(host.platform.forced_keyframes(), forced + 1);
    assert_eq!(host.platform.encoders_started().len(), 1);

    let changed = client.start_stream(start(15)).await;
    assert_eq!(changed.epoch, 2);
    assert_eq!(changed.fps, 15);
    assert_eq!(host.platform.encoders_started().len(), 2);
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn loss_requests_a_keyframe_without_reconfiguring_the_stream() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(start(20)).await;
    let forced = host.platform.forced_keyframes();
    let request = pb::KeyframeRequest {
        reason: pb::keyframe_request::Reason::Loss as i32,
        last_good_frame: 0,
    };
    client.send(request).await;
    host.wait_until(|| host.platform.forced_keyframes() == forced + 1)
        .await;
    no_stream_config(&mut client, QUIET).await;
    assert_eq!(host.platform.forced_keyframes(), forced + 1);
}

#[tokio::test]
async fn token_resume_after_background_continues_the_saved_stream_without_start() {
    let host = WireHost::start().await;
    let mut old = host.connect().await;
    let (_, auth) = old.hello().await;
    let initial = old.start_stream(start(20)).await;
    assert_eq!(initial.epoch, 1);
    old.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| host.conns().is_empty()).await;
    assert_eq!(host.platform.active_captures(), 0);

    let mut resumed = host.connect().await;
    let (_, next_auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token.clone(),
            ..client_hello()
        })
        .await;
    assert!(next_auth.resumed);
    assert_eq!(next_auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(next_auth.session_token.len(), 16);
    assert_ne!(next_auth.session_token, auth.session_token);
    let config = stream_config(&mut resumed).await;
    assert_eq!(config.epoch, 2);
    assert_eq!(config.fps, 20);
    assert_eq!(config.display_id, initial.display_id);
    first_keyframe(&resumed, &config).await;
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn token_resume_replaces_a_live_stream_before_starting_the_new_capture() {
    let host = WireHost::start().await;
    let mut old = host.connect().await;
    let (_, auth) = old.hello().await;
    let initial = old.start_stream(start(20)).await;
    assert_eq!(initial.epoch, 1);
    assert_eq!(host.platform.active_captures(), 1);

    let replacement = async {
        let mut resumed = host.connect().await;
        let (_, next_auth) = resumed
            .hello_with(pb::ClientHello {
                resume_token: auth.session_token.clone(),
                ..client_hello()
            })
            .await;
        assert!(next_auth.resumed);
        assert_eq!(next_auth.status, pb::auth_result::Status::Ok as i32);
        assert_eq!(next_auth.session_token.len(), 16);
        assert_ne!(next_auth.session_token, auth.session_token);
        let reason = old
            .next_matching(|msg| match msg {
                pb::envelope::Msg::Goodbye(goodbye) => Some(goodbye.reason),
                _ => None,
            })
            .await;
        assert_eq!(reason, pb::goodbye::Reason::Replaced as i32);
        assert_eq!(old.close_code().await, 3);
        let config = stream_config(&mut resumed).await;
        assert_eq!(config.epoch, 2);
        assert_eq!(config.fps, 20);
        assert_eq!(config.display_id, initial.display_id);
        first_keyframe(&resumed, &config).await;
        assert_eq!(host.platform.active_captures(), 1);
        assert_eq!(host.conns().len(), 1);
        resumed
    };
    tokio::pin!(replacement);
    let mut sample = tokio::time::interval(Duration::from_millis(1));
    let resumed = loop {
        tokio::select! {
            client = &mut replacement => break client,
            _ = sample.tick() => assert!(host.platform.active_captures() <= 1),
        }
    };
    assert!(resumed.conn.close_reason().is_none());
}

#[tokio::test]
async fn token_resume_without_a_previous_stream_waits_for_start() {
    let host = WireHost::start().await;
    let mut old = host.connect().await;
    let (_, auth) = old.hello().await;
    old.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| host.conns().is_empty()).await;

    let mut resumed = host.connect().await;
    let (_, next_auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert!(next_auth.resumed);
    no_stream_config(&mut resumed, Duration::from_millis(500)).await;
    assert_eq!(host.platform.active_captures(), 0);
    let config = resumed.start_stream(start(20)).await;
    assert_eq!(config.epoch, 1);
    first_keyframe(&resumed, &config).await;
}
