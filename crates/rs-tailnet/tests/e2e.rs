#![cfg(all(unix, feature = "testkit"))]

mod support;

use bytes::Bytes;
use rs_tailnet::{
    testkit::{paths_or_skip, TestNetOptions},
    BackendState, Code, TailnetEvent,
};
use rsp_transport::{
    connect_tcp_via,
    dial::DefaultDialer,
    tls::{client_config, server_config, ServerPin},
    CloseCode, Conn, DatagramKind, QuicListener, StreamKind, TcpListener,
};
use std::{
    collections::BTreeSet,
    io::ErrorKind,
    net::SocketAddr,
    sync::Arc,
    time::{Duration, Instant},
};
use support::{bounded, controls, identities, local, quic_pair, self_ip4, tcp_pair, Fixture};
use tokio::io::{AsyncReadExt, AsyncWriteExt};

fn preauth() -> TestNetOptions {
    TestNetOptions {
        auth_key: Some("tskey-rs-test".into()),
        ..Default::default()
    }
}

#[tokio::test]
async fn preauth_reaches_running() {
    let Some(paths) = paths_or_skip("preauth_reaches_running") else {
        return;
    };
    let fixture = Fixture::new(paths, preauth()).await;
    fixture.start().await;
    fixture.running().await;
    let status = fixture.status().await;
    assert_eq!(status.state, BackendState::Running);
    let ip = self_ip4(&status).octets();
    assert!(ip[0] == 100 && (64..128).contains(&ip[1]));
    assert_eq!(status.self_node.unwrap().derp_region, "test");
    assert!(status
        .peers
        .iter()
        .any(|peer| peer.hostname == "rs-peer" && peer.online));
    fixture.stop().await;
    let log = std::fs::read_to_string(fixture.cfg.log_file.as_ref().unwrap()).unwrap();
    assert!(!log.contains("tskey-rs-test"), "tailnet log leaked authKey");
    fixture.finish().await;
}

#[tokio::test]
async fn preauth_restart_without_key_keeps_identity() {
    let Some(paths) = paths_or_skip("preauth_restart_without_key_keeps_identity") else {
        return;
    };
    let mut fixture = Fixture::new(paths, preauth()).await;
    fixture.start().await;
    fixture.running().await;
    let first = fixture.status().await;
    let first_self = first.self_node.clone().unwrap();
    let first_ip = self_ip4(&first);
    let first_hostname = first_self.hostname.clone();
    let first_dns_name = first_self.dns_name.clone();

    fixture.stop().await;
    fixture.events.lock().unwrap().clear();
    fixture.cfg.auth_key = None;
    fixture.start().await;
    fixture.running().await;
    let second = fixture.status().await;
    let second_self = second.self_node.as_ref().unwrap();
    assert_eq!(self_ip4(&second), first_ip);
    assert_eq!(second_self.hostname, first_hostname);
    assert_eq!(second_self.dns_name, first_dns_name);
    assert_eq!(second_self.ips, first_self.ips);
    assert!(!fixture.events.lock().unwrap().iter().any(|event| {
        matches!(
            event,
            TailnetEvent::AuthUrl { .. }
                | TailnetEvent::State {
                    state: BackendState::NeedsLogin,
                    ..
                }
        )
    }));
    fixture.finish().await;
}

#[tokio::test]
async fn preauth_fresh_node_without_key_needs_login() {
    let Some(paths) = paths_or_skip("preauth_fresh_node_without_key_needs_login") else {
        return;
    };
    let mut fixture = Fixture::new(
        paths,
        TestNetOptions {
            auth_key: Some("tskey-rs-test".into()),
            ..Default::default()
        },
    )
    .await;
    fixture.cfg.auth_key = None;
    fixture.start().await;
    let event = fixture
        .event(15, |event| {
            matches!(
                event,
                TailnetEvent::State {
                    state: BackendState::NeedsLogin,
                    ..
                }
            )
        })
        .await;
    assert!(matches!(
        event,
        TailnetEvent::State {
            state: BackendState::NeedsLogin,
            ..
        }
    ));
    assert_eq!(fixture.status().await.state, BackendState::NeedsLogin);
    fixture.finish().await;
}

