mod support;

use std::sync::atomic::Ordering;
use std::sync::Arc;
use std::time::Duration;

use rs_mobile::rscore::{
    api_diag::{refresh_network, set_setting, status, NetworkState},
    error::CoreError,
    netinfo::{network_status, read_interfaces, RawIface},
    runtime::Core,
};
use serde_json::json;

use support::{test_core_with, EventCollector};

pub fn iface(name: &str, addr: &str) -> RawIface {
    RawIface {
        name: name.to_owned(),
        addrs: vec![addr.parse().unwrap()],
    }
}

pub fn changed(core: &Arc<Core>, events: &EventCollector) -> Vec<serde_json::Value> {
    assert!(core.events().flush(Duration::from_secs(1)));
    events.of_type("network.changed")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn status_serializes_interfaces_excluding_loopback_and_starts_one_poller() {
    let core = test_core_with(|_, _| {});
    let snapshot = status(&core).await.unwrap();
    let json = serde_json::to_value(snapshot).unwrap();
    let ifaces = json["ifaces"].as_array().unwrap();
    assert!(ifaces
        .iter()
        .all(|iface| iface["name"] != "lo" && iface["name"] != "lo0"));
    let state = core.state::<NetworkState>();
    assert!(state.polling_started.load(Ordering::Acquire));
    status(&core).await.unwrap();
    assert_eq!(Arc::strong_count(&core), 1, "poller must not retain Core");
    let weak = Arc::downgrade(&core);
    core.shutdown();
    drop(core);
    assert!(weak.upgrade().is_none());
}

#[test]
pub fn only_changed_snapshots_emit_network_events() {
    let core = test_core_with(|_, _| {});
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    let first = [iface("en0", "192.168.31.20/24")];
    refresh_network(&core, &first);
    assert!(changed(&core, &events).is_empty());
    let second = [iface("utun4", "10.99.4.27/32")];
    let expected = refresh_network(&core, &second);
    let changed_events = changed(&core, &events);
    assert_eq!(changed_events.len(), 1);
    assert_eq!(
        changed_events[0],
        json!({"type":"network.changed", "status":expected})
    );
    assert_eq!(expected.ifaces[0].name, "utun4");
    assert_eq!(expected.ifaces[0].addrs, ["10.99.4.27/32"]);
    refresh_network(&core, &second);
    assert_eq!(changed(&core, &events).len(), 1);
}

#[test]
pub fn settings_validate_values_and_recompute_with_registered_vpn_pools() {
    let core = test_core_with(|_, _| {});
    set_setting(
        &core,
        "networks",
        r#"[{"id":"n","name":"公司","cidrs":["10.30.0.0/16"],"vpnPools":["10.99.0.0/16"]}]"#,
    )
    .unwrap();
    let snapshot = refresh_network(&core, &[iface("utun4", "10.99.4.27/32")]);
    assert!(snapshot.ifaces[0].is_vpn);
    // A pool also recognizes VPNs whose interface name has no tunnel prefix.
    assert!(refresh_network(&core, &[iface("en4", "10.99.4.27/32")]).ifaces[0].is_vpn);
    set_setting(&core, "networks", "[]").unwrap();
    assert!(!refresh_network(&core, &[iface("en4", "10.99.4.27/32")]).ifaces[0].is_vpn);
    assert!(matches!(
        set_setting(&core, "quality.default", "\"ultra\""),
        Err(CoreError::Internal { .. })
    ));
    assert!(set_setting(&core, "x.y", "1").is_ok());
    assert_eq!(
        set_setting(&core, "quality.default", "not json"),
        Err(CoreError::internal("bad json"))
    );
    assert!(set_setting(&core, "quality.default", "\"clear\"").is_ok());
}

#[test]
pub fn changing_network_settings_immediately_refreshes_the_interface_snapshot() {
    let core = test_core_with(|_, _| {});
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    refresh_network(&core, &[iface("synthetic", "192.168.31.20/24")]);
    set_setting(
        &core,
        "networks",
        r#"[{"id":"n","name":"公司","cidrs":[],"vpnPools":["10.99.0.0/16"]}]"#,
    )
    .unwrap();
    let result = changed(&core, &events);
    assert_eq!(result.len(), 1);
    let raw = read_interfaces().unwrap();
    let pools = ["10.99.0.0/16".parse().unwrap()];
    assert_eq!(
        result[0]["status"],
        serde_json::to_value(network_status(&raw, &pools)).unwrap()
    );
}
