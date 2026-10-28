mod support;

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use host_fake::harness::TestHost;
use rs_mobile::rscore::{
    api_diag::{failure_priority, probe, probe_address, probe_with_net},
    error::CoreError,
    netinfo::{local_net, RawIface},
    runtime::Core,
    store::{LastGoodRow, PairedHostRow},
};
use rsp_core::trust::Permission;
use rsp_transport::dial::{AttemptResult, LocalNet};
use serde_json::Value;

use support::{test_core_with, with_identity, EventCollector};

pub fn paired_core(host: &TestHost) -> Arc<Core> {
    let core = test_core_with(|_, opts| {
        opts.timeouts.probe = Duration::from_secs(1);
    });
    with_identity(&core, "diagnostics");
    host.pair(&core.local_identity().unwrap(), Permission::Control);
    save_host(&core, host, vec![host.quic_addr], host.fp);
    core
}

pub fn save_host(core: &Arc<Core>, host: &TestHost, addresses: Vec<SocketAddr>, fp: [u8; 32]) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp,
            name: "test host".into(),
            addresses,
            paired_at_us: core.now_us(),
            permission: None,
        })
        .unwrap();
}

pub fn set_last_good(core: &Arc<Core>, host: &TestHost, addr: &str, path: &str) {
    core.store()
        .set_last_good(&LastGoodRow {
            host_id: host.host_id.clone(),
            addr: addr.parse().unwrap(),
            path: path.into(),
            transport: "quic".into(),
            at_us: core.now_us(),
        })
        .unwrap();
}

pub fn collect(core: &Arc<Core>) -> Arc<EventCollector> {
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    events
}

