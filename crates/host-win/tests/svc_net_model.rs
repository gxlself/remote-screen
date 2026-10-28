use std::fs;
use std::time::Duration;

use host_core::config::{apply_patch, HostConfig};
use host_core::listen::bind_addrs;
use host_core::platform::Nic;
use host_win::svc_net_model::*;
use serde_json::json;

const MAC: [u8; 6] = [0x02, 0xab, 0xcd, 0x00, 0xef, 0xff];

fn names(values: &[&str]) -> Vec<String> {
    values.iter().map(|value| (*value).to_owned()).collect()
}

fn nic(name: &str) -> Nic {
    Nic {
        name: name.into(),
        addresses: vec!["10.30.0.2".parse().unwrap()],
        networks: vec!["10.30.0.0/24".into()],
        mac: Some(MAC),
        tailnet: false,
    }
}

fn book(name: &str) -> NicBook {
    [(name.to_owned(), mac_string(MAC))].into()
}

#[test]
fn matching_name_records_mac_and_preserves_order() {
    assert_eq!(mac_string(MAC), "02:ab:cd:00:ef:ff");
    assert_eq!(BIND_RETRY, Duration::from_secs(15));
    let result = follow_nic_names(
        &names(&["以太网", "missing"]),
        &[nic("以太网")],
        &NicBook::new(),
    );
    assert_eq!(result.interfaces, names(&["以太网", "missing"]));
    assert_eq!(result.book, book("以太网"));
    assert!(result.renames.is_empty());
}

#[test]
fn unique_mac_follows_rename_and_restores_bind_addresses() {
    let nics = [nic("LAN-test")];
    let result = follow_nic_names(&names(&["以太网"]), &nics, &book("以太网"));
    assert_eq!(result.interfaces, names(&["LAN-test"]));
    assert_eq!(result.renames, vec![("以太网".into(), "LAN-test".into())]);
    assert_eq!(result.book, book("LAN-test"));
    let mut config = HostConfig::default();
    config.intranet.enabled = true;
    config.intranet.interfaces = names(&["以太网"]);
    assert!(bind_addrs(&listen_policy(&config, 7400), &nics).is_empty());
    let updated = apply_patch(&config, &interfaces_patch(&result.interfaces)).unwrap();
    let bound = bind_addrs(&listen_policy(&updated, 7400), &nics);
    assert_eq!(bound.len(), 1);
    assert_eq!(bound[0].0, "10.30.0.2:7400".parse().unwrap());
    let stable = follow_nic_names(&result.interfaces, &nics, &result.book);
    assert_eq!(stable.interfaces, result.interfaces);
    assert!(stable.renames.is_empty());
}

#[test]
fn ambiguous_mac_keeps_missing_name() {
    let result = follow_nic_names(
        &names(&["old"]),
        &[nic("physical"), nic("virtual")],
        &book("old"),
    );
    assert_eq!(result.interfaces, names(&["old"]));
    assert!(result.renames.is_empty());
    assert_eq!(result.book, book("old"));
}

#[test]
fn configured_candidate_cannot_be_claimed_even_if_it_appears_later() {
    for configured in [names(&["old", "LAN-test"]), names(&["LAN-test", "old"])] {
        let result = follow_nic_names(&configured, &[nic("LAN-test")], &book("old"));
        assert_eq!(result.interfaces, configured);
        assert!(result.renames.is_empty());
    }
}

#[test]
fn candidate_can_only_be_claimed_once() {
    let mut known = book("first");
    known.extend(book("second"));
    let result = follow_nic_names(&names(&["first", "second"]), &[nic("LAN-test")], &known);
    assert_eq!(result.interfaces, names(&["LAN-test", "second"]));
    assert_eq!(result.renames, vec![("first".into(), "LAN-test".into())]);
    assert!(!result.book.contains_key("first"));
}

#[test]
fn tailnet_candidates_are_ignored() {
    let mut tailnet = nic("Tailscale");
    tailnet.tailnet = true;
    tailnet.addresses = vec!["100.64.0.2".parse().unwrap()];
    let result = follow_nic_names(&names(&["old"]), &[tailnet.clone()], &book("old"));
    assert_eq!(result.interfaces, names(&["old"]));
    assert!(result.renames.is_empty());
    let result = follow_nic_names(&names(&["old"]), &[tailnet, nic("LAN-test")], &book("old"));
    assert_eq!(result.interfaces, names(&["LAN-test"]));
}

#[test]
fn missing_book_or_changed_wifi_mac_does_not_guess() {
    let nics = [nic("LAN-test")];
    for known in [
        NicBook::new(),
        [("old".into(), "02:00:00:00:00:01".into())].into(),
    ] {
        let result = follow_nic_names(&names(&["old"]), &nics, &known);
        assert_eq!(result.interfaces, names(&["old"]));
        assert!(result.renames.is_empty());
    }
}

