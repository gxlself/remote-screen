//! External tc netem profiles; ordinary CI runs the same assertions on loopback.

use std::time::Duration;

use host_fake::faultnet::FaultProxy;
use host_fake::harness::{app_identity, open, params, run_for, FrameReport, TestHost};
use rsp_client::session::{EndReason, EventStream, SessionEvent};
use rsp_core::errors::ErrorCode;
use rsp_core::{pb, trust::Permission};
use tokio::time::{timeout_at, Instant};

fn profile() -> String {
    std::env::var("RS_NETEM_PROFILE").unwrap_or_else(|_| "none".into())
}

#[derive(Default)]
struct Frames {
    at: Vec<Instant>,
    ended: Option<(Option<ErrorCode>, EndReason, String)>,
    closed: bool,
    errors: Vec<(ErrorCode, String)>,
}

impl Frames {
    fn record(&mut self, event: SessionEvent) {
        match event {
            SessionEvent::Frame(_) => self.at.push(Instant::now()),
            SessionEvent::Ended { code, kind, reason } => {
                self.ended = Some((code, kind, reason));
            }
            SessionEvent::Error { code, message } => self.errors.push((code, message)),
            _ => {}
        }
    }

    async fn until(&mut self, events: &mut EventStream, duration: Duration) {
        let deadline = Instant::now() + duration;
        while self.ended.is_none() && !self.closed {
            match timeout_at(deadline, events.recv()).await {
                Ok(Some(event)) => self.record(event),
                Ok(None) => self.closed = true,
                Err(_) => break,
            }
        }
    }

    fn report(&self, started: Instant) -> FrameReport {
        let max_gap = self
            .at
            .windows(2)
            .map(|pair| pair[1] - pair[0])
            .max()
            .unwrap_or_default();
        FrameReport {
            frames: self.at.len() as u64,
            first_frame: self.at.first().map(|at| *at - started),
            max_gap,
            ended: self.ended.as_ref().map(|(code, kind, _)| (*code, *kind)),
            errors: self.errors.clone(),
            ..FrameReport::default()
        }
    }

    fn max_gap_end(&self) -> Option<Instant> {
        self.at
            .windows(2)
            .enumerate()
            .max_by_key(|(_, pair)| pair[1] - pair[0])
            .map(|(_, pair)| pair[1])
    }
}

#[test]
fn netem_profile_name_is_known() {
    let profile = profile();
    if profile == "none" && std::env::var("RS_REQUIRE_NETEM").as_deref() == Ok("1") {
        panic!("RS_REQUIRE_NETEM=1 but RS_NETEM_PROFILE=none (netns unavailable?)");
    }
    assert!(
        matches!(
            profile.as_str(),
            "none"
                | "wifi-good"
                | "wifi-busy"
                | "5g-city"
                | "derp"
                | "weak"
                | "lossy"
                | "udp-blocked"
                | "handover"
                | "bw-step"
        ),
        "unknown RS_NETEM_PROFILE: {profile}",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn netem_session_meets_profile() {
    let profile = profile();
    let observation = if profile == "handover" {
        Duration::from_secs(70)
    } else {
        Duration::from_secs(8)
    };
    let timeout_limit = if profile == "handover" {
        Duration::from_secs(100)
    } else {
        Duration::from_secs(30)
    };
    tokio::time::timeout(timeout_limit, async {
        let host = TestHost::start().await;
        let id = app_identity();
        host.pair(&id, Permission::Control);
        // Straight-through forwarding provides one advertised port for both
        // protocols, so the external UDP firewall can exercise TCP fallback.
        let proxy = FaultProxy::start(host.quic_addr, host.tcp_addr, 0xa17a_0017)
            .await
            .expect("start same-port loopback proxy");
        let mut options = params(&host, id, &[proxy.addr()]);
        // Stay below the slowest profile, including both loopback proxy hops.
        options.start.max_width = 1280;
        options.start.fps = 30;
        options.start.bitrate_kbps = 1_000;
        let (session, mut events) = open(options).await.expect("open netem session");
        let (report, handover_frames) = if profile == "handover" {
            let started = Instant::now();
            let mut frames = Frames::default();
            frames.until(&mut events, observation).await;
            (frames.report(started), Some(frames))
        } else {
            (run_for(&mut events, observation).await, None)
        };
        let stats = session.stats();
        let transport = stats.hud.as_ref().map(|hud| hud.transport);
        let rtt_ms = stats.rtt_us.map_or_else(
            || "unavailable".into(),
            |us| format!("{:.3}", us as f64 / 1000.0),
        );
        eprintln!(
            "netem {profile}: frames={} max_gap_ms={} rtt_ms={rtt_ms} transport={transport:?}",
            report.frames,
            report.max_gap.as_millis(),
        );
        eprintln!(
            "netem {profile}: ended={:?} errors={:?} frames_recovered={} frames_lost={} recv_kbps={}",
            report.ended, report.errors, stats.frames_recovered, stats.frames_lost, stats.recv_kbps,
        );
        session.close();
        proxy.shutdown().await;
        host.stop().await;

        if profile == "handover" {
            // With only loopback available, this checks that a three-second
            // loss survives the client's ten-second QUIC idle timeout and
            // that video resumes on the same path.
            let frames = handover_frames.expect("handover frame timestamps");
            let max_gap_end = frames.max_gap_end();
            assert!(report.ended.is_none(), "{profile}: {report:?}");
            assert!(report.errors.is_empty(), "{profile}: {report:?}");
            assert!(
                report.max_gap >= Duration::from_millis(2_500),
                "{profile}: no three-second netem interruption observed: {report:?}",
            );
            assert!(
                report.max_gap <= Duration::from_millis(5_000),
                "{profile}: interruption exceeded five seconds: {report:?}",
            );
            let max_gap_end = max_gap_end.expect("handover has a frame after the interruption");
            assert!(
                frames.at.iter().any(|at| *at > max_gap_end),
                "{profile}: no frame after the maximum gap: {report:?}",
            );
            return;
        }
        assert!(report.ended.is_none(), "{profile}: {report:?}");
        assert!(report.errors.is_empty(), "{profile}: {report:?}");
        assert!(report.frames > 1, "{profile}: video did not flow: {report:?}");
        let max_gap_ms = match profile.as_str() {
            "none" | "wifi-good" => Some(300),
            "wifi-busy" | "5g-city" | "derp" | "weak" => Some(500),
            "lossy" | "udp-blocked" | "bw-step" => None,
            _ => panic!("unknown RS_NETEM_PROFILE: {profile}"),
        };
        if let Some(limit) = max_gap_ms {
            assert!(
                report.max_gap <= Duration::from_millis(limit),
                "{profile}: frame gap exceeds {limit} ms: {report:?}",
            );
        }
        match profile.as_str() {
            "none" | "wifi-good" => assert!(
                stats.rtt_us.expect("clock sync reports RTT") < 50_000,
                "{profile}: {stats:?}",
            ),
            "derp" => assert!(
                stats.rtt_us.expect("clock sync reports RTT") >= 100_000,
                "{profile}: {stats:?}",
            ),
            "weak" => assert!(
                stats.rtt_us.expect("clock sync reports RTT") >= 180_000,
                "{profile}: {stats:?}",
            ),
            "lossy" => assert!(
                stats.frames_recovered > 0 || stats.frames_lost == 0,
                "{profile}: {stats:?}",
            ),
            "udp-blocked" => assert_eq!(transport, Some(pb::Transport::Tcp), "{stats:?}"),
            _ => {}
        }
    })
    .await
    .expect("netem session completes within the profile observation timeout");
}
