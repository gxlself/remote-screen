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
            Ok(())
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
async fn system_mode_never_starts() {
    let f = Fixture::new(Inputs {
        mode: TailnetMode::System,
        ..embedded()
    });
    settle().await;
    assert!(f.node.calls().is_empty());
    assert_eq!(f.sink.routes(), [TailnetRoute::System]);
    assert_eq!(f.ctl.status().active, TailnetRoute::System);
    assert_eq!(f.ctl.status().embedded, None);
}

#[tokio::test(start_paused = true)]
async fn embedded_starts_then_ready() {
    let f = Fixture::new(embedded());
    settle().await;
    assert_eq!(f.node.calls(), [START, "snapshot"]);
    assert_eq!(f.sink.readiness(), ["Starting"]);
    f.running().await;
    assert_eq!(f.sink.routes(), [TailnetRoute::Embedded]);
    let summary = f.ctl.status().embedded.unwrap();
    assert_eq!(summary.state, "running");
    assert_eq!(summary.ips, ["100.64.0.21"]);
    assert_eq!(summary.hostname, HOSTNAME);
    assert_eq!(summary.derp_region, "1");
    assert_eq!(summary.derp_region_name, "Example");
    let values = f.sink.ready_values.lock().unwrap();
    let Readiness::Ready {
        dialer,
        magic_dns_suffix,
        peer_names,
    } = values.last().unwrap()
    else {
        panic!("expected Ready");
    };
    assert!(Arc::ptr_eq(
        dialer,
        f.node.dialer.lock().unwrap().as_ref().unwrap()
    ));
    assert_eq!(magic_dns_suffix, "ts.example.com");
    assert_eq!(peer_names, &["studio-mac", "office-win"]);
}

#[tokio::test(start_paused = true)]
async fn no_control_url_stays_system() {
    let f = Fixture::new(Inputs {
        control_url: None,
        ..embedded()
    });
    settle().await;
    assert!(f.node.calls().is_empty());
    assert_eq!(f.sink.routes(), [TailnetRoute::System]);
    assert!(!f.ctl.status().control_url_set);
    assert_eq!(f.sink.readiness(), ["Down"]);
}

#[tokio::test(start_paused = true)]
async fn background_grace_then_stop() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(29)).await;
    assert_eq!(f.node.count("stop"), 0);
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("stop"), 1);
    assert_eq!(f.sink.readiness().last(), Some(&"Idle"));
    assert_eq!(f.ctl.status().embedded, None);
    tick(Duration::from_secs(120)).await;
    assert_eq!(f.node.count("stop"), 1);
}

#[tokio::test(start_paused = true)]
async fn live_connection_keeps_node() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.live.0.store(true, Ordering::SeqCst);
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(120)).await;
    assert_eq!(f.node.count("stop"), 0);
    f.live.0.store(false, Ordering::SeqCst);
    tick(Duration::from_secs(5)).await;
    assert_eq!(f.node.count("stop"), 1);
    assert_eq!(f.sink.readiness().last(), Some(&"Idle"));
}

#[tokio::test(start_paused = true)]
async fn control_url_change_restarts() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    let before = f.node.calls().len();
    f.inputs.control_url = Some("https://hs2.example.com".to_owned());
    f.update().await;
    // A successful restart also schedules the required immediate snapshot.
    assert_eq!(
        &f.node.calls()[before..],
        [
            "stop",
            "start https://hs2.example.com rs-ios-k7m2q4xa",
            "snapshot",
        ]
    );
    assert_eq!(f.sink.readiness().last(), Some(&"Starting"));
    assert_eq!(f.ctl.status().embedded.unwrap().state, "starting");
    assert_eq!(f.sink.routes(), [TailnetRoute::Embedded]);
}

#[tokio::test(start_paused = true)]
async fn mode_to_system_stops_now() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.inputs.mode = TailnetMode::System;
    f.update().await;
    assert_eq!(f.node.count("stop"), 1);
    assert_eq!(
        f.sink.routes(),
        [TailnetRoute::Embedded, TailnetRoute::System]
    );
    assert_eq!(f.sink.readiness().last(), Some(&"Down"));
}

#[tokio::test(start_paused = true)]
async fn unsupported_marks_unavailable() {
    let node = FakeNode::new(true);
    node.start_results
        .lock()
        .unwrap()
        .push_back(Err(CoreError::unsupported("x")));
    let mut f = Fixture::with_node(embedded(), node, ControllerConfig::default());
    settle().await;
    assert!(!f.ctl.status().available);
    assert_eq!(f.ctl.status().active, TailnetRoute::System);
    assert_eq!(
        f.sink.routes(),
        [TailnetRoute::Embedded, TailnetRoute::System]
    );
    assert_eq!(f.sink.readiness(), ["Starting", "Down"]);
    f.inputs.app_active = false;
    f.update().await;
    f.inputs.app_active = true;
    f.inputs.control_url = Some("https://hs2.example.com".to_owned());
    f.update().await;
    tick(Duration::from_secs(120)).await;
    assert_eq!(f.node.calls(), [START]);
}

