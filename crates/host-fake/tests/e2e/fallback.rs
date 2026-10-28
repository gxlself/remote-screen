//! Dial fault injection: protocol §10 and release checks IT-04 / IT-05.

use std::future::Future;
use std::time::Duration;

use host_fake::faultnet::{Dir, FaultProxy, TcpMode, TcpRule, UdpRule};
use host_fake::harness::{
    app_identity, open, params, wait_end, wait_frames, wait_state, FrameReport, TestHost,
};
use rsp_client::session::{
    EventStream, SessionError, SessionHandle, SessionParams, SessionStateKind,
};
use rsp_core::{errors::ErrorCode, pb, trust::Permission};
use rsp_transport::dial::{IfaceInfo, IpNet, LocalNet, PathKind};
use tokio::time::{sleep, timeout, Instant};

// At 30% loss the first Initial is dropped and its retry is forwarded.
const SEED: u64 = 0xa17e_0009;
const TCP_AFTER: Duration = Duration::from_millis(250);

async fn bounded(test: impl Future<Output = ()>) {
    timeout(Duration::from_secs(9), test)
        .await
        .expect("fallback test completes within nine seconds");
}

async fn fixture(udp: UdpRule, tcp: TcpMode) -> (TestHost, FaultProxy) {
    let host = TestHost::start().await;
    let proxy = FaultProxy::start(host.quic_addr, host.tcp_addr, SEED)
        .await
        .expect("start same-port UDP/TCP fault proxy");
    proxy.set_udp(Dir::ToHost, udp.clone());
    proxy.set_udp(Dir::ToClient, udp);
    proxy.set_tcp(TcpRule {
        mode: tcp,
        ..TcpRule::default()
    });
    (host, proxy)
}

fn paired_params(host: &TestHost, proxy: &FaultProxy) -> SessionParams {
    let app = app_identity();
    host.pair(&app, Permission::Control);
    let mut p = params(host, app, &[proxy.addr()]);
    p.dial_options.tcp_after = TCP_AFTER;
    p
}

fn udp_blackhole() -> UdpRule {
    UdpRule {
        blackhole: true,
        ..UdpRule::default()
    }
}

