use std::{
    collections::VecDeque,
    io,
    net::SocketAddr,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, Weak,
    },
    time::Duration,
};

use rs_mobile::rscore::{
    dto_core::{TailnetRoute, TailnetRouteStatus},
    error::CoreError,
    tailnet_ctl::{
        ControllerConfig, EmbeddedNode, Inputs, LiveConnections, NodeFuture, NodeSnapshot,
        RouteSink, TailnetController,
    },
    tailnet_policy::TailnetMode,
    tailnet_split::Readiness,
};
use rsp_transport::{net::NetFuture, net::NetStream, NetDialer};
use tokio::{runtime::Handle, time::advance};

const HOSTNAME: &str = "rs-ios-k7m2q4xa";
const URL: &str = "https://hs.example.com";
const START: &str = "start https://hs.example.com rs-ios-k7m2q4xa";

#[derive(Debug)]
struct FakeDialer;

impl NetDialer for FakeDialer {
    fn shared_quic_endpoint(&self, _bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        Ok(None)
    }

    fn dial_udp(&self, _peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        Box::pin(async { Err(io::ErrorKind::NotConnected.into()) })
    }

    fn dial_tcp(&self, _peer: SocketAddr) -> NetFuture<'_, NetStream> {
        Box::pin(async { Err(io::ErrorKind::NotConnected.into()) })
    }

    fn dial_tcp_host<'a>(&'a self, _host: &'a str, _port: u16) -> NetFuture<'a, NetStream> {
        Box::pin(async { Err(io::ErrorKind::NotConnected.into()) })
    }
}

struct FakeNode {
    calls: Mutex<Vec<String>>,
    start_results: Mutex<VecDeque<Result<(), CoreError>>>,
    snap: Mutex<NodeSnapshot>,
    dialer: Mutex<Option<Arc<dyn NetDialer>>>,
    available: bool,
    stop_error: AtomicBool,
    snapshot_error: AtomicBool,
    rebind_error: AtomicBool,
    rebind_pending: AtomicBool,
    start_pending: AtomicBool,
}

impl FakeNode {
    fn new(available: bool) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            start_results: Mutex::new(VecDeque::new()),
            snap: Mutex::new(NodeSnapshot {
                state: "starting".to_owned(),
                ..NodeSnapshot::default()
            }),
            dialer: Mutex::new(None),
            available,
            stop_error: AtomicBool::new(false),
            snapshot_error: AtomicBool::new(false),
            rebind_error: AtomicBool::new(false),
            rebind_pending: AtomicBool::new(false),
            start_pending: AtomicBool::new(false),
        }
    }

    fn run(&self, ips: &[&str]) {
        *self.snap.lock().unwrap() = NodeSnapshot {
            state: "running".to_owned(),
            hostname: HOSTNAME.to_owned(),
            ips: ips.iter().map(|ip| (*ip).to_owned()).collect(),
            derp_region: "1".to_owned(),
            derp_region_name: "Example".to_owned(),
            magic_dns_suffix: "ts.example.com".to_owned(),
            peer_names: vec!["studio-mac".to_owned(), "office-win".to_owned()],
        };
        *self.dialer.lock().unwrap() = Some(Arc::new(FakeDialer));
    }

    fn calls(&self) -> Vec<String> {
        self.calls.lock().unwrap().clone()
    }

    fn count(&self, prefix: &str) -> usize {
        self.calls()
            .iter()
            .filter(|call| call.starts_with(prefix))
            .count()
    }
}

impl EmbeddedNode for FakeNode {
    fn available(&self) -> bool {
        self.available
    }

