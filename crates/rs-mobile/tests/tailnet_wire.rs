//! Run with nextest: environment and tailnet ABI state are process-global.
#![cfg(unix)]

mod support;

use std::{
    io,
    net::SocketAddr,
    sync::{Arc, Mutex},
    time::Duration,
};

use rs_mobile::rscore::{
    api_diag::{refresh_network, set_setting},
    error::CoreError,
    netinfo::RawIface,
    runtime::Core,
    tailnet_ctl::{ControllerConfig, EmbeddedNode, NodeFuture, NodeSnapshot},
    tailnet_wire::{self, WireOptions},
};
use rsp_transport::{net::NetFuture, NetDialer, NetStream};
use serde_json::json;
use tokio::{
    io::{AsyncReadExt, AsyncWriteExt},
    net::{TcpListener, TcpStream},
    task::{JoinHandle, JoinSet},
    time::{sleep, timeout},
};

use support::{test_core_with, with_identity, EventCollector};

#[derive(Debug, Default)]
struct BaseNet(Mutex<Vec<String>>);

impl NetDialer for BaseNet {
    fn shared_quic_endpoint(&self, _: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        Ok(None)
    }

    fn dial_udp(&self, _: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        self.0.lock().unwrap().push(peer.to_string());
        Box::pin(async { Err(io::ErrorKind::NotConnected.into()) })
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, _: u16) -> NetFuture<'a, NetStream> {
        self.0.lock().unwrap().push(host.to_owned());
        Box::pin(async { Err(io::ErrorKind::NotConnected.into()) })
    }
}

#[derive(Debug)]
struct LocalDialer {
    echo: SocketAddr,
}

impl NetDialer for LocalDialer {
    fn shared_quic_endpoint(&self, _: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        Ok(None)
    }

    fn dial_udp(&self, _: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        Box::pin(async { Err(io::ErrorKind::Unsupported.into()) })
    }

    fn dial_tcp(&self, _: SocketAddr) -> NetFuture<'_, NetStream> {
        Box::pin(async { Ok(Box::new(TcpStream::connect(self.echo).await?) as NetStream) })
    }

    fn dial_tcp_host<'a>(&'a self, _: &'a str, _: u16) -> NetFuture<'a, NetStream> {
        Box::pin(async { Ok(Box::new(TcpStream::connect(self.echo).await?) as NetStream) })
    }
}

struct FakeNode {
    calls: Mutex<Vec<String>>,
    snapshot: Mutex<NodeSnapshot>,
    net: Arc<LocalDialer>,
}

impl FakeNode {
    fn run(&self) {
        let mut snapshot = self.snapshot.lock().unwrap();
        snapshot.state = "running".into();
        snapshot.ips = vec!["100.64.0.21".into()];
        snapshot.peer_names = vec!["rs-peer".into()];
    }

    fn count(&self, prefix: &str) -> usize {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter(|s| s.starts_with(prefix))
            .count()
    }
}

impl EmbeddedNode for FakeNode {
    fn available(&self) -> bool {
        true
    }

    fn start<'a>(&'a self, url: &'a str, hostname: &'a str) -> NodeFuture<'a, ()> {
        Box::pin(async move {
            *self.snapshot.lock().unwrap() = NodeSnapshot {
                state: "starting".into(),
                hostname: hostname.into(),
                ..NodeSnapshot::default()
            };
            self.calls
                .lock()
                .unwrap()
                .push(format!("start {url} {hostname}"));
            Ok(())
        })
    }

    fn stop(&self) -> NodeFuture<'_, ()> {
        Box::pin(async {
            self.snapshot.lock().unwrap().state = "stopped".into();
            self.calls.lock().unwrap().push("stop".into());
            Ok(())
        })
    }

    fn rebind(&self) -> NodeFuture<'_, ()> {
        Box::pin(async {
            self.calls.lock().unwrap().push("rebind".into());
            Ok(())
        })
    }

    fn snapshot(&self) -> NodeFuture<'_, NodeSnapshot> {
        Box::pin(async { Ok(self.snapshot.lock().unwrap().clone()) })
    }

    fn dialer(&self) -> Option<Arc<dyn NetDialer>> {
        (self.snapshot.lock().unwrap().state == "running")
            .then(|| self.net.clone() as Arc<dyn NetDialer>)
    }
}

struct Echo {
    addr: SocketAddr,
    task: JoinHandle<()>,
}