pub async fn results(events: &EventCollector, id: u64) -> Vec<Value> {
    let id = id.to_string();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let results: Vec<_> = events
                .of_type("probe.result")
                .into_iter()
                .filter(|event| event["probeId"] == id)
                .collect();
            if results.iter().any(|event| event["last"] == true) {
                assert_eq!(results.last().unwrap()["last"], true);
                assert_eq!(
                    results.iter().filter(|event| event["last"] == true).count(),
                    1
                );
                return results;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("probe results must finish within three seconds")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn paired_quic_probe_returns_immediately_and_emits_one_result() {
    let host = TestHost::start().await;
    let core = paired_core(&host);
    let events = collect(&core);
    let id = tokio::time::timeout(Duration::from_millis(500), probe(&core, &host.host_id))
        .await
        .expect("probe must not wait for dialing")
        .unwrap();
    assert_eq!(id, 1);
    let result = results(&events, id).await;
    assert_eq!(result.len(), 1);
    let result = &result[0];
    assert_eq!(result["outcome"], "ok");
    assert_eq!(result["transport"], "quic");
    assert!(result["ms"].is_number());
    assert_eq!(result["hostId"], host.host_id);
    assert_eq!(result["addr"], "127.0.0.1");
    assert_eq!(result["port"], host.quic_addr.port());
    assert_eq!(result["last"], true);
    assert!(core.events().flush(Duration::from_secs(1)));
    assert_eq!(events.of_type("probe.result").len(), 1);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn every_candidate_reports_and_the_last_delivery_finishes_the_probe() {
    let host = TestHost::start().await;
    let core = paired_core(&host);
    let closed = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let closed_addr = closed.local_addr().unwrap();
    drop(closed);
    save_host(&core, &host, vec![host.quic_addr, closed_addr], host.fp);
    // A stale intranet address must not replace a last-good row on
    // the intranet path now supplies one.
    set_last_good(&core, &host, "10.30.1.5:7400", "intranet");
    let events = collect(&core);
    let local = local_net(
        &[RawIface {
            name: "en0".into(),
            addrs: vec!["192.168.31.20/24".parse().unwrap()],
        }],
        &[],
    );
    let id = probe_with_net(&core, &host.host_id, local).await.unwrap();
    let result = results(&events, id).await;
    assert_eq!(result.len(), 3);
    assert!(result.iter().all(|event| event["hostId"] == host.host_id));
    let vpn = result
        .iter()
        .find(|event| event["addr"] == "10.30.1.5")
        .unwrap();
    assert_eq!(vpn["outcome"], "needsVpn");
    assert_eq!(vpn["path"], "intranet");
    assert!(vpn["ms"].is_null());
    let unreachable = result
        .iter()
        .find(|event| event["port"] == closed_addr.port())
        .unwrap();
    assert!(matches!(
        unreachable["outcome"].as_str(),
        Some("refused" | "timeout")
    ));
    assert!(unreachable["ms"].is_number());
    assert!(matches!(
        unreachable["reason"].as_str(),
        Some("Refused" | "TimedOut")
    ));
    assert!(result.iter().any(|event| event["outcome"] == "ok"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn wrong_paired_fingerprint_is_reported_without_exposing_the_fingerprint() {
    let host = TestHost::start().await;
    let core = paired_core(&host);
    let wrong_fp = support::TestSigner::new();
    let wrong_identity = rsp_transport::identity::LocalIdentity::generate_external(
        rsp_transport::identity::CertKind::Host,
        wrong_fp,
    )
    .unwrap();
    save_host(
        &core,
        &host,
        vec![host.quic_addr],
        wrong_identity.fingerprint(),
    );
    let events = collect(&core);
    let id = probe(&core, &host.host_id).await.unwrap();
    let result = results(&events, id).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["outcome"], "fingerprintMismatch");
    assert_eq!(result[0]["reason"], "FingerprintMismatch");
    assert_eq!(result[0]["hostId"], host.host_id);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn address_probe_discovers_identity_supports_dns_and_defaults_the_port() {
    let host = TestHost::start().await;
    let core = paired_core(&host);
    let events = collect(&core);
    let id = probe_address(&core, " 127.0.0.1 ", Some(host.quic_addr.port()))
        .await
        .unwrap();
    let result = results(&events, id).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["outcome"], "ok");
    assert_eq!(result[0]["hostId"], host.host_id);
    assert_eq!(result[0]["transport"], "quic");
    assert!(result[0]["ms"].is_number());
    assert!(result[0].get("reason").is_none());

    let default_id = probe_address(&core, "127.0.0.1", None).await.unwrap();
    assert_eq!(default_id, id + 1);
    let result = results(&events, default_id).await;
    assert!(result.iter().all(|event| event["port"] == 7400));
    for result in &result {
        if result["outcome"] != "ok" {
            assert!(result["hostId"].is_null());
        }
    }
    assert_eq!(
        probe_address(&core, "a b", None).await,
        Err(CoreError::internal("bad address"))
    );
    assert_eq!(
        probe_address(&core, "   ", None).await,
        Err(CoreError::internal("bad address"))
    );
    let dns_id = probe_address(&core, "localhost", Some(host.quic_addr.port()))
        .await
        .unwrap();
    assert_eq!(dns_id, default_id + 1);
    let result = results(&events, dns_id).await;
    assert!(!result.is_empty() && result.len() <= 4);
    assert!(result
        .iter()
        .any(|event| event["outcome"] == "ok" && event["hostId"] == host.host_id));

    let ipv6_id = probe_address(&core, "[::1]", Some(host.quic_addr.port()))
        .await
        .unwrap();
    let result = results(&events, ipv6_id).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["addr"], "::1");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn unknown_host_and_missing_identity_reject_without_events() {
    let host = TestHost::start().await;
    let core = test_core_with(|_, _| {});
    let events = collect(&core);
    assert_eq!(
        probe(&core, "missing").await,
        Err(CoreError::internal("unknown host"))
    );
    save_host(&core, &host, vec![host.quic_addr], host.fp);
    assert!(matches!(
        probe(&core, &host.host_id).await,
        Err(CoreError::Internal { .. })
    ));
    assert!(matches!(
        probe_address(&core, "127.0.0.1", Some(host.quic_addr.port())).await,
        Err(CoreError::Internal { .. })
    ));
    assert!(core.events().flush(Duration::from_secs(1)));
    assert!(events.of_type("probe.result").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn absent_addresses_and_lan_pruning_still_finish_exactly_once() {
    let host = TestHost::start().await;
    let core = paired_core(&host);
    save_host(&core, &host, Vec::new(), host.fp);
    let events = collect(&core);
    let id = probe(&core, &host.host_id).await.unwrap();
    let result = results(&events, id).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["outcome"], "noRoute");
    assert_eq!(result[0]["path"], "manual");
    assert_eq!(result[0]["addr"], "");
    assert_eq!(result[0]["port"], 0);
    assert!(result[0]["ms"].is_null());

    // A stale LAN address must not replace a last-good row on the
    // LAN path now supplies one.
    set_last_good(&core, &host, "192.168.31.20:7400", "lan");
    let id = probe_with_net(&core, &host.host_id, LocalNet::default())
        .await
        .unwrap();
    let result = results(&events, id).await;
    assert_eq!(result.len(), 1);
    assert_eq!(result[0]["outcome"], "skipped");
    assert_eq!(result[0]["path"], "lan");
    assert!(result[0]["ms"].is_null());
}

#[test]
pub fn failure_evidence_has_the_specified_priority() {
    let evidence = [
        AttemptResult::FingerprintMismatch { got: [7; 32] },
        AttemptResult::Refused,
        AttemptResult::TimedOut,
        AttemptResult::TlsFailed,
    ];
    for pair in evidence.windows(2) {
        assert!(failure_priority(&pair[0]) < failure_priority(&pair[1]));
    }
    assert_eq!(
        failure_priority(&AttemptResult::TlsFailed),
        failure_priority(&AttemptResult::Failed)
    );
}
