mod support;

use std::net::{SocketAddr, TcpListener};
use std::sync::Arc;
use std::time::Duration;

use host_fake::harness::TestHost;
use rs_mobile::rscore::{
    api_diag::{probe, set_setting},
    api_sessions::{host_params, session_params},
    api_ssh::SERVERS_SYNC_KV,
    candidates::{manual_candidate, TargetError},
    dto_core::{OpenIntent, OpenOpts},
    manual_addrs::{host_target_with_manual, parse_manual_computers},
    runtime::Core,
    store::PairedHostRow,
    tables::SessionTable,
};
use rsp_core::{pb, trust::Permission};
use rsp_transport::dial::{CandidateSource, LocalNet};
use serde_json::{json, Value};

use support::{test_core, test_core_with, with_identity, EventCollector};

fn socket(value: &str) -> SocketAddr {
    value.parse().unwrap()
}

fn computer(host_id: Value, host: &str, port: Value) -> Value {
    json!({"id": "manual", "kind": "computer", "host": host, "port": port,
        "hostId": host_id, "name": null, "networkId": null, "addedAtUs": 1})
}

fn servers(entries: Vec<Value>) -> Value {
    json!({"v": 1, "entries": entries, "tailscaleSsh": {}})
}

fn remember(core: &Arc<Core>, value: &Value) {
    core.store()
        .kv_set(SERVERS_SYNC_KV, &value.to_string())
        .unwrap();
}

fn save_paired(
    core: &Arc<Core>,
    host_id: &str,
    fp: [u8; 32],
    addresses: Vec<SocketAddr>,
) -> PairedHostRow {
    let row = PairedHostRow {
        host_id: host_id.into(),
        fp,
        name: "Paired Mac".into(),
        addresses,
        paired_at_us: core.now_us(),
        permission: None,
    };
    core.store().upsert_paired_host(&row).unwrap();
    row
}

fn closed_addr() -> SocketAddr {
    let closed = TcpListener::bind(("127.0.0.1", 0)).unwrap();
    let addr = closed.local_addr().unwrap();
    drop(closed);
    addr
}

#[test]
fn parse_keeps_computer_entries_and_skips_bad_ones_individually() {
    let value = servers(vec![
        computer(json!("h1"), "10.30.0.9", json!(7400)),
        json!({"id": "ssh", "kind": "ssh", "name": "S", "host": "10.30.0.7", "port": 22,
            "user": "ops", "auth": "password", "hostId": "h1"}),
        computer(Value::Null, "10.30.0.10", json!(7400)),
        computer(json!(""), "10.30.0.11", json!(7400)),
        computer(json!("h1"), "mac.local", json!(7400)),
        computer(json!("h1"), "10.30.0.12", json!(0)),
        computer(json!("h1"), "10.30.0.13", json!(65536)),
        computer(json!("h1"), "10.30.0.14", json!("7400")),
        json!({"kind": "computer", "hostId": "h1", "port": 7400}),
        json!("not an entry"),
        computer(json!("h2"), "[::1]", json!(7401)),
        computer(json!("h2"), "fd00::2", json!(7402)),
    ]);
    assert_eq!(
        parse_manual_computers(Some(&value)),
        vec![
            ("h1".to_owned(), socket("10.30.0.9:7400")),
            ("h2".to_owned(), socket("[::1]:7401")),
            ("h2".to_owned(), socket("[fd00::2]:7402")),
        ]
    );

    let mut v2 = value;
    v2["v"] = json!(2);
    for rejected in [
        v2,
        json!({"v": 1}),
        json!({"v": 1, "entries": {}}),
        json!([]),
    ] {
        assert!(parse_manual_computers(Some(&rejected)).is_empty());
    }
    assert!(parse_manual_computers(None).is_empty());
}

