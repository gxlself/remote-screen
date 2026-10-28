use std::{future::Future, io, net::SocketAddr, sync::Arc, sync::Mutex, time::Duration};

use bytes::Bytes;
use rsp_transport::{
    connect_tcp_via,
    dial::{
        race, AttemptResult, Candidate, CandidateSource, DefaultDialer, DialOpts, DialPlan,
        PathKind,
    },
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    net::NetFuture,
    tls::{client_config, server_config, ServerPin},
    CloseCode, Conn, ConnError, ControlStream, DatagramKind, NetDialer, NetStream, QuicListener,
    StreamKind, SystemNetDialer, TcpListener, TransportKind,
};
use tokio_rustls::TlsAcceptor;

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    SharedQuicEndpoint(SocketAddr),
    DialUdp(SocketAddr),
    DialTcp(SocketAddr),
    DialTcpHost(String, u16),
}

#[derive(Debug, Default)]
struct RecordingNet {
    system: SystemNetDialer,
    calls: Mutex<Vec<Call>>,
    per_peer: bool,
    refuse_udp: Option<SocketAddr>,
}

impl RecordingNet {
    fn new(per_peer: bool) -> Self {
        Self {
            per_peer,
            ..Self::default()
        }
    }

    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl NetDialer for RecordingNet {
    fn shared_quic_endpoint(&self, bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        self.record(Call::SharedQuicEndpoint(bind));
        if self.per_peer {
            Ok(None)
        } else {
            self.system.shared_quic_endpoint(bind)
        }
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        self.record(Call::DialUdp(peer));
        if self.refuse_udp == Some(peer) {
            Box::pin(async { Err(io::ErrorKind::ConnectionRefused.into()) })
        } else {
            self.system.dial_udp(peer)
        }
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        self.record(Call::DialTcp(peer));
        self.system.dial_tcp(peer)
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        self.record(Call::DialTcpHost(host.to_owned(), port));
        self.system.dial_tcp_host(host, port)
    }
}

fn local() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(5), future)
        .await
        .expect("local network operation timed out")
}

fn identities() -> (LocalIdentity, LocalIdentity) {
    (
        LocalIdentity::generate_software(CertKind::Host).unwrap(),
        LocalIdentity::generate_software(CertKind::App).unwrap(),
    )
}

async fn quic_pair(dialer: &DefaultDialer, listener: &QuicListener) -> (Conn, Conn, SocketAddr) {
    let addr = listener.local_addr().unwrap();
    let accept = async {
        let incoming = listener.accept().await.unwrap();
        let remote = incoming.remote_addr();
        (incoming.accept().await.unwrap(), remote)
    };
    let (client, (server, remote)) =
        bounded(async { tokio::join!(dialer.dial(addr, TransportKind::Quic), accept) }).await;
    (client.unwrap(), server, remote)
}

async fn controls(client: &Conn, server: &Conn) -> (ControlStream, ControlStream) {
    bounded(async {
        let client_control = client.open_control().await.unwrap();
        client_control.send(b"bootstrap").await.unwrap();
        let server_control = server.accept_control().await.unwrap();
        assert_eq!(
            server_control.recv().await.unwrap().unwrap(),
            b"bootstrap"[..]
        );
        server_control.send(b"ready").await.unwrap();
        assert_eq!(client_control.recv().await.unwrap().unwrap(), b"ready"[..]);
        (client_control, server_control)
    })
    .await
}

async fn tcp_pair(
    dial: impl Future<Output = Result<Conn, ConnError>>,
    listener: &TcpListener,
) -> (Conn, Conn, ControlStream, ControlStream) {
    // The server returns its main connection only after the first record.
    let connect = async {
        let client = dial.await.unwrap();
        let control = client.open_control().await.unwrap();
        control.send(b"bootstrap").await.unwrap();
        (client, control)
    };
    let accept = async {
        listener
            .accept()
            .await
            .unwrap()
            .accept()
            .await
            .unwrap()
            .unwrap()
    };
    bounded(async {
        let ((client, client_control), server) = tokio::join!(connect, accept);
        let server_control = server.accept_control().await.unwrap();
        assert_eq!(
            server_control.recv().await.unwrap().unwrap(),
            b"bootstrap"[..]
        );
        server_control.send(b"ready").await.unwrap();
        assert_eq!(client_control.recv().await.unwrap().unwrap(), b"ready"[..]);
        (client, server, client_control, server_control)
    })
    .await
}