#[tokio::test]
async fn interactive_login_via_admin() {
    let Some(paths) = paths_or_skip("interactive_login_via_admin") else {
        return;
    };
    let fixture = Fixture::new(
        paths,
        TestNetOptions {
            require_auth: true,
            ..Default::default()
        },
    )
    .await;
    fixture.start().await;
    let TailnetEvent::AuthUrl { url } = fixture
        .event(15, |event| matches!(event, TailnetEvent::AuthUrl { .. }))
        .await
    else {
        unreachable!()
    };
    assert!(url.contains("/auth/"));
    assert_eq!(fixture.status().await.auth_url.as_ref(), Some(&url));
    fixture.complete(url).await;
    fixture.running().await;
    fixture.finish().await;
}

async fn datagrams(sender: &Conn, receiver: &Conn, marker: u8) -> usize {
    let send = async {
        for sequence in 0..100u8 {
            let mut payload = vec![marker; 1000];
            payload[0] = sequence;
            sender
                .send_datagram_wait(DatagramKind::Video, Bytes::from(payload))
                .await
                .unwrap();
            tokio::time::sleep(Duration::from_millis(3)).await;
        }
    };
    let receive = async {
        let mut seen = BTreeSet::new();
        while seen.len() < 100 {
            let Ok(packet) =
                tokio::time::timeout(Duration::from_secs(3), receiver.recv_datagram()).await
            else {
                break;
            };
            let (kind, payload) = packet.unwrap();
            assert_eq!(kind, DatagramKind::Video);
            assert_eq!(payload.len(), 1000);
            assert!(payload[0] < 100);
            assert!(payload[1..].iter().all(|byte| *byte == marker));
            seen.insert(payload[0]);
        }
        seen.len()
    };
    let ((), count) = tokio::join!(send, receive);
    count
}

#[tokio::test]
async fn quic_over_tailnet() {
    let Some(paths) = paths_or_skip("quic_over_tailnet") else {
        return;
    };
    let (host, app) = identities();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let fixture = Fixture::new(
        paths,
        TestNetOptions {
            forward_udp: vec![(7400, listener.local_addr().unwrap())],
            ..preauth()
        },
    )
    .await;
    fixture.start().await;
    fixture.running().await;
    let dialer = DefaultDialer::with_net(fixture.node.net_dialer(), None, &app).unwrap();
    let peer = SocketAddr::new(fixture.testnet.peer_ip4().into(), 7400);
    let began = Instant::now();
    let (client, server) = quic_pair(&dialer, &listener, peer).await;
    let connect = began.elapsed();
    let (tx, rx) = controls(&client, &server).await;
    bounded(async {
        for sequence in 0..3 {
            let message = format!("control-{sequence}");
            tx.send(message.as_bytes()).await.unwrap();
            let received = rx.recv().await.unwrap().unwrap();
            assert_eq!(received.as_ref(), message.as_bytes());
            rx.send(&received).await.unwrap();
            assert_eq!(
                tx.recv().await.unwrap().unwrap().as_ref(),
                message.as_bytes()
            );
        }
    })
    .await;
    let (to_server, to_client) = bounded(async {
        tokio::join!(
            datagrams(&client, &server, 0x5a),
            datagrams(&server, &client, 0xa5)
        )
    })
    .await;
    assert!(
        to_server >= 95,
        "only {to_server}/100 datagrams reached server"
    );
    assert!(
        to_client >= 95,
        "only {to_client}/100 datagrams reached client"
    );
    let payload: Vec<_> = (0..1024 * 1024).map(|index| (index % 251) as u8).collect();
    let began = Instant::now();
    let send = async {
        let mut stream = client.open_uni(StreamKind::File).await.unwrap();
        stream.write_raw(&payload).await.unwrap();
        stream.finish().await.unwrap();
    };
    let receive = async {
        let (kind, mut stream) = server.accept_uni().await.unwrap();
        assert_eq!(kind, StreamKind::File);
        let mut got = Vec::new();
        let mut chunk = vec![0u8; 65536];
        while let Some(length) = stream.read_raw(&mut chunk).await.unwrap() {
            got.extend_from_slice(&chunk[..length]);
            assert!(got.len() <= payload.len());
        }
        got
    };
    let ((), got) = bounded(async { tokio::join!(send, receive) }).await;
    let transfer = began.elapsed();
    assert_eq!(got, payload);
    eprintln!("QUIC_METRICS connect_ms={:.2} mib=1 transfer_ms={:.2} mib_per_s={:.2} datagrams={to_server}/{to_client}", connect.as_secs_f64() * 1000.0, transfer.as_secs_f64() * 1000.0, 1.0 / transfer.as_secs_f64());
    client.close(CloseCode::Normal, b"done");
    server.close(CloseCode::Normal, b"done");
    fixture.finish().await;
}

