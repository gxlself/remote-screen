//! IT-08: real loopback QUIC failover, input discard, and old-path retirement.

use std::future::Future;
use std::time::Duration;

use host_fake::faultnet::{Dir, FaultProxy, UdpRule};
use host_fake::harness::{app_identity, open, TestHost, TestHostOptions};
use host_fake::FakePlatform;
use rsp_client::session::{
    EndReason, EventStream, SessionEvent, SessionHandle, SessionParams, SessionStateKind,
};
use rsp_core::hid::{InputMsg, Mods};
use rsp_core::{errors::ErrorCode, pb, trust::Permission};
use rsp_transport::dial::{Candidate, CandidateSource, PathKind};
use tokio::time::{sleep, sleep_until, timeout, timeout_at, Instant};

const INPUT_PERIOD: Duration = Duration::from_millis(50);

async fn within_budget(test: impl Future<Output = ()>) {
    timeout(Duration::from_secs(15), test)
        .await
        .expect("failover scenario, including setup and cleanup, finishes within 15 s");
}

async fn fixture() -> (
    FakePlatform,
    TestHost,
    FaultProxy,
    FaultProxy,
    SessionParams,
) {
    let platform = FakePlatform::new();
    // The default backup requests a 15 s heartbeat, currently capped at 5 s by
    // Conn. Hold synthetic capture during startup so the one-second window
    // after the first actual frame includes that heartbeat, not TLS traffic.
    platform.set_capture_paused(true);
    let host = TestHost::start_with_platform(platform.clone(), TestHostOptions::default()).await;
    let app = app_identity();
    host.pair(&app, Permission::Control);
    let primary = FaultProxy::start(host.quic_addr, host.tcp_addr, 0xa703_a001)
        .await
        .expect("start primary loopback proxy");
    let backup = FaultProxy::start(host.quic_addr, host.tcp_addr, 0xa703_b001)
        .await
        .expect("start backup loopback proxy");
    let mut params = SessionParams::from_shared(
        vec![
            Candidate {
                addr: primary.addr(),
                kind: PathKind::Tailnet,
                source: CandidateSource::Manual,
            },
            Candidate {
                addr: backup.addr(),
                kind: PathKind::Manual,
                source: CandidateSource::Manual,
            },
        ],
        host.fp,
        app,
    );
    params.device_name = "e2e".into();
    params.platform = pb::Platform::Ios;
    params.dial_options.stagger = Duration::ZERO;
    params.dial_options.grace = Duration::from_millis(300);
    params.dial_options.tcp_after = Duration::from_secs(5);
    params.start.fps = 30;
    params.start.bitrate_kbps = 4_000;
    (platform, host, primary, backup, params)
}

#[derive(Default)]
struct Frames {
    at: Vec<Instant>,
    reconnecting: Vec<Instant>,
    ended: Option<(Option<ErrorCode>, EndReason, String)>,
    closed: bool,
    errors: Vec<(ErrorCode, String)>,
}

