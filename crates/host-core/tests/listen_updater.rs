use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::sync::Arc;
use std::time::Duration;

use host_core::listen::{HostListener, ListenError, ListenPolicy, PathClass};
use host_core::platform::Nic;
use rsp_transport::identity::{CertKind, LocalIdentity};
use rsp_transport::tls::{client_config, server_config, ServerPin};
use rsp_transport::{connect_tcp, TransportKind};
use tokio::sync::mpsc;
use tokio::time::timeout;

fn nic(last: u8) -> Nic {
    Nic {
        name: format!("tailscale-test-{last}"),
        addresses: vec![IpAddr::V4(Ipv4Addr::new(127, 0, 0, last))],
        networks: Vec::new(),
        mac: None,
        tailnet: true,
    }
}

fn tcp_addr(listener: &HostListener, last: u8) -> SocketAddr {
    listener
        .bound()
        .into_iter()
        .find_map(|(addr, path, transport)| {
            (transport == TransportKind::Tcp
                && path == PathClass::Tailnet
                && addr.ip() == IpAddr::V4(Ipv4Addr::new(127, 0, 0, last)))
            .then_some(addr)
        })
        .expect("TCP listener")
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updater_and_clone_work_while_accepting_and_preserve_connections() {
    timeout(Duration::from_secs(10), async {
        let host = LocalIdentity::generate_software(CertKind::Host).unwrap();
        let app = LocalIdentity::generate_software(CertKind::App).unwrap();
        let policy = ListenPolicy {
            port: 0,
            ..ListenPolicy::default()
        };
        let listener = Arc::new(
            HostListener::start(
                policy.clone(),
                vec![nic(1)],
                Arc::new(server_config(&host).unwrap()),
            )
            .await
            .unwrap(),
        );
        let updater = listener.updater();
        let cloned_updater = updater.clone();
        let (accepted_tx, mut accepted_rx) = mpsc::channel(4);
        let accepting = listener.clone();
        let accept_task = tokio::spawn(async move {
            loop {
                let accepted = accepting.accept().await.unwrap();
                if accepted_tx.send(accepted).await.is_err() {
                    break;
                }
            }
        });
        let client_cfg =
            Arc::new(client_config(&app, ServerPin::Fingerprint(host.fingerprint())).unwrap());
        let client = connect_tcp(tcp_addr(&listener, 1), client_cfg.clone())
            .await
            .unwrap();
        let control = client.open_control().await.unwrap();
        control.send(b"before update").await.unwrap();
        let accepted = accepted_rx.recv().await.unwrap();
        let server_control = accepted.conn.accept_control().await.unwrap();
        assert_eq!(
            server_control.recv().await.unwrap().unwrap(),
            &b"before update"[..]
        );

        let update_policy = policy.clone();
        tokio::spawn(async move { updater.update(update_policy, vec![nic(1), nic(2)]).await })
            .await
            .unwrap()
            .unwrap();
        let second_addr = tcp_addr(&listener, 2);
        let second = connect_tcp(second_addr, client_cfg).await.unwrap();
        let second_control = second.open_control().await.unwrap();
        second_control.send(b"new address").await.unwrap();
        let second_accepted = accepted_rx.recv().await.unwrap();
        assert_eq!(second_accepted.local, second_addr);
        let second_server = second_accepted.conn.accept_control().await.unwrap();
        assert_eq!(
            second_server.recv().await.unwrap().unwrap(),
            &b"new address"[..]
        );

        // The cloned handle removes the original listener, but its connection
        // and the connection on the new address both remain usable.
        cloned_updater.update(policy, vec![nic(2)]).await.unwrap();
        assert!(listener
            .bound()
            .iter()
            .all(|(addr, _, _)| addr.ip() == nic(2).addresses[0]));
        assert_eq!(tcp_addr(&listener, 2), second_addr);
        control.send(b"after update").await.unwrap();
        assert_eq!(
            server_control.recv().await.unwrap().unwrap(),
            &b"after update"[..]
        );
        server_control.send(b"reply").await.unwrap();
        assert_eq!(control.recv().await.unwrap().unwrap(), &b"reply"[..]);
        second_server.send(b"second reply").await.unwrap();
        assert_eq!(
            second_control.recv().await.unwrap().unwrap(),
            &b"second reply"[..]
        );
        accept_task.abort();
        assert!(accept_task.await.unwrap_err().is_cancelled());
    })
    .await
    .expect("listener update test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn updater_is_closed_after_listener_drop_and_cannot_bind_a_new_address() {
    let identity = LocalIdentity::generate_software(CertKind::Host).unwrap();
    let mut policy = ListenPolicy {
        port: 0,
        ..ListenPolicy::default()
    };
    let listener = HostListener::start(
        policy.clone(),
        vec![nic(1)],
        Arc::new(server_config(&identity).unwrap()),
    )
    .await
    .unwrap();
    let updater = listener.updater();
    // Use the dynamically allocated port so a mistakenly created binding is
    // observable, without reserving a fixed test port.
    policy.port = tcp_addr(&listener, 1).port();
    let new_addr = SocketAddr::new(nic(2).addresses[0], policy.port);
    drop(listener);
    assert!(matches!(
        updater.update(policy.clone(), vec![nic(2)]).await,
        Err(ListenError::Closed)
    ));
    assert!(matches!(
        updater.clone().update(policy, vec![nic(2)]).await,
        Err(ListenError::Closed)
    ));
    assert!(timeout(
        Duration::from_secs(2),
        tokio::net::TcpStream::connect(new_addr)
    )
    .await
    .unwrap()
    .is_err());
    assert!(tokio::net::UdpSocket::bind(new_addr).await.is_ok());
}