#[tokio::test]
async fn tcp_fallback_and_input_over_tailnet() {
    let Some(paths) = paths_or_skip("tcp_fallback_and_input_over_tailnet") else {
        return;
    };
    let (host, app) = identities();
    let listener = TcpListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let fixture = Fixture::new(
        paths,
        TestNetOptions {
            forward_tcp: vec![(7400, listener.local_addr().unwrap())],
            ..preauth()
        },
    )
    .await;
    fixture.start().await;
    fixture.running().await;
    let peer = SocketAddr::new(fixture.testnet.peer_ip4().into(), 7400);
    let cfg = Arc::new(client_config(&app, ServerPin::Any).unwrap());
    let (client, server, tx, rx) = tcp_pair(
        connect_tcp_via(fixture.node.net_dialer(), peer, cfg),
        &listener,
    )
    .await;
    let token = vec![0x71; 16];
    client.set_session_token(token.clone()).unwrap();
    listener.register_token(token.clone(), &server).unwrap();
    let attach = async { listener.accept().await.unwrap().accept().await.unwrap() };
    let (accepted, stream) =
        bounded(async { tokio::join!(attach, client.open_uni(StreamKind::Input)) }).await;
    assert!(accepted.is_none());
    let mut stream = stream.unwrap();
    let (kind, mut input) = bounded(server.accept_uni()).await.unwrap();
    assert_eq!(kind, StreamKind::Input);
    bounded(async {
        stream.send_record(b"tailnet-input").await.unwrap();
        stream.finish().await.unwrap();
        assert_eq!(
            input.recv_record().await.unwrap().unwrap(),
            b"tailnet-input"[..]
        );
        assert!(input.recv_record().await.unwrap().is_none());
        tx.send(b"main still works").await.unwrap();
        assert_eq!(rx.recv().await.unwrap().unwrap(), b"main still works"[..]);
    })
    .await;
    listener.unregister_token(&token);
    client.close(CloseCode::Normal, b"done");
    server.close(CloseCode::Normal, b"done");
    fixture.finish().await;
}

#[tokio::test]
async fn dial_tcp_host_magicdns() {
    let Some(paths) = paths_or_skip("dial_tcp_host_magicdns") else {
        return;
    };
    let fixture = Fixture::new(paths, preauth()).await;
    fixture.start().await;
    fixture.running().await;
    let net = fixture.node.net_dialer();
    for hostname in [
        "rs-peer",
        fixture.testnet.peer_dns_name.trim_end_matches('.'),
    ] {
        let mut stream = bounded(net.dial_tcp_host(hostname, fixture.testnet.tcp_echo))
            .await
            .unwrap();
        let payload: Vec<_> = (0..65536).map(|i| (i % 251) as u8).collect();
        let mut received = vec![0u8; payload.len()];
        bounded(async {
            stream.write_all(&payload).await.unwrap();
            stream.read_exact(&mut received).await.unwrap();
        })
        .await;
        assert_eq!(received, payload);
    }
    fixture.finish().await;
}

