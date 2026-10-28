use std::net::{IpAddr, Ipv4Addr};

use host_core::nic_model::{is_tailnet_address, to_nics, AdapterRecord};

fn record(
    friendly_name: &str,
    description: &str,
    if_type: u32,
    oper_up: bool,
    mac: &[u8],
    unicast: &[(IpAddr, u8)],
) -> AdapterRecord {
    AdapterRecord {
        friendly_name: friendly_name.to_owned(),
        description: description.to_owned(),
        if_type,
        oper_up,
        mac: mac.to_vec(),
        unicast: unicast.to_vec(),
    }
}

#[test]
fn filters_adapters_and_builds_cidr_networks() {
    let nics = to_nics(&[
        record(
            "以太网",
            "Ethernet",
            6,
            true,
            &[0, 0x15, 0x5d, 1, 2, 3],
            &[
                ("fe80::1".parse().expect("valid IPv6"), 64),
                ("10.30.12.48".parse().expect("valid IPv4"), 24),
            ],
        ),
        record(
            "Loopback Pseudo-Interface 1",
            "loopback",
            24,
            true,
            &[],
            &[("127.0.0.1".parse().expect("valid IPv4"), 8)],
        ),
        record(
            "WLAN",
            "Wireless",
            71,
            false,
            &[1, 2, 3, 4, 5, 6],
            &[("192.168.31.5".parse().expect("valid IPv4"), 24)],
        ),
    ]);

    assert_eq!(nics.len(), 1);
    assert_eq!(nics[0].name, "以太网");
    assert_eq!(
        nics[0].addresses,
        vec![
            "10.30.12.48".parse::<IpAddr>().expect("valid IPv4"),
            "fe80::1".parse::<IpAddr>().expect("valid IPv6"),
        ]
    );
    assert_eq!(
        nics[0].networks,
        vec!["10.30.12.0/24".to_owned(), "fe80::/64".to_owned()]
    );
    assert_eq!(nics[0].mac, Some([0, 0x15, 0x5d, 1, 2, 3]));
    assert!(!nics[0].tailnet);
}

#[test]
fn recognizes_tailnet_and_rejects_invalid_macs() {
    let nics = to_nics(&[record(
        "Tailscale",
        "Tailscale",
        53,
        true,
        &[],
        &[
            ("100.64.0.12".parse().expect("valid IPv4"), 32),
            ("fd7a:115c:a1e0::c".parse().expect("valid IPv6"), 128),
        ],
    )]);

    assert_eq!(nics.len(), 1);
    assert!(nics[0].tailnet);
    assert_eq!(nics[0].mac, None);
    assert_eq!(
        nics[0].networks,
        vec![
            "100.64.0.12/32".to_owned(),
            "fd7a:115c:a1e0::c/128".to_owned(),
        ]
    );

    let invalid = to_nics(&[
        record(
            "zero",
            "zero",
            6,
            true,
            &[0; 6],
            &[("10.0.0.1".parse().expect("valid IPv4"), 24)],
        ),
        record(
            "eight",
            "eight",
            6,
            true,
            &[1, 2, 3, 4, 5, 6, 7, 8],
            &[("10.0.0.2".parse().expect("valid IPv4"), 24)],
        ),
    ]);
    assert_eq!(invalid[0].mac, None);
    assert_eq!(invalid[1].mac, None);
}

#[test]
fn skips_empty_addresses_and_keeps_invalid_prefix_address() {
    let nics = to_nics(&[
        record("empty", "empty", 6, true, &[1, 2, 3, 4, 5, 6], &[]),
        record(
            "prefix",
            "prefix",
            6,
            true,
            &[1, 2, 3, 4, 5, 6],
            &[("10.0.0.1".parse().expect("valid IPv4"), 40)],
        ),
    ]);

    assert_eq!(nics.len(), 1);
    assert_eq!(
        nics[0].addresses,
        vec!["10.0.0.1".parse::<IpAddr>().expect("valid IPv4")]
    );
    assert!(nics[0].networks.is_empty());
}

#[test]
fn sorts_names_addresses_and_uses_description_fallback() {
    let nics = to_nics(&[
        record(
            "b",
            "b description",
            6,
            true,
            &[1, 2, 3, 4, 5, 6],
            &[("10.0.0.2".parse().expect("valid IPv4"), 24)],
        ),
        record(
            "a",
            "a description",
            6,
            true,
            &[1, 2, 3, 4, 5, 7],
            &[
                ("2001:db8::2".parse().expect("valid IPv6"), 64),
                ("10.0.0.3".parse().expect("valid IPv4"), 24),
                ("10.0.0.1".parse().expect("valid IPv4"), 24),
                ("10.0.0.1".parse().expect("valid IPv4"), 24),
            ],
        ),
        record(
            "",
            "description fallback",
            6,
            true,
            &[1, 2, 3, 4, 5, 8],
            &[("192.168.1.1".parse().expect("valid IPv4"), 24)],
        ),
    ]);

    assert_eq!(
        nics.iter().map(|nic| nic.name.as_str()).collect::<Vec<_>>(),
        vec!["a", "b", "description fallback"]
    );
    assert_eq!(
        nics[0].addresses,
        vec![
            "10.0.0.1".parse::<IpAddr>().expect("valid IPv4"),
            "10.0.0.3".parse::<IpAddr>().expect("valid IPv4"),
            "2001:db8::2".parse::<IpAddr>().expect("valid IPv6"),
        ]
    );
    assert_eq!(
        nics[0].networks,
        vec!["10.0.0.0/24".to_owned(), "2001:db8::/64".to_owned()]
    );
}

#[test]
fn tailnet_ranges_are_exact() {
    assert!(is_tailnet_address(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
    assert!(is_tailnet_address(IpAddr::V4(Ipv4Addr::new(
        100, 127, 255, 255,
    ))));
    assert!(is_tailnet_address(IpAddr::V6(
        "fd7a:115c:a1e0::1".parse().expect("valid IPv6"),
    )));
    assert!(!is_tailnet_address(IpAddr::V4(Ipv4Addr::new(
        100, 128, 0, 1
    ))));
    assert!(!is_tailnet_address(IpAddr::V4(Ipv4Addr::new(
        192, 168, 31, 5,
    ))));
    assert!(!is_tailnet_address(IpAddr::V6(
        "fd7a:115c:a1e1::1".parse().expect("valid IPv6"),
    )));
}
