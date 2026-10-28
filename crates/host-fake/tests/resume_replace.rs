//! Session replacement and resume over real loopback transports.

use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use host_fake::faultnet::{Dir, FaultProxy, UdpRule};
use host_fake::harness::{app_identity, open, params, TestHost};
use rsp_client::session::{EndReason, EventStream, SessionEvent, SessionHandle, SessionParams};
use rsp_core::{pb, trust::Permission};
use tokio::time::{sleep, timeout, timeout_at, Instant};

async fn within_budget(test: impl Future<Output = ()>) {
    timeout(Duration::from_secs(15), test)
        .await
        .expect("resume replacement test completes within 15 s");
}

async fn start_proxy(host: &TestHost, seed: u64) -> FaultProxy {
    let mut attempts = 0;
    loop {
        attempts += 1;
        match FaultProxy::start(host.quic_addr, host.tcp_addr, seed).await {
            Ok(proxy) => return proxy,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && attempts < 8 => {
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("start fault proxy after {attempts} attempts: {error}"),
        }
    }
}

async fn initial_stream(events: &mut EventStream) -> pb::StreamConfig {
    timeout(Duration::from_secs(3), async {
        let mut streaming = false;
        let mut config = None;
        let mut frame_after_config = false;
        while !(streaming && config.is_some() && frame_after_config) {
            match events
                .recv()
                .await
                .expect("session stays open during startup")
            {
                SessionEvent::StateChanged(rsp_client::session::SessionStateKind::Streaming) => {
                    streaming = true;
                }
                SessionEvent::StreamConfig(value) => config = Some(value),
                SessionEvent::Frame(frame) if config.is_some() => {
                    frame_after_config = true;
                    assert!(
                        frame.frame.keyframe,
                        "first frame after StreamConfig is keyframe"
                    );
                }
                SessionEvent::Ended { kind, reason, .. } => {
                    panic!("session ended during startup: {kind:?}: {reason}")
                }
                _ => {}
            }
        }
        config.expect("startup includes StreamConfig")
    })
    .await
    .expect("session reaches Streaming with a frame within 3 s")
}

async fn resumed_stream(
    events: &mut EventStream,
    started: Instant,
) -> (pb::StreamConfig, bool, Duration, Duration) {
    timeout_at(started + Duration::from_secs(2), async {
        let mut streaming_at = None;
        let mut config = None;
        let mut first_frame_at = None;
        let mut first_frame_keyframe = None;
        while first_frame_at.is_none() {
            match events.recv().await.expect("resumed session stays open") {
                SessionEvent::StateChanged(rsp_client::session::SessionStateKind::Streaming) => {
                    streaming_at.get_or_insert(Instant::now());
                }
                SessionEvent::StreamConfig(value) => config = Some(value),
                SessionEvent::Frame(frame) if config.is_some() => {
                    first_frame_at = Some(Instant::now());
                    first_frame_keyframe = Some(frame.frame.keyframe);
                }
                SessionEvent::Ended { kind, reason, .. } => {
                    panic!("resumed session ended early: {kind:?}: {reason}")
                }
                _ => {}
            }
        }
        (
            config.expect("resumed session includes StreamConfig"),
            first_frame_keyframe.expect("first resumed frame"),
            streaming_at
                .expect("resumed session reaches Streaming")
                .duration_since(started),
            first_frame_at
                .expect("resumed session receives a frame")
                .duration_since(started),
        )
    })
    .await
    .expect("resumed session reaches Streaming and receives a frame within 2 s")
}

fn resume_params(
    host: &TestHost,
    app: Arc<rsp_transport::identity::LocalIdentity>,
    token: Vec<u8>,
) -> SessionParams {
    let mut params = params(host, app, &[host.quic_addr]);
    params.resume_token = Some(token);
    params
}

async fn hud_transport(session: &SessionHandle) -> pb::Transport {
    timeout(Duration::from_secs(3), async {
        loop {
            if let Some(hud) = session.stats().hud {
                return hud.transport;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("HUD reports transport within 3 s")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn quic_dead_old_path_resumes_within_one_second() {
    within_budget(async {
        let host = TestHost::start().await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let proxy = start_proxy(&host, 0xa7_0001).await;
        let mut old_params = params(&host, Arc::clone(&app), &[proxy.addr()]);
        old_params.dial_options.tcp_after = Duration::from_secs(5);
        let (old, mut old_events) = open(old_params).await.expect("open old QUIC session");
        let old_config = initial_stream(&mut old_events).await;
        let token = old.resume_token().expect("old session has resume token");

        proxy.cut();
        let started = Instant::now();
        let (new, mut new_events) = open(resume_params(&host, Arc::clone(&app), token))
            .await
            .expect("open resumed QUIC session");
        let (new_config, first_keyframe, streaming_after, frame_after) =
            resumed_stream(&mut new_events, started).await;
        eprintln!(
            "quic_dead_old_path_resumes_within_one_second: streaming_after={streaming_after:?} frame_after={frame_after:?}"
        );
        assert!(new.resumed(), "new session reports resumed");
        assert!(streaming_after <= Duration::from_secs(1));
        assert!(frame_after <= Duration::from_secs(2));
        assert_eq!(new_config.epoch, old_config.epoch + 1);
        assert!(first_keyframe, "first resumed frame is keyframe");

        new.close();
        old.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_dead_old_path_resumes_within_one_second() {
    within_budget(async {
        let host = TestHost::start().await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let proxy = start_proxy(&host, 0xa7_0002).await;
        proxy.set_udp(Dir::ToHost, UdpRule { blackhole: true, ..UdpRule::default() });
        proxy.set_udp(Dir::ToClient, UdpRule { blackhole: true, ..UdpRule::default() });
        let old_params = params(&host, Arc::clone(&app), &[proxy.addr()]);
        let (old, mut old_events) = open(old_params).await.expect("open old TCP session");
        let old_config = initial_stream(&mut old_events).await;
        assert_eq!(hud_transport(&old).await, pb::Transport::Tcp);
        let token = old.resume_token().expect("old session has resume token");

        proxy.cut();
        let started = Instant::now();
        let (new, mut new_events) = open(resume_params(&host, Arc::clone(&app), token))
            .await
            .expect("open resumed QUIC session");
        let (new_config, first_keyframe, streaming_after, frame_after) =
            resumed_stream(&mut new_events, started).await;
        eprintln!(
            "tcp_dead_old_path_resumes_within_one_second: streaming_after={streaming_after:?} frame_after={frame_after:?}"
        );
        assert!(new.resumed(), "new session reports resumed");
        assert!(streaming_after <= Duration::from_secs(1));
        assert!(frame_after <= Duration::from_secs(2));
        assert_eq!(new_config.epoch, old_config.epoch + 1);
        assert!(first_keyframe, "first resumed frame is keyframe");

        new.close();
        old.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn live_old_connection_still_gets_replaced_goodbye() {
    within_budget(async {
        let host = TestHost::start().await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let (old, mut old_events) = open(host_params(&host, Arc::clone(&app)))
            .await
            .expect("open old direct session");
        initial_stream(&mut old_events).await;
        let token = old.resume_token().expect("old session has resume token");
        let (new, mut new_events) = open(resume_params(&host, Arc::clone(&app), token))
            .await
            .expect("open replacement session");
        let _ = initial_stream(&mut new_events).await;
        let ended = timeout(Duration::from_secs(3), async {
            loop {
                if let SessionEvent::Ended { kind, .. } = old_events
                    .recv()
                    .await
                    .expect("old event stream remains open")
                {
                    break kind;
                }
            }
        })
        .await
        .expect("old session receives replacement ending");
        assert!(
            matches!(
                ended,
                EndReason::Goodbye(pb::goodbye::Reason::Replaced) | EndReason::Close(3)
            ),
            "old session ended with {ended:?}"
        );

        new.close();
        old.close();
        host.stop().await;
    })
    .await;
}

fn host_params(host: &TestHost, app: Arc<rsp_transport::identity::LocalIdentity>) -> SessionParams {
    params(host, app, &[host.quic_addr])
}
