use std::future::Future;
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicUsize, Ordering},
    Arc,
};
use std::time::Duration;

use bytes::Bytes;
use prost::Message;
use rsp_transport::{
    connect_quic, connect_tcp, CloseCode, Conn, ConnError, ControlStream, DatagramKind,
    QuicListener, StreamKind, TcpListener, TransportKind, MAX_CONTROL_FRAME,
};
use rustls::{
    client::danger::{HandshakeSignatureValid, ServerCertVerified, ServerCertVerifier},
    pki_types::{CertificateDer, PrivatePkcs8KeyDer, ServerName, UnixTime},
    server::danger::{ClientCertVerified, ClientCertVerifier},
    ClientConfig, DigitallySignedStruct, DistinguishedName, ServerConfig, SignatureScheme,
};
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::net::{TcpListener as SocketListener, TcpStream};
use tokio_rustls::{TlsAcceptor, TlsConnector};

fn local() -> SocketAddr {
    "127.0.0.1:0".parse().unwrap()
}
async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("test timed out")
}

#[derive(Debug)]
struct AcceptAnyCertificate;
impl AcceptAnyCertificate {
    fn signature(
        message: &[u8],
        cert: &CertificateDer<'_>,
        dss: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        rustls::crypto::verify_tls13_signature(
            message,
            cert,
            dss,
            &rustls::crypto::ring::default_provider().signature_verification_algorithms,
        )
    }
    fn schemes() -> Vec<SignatureScheme> {
        rustls::crypto::ring::default_provider()
            .signature_verification_algorithms
            .supported_schemes()
    }
}
impl ServerCertVerifier for AcceptAnyCertificate {
    fn verify_server_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: &ServerName<'_>,
        _: &[u8],
        _: UnixTime,
    ) -> Result<ServerCertVerified, rustls::Error> {
        Ok(ServerCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::signature(m, c, d)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::signature(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        Self::schemes()
    }
}
impl ClientCertVerifier for AcceptAnyCertificate {
    fn root_hint_subjects(&self) -> &[DistinguishedName] {
        &[]
    }
    fn verify_client_cert(
        &self,
        _: &CertificateDer<'_>,
        _: &[CertificateDer<'_>],
        _: UnixTime,
    ) -> Result<ClientCertVerified, rustls::Error> {
        Ok(ClientCertVerified::assertion())
    }
    fn verify_tls12_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::signature(m, c, d)
    }
    fn verify_tls13_signature(
        &self,
        m: &[u8],
        c: &CertificateDer<'_>,
        d: &DigitallySignedStruct,
    ) -> Result<HandshakeSignatureValid, rustls::Error> {
        Self::signature(m, c, d)
    }
    fn supported_verify_schemes(&self) -> Vec<SignatureScheme> {
        Self::schemes()
    }
}

fn configs() -> (ServerConfig, Arc<ClientConfig>) {
    let provider = Arc::new(rustls::crypto::ring::default_provider());
    let server = rcgen::generate_simple_self_signed(vec!["rs-host".into()]).unwrap();
    let client = rcgen::generate_simple_self_signed(vec!["rs-client".into()]).unwrap();
    let mut server_cfg = ServerConfig::builder_with_provider(provider.clone())
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .with_client_cert_verifier(Arc::new(AcceptAnyCertificate))
        .with_single_cert(
            vec![server.cert.der().clone()],
            PrivatePkcs8KeyDer::from(server.signing_key.serialize_der()).into(),
        )
        .unwrap();
    let mut client_cfg = ClientConfig::builder_with_provider(provider)
        .with_protocol_versions(&[&rustls::version::TLS13])
        .unwrap()
        .dangerous()
        .with_custom_certificate_verifier(Arc::new(AcceptAnyCertificate))
        .with_client_auth_cert(
            vec![client.cert.der().clone()],
            PrivatePkcs8KeyDer::from(client.signing_key.serialize_der()).into(),
        )
        .unwrap();
    server_cfg.alpn_protocols = vec![b"rsp/1".to_vec()];
    client_cfg.alpn_protocols = vec![b"rsp/1".to_vec()];
    (server_cfg, Arc::new(client_cfg))
}

struct Pair {
    client: Conn,
    server: Conn,
    client_control: ControlStream,
    server_control: ControlStream,
    tcp_listener: Option<Arc<TcpListener>>,
    _quic_listener: Option<Arc<QuicListener>>,
    _endpoint: Option<quinn::Endpoint>,
}
async fn pair(kind: TransportKind) -> Pair {
    let (server_cfg, client_cfg) = configs();
    let (client, accept, tcp_listener, quic_listener, endpoint) = match kind {
        TransportKind::Quic => {
            let listener = Arc::new(QuicListener::bind(local(), server_cfg).await.unwrap());
            let endpoint = quinn::Endpoint::client(local()).unwrap();
            let host = listener.clone();
            let accept =
                tokio::spawn(async move { host.accept().await.unwrap().accept().await.unwrap() });
            let client = connect_quic(&endpoint, listener.local_addr().unwrap(), client_cfg, false)
                .await
                .unwrap();
            (client, accept, None, Some(listener), Some(endpoint))
        }
        TransportKind::Tcp => {
            let listener = Arc::new(TcpListener::bind(local(), server_cfg).await.unwrap());
            let host = listener.clone();
            let accept = tokio::spawn(async move {
                host.accept()
                    .await
                    .unwrap()
                    .accept()
                    .await
                    .unwrap()
                    .unwrap()
            });
            let client = connect_tcp(listener.local_addr().unwrap(), client_cfg)
                .await
                .unwrap();
            (client, accept, Some(listener), None, None)
        }
    };
    let client_control = client.open_control().await.unwrap();
    client_control.send(b"bootstrap").await.unwrap();
    let server = bounded(accept).await.unwrap();
    let server_control = bounded(server.accept_control()).await.unwrap();
    assert_eq!(
        bounded(server_control.recv()).await.unwrap().unwrap(),
        b"bootstrap"[..]
    );
    Pair {
        client,
        server,
        client_control,
        server_control,
        tcp_listener,
        _quic_listener: quic_listener,
        _endpoint: endpoint,
    }
}

#[tokio::test]
async fn controls_round_trip_and_boundaries_on_both_transports() {
    for kind in [TransportKind::Quic, TransportKind::Tcp] {
        let p = bounded(pair(kind)).await;
        assert_eq!(p.client.kind(), kind);
        assert_eq!(p.server.kind(), kind);
        assert_eq!(p.client.peer_certificates().len(), 1);
        assert_eq!(p.server.peer_certificates().len(), 1);
        assert!(p.server.remote_addr().ip().is_loopback());
        let boundary = vec![0x55; MAX_CONTROL_FRAME];
        let (sent, received) = bounded(async {
            tokio::join!(p.client_control.send(&boundary), p.server_control.recv())
        })
        .await;
        sent.unwrap();
        assert_eq!(received.unwrap().unwrap(), boundary);
        p.server_control.send(b"reply").await.unwrap();
        assert_eq!(
            bounded(p.client_control.recv()).await.unwrap().unwrap(),
            b"reply"[..]
        );
        assert!(matches!(
            p.client_control.send(&vec![0; MAX_CONTROL_FRAME + 1]).await,
            Err(ConnError::Protocol(_))
        ));
        p.client_control.send(b"").await.unwrap();
        assert_eq!(
            bounded(p.server_control.recv()).await.unwrap().unwrap(),
            Bytes::new()
        );
        assert!(p.client.open_control().await.is_err());
        assert!(p.server.accept_control().await.is_err());
        p.client.close(CloseCode::Normal, b"done");
    }
}

#[tokio::test]
async fn clipboard_streams_preserve_records_raw_bytes_and_fin() {
    for kind in [TransportKind::Quic, TransportKind::Tcp] {
        let p = pair(kind).await;
        let mut a = p.client.open_uni(StreamKind::Clipboard).await.unwrap();
        let mut b = p.client.open_uni(StreamKind::Clipboard).await.unwrap();
        a.send_record(b"header-a").await.unwrap();
        b.send_record(b"header-b").await.unwrap();
        a.write_raw(b"\xffRSP-FIN").await.unwrap();
        b.write_raw(&vec![0x22; 150_000]).await.unwrap();
        a.finish().await.unwrap();
        b.finish().await.unwrap();
        for (header, raw) in [
            (b"header-a".as_slice(), b"\xffRSP-FIN".to_vec()),
            (b"header-b".as_slice(), vec![0x22; 150_000]),
        ] {
            let (stream_kind, mut rx) = bounded(p.server.accept_uni()).await.unwrap();
            assert_eq!(stream_kind, StreamKind::Clipboard);
            assert_eq!(bounded(rx.recv_record()).await.unwrap().unwrap(), header);
            let mut received = Vec::new();
            let mut buf = [0; 1024];
            while let Some(n) = bounded(rx.read_raw(&mut buf)).await.unwrap() {
                received.extend_from_slice(&buf[..n]);
            }
            assert_eq!(received, raw);
        }
        let mut reply = p.server.open_uni(StreamKind::Clipboard).await.unwrap();
        reply.send_record(b"back").await.unwrap();
        reply.finish().await.unwrap();
        let (_, mut rx) = bounded(p.client.accept_uni()).await.unwrap();
        assert_eq!(
            bounded(rx.recv_record()).await.unwrap().unwrap(),
            b"back"[..]
        );
        assert!(rx.recv_record().await.unwrap().is_none());
        p.client.close(CloseCode::Normal, b"done");
    }
}

#[tokio::test]
async fn quic_all_uni_kinds_finish() {
    let p = pair(TransportKind::Quic).await;
    for kind in [
        StreamKind::Input,
        StreamKind::Cursor,
        StreamKind::Clipboard,
        StreamKind::File,
    ] {
        let mut tx = p.client.open_uni(kind).await.unwrap();
        tx.send_record(b"data").await.unwrap();
        tx.send_record(b"").await.unwrap();
        tx.finish().await.unwrap();
        let (got, mut rx) = bounded(p.server.accept_uni()).await.unwrap();
        assert_eq!(got, kind);
        assert_eq!(
            bounded(rx.recv_record()).await.unwrap().unwrap(),
            b"data"[..]
        );
        assert_eq!(rx.recv_record().await.unwrap().unwrap(), Bytes::new());
        assert!(rx.recv_record().await.unwrap().is_none());
        assert!(tx.send_record(b"after FIN").await.is_err());
    }
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn cursor_tcp_is_persistent_until_connection_close() {
    let p = pair(TransportKind::Tcp).await;
    let mut tx = p.server.open_uni(StreamKind::Cursor).await.unwrap();
    tx.send_record(b"shape").await.unwrap();
    let (kind, mut rx) = bounded(p.client.accept_uni()).await.unwrap();
    assert_eq!(kind, StreamKind::Cursor);
    assert_eq!(
        bounded(rx.recv_record()).await.unwrap().unwrap(),
        b"shape"[..]
    );
    assert!(matches!(tx.finish().await, Err(ConnError::Unsupported(_))));
    let mut file = p.client.open_uni(StreamKind::File).await.unwrap();
    file.write_raw(b"file bytes").await.unwrap();
    file.finish().await.unwrap();
    let (kind, mut file_rx) = bounded(p.server.accept_uni()).await.unwrap();
    assert_eq!(kind, StreamKind::File);
    let mut bytes = [0; 10];
    assert_eq!(
        bounded(file_rx.read_raw(&mut bytes)).await.unwrap(),
        Some(10)
    );
    assert_eq!(&bytes, b"file bytes");
    assert_eq!(bounded(file_rx.read_raw(&mut bytes)).await.unwrap(), None);
    tx.send_record(b"shape after file FIN").await.unwrap();
    assert_eq!(
        bounded(rx.recv_record()).await.unwrap().unwrap(),
        b"shape after file FIN"[..]
    );
    p.server.close(CloseCode::Normal, b"done");
    assert!(bounded(rx.recv_record()).await.unwrap().is_none());
}

#[tokio::test]
async fn datagrams_round_trip_on_both_transports() {
    for transport in [TransportKind::Quic, TransportKind::Tcp] {
        let p = pair(transport).await;
        for kind in [
            DatagramKind::Video,
            DatagramKind::Audio,
            DatagramKind::Pointer,
            DatagramKind::Probe,
        ] {
            p.server
                .send_datagram(kind, Bytes::from_static(b"media"))
                .unwrap();
            assert_eq!(
                bounded(p.client.recv_datagram()).await.unwrap(),
                (kind, Bytes::from_static(b"media"))
            );
            p.client.send_datagram(kind, Bytes::new()).unwrap();
            assert_eq!(
                bounded(p.server.recv_datagram()).await.unwrap(),
                (kind, Bytes::new())
            );
        }
        assert_eq!(
            p.client.max_datagram_size().is_none(),
            transport == TransportKind::Tcp
        );
        if let Some(max) = p.client.max_datagram_size() {
            assert!(matches!(
                p.client
                    .send_datagram(DatagramKind::Probe, Bytes::from(vec![0; 65_535])),
                Err(ConnError::SendDatagram(quinn::SendDatagramError::TooLarge))
            ));
            p.client
                .send_datagram(DatagramKind::Probe, Bytes::from(vec![0; max]))
                .unwrap();
            assert_eq!(
                bounded(p.server.recv_datagram()).await.unwrap().1.len(),
                max
            );
        }
        p.client.close(CloseCode::Normal, b"done");
    }
}

#[tokio::test]
async fn quic_close_codes_and_mutable_keepalive() {
    for code in [
        CloseCode::Normal,
        CloseCode::Protocol,
        CloseCode::Auth,
        CloseCode::Replaced,
        CloseCode::HostShutdown,
        CloseCode::Idle,
        CloseCode::Revoked,
    ] {
        let p = pair(TransportKind::Quic).await;
        p.client.set_keepalive(Duration::from_millis(20)).unwrap();
        p.client.set_keepalive(Duration::from_secs(15)).unwrap();
        assert!(p.client.set_keepalive(Duration::ZERO).is_err());
        p.server.close(code, b"test close");
        let reason = bounded(p.client.closed()).await;
        assert_eq!(reason.code, Some(code as u64));
        assert_eq!(reason.reason, b"test close"[..]);
    }
}

#[tokio::test]
async fn tcp_close_uses_tls_eof_without_inventing_application_codes() {
    let p = pair(TransportKind::Tcp).await;
    p.server.close(CloseCode::Revoked, b"revoked");
    assert_eq!(bounded(p.server.closed()).await.code, Some(6));
    assert_eq!(bounded(p.client.closed()).await.code, None);
    assert!(bounded(p.client_control.recv()).await.unwrap().is_none());
    assert!(p
        .client
        .send_datagram(DatagramKind::Audio, Bytes::new())
        .is_err());
}

#[tokio::test]
async fn tcp_input_uses_second_tls_socket_and_token_registration() {
    let p = pair(TransportKind::Tcp).await;
    let listener = p.tcp_listener.as_ref().unwrap();
    let token = vec![0x71; 16];
    p.client.set_session_token(token.clone()).unwrap();
    listener.register_token(token.clone(), &p.server).unwrap();
    let attach = async { listener.accept().await.unwrap().accept().await.unwrap() };
    let (accepted, tx) =
        bounded(async { tokio::join!(attach, p.client.open_uni(StreamKind::Input)) }).await;
    assert!(accepted.is_none());
    let mut tx = tx.unwrap();
    let (kind, mut rx) = bounded(p.server.accept_uni()).await.unwrap();
    assert_eq!(kind, StreamKind::Input);
    tx.send_record(b"input").await.unwrap();
    tx.finish().await.unwrap();
    assert_eq!(
        bounded(rx.recv_record()).await.unwrap().unwrap(),
        b"input"[..]
    );
    assert!(bounded(rx.recv_record()).await.unwrap().is_none());
    p.client_control.send(b"main still works").await.unwrap();
    assert_eq!(
        bounded(p.server_control.recv()).await.unwrap().unwrap(),
        b"main still works"[..]
    );
    listener.unregister_token(&token);
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn tcp_wrong_input_token_is_rejected() {
    let p = pair(TransportKind::Tcp).await;
    let listener = p.tcp_listener.as_ref().unwrap();
    listener.register_token(vec![1; 16], &p.server).unwrap();
    p.client.set_session_token(vec![2; 16]).unwrap();
    let attach = async { listener.accept().await.unwrap().accept().await };
    let (accepted, tx) =
        bounded(async { tokio::join!(attach, p.client.open_uni(StreamKind::Input)) }).await;
    assert!(matches!(accepted, Err(ConnError::Protocol(_))));
    drop(tx);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), p.server.accept_uni())
            .await
            .is_err()
    );
    p.client_control.send(b"main survives").await.unwrap();
    assert_eq!(
        bounded(p.server_control.recv()).await.unwrap().unwrap(),
        b"main survives"[..]
    );
    p.client.close(CloseCode::Normal, b"done");
}

