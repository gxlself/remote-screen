use std::net::{IpAddr, Ipv4Addr};
use std::sync::Arc;
use std::time::Duration;

use host_core::listen::{admit, bind_addrs, ListenPolicy, PathClass};
use host_core::platform::Nic;
use ipnet::IpNet;

fn ipnet(value: &str) -> IpNet {
    value.parse().expect("test CIDR")
}

fn nic(name: &str, addresses: &[IpAddr]) -> Nic {
    Nic {
        name: name.to_owned(),
        addresses: addresses.to_vec(),
        networks: Vec::new(),
        mac: None,
        tailnet: false,
    }
}

#[test]
fn bind_addrs_and_admit_cover_tailnet_intranet_lan_and_mapped_sources() {
    let policy = ListenPolicy {
        tailnet: true,
        intranet_nics: vec!["en0".to_owned()],
        allowed_cidrs: vec![ipnet("192.168.1.0/24"), ipnet("10.30.0.0/16")],
        lan_cidrs: vec![ipnet("192.168.1.0/24")],
        port: 7400,
    };
    let nics = vec![
        nic(
            "tailscale0",
            &[
                IpAddr::V4(Ipv4Addr::new(100, 64, 0, 2)),
                IpAddr::V6("fd7a:115c:a1e0::2".parse().expect("tailnet v6")),
            ],
        ),
        nic("en0", &[IpAddr::V4(Ipv4Addr::new(192, 168, 1, 20))]),
    ];
    let bound = bind_addrs(&policy, &nics);
    assert!(bound.contains(&(
        "100.64.0.2:7400".parse().expect("address"),
        PathClass::Tailnet
    )));
    assert!(bound.contains(&(
        "[fd7a:115c:a1e0::2]:7400".parse().expect("address"),
        PathClass::Tailnet
    )));
    assert!(bound.contains(&(
        "192.168.1.20:7400".parse().expect("address"),
        PathClass::Lan
    )));

    assert_eq!(
        admit(
            "100.64.0.2:7400".parse().expect("local"),
            "203.0.113.9:4000".parse().expect("remote"),
            &policy,
        ),
        Some(PathClass::Tailnet)
    );
    assert_eq!(
        admit(
            "192.168.1.20:7400".parse().expect("local"),
            "192.168.1.8:4000".parse().expect("remote"),
            &policy,
        ),
        Some(PathClass::Lan)
    );
    assert_eq!(
        admit(
            "192.168.1.20:7400".parse().expect("local"),
            "10.30.4.8:4000".parse().expect("remote"),
            &policy,
        ),
        Some(PathClass::Intranet)
    );
    assert_eq!(
        admit(
            "192.168.1.20:7400".parse().expect("local"),
            "172.16.1.8:4000".parse().expect("remote"),
            &policy,
        ),
        None
    );
    assert_eq!(
        admit(
            "192.168.1.20:7400".parse().expect("local"),
            "[::ffff:192.168.1.8]:4000".parse().expect("mapped remote"),
            &policy,
        ),
        Some(PathClass::Lan)
    );

    let mut closed = policy.clone();
    closed.intranet_nics.clear();
    assert!(bind_addrs(&closed, &nics)
        .iter()
        .all(|(_, path)| *path == PathClass::Tailnet));
    assert_eq!(
        admit(
            "192.168.1.20:7400".parse().expect("local"),
            "192.168.1.8:4000".parse().expect("remote"),
            &closed,
        ),
        None
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_uses_nic_tailnet_injection_and_filters_tcp_before_tls() {
    use host_core::listen::HostListener;
    use rsp_transport::identity::{CertKind, LocalIdentity};
    use rsp_transport::tls::{client_config, server_config, ServerPin};
    use rsp_transport::{connect_tcp, TransportKind};
    use tokio::io::AsyncReadExt;
    use tokio::net::TcpSocket;

    let host = LocalIdentity::generate_software(CertKind::Host).expect("host identity");
    let app = LocalIdentity::generate_software(CertKind::App).expect("app identity");
    let tls = Arc::new(server_config(&host).expect("server config"));
    let policy = ListenPolicy {
        tailnet: true,
        intranet_nics: vec!["lan0".to_owned()],
        allowed_cidrs: vec![ipnet("127.0.0.1/32")],
        lan_cidrs: Vec::new(),
        port: 0,
    };
    let nics = vec![
        Nic {
            name: "tailscale-test".to_owned(),
            addresses: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
            networks: Vec::new(),
            mac: None,
            tailnet: true,
        },
        nic("lan0", &[IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))]),
    ];
    let listener = HostListener::start(policy, nics, tls)
        .await
        .expect("listener");
    let tcp_addr = listener
        .bound()
        .into_iter()
        .find_map(|(addr, path, transport)| {
            (transport == TransportKind::Tcp && path == PathClass::Intranet).then_some(addr)
        })
        .expect("intranet TCP listener");

    let client_cfg = Arc::new(
        client_config(&app, ServerPin::Fingerprint(host.fingerprint())).expect("client config"),
    );
    let client = connect_tcp(tcp_addr, client_cfg)
        .await
        .expect("allowed TCP");
    let control = client.open_control().await.expect("control");
    control.send(b"hello").await.expect("first record");
    let accepted = tokio::time::timeout(Duration::from_secs(2), listener.accept())
        .await
        .expect("accept timeout")
        .expect("accepted connection");
    assert_eq!(accepted.path, PathClass::Intranet);
    assert_eq!(accepted.conn.kind(), TransportKind::Tcp);

    let socket = TcpSocket::new_v4().expect("source socket");
    socket
        .bind("127.0.0.3:0".parse().expect("source address"))
        .expect("bind source socket");
    let mut socket = socket.connect(tcp_addr).await.expect("connect disallowed");
    let mut one = [0u8; 1];
    let read = tokio::time::timeout(Duration::from_secs(2), socket.read(&mut one))
        .await
        .expect("close timeout")
        .expect("read");
    assert_eq!(read, 0, "source filtering must close before TLS");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_ignores_disallowed_quic_source_without_a_handshake_response() {
    use host_core::listen::HostListener;
    use rsp_transport::identity::{CertKind, LocalIdentity};
    use rsp_transport::quic::connect_quic;
    use rsp_transport::tls::{client_config, server_config, ServerPin};
    use rsp_transport::TransportKind;

    let host = LocalIdentity::generate_software(CertKind::Host).expect("host identity");
    let app = LocalIdentity::generate_software(CertKind::App).expect("app identity");
    let tls = Arc::new(server_config(&host).expect("server config"));
    let policy = ListenPolicy {
        tailnet: false,
        intranet_nics: vec!["lan0".to_owned()],
        allowed_cidrs: vec![ipnet("127.0.0.1/32")],
        lan_cidrs: Vec::new(),
        port: 0,
    };
    let listener = HostListener::start(
        policy,
        vec![nic("lan0", &[IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))])],
        tls,
    )
    .await
    .expect("listener");
    let quic_addr = listener
        .bound()
        .into_iter()
        .find_map(|(addr, path, transport)| {
            (transport == TransportKind::Quic && path == PathClass::Intranet).then_some(addr)
        })
        .expect("intranet QUIC listener");
    let client_cfg = Arc::new(
        client_config(&app, ServerPin::Fingerprint(host.fingerprint())).expect("client config"),
    );
    let endpoint = quinn::Endpoint::client("127.0.0.3:0".parse().expect("client address"))
        .expect("client endpoint");
    let result = tokio::time::timeout(
        Duration::from_millis(500),
        connect_quic(&endpoint, quic_addr, client_cfg, false),
    )
    .await;
    assert!(
        result.is_err(),
        "ignored QUIC source must not complete a handshake"
    );
    assert!(listener
        .stats()
        .iter()
        .any(|stats| stats.ignored > 0 && stats.tls_failed == 0));
    endpoint.close(0u32.into(), b"done");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn listener_update_adds_and_removes_addresses_without_closing_existing_connections() {
    use host_core::listen::HostListener;
    use rsp_transport::identity::{CertKind, LocalIdentity};
    use rsp_transport::tls::{client_config, server_config, ServerPin};
    use rsp_transport::{connect_tcp, TransportKind};
    use tokio::net::TcpStream;

    let host = LocalIdentity::generate_software(CertKind::Host).expect("host identity");
    let app = LocalIdentity::generate_software(CertKind::App).expect("app identity");
    let tls = Arc::new(server_config(&host).expect("server config"));
    let policy = ListenPolicy {
        tailnet: true,
        intranet_nics: Vec::new(),
        allowed_cidrs: Vec::new(),
        lan_cidrs: Vec::new(),
        port: 0,
    };
    let first_nic = Nic {
        name: "tailscale-test".to_owned(),
        addresses: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 1))],
        networks: Vec::new(),
        mac: None,
        tailnet: true,
    };
    let second_nic = Nic {
        name: "tailscale-test-2".to_owned(),
        addresses: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2))],
        networks: Vec::new(),
        mac: None,
        tailnet: true,
    };
    let listener = HostListener::start(policy.clone(), vec![first_nic.clone()], tls)
        .await
        .expect("listener");
    let client_cfg = Arc::new(
        client_config(&app, ServerPin::Fingerprint(host.fingerprint())).expect("client config"),
    );
    let first_addr = listener
        .bound()
        .into_iter()
        .find_map(|(addr, path, transport)| {
            (transport == TransportKind::Tcp && path == PathClass::Tailnet).then_some(addr)
        })
        .expect("first address");
    let first_client = connect_tcp(first_addr, client_cfg.clone())
        .await
        .expect("first connection");
    let first_control = first_client.open_control().await.expect("first control");
    first_control.send(b"before update").await.expect("send");
    let first_accepted = listener.accept().await.expect("first accepted");
    let first_server_control = first_accepted
        .conn
        .accept_control()
        .await
        .expect("first server control");
    assert_eq!(
        first_server_control
            .recv()
            .await
            .expect("record")
            .expect("payload"),
        &b"before update"[..]
    );

    listener
        .update(policy.clone(), vec![first_nic.clone(), second_nic])
        .await
        .expect("add address");
    let second_addr = listener
        .bound()
        .into_iter()
        .find_map(|(addr, path, transport)| {
            (transport == TransportKind::Tcp
                && path == PathClass::Tailnet
                && addr.ip() == IpAddr::V4(Ipv4Addr::new(127, 0, 0, 2)))
            .then_some(addr)
        })
        .expect("second address");
    let second_client = connect_tcp(second_addr, client_cfg)
        .await
        .expect("second connection");
    let second_control = second_client.open_control().await.expect("second control");
    second_control.send(b"new address").await.expect("send");
    let second_accepted = listener.accept().await.expect("second accepted");
    assert_eq!(second_accepted.path, PathClass::Tailnet);

    listener
        .update(policy, vec![first_nic])
        .await
        .expect("remove address");
    first_control
        .send(b"after update")
        .await
        .expect("existing connection");
    assert_eq!(
        first_server_control
            .recv()
            .await
            .expect("record")
            .expect("payload"),
        &b"after update"[..]
    );
    assert!(
        tokio::time::timeout(Duration::from_millis(500), TcpStream::connect(second_addr))
            .await
            .expect("removed connect timeout")
            .is_err()
    );
}