#[test]
fn missing_or_zero_mac_is_neither_recorded_nor_matched() {
    for mac in [None, Some([0; 6])] {
        let mut adapter = nic("LAN-test");
        adapter.mac = mac;
        let same_name =
            follow_nic_names(&names(&["LAN-test"]), &[adapter.clone()], &NicBook::new());
        assert!(same_name.book.is_empty());
        for old_mac in [mac_string(MAC), mac_string([0; 6])] {
            let known = [("old".into(), old_mac)].into();
            let renamed = follow_nic_names(&names(&["old"]), &[adapter.clone()], &known);
            assert!(renamed.renames.is_empty());
        }
    }
}

#[test]
fn same_name_wins_even_if_the_recorded_mac_changed() {
    let mut adapter = nic("old");
    adapter.mac = Some([2, 0, 0, 0, 0, 1]);
    let result = follow_nic_names(&names(&["old"]), &[adapter, nic("candidate")], &book("old"));
    assert_eq!(result.interfaces, names(&["old"]));
    assert!(result.renames.is_empty());
    assert_eq!(result.book["old"], "02:00:00:00:00:01");
}

#[test]
fn deduplication_keeps_order_and_prunes_stale_book_entries() {
    let mut known = book("stale");
    known.extend(book("old"));
    let result = follow_nic_names(
        &names(&["old", "missing", "old", "missing"]),
        &[nic("LAN-test")],
        &known,
    );
    assert_eq!(result.interfaces, names(&["LAN-test", "missing"]));
    assert_eq!(result.book, book("LAN-test"));
    assert_eq!(follow_nic_names(&[], &[], &known), NicFollow::default());
}

#[test]
fn book_roundtrip_replacement_missing_corruption_and_temporary_cleanup() {
    let dir = std::env::temp_dir().join(format!("rs-svc-net-book-{}", std::process::id()));
    fs::create_dir_all(&dir).unwrap();
    let path = dir.join(NIC_BOOK);
    assert!(load_book(&path).is_empty());
    let known = book("以太网");
    save_book(&path, &known).unwrap();
    assert_eq!(load_book(&path), known);
    save_book(&path, &book("LAN-test")).unwrap();
    assert_eq!(load_book(&path), book("LAN-test"));
    assert_eq!(fs::read_dir(&dir).unwrap().count(), 1);
    fs::write(&path, b"{bad}").unwrap();
    assert!(load_book(&path).is_empty());
    fs::write(&path, br#"{"LAN-test":42}"#).unwrap();
    assert!(load_book(&path).is_empty());
    fs::remove_dir_all(dir).unwrap();
}

#[test]
fn listening_policy_matches_agent_defaults_and_skips_invalid_sources() {
    let mut config = HostConfig::default();
    config.intranet.interfaces = names(&["LAN-test"]);
    config.intranet.allow_sources = names(&["10.30.0.0/16", "invalid", "10.99.0.0/16"]);
    let disabled = listen_policy(&config, 7410);
    assert!(disabled.intranet_nics.is_empty());
    assert!(disabled.tailnet);
    assert_eq!(disabled.port, 7410);
    assert!(disabled.lan_cidrs.is_empty());
    assert_eq!(
        disabled
            .allowed_cidrs
            .iter()
            .map(ToString::to_string)
            .collect::<Vec<_>>(),
        names(&["10.30.0.0/16", "10.99.0.0/16"])
    );
    config.intranet.enabled = true;
    assert_eq!(
        listen_policy(&config, 7411).intranet_nics,
        config.intranet.interfaces
    );
}

#[test]
fn interfaces_patch_is_accepted_by_the_public_config_api() {
    let interfaces = names(&["以太网", "LAN-test"]);
    let patch = interfaces_patch(&interfaces);
    assert_eq!(patch, json!({"intranet": {"interfaces": interfaces}}));
    let original = HostConfig::default();
    let updated = apply_patch(&original, &patch).unwrap();
    assert_eq!(updated.intranet.interfaces, interfaces);
    assert_eq!(updated.intranet.enabled, original.intranet.enabled);
    assert_eq!(
        updated.intranet.allow_sources,
        original.intranet.allow_sources
    );
}

#[test]
fn nic_json_matches_network_ui_shape_including_null_mac() {
    let mut adapter = nic("LAN-test");
    adapter.addresses.push("fd7a:115c:a1e0::1".parse().unwrap());
    assert_eq!(
        nic_json(&adapter),
        json!({
            "name": "LAN-test",
            "addresses": ["10.30.0.2", "fd7a:115c:a1e0::1"],
            "networks": ["10.30.0.0/24"],
            "mac": "02:ab:cd:00:ef:ff",
            "tailnet": false
        })
    );
    adapter.mac = None;
    adapter.tailnet = true;
    assert!(nic_json(&adapter)["mac"].is_null());
    assert_eq!(nic_json(&adapter)["tailnet"], true);
}