    fn start<'a>(&'a self, control_url: &'a str, hostname: &'a str) -> NodeFuture<'a, ()> {
        Box::pin(async move {
            self.calls
                .lock()
                .unwrap()
                .push(format!("start {control_url} {hostname}"));
            if self.start_pending.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            let result = self
                .start_results
                .lock()
                .unwrap()
                .pop_front()
                .unwrap_or(Ok(()));
            if result.is_ok() {
                *self.snap.lock().unwrap() = NodeSnapshot {
                    state: "starting".to_owned(),
                    ..NodeSnapshot::default()
                };
                *self.dialer.lock().unwrap() = None;
            }
            result
        })
    }

    fn stop(&self) -> NodeFuture<'_, ()> {
        Box::pin(async {
            self.calls.lock().unwrap().push("stop".to_owned());
            if self.stop_error.load(Ordering::SeqCst) {
                Err(CoreError::network("stop failed"))
            } else {
                Ok(())
            }
        })
    }

    fn rebind(&self) -> NodeFuture<'_, ()> {
        Box::pin(async {
            self.calls.lock().unwrap().push("rebind".to_owned());
            if self.rebind_pending.load(Ordering::SeqCst) {
                std::future::pending::<()>().await;
            }
            if self.rebind_error.load(Ordering::SeqCst) {
                Err(CoreError::network("x"))
            } else {
                Ok(())
            }
        })
    }

    fn snapshot(&self) -> NodeFuture<'_, NodeSnapshot> {
        Box::pin(async {
            self.calls.lock().unwrap().push("snapshot".to_owned());
            if self.snapshot_error.load(Ordering::SeqCst) {
                Err(CoreError::network("snapshot failed"))
            } else {
                Ok(self.snap.lock().unwrap().clone())
            }
        })
    }

    fn dialer(&self) -> Option<Arc<dyn NetDialer>> {
        self.dialer.lock().unwrap().clone()
    }
}

#[derive(Default)]
struct FakeSink {
    routes: Mutex<Vec<TailnetRoute>>,
    readiness: Mutex<Vec<&'static str>>,
    ready_values: Mutex<Vec<Readiness>>,
    statuses: Mutex<Vec<TailnetRouteStatus>>,
    controller: Mutex<Weak<TailnetController>>,
    cached_statuses: Mutex<Vec<TailnetRouteStatus>>,
}

impl FakeSink {
    fn routes(&self) -> Vec<TailnetRoute> {
        self.routes.lock().unwrap().clone()
    }

    fn readiness(&self) -> Vec<&'static str> {
        self.readiness.lock().unwrap().clone()
    }
}

impl RouteSink for FakeSink {
    fn route_changed(&self, route: TailnetRoute) {
        self.routes.lock().unwrap().push(route);
    }

    fn readiness_changed(&self, readiness: Readiness) {
        self.readiness.lock().unwrap().push(match &readiness {
            Readiness::Down => "Down",
            Readiness::Idle => "Idle",
            Readiness::Starting => "Starting",
            Readiness::Ready { .. } => "Ready",
        });
        self.ready_values.lock().unwrap().push(readiness);
    }

    fn status_changed(&self, status: &TailnetRouteStatus) {
        if let Some(controller) = self.controller.lock().unwrap().upgrade() {
            self.cached_statuses
                .lock()
                .unwrap()
                .push(controller.status());
        }
        self.statuses.lock().unwrap().push(status.clone());
    }
}

#[derive(Default)]
struct FakeLive(AtomicBool);

