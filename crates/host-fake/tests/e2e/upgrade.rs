//! TCP-to-QUIC upgrade of a live session over the real host-fake listeners
//! (protocol §1.3, §10.4). UPGRADE_EVERY is not configurable: these tests
//! wait for it in real time.

use std::future::Future;
use std::ops::RangeBounds;
use std::time::Duration;

use host_fake::faultnet::{Dir, FaultProxy, UdpRule};
use host_fake::harness::{app_identity, open, params, TestHost, TestHostOptions};
use host_fake::FakePlatform;
use rsp_client::session::{
    EndReason, EventStream, SessionEvent, SessionHandle, SessionParams, SessionStateKind,
};
use rsp_core::hid::{InputMsg, Mods};
use rsp_core::{errors::ErrorCode, pb, trust::Permission};
use tokio::time::{timeout, timeout_at, Instant};

const SEED: u64 = 0xa7_e2e4_0001;
const TCP_AFTER: Duration = Duration::from_millis(250);
const POLL: Duration = Duration::from_millis(10);
const WINDOW: Duration = Duration::from_secs(2);

async fn bounded(test: impl Future<Output = ()>) {
    timeout(Duration::from_secs(75), test)
        .await
        .expect("upgrade scenario, including setup and cleanup, finishes within 75 s");
}

fn udp_blackhole() -> UdpRule {
    UdpRule {
        blackhole: true,
        ..UdpRule::default()
    }
}

/// Both UDP directions start blackholed, so the session comes up on TCP.
async fn fixture() -> (FakePlatform, TestHost, FaultProxy, SessionParams) {
    let platform = FakePlatform::new();
    let host = TestHost::start_with_platform(platform.clone(), TestHostOptions::default()).await;
    let proxy = FaultProxy::start(host.quic_addr, host.tcp_addr, SEED)
        .await
        .expect("start same-port UDP/TCP fault proxy");
    proxy.set_udp(Dir::ToHost, udp_blackhole());
    proxy.set_udp(Dir::ToClient, udp_blackhole());
    let app = app_identity();
    host.pair(&app, Permission::Control);
    let mut p = params(&host, app, &[proxy.addr()]);
    p.dial_options.tcp_after = TCP_AFTER;
    p.start.fps = 30;
    p.start.bitrate_kbps = 4_000;
    (platform, host, proxy, p)
}

fn key_pair(usage: u16) -> [InputMsg; 2] {
    [true, false].map(|down| InputMsg::Key {
        usage,
        down,
        mods: Mods::NONE,
    })
}

fn send_key(session: &SessionHandle, usage: u16) {
    for input in key_pair(usage) {
        session.send_input(input).expect("encode keyboard input");
    }
}

fn keyboard(platform: &FakePlatform) -> Vec<InputMsg> {
    platform
        .injected_msgs()
        .into_iter()
        .filter(|msg| matches!(msg, InputMsg::Key { .. }))
        .collect()
}

/// Every session event of interest, stamped on arrival.
#[derive(Default)]
struct Timeline {
    frames: Vec<(Instant, u8)>,
    configs: Vec<(Instant, u32)>,
    transport: Option<pb::Transport>,
    quic_at: Option<Instant>,
    reconnecting: Vec<Instant>,
    ended: Option<(Option<ErrorCode>, EndReason, String)>,
    closed: bool,
    errors: Vec<(ErrorCode, String)>,
}

