use std::net::{IpAddr, Ipv4Addr, UdpSocket};
use std::time::Duration;

use host_core::platform::Nic;
use host_core::wol::{
    magic_packet, parse_mac, route_of, select_routes, send_magic_udp, WolError, WOL_PORT,
};

fn nic(name: &str, addresses: &[&str], networks: &[&str], tailnet: bool) -> Nic {
    Nic {
        name: name.to_owned(),
        addresses: addresses
            .iter()
            .map(|value| value.parse::<IpAddr>().expect("test address"))
            .collect(),
        networks: networks.iter().map(|value| (*value).to_owned()).collect(),
        mac: Some([0xa4, 0x83, 0xe7, 0x01, 0x02, 0x0f]),
        tailnet,
    }
}

fn sample_nics() -> Vec<Nic> {
    vec![
        nic("en0", &["192.168.31.20"], &["192.168.31.0/24"], false),
        nic("en1", &["10.30.4.8"], &["10.30.0.0/16"], false),
        nic("utun3", &["100.64.0.7"], &["100.64.0.0/10"], true),
    ]
}

#[test]
fn parses_colon_and_hyphen_mac_addresses() {
    let expected = [0xa4, 0x83, 0xe7, 0x01, 0x02, 0x0f];
    assert_eq!(parse_mac("a4:83:e7:01:02:0f"), Ok(expected));
    assert_eq!(parse_mac("A4-83-E7-01-02-0F"), Ok(expected));
}

#[test]
fn rejects_invalid_mac_addresses() {
    for value in [
        "a4:83:e7-01:02:0f",
        "a4:83:e7:01:02",
        "a4:83:e7:01:02:0g",
        "",
        "a483e701020f",
    ] {
        assert_eq!(parse_mac(value), Err(WolError::Mac), "{value:?}");
    }
}

#[test]
fn builds_magic_packet() {
    let mac = [0xa4, 0x83, 0xe7, 0x01, 0x02, 0x0f];
    let packet = magic_packet(mac);

    assert_eq!(packet.len(), 102);
    assert!(packet[..6].iter().all(|byte| *byte == 0xff));
    assert_eq!(&packet[6..12], mac.as_slice());
    assert_eq!(&packet[96..102], mac.as_slice());
    for chunk in packet[6..].chunks_exact(mac.len()) {
        assert_eq!(chunk, mac.as_slice());
    }
}

#[test]
fn selects_routes_by_broadcast_and_excludes_tailnet() {
    let nics = sample_nics();

    let routes = select_routes(&nics, "192.168.31.255".parse().expect("broadcast"));
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].nic.name, "en0");
    assert_eq!(routes[0].source, Ipv4Addr::new(192, 168, 31, 20));
    assert_eq!(
        routes[0].network,
        "192.168.31.0/24".parse().expect("network")
    );
    assert_eq!(routes[0].broadcast, Ipv4Addr::new(192, 168, 31, 255));
    assert_eq!(routes[0].nic.networks, vec!["192.168.31.0/24"]);
    assert_eq!(
        routes[0].nic.addresses,
        vec![IpAddr::V4(Ipv4Addr::new(192, 168, 31, 20))]
    );

    let routes = select_routes(&nics, "10.30.255.255".parse().expect("broadcast"));
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].nic.name, "en1");
    assert_eq!(routes[0].source, Ipv4Addr::new(10, 30, 4, 8));

    assert!(select_routes(&nics, "100.127.255.255".parse().expect("broadcast")).is_empty());
    assert!(select_routes(&nics, Ipv4Addr::BROADCAST).is_empty());
}

#[test]
fn ignores_ipv6_and_short_prefixes() {
    let mixed = nic(
        "en0",
        &["fd00::20", "192.168.31.20"],
        &["fd00::/64", "192.168.31.0/24"],
        false,
    );
    let routes = select_routes(&[mixed], "192.168.31.255".parse().expect("broadcast"));
    assert_eq!(routes.len(), 1);
    assert_eq!(routes[0].nic.networks, vec!["192.168.31.0/24"]);
    assert_eq!(
        routes[0].nic.addresses,
        vec![IpAddr::V4(Ipv4Addr::new(192, 168, 31, 20))]
    );

    let point_to_point = nic(
        "p2p0",
        &["192.168.40.1", "192.168.41.20"],
        &["192.168.40.0/31", "192.168.41.20/32"],
        false,
    );
    assert!(select_routes(
        std::slice::from_ref(&point_to_point),
        "192.168.40.1".parse().expect("broadcast")
    )
    .is_empty());
    assert!(select_routes(
        std::slice::from_ref(&point_to_point),
        "192.168.41.20".parse().expect("broadcast")
    )
    .is_empty());
}

#[test]
fn truncates_networks_and_chooses_first_sorted_source() {
    let nic = nic(
        "en0",
        &["fd00::20", "192.168.31.30", "192.168.31.4", "10.0.0.2"],
        &["not-a-network", "192.168.31.99/24"],
        false,
    );

    let routes = select_routes(&[nic], "192.168.31.255".parse().expect("broadcast"));
    assert_eq!(routes.len(), 1);
    assert_eq!(
        routes[0].network,
        "192.168.31.0/24".parse().expect("network")
    );
    assert_eq!(routes[0].source, Ipv4Addr::new(192, 168, 31, 4));
    assert_eq!(routes[0].nic.networks, vec!["192.168.31.0/24"]);
    assert_eq!(
        routes[0].nic.addresses,
        vec![
            IpAddr::V4(Ipv4Addr::new(192, 168, 31, 4)),
            IpAddr::V4(Ipv4Addr::new(192, 168, 31, 30)),
        ]
    );
    assert_eq!(
        route_of(&routes[0].nic),
        Some((routes[0].source, routes[0].broadcast))
    );
}

#[test]
fn sorts_multiple_matching_routes_by_nic_name() {
    let nics = vec![
        nic("en1", &["192.168.31.21"], &["192.168.31.0/24"], false),
        nic("en0", &["192.168.31.20"], &["192.168.31.0/24"], false),
    ];

    let routes = select_routes(&nics, "192.168.31.255".parse().expect("broadcast"));
    assert_eq!(
        routes
            .iter()
            .map(|route| route.nic.name.as_str())
            .collect::<Vec<_>>(),
        vec!["en0", "en1"]
    );
}

#[test]
fn sends_magic_packet_over_udp() {
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("read timeout");
    let port = receiver.local_addr().expect("receiver address").port();
    let mac = [0xa4, 0x83, 0xe7, 0x01, 0x02, 0x0f];

    send_magic_udp(mac, Ipv4Addr::LOCALHOST, Ipv4Addr::LOCALHOST, port).expect("send packet");

    let mut buffer = [0_u8; 256];
    let (length, _) = receiver.recv_from(&mut buffer).expect("receive packet");
    assert_eq!(&buffer[..length], magic_packet(mac).as_slice());
    assert_eq!(WOL_PORT, 9);
}

#[test]
fn resolves_route_from_narrowed_nic() {
    let narrowed = nic("en0", &["192.168.31.20"], &["192.168.31.0/24"], false);
    assert_eq!(
        route_of(&narrowed),
        Some((
            Ipv4Addr::new(192, 168, 31, 20),
            Ipv4Addr::new(192, 168, 31, 255)
        ))
    );

    let ipv6_only = nic("en0", &["fd00::20"], &["fd00::/64"], false);
    assert_eq!(route_of(&ipv6_only), None);
}