impl LiveConnections for FakeLive {
    fn any(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

struct Fixture {
    node: Arc<FakeNode>,
    sink: Arc<FakeSink>,
    live: Arc<FakeLive>,
    ctl: Arc<TailnetController>,
    inputs: Inputs,
}

impl Fixture {
    fn new(inputs: Inputs) -> Self {
        Self::with_node(inputs, FakeNode::new(true), ControllerConfig::default())
    }

    fn with_node(inputs: Inputs, node: FakeNode, cfg: ControllerConfig) -> Self {
        let node = Arc::new(node);
        let sink = Arc::new(FakeSink::default());
        let live = Arc::new(FakeLive::default());
        let ctl = TailnetController::spawn(
            &Handle::current(),
            node.clone(),
            sink.clone(),
            live.clone(),
            HOSTNAME.to_owned(),
            cfg,
            inputs.clone(),
        );
        *sink.controller.lock().unwrap() = Arc::downgrade(&ctl);
        Self {
            node,
            sink,
            live,
            ctl,
            inputs,
        }
    }

    async fn update(&self) {
        self.ctl.update(self.inputs.clone());
        settle().await;
    }

    async fn running(&self) {
        settle().await;
        self.node.run(&["100.64.0.21"]);
        self.changed().await;
        assert_eq!(self.sink.readiness().last(), Some(&"Ready"));
    }

    async fn idle(&mut self) {
        self.running().await;
        self.inputs.app_active = false;
        self.update().await;
        tick(Duration::from_secs(31)).await;
        assert_eq!(self.node.count("stop"), 1);
        assert_eq!(self.sink.readiness().last(), Some(&"Idle"));
    }

    async fn needs_login(&self) {
        settle().await;
        self.node.snap.lock().unwrap().state = "needsLogin".to_owned();
        self.changed().await;
        assert_eq!(self.sink.readiness().last(), Some(&"Down"));
    }

    async fn changed(&self) {
        self.ctl.node_changed();
        settle().await;
        tick(Duration::from_millis(300)).await;
    }
}

fn embedded() -> Inputs {
    Inputs {
        mode: TailnetMode::Embedded,
        control_url: Some(URL.to_owned()),
        system_tailnet_up: false,
        app_active: true,
    }
}

async fn settle() {
    for _ in 0..20 {
        tokio::task::yield_now().await;
    }
}

async fn tick(duration: Duration) {
    advance(duration).await;
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn foreground_rebinds_once() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    let readiness = f.sink.readiness();
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(10)).await;
    f.inputs.app_active = true;
    f.update().await;
    tick(Duration::from_millis(999)).await;
    assert_eq!(f.node.count("rebind"), 0);
    tick(Duration::from_millis(1)).await;
    assert_eq!(f.node.count("rebind"), 1);
    assert_eq!(f.node.count("stop"), 0);
    assert_eq!(f.sink.readiness(), readiness);
    assert_eq!(f.sink.readiness().last(), Some(&"Ready"));
    assert_eq!(f.sink.routes(), [TailnetRoute::Embedded]);
    // Repeated active inputs do not schedule another rebind.
    f.update().await;
    tick(Duration::from_secs(31)).await;
    assert_eq!(f.node.count("rebind"), 1);
    assert_eq!(f.node.count("stop"), 0);
}

#[tokio::test(start_paused = true)]
async fn foreground_after_stop_restarts() {
    let mut f = Fixture::new(embedded());
    f.idle().await;
    let before = f.node.calls().len();
    f.inputs.app_active = true;
    f.update().await;
    assert_eq!(f.node.count("start"), 2);
    assert_eq!(&f.node.calls()[before..], [START, "snapshot"]);
    assert_eq!(
        f.sink.readiness(),
        ["Starting", "Ready", "Idle", "Starting"]
    );
    f.running().await;
    assert_eq!(f.sink.readiness().last(), Some(&"Ready"));
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("rebind"), 0);
}

#[tokio::test(start_paused = true)]
async fn wake_from_idle_starts() {
    let mut f = Fixture::new(embedded());
    f.idle().await;
    f.ctl.wake();
    settle().await;
    assert!(!f.inputs.app_active);
    assert_eq!(f.node.count("start"), 2);
    assert_eq!(f.sink.readiness().last(), Some(&"Starting"));
    f.running().await;
    tick(Duration::from_secs(59)).await;
    assert_eq!(f.node.count("stop"), 1);
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("stop"), 2);
    assert_eq!(f.sink.readiness().last(), Some(&"Idle"));
}

