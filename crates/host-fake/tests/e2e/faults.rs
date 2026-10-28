//! Session faults over real loopback transports; recovery/rate convergence is
//! deliberately limited to the behavior currently wired into the session.

use std::future::Future;
use std::time::Duration;

use host_fake::faultnet::{Dir, FaultProxy, TcpMode, TcpRule, UdpRule};
use host_fake::harness::{app_identity, open, params, wait_end, TestHost, TestHostOptions};
use host_fake::FakePlatform;
use rsp_client::session::{EndReason, EventStream, SessionEvent, SessionParams, SessionStateKind};
use rsp_core::hid::{InputMsg, Mods};
use rsp_core::{errors::ErrorCode, pb, trust::Permission};
use tokio::time::{sleep, sleep_until, timeout, timeout_at, Instant};

async fn within_budget(test: impl Future<Output = ()>) {
    timeout(Duration::from_secs(15), test)
        .await
        .expect("fault scenario, including setup and cleanup, finishes within 15 s");
}

async fn fixture(seed: u64) -> (FakePlatform, TestHost, FaultProxy, SessionParams) {
    let platform = FakePlatform::new();
    let host = TestHost::start_with_platform(platform.clone(), TestHostOptions::default()).await;
    let app = app_identity();
    host.pair(&app, Permission::Control);
    let mut bind_attempts = 0;
    let proxy = loop {
        bind_attempts += 1;
        match FaultProxy::start(host.quic_addr, host.tcp_addr, seed).await {
            Ok(proxy) => break proxy,
            // The proxy allocates UDP :0 before binding the same TCP port.
            // That number can already belong to an unrelated TCP listener.
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && bind_attempts < 8 => {
                tokio::task::yield_now().await;
            }
            Err(error) => {
                panic!("start loopback fault proxy after {bind_attempts} attempts: {error}")
            }
        }
    };
    let mut options = params(&host, app, &[proxy.addr()]);
    // These scenarios exercise QUIC unless explicitly testing TCP fallback.
    options.dial_options.tcp_after = Duration::from_secs(5);
    options.start.fps = 30;
    options.start.bitrate_kbps = 4_000;
    (platform, host, proxy, options)
}

#[derive(Default)]
struct Frames {
    at: Vec<Instant>,
    fps: Option<u32>,
    ended: Option<(Option<ErrorCode>, EndReason, String)>,
    closed: bool,
    errors: Vec<(ErrorCode, String)>,
}

impl Frames {
    fn record(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Frame(_) => self.at.push(Instant::now()),
            SessionEvent::StreamConfig(config) => self.fps = Some(config.fps),
            SessionEvent::Ended { code, kind, reason } => {
                self.ended = Some((code, kind, reason));
            }
            SessionEvent::Error { code, message } => self.errors.push((code, message)),
            _ => {}
        }
    }

    async fn until(&mut self, events: &mut EventStream, deadline: Instant) {
        while self.ended.is_none() && !self.closed {
            match timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => self.record(event),
                Ok(None) => self.closed = true,
                Err(_) => break,
            }
        }
    }

    fn assert_alive(&self) {
        assert!(
            self.ended.is_none() && !self.closed,
            "session stays alive: ended={:?}, closed={}, errors={:?}",
            self.ended,
            self.closed,
            self.errors
        );
    }

    fn last(&self) -> Instant {
        *self.at.last().expect("session has produced a frame")
    }

    fn count(&self, since: Instant, until: Instant) -> usize {
        self.at
            .iter()
            .filter(|at| (since..=until).contains(at))
            .count()
    }

    // Include the leading/trailing silence: a freeze must not disappear just
    // because the observation window ended before the next frame arrived.
    fn max_gap(&self, since: Instant, until: Instant) -> Duration {
        let mut previous = since;
        let mut largest = Duration::ZERO;
        for at in self
            .at
            .iter()
            .copied()
            .filter(|at| (since..=until).contains(at))
        {
            largest = largest.max(at - previous);
            previous = at;
        }
        largest.max(until - previous)
    }
}