impl Timeline {
    fn record(&mut self, event: SessionEvent, now: Instant) {
        match event {
            SessionEvent::Frame(frame) => self.frames.push((now, frame.frame.epoch)),
            SessionEvent::StreamConfig(config) => self.configs.push((now, config.epoch)),
            SessionEvent::Stats(stats) => {
                if let Some(hud) = stats.hud {
                    self.transport = Some(hud.transport);
                    if hud.transport == pb::Transport::Quic {
                        self.quic_at.get_or_insert(now);
                    }
                }
            }
            SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                self.reconnecting.push(now);
            }
            SessionEvent::Ended { code, kind, reason } => self.ended = Some((code, kind, reason)),
            SessionEvent::Error { code, message } => self.errors.push((code, message)),
            _ => {}
        }
    }

    /// Records events until `done` holds, the deadline passes, or the session
    /// ends. Returns whether `done` held.
    async fn until(
        &mut self,
        events: &mut EventStream,
        deadline: Instant,
        mut done: impl FnMut(&Self) -> bool,
    ) -> bool {
        while !done(self) {
            let now = Instant::now();
            if self.ended.is_some() || self.closed || now >= deadline {
                return false;
            }
            match timeout_at(deadline.min(now + POLL), events.recv()).await {
                Ok(Some(event)) => self.record(event, Instant::now()),
                Ok(None) => self.closed = true,
                Err(_) => {}
            }
        }
        true
    }

    fn first_frame(&self) -> Option<Instant> {
        self.frames.first().map(|(at, _)| *at)
    }

    fn frames_in(&self, range: impl RangeBounds<Instant>) -> usize {
        self.frames
            .iter()
            .filter(|(at, _)| range.contains(at))
            .count()
    }

    fn max_gap(&self, since: Instant, until: Instant) -> Duration {
        let mut previous = since;
        let mut largest = Duration::ZERO;
        for (at, _) in self
            .frames
            .iter()
            .filter(|(at, _)| (since..=until).contains(at))
        {
            largest = largest.max(*at - previous);
            previous = *at;
        }
        largest.max(until - previous)
    }

    fn assert_alive(&self, origin: Instant) {
        assert!(
            self.ended.is_none() && !self.closed && self.reconnecting.is_empty(),
            "session stays alive without Reconnecting: {}",
            self.summary(origin)
        );
    }

    fn summary(&self, origin: Instant) -> String {
        let since = |at: Instant| at.saturating_duration_since(origin);
        format!(
            "frames={} configs={:?} transport={:?} quic_at={:?} reconnecting={:?} ended={:?} closed={} errors={:?}",
            self.frames.len(),
            self.configs
                .iter()
                .map(|(at, epoch)| (since(*at), *epoch))
                .collect::<Vec<_>>(),
            self.transport,
            self.quic_at.map(since),
            self.reconnecting.iter().copied().map(since).collect::<Vec<_>>(),
            self.ended,
            self.closed,
            self.errors,
        )
    }
}

