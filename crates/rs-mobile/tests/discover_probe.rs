mod support;

use std::{
    future::Future,
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use rs_mobile::rscore::{
    api_discover::discover_probe,
    discover::probe::probe,
    dto_core::{DiscoverOutcome, DiscoverProbe},
    error::CoreError,
    runtime::{install_core, take_core, Core},
};
use rsp_transport::{net::NetFuture, NetDialer, NetStream, SystemNetDialer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::TcpListener,
};

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(3), future)
        .await
        .expect("local probe must finish within three seconds")
}

async fn listener() -> (TcpListener, SocketAddr) {
    let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
    let addr = listener.local_addr().unwrap();
    (listener, addr)
}

async fn system_probe(addr: SocketAddr, kind: &str, timeout_ms: u64) -> DiscoverProbe {
    bounded(probe(
        &SystemNetDialer,
        &addr.ip().to_string(),
        addr.port(),
        kind,
        Duration::from_millis(timeout_ms),
    ))
    .await
    .unwrap()
}

#[derive(Debug)]
enum DialMode {
    Pending,
    Error(io::ErrorKind),
    Connect { peer: SocketAddr, delay: Duration },
}

#[derive(Debug)]
struct TestNet {
    mode: DialMode,
    calls: Mutex<Vec<(String, u16)>>,
}

impl TestNet {
    fn new(mode: DialMode) -> Self {
        Self {
            mode,
            calls: Mutex::new(Vec::new()),
        }
    }

    fn calls(&self) -> Vec<(String, u16)> {
        self.calls.lock().unwrap().clone()
    }
}

impl NetDialer for TestNet {
    fn shared_quic_endpoint(&self, _: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        panic!("discovery must not create a QUIC endpoint")
    }

    fn dial_udp(&self, _: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        panic!("discovery must not dial UDP")
    }

    fn dial_tcp(&self, _: SocketAddr) -> NetFuture<'_, NetStream> {
        panic!("discovery must resolve hosts through the selected network")
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        self.calls.lock().unwrap().push((host.to_owned(), port));
        Box::pin(async move {
            match self.mode {
                DialMode::Pending => std::future::pending().await,
                DialMode::Error(kind) => Err(kind.into()),
                DialMode::Connect { peer, delay } => {
                    tokio::time::sleep(delay).await;
                    SystemNetDialer.dial_tcp(peer).await
                }
            }
        })
    }
}

#[tokio::test]
async fn vnc_open() {
    let (listener, addr) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"RFB ").await.unwrap();
        tokio::task::yield_now().await;
        stream.write_all(b"003.889\n").await.unwrap();
        let mut reply = [0; 1];
        assert_eq!(stream.read(&mut reply).await.unwrap(), 0);
    });
    let result = system_probe(addr, "vnc", 1000).await;
    assert_eq!(result.outcome, DiscoverOutcome::Open);
    assert_eq!(result.banner.as_deref(), Some("RFB 003.889"));
    assert_eq!(result.addr, "127.0.0.1");
    assert_eq!(result.port, addr.port());
    bounded(server).await.unwrap();
}

#[tokio::test]
async fn vnc_mismatch() {
    let (listener, addr) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"SSH-2.0-x\r\n").await.unwrap();
    });
    let result = system_probe(addr, "vnc", 500).await;
    assert_eq!(result.outcome, DiscoverOutcome::Mismatch);
    assert_eq!(result.banner, None);
    bounded(server).await.unwrap();
}

#[tokio::test]
async fn vnc_rejects_a_full_wrong_banner_and_a_truncated_version() {
    for banner in [b"SSH-2.0-xx\r\n".as_slice(), b"RFB 003.889"] {
        let (listener, addr) = listener().await;
        let banner = banner.to_vec();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            stream.write_all(&banner).await.unwrap();
        });
        let result = system_probe(addr, "vnc", 500).await;
        assert_eq!(result.outcome, DiscoverOutcome::Mismatch);
        assert_eq!(result.banner, None);
        bounded(server).await.unwrap();
    }
}

