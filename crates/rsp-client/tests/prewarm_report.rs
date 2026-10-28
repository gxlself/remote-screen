#[allow(dead_code)]
mod support;

use std::{future::Future, net::SocketAddr, pin::Pin, sync::Arc};

use rsp_client::{prewarm::PrewarmError, PrewarmPool};
use rsp_transport::{
    dial::{AttemptResult, Candidate, CandidateSource, DefaultDialer, DialPlan, PathKind},
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    Conn, ConnError, TransportKind,
};

fn plan(addr: SocketAddr) -> DialPlan {
    DialPlan {
        candidates: vec![Candidate {
            addr,
            kind: PathKind::Manual,
            source: CandidateSource::Manual,
        }],
        ..DialPlan::default()
    }
}

struct Refusing;
impl Dialer for Refusing {
    fn dial<'a>(
        &'a self,
        _: SocketAddr,
        _: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        Box::pin(async { Err(ConnError::Closed) })
    }
}

#[tokio::test]
async fn warm_report_returns_the_winning_path_and_still_pools_the_connection() {
    let host = support::Host::bind().await;
    let identity = LocalIdentity::generate_software(CertKind::App).expect("identity");
    let endpoint =
        quinn::Endpoint::client("127.0.0.1:0".parse().expect("loopback")).expect("endpoint");
    let dialer = Arc::new(DefaultDialer::new(endpoint, &identity).expect("dialer"));
    let pool = PrewarmPool::new();
    let plan = plan(host.addr);
    let (report, server) = tokio::join!(pool.warm_report(&plan, dialer, host.fingerprint), async {
        host.listener
            .accept()
            .await
            .expect("incoming")
            .accept()
            .await
            .expect("accept")
    });
    let report = report.expect("warm report");
    assert_eq!(report.path.addr, host.addr);
    assert_eq!(report.path.kind, PathKind::Manual);
    assert_eq!(report.transport, TransportKind::Quic);
    assert!(
        report.attempts.iter().any(|log| {
            log.result == AttemptResult::Connected
                && log.transport == Some(TransportKind::Quic)
                && log.candidate.addr == host.addr
                && log.started_at_us.is_some()
                && log.finished_at_us.is_some()
        }),
        "{:?}",
        report.attempts
    );
    assert_eq!(pool.len(), 1);
    let conn = pool.take(host.fingerprint).expect("pooled connection");
    assert_eq!(conn.kind(), TransportKind::Quic);
    assert_eq!(conn.remote_addr(), host.addr);
    assert!(pool.is_empty());
    drop((conn, server));
}

#[tokio::test]
async fn warm_report_fails_with_the_same_dial_error_as_warm() {
    let pool = PrewarmPool::new();
    let plan = plan("192.0.2.1:7400".parse().expect("documentation address"));
    let report = pool.warm_report(&plan, Arc::new(Refusing), [1; 32]).await;
    let warm = pool.warm(&plan, Arc::new(Refusing), [1; 32]).await;
    let outcomes = |error: PrewarmError| match error {
        PrewarmError::Dial(failure) => failure
            .attempts
            .into_iter()
            .map(|log| (log.candidate.addr, log.transport, log.result))
            .collect::<Vec<_>>(),
        other => panic!("expected a dial failure, got {other:?}"),
    };
    let report = outcomes(report.expect_err("warm_report fails"));
    assert_eq!(report, outcomes(warm.expect_err("warm fails")));
    assert!(report
        .iter()
        .all(|(_, _, result)| *result != AttemptResult::Connected));
    assert!(pool.is_empty());
}