#[tokio::test(start_paused = true)]
async fn start_error_retries() {
    let node = FakeNode::new(true);
    node.start_results
        .lock()
        .unwrap()
        .push_back(Err(CoreError::network("x")));
    let mut f = Fixture::with_node(embedded(), node, ControllerConfig::default());
    settle().await;
    assert_eq!(f.sink.readiness(), ["Starting", "Down"]);
    assert_eq!(f.ctl.status().embedded, None);
    f.inputs.system_tailnet_up = true;
    f.update().await;
    tick(Duration::from_secs(29)).await;
    assert_eq!(f.node.count("start"), 1);
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.calls(), [START, START, "snapshot"]);
    assert_eq!(f.sink.readiness().last(), Some(&"Starting"));
}

#[tokio::test(start_paused = true)]
async fn auto_follows_system_tailnet() {
    let mut f = Fixture::new(Inputs {
        mode: TailnetMode::Auto,
        system_tailnet_up: true,
        ..embedded()
    });
    settle().await;
    assert!(f.node.calls().is_empty());
    assert_eq!(f.sink.routes(), [TailnetRoute::System]);
    f.inputs.system_tailnet_up = false;
    f.update().await;
    f.running().await;
    assert_eq!(f.sink.routes().last(), Some(&TailnetRoute::Embedded));
    f.live.0.store(true, Ordering::SeqCst);
    f.inputs.system_tailnet_up = true;
    f.update().await;
    assert_eq!(f.sink.routes().last(), Some(&TailnetRoute::System));
    assert_eq!(f.node.count("stop"), 0);
    f.live.0.store(false, Ordering::SeqCst);
    tick(Duration::from_secs(5)).await;
    assert_eq!(f.node.count("stop"), 1);
}

#[tokio::test(start_paused = true)]
async fn needs_login_reports_down() {
    let f = Fixture::new(embedded());
    settle().await;
    f.node.snap.lock().unwrap().state = "needsLogin".to_owned();
    f.changed().await;
    assert_eq!(f.sink.readiness().last(), Some(&"Down"));
    assert_eq!(f.ctl.status().embedded.unwrap().state, "needsLogin");
}

#[tokio::test(start_paused = true)]
async fn dedup_reports() {
    let f = Fixture::new(embedded());
    f.running().await;
    let before = f.sink.readiness();
    for _ in 0..3 {
        f.ctl.update(f.inputs.clone());
    }
    f.changed().await;
    assert_eq!(f.sink.routes().len(), 1);
    assert_eq!(f.sink.readiness(), before);
    let statuses = f.sink.statuses.lock().unwrap();
    assert!(statuses.windows(2).all(|pair| pair[0] != pair[1]));
}

#[tokio::test(start_paused = true)]
async fn shutdown_is_quiet() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.ctl.node_changed();
    let before = f.node.calls();
    let status = f.ctl.status();
    f.ctl.shutdown();
    f.inputs.mode = TailnetMode::System;
    f.update().await;
    f.ctl.node_changed();
    f.ctl.shutdown();
    tick(Duration::from_secs(120)).await;
    assert_eq!(f.node.calls(), before);
    assert_eq!(f.ctl.status(), status);
}

#[tokio::test(start_paused = true)]
async fn unavailable_build_never_starts() {
    let f = Fixture::with_node(
        embedded(),
        FakeNode::new(false),
        ControllerConfig::default(),
    );
    settle().await;
    assert!(f.node.calls().is_empty());
    assert!(!f.ctl.status().available);
    assert_eq!(f.sink.routes(), [TailnetRoute::System]);
    assert_eq!(f.sink.readiness(), ["Down"]);
}

#[tokio::test(start_paused = true)]
async fn snapshot_events_debounce_from_last_event() {
    let f = Fixture::new(embedded());
    settle().await;
    f.node.run(&["100.64.0.21"]);
    for _ in 0..3 {
        f.ctl.node_changed();
        settle().await;
        tick(Duration::from_millis(200)).await;
        assert_eq!(f.node.count("snapshot"), 1);
    }
    tick(Duration::from_millis(101)).await;
    assert_eq!(f.node.count("snapshot"), 2);
    assert_eq!(f.sink.readiness().last(), Some(&"Ready"));
}

#[tokio::test(start_paused = true)]
async fn status_cache_is_updated_before_callback() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.inputs.mode = TailnetMode::System;
    f.update().await;
    let statuses = f.sink.statuses.lock().unwrap();
    assert!(statuses.len() >= 3);
    assert_eq!(*statuses, *f.sink.cached_statuses.lock().unwrap());
    let first = statuses[0].embedded.as_ref().unwrap();
    assert_eq!(first.state, "starting");
    assert!(first.hostname.is_empty() && first.ips.is_empty());
}

