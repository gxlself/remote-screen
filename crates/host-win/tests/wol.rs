use host_core::platform::Nic;
use host_core::wol::magic_packet;
use host_win::wol::{send_wol, send_wol_to_port};
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr, UdpSocket};
use std::time::Duration;

#[test]
fn sends_one_magic_packet_to_requested_port() {
    let receiver = UdpSocket::bind("127.0.0.1:0").expect("bind receiver");
    let port = receiver.local_addr().expect("receiver address").port();
    receiver
        .set_read_timeout(Some(Duration::from_secs(2)))
        .expect("set read timeout");
    let nic = Nic {
        name: "loopback-test".to_owned(),
        addresses: vec![IpAddr::V4(Ipv4Addr::LOCALHOST)],
        networks: vec!["127.0.0.1/32".to_owned()],
        mac: None,
        tailnet: false,
    };
    let mac = [0, 0x15, 0x5d, 1, 2, 3];

    send_wol_to_port(mac, &nic, port).expect("send packet");
    let mut packet = [0_u8; 1024];
    let (length, source): (usize, SocketAddr) =
        receiver.recv_from(&mut packet).expect("receive packet");
    assert_eq!(source.ip(), IpAddr::V4(Ipv4Addr::LOCALHOST));
    assert_eq!(&packet[..length], &magic_packet(mac));
    assert_eq!(length, 102);

    receiver
        .set_read_timeout(Some(Duration::from_millis(100)))
        .expect("set short read timeout");
    assert!(receiver.recv_from(&mut packet).is_err());
}

#[test]
fn send_wol_rejects_ipv6_only_nic() {
    let nic = Nic {
        name: "ipv6-only".to_owned(),
        addresses: vec![IpAddr::V6(Ipv6Addr::LOCALHOST)],
        networks: vec!["::1/128".to_owned()],
        mac: None,
        tailnet: false,
    };
    let error = send_wol([0, 1, 2, 3, 4, 5], &nic).expect_err("IPv6-only NIC must fail");
    assert_eq!(
        error.to_string(),
        "platform operation failed: WOL requires an IPv4 network with a broadcast address"
    );
}