#[tokio::test(start_paused = true)]
async fn wake_then_session_keeps_node() {
    let mut f = Fixture::new(embedded());
    f.idle().await;
    f.ctl.wake();
    settle().await;
    f.running().await;
    tick(Duration::from_secs(30)).await;
    f.live.0.store(true, Ordering::SeqCst);
    tick(Duration::from_secs(120)).await;
    assert_eq!(f.node.count("stop"), 1);
    assert_eq!(f.sink.readiness().last(), Some(&"Ready"));
    f.live.0.store(false, Ordering::SeqCst);
    tick(Duration::from_secs(5)).await;
    assert_eq!(f.node.count("stop"), 2);
}

#[tokio::test(start_paused = true)]
async fn network_change_debounced() {
    let f = Fixture::new(embedded());
    f.running().await;
    for _ in 0..5 {
        f.ctl.network_changed();
        settle().await;
        tick(Duration::from_millis(100)).await;
        assert_eq!(f.node.count("rebind"), 0);
    }
    tick(Duration::from_millis(899)).await;
    assert_eq!(f.node.count("rebind"), 0);
    tick(Duration::from_millis(1)).await;
    assert_eq!(f.node.count("rebind"), 1);
    assert_eq!(f.node.count("stop"), 0);
    assert_eq!(f.sink.readiness(), ["Starting", "Ready"]);
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("rebind"), 1);
}

#[tokio::test(start_paused = true)]
async fn network_change_ignored_unless_running() {
    let mut f = Fixture::new(embedded());
    settle().await;
    assert_eq!(f.ctl.status().embedded.unwrap().state, "starting");
    f.ctl.network_changed();
    settle().await;
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("rebind"), 0);
    f.needs_login().await;
    f.ctl.network_changed();
    settle().await;
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("rebind"), 0);
    f.running().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(31)).await;
    assert_eq!(f.sink.readiness().last(), Some(&"Idle"));
    f.ctl.network_changed();
    settle().await;
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("rebind"), 0);
    assert_eq!(f.node.count("start"), 1);
}

#[tokio::test(start_paused = true)]
async fn needs_login_held_in_background() {
    let mut f = Fixture::new(embedded());
    f.needs_login().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(300)).await;
    assert_eq!(f.node.count("stop"), 0);
    tick(Duration::from_secs(299)).await;
    assert_eq!(f.node.count("stop"), 0);
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("stop"), 1);
    assert_eq!(f.sink.readiness().last(), Some(&"Idle"));
}

#[tokio::test(start_paused = true)]
async fn login_done_clears_hold() {
    let mut f = Fixture::new(embedded());
    f.needs_login().await;
    f.running().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(29)).await;
    assert_eq!(f.node.count("stop"), 0);
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("stop"), 1);
    assert_eq!(f.sink.readiness().last(), Some(&"Idle"));
}

#[tokio::test(start_paused = true)]
async fn rebind_error_only_logged() {
    let f = Fixture::new(embedded());
    f.running().await;
    f.node.rebind_error.store(true, Ordering::SeqCst);
    let before = f.node.calls().len();
    f.ctl.network_changed();
    settle().await;
    tick(Duration::from_secs(1)).await;
    assert_eq!(&f.node.calls()[before..], ["rebind", "snapshot"]);
    assert_eq!(f.node.count("stop"), 0);
    assert_eq!(f.sink.readiness(), ["Starting", "Ready"]);
}

#[tokio::test(start_paused = true)]
async fn repeated_wake_does_not_extend_window() {
    let mut f = Fixture::new(embedded());
    f.idle().await;
    f.ctl.wake();
    settle().await;
    f.running().await;
    tick(Duration::from_secs(30)).await;
    f.ctl.wake();
    settle().await;
    tick(Duration::from_secs(29)).await;
    assert_eq!(f.node.count("stop"), 1);
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("stop"), 2);
    assert_eq!(f.sink.readiness().last(), Some(&"Idle"));
    // A wake after expiry opens a new window.
    f.ctl.wake();
    settle().await;
    assert_eq!(f.node.count("start"), 3);
    tick(Duration::from_secs(59)).await;
    assert_eq!(f.node.count("stop"), 2);
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("stop"), 3);
}

