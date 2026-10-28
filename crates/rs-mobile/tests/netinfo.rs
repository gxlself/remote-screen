use std::{collections::BTreeSet, net::IpAddr};

use rs_mobile::rscore::netinfo::{
    has_tailnet, is_tailnet_ip, is_vpn, local_net, local_net_excluding, network_status,
    network_status_marked, read_interfaces, RawIface,
};
use rsp_transport::dial::{plan, Candidate, CandidateSource, IpNet, PathKind, Skipped};
use serde_json::json;

fn iface(name: &str, addrs: &[&str]) -> RawIface {
    RawIface {
        name: name.into(),
        addrs: addrs.iter().map(|addr| addr.parse().unwrap()).collect(),
    }
}

#[test]
fn vpn_pools_and_tunnel_names_classify_interfaces() {
    let pools: Vec<IpNet> = vec!["10.99.0.0/16".parse().unwrap()];
    for (raw, pools, expected) in [
        (iface("utun3", &["100.64.0.12/32"]), &pools[..], false),
        (iface("utun4", &["10.99.4.27/32"]), &pools[..], true),
        (iface("utun5", &["10.8.0.2/24"]), &[][..], true),
        (iface("en0", &["10.99.4.27/24"]), &pools[..], true),
        (iface("en0", &["192.168.31.20/24"]), &pools[..], false),
    ] {
        assert_eq!(is_vpn(&raw, pools), expected, "{}", raw.name);
        assert_eq!(local_net(&[raw], pools).has_vpn(), expected);
    }
}

#[test]
fn tunnel_name_fallback_excludes_tailnet_only_and_empty_interfaces() {
    for name in ["utun3", "ipsec0", "ppp0", "tun0", "tap0", "wg0"] {
        assert!(is_vpn(&iface(name, &["10.99.4.27/32"]), &[]));
        assert!(!is_vpn(
            &iface(name, &["100.64.0.12/32", "fd7a:115c:a1e0::12/128"]),
            &[]
        ));
        assert!(!is_vpn(&iface(name, &[]), &[]));
        assert!(is_vpn(
            &iface(name, &["100.64.0.12/32", "10.99.4.27/32"]),
            &[]
        ));
    }
    assert!(!is_vpn(&iface("en0", &["100.64.0.12/32"]), &[]));
    // An explicitly registered pool takes precedence over tunnel-name exclusion.
    assert!(is_vpn(
        &iface("utun3", &["fd7a:115c:a1e0::12/128"]),
        &["fd7a:115c:a1e0::/48".parse().unwrap()]
    ));
}

#[test]
fn local_net_preserves_interface_addresses_and_direct_routes() {
    let raw = vec![iface("en0", &["192.168.31.20/24"])];
    let local = local_net(&raw, &[]);
    assert_eq!(local.interfaces.len(), 1);
    assert_eq!(local.interfaces[0].name, "en0");
    assert_eq!(local.interfaces[0].addrs, raw[0].addrs);
    assert!(local.is_direct("192.168.31.7".parse().unwrap()));
    assert!(!local.is_direct("10.30.1.5".parse().unwrap()));
    assert!(!local.has_vpn());

    let local = local_net(
        &[
            iface("en0", &["192.168.31.20/24"]),
            iface("utun4", &["10.99.4.27/32"]),
        ],
        &["10.99.0.0/16".parse().unwrap()],
    );
    assert!(local.has_vpn());
    assert!(local.is_direct("192.168.31.7".parse().unwrap()));
}

#[test]
fn network_status_json_is_sorted_and_omits_work_profile() {
    let raw = vec![
        iface("utun4", &["10.99.4.27/32"]),
        iface("utun3", &["100.64.0.12/32", "fd7a:115c:a1e0::12/128"]),
        iface("en0", &["192.168.31.20/24"]),
    ];
    let status = network_status(&raw, &["10.99.0.0/16".parse().unwrap()]);
    assert_eq!(
        serde_json::to_value(status).unwrap(),
        json!({"ifaces": [
            {"name": "en0", "addrs": ["192.168.31.20/24"], "isVpn": false},
            {"name": "utun3", "addrs": ["100.64.0.12/32", "fd7a:115c:a1e0::12/128"], "isVpn": false},
            {"name": "utun4", "addrs": ["10.99.4.27/32"], "isVpn": true}
        ]})
    );
}