/// Opens the session and waits for its first frame and HUD sample, on TCP.
async fn tcp_baseline(
    platform: &FakePlatform,
    p: SessionParams,
) -> (SessionHandle, EventStream, Timeline, Instant) {
    let (session, mut events) = timeout(Duration::from_secs(3), open(p))
        .await
        .expect("TCP fallback opens within 3 s")
        .expect("TCP fallback connects");
    let mut run = Timeline::default();
    run.until(
        &mut events,
        Instant::now() + Duration::from_secs(3),
        |run| run.first_frame().is_some() && run.transport.is_some(),
    )
    .await;
    let first = run
        .first_frame()
        .unwrap_or_else(|| panic!("no first frame within 3 s: {}", run.summary(Instant::now())));
    run.assert_alive(first);
    assert_eq!(
        run.transport,
        Some(pb::Transport::Tcp),
        "baseline HUD transport"
    );
    assert!(!session.resumed(), "baseline is a fresh Host session");
    assert_eq!(platform.active_captures(), 1, "one Host stream on TCP");
    (session, events, run, first)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_fallback_upgrades_to_quic_when_udp_returns() {
    bounded(async {
        let (platform, host, proxy, p) = fixture().await;
        let (session, mut events, mut run, first) = tcp_baseline(&platform, p).await;
        let baseline = proxy.stats();
        proxy.set_udp(Dir::ToHost, UdpRule::default());
        proxy.set_udp(Dir::ToClient, UdpRule::default());
        let restored = Instant::now();

        // A key on TCP must not be injected again after the upgrade.
        send_key(&session, 0x05);
        let tcp_keys = key_pair(0x05).to_vec();
        let injected = run
            .until(&mut events, Instant::now() + WINDOW, |_| {
                keyboard(&platform).len() >= tcp_keys.len()
            })
            .await;
        assert!(injected, "TCP key injected: {:?}", keyboard(&platform));
        assert_eq!(keyboard(&platform), tcp_keys);

        run.until(&mut events, first + Duration::from_secs(40), |run| {
            run.quic_at.is_some()
        })
        .await;
        let switch = run
            .quic_at
            .unwrap_or_else(|| panic!("HUD never reported QUIC: {}", run.summary(first)));
        run.assert_alive(first);
        let after_first = switch - first;
        assert!(
            (Duration::from_secs(29)..=Duration::from_secs(40)).contains(&after_first),
            "HUD switched to QUIC {after_first:?} after the first frame"
        );
        assert!(session.resumed(), "Host resumed the TCP session on QUIC");

        send_key(&session, 0x04);
        let sent = Instant::now();
        let expected: Vec<_> = [0x05, 0x04].into_iter().flat_map(key_pair).collect();
        let injected = run
            .until(&mut events, sent + WINDOW, |_| {
                keyboard(&platform).len() >= expected.len()
            })
            .await;
        let key_latency = sent.elapsed();
        assert!(injected, "QUIC key injected within 2 s: {:?}", keyboard(&platform));

        // Cover the post-switch window and give a duplicate time to appear.
        let end = (switch + WINDOW).max(Instant::now() + Duration::from_secs(1));
        run.until(&mut events, end, |_| false).await;
        run.assert_alive(first);
        let before = run.frames_in(switch - WINDOW..switch);
        let after = run.frames_in(switch..switch + WINDOW);
        let max_gap = run.max_gap(switch - WINDOW, switch + WINDOW);
        let stats = proxy.stats();
        eprintln!(
            "tcp_fallback_upgrades_to_quic_when_udp_returns: restore_after_first={:?} switch_after_first={after_first:?} switch_after_restore={:?} frames_before={before} frames_after={after} max_gap={max_gap:?} key_latency={key_latency:?} resumed={} captures={} timeline: {} baseline_proxy={baseline:?} proxy={stats:?}",
            restored - first,
            switch - restored,
            session.resumed(),
            platform.active_captures(),
            run.summary(first),
        );
        assert_eq!(run.transport, Some(pb::Transport::Quic), "HUD stays on QUIC");
        assert!(before >= 20, "{before} frames in the 2 s before the switch");
        assert!(after >= 20, "{after} frames in the 2 s after the switch");
        assert_eq!(
            keyboard(&platform),
            expected,
            "each key is injected exactly once"
        );
        assert_eq!(
            platform.active_captures(),
            1,
            "the Host streams one session after the upgrade"
        );
        assert!(
            stats.to_host.forwarded > baseline.to_host.forwarded,
            "QUIC packets crossed the restored UDP path"
        );
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_still_blocked_stays_on_tcp() {
    bounded(async {
        let (platform, host, proxy, p) = fixture().await;
        let (session, mut events, mut run, first) = tcp_baseline(&platform, p).await;
        // UDP datagrams the proxy received toward the Host, once per second
        // after the first frame. Every one of them is blackholed.
        let mut received = vec![proxy.stats().to_host.received];
        for second in 1..=65 {
            run.until(&mut events, first + Duration::from_secs(second), |_| false)
                .await;
            received.push(proxy.stats().to_host.received);
        }
        let end = Instant::now();
        let last = run.frames_in(end - WINDOW..=end);
        let stats = proxy.stats();
        eprintln!(
            "udp_still_blocked_stays_on_tcp: to_host_received_per_second={received:?} last_2s_frames={last} resumed={} captures={} timeline: {} proxy={stats:?}",
            session.resumed(),
            platform.active_captures(),
            run.summary(first),
        );
        run.assert_alive(first);
        assert_eq!(run.transport, Some(pb::Transport::Tcp), "HUD stays on TCP");
        assert!(run.quic_at.is_none(), "HUD never reported QUIC");
        assert!(last >= 20, "{last} frames in the last 2 s");
        assert!(!session.resumed(), "no resume without QUIC");
        assert_eq!(stats.to_host.forwarded, 0, "no UDP datagram reached the Host");
        // Upgrade probes start 30 s and 60 s after the session opened; UDP is
        // otherwise idle while TCP carries the session.
        assert_eq!(received[25], received[5], "no UDP before the first probe");
        assert!(received[35] > received[25], "probe near 30 s");
        assert_eq!(received[55], received[35], "no UDP between the probes");
        assert!(received[65] > received[55], "probe near 60 s");
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}