fn raw_quic_config(cfg: Arc<ClientConfig>) -> quinn::ClientConfig {
    quinn::ClientConfig::new(Arc::new(
        quinn::crypto::rustls::QuicClientConfig::try_from(cfg).unwrap(),
    ))
}
async fn raw_quic() -> (Arc<QuicListener>, quinn::Endpoint, quinn::Connection, Conn) {
    let (server_cfg, client_cfg) = configs();
    let listener = Arc::new(QuicListener::bind(local(), server_cfg).await.unwrap());
    let endpoint = quinn::Endpoint::client(local()).unwrap();
    let host = listener.clone();
    let accept = async move { host.accept().await.unwrap().accept().await.unwrap() };
    let dial = endpoint
        .connect_with(
            raw_quic_config(client_cfg),
            listener.local_addr().unwrap(),
            "rs-host",
        )
        .unwrap();
    let (server, client) = bounded(async { tokio::join!(accept, dial) }).await;
    (listener, endpoint, client.unwrap(), server)
}

#[tokio::test]
async fn quic_unknown_stream_and_datagram_are_ignored() {
    let (_listener, _endpoint, client, server) = raw_quic().await;
    let mut bad = client.open_uni().await.unwrap();
    bad.write_all(&[0x7f, 1, 2]).await.unwrap();
    let mut good = client.open_uni().await.unwrap();
    good.write_all(b"\x02\0\0\0\x02ok").await.unwrap();
    good.finish().unwrap();
    let (kind, mut rx) = bounded(server.accept_uni()).await.unwrap();
    assert_eq!(kind, StreamKind::Cursor);
    assert_eq!(rx.recv_record().await.unwrap().unwrap(), b"ok"[..]);
    assert_eq!(bounded(bad.stopped()).await.unwrap(), Some(1u32.into()));
    client
        .send_datagram(Bytes::from_static(b"\x80ignored"))
        .unwrap();
    client.send_datagram(Bytes::from_static(b"\x12ok")).unwrap();
    assert_eq!(
        bounded(server.recv_datagram()).await.unwrap(),
        (DatagramKind::Pointer, Bytes::from_static(b"ok"))
    );
    assert_eq!(server.unknown_datagram_count(), 1);
    server.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn quic_fragmented_control_and_inbound_oversize_close() {
    let (_listener, _endpoint, client, server) = raw_quic().await;
    let (mut send, _recv) = client.open_bi().await.unwrap();
    send.write_all(&[0, 0]).await.unwrap();
    let control = bounded(server.accept_control()).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(20), control.recv())
            .await
            .is_err()
    );
    send.write_all(b"\0\x03abc").await.unwrap();
    assert_eq!(bounded(control.recv()).await.unwrap().unwrap(), b"abc"[..]);
    send.write_all(&((MAX_CONTROL_FRAME + 1) as u32).to_be_bytes())
        .await
        .unwrap();
    assert!(matches!(
        bounded(control.recv()).await,
        Err(ConnError::Protocol(_))
    ));
    match bounded(client.closed()).await {
        quinn::ConnectionError::ApplicationClosed(e) => assert_eq!(e.error_code.into_inner(), 1),
        other => panic!("unexpected close {other:?}"),
    }
}

