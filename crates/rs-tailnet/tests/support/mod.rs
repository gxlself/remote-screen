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
    panic::Location,
    path::Path,
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

/// Budget for one fixture step. Each step normally takes well under a second;
/// a stall must fail naming the step instead of reaching nextest's timeout.
pub const STEP: Duration = Duration::from_secs(20);

pub fn local() -> SocketAddr {
    SocketAddr::from(([127, 0, 0, 1], 0))
}

pub fn identities() -> (LocalIdentity, LocalIdentity) {
    (
        LocalIdentity::generate_software(CertKind::Host).unwrap(),
        LocalIdentity::generate_software(CertKind::App).unwrap(),
    )
}

pub async fn bounded<T>(step: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(STEP, future)
        .await
        .unwrap_or_else(|_| panic!("{step}: no result within {STEP:?}"))
}

/// Runs a blocking node call on a detached thread. With spawn_blocking a hung
/// call would still hang the test: runtime shutdown waits for blocking tasks.
pub async fn blocking<T: Send + 'static>(
    step: &str,
    limit: Duration,
    call: impl FnOnce() -> T + Send + 'static,
) -> T {
    let (tx, rx) = tokio::sync::oneshot::channel();
    thread::Builder::new()
        .name("rs-tailnet-test-call".into())
        .spawn(move || {
            let _ = tx.send(call());
        })
        .unwrap();
    match tokio::time::timeout(limit, rx).await {
        Ok(Ok(value)) => value,
        Ok(Err(_)) => panic!("{step}: call panicked"),
        Err(_) => panic!("{step}: call still blocked after {limit:?}"),
    }
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
        let require_auth = options.require_auth;
        // TestNet::spawn bounds the helper's announcement at 30 s and then
        // reports its output tails; leave it room to do so.
        let (testnet, node) = blocking("spawn testnet", STEP * 2, move || {
            let testnet = Arc::new(TestNet::spawn(&paths, &options).unwrap());
            let node = Tailnet::global(rs_tailnet::load(&paths.lib).unwrap());
            (testnet, node)
        })
        .await;
        let cfg = TailnetConfig {
            dir: temp_dir("node"),
            log_file: Some(temp_dir("log").join("tailnet.log")),
            hostname: "rs-app".into(),
            control_url: testnet.control_url.clone(),
            auth_key: if require_auth {
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
    }

    // The lifecycle wrappers are plain functions returning futures so that
    // #[track_caller] can name the test line in a timeout message.
    #[track_caller]
    pub fn start(&self) -> impl Future<Output = ()> + 'static {
        let at = Location::caller();
        let (node, cfg, events) = (self.node.clone(), self.cfg.clone(), self.events.clone());
        async move {
            let callback = Arc::new(move |event: TailnetEvent| events.lock().unwrap().push(event));
            blocking(&format!("start node at {at}"), STEP, move || {
                node.start(&cfg, callback)
            })
            .await
            .unwrap_or_else(|error| panic!("start node at {at}: {error}"));
        }
    }

    #[track_caller]
    pub fn stop(&self) -> impl Future<Output = ()> + 'static {
        let at = Location::caller();
        let node = self.node.clone();
        async move {
            blocking(&format!("stop node at {at}"), STEP, move || node.stop())
                .await
                .unwrap_or_else(|error| panic!("stop node at {at}: {error}"));
        }
    }

    #[track_caller]
    pub fn status(&self) -> impl Future<Output = TailnetStatus> + 'static {
        let at = Location::caller();
        let node = self.node.clone();
        async move {
            blocking(&format!("node status at {at}"), STEP, move || node.status())
                .await
                .unwrap_or_else(|error| panic!("node status at {at}: {error}"))
        }
    }

    #[track_caller]
    pub fn event<'a>(
        &'a self,
        seconds: u64,
        predicate: impl Fn(&TailnetEvent) -> bool + 'a,
    ) -> impl Future<Output = TailnetEvent> + 'a {
        self.wait_event(Location::caller(), seconds, predicate)
    }

    #[track_caller]
    pub fn running(&self) -> impl Future<Output = ()> + '_ {
        let wait = self.wait_event(Location::caller(), 30, |event| {
            matches!(
                event,
                TailnetEvent::State {
                    state: BackendState::Running,
                    ..
                }
            )
        });
        async move {
            wait.await;
        }
    }

    async fn wait_event(
        &self,
        at: &Location<'_>,
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
            panic!("tailnet event awaited at {at} did not arrive within {seconds} s: states={states:?}, auth_url_count={auth_urls}, error_count={errors}")
        })
    }

    #[track_caller]
    pub fn complete(&self, url: String) -> impl Future<Output = ()> + 'static {
        let at = Location::caller();
        let testnet = self.testnet.clone();
        async move {
            blocking(&format!("complete login at {at}"), STEP, move || {
                testnet.complete(&url)
            })
            .await
            .unwrap_or_else(|error| panic!("complete login at {at}: {error}"));
        }
    }

    #[track_caller]
    pub fn finish(self) -> impl Future<Output = ()> + 'static {
        let at = Location::caller();
        let stop = self.stop();
        async move {
            stop.await;
            // Drop bounds its own stop at STEP; TestNet then waits up to 5 s
            // for the helper to exit before killing it.
            blocking(&format!("drop fixture at {at}"), STEP * 2, move || {
                drop(self)
            })
            .await;
        }
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        // Explicit finish runs this on its own thread. This also cleans up if
        // any assertion unwinds before finish, while TestNet subsequently reaps
        // Go. A hung stop is left behind so the failure still gets reported.
        let node = self.node.clone();
        let mut dirs = vec![self.cfg.dir.clone()];
        dirs.extend(
            self.cfg
                .log_file
                .as_ref()
                .and_then(|path| path.parent())
                .map(Path::to_path_buf),
        );
        let (done, cleaned) = mpsc::channel();
        let cleanup = thread::Builder::new().name("rs-tailnet-test-cleanup".into());
        let _ = cleanup.spawn(move || {
            let _ = node.stop();
            for dir in dirs {
                let _ = std::fs::remove_dir_all(dir);
            }
            let _ = done.send(());
        });
        if cleaned.recv_timeout(STEP).is_err() {
            eprintln!("fixture cleanup: tailnet stop still blocked after {STEP:?}");
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
    let (client, server) = bounded("QUIC dial and accept", async {
        tokio::join!(dialer.dial(peer, TransportKind::Quic), accept)
    })
    .await;
    (client.unwrap(), server)
}

pub async fn controls(client: &Conn, server: &Conn) -> (ControlStream, ControlStream) {
    bounded("control stream bootstrap", async {
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
    bounded("TCP dial, accept and bootstrap", async {
        let ((client, tx), server) = tokio::join!(connect, accept);
        let rx = server.accept_control().await.unwrap();
        assert_eq!(rx.recv().await.unwrap().unwrap(), b"bootstrap"[..]);
        (client, server, tx, rx)
    })
    .await
}