impl Echo {
    async fn start() -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let addr = listener.local_addr().unwrap();
        let task = tokio::spawn(async move {
            let mut clients = JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (stream, _) = accepted.unwrap();
                        clients.spawn(async move {
                            let (mut read, mut write) = stream.into_split();
                            let _ = tokio::io::copy(&mut read, &mut write).await;
                        });
                    }
                    _ = clients.join_next(), if !clients.is_empty() => {}
                }
            }
        });
        Self { addr, task }
    }
}

impl Drop for Echo {
    fn drop(&mut self) {
        self.task.abort();
    }
}

struct RunningCore {
    core: Arc<Core>,
    node: Arc<FakeNode>,
    base: Arc<BaseNet>,
    events: Arc<EventCollector>,
    _echo: Echo,
}

async fn wait_until(seconds: u64, predicate: impl Fn() -> bool) {
    timeout(Duration::from_secs(seconds), async {
        while !predicate() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("condition did not become true before deadline");
}

async fn running_core() -> RunningCore {
    let echo = Echo::start().await;
    let base = Arc::new(BaseNet::default());
    let core = test_core_with(|_, opts| {
        opts.net = Some(base.clone());
    });
    with_identity(&core, "phone");
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    let node = Arc::new(FakeNode {
        calls: Mutex::new(Vec::new()),
        snapshot: Mutex::new(NodeSnapshot::default()),
        net: Arc::new(LocalDialer { echo: echo.addr }),
    });
    tailnet_wire::install_with(
        &core,
        node.clone(),
        WireOptions {
            controller: ControllerConfig {
                background_grace: Duration::from_secs(1),
                ..ControllerConfig::default()
            },
            ..WireOptions::default()
        },
    )
    .unwrap();
    // Installing twice must preserve the original base network and controller.
    tailnet_wire::install(&core).unwrap();
    set_setting(&core, "tailnet.mode", "\"embedded\"").unwrap();
    set_setting(&core, "tailnet.controlUrl", "\"https://hs.example.com\"").unwrap();
    wait_until(5, || node.count("start") == 1).await;
    let expected = format!(
        "start https://hs.example.com rs-ios-{}",
        &core.device_id().unwrap()[..8]
    );
    assert_eq!(*node.calls.lock().unwrap(), [expected]);
    node.run();
    tailnet_wire::node_changed(&core);
    wait_until(5, || {
        events.of_type("network.changed").iter().any(|event| {
            event["status"]["tailnet"]["active"] == "embedded"
                && event["status"]["tailnet"]["embedded"]["state"] == "running"
        })
    })
    .await;
    RunningCore {
        core,
        node,
        base,
        events,
        _echo: echo,
    }
}

fn iface(name: &str, addr: &str) -> RawIface {
    RawIface {
        name: name.into(),
        addrs: vec![addr.parse().unwrap()],
    }
}

async fn echo(mut stream: NetStream) {
    timeout(Duration::from_secs(5), async {
        let data = vec![0x5a; 4096];
        stream.write_all(&data).await.unwrap();
        let mut got = vec![0; data.len()];
        stream.read_exact(&mut got).await.unwrap();
        assert_eq!(got, data);
    })
    .await
    .unwrap();
}

#[test]
fn untouched_core_has_no_tailnet() {
    let core = test_core_with(|_, _| {});
    set_setting(&core, "networks", "[]").unwrap();
    tailnet_wire::on_setting(&core, "quality.default").unwrap();
    tailnet_wire::node_changed(&core);
    assert!(refresh_network(&core, &[]).tailnet.is_none());
    assert!(!tailnet_wire::installed(&core));
}

#[tokio::test]
async fn unavailable_build_reports_status() {
    std::env::remove_var("RS_TAILNET_LIB");
    let core = test_core_with(|_, _| {});
    let base = core.net();
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    tailnet_wire::install(&core).unwrap();
    set_setting(&core, "tailnet.mode", "\"embedded\"").unwrap();
    set_setting(&core, "tailnet.controlUrl", "\"https://hs.example.com\"").unwrap();
    let expected = json!({"mode":"embedded", "active":"system", "available":false,
        "controlUrlSet":true, "embedded":null});
    wait_until(5, || {
        events
            .of_type("network.changed")
            .iter()
            .any(|event| event["status"]["tailnet"] == expected)
    })
    .await;
    assert!(Arc::ptr_eq(&base, &core.net()));
}

#[tokio::test]
async fn fake_node_switches_net() {
    let fixture = running_core().await;
    echo(
        fixture
            .core
            .net()
            .dial_tcp("100.64.0.12:7400".parse().unwrap())
            .await
            .unwrap(),
    )
    .await;
    // Non-tailnet addresses still use the injected base network.
    assert!(fixture
        .core
        .net()
        .dial_tcp("10.30.1.5:7400".parse().unwrap())
        .await
        .is_err());
    assert_eq!(*fixture.base.0.lock().unwrap(), ["10.30.1.5:7400"]);
}

#[tokio::test]
async fn background_stop_and_wake() {
    let fixture = running_core().await;
    set_setting(&fixture.core, "device.appActive", "false").unwrap();
    wait_until(3, || fixture.node.count("stop") == 1).await;
    let net = fixture.core.net();
    let dial = tokio::spawn(async move { net.dial_tcp("100.64.0.12:7400".parse().unwrap()).await });
    wait_until(2, || fixture.node.count("start") == 2).await;
    fixture.node.run();
    tailnet_wire::node_changed(&fixture.core);
    echo(
        timeout(Duration::from_secs(5), dial)
            .await
            .unwrap()
            .unwrap()
            .unwrap(),
    )
    .await;
}

#[tokio::test]
async fn mode_system_restores_base() {
    let fixture = running_core().await;
    set_setting(&fixture.core, "tailnet.mode", "\"system\"").unwrap();
    let base: Arc<dyn NetDialer> = fixture.base.clone();
    wait_until(2, || {
        Arc::ptr_eq(&base, &fixture.core.net()) && fixture.node.count("stop") == 1
    })
    .await;
}

#[tokio::test]
async fn iface_change_rebinds() {
    let fixture = running_core().await;
    let first = [
        iface("rs-test-a", "192.168.31.45/24"),
        iface("rs-test-b", "10.30.1.5/16"),
    ];
    refresh_network(&fixture.core, &first);
    // The first refresh may differ from the real interfaces sampled by
    // install_with. Let its debounced rebind, if any, finish before recording
    // the baseline count.
    sleep(Duration::from_millis(1200)).await;
    let n0 = fixture.node.count("rebind");

    // Interface order is normalized by network_status_marked, so this is the
    // same snapshot and must not schedule another rebind.
    let same = [
        iface("rs-test-b", "10.30.1.5/16"),
        iface("rs-test-a", "192.168.31.45/24"),
    ];
    refresh_network(&fixture.core, &same);
    sleep(Duration::from_millis(1200)).await;
    assert_eq!(fixture.node.count("rebind"), n0);

    let changed = [
        iface("rs-test-b", "10.30.1.6/16"),
        iface("rs-test-a", "192.168.31.45/24"),
    ];
    refresh_network(&fixture.core, &changed);
    wait_until(3, || fixture.node.count("rebind") > n0).await;
    assert_eq!(fixture.node.count("rebind"), n0 + 1);
}

#[tokio::test]
async fn core_not_retained() {
    let RunningCore {
        core,
        node,
        base,
        events,
        _echo,
    } = running_core().await;
    drop((node, base, events));
    let settled = timeout(Duration::from_secs(2), async {
        while Arc::strong_count(&core) != 1 {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .is_ok();
    let strong_count = Arc::strong_count(&core);
    assert!(
        settled,
        "Core remained retained after 2s (strong_count={strong_count})"
    );
    assert_eq!(
        strong_count, 1,
        "Core strong count changed after settling (strong_count={strong_count})"
    );
    let weak = Arc::downgrade(&core);
    drop(core);
    assert!(weak.upgrade().is_none());
}

#[tokio::test]
async fn dropped_core_node_returns_errors() {
    let core = test_core_with(|_, _| {});
    let node = tailnet_wire::real_node(&core, true);
    drop(core);
    assert_eq!(
        node.start("https://hs.example.com", "rs-ios-test").await,
        Err(CoreError::internal("core dropped"))
    );
    assert_eq!(node.stop().await, Err(CoreError::internal("core dropped")));
    assert_eq!(
        node.rebind().await,
        Err(CoreError::internal("core dropped"))
    );
    assert_eq!(
        node.snapshot().await,
        Err(CoreError::internal("core dropped"))
    );
    assert!(node.dialer().is_none());
}