async fn first_frame(events: &mut EventStream) -> Frames {
    let mut frames = Frames::default();
    timeout(Duration::from_secs(2), async {
        while frames.at.is_empty() {
            frames.record(events.recv().await.expect("stream stays open before frame"));
            frames.assert_alive();
        }
    })
    .await
    .expect("session produces its first frame within 2 s");
    frames
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn hard_cut_is_detected() {
    within_budget(async {
        let (_, host, proxy, options) = fixture(0x00a1_7f01).await;
        let (session, mut events) = open(options).await.expect("open QUIC session");
        first_frame(&mut events).await;
        let cut_at = Instant::now();
        proxy.cut();
        let mut branch = None;
        let mut detected_after = None;
        let mut buffered_frames = 0;
        let mut end = None;
        while let Ok(event) = timeout_at(cut_at + Duration::from_secs(12), events.recv()).await {
            let Some(event) = event else { break };
            match event {
                SessionEvent::Frame(_) => {
                    assert!(
                        cut_at.elapsed() < Duration::from_millis(500),
                        "new frame after the hard-cut drain window"
                    );
                    buffered_frames += 1;
                }
                SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                    if branch.is_none() {
                        branch = Some("Reconnecting");
                        detected_after = Some(cut_at.elapsed());
                    }
                }
                SessionEvent::Ended { code, kind, reason } => {
                    if branch.is_none() {
                        assert_eq!(kind, EndReason::ConnectionLost, "{code:?}: {reason}");
                        branch = Some("Ended(ConnectionLost)");
                        detected_after = Some(cut_at.elapsed());
                    }
                    end = Some((code, kind));
                }
                _ => {}
            }
        }
        eprintln!(
            "hard_cut_is_detected: branch={branch:?} detected_after={detected_after:?} buffered_frames={buffered_frames} frames_after_500ms=0 end={end:?} elapsed={:?}",
            cut_at.elapsed()
        );
        assert!(branch.is_some(), "hard cut detected within 12 s");
        assert!(detected_after.expect("detection time") <= Duration::from_secs(12));
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn blip_500ms_survives() {
    within_budget(async {
        let (_, host, proxy, options) = fixture(0x00a1_7f02).await;
        let (session, mut events) = open(options).await.expect("open QUIC session");
        let mut frames = first_frame(&mut events).await;
        let start = frames.last();
        let cut_at = Instant::now();
        proxy.cut();
        frames
            .until(&mut events, cut_at + Duration::from_millis(500))
            .await;
        proxy.restore();
        let restored = Instant::now();
        let deadline = restored + Duration::from_secs(5);
        frames.until(&mut events, deadline).await;
        let resumed_after = frames
            .at
            .iter()
            .find(|at| **at >= restored)
            .map(|at| *at - restored);
        let max_gap = frames.max_gap(start, deadline);
        eprintln!(
            "blip_500ms_survives: cut={:?} resumed_after={resumed_after:?} frames={} max_gap={max_gap:?} survived=5s",
            restored - cut_at,
            frames.at.len()
        );
        frames.assert_alive();
        assert!(resumed_after.is_some_and(|gap| gap <= Duration::from_secs(2)));
        assert!(max_gap <= Duration::from_millis(2_500));
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn uplink_loss_input_still_delivered() {
    within_budget(async {
        let (platform, host, proxy, options) = fixture(0x00a1_7f03).await;
        let (session, mut events) = open(options).await.expect("open QUIC session");
        let mut frames = first_frame(&mut events).await;
        let last_before_loss = frames.last();
        let before_loss = proxy.stats().to_host;
        proxy.set_udp(
            Dir::ToHost,
            UdpRule {
                // Deterministic 20% loss: one of every five packets, with the
                // pattern aligned to the post-handshake receive counter.
                burst: Some((5, 1)),
                skip_first: u32::try_from(before_loss.received)
                    .expect("isolated fixture packet count fits u32"),
                ..UdpRule::default()
            },
        );
        let expected: Vec<_> = (0..10)
            .flat_map(|key| {
                [true, false].map(|down| InputMsg::Key {
                    usage: 0x04 + key,
                    down,
                    mods: Mods::NONE,
                })
            })
            .collect();
        let started = Instant::now();
        let deadline = started + Duration::from_secs(5);
        let traffic = async {
            for (index, input) in expected.iter().enumerate() {
                sleep_until(started + Duration::from_millis(index as u64 * 100)).await;
                session.send_input(input.clone()).expect("encode key input");
            }
            let sent_after = started.elapsed();
            assert!(sent_after <= Duration::from_secs(2));
            let delivered = timeout(Duration::from_secs(3), async {
                // injected_events() is the M0 pointer-only view. Keyboard
                // injection is recorded by the RSP/1 injected_msgs() accessor.
                while platform.injected_msgs().len() < expected.len() {
                    sleep(Duration::from_millis(10)).await;
                }
            })
            .await
            .is_ok();
            (sent_after, started.elapsed(), delivered)
        };
        let ((sent_after, delivered_after, delivered), ()) =
            tokio::join!(traffic, frames.until(&mut events, deadline));
        let injected = platform.injected_msgs();
        let max_gap = frames.max_gap(last_before_loss, Instant::now());
        let after_loss = proxy.stats().to_host;
        let dropped = after_loss.dropped - before_loss.dropped;
        let received = after_loss.received - before_loss.received;
        eprintln!(
            "uplink_loss_input_still_delivered: injected={} sent_after={sent_after:?} delivered_after={delivered_after:?} delivered={delivered} frames={} max_gap={max_gap:?} uplink_dropped={dropped} uplink_received={received} loss_pattern=1/5 ended={:?}",
            injected.len(),
            frames.at.len(),
            frames.ended
        );
        frames.assert_alive();
        assert!(delivered, "all reliable keys injected within 3 s after sending");
        assert_eq!(injected, expected, "all 20 keys injected exactly once in order");
        assert!(dropped > 0, "uplink loss was exercised");
        assert!(frames.count(started, deadline) > 0);
        assert!(max_gap <= Duration::from_secs(1));
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn latency_jitter_rtt() {
    within_budget(async {
        let (_, host, proxy, mut options) = fixture(0x00a1_7f04).await;
        // Keep a small stream so the RTT scenario does not require the Host
        // congestion-control feedback hook, which is not wired yet.
        options.start.fps = 15;
        options.start.bitrate_kbps = 500;
        let (session, mut events) = open(options).await.expect("open QUIC session");
        let mut frames = first_frame(&mut events).await;
        let fps = frames.fps.expect("negotiated stream frame rate");
        assert!(fps > 0);
        for dir in [Dir::ToHost, Dir::ToClient] {
            proxy.set_udp(
                dir,
                UdpRule {
                    delay_ms: 100,
                    jitter_ms: 20,
                    ..UdpRule::default()
                },
            );
        }
        let started = Instant::now();
        let deadline = started + Duration::from_secs(4);
        frames.until(&mut events, deadline).await;
        let stats = session.stats();
        let proxy_stats = proxy.stats();
        let rtt_us = stats.rtt_us;
        let count = frames.count(started, deadline);
        let minimum = (u64::from(fps) * 4 * 8).div_ceil(10);
        eprintln!(
            "latency_jitter_rtt: rtt_us={rtt_us:?} frames={count} fps={fps} minimum={minimum} start_kbps=500 max_gap={:?} lost={} proxy_udp_dropped=[{},{}] elapsed={:?}",
            frames.max_gap(started, deadline),
            stats.frames_lost,
            proxy_stats.to_host.dropped,
            proxy_stats.to_client.dropped,
            started.elapsed()
        );
        frames.assert_alive();
        assert!(rtt_us.is_some_and(|rtt| (160_000..=400_000).contains(&rtt)));
        assert!(count as u64 >= minimum);
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn host_abort_ends_session() {
    within_budget(async {
        let (_, host, proxy, options) = fixture(0x00a1_7f05).await;
        let (session, mut events) = open(options).await.expect("open QUIC session");
        first_frame(&mut events).await;
        let aborted = Instant::now();
        host.abort();
        let (code, kind, reason) = wait_end(&mut events, Duration::from_secs(12))
            .await
            .expect("Host abort ends the session within 12 s");
        let ended_after = aborted.elapsed();
        let next = timeout(Duration::from_millis(500), events.recv())
            .await
            .expect("event stream closes after Ended");
        eprintln!(
            "host_abort_ends_session: code={code:?} kind={kind:?} reason={reason:?} ended_after={ended_after:?} stream_closed={}",
            next.is_none()
        );
        assert!(
            code.is_none()
                || matches!(
                    kind,
                    EndReason::ConnectionLost | EndReason::Goodbye(pb::goodbye::Reason::HostShutdown)
                )
        );
        assert!(next.is_none(), "Ended is the final event: {next:?}");
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_slow_link_keeps_streaming() {
    within_budget(async {
        let (_, host, proxy, mut options) = fixture(0x00a1_7f06).await;
        for dir in [Dir::ToHost, Dir::ToClient] {
            proxy.set_udp(
                dir,
                UdpRule {
                    blackhole: true,
                    ..UdpRule::default()
                },
            );
        }
        proxy.set_tcp(TcpRule {
            delay_ms: 50,
            rate_kbps: Some(2_000),
            ..TcpRule::default()
        });
        options.dial_options.tcp_after = Duration::from_millis(250);
        let (session, mut events) = open(options).await.expect("open TCP fallback session");
        let started = Instant::now();
        let deadline = started + Duration::from_secs(6);
        let mut frames = Frames::default();
        frames.until(&mut events, deadline).await;
        let stats = session.stats();
        let proxy_stats = proxy.stats();
        eprintln!(
            "tcp_slow_link_keeps_streaming: frames={} recv_kbps={} max_gap={:?} transport={:?} tcp_bytes={} elapsed={:?} ended={:?} errors={:?}",
            frames.at.len(),
            stats.recv_kbps,
            frames.max_gap(started, Instant::now()),
            stats.hud.map(|hud| hud.transport),
            proxy_stats.tcp_bytes_to_client,
            started.elapsed(),
            frames.ended,
            frames.errors
        );
        frames.assert_alive();
        assert!(frames.at.len() >= 20);
        assert_eq!(stats.hud.expect("TCP HUD stats").transport, pb::Transport::Tcp);
        assert!(proxy_stats.to_host.dropped > 0, "UDP blackhole was exercised");
        assert!(proxy_stats.tcp_bytes_to_client > 0);
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn bandwidth_step_survives() {
    within_budget(async {
        let (_, host, proxy, mut options) = fixture(0x00a1_7f07).await;
        // Exercise shaping at a LAN rate above the 4 Mbps floor; the controller
        // clamps LAN targets to 4 Mbps, so a 3 Mbps stage cannot be observed
        // (03 §7.3). IT-18 bitrate
        // convergence needs the Host feedback hook and is not asserted here.
        options.start.bitrate_kbps = 4_000;
        proxy.set_udp(
            Dir::ToClient,
            UdpRule {
                rate_kbps: Some(20_000),
                ..UdpRule::default()
            },
        );
        let (session, mut events) = open(options).await.expect("open QUIC session");
        let mut frames = first_frame(&mut events).await;
        let started = Instant::now();
        let mut recv_kbps = Vec::new();
        let mut low_started = started;
        let mut low_ended = started;
        // IT-18's lightweight 3 s / 5 s / 2 s stages; record convergence only.
        for second in 0..10 {
            if second == 3 || second == 8 {
                proxy.set_udp(
                    Dir::ToClient,
                    UdpRule {
                        rate_kbps: Some(if second == 3 { 6_000 } else { 20_000 }),
                        ..UdpRule::default()
                    },
                );
                if second == 3 {
                    low_started = Instant::now();
                } else {
                    low_ended = Instant::now();
                }
            }
            frames
                .until(&mut events, started + Duration::from_secs(second + 1))
                .await;
            recv_kbps.push(session.stats().recv_kbps);
        }
        let low_frames = frames.count(low_started, low_ended);
        let low_max_gap = frames.max_gap(low_started, low_ended);
        eprintln!(
            "bandwidth_step_survives: recv_kbps_each_second={recv_kbps:?} limits_kbps=20000x3,6000x5,20000x2 start_kbps=4000 frames={} low_frames={low_frames} low_max_gap={low_max_gap:?} elapsed={:?}",
            frames.at.len(),
            started.elapsed()
        );
        frames.assert_alive();
        assert!(low_frames > 0);
        assert!(low_max_gap <= Duration::from_millis(1_500));
        assert!(frames.count(low_ended, started + Duration::from_secs(10)) > 0);
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn drop_handshake_then_recover() {
    within_budget(async {
        let (_, host, proxy, mut options) = fixture(0x00a1_7f08).await;
        for dir in [Dir::ToHost, Dir::ToClient] {
            proxy.set_udp(
                dir,
                UdpRule {
                    skip_first: 0,
                    blackhole: true,
                    ..UdpRule::default()
                },
            );
        }
        proxy.set_tcp(TcpRule {
            mode: TcpMode::Blackhole,
            ..TcpRule::default()
        });
        options.dial_options.timeout = Duration::from_secs(4);
        let started = Instant::now();
        let restore = async {
            sleep_until(started + Duration::from_secs(1)).await;
            let dropped = proxy.stats().to_host.dropped;
            proxy.restore();
            assert!(dropped > 0, "initial handshake packets were blackholed");
            dropped
        };
        let connect = async {
            let result = timeout(Duration::from_secs(4), open(options))
                .await
                .expect("open completes within the 4 s dial timeout")
                .expect("handshake retransmission succeeds after restore");
            (result, started.elapsed())
        };
        let (((session, mut events), opened_after), dropped) = tokio::join!(connect, restore);
        let frames = first_frame(&mut events).await;
        eprintln!(
            "drop_handshake_then_recover: opened_after={opened_after:?} first_frame_after={:?} initial_dropped={dropped} frames={}",
            started.elapsed(),
            frames.at.len()
        );
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}