#[tokio::test]
async fn quic_cancelled_control_accept_can_be_retried() {
    let (_listener, _endpoint, client, server) = raw_quic().await;
    assert!(
        tokio::time::timeout(Duration::from_millis(20), server.accept_control())
            .await
            .is_err()
    );
    let (mut send, _) = client.open_bi().await.unwrap();
    send.write_all(b"\0\0\0\x02ok").await.unwrap();
    let control = bounded(server.accept_control()).await.unwrap();
    assert_eq!(bounded(control.recv()).await.unwrap().unwrap(), b"ok"[..]);
    server.close(CloseCode::Normal, b"done");
}

async fn raw_tcp_client(
    cfg: Arc<ClientConfig>,
    addr: SocketAddr,
) -> tokio_rustls::client::TlsStream<TcpStream> {
    TlsConnector::from(cfg)
        .connect(
            ServerName::try_from("rs-host").unwrap(),
            TcpStream::connect(addr).await.unwrap(),
        )
        .await
        .unwrap()
}
async fn raw_tcp() -> (
    Arc<TcpListener>,
    tokio_rustls::client::TlsStream<TcpStream>,
    Conn,
) {
    let (server_cfg, client_cfg) = configs();
    let listener = Arc::new(TcpListener::bind(local(), server_cfg).await.unwrap());
    let host = listener.clone();
    let accept = tokio::spawn(async move {
        host.accept()
            .await
            .unwrap()
            .accept()
            .await
            .unwrap()
            .unwrap()
    });
    let mut client = raw_tcp_client(client_cfg, listener.local_addr().unwrap()).await;
    client
        .write_all(&rsp_transport::encode_tcp_record(0, b"ready").unwrap())
        .await
        .unwrap();
    let server = bounded(accept).await.unwrap();
    let control = server.accept_control().await.unwrap();
    assert_eq!(control.recv().await.unwrap().unwrap(), b"ready"[..]);
    (listener, client, server)
}