impl Frames {
    fn record(&mut self, event: SessionEvent, now: Instant) {
        match event {
            SessionEvent::Frame(_) => self.at.push(now),
            SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                self.reconnecting.push(now);
            }
            SessionEvent::Ended { code, kind, reason } => self.ended = Some((code, kind, reason)),
            SessionEvent::Error { code, message } => self.errors.push((code, message)),
            _ => {}
        }
        self.assert_alive();
    }

    async fn until(&mut self, events: &mut EventStream, deadline: Instant) {
        loop {
            match timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => self.record(event, Instant::now()),
                Ok(None) => {
                    self.closed = true;
                    self.assert_alive();
                }
                Err(_) => return,
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

    // Include both boundary gaps, as in faults::Frames::max_gap.
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

async fn first_frame(
    platform: &FakePlatform,
    params: SessionParams,
) -> (SessionHandle, EventStream, Frames) {
    let (session, mut events) = open(params).await.expect("open QUIC session");
    let mut frames = Frames::default();
    frames
        .until(&mut events, Instant::now() + Duration::from_secs(4))
        .await;
    assert!(
        frames.at.is_empty(),
        "startup capture is held until the heartbeat window"
    );
    platform.set_capture_paused(false);
    timeout(Duration::from_secs(2), async {
        while frames.at.is_empty() {
            let event = events
                .recv()
                .await
                .expect("stream stays open before first frame");
            frames.record(event, Instant::now());
        }
    })
    .await
    .expect("session produces a frame within 2 s of enabling capture");
    (session, events, frames)
}

async fn check_backup(backup: &FaultProxy, events: &mut EventStream, frames: &mut Frames) {
    let before = backup.stats();
    frames
        .until(events, Instant::now() + Duration::from_secs(1))
        .await;
    let after = backup.stats();
    assert!(
        after.udp_clients >= 1 && after.to_host.received > before.to_host.received,
        "dial kept no backup: udp_clients={}, to_host.received={} -> {}",
        after.udp_clients,
        before.to_host.received,
        after.to_host.received
    );
    assert_eq!(after.tcp_accepted, 0, "backup uses QUIC");
    eprintln!(
        "backup heartbeat during 1 s after first frame: clients={}, received={} -> {}",
        after.udp_clients, before.to_host.received, after.to_host.received
    );
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

struct Recovery {
    cut: Instant,
    reconnecting: Instant,
    streaming: Instant,
    frame: Instant,
    discarded_usages: Vec<u16>,
    inputs_while_reconnecting: usize,
}

impl Recovery {
    fn report(&self, name: &str) {
        eprintln!(
            "{name}: cut_to_reconnecting={:?} reconnecting_to_streaming={:?} reconnecting_to_frame={:?} cut_to_frame={:?} discarded_keys={} keys_while_reconnecting={}",
            self.reconnecting - self.cut,
            self.streaming - self.reconnecting,
            self.frame - self.reconnecting,
            self.frame - self.cut,
            self.discarded_usages.len(),
            self.inputs_while_reconnecting,
        );
    }

    fn assert_budget(&self, session: &SessionHandle) {
        assert!(
            session.resumed(),
            "backup resumes the retained Host session"
        );
        assert!(
            self.reconnecting - self.cut >= Duration::from_millis(1_400),
            "Reconnecting must follow the 1.5 s liveness deadline"
        );
        // A7: cut to new-path frame. IT-08: Reconnecting to new-path frame.
        assert!(
            self.frame - self.cut <= Duration::from_secs(2),
            "cut to first new-path frame exceeds 2 s: {:?}",
            self.frame - self.cut
        );
        assert!(
            self.frame - self.reconnecting <= Duration::from_secs(1),
            "Reconnecting to first new-path frame exceeds 1 s: {:?}",
            self.frame - self.reconnecting
        );
    }
}

async fn switch(
    primary: &FaultProxy,
    backup: &FaultProxy,
    input: Option<&SessionHandle>,
    events: &mut EventStream,
    frames: &mut Frames,
) -> Recovery {
    // The 80 ms return delay leaves a full 50 ms input tick after Host
    // admission and before the App sees AuthResult. Removing both App gates
    // must then expose injected input instead of being masked by the Host gate.
    // Both delays are included in the unchanged 2 s / 1 s budgets.
    for (dir, delay_ms) in [(Dir::ToHost, 20), (Dir::ToClient, 80)] {
        backup.set_udp(
            dir,
            UdpRule {
                delay_ms,
                ..UdpRule::default()
            },
        );
    }
    while let Ok(event) = events.try_recv() {
        frames.record(event, Instant::now());
    }
    assert!(
        frames.reconnecting.is_empty(),
        "no reconnect before cutting A"
    );
    primary.cut();
    let cut = Instant::now();
    let mut reconnecting = None;
    let mut streaming = None;
    let mut next_send = cut;
    let mut usage = 0x10_u16;
    let mut discarded_usages = Vec::new();
    let mut inputs_while_reconnecting = 0;
    loop {
        tokio::select! {
            biased;
            _ = sleep_until(cut + Duration::from_secs(3)) => {
                panic!("no recovered frame: cut elapsed={:?}, reconnecting={:?}, streaming={:?}",
                    cut.elapsed(), reconnecting.map(|at| at - cut), streaming.map(|at| at - cut));
            }
            event = events.recv() => {
                let event = event.expect("session event stream stays open during failover");
                let now = Instant::now();
                match &event {
                    SessionEvent::StateChanged(SessionStateKind::Reconnecting) => {
                        assert!(reconnecting.replace(now).is_none(), "no second Reconnecting");
                    }
                    SessionEvent::StateChanged(SessionStateKind::Streaming) => {
                        assert!(reconnecting.is_some(), "Reconnecting precedes resumed Streaming");
                        streaming.get_or_insert(now);
                    }
                    SessionEvent::Frame(_) if streaming.is_some() => {
                        frames.record(event, now);
                        return Recovery {
                            cut,
                            reconnecting: reconnecting.expect("Reconnecting observed"),
                            streaming: streaming.expect("resumed Streaming observed"),
                            frame: now,
                            discarded_usages,
                            inputs_while_reconnecting,
                        };
                    }
                    SessionEvent::Ended { .. } => {
                        eprintln!("failover ended: cut_elapsed={:?}, reconnecting_elapsed={:?}",
                            cut.elapsed(), reconnecting.map(|at| now - at));
                    }
                    _ => {}
                }
                frames.record(event, now);
            }
            _ = sleep_until(next_send), if input.is_some() && streaming.is_none() => {
                send_key(input.expect("input sender enabled"), usage);
                discarded_usages.push(usage);
                inputs_while_reconnecting += usize::from(reconnecting.is_some());
                usage += 1;
                // Keep the post-recovery sentinel usages distinct. The index
                // i in 0x10+i remains increasing, skipping those three values.
                if usage == 0x20 {
                    usage = 0x23;
                }
                next_send += INPUT_PERIOD;
            }
        }
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it08_primary_cut_switches_to_backup() {
    within_budget(async {
        let (platform, host, primary, backup, params) = fixture().await;
        let (session, mut events, mut frames) = first_frame(&platform, params).await;
        check_backup(&backup, &mut events, &mut frames).await;
        for usage in [0x04, 0x05] {
            send_key(&session, usage);
        }
        let baseline: Vec<_> = [0x04, 0x05].into_iter().flat_map(key_pair).collect();
        timeout(Duration::from_secs(1), async {
            while keyboard(&platform).len() < baseline.len() {
                sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("baseline keys are injected before cutting A");
        assert_eq!(keyboard(&platform), baseline);
        let releases = platform.release_count();
        let recovery = switch(&primary, &backup, Some(&session), &mut events, &mut frames).await;
        recovery.report("it08_primary_cut_switches_to_backup");
        recovery.assert_budget(&session);
        assert!(
            recovery.inputs_while_reconnecting > 0,
            "exercise input during Reconnecting"
        );
        for usage in 0x20..=0x22 {
            send_key(&session, usage);
        }
        frames
            .until(&mut events, Instant::now() + Duration::from_secs(1))
            .await;
        let expected: Vec<_> = [0x04, 0x05, 0x20, 0x21, 0x22]
            .into_iter()
            .flat_map(key_pair)
            .collect();
        let injected = keyboard(&platform);
        assert_eq!(
            injected, expected,
            "only baseline and restored keys are injected, exactly once and in order"
        );
        for usage in &recovery.discarded_usages {
            assert!(
                !injected
                    .iter()
                    .any(|msg| matches!(msg, InputMsg::Key { usage: got, .. } if got == usage)),
                "input sent during the cut was replayed: usage={usage:#x}"
            );
        }
        assert!(
            platform.release_count() > releases,
            "recovery releases held input"
        );
        assert_eq!(frames.reconnecting.len(), 1, "one reconnect only");
        session.close();
        primary.shutdown().await;
        backup.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it08_old_path_is_abandoned_and_stream_is_stable() {
    within_budget(async {
        let (platform, host, primary, backup, params) = fixture().await;
        let (session, mut events, mut frames) = first_frame(&platform, params).await;
        check_backup(&backup, &mut events, &mut frames).await;
        let recovery = switch(&primary, &backup, None, &mut events, &mut frames).await;
        recovery.report("it08_old_path_is_abandoned_and_stream_is_stable");
        recovery.assert_budget(&session);
        frames.until(&mut events, recovery.frame + Duration::from_millis(500)).await;
        let old_received = primary.stats().to_host.received;
        let before = backup.stats().to_client.forwarded;
        let mut previous = before;
        let since = Instant::now();
        for second in 1..=3 {
            frames.until(&mut events, since + Duration::from_secs(second)).await;
            assert_eq!(primary.stats().to_host.received, old_received,
                "old path sends no packets after 500 ms of drain");
            let forwarded = backup.stats().to_client.forwarded;
            assert!(forwarded > previous,
                "B keeps forwarding datagrams after recovery");
            previous = forwarded;
            assert_eq!(frames.reconnecting.len(), 1, "no second Reconnecting");
        }
        let until = Instant::now();
        let max_gap = frames.max_gap(since, until);
        eprintln!("it08_old_path_is_abandoned_and_stream_is_stable: old_received={old_received} -> {} B_forwarded_delta={} stable_frames={} max_gap={max_gap:?}",
            primary.stats().to_host.received, backup.stats().to_client.forwarded - before,
            frames.at.iter().filter(|at| (since..=until).contains(at)).count());
        assert!(max_gap <= Duration::from_millis(500), "stable frame max_gap={max_gap:?} exceeds 500 ms");
        session.close();
        primary.shutdown().await;
        backup.shutdown().await;
        host.stop().await;
    }).await;
}