#[tokio::test(start_paused = true)]
async fn repeated_login_snapshots_do_not_extend_hold() {
    let mut f = Fixture::new(embedded());
    f.needs_login().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(300)).await;
    f.changed().await;
    tick(Duration::from_secs(299)).await;
    assert_eq!(f.node.count("stop"), 0);
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("stop"), 1);
}

#[tokio::test(start_paused = true)]
async fn custom_wake_window_is_used() {
    let cfg = ControllerConfig {
        wake_window: Duration::from_secs(2),
        ..ControllerConfig::default()
    };
    let mut f = Fixture::with_node(embedded(), FakeNode::new(true), cfg);
    f.idle().await;
    f.ctl.wake();
    settle().await;
    f.running().await;
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("stop"), 1);
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("stop"), 2);
}

#[tokio::test(start_paused = true)]
async fn custom_login_hold_is_used() {
    let cfg = ControllerConfig {
        background_grace: Duration::from_secs(1),
        login_hold: Duration::from_secs(2),
        ..ControllerConfig::default()
    };
    let mut f = Fixture::with_node(embedded(), FakeNode::new(true), cfg);
    f.needs_login().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_millis(1999)).await;
    assert_eq!(f.node.count("stop"), 0);
    tick(Duration::from_millis(1)).await;
    assert_eq!(f.node.count("stop"), 1);
}

#[tokio::test(start_paused = true)]
async fn custom_rebind_debounce_is_used() {
    let cfg = ControllerConfig {
        rebind_debounce: Duration::from_millis(50),
        ..ControllerConfig::default()
    };
    let mut f = Fixture::with_node(embedded(), FakeNode::new(true), cfg);
    f.running().await;
    f.ctl.network_changed();
    settle().await;
    tick(Duration::from_millis(49)).await;
    assert_eq!(f.node.count("rebind"), 0);
    tick(Duration::from_millis(1)).await;
    assert_eq!(f.node.count("rebind"), 1);
    f.inputs.app_active = false;
    f.update().await;
    f.inputs.app_active = true;
    f.update().await;
    tick(Duration::from_millis(49)).await;
    assert_eq!(f.node.count("rebind"), 1);
    tick(Duration::from_millis(1)).await;
    assert_eq!(f.node.count("rebind"), 2);
}

#[tokio::test(start_paused = true)]
async fn stop_cancels_pending_rebind() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.ctl.network_changed();
    settle().await;
    f.inputs.mode = TailnetMode::System;
    f.update().await;
    assert_eq!(f.node.count("stop"), 1);
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("rebind"), 0);
    f.inputs.mode = TailnetMode::Embedded;
    f.update().await;
    f.running().await;
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("start"), 2);
    assert_eq!(f.node.count("rebind"), 0);
}

#[tokio::test(start_paused = true)]
async fn foreground_restart_reports_starting_before_start_returns() {
    let mut f = Fixture::new(embedded());
    f.idle().await;
    f.node.start_pending.store(true, Ordering::SeqCst);
    f.inputs.app_active = true;
    f.update().await;
    assert_eq!(f.node.count("start"), 2);
    assert_eq!(f.sink.readiness().last(), Some(&"Starting"));
    f.ctl.shutdown();
}

#[tokio::test(start_paused = true)]
async fn rebind_in_progress_keeps_readiness() {
    let f = Fixture::new(embedded());
    f.running().await;
    f.node.rebind_pending.store(true, Ordering::SeqCst);
    f.ctl.network_changed();
    settle().await;
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("rebind"), 1);
    assert_eq!(f.node.count("stop"), 0);
    assert_eq!(f.sink.readiness(), ["Starting", "Ready"]);
    f.ctl.shutdown();
}
