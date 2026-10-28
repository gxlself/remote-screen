use std::{
    io,
    net::{IpAddr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

use rs_mobile::rscore::tailnet_split::{
    classify_host, classify_ip, Readiness, SplitNetDialer, Target,
};
use rsp_transport::{net::NetFuture, net::NetStream, NetDialer};
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    time::advance,
};

#[derive(Debug)]
struct FakeNet {
    name: &'static str,
    calls: Mutex<Vec<String>>,
    tcp_to: Option<SocketAddr>,
    err: Option<io::ErrorKind>,
}

impl FakeNet {
    fn new(name: &'static str) -> Arc<Self> {
        Arc::new(Self {
            name,
            calls: Mutex::new(Vec::new()),
            tcp_to: None,
            err: None,
        })
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn record(&self, call: String) {
        self.calls.lock().unwrap().push(call);
    }

    fn error(&self) -> io::Error {
        io::Error::from(self.err.unwrap_or(io::ErrorKind::ConnectionRefused))
    }
}

impl NetDialer for FakeNet {
    fn shared_quic_endpoint(&self, _bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        Ok(None)
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        self.record(format!("{}:udp:{peer}", self.name));
        let error = self.error();
        Box::pin(async move { Err(error) })
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        self.record(format!("{}:tcp:{peer}", self.name));
        let tcp_to = self.tcp_to;
        let error = self.err.map(io::Error::from);
        Box::pin(async move {
            if let Some(error) = error {
                return Err(error);
            }
            if let Some(target) = tcp_to {
                Ok(Box::new(TcpStream::connect(target).await?) as NetStream)
            } else {
                Ok(Box::new(tokio::io::duplex(64).0) as NetStream)
            }
        })
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        self.record(format!("{}:host:{host}:{port}", self.name));
        let tcp_to = self.tcp_to;
        let error = self.err.map(io::Error::from);
        Box::pin(async move {
            if let Some(error) = error {
                return Err(error);
            }
            if let Some(target) = tcp_to {
                Ok(Box::new(TcpStream::connect(target).await?) as NetStream)
            } else {
                Ok(Box::new(tokio::io::duplex(64).0) as NetStream)
            }
        })
    }
}

fn ready(embedded: Arc<FakeNet>, suffix: &str, peer_names: &[&str]) -> Readiness {
    Readiness::Ready {
        dialer: embedded,
        magic_dns_suffix: suffix.to_owned(),
        peer_names: peer_names.iter().map(|name| (*name).to_owned()).collect(),
    }
}

fn ip(value: &str) -> IpAddr {
    value.parse().unwrap()
}

#[tokio::test]
async fn classify_ip_table() {
    for value in ["100.64.0.12", "100.127.255.1", "fd7a:115c:a1e0::5"] {
        assert_eq!(classify_ip(ip(value)), Target::Embedded, "{value}");
    }
    for value in [
        "100.63.0.1",
        "100.128.0.1",
        "10.30.12.48",
        "192.168.31.20",
        "::1",
    ] {
        assert_eq!(classify_ip(ip(value)), Target::System, "{value}");
    }
}

#[tokio::test]
async fn classify_host_table() {
    let peers = ["studio-mac".to_owned(), "office-win".to_owned()];
    for (host, target) in [
        ("100.64.0.7", Target::Embedded),
        ("office-win.ts.example.com.", Target::Embedded),
        ("OFFICE-WIN", Target::Embedded),
        ("studio-mac", Target::Embedded),
        ("build-srv", Target::System),
        ("build-srv.corp.example.com", Target::System),
        ("10.30.0.5", Target::System),
    ] {
        assert_eq!(
            classify_host(host, "ts.example.com", &peers),
            target,
            "{host}"
        );
    }
    assert_eq!(
        classify_host("office-win.ts.example.com", "", &peers),
        Target::System
    );
}

#[tokio::test]
async fn shared_endpoint_is_none() {
    let system = FakeNet::new("system");
    let split = SplitNetDialer::new(system, Duration::from_secs(1));
    for readiness in [Readiness::Down, Readiness::Idle, Readiness::Starting] {
        split.set_readiness(readiness);
        assert!(split
            .shared_quic_endpoint("0.0.0.0:0".parse().unwrap())
            .unwrap()
            .is_none());
    }
    let embedded = FakeNet::new("emb");
    split.set_readiness(ready(embedded, "ts.example.com", &["studio-mac"]));
    assert!(split
        .shared_quic_endpoint("0.0.0.0:0".parse().unwrap())
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn routes_by_target() {
    let system = FakeNet::new("system");
    let embedded = FakeNet::new("emb");
    let split = SplitNetDialer::new(system.clone(), Duration::from_secs(1));
    split.set_readiness(ready(embedded.clone(), "ts.example.com", &["studio-mac"]));

    let _ = split.dial_tcp("100.64.0.12:7400".parse().unwrap()).await;
    let _ = split.dial_tcp("10.30.12.48:7400".parse().unwrap()).await;
    let _ = split.dial_udp("100.64.0.12:7400".parse().unwrap()).await;
    let _ = split.dial_udp("192.168.31.20:7400".parse().unwrap()).await;
    let _ = split.dial_tcp_host("studio-mac", 7300).await;
    let _ = split.dial_tcp_host("build-srv", 22).await;

    assert_eq!(
        embedded.calls(),
        vec![
            "emb:tcp:100.64.0.12:7400",
            "emb:udp:100.64.0.12:7400",
            "emb:host:studio-mac:7300",
        ]
    );
    assert_eq!(
        system.calls(),
        vec![
            "system:tcp:10.30.12.48:7400",
            "system:udp:192.168.31.20:7400",
            "system:host:build-srv:22",
        ]
    );
}

#[tokio::test]
async fn ready_dial_connects() {
    let listener = TcpListener::bind("127.0.0.1:0").await.unwrap();
    let embedded = Arc::new(FakeNet {
        name: "emb",
        calls: Mutex::new(Vec::new()),
        tcp_to: Some(listener.local_addr().unwrap()),
        err: None,
    });
    let split = SplitNetDialer::new(FakeNet::new("system"), Duration::from_secs(1));
    split.set_readiness(ready(embedded, "ts.example.com", &[]));

    let mut stream = split
        .dial_tcp("100.64.0.12:7400".parse().unwrap())
        .await
        .unwrap();
    stream.write_all(b"hello").await.unwrap();
    let (mut accepted, _) = listener.accept().await.unwrap();
    let mut received = [0; 5];
    accepted.read_exact(&mut received).await.unwrap();
    assert_eq!(&received, b"hello");
}

#[tokio::test]
async fn errors_keep_kind() {
    for kind in [io::ErrorKind::ConnectionRefused, io::ErrorKind::TimedOut] {
        let embedded = Arc::new(FakeNet {
            name: "emb",
            calls: Mutex::new(Vec::new()),
            tcp_to: None,
            err: Some(kind),
        });
        let split = SplitNetDialer::new(FakeNet::new("system"), Duration::from_secs(1));
        split.set_readiness(ready(embedded, "", &[]));
        let error = split
            .dial_tcp("100.64.0.12:7400".parse().unwrap())
            .await
            .err()
            .expect("fake dial should fail");
        assert_eq!(error.kind(), kind);
    }
}

#[tokio::test(start_paused = true)]
async fn down_fails_fast() {
    let system = FakeNet::new("system");
    let split = SplitNetDialer::new(system.clone(), Duration::from_secs(8));
    let error = split
        .dial_tcp("100.64.0.12:7400".parse().unwrap())
        .await
        .err()
        .expect("down embedded dial should fail");
    assert_eq!(error.kind(), io::ErrorKind::NotConnected);
    let _ = split.dial_tcp("10.30.12.48:7400".parse().unwrap()).await;
    assert_eq!(system.calls(), vec!["system:tcp:10.30.12.48:7400"]);
}

#[tokio::test(start_paused = true)]
async fn starting_waits_then_succeeds() {
    let embedded = FakeNet::new("emb");
    let split = SplitNetDialer::new(FakeNet::new("system"), Duration::from_secs(8));
    split.set_readiness(Readiness::Starting);
    let pending = {
        let split = split.clone();
        tokio::spawn(async move { split.dial_tcp("100.64.0.12:7400".parse().unwrap()).await })
    };
    tokio::task::yield_now().await;
    advance(Duration::from_secs(3)).await;
    split.set_readiness(ready(embedded.clone(), "", &[]));
    pending.await.unwrap().unwrap();
    assert_eq!(embedded.calls(), vec!["emb:tcp:100.64.0.12:7400"]);
}

#[tokio::test(start_paused = true)]
async fn starting_times_out() {
    let split = SplitNetDialer::new(FakeNet::new("system"), Duration::from_secs(8));
    split.set_readiness(Readiness::Starting);
    let pending = {
        let split = split.clone();
        tokio::spawn(async move { split.dial_tcp("100.64.0.12:7400".parse().unwrap()).await })
    };
    tokio::task::yield_now().await;
    advance(Duration::from_secs(8) + Duration::from_millis(1)).await;
    let error = pending
        .await
        .unwrap()
        .err()
        .expect("timed out dial should fail");
    assert_eq!(error.kind(), io::ErrorKind::TimedOut);
}

#[tokio::test(start_paused = true)]
async fn idle_calls_wake_once() {
    let split = SplitNetDialer::new(FakeNet::new("system"), Duration::from_secs(8));
    let wake_count = Arc::new(std::sync::atomic::AtomicUsize::new(0));
    let count = wake_count.clone();
    split.set_wake(Arc::new(move || {
        count.fetch_add(1, std::sync::atomic::Ordering::SeqCst);
    }));
    split.set_readiness(Readiness::Idle);

    let first = {
        let split = split.clone();
        tokio::spawn(async move { split.dial_tcp("100.64.0.12:7400".parse().unwrap()).await })
    };
    let second = {
        let split = split.clone();
        tokio::spawn(async move { split.dial_tcp("100.64.0.13:7400".parse().unwrap()).await })
    };
    tokio::task::yield_now().await;
    assert_eq!(wake_count.load(std::sync::atomic::Ordering::SeqCst), 2);
    advance(Duration::from_secs(8) + Duration::from_millis(1)).await;
    assert_eq!(
        first
            .await
            .unwrap()
            .err()
            .expect("first dial should time out")
            .kind(),
        io::ErrorKind::TimedOut
    );
    assert_eq!(
        second
            .await
            .unwrap()
            .err()
            .expect("second dial should time out")
            .kind(),
        io::ErrorKind::TimedOut
    );
}

#[tokio::test(start_paused = true)]
async fn idle_wake_can_start_node() {
    let embedded = FakeNet::new("emb");
    let split = SplitNetDialer::new(FakeNet::new("system"), Duration::from_secs(8));
    let wake_split = split.clone();
    split.set_wake(Arc::new(move || {
        wake_split.set_readiness(Readiness::Starting)
    }));
    split.set_readiness(Readiness::Idle);
    let pending = {
        let split = split.clone();
        tokio::spawn(async move { split.dial_tcp("100.64.0.12:7400".parse().unwrap()).await })
    };
    tokio::task::yield_now().await;
    advance(Duration::from_secs(1)).await;
    split.set_readiness(ready(embedded.clone(), "", &[]));
    pending.await.unwrap().unwrap();
    assert_eq!(embedded.calls(), vec!["emb:tcp:100.64.0.12:7400"]);
}

#[tokio::test(start_paused = true)]
async fn names_survive_restart() {
    let system = FakeNet::new("system");
    let embedded = FakeNet::new("emb");
    let split = SplitNetDialer::new(system.clone(), Duration::from_secs(8));
    split.set_readiness(ready(embedded.clone(), "ts.example.com", &["studio-mac"]));
    split.set_readiness(Readiness::Starting);
    let pending = {
        let split = split.clone();
        tokio::spawn(async move { split.dial_tcp_host("studio-mac", 7300).await })
    };
    tokio::task::yield_now().await;
    assert!(system.calls().is_empty());
    split.set_readiness(ready(embedded.clone(), "ts.example.com", &["studio-mac"]));
    pending.await.unwrap().unwrap();
    assert_eq!(embedded.calls(), vec!["emb:host:studio-mac:7300"]);
}

#[tokio::test(start_paused = true)]
async fn drop_cancels_wait() {
    let split = SplitNetDialer::new(FakeNet::new("system"), Duration::from_secs(8));
    split.set_readiness(Readiness::Starting);
    let before = Arc::strong_count(&split);
    let pending = {
        let split = split.clone();
        tokio::spawn(async move { split.dial_tcp("100.64.0.12:7400".parse().unwrap()).await })
    };
    tokio::task::yield_now().await;
    pending.abort();
    let _ = pending.await;
    assert_eq!(Arc::strong_count(&split), before);
    split.set_readiness(Readiness::Ready {
        dialer: FakeNet::new("emb"),
        magic_dns_suffix: String::new(),
        peer_names: Vec::new(),
    });
}