#[tokio::test]
async fn rdp_open() {
    let (listener, addr) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 19];
        stream.read_exact(&mut request).await.unwrap();
        assert_eq!(&request[..4], &[0x03, 0x00, 0x00, 0x13]);
        assert_eq!(&request[4..11], &[0x0e, 0xe0, 0, 0, 0, 0, 0]);
        assert_eq!(&request[11..15], &[0x01, 0x00, 0x08, 0x00]);
        assert_eq!(u32::from_le_bytes(request[15..19].try_into().unwrap()), 3);
        let confirm = [
            0x03, 0x00, 0x00, 0x13, 0x0e, 0xd0, 0x00, 0x00, 0x12, 0x34, 0x00, 0x02, 0x00, 0x08,
            0x00, 0x02, 0x00, 0x00, 0x00,
        ];
        stream.write_all(&confirm[..2]).await.unwrap();
        tokio::task::yield_now().await;
        stream.write_all(&confirm[2..]).await.unwrap();
        let mut reply = [0; 1];
        assert_eq!(stream.read(&mut reply).await.unwrap(), 0);
    });
    let result = system_probe(addr, "rdp", 1000).await;
    assert_eq!(result.outcome, DiscoverOutcome::Open);
    assert_eq!(result.banner, None);
    bounded(server).await.unwrap();
}

#[tokio::test]
async fn rdp_mismatch() {
    let (listener, addr) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut request = [0; 19];
        stream.read_exact(&mut request).await.unwrap();
        let mut reply = [0; 1];
        assert_eq!(stream.read(&mut reply).await.unwrap(), 0);
    });
    let started = Instant::now();
    let result = system_probe(addr, "rdp", 200).await;
    assert_eq!(result.outcome, DiscoverOutcome::Mismatch);
    assert!((200..800).contains(&result.ms));
    assert!(started.elapsed() < Duration::from_millis(800));
    bounded(server).await.unwrap();
}

#[tokio::test]
async fn rdp_rejects_wrong_signature_invalid_lengths_and_incomplete_tpkt() {
    for response in [
        b"HTTP".as_slice(),
        &[0x03, 0x00, 0x00, 0x03],
        &[0x03, 0x00, 0xff, 0xff],
        &[0x03, 0x00, 0x00, 0x13],
        &[0x03, 0x00, 0x00, 0x13, 0x0e, 0xd0],
    ] {
        let (listener, addr) = listener().await;
        let response = response.to_vec();
        let server = tokio::spawn(async move {
            let (mut stream, _) = listener.accept().await.unwrap();
            let mut request = [0; 19];
            stream.read_exact(&mut request).await.unwrap();
            stream.write_all(&response).await.unwrap();
        });
        let result = system_probe(addr, "rdp", 500).await;
        assert_eq!(result.outcome, DiscoverOutcome::Mismatch);
        assert_eq!(result.banner, None);
        bounded(server).await.unwrap();
    }
}

#[tokio::test]
async fn refused() {
    let (listener, addr) = listener().await;
    drop(listener);
    let result = system_probe(addr, "vnc", 1000).await;
    assert_eq!(result.outcome, DiscoverOutcome::Refused);
    assert_eq!(result.banner, None);
    assert!(result.ms < 500, "{result:?}");
}

#[tokio::test]
async fn timeout() {
    let net = TestNet::new(DialMode::Pending);
    let started = Instant::now();
    let result = bounded(probe(
        &net,
        "hs.example.com",
        5900,
        "vnc",
        Duration::from_millis(300),
    ))
    .await
    .unwrap();
    assert_eq!(result.outcome, DiscoverOutcome::Timeout);
    assert_eq!(result.banner, None);
    assert!((300..800).contains(&result.ms), "{result:?}");
    assert!((Duration::from_millis(300)..Duration::from_millis(800)).contains(&started.elapsed()));
    assert_eq!(net.calls(), [("hs.example.com".to_owned(), 5900)]);
}

#[tokio::test]
async fn resolution_and_unreachable_errors_are_timeouts() {
    for kind in [
        io::ErrorKind::NotFound,
        io::ErrorKind::InvalidInput,
        io::ErrorKind::NetworkUnreachable,
        io::ErrorKind::HostUnreachable,
        io::ErrorKind::TimedOut,
    ] {
        let net = TestNet::new(DialMode::Error(kind));
        let result = probe(&net, "hs.example.com", 3389, "rdp", Duration::from_secs(1))
            .await
            .unwrap();
        assert_eq!(result.outcome, DiscoverOutcome::Timeout, "{kind:?}");
        assert_eq!(result.banner, None);
        assert_eq!(net.calls().len(), 1);
    }
}

#[tokio::test]
async fn tcp_open() {
    let (listener, addr) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut reply = [0; 1];
        assert_eq!(stream.read(&mut reply).await.unwrap(), 0);
    });
    let result = system_probe(addr, "tcp", 1000).await;
    assert_eq!(result.outcome, DiscoverOutcome::Open);
    assert_eq!(result.banner, None);
    assert!(result.ms < 500, "{result:?}");
    bounded(server).await.unwrap();
}