#[tokio::test]
async fn tcp_unknown_channels_and_oversize_disconnect() {
    let (_listener, mut client, server) = raw_tcp().await;
    for (channel, payload) in [
        (9, b"ignored".as_slice()),
        (0x80, b"unknown".as_slice()),
        (0x12, b"pointer".as_slice()),
    ] {
        client
            .write_all(&rsp_transport::encode_tcp_record(channel, payload).unwrap())
            .await
            .unwrap();
    }
    assert_eq!(
        bounded(server.recv_datagram()).await.unwrap(),
        (DatagramKind::Pointer, Bytes::from_static(b"pointer"))
    );
    assert_eq!(server.unknown_datagram_count(), 1);
    client.write_all(&[0, 0, 0x10, 0, 1]).await.unwrap();
    assert_eq!(bounded(server.closed()).await.code, Some(1));
    let mut byte = [0; 1];
    assert!(matches!(
        bounded(client.read(&mut byte)).await,
        Ok(0) | Err(_)
    ));
}

#[tokio::test]
async fn tcp_latest_video_queue_and_keyframe_throttle_when_peer_does_not_read() {
    let (server_cfg, client_cfg) = configs();
    let socket = SocketListener::bind(local()).await.unwrap();
    let acceptor = TlsAcceptor::from(Arc::new(server_cfg));
    let accept = async {
        acceptor
            .accept(socket.accept().await.unwrap().0)
            .await
            .unwrap()
    };
    let (mut raw_server, client) = bounded(async {
        tokio::join!(
            accept,
            connect_tcp(socket.local_addr().unwrap(), client_cfg)
        )
    })
    .await;
    let client = client.unwrap();
    let calls = Arc::new(AtomicUsize::new(0));
    let seen = calls.clone();
    client
        .set_keyframe_request_callback(Arc::new(move || {
            seen.fetch_add(1, Ordering::Relaxed);
        }))
        .unwrap();
    client
        .send_datagram(DatagramKind::Video, Bytes::from(vec![0xaa; 16 << 20]))
        .unwrap();
    tokio::time::sleep(Duration::from_millis(30)).await;
    for id in 0..20 {
        client
            .send_datagram(DatagramKind::Video, Bytes::from(vec![id; 1024]))
            .unwrap();
    }
    assert_eq!(calls.load(Ordering::Relaxed), 1);
    assert_eq!(client.dropped_video_frames(), 19);
    tokio::time::sleep(Duration::from_millis(520)).await;
    client
        .send_datagram(DatagramKind::Video, Bytes::from_static(b"latest"))
        .unwrap();
    assert_eq!(calls.load(Ordering::Relaxed), 2);
    let first = bounded(read_tcp_record(&mut raw_server)).await;
    assert_eq!(first.0, 0x10);
    assert_eq!(first.1.len(), 16 << 20);
    assert_eq!(
        bounded(read_tcp_record(&mut raw_server)).await,
        (0x10, Bytes::from_static(b"latest"))
    );
    client.close(CloseCode::Normal, b"done");
}