async fn hud_transport(session: &SessionHandle, budget: Duration) -> Option<pb::Transport> {
    timeout(budget, async {
        loop {
            if let Some(hud) = session.stats().hud {
                return hud.transport;
            }
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .ok()
}

async fn receive(
    session: &SessionHandle,
    events: &mut EventStream,
    frames: u64,
    budget: Duration,
) -> (FrameReport, Option<pb::Transport>) {
    tokio::join!(
        wait_frames(events, frames, budget),
        hud_transport(session, budget.min(Duration::from_secs(2))),
    )
}

async fn dial_failure(mut p: SessionParams) -> (SessionError, Duration) {
    p.dial_options.timeout = Duration::from_secs(1);
    let started = Instant::now();
    let result = timeout(Duration::from_secs(2), open(p))
        .await
        .expect("one-second failed dial returns within two seconds");
    match result {
        Err(error) => (error, started.elapsed()),
        Ok((session, _)) => {
            session.close();
            panic!("fault-injected dial unexpectedly connected");
        }
    }
}

async fn check_failure(
    name: &str,
    p: SessionParams,
    proxy: &FaultProxy,
    expected: ErrorCode,
    tcp: TcpMode,
) {
    let (error, elapsed) = dial_failure(p).await;
    let code = match &error {
        SessionError::Dial { code, .. } => Some(code.code()),
        _ => None,
    };
    eprintln!(
        "{name} elapsed={elapsed:?} code={code:?} transport=None proxy={:?}",
        proxy.stats(),
    );
    assert!(
        matches!(error, SessionError::Dial { code, .. } if code == expected),
        "expected {expected:?}, got {error:?}",
    );
    let stats = proxy.stats();
    assert!(
        stats.to_host.dropped > 0,
        "UDP fault was exercised: {stats:?}"
    );
    if tcp == TcpMode::Refuse {
        assert_eq!(stats.tcp_accepted, 0, "TCP listener refused the dial");
    } else {
        assert!(
            stats.tcp_accepted >= 1,
            "TCP blackhole was exercised: {stats:?}"
        );
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it05_udp_blocked_falls_back_to_tcp() {
    bounded(async {
        let (host, proxy) = fixture(udp_blackhole(), TcpMode::Forward).await;
        let p = paired_params(&host, &proxy);
        let started = Instant::now();
        let (session, mut events) = timeout(Duration::from_millis(1500), open(p))
            .await
            .expect("TCP fallback opens within 1500 ms")
            .expect("TCP fallback connects");
        let opened = started.elapsed();
        let (report, transport) = receive(&session, &mut events, 30, Duration::from_secs(3)).await;
        let stats = proxy.stats();
        eprintln!(
            "it05_udp_blocked_falls_back_to_tcp open={opened:?} elapsed={:?} code=0 transport={transport:?} frames={} errors={:?} ended={:?} proxy={stats:?}",
            started.elapsed(), report.frames, report.errors, report.ended,
        );
        assert!(opened >= TCP_AFTER, "fallback opened too early: {opened:?}");
        assert!(opened <= Duration::from_millis(1500), "open={opened:?}");
        assert!(report.frames >= 30, "{report:?}");
        assert_eq!(transport, Some(pb::Transport::Tcp));
        assert!(stats.tcp_accepted >= 1, "{stats:?}");
        assert!(stats.to_host.dropped > 0, "{stats:?}");
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn udp_ok_prefers_quic() {
    bounded(async {
        let (host, proxy) = fixture(UdpRule::default(), TcpMode::Forward).await;
        let started = Instant::now();
        let (session, mut events) = open(paired_params(&host, &proxy))
            .await
            .expect("unimpaired UDP connects");
        let opened = started.elapsed();
        let (report, transport) = receive(&session, &mut events, 1, Duration::from_secs(2)).await;
        eprintln!(
            "udp_ok_prefers_quic open={opened:?} elapsed={:?} code=0 transport={transport:?} frames={} proxy={:?}",
            started.elapsed(), report.frames, proxy.stats(),
        );
        assert_eq!(transport, Some(pb::Transport::Quic));
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn tcp_refused_without_metadata_is_generic() {
    bounded(async {
        let (host, proxy) = fixture(udp_blackhole(), TcpMode::Refuse).await;
        let p = paired_params(&host, &proxy);
        check_failure(
            "tcp_refused_without_metadata_is_generic",
            p,
            &proxy,
            ErrorCode::Other,
            TcpMode::Refuse,
        )
        .await;
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn dual_timeout_without_metadata_is_generic() {
    bounded(async {
        let (host, proxy) = fixture(udp_blackhole(), TcpMode::Blackhole).await;
        let p = paired_params(&host, &proxy);
        check_failure(
            "dual_timeout_without_metadata_is_generic",
            p,
            &proxy,
            ErrorCode::Other,
            TcpMode::Blackhole,
        )
        .await;
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn intranet_both_timeout_is_161() {
    bounded(async {
        let (host, proxy) = fixture(udp_blackhole(), TcpMode::Blackhole).await;
        let mut p = paired_params(&host, &proxy);
        p.candidates[0].kind = PathKind::Intranet;
        let loopback: IpNet = "127.0.0.0/8".parse().expect("loopback CIDR");
        p.intranet_cidrs = vec![loopback];
        p.local_net = LocalNet {
            interfaces: vec![IfaceInfo {
                name: "lo".into(),
                addrs: vec![loopback],
                is_vpn: false,
            }],
        };
        assert!(p.local_net.is_direct(proxy.addr().ip()));
        check_failure(
            "intranet_both_timeout_is_161",
            p,
            &proxy,
            ErrorCode::PortBlocked,
            TcpMode::Blackhole,
        )
        .await;
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn it04_wrong_host_fingerprint_is_150() {
    bounded(async {
        let (host, proxy) = fixture(UdpRule::default(), TcpMode::Forward).await;
        // An unpaired client's ClientHello would immediately create an
        // auth.denied audit record. An empty queue thus observes the Host side,
        // including connections rejected before an EventStream can be returned.
        let app = app_identity();
        let mut p = params(&host, app.clone(), &[proxy.addr()]);
        p.host_fingerprint = app_identity().fingerprint();
        assert_ne!(p.host_fingerprint, host.fp);
        let audit = host.security.audit.as_ref().expect("Host audit enabled");
        assert!(audit.is_empty());
        let (error, elapsed) = dial_failure(p).await;
        sleep(Duration::from_millis(50)).await;
        let audit_count = audit.len();
        let code = match &error {
            SessionError::Dial { code, .. } => Some(code.code()),
            _ => None,
        };
        eprintln!(
            "it04_wrong_host_fingerprint_is_150 elapsed={elapsed:?} code={code:?} transport=None audit_count={audit_count} proxy={:?}",
            proxy.stats(),
        );
        assert!(
            matches!(error, SessionError::Dial { code: ErrorCode::FingerprintMismatch, .. }),
            "{error:?}",
        );
        assert_eq!(audit_count, 0, "no ClientHello reached Host authentication");

        // Positive control: the same unpaired identity and proxy with the real
        // pin must reach authentication and make this audit observer nonempty.
        let (session, mut events) = open(params(&host, app, &[proxy.addr()]))
            .await
            .expect("positive control reaches Host");
        let (code, _, _) = wait_end(&mut events, Duration::from_secs(1))
            .await
            .expect("unpaired positive control ends");
        assert_eq!(code, Some(ErrorCode::NotPaired));
        assert!(audit.len() > audit_count, "ClientHello creates a denial audit");
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn handshake_loss_30pct_still_connects() {
    bounded(async {
        let (host, proxy) = fixture(
            UdpRule {
                loss_pct: 30.0,
                skip_first: 0,
                ..UdpRule::default()
            },
            TcpMode::Forward,
        )
        .await;
        let mut p = paired_params(&host, &proxy);
        // Keep TCP outside the dial budget: this test must exercise QUIC's
        // handshake retransmission rather than succeed through TCP fallback.
        p.dial_options.tcp_after = Duration::from_secs(5);
        p.dial_options.timeout = Duration::from_secs(5);
        // Small frames keep this focused on handshake recovery, rather than
        // the probability of losing a fragment of a large 12 Mbps frame.
        p.start.max_width = 640;
        p.start.bitrate_kbps = 500;
        let started = Instant::now();
        let deadline = started + Duration::from_secs(5);
        let (session, mut events) = tokio::time::timeout_at(deadline, open(p))
            .await
            .expect("lossy QUIC opens within five seconds")
            .expect("QUIC handshake survives 30 percent loss");
        let opened = started.elapsed();
        // open() returns before RSP authentication. Keep both loss rules active
        // until the reliable handshake reaches Streaming; then remove only the
        // UDP impairment. Sustained media loss belongs to IT-07, not this test
        // of QUIC handshake retransmission.
        let streaming = wait_state(
            &mut events,
            SessionStateKind::Streaming,
            deadline.saturating_duration_since(Instant::now()),
        )
        .await;
        let handshaken = started.elapsed();
        let handshake_stats = proxy.stats();
        proxy.set_udp(Dir::ToHost, UdpRule::default());
        proxy.set_udp(Dir::ToClient, UdpRule::default());
        let (report, transport) = receive(
            &session,
            &mut events,
            1,
            deadline.saturating_duration_since(Instant::now()),
        )
        .await;
        let stats = proxy.stats();
        eprintln!(
            "handshake_loss_30pct_still_connects seed={SEED:#x} open={opened:?} handshake={handshaken:?} streaming={streaming} elapsed={:?} code=0 transport={transport:?} frames={} first_frame={:?} handshake_drops={}/{} proxy={stats:?}",
            started.elapsed(), report.frames, report.first_frame,
            handshake_stats.to_host.dropped, handshake_stats.to_client.dropped,
        );
        assert!(streaming, "RSP authentication survives handshake loss");
        assert_eq!(report.frames, 1, "{report:?}");
        assert!(started.elapsed() <= Duration::from_secs(5));
        assert_eq!(transport, Some(pb::Transport::Quic));
        assert!(handshake_stats.to_host.dropped >= 1, "the initial QUIC handshake packet was dropped");
        assert_eq!(stats.tcp_accepted, 0, "handshake used QUIC retransmission");
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn mtu_blackhole_1200() {
    bounded(async {
        let (host, proxy) = fixture(
            UdpRule {
                max_datagram: Some(1200),
                ..UdpRule::default()
            },
            TcpMode::Forward,
        )
        .await;
        let mut p = paired_params(&host, &proxy);
        p.dial_options.tcp_after = Duration::from_secs(5);
        let started = Instant::now();
        let deadline = started + Duration::from_secs(5);
        let (session, mut events) = tokio::time::timeout_at(deadline, open(p))
            .await
            .expect("MTU-limited QUIC opens within five seconds")
            .expect("QUIC connects over a 1200-byte path");
        let opened = started.elapsed();
        let (report, transport) = receive(
            &session,
            &mut events,
            60,
            deadline.saturating_duration_since(Instant::now()),
        )
        .await;
        let stats = proxy.stats();
        let oversize_dropped = stats.to_host.oversize_dropped + stats.to_client.oversize_dropped;
        eprintln!(
            "mtu_blackhole_1200 open={opened:?} elapsed={:?} code=0 transport={transport:?} frames={} oversize_dropped={oversize_dropped} proxy={stats:?}",
            started.elapsed(), report.frames,
        );
        assert!(report.frames >= 60, "1200-byte MTU must sustain video: {report:?}");
        assert!(started.elapsed() <= Duration::from_secs(5));
        assert_eq!(transport, Some(pb::Transport::Quic));
        assert!(oversize_dropped > 0, "MTU fault dropped QUIC probe packets");
        session.close();
        proxy.shutdown().await;
        host.stop().await;
    })
    .await;
}
