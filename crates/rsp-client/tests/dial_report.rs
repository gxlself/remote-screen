#[allow(dead_code)]
mod support;

use std::{
    net::{SocketAddr, UdpSocket},
    sync::Arc,
};

use rsp_client::session::{DialReport, EventStream, Session, SessionEvent, SessionParams};
use rsp_core::pb;
use rsp_transport::{
    dial::{AttemptResult, Candidate, CandidateSource, IfaceInfo, LocalNet, PathKind, Skipped},
    identity::{CertKind, LocalIdentity},
    TransportKind,
};
use tokio::time::timeout;

fn identity() -> LocalIdentity {
    LocalIdentity::generate_software(CertKind::App).expect("app identity")
}

fn manual(addr: SocketAddr) -> Candidate {
    Candidate {
        addr,
        kind: PathKind::Manual,
        source: CandidateSource::Manual,
    }
}

/// Accepts one session, streams nothing and says goodbye.
fn serve(host: support::Host) -> tokio::task::JoinHandle<()> {
    tokio::spawn(async move {
        let (conn, control) = host.accept().await;
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::finish(&conn, &control).await;
    })
}

async fn ended(events: &mut EventStream) {
    timeout(support::TEST_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            if matches!(event, SessionEvent::Ended { .. }) {
                return;
            }
        }
    })
    .await
    .expect("session ends");
}

#[tokio::test]
async fn report_names_the_primary_and_keeps_every_attempt() {
    let host = support::Host::bind().await;
    let (host_addr, fingerprint) = (host.addr, host.fingerprint);
    // Bound but silent: QUIC Initial packets to it are never answered.
    let dead = UdpSocket::bind("127.0.0.1:0").expect("silent socket");
    let dead_addr = dead.local_addr().expect("silent address");
    let server = serve(host);
    let params = SessionParams::new(vec![dead_addr, host_addr], fingerprint, identity());
    let (handle, mut events) = Session::open(params).await.expect("open");
    let report = handle.dial_report();
    assert_eq!(report.primary, Some(manual(host_addr)));
    assert_eq!(report.transport, Some(TransportKind::Quic));
    let primary = report
        .attempts
        .iter()
        .find(|log| log.candidate.addr == host_addr && log.transport == Some(TransportKind::Quic))
        .expect("Host QUIC attempt");
    assert_eq!(primary.result, AttemptResult::Connected);
    assert!(primary.started_at_us.is_some());
    assert!(primary.finished_at_us.is_some());
    let silent: Vec<_> = report
        .attempts
        .iter()
        .filter(|log| log.candidate.addr == dead_addr)
        .collect();
    assert!(!silent.is_empty(), "{report:?}");
    assert!(
        silent
            .iter()
            .all(|log| log.result != AttemptResult::Connected),
        "{report:?}"
    );
    ended(&mut events).await;
    server.await.expect("Host script");
    drop(dead);
}

#[tokio::test]
async fn report_keeps_lan_candidates_skipped_by_the_plan() {
    let host = support::Host::bind().await;
    let (host_addr, fingerprint) = (host.addr, host.fingerprint);
    let lan: SocketAddr = "192.0.2.10:7000".parse().expect("LAN address");
    let server = serve(host);
    let mut params = SessionParams::from_candidates(
        vec![
            Candidate {
                addr: lan,
                kind: PathKind::Lan,
                source: CandidateSource::Mdns,
            },
            manual(host_addr),
        ],
        fingerprint,
        identity(),
    );
    params.local_net = LocalNet {
        interfaces: vec![IfaceInfo {
            name: "en0".into(),
            addrs: vec!["10.9.8.7/24".parse().expect("interface network")],
            is_vpn: false,
        }],
    };
    let (handle, mut events) = Session::open(params).await.expect("open");
    let report = handle.dial_report();
    assert!(
        report.attempts.iter().any(|log| log.candidate.addr == lan
            && log.transport.is_none()
            && log.result == AttemptResult::Skipped(Skipped::NotOnLan)),
        "{report:?}"
    );
    assert_eq!(report.primary, Some(manual(host_addr)));
    ended(&mut events).await;
    server.await.expect("Host script");
}

#[tokio::test]
async fn prewarmed_connection_reports_no_dial() {
    let host = support::Host::bind().await;
    let identity = identity();
    let endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().expect("loopback")).expect("endpoint");
    let config = rsp_transport::tls::client_config(
        &identity,
        rsp_transport::tls::ServerPin::Fingerprint(host.fingerprint),
    )
    .expect("TLS");
    let (client, server) = tokio::join!(
        rsp_transport::connect_quic(&endpoint, host.addr, Arc::new(config), true),
        async {
            host.listener
                .accept()
                .await
                .expect("incoming")
                .accept()
                .await
                .expect("accept")
        }
    );
    let pool = Arc::new(rsp_client::PrewarmPool::new());
    pool.insert(host.fingerprint, client.expect("client"))
        .expect("insert");
    let mut params = SessionParams::single(host.addr, host.fingerprint, identity);
    params.prewarm = Some(pool.clone());
    let script = tokio::spawn(async move {
        let control = server.accept_control().await.expect("reused control");
        support::hello(&control).await;
        support::auth(&control, pb::auth_result::Status::Ok).await;
        assert!(matches!(
            support::recv_command(&control).await,
            pb::envelope::Msg::StartStream(_)
        ));
        support::finish(&server, &control).await;
    });
    let (handle, mut events) = Session::open(params).await.expect("reuse");
    assert!(handle.handshake_timings().reused);
    assert_eq!(handle.dial_report(), DialReport::default());
    ended(&mut events).await;
    script.await.expect("reuse script");
    drop((host, endpoint));
}