async fn read_tcp_record<R: tokio::io::AsyncRead + Unpin>(read: &mut R) -> (u8, Bytes) {
    let mut header = [0; 5];
    read.read_exact(&mut header).await.unwrap();
    let len = u32::from_be_bytes(header[1..].try_into().unwrap()) as usize;
    let mut data = vec![0; len];
    read.read_exact(&mut data).await.unwrap();
    (header[0], Bytes::from(data))
}

#[tokio::test]
async fn alpn_required_before_any_network_io() {
    let (mut server, client) = configs();
    server.alpn_protocols.clear();
    let mut client = (*client).clone();
    client.alpn_protocols.clear();
    assert!(matches!(
        QuicListener::bind(local(), server.clone()).await,
        Err(ConnError::MissingAlpn)
    ));
    assert!(matches!(
        TcpListener::bind(local(), server).await,
        Err(ConnError::MissingAlpn)
    ));
    assert!(matches!(
        connect_tcp(local(), Arc::new(client.clone())).await,
        Err(ConnError::MissingAlpn)
    ));
    let endpoint = quinn::Endpoint::client(local()).unwrap();
    assert!(matches!(
        connect_quic(&endpoint, local(), Arc::new(client), false).await,
        Err(ConnError::MissingAlpn)
    ));
}

