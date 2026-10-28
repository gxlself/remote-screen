use rs_tailnet::{
    testkit::{temp_dir, TestNet, TestNetOptions, TestPaths},
    BackendState, Tailnet, TailnetConfig, TailnetEvent, TailnetStatus,
};
use rsp_transport::{
    dial::DefaultDialer,
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    Conn, ConnError, ControlStream, QuicListener, TcpListener, TransportKind,
};
use std::{
    future::Future,
    net::{Ipv4Addr, SocketAddr},
    sync::{Arc, Mutex},
    time::Duration,
};

pub fn local() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

pub fn identities() -> (LocalIdentity, LocalIdentity) {
    (
        LocalIdentity::generate_software(CertKind::Host).unwrap(),
        LocalIdentity::generate_software(CertKind::App).unwrap(),
    )
}

pub async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(30), future)
        .await
        .expect("tailnet operation timed out")
}

pub struct Fixture {
    pub node: Arc<Tailnet>,
    pub testnet: Arc<TestNet>,
    pub cfg: TailnetConfig,
    pub events: Arc<Mutex<Vec<TailnetEvent>>>,
    // cargo test can run tests in the same process; Go has one node per process.
    _serial: tokio::sync::MutexGuard<'static, ()>,
}

impl Fixture {
    pub async fn new(paths: TestPaths, options: TestNetOptions) -> Self {
        static SERIAL: tokio::sync::Mutex<()> = tokio::sync::Mutex::const_new(());
        let serial = SERIAL.lock().await;
        tokio::task::spawn_blocking(move || {
            let testnet = Arc::new(TestNet::spawn(&paths, &options).unwrap());
            let node = Tailnet::global(rs_tailnet::load(&paths.lib).unwrap());
            let cfg = TailnetConfig {
                dir: temp_dir("node"),
                log_file: Some(temp_dir("log").join("tailnet.log")),
                hostname: "rs-app".into(),
                control_url: testnet.control_url.clone(),
                auth_key: if options.require_auth {
                    None
                } else {
                    Some(testnet.auth_key.clone())
                },
                disable_netns: true,
                ..Default::default()
            };
            Self {
                node,
                testnet,
                cfg,
                events: Arc::new(Mutex::new(Vec::new())),
                _serial: serial,
            }
        })
        .await
        .unwrap()
    }

    pub async fn start(&self) {
        let (node, cfg, events) = (self.node.clone(), self.cfg.clone(), self.events.clone());
        tokio::task::spawn_blocking(move || {
            node.start(
                &cfg,
                Arc::new(move |event| {
                    events.lock().unwrap().push(event);
                }),
            )
        })
        .await
        .unwrap()
        .unwrap();
    }

    pub async fn stop(&self) {
        let node = self.node.clone();
        tokio::task::spawn_blocking(move || node.stop())
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn status(&self) -> TailnetStatus {
        let node = self.node.clone();
        tokio::task::spawn_blocking(move || node.status())
            .await
            .unwrap()
            .unwrap()
    }

    pub async fn event(
        &self,
        seconds: u64,
        predicate: impl Fn(&TailnetEvent) -> bool,
    ) -> TailnetEvent {
        tokio::time::timeout(Duration::from_secs(seconds), async {
            loop {
                if let Some(event) = self
                    .events
                    .lock()
                    .unwrap()
                    .iter()
                    .find(|event| predicate(event))
                    .cloned()
                {
                    return event;
                }
                tokio::time::sleep(Duration::from_millis(20)).await;
            }
        })
        .await
        .unwrap_or_else(|_| {
            let events = self.events.lock().unwrap();
            let states: Vec<_> = events.iter().filter_map(|event| match event {
                TailnetEvent::State { state, .. } => Some(*state), _ => None,
            }).collect();
            let auth_urls = events.iter().filter(|event| matches!(event, TailnetEvent::AuthUrl { .. })).count();
            let errors = events.iter().filter(|event| matches!(event, TailnetEvent::Error { .. })).count();
            drop(events);
            panic!("expected tailnet event did not arrive: states={states:?}, auth_url_count={auth_urls}, error_count={errors}")
        })
    }

    pub async fn running(&self) {
        self.event(30, |event| {
            matches!(
                event,
                TailnetEvent::State {
                    state: BackendState::Running,
                    ..
                }
            )
        })
        .await;
    }

    pub async fn complete(&self, url: String) {
        let testnet = self.testnet.clone();
        tokio::task::spawn_blocking(move || testnet.complete(&url))
            .await
            .unwrap()
            .unwrap();
    }

    pub async fn finish(self) {
        self.stop().await;
        tokio::task::spawn_blocking(move || drop(self))
            .await
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Explicit finish runs this on a blocking worker. This also cleans up if
        // any assertion unwinds before finish, while TestNet subsequently reaps Go.
        let _ = self.node.stop();
        let _ = std::fs::remove_dir_all(&self.cfg.dir);
        if let Some(parent) = self.cfg.log_file.as_ref().and_then(|path| path.parent()) {
            let _ = std::fs::remove_dir_all(parent);
        }
    }
}

pub fn self_ip4(status: &TailnetStatus) -> Ipv4Addr {
    status
        .self_node
        .as_ref()
        .unwrap()
        .ips
        .iter()
        .find_map(|ip| match ip {
            std::net::IpAddr::V4(ip) => Some(*ip),
            _ => None,
        })
        .unwrap()
}

// Based on rsp-transport/tests/{fdnet,net_route}.rs: software identities and
// transport-native control, datagram, uni-stream and input attachment helpers.
pub async fn quic_pair(
    dialer: &DefaultDialer,
    listener: &QuicListener,
    peer: SocketAddr,
) -> (Conn, Conn) {
    let accept = async { listener.accept().await.unwrap().accept().await.unwrap() };
    let (client, server) =
        bounded(async { tokio::join!(dialer.dial(peer, TransportKind::Quic), accept) }).await;
    (client.unwrap(), server)
}

pub async fn controls(client: &Conn, server: &Conn) -> (ControlStream, ControlStream) {
    bounded(async {
        let tx = client.open_control().await.unwrap();
        tx.send(b"bootstrap").await.unwrap();
        let rx = server.accept_control().await.unwrap();
        assert_eq!(rx.recv().await.unwrap().unwrap(), b"bootstrap"[..]);
        (tx, rx)
    })
    .await
}

pub async fn tcp_pair(
    dial: impl Future<Output = Result<Conn, ConnError>>,
    listener: &TcpListener,
) -> (Conn, Conn, ControlStream, ControlStream) {
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
        let ((client, tx), server) = tokio::join!(connect, accept);
        let rx = server.accept_control().await.unwrap();
        assert_eq!(rx.recv().await.unwrap().unwrap(), b"bootstrap"[..]);
        (client, server, tx, rx)
    })
    .await
}