#[test]
fn host_params_appends_same_host_manual_addresses_after_existing_candidates() {
    let core = test_core();
    with_identity(&core, "manual addresses");
    let unreachable = closed_addr();
    save_paired(&core, "h1", [7; 32], vec![unreachable]);
    remember(
        &core,
        &servers(vec![
            computer(json!("h2"), "10.30.0.10", json!(7400)),
            computer(json!("h1"), "10.30.0.9", json!(7400)),
            computer(
                json!("h1"),
                &unreachable.ip().to_string(),
                json!(unreachable.port()),
            ),
            computer(json!("h1"), "10.30.0.9", json!(7400)),
        ]),
    );
    let params = host_params(&core, "h1", pb::Intent::Control).unwrap();
    assert_eq!(
        params
            .candidates
            .iter()
            .map(|candidate| (candidate.addr, candidate.source))
            .collect::<Vec<_>>(),
        vec![
            (unreachable, CandidateSource::Cache),
            (socket("10.30.0.9:7400"), CandidateSource::Manual),
        ]
    );
    assert_eq!(params.host_fingerprint, [7; 32]);

    // An unreadable copy never fails the dial; it only adds nothing.
    core.store().kv_set(SERVERS_SYNC_KV, "not json").unwrap();
    let params = host_params(&core, "h1", pb::Intent::Control).unwrap();
    assert_eq!(
        params
            .candidates
            .iter()
            .map(|candidate| candidate.addr)
            .collect::<Vec<_>>(),
        vec![unreachable]
    );
    core.shutdown();
}

#[test]
fn manual_addresses_alone_dial_a_paired_host_under_its_paired_pin() {
    let core = test_core();
    let paired = save_paired(&core, "h1", [7; 32], Vec::new());
    let local = LocalNet::default();
    let target = |paired: Option<&PairedHostRow>| {
        host_target_with_manual(&core, "h1", paired, None, &local, &[])
    };
    assert_eq!(target(Some(&paired)).unwrap_err(), TargetError::NoAddress);

    remember(
        &core,
        &servers(vec![
            computer(json!("h2"), "10.30.0.10", json!(7400)),
            computer(json!("h1"), "10.30.0.9", json!(7400)),
        ]),
    );
    let found = target(Some(&paired)).unwrap();
    assert_eq!(
        found.candidates,
        vec![manual_candidate(socket("10.30.0.9:7400"))]
    );
    assert_eq!(found.fp, paired.fp);
    assert_eq!(found.host_name, "Paired Mac");
    assert_eq!(target(None).unwrap_err(), TargetError::NotPaired);
    core.shutdown();
}

async fn probe_results(events: &EventCollector, id: u64) -> Vec<Value> {
    let id = id.to_string();
    tokio::time::timeout(Duration::from_secs(3), async {
        loop {
            let results: Vec<_> = events
                .of_type("probe.result")
                .into_iter()
                .filter(|event| event["probeId"] == id)
                .collect();
            if results.iter().any(|event| event["last"] == true) {
                return results;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("probe results must finish within three seconds")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_reaches_the_host_through_its_manual_address() {
    let host = TestHost::start().await;
    let core = test_core_with(|_, opts| opts.timeouts.probe = Duration::from_secs(1));
    with_identity(&core, "manual probe");
    host.pair(&core.local_identity().unwrap(), Permission::Control);
    let unreachable = closed_addr();
    save_paired(&core, &host.host_id, host.fp, vec![unreachable]);
    remember(
        &core,
        &servers(vec![computer(
            json!(host.host_id),
            &host.quic_addr.ip().to_string(),
            json!(host.quic_addr.port()),
        )]),
    );
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    let id = probe(&core, &host.host_id).await.unwrap();
    let results = probe_results(&events, id).await;
    let manual = results
        .iter()
        .find(|event| event["port"] == host.quic_addr.port())
        .expect("the manual address is probed");
    assert_eq!(manual["outcome"], "ok");
    assert_eq!(manual["path"], "manual");
    assert_eq!(manual["hostId"], host.host_id);
    assert!(results
        .iter()
        .any(|event| event["port"] == unreachable.port() && event["outcome"] != "ok"));
}

#[test]
fn the_pushed_servers_setting_feeds_session_candidates() {
    let core = test_core();
    with_identity(&core, "hubless");
    save_paired(&core, "h1", [7; 32], Vec::new());
    set_setting(
        &core,
        "sync.servers",
        &servers(vec![computer(json!("h1"), "10.30.0.9", json!(7400))]).to_string(),
    )
    .unwrap();

    let opts = OpenOpts {
        intent: OpenIntent::Control,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    };
    let entry = core
        .state::<SessionTable>()
        .insert("h1", "Paired Mac", opts, core.now_us());
    let params = session_params(&core, &entry).unwrap();
    assert_eq!(
        params.candidates,
        vec![manual_candidate(socket("10.30.0.9:7400"))]
    );
    assert_eq!(params.candidates[0].source, CandidateSource::Manual);
    assert_eq!(params.host_fingerprint, [7; 32]);
    core.state::<SessionTable>().remove(entry.id);
    core.shutdown();
}