#[tokio::test]
async fn input_attachment_uses_contract_envelope_bytes() {
    let p = pair(TransportKind::Tcp).await;
    let listener = p.tcp_listener.as_ref().unwrap();
    let token = vec![0x37; 16];
    listener.register_token(token.clone(), &p.server).unwrap();
    let (_server_cfg, different_client_cfg) = configs();
    let rejected = async { listener.accept().await.unwrap().accept().await };
    let send = async {
        let mut tls = raw_tcp_client(different_client_cfg, listener.local_addr().unwrap()).await;
        let envelope = rsp_core::pb::Envelope {
            msg: Some(rsp_core::pb::envelope::Msg::AttachInput(
                rsp_core::pb::AttachInput {
                    session_token: token,
                },
            )),
        };
        tls.write_all(&rsp_transport::encode_tcp_record(0, &envelope.encode_to_vec()).unwrap())
            .await
            .unwrap();
        tls
    };
    let (result, _socket) = bounded(async { tokio::join!(rejected, send) }).await;
    assert!(matches!(result, Err(ConnError::Protocol(_))));
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn quic_stop_preserves_the_code_and_other_channels() {
    let p = pair(TransportKind::Quic).await;
    let mut send = p.client.as_quic().unwrap().open_uni().await.unwrap();
    send.write_all(b"\x03\0\0\0\x06queued").await.unwrap();
    let (_, mut recv) = bounded(p.server.accept_uni()).await.unwrap();
    let mut first = [0; 1];
    assert_eq!(recv.read_raw(&mut first).await.unwrap(), Some(1));
    recv.stop(23).unwrap();
    assert_eq!(bounded(send.stopped()).await.unwrap(), Some(23u32.into()));
    assert_eq!(recv.read_raw(&mut first).await.unwrap(), None);
    p.client_control.send(b"still open").await.unwrap();
    assert_eq!(
        p.server_control.recv().await.unwrap().unwrap(),
        b"still open"[..]
    );
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn tcp_stop_discards_the_stream_and_preserves_other_channels() {
    let p = pair(TransportKind::Tcp).await;
    let mut send = p.client.open_uni(StreamKind::Clipboard).await.unwrap();
    send.send_record(b"first").await.unwrap();
    send.send_record(b"queued").await.unwrap();
    let (_, mut recv) = bounded(p.server.accept_uni()).await.unwrap();
    let mut byte = [0; 1];
    assert_eq!(recv.read_raw(&mut byte).await.unwrap(), Some(1));
    recv.stop(23).unwrap();
    assert_eq!(recv.read_raw(&mut byte).await.unwrap(), None);
    assert_eq!(recv.recv_record().await.unwrap(), None);
    send.send_record(b"discarded").await.unwrap();
    send.finish().await.unwrap();
    let mut next = p.client.open_uni(StreamKind::Clipboard).await.unwrap();
    next.send_record(b"other stream").await.unwrap();
    next.finish().await.unwrap();
    let (_, mut other) = bounded(p.server.accept_uni()).await.unwrap();
    assert_eq!(
        bounded(other.recv_record()).await.unwrap().unwrap(),
        b"other stream"[..]
    );
    p.client_control.send(b"still open").await.unwrap();
    assert_eq!(
        p.server_control.recv().await.unwrap().unwrap(),
        b"still open"[..]
    );
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn quic_reset_preserves_the_code_and_other_channels() {
    let p = pair(TransportKind::Quic).await;
    let mut send = p.client.open_uni(StreamKind::Clipboard).await.unwrap();
    let mut recv = bounded(p.server.as_quic().unwrap().accept_uni())
        .await
        .unwrap();
    let mut byte = [0; 1];
    recv.read_exact(&mut byte).await.unwrap();
    assert_eq!(byte, [0x03]);
    send.reset(29).unwrap();
    assert!(
        matches!(bounded(recv.read(&mut byte)).await, Err(quinn::ReadError::Reset(code)) if code == 29u32.into())
    );
    assert!(matches!(
        send.write_raw(b"after reset").await,
        Err(ConnError::Closed)
    ));
    p.client_control.send(b"still open").await.unwrap();
    assert_eq!(
        p.server_control.recv().await.unwrap().unwrap(),
        b"still open"[..]
    );
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn tcp_reset_is_a_noop_and_preserves_other_channels() {
    let p = pair(TransportKind::Tcp).await;
    let mut send = p.client.open_uni(StreamKind::Clipboard).await.unwrap();
    send.reset(29).unwrap();
    send.send_record(b"after reset").await.unwrap();
    send.finish().await.unwrap();
    let (_, mut recv) = bounded(p.server.accept_uni()).await.unwrap();
    assert_eq!(
        bounded(recv.recv_record()).await.unwrap().unwrap(),
        b"after reset"[..]
    );
    assert!(recv.recv_record().await.unwrap().is_none());
    p.client_control.send(b"still open").await.unwrap();
    assert_eq!(
        p.server_control.recv().await.unwrap().unwrap(),
        b"still open"[..]
    );
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn quic_waiting_datagrams_preserve_type_bytes_and_size_errors() {
    let p = pair(TransportKind::Quic).await;
    p.client
        .send_datagram_wait(DatagramKind::Pointer, Bytes::from_static(b"pointer"))
        .await
        .unwrap();
    assert_eq!(
        bounded(p.server.recv_datagram()).await.unwrap(),
        (DatagramKind::Pointer, Bytes::from_static(b"pointer"))
    );
    let max = p.client.max_datagram_size().unwrap();
    assert!(matches!(
        p.client
            .send_datagram_wait(DatagramKind::Video, Bytes::from(vec![0; max + 1]))
            .await,
        Err(ConnError::SendDatagram(quinn::SendDatagramError::TooLarge))
    ));
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn tcp_waiting_datagrams_use_the_existing_record_channels() {
    let p = pair(TransportKind::Tcp).await;
    assert_eq!(p.client.max_datagram_size(), None);
    for kind in [DatagramKind::Pointer, DatagramKind::Video] {
        p.client
            .send_datagram_wait(kind, Bytes::from_static(b"media"))
            .await
            .unwrap();
        assert_eq!(
            bounded(p.server.recv_datagram()).await.unwrap(),
            (kind, Bytes::from_static(b"media"))
        );
    }
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn quic_compatibility_connection_and_recv_adapters_preserve_raw_bytes() {
    let (_listener, _endpoint, client, server) = raw_quic().await;
    let conn = Conn::from(client.clone());
    assert_eq!(conn.kind(), TransportKind::Quic);
    assert_eq!(conn.remote_addr(), client.remote_address());
    assert_eq!(conn.as_quic().unwrap().stable_id(), client.stable_id());
    assert_eq!(conn.peer_certificates().len(), 1);
    let control = conn.open_control().await.unwrap();
    control.send(b"control").await.unwrap();
    let peer_control = bounded(server.accept_control()).await.unwrap();
    assert_eq!(peer_control.recv().await.unwrap().unwrap(), b"control"[..]);
    let mut send = client.open_uni().await.unwrap();
    // M0 input has no type byte; the host consumes the first length byte.
    send.write_all(b"\0\0\0\x03old").await.unwrap();
    send.finish().unwrap();
    let mut raw = bounded(server.as_quic().unwrap().accept_uni())
        .await
        .unwrap();
    let mut first = [0; 1];
    raw.read_exact(&mut first).await.unwrap();
    assert_eq!(first, [0]);
    let mut recv = rsp_transport::UniRecv::from(raw);
    let mut out = Vec::new();
    let mut chunk = [0; 2];
    while let Some(n) = bounded(recv.read_raw(&mut chunk)).await.unwrap() {
        out.extend_from_slice(&chunk[..n]);
    }
    assert_eq!(out, b"\0\0\x03old");
    conn.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn quic_raw_control_reads_leave_protocol_close_to_the_session() {
    let (_listener, _endpoint, client, server) = raw_quic().await;
    let (mut send, _recv) = client.open_bi().await.unwrap();
    let header = (MAX_CONTROL_FRAME as u32 + 1).to_be_bytes();
    send.write_all(&header).await.unwrap();
    let control = bounded(server.accept_control()).await.unwrap();
    let mut got = [0; 4];
    let mut filled = 0;
    while filled < got.len() {
        filled += bounded(control.read_raw(&mut got[filled..]))
            .await
            .unwrap()
            .unwrap();
    }
    assert_eq!(got, header);
    assert!(
        tokio::time::timeout(Duration::from_millis(50), server.closed())
            .await
            .is_err()
    );
    server.close(CloseCode::Protocol, b"protocol error");
    assert!(
        matches!(bounded(client.closed()).await, quinn::ConnectionError::ApplicationClosed(close) if close.error_code == 1u32.into() && close.reason.as_ref() == b"protocol error")
    );
}

#[tokio::test]
async fn tcp_raw_control_reads_are_explicitly_unsupported() {
    let p = pair(TransportKind::Tcp).await;
    assert!(matches!(
        p.server_control.read_raw(&mut [0; 1]).await,
        Err(ConnError::Unsupported(_))
    ));
    p.client_control.send(b"still open").await.unwrap();
    assert_eq!(
        p.server_control.recv().await.unwrap().unwrap(),
        b"still open"[..]
    );
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn quic_control_stopped_waits_for_fin_ack_and_preserves_stop_code() {
    let p = pair(TransportKind::Quic).await;
    p.server_control.send(b"last reply").await.unwrap();
    p.server_control.finish().await.unwrap();
    assert_eq!(
        bounded(p.client_control.recv()).await.unwrap().unwrap(),
        b"last reply"[..]
    );
    assert!(p.client_control.recv().await.unwrap().is_none());
    assert_eq!(bounded(p.server_control.stopped()).await.unwrap(), None);
    p.client.close(CloseCode::Normal, b"done");

    let (_listener, _endpoint, client, server) = raw_quic().await;
    let (mut send, mut recv) = client.open_bi().await.unwrap();
    send.write_all(b"\0\0\0\x01x").await.unwrap();
    let control = bounded(server.accept_control()).await.unwrap();
    assert_eq!(control.recv().await.unwrap().unwrap(), b"x"[..]);
    recv.stop(31u32.into()).unwrap();
    assert_eq!(bounded(control.stopped()).await.unwrap(), Some(31));
    server.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn tcp_control_stopped_is_explicitly_unsupported() {
    let p = pair(TransportKind::Tcp).await;
    assert!(matches!(
        p.server_control.stopped().await,
        Err(ConnError::Unsupported(_))
    ));
    p.client_control.send(b"still open").await.unwrap();
    assert_eq!(
        p.server_control.recv().await.unwrap().unwrap(),
        b"still open"[..]
    );
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn datagram_backlog_is_bounded_and_drains_on_quic() {
    let p = bounded(pair(TransportKind::Quic)).await;
    assert_eq!(p.client.datagram_backlog_bytes(), 0);
    // Conn prepends the one-byte kind, so each queued datagram is exactly 1 KiB.
    let payload = Bytes::from(vec![0; 1023]);
    for _ in 0..200 {
        p.client
            .send_datagram(DatagramKind::Audio, payload.clone())
            .unwrap();
    }
    // Quinn counts each queued Datagram's Bytes metadata as well as its payload.
    let max_backlog = 200 * (1024 + std::mem::size_of::<Bytes>());
    assert!(p.client.datagram_backlog_bytes() <= max_backlog);
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(p.client.datagram_backlog_bytes(), 0);
    p.client.close(CloseCode::Normal, b"done");
}

#[tokio::test]
async fn datagram_backlog_is_zero_on_tcp() {
    let p = bounded(pair(TransportKind::Tcp)).await;
    assert_eq!(p.client.datagram_backlog_bytes(), 0);
    p.client
        .send_datagram(DatagramKind::Audio, Bytes::from_static(&[0, 0]))
        .unwrap();
    assert_eq!(p.client.datagram_backlog_bytes(), 0);
    p.client.close(CloseCode::Normal, b"done");
}