#[tokio::test(start_paused = true)]
async fn ready_changes_include_dialer_suffix_and_peers() {
    let f = Fixture::new(embedded());
    f.running().await;
    let before = f.sink.readiness().len();
    f.changed().await;
    assert_eq!(f.sink.readiness().len(), before);
    *f.node.dialer.lock().unwrap() = Some(Arc::new(FakeDialer));
    f.changed().await;
    assert_eq!(f.sink.readiness().len(), before + 1);
    f.node.snap.lock().unwrap().magic_dns_suffix = "new.ts.example.com".to_owned();
    f.changed().await;
    assert_eq!(f.sink.readiness().len(), before + 2);
    f.node
        .snap
        .lock()
        .unwrap()
        .peer_names
        .push("home-mac".to_owned());
    f.changed().await;
    assert_eq!(f.sink.readiness().len(), before + 3);
    *f.node.dialer.lock().unwrap() = None;
    f.changed().await;
    assert_eq!(f.sink.readiness().last(), Some(&"Down"));
}

#[tokio::test(start_paused = true)]
async fn stop_error_still_clears_node_state() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.node.stop_error.store(true, Ordering::SeqCst);
    f.inputs.mode = TailnetMode::System;
    f.update().await;
    assert_eq!(f.node.count("stop"), 1);
    assert_eq!(f.ctl.status().embedded, None);
    assert_eq!(f.sink.readiness().last(), Some(&"Down"));
    tick(Duration::from_secs(120)).await;
    assert_eq!(f.node.count("stop"), 1);
}

#[tokio::test(start_paused = true)]
async fn snapshot_error_preserves_last_report_and_recovers() {
    let f = Fixture::new(embedded());
    f.running().await;
    let before = f.ctl.status();
    f.node.snapshot_error.store(true, Ordering::SeqCst);
    f.node.snap.lock().unwrap().state = "needsLogin".to_owned();
    f.changed().await;
    assert_eq!(f.ctl.status(), before);
    assert_eq!(f.sink.readiness().last(), Some(&"Ready"));
    f.node.snapshot_error.store(false, Ordering::SeqCst);
    f.changed().await;
    assert_eq!(f.sink.readiness().last(), Some(&"Down"));
}

#[tokio::test(start_paused = true)]
async fn foreground_cancels_grace_and_next_background_gets_full_grace() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(20)).await;
    f.inputs.app_active = true;
    f.update().await;
    tick(Duration::from_secs(20)).await;
    assert_eq!(f.node.count("stop"), 0);
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(29)).await;
    assert_eq!(f.node.count("stop"), 0);
    tick(Duration::from_secs(2)).await;
    assert_eq!(f.node.count("stop"), 1);
}

#[tokio::test(start_paused = true)]
async fn pending_retry_does_not_start_without_demand() {
    let node = FakeNode::new(true);
    node.start_results
        .lock()
        .unwrap()
        .push_back(Err(CoreError::network("x")));
    let mut f = Fixture::with_node(embedded(), node, ControllerConfig::default());
    settle().await;
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(60)).await;
    assert_eq!(f.node.count("start"), 1);
    f.inputs.app_active = true;
    f.update().await;
    assert_eq!(f.node.count("start"), 2);
}

#[tokio::test(start_paused = true)]
async fn custom_grace_and_live_recheck_are_used() {
    let cfg = ControllerConfig {
        background_grace: Duration::from_secs(2),
        live_recheck: Duration::from_secs(1),
        ..ControllerConfig::default()
    };
    let mut f = Fixture::with_node(embedded(), FakeNode::new(true), cfg);
    f.running().await;
    f.live.0.store(true, Ordering::SeqCst);
    f.inputs.app_active = false;
    f.update().await;
    tick(Duration::from_secs(3)).await;
    assert_eq!(f.node.count("stop"), 0);
    f.live.0.store(false, Ordering::SeqCst);
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("stop"), 1);
}

#[tokio::test(start_paused = true)]
async fn shutdown_cancels_pending_start() {
    let node = FakeNode::new(true);
    node.start_pending.store(true, Ordering::SeqCst);
    let f = Fixture::with_node(embedded(), node, ControllerConfig::default());
    settle().await;
    assert_eq!(f.node.calls(), [START]);
    assert_eq!(f.sink.readiness(), ["Starting"]);
    let weak_node = Arc::downgrade(&f.node);
    let ctl = f.ctl.clone();
    ctl.shutdown();
    drop(f);
    settle().await;
    assert!(weak_node.upgrade().is_none());
    ctl.update(embedded());
}

#[tokio::test(start_paused = true)]
async fn stopped_node_ignores_pending_snapshot() {
    let mut f = Fixture::new(embedded());
    f.running().await;
    let before = f.node.count("snapshot");
    f.ctl.node_changed();
    settle().await;
    f.inputs.mode = TailnetMode::System;
    f.update().await;
    tick(Duration::from_secs(1)).await;
    assert_eq!(f.node.count("snapshot"), before);
    assert_eq!(f.ctl.status().embedded, None);
}