#[tokio::test]
async fn unknown_kind() {
    let net = TestNet::new(DialMode::Pending);
    for kind in ["ssh", "VNC", ""] {
        assert_eq!(
            bounded(probe(&net, "hs.example.com", 5900, kind, Duration::ZERO)).await,
            Err(CoreError::Internal {
                message: "unknown probe kind".to_owned()
            })
        );
    }
    assert!(net.calls().is_empty());
}

#[tokio::test(start_paused = true)]
async fn timeout_clamped() {
    for (requested_ms, expected_ms) in [(10, 200), (u32::MAX, 5000)] {
        let net = TestNet::new(DialMode::Pending);
        let started = tokio::time::Instant::now();
        let result = probe(
            &net,
            "hs.example.com",
            5900,
            "tcp",
            Duration::from_millis(u64::from(requested_ms)),
        )
        .await
        .unwrap();
        assert_eq!(result.outcome, DiscoverOutcome::Timeout);
        assert_eq!(result.ms, expected_ms);
        assert_eq!(
            started.elapsed(),
            Duration::from_millis(u64::from(expected_ms))
        );
        assert_eq!(net.calls().len(), 1);
    }
}

#[tokio::test]
async fn dialing_and_protocol_checks_share_one_deadline() {
    let (listener, addr) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        let mut reply = [0; 1];
        assert_eq!(stream.read(&mut reply).await.unwrap(), 0);
    });
    let net = TestNet::new(DialMode::Connect {
        peer: addr,
        delay: Duration::from_millis(300),
    });
    let result = bounded(probe(
        &net,
        "hs.example.com",
        5900,
        "vnc",
        Duration::from_millis(500),
    ))
    .await
    .unwrap();
    assert_eq!(result.outcome, DiscoverOutcome::Mismatch);
    assert!((500..700).contains(&result.ms), "{result:?}");
    assert_eq!(net.calls(), [("hs.example.com".to_owned(), 5900)]);
    bounded(server).await.unwrap();
}

struct InstalledCore(Arc<Core>);

impl Drop for InstalledCore {
    fn drop(&mut self) {
        take_core();
        self.0.shutdown();
        std::fs::remove_dir_all(self.0.data_dir()).unwrap();
    }
}

// Only this test mutates the process-wide Core.
#[tokio::test]
async fn exported_api_uses_the_selected_core_network_and_serializes_results() {
    let (listener, addr) = listener().await;
    let server = tokio::spawn(async move {
        let (mut stream, _) = listener.accept().await.unwrap();
        stream.write_all(b"RFB 003.889\n").await.unwrap();
        let mut reply = [0; 1];
        assert_eq!(stream.read(&mut reply).await.unwrap(), 0);
    });
    let net = Arc::new(TestNet::new(DialMode::Connect {
        peer: addr,
        delay: Duration::ZERO,
    }));
    let c = support::test_core_with(|_, opts| opts.net = Some(net.clone()));
    install_core(c.clone());
    let _installed = InstalledCore(c.clone());
    let json = bounded(discover_probe(
        "hs.example.com".to_owned(),
        5900,
        "vnc".to_owned(),
        1000,
    ))
    .await
    .unwrap();
    let result: DiscoverProbe = serde_json::from_str(&json).unwrap();
    assert_eq!(result.outcome, DiscoverOutcome::Open);
    assert_eq!(result.addr, "hs.example.com");
    assert_eq!(result.port, 5900);
    assert_eq!(result.banner.as_deref(), Some("RFB 003.889"));
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["outcome"], "open");
    assert!(value["ms"].is_u64());
    assert_eq!(net.calls(), [("hs.example.com".to_owned(), 5900)]);
    bounded(server).await.unwrap();

    let refused = Arc::new(TestNet::new(DialMode::Error(
        io::ErrorKind::ConnectionRefused,
    )));
    c.set_net(refused.clone());
    let json = bounded(discover_probe(
        "hs.example.com".to_owned(),
        3389,
        "rdp".to_owned(),
        1000,
    ))
    .await
    .unwrap();
    let value: serde_json::Value = serde_json::from_str(&json).unwrap();
    assert_eq!(value["outcome"], "refused");
    assert!(value.get("banner").is_none());
    assert_eq!(refused.calls(), [("hs.example.com".to_owned(), 3389)]);

    assert_eq!(
        discover_probe("hs.example.com".to_owned(), 5900, "ssh".to_owned(), 1000).await,
        Err(CoreError::internal("unknown probe kind"))
    );
    assert_eq!(refused.calls().len(), 1);
}