#[tokio::test]
async fn stop_fails_conn_then_restart_keeps_identity() {
    let Some(paths) = paths_or_skip("stop_fails_conn_then_restart_keeps_identity") else {
        return;
    };
    let (host, app) = identities();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let mut fixture = Fixture::new(
        paths,
        TestNetOptions {
            forward_udp: vec![(7400, listener.local_addr().unwrap())],
            // Interactive auth: the first start must go through AuthUrl, and the
            // restart below must reuse the persisted identity without a new login
            // or any credentials.
            require_auth: true,
            ..Default::default()
        },
    )
    .await;
    fixture.start().await;
    let TailnetEvent::AuthUrl { url } = fixture
        .event(15, |event| matches!(event, TailnetEvent::AuthUrl { .. }))
        .await
    else {
        unreachable!()
    };
    fixture.complete(url).await;
    fixture.running().await;
    let first_ip = self_ip4(&fixture.status().await);
    let dialer = DefaultDialer::with_net(fixture.node.net_dialer(), None, &app).unwrap();
    let (client, server) = quic_pair(
        &dialer,
        &listener,
        SocketAddr::new(fixture.testnet.peer_ip4().into(), 7400),
    )
    .await;
    let _controls = controls(&client, &server).await;
    fixture.stop().await;
    tokio::time::timeout(Duration::from_secs(12), client.closed())
        .await
        .expect("tailnet stop did not close QUIC");
    assert!(fixture
        .events
        .lock()
        .unwrap()
        .contains(&TailnetEvent::Stopped));
    assert_eq!(fixture.status().await.state, BackendState::Stopped);
    fixture.events.lock().unwrap().clear();
    fixture.cfg.auth_key = None;
    fixture.start().await;
    fixture.running().await;
    assert_eq!(self_ip4(&fixture.status().await), first_ip);
    assert!(!fixture
        .events
        .lock()
        .unwrap()
        .iter()
        .any(|event| matches!(event, TailnetEvent::AuthUrl { .. })));
    server.close(CloseCode::Normal, b"done");
    fixture.finish().await;
}

#[tokio::test]
async fn dial_errors_real() {
    let Some(paths) = paths_or_skip("dial_errors_real") else {
        return;
    };
    let fixture = Fixture::new(
        paths,
        TestNetOptions {
            require_auth: true,
            ..Default::default()
        },
    )
    .await;
    let node = fixture.node.clone();
    let peer = SocketAddr::new(fixture.testnet.peer_ip4().into(), 7400);
    let error =
        tokio::task::spawn_blocking(move || node.dial_udp(peer, Duration::from_millis(500)))
            .await
            .unwrap()
            .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::Other);
    fixture.start().await;
    let TailnetEvent::AuthUrl { url } = fixture
        .event(15, |event| matches!(event, TailnetEvent::AuthUrl { .. }))
        .await
    else {
        unreachable!()
    };
    let node = fixture.node.clone();
    let began = Instant::now();
    let error = tokio::task::spawn_blocking(move || {
        node.dial_tcp(&peer.ip().to_string(), 7402, Duration::from_millis(500))
    })
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::NotConnected);
    assert!(began.elapsed() < Duration::from_millis(500));
    fixture.complete(url).await;
    fixture.running().await;
    let node = fixture.node.clone();
    let error = tokio::task::spawn_blocking(move || {
        node.dial_tcp(&peer.ip().to_string(), 7499, Duration::from_secs(5))
    })
    .await
    .unwrap()
    .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::ConnectionRefused);
    fixture.stop().await;
    let node = fixture.node.clone();
    let mut cfg = fixture.cfg.clone();
    cfg.control_url = "ftp://x".into();
    let error = tokio::task::spawn_blocking(move || node.start(&cfg, Arc::new(|_| {})))
        .await
        .unwrap()
        .unwrap_err();
    assert_eq!(error.code, Code::Invalid);
    fixture.finish().await;
}
