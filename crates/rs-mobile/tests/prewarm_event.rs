mod support;

use std::{
    net::{SocketAddr, UdpSocket},
    sync::Arc,
    time::{Duration, Instant},
};

use host_fake::harness::{TestHost, TestHostOptions};
use rs_mobile::rscore::{
    api_sessions as sessions, runtime::Core, settings::SettingsState, store::PairedHostRow,
};
use rsp_client::PrewarmPool;
use rsp_core::trust::Permission;
use rsp_transport::dial::DialOpts;
use serde_json::Value;
use tokio::time::{sleep, timeout};

const TWO_SECONDS: Duration = Duration::from_secs(2);

async fn until(limit: Duration, predicate: impl Fn() -> bool) {
    timeout(limit, async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

fn new_core() -> (Arc<Core>, Arc<support::EventCollector>) {
    let core = support::test_core_with(|_, options| options.worker_threads = 2);
    support::with_identity(&core, "Prewarm test device");
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    (core, events)
}

fn pair_row(core: &Arc<Core>, host_id: &str, fp: [u8; 32], address: SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host_id.into(),
            fp,
            name: "Paired test host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

async fn paired_host(core: &Arc<Core>) -> TestHost {
    let host = TestHost::start_with(TestHostOptions {
        tcp: false,
        ..Default::default()
    })
    .await;
    host.pair(&core.local_identity().unwrap(), Permission::Control);
    pair_row(core, &host.host_id, host.fp, host.quic_addr);
    host
}

fn set_prewarm(core: &Arc<Core>, enabled: bool) {
    core.state::<SettingsState>()
        .0
        .lock()
        .unwrap()
        .prewarm_enabled = enabled;
}

fn probe_results(events: &support::EventCollector) -> Vec<Value> {
    events.of_type("probe.result")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn successful_prewarm_reports_its_path_and_latency() {
    let (core, events) = new_core();
    let host = paired_host(&core).await;
    sessions::prewarm(&core, vec![host.host_id.clone()]);
    until(TWO_SECONDS, || !probe_results(&events).is_empty()).await;
    assert_eq!(core.state::<PrewarmPool>().len(), 1);
    let results = probe_results(&events);
    assert_eq!(results.len(), 1, "{results:?}");
    let event = &results[0];
    assert_eq!(event["probeId"], "prewarm");
    assert_eq!(event["hostId"], host.host_id.as_str());
    assert_eq!(event["outcome"], "ok");
    assert!(event["ms"].is_u64(), "{event}");
    assert_eq!(event["last"], true);
    assert!(event.get("hello").is_none(), "{event}");
    assert!(event.get("reason").is_none(), "{event}");
    assert_eq!(event["path"], "manual");
    assert_eq!(event["addr"], "127.0.0.1");
    assert_eq!(event["port"], host.quic_addr.port());
    assert_eq!(event["transport"], "quic");
    core.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn disabled_prewarm_reports_nothing() {
    let (core, events) = new_core();
    let host = paired_host(&core).await;
    set_prewarm(&core, false);
    sessions::prewarm(&core, vec![host.host_id.clone()]);
    sleep(Duration::from_millis(500)).await;
    assert!(core.state::<PrewarmPool>().is_empty());
    assert!(probe_results(&events).is_empty());
    // The same Host reports once prewarm is enabled again.
    set_prewarm(&core, true);
    sessions::prewarm(&core, vec![host.host_id.clone()]);
    until(TWO_SECONDS, || !probe_results(&events).is_empty()).await;
    core.shutdown();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn unreachable_prewarm_reports_nothing() {
    let (core, events) = new_core();
    let host = paired_host(&core).await;
    let blackhole = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    pair_row(&core, "blackhole", [7; 32], blackhole.local_addr().unwrap());
    let started = Instant::now();
    sessions::prewarm(&core, vec!["blackhole".into(), host.host_id.clone()]);
    until(TWO_SECONDS, || !probe_results(&events).is_empty()).await;
    // Let the unreachable dial use up its whole budget and fail.
    let settled = DialOpts::default().timeout + Duration::from_millis(500);
    sleep(settled.saturating_sub(started.elapsed())).await;
    let results = probe_results(&events);
    assert_eq!(results.len(), 1, "{results:?}");
    assert_eq!(results[0]["hostId"], host.host_id.as_str());
    assert_eq!(core.state::<PrewarmPool>().len(), 1);
    drop(blackhole);
    core.shutdown();
}