#[test]
fn interface_snapshot_excludes_loopback_and_all_link_local_addresses() {
    let raw = read_interfaces().expect("read local interfaces");
    let mut names = BTreeSet::new();
    for iface in raw {
        assert!(names.insert(iface.name));
        assert!(!iface.addrs.is_empty());
        for net in iface.addrs {
            assert!(!net.addr().is_loopback());
            match net.addr() {
                IpAddr::V4(ip) => {
                    assert!(!ip.is_link_local());
                    assert!(net.prefix_len() <= 32);
                }
                IpAddr::V6(ip) => {
                    assert!(!ip.is_unicast_link_local());
                    assert!(net.prefix_len() <= 128);
                }
            }
        }
    }
}

#[test]
fn tailnet_ranges_include_ipv4_and_ipv6_boundaries() {
    for addr in [
        "100.64.0.0",
        "100.64.0.1",
        "100.127.255.254",
        "100.127.255.255",
        "fd7a:115c:a1e0::1",
        "fd7a:115c:a1e0:ffff:ffff:ffff:ffff:ffff",
    ] {
        assert!(is_tailnet_ip(addr.parse().unwrap()), "{addr}");
    }
    for addr in [
        "100.63.255.255",
        "100.128.0.1",
        "10.30.0.1",
        "fd7a:115c:a1df::1",
        "fd7a:115c:a1e1::1",
    ] {
        assert!(!is_tailnet_ip(addr.parse().unwrap()), "{addr}");
    }
}

#[test]
fn has_tailnet_scans_all_interfaces_and_address_families() {
    assert!(!has_tailnet(&[]));
    assert!(!has_tailnet(&[iface("en0", &["192.168.31.20/24"])]));
    for addr in ["100.64.0.12/32", "fd7a:115c:a1e0::12/128"] {
        assert!(has_tailnet(&[
            iface("en0", &["192.168.31.20/24"]),
            iface("utun3", &[addr]),
        ]));
    }
}

#[test]
fn empty_snapshots_produce_empty_network_status_and_routes() {
    assert!(local_net(&[], &[]).interfaces.is_empty());
    assert_eq!(
        serde_json::to_value(network_status(&[], &[])).unwrap(),
        json!({"ifaces": []})
    );
}

#[test]
fn network_status_marks_work_profile_interfaces() {
    let raw = vec![
        iface("utun4", &["10.99.4.27/32"]),
        iface("en0", &["192.168.31.20/24"]),
    ];
    let pools = ["10.99.0.0/16".parse().unwrap()];
    let marked = network_status_marked(&raw, &pools, &["utun4".into()]);
    assert_eq!(
        serde_json::to_value(marked).unwrap(),
        json!({"ifaces": [
            {"name":"en0", "addrs":["192.168.31.20/24"], "isVpn":false},
            {"name":"utun4", "addrs":["10.99.4.27/32"], "isVpn":true, "workProfile":true}
        ]})
    );
    assert_eq!(
        network_status_marked(&raw, &pools, &[]),
        network_status(&raw, &pools)
    );
}

#[test]
fn local_net_excluding_removes_work_profile_vpn_from_routes() {
    let raw = vec![iface("utun4", &["10.99.4.27/32"])];
    let pools = ["10.99.0.0/16".parse().unwrap()];
    let excluded = ["utun4".to_owned()];
    let local = local_net_excluding(&raw, &pools, &excluded);
    assert!(!local.has_vpn());
    assert!(!local.is_direct("10.99.4.1".parse().unwrap()));
    assert_eq!(
        local_net_excluding(&raw, &pools, &[]),
        local_net(&raw, &pools)
    );

    let candidate = Candidate {
        addr: "10.30.1.5:7400".parse().unwrap(),
        kind: PathKind::Intranet,
        source: CandidateSource::Cache,
    };
    let intranet = ["10.30.0.0/16".parse().unwrap()];
    assert!(
        plan(vec![candidate.clone()], &local_net(&raw, &pools), &intranet)
            .skipped
            .is_empty()
    );
    assert_eq!(
        plan(vec![candidate], &local, &intranet).skipped[0].1,
        Skipped::NeedsVpn
    );
}