fn close(client: &Conn, server: &Conn) {
    client.close(CloseCode::Normal, b"done");
    server.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn new_keeps_one_shared_endpoint() {
    let (host, app) = identities();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let endpoint = quinn::Endpoint::client(local()).unwrap();
    let port = endpoint.local_addr().unwrap().port();
    let dialer = DefaultDialer::new(endpoint, &app).unwrap();
    let (client_a, server_a, remote_a) = quic_pair(&dialer, &listener).await;
    let (client_b, server_b, remote_b) = quic_pair(&dialer, &listener).await;
    assert_eq!(remote_a.port(), port);
    assert_eq!(remote_b.port(), port);
    close(&client_a, &server_a);
    close(&client_b, &server_b);
}

#[tokio::test]
async fn with_net_shared_records_tcp() {
    let (host, app) = identities();
    let net = Arc::new(RecordingNet::new(false));
    let endpoint = net.shared_quic_endpoint(local()).unwrap();
    assert!(endpoint.is_some());
    let dialer = DefaultDialer::with_net(net.clone(), endpoint, &app).unwrap();
    let quic = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let (client, server, _) = quic_pair(&dialer, &quic).await;
    let _controls = controls(&client, &server).await;
    assert_eq!(net.calls(), vec![Call::SharedQuicEndpoint(local())]);
    close(&client, &server);

    let tcp = TcpListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let addr = tcp.local_addr().unwrap();
    let (client, server, _, _) = tcp_pair(dialer.dial(addr, TransportKind::Tcp), &tcp).await;
    assert_eq!(
        net.calls(),
        vec![Call::SharedQuicEndpoint(local()), Call::DialTcp(addr)]
    );
    close(&client, &server);
}

#[tokio::test]
async fn tcp_input_attach_uses_same_net() {
    // Existing session runners share a reference to their input sender across
    // awaits, even though an injected NetStream need only implement Send.
    fn assert_send_sync<T: Send + Sync>() {}
    assert_send_sync::<rsp_transport::UniSend>();

    let (host, app) = identities();
    let net = Arc::new(RecordingNet::new(false));
    let listener = TcpListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let cfg = Arc::new(client_config(&app, ServerPin::Any).unwrap());
    let (client, server, client_control, server_control) =
        tcp_pair(connect_tcp_via(net.clone(), addr, cfg), &listener).await;
    let token = vec![0x71; 16];
    client.set_session_token(token.clone()).unwrap();
    listener.register_token(token.clone(), &server).unwrap();
    let attach = async { listener.accept().await.unwrap().accept().await.unwrap() };
    let (accepted, tx) =
        bounded(async { tokio::join!(attach, client.open_uni(StreamKind::Input)) }).await;
    assert!(accepted.is_none());
    let mut tx = tx.unwrap();
    let (kind, mut rx) = bounded(server.accept_uni()).await.unwrap();
    assert_eq!(kind, StreamKind::Input);
    bounded(async {
        tx.send_record(b"input").await.unwrap();
        tx.finish().await.unwrap();
        assert_eq!(rx.recv_record().await.unwrap().unwrap(), b"input"[..]);
        assert!(rx.recv_record().await.unwrap().is_none());
        client_control.send(b"main still works").await.unwrap();
        assert_eq!(
            server_control.recv().await.unwrap().unwrap(),
            b"main still works"[..]
        );
    })
    .await;
    assert_eq!(net.calls(), vec![Call::DialTcp(addr), Call::DialTcp(addr)]);
    listener.unregister_token(&token);
    close(&client, &server);
}

#[tokio::test]
async fn per_peer_endpoint_via_dial_udp() {
    let (host, app) = identities();
    let net = Arc::new(RecordingNet::new(true));
    let shared = net.shared_quic_endpoint(local()).unwrap();
    assert!(shared.is_none());
    let dialer = DefaultDialer::with_net(net.clone(), shared, &app).unwrap();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let addr = listener.local_addr().unwrap();
    let (client_a, server_a, remote_a) = quic_pair(&dialer, &listener).await;
    let (client_b, server_b, remote_b) = quic_pair(&dialer, &listener).await;
    assert_ne!(remote_a.port(), remote_b.port());
    assert_eq!(
        net.calls(),
        vec![
            Call::SharedQuicEndpoint(local()),
            Call::DialUdp(addr),
            Call::DialUdp(addr)
        ]
    );
    for (client, server, payload) in [
        (&client_a, &server_a, Bytes::from_static(b"first")),
        (&client_b, &server_b, Bytes::from_static(b"second")),
    ] {
        client
            .send_datagram(DatagramKind::Probe, payload.clone())
            .unwrap();
        assert_eq!(
            bounded(server.recv_datagram()).await.unwrap(),
            (DatagramKind::Probe, payload.clone())
        );
        server
            .send_datagram(DatagramKind::Probe, payload.clone())
            .unwrap();
        assert_eq!(
            bounded(client.recv_datagram()).await.unwrap(),
            (DatagramKind::Probe, payload)
        );
        close(client, server);
    }
}

#[tokio::test]
async fn per_peer_connection_outlives_dialer() {
    let (host, app) = identities();
    let net = Arc::new(RecordingNet::new(true));
    let weak_net = Arc::downgrade(&net);
    let dialer = DefaultDialer::with_net(net.clone(), None, &app).unwrap();
    let listener = QuicListener::bind(local(), server_config(&host).unwrap())
        .await
        .unwrap();
    let (client, server, _) = quic_pair(&dialer, &listener).await;
    let (client_control, server_control) = controls(&client, &server).await;
    drop(dialer);
    drop(net);
    assert!(weak_net.upgrade().is_none());
    tokio::time::timeout(Duration::from_secs(1), async {
        for index in 0..3 {
            let payload = [index];
            client_control.send(&payload).await.unwrap();
            assert_eq!(server_control.recv().await.unwrap().unwrap(), payload[..]);
            server_control.send(&payload).await.unwrap();
            assert_eq!(client_control.recv().await.unwrap().unwrap(), payload[..]);
        }
    })
    .await
    .expect("connection must survive dropping its dialer and endpoint");
    close(&client, &server);
}

#[tokio::test]
async fn per_peer_race_falls_back_to_tcp() {
    let (host, app) = identities();
    let socket = tokio::net::TcpListener::bind(local()).await.unwrap();
    let addr = socket.local_addr().unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_config(&host).unwrap()));
    let net = Arc::new(RecordingNet {
        refuse_udp: Some(addr),
        ..RecordingNet::new(true)
    });
    let dialer = DefaultDialer::with_net(net.clone(), None, &app).unwrap();
    let plan = DialPlan {
        candidates: vec![Candidate {
            addr,
            kind: PathKind::Manual,
            source: CandidateSource::Manual,
        }],
        ..DialPlan::default()
    };
    let opts = DialOpts::default();
    let accept = async {
        acceptor
            .accept(socket.accept().await.unwrap().0)
            .await
            .unwrap()
    };
    let (outcome, _peer) =
        bounded(async { tokio::join!(race(&plan, &dialer, host.fingerprint(), &opts), accept) })
            .await;
    let outcome = outcome.unwrap();
    assert_eq!(outcome.transport, TransportKind::Tcp);
    let quic_attempt = outcome
        .attempts
        .iter()
        .find(|attempt| {
            attempt.candidate.addr == addr && attempt.transport == Some(TransportKind::Quic)
        })
        .unwrap();
    assert_eq!(quic_attempt.result, AttemptResult::Refused);
    assert_eq!(net.calls(), vec![Call::DialUdp(addr), Call::DialTcp(addr)]);
    assert!(outcome.backup.is_none());
    outcome.primary.close(CloseCode::Normal, b"done");
}
