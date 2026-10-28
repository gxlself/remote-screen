//! Message-driven lifecycle and routing for the app's embedded tailnet node.

use std::{
    future::{pending, Future},
    pin::Pin,
    sync::{Arc, Mutex, MutexGuard},
    time::Duration,
};

use rsp_transport::NetDialer;
use tokio::{runtime::Handle, sync::mpsc, task::JoinHandle, time::Instant};

use super::{
    dto_core::{EmbeddedNodeSummary, TailnetRoute, TailnetRouteStatus},
    error::CoreError,
    tailnet_policy::{
        decide, phase_of, route_status, Decision, NodePhase, PolicyInput, StopWhen, TailnetMode,
        BACKGROUND_GRACE, LIVE_RECHECK, LOGIN_HOLD, START_RETRY, WAKE_WINDOW,
    },
    tailnet_split::Readiness,
};

pub type NodeFuture<'a, T> = Pin<Box<dyn Future<Output = Result<T, CoreError>> + Send + 'a>>;

/// Safe for network status reporting; deliberately excludes the login URL.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct NodeSnapshot {
    pub state: String,
    pub hostname: String,
    pub ips: Vec<String>,
    pub derp_region: String,
    pub derp_region_name: String,
    pub magic_dns_suffix: String,
    pub peer_names: Vec<String>,
}

pub trait EmbeddedNode: Send + Sync + 'static {
    fn available(&self) -> bool;
    fn start<'a>(&'a self, control_url: &'a str, hostname: &'a str) -> NodeFuture<'a, ()>;
    fn stop(&self) -> NodeFuture<'_, ()>;
    fn rebind(&self) -> NodeFuture<'_, ()>;
    fn snapshot(&self) -> NodeFuture<'_, NodeSnapshot>;
    /// Returns the node's dialer when it is Running.
    fn dialer(&self) -> Option<Arc<dyn NetDialer>>;
}

pub trait RouteSink: Send + Sync + 'static {
    fn route_changed(&self, route: TailnetRoute);
    fn readiness_changed(&self, readiness: Readiness);
    fn status_changed(&self, status: &TailnetRouteStatus);
}

pub trait LiveConnections: Send + Sync + 'static {
    fn any(&self) -> bool;
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Inputs {
    pub mode: TailnetMode,
    pub control_url: Option<String>,
    pub system_tailnet_up: bool,
    pub app_active: bool,
}

#[derive(Debug, Clone)]
pub struct ControllerConfig {
    pub background_grace: Duration,
    pub live_recheck: Duration,
    pub start_retry: Duration,
    pub snapshot_debounce: Duration,
    pub wake_window: Duration,
    pub login_hold: Duration,
    pub rebind_debounce: Duration,
}

impl Default for ControllerConfig {
    fn default() -> Self {
        Self {
            background_grace: BACKGROUND_GRACE,
            live_recheck: LIVE_RECHECK,
            start_retry: START_RETRY,
            snapshot_debounce: Duration::from_millis(300),
            wake_window: WAKE_WINDOW,
            login_hold: LOGIN_HOLD,
            rebind_debounce: Duration::from_secs(1),
        }
    }
}

enum Msg {
    Update(Inputs),
    NodeChanged,
    Wake,
    NetworkChanged,
    Shutdown,
}

pub struct TailnetController {
    tx: mpsc::UnboundedSender<Msg>,
    status: Arc<Mutex<TailnetRouteStatus>>,
    task: Mutex<Option<JoinHandle<()>>>,
}

impl TailnetController {
    pub fn spawn(
        handle: &Handle,
        node: Arc<dyn EmbeddedNode>,
        sink: Arc<dyn RouteSink>,
        live: Arc<dyn LiveConnections>,
        hostname: String,
        cfg: ControllerConfig,
        initial: Inputs,
    ) -> Arc<Self> {
        let available = node.available();
        let (tx, rx) = mpsc::unbounded_channel();
        let status = Arc::new(Mutex::new(route_status(
            initial.mode,
            TailnetRoute::System,
            available,
            initial.control_url.is_some(),
            None,
        )));
        let state = ControllerTask {
            node,
            sink,
            live,
            hostname,
            cfg,
            available,
            started: false,
            url_in_use: None,
            snapshot: None,
            background_since: (!initial.app_active).then(Instant::now),
            woken_until: None,
            needs_login_since: None,
            rebind_at: None,
            retry_at: None,
            snapshot_at: None,
            live_recheck_at: None,
            stopped_after_grace: false,
            last_route: None,
            last_readiness: None,
            last_status: None,
            status: status.clone(),
            inputs: initial,
        };
        Arc::new(Self {
            tx,
            status,
            task: Mutex::new(Some(handle.spawn(state.run(rx)))),
        })
    }

    pub fn update(&self, inputs: Inputs) {
        let _ = self.tx.send(Msg::Update(inputs));
    }

    pub fn node_changed(&self) {
        let _ = self.tx.send(Msg::NodeChanged);
    }

    /// Temporarily request the node for a dial while the app is in the background.
    /// Further requests within the current wake window do not extend it.
    pub fn wake(&self) {
        let _ = self.tx.send(Msg::Wake);
    }

    /// Debounce interface changes before refreshing a running node's bindings.
    pub fn network_changed(&self) {
        let _ = self.tx.send(Msg::NetworkChanged);
    }

    pub fn status(&self) -> TailnetRouteStatus {
        lock(&self.status).clone()
    }

    /// End controller work, including an in-flight node operation. The node's
    /// shutdown hook owns stopping the node itself.
    pub fn shutdown(&self) {
        let _ = self.tx.send(Msg::Shutdown);
        if let Some(task) = lock(&self.task).take() {
            task.abort();
        }
    }
}

impl Drop for TailnetController {
    fn drop(&mut self) {
        self.shutdown();
    }
}

struct ControllerTask {
    node: Arc<dyn EmbeddedNode>,
    sink: Arc<dyn RouteSink>,
    live: Arc<dyn LiveConnections>,
    hostname: String,
    cfg: ControllerConfig,
    inputs: Inputs,
    available: bool,
    started: bool,
    url_in_use: Option<String>,
    snapshot: Option<NodeSnapshot>,
    background_since: Option<Instant>,
    woken_until: Option<Instant>,
    needs_login_since: Option<Instant>,
    rebind_at: Option<Instant>,
    retry_at: Option<Instant>,
    snapshot_at: Option<Instant>,
    live_recheck_at: Option<Instant>,
    stopped_after_grace: bool,
    last_route: Option<TailnetRoute>,
    last_readiness: Option<Readiness>,
    last_status: Option<TailnetRouteStatus>,
    status: Arc<Mutex<TailnetRouteStatus>>,
}

impl ControllerTask {
    async fn run(mut self, mut rx: mpsc::UnboundedReceiver<Msg>) {
        let mut deadline = self.reconcile().await;
        loop {
            tokio::select! {
                msg = rx.recv() => match msg {
                    Some(Msg::Update(inputs)) => {
                        if inputs == self.inputs {
                            continue;
                        }
                        if inputs.app_active != self.inputs.app_active {
                            let now = Instant::now();
                            self.background_since = (!inputs.app_active).then_some(now);
                            if inputs.app_active && self.started {
                                self.rebind_at = Some(now + self.cfg.rebind_debounce);
                            }
                        }
                        self.inputs = inputs;
                    }
                    Some(Msg::NodeChanged) => {
                        self.snapshot_at = Some(Instant::now() + self.cfg.snapshot_debounce);
                    }
                    Some(Msg::Wake) => {
                        let now = Instant::now();
                        if self.woken_until.is_none_or(|until| now >= until) {
                            self.woken_until = Some(now + self.cfg.wake_window);
                        }
                    }
                    Some(Msg::NetworkChanged) => {
                        if self.started && self.phase() == NodePhase::Running {
                            self.rebind_at = Some(Instant::now() + self.cfg.rebind_debounce);
                        }
                    }
                    Some(Msg::Shutdown) | None => break,
                },
                () = wait_until(deadline) => {}
            }
            deadline = self.reconcile().await;
        }
    }

    fn decision(&self, live_connections: bool, now: Instant) -> Decision {
        decide(&PolicyInput {
            mode: self.inputs.mode,
            available: self.available,
            control_url_set: self.inputs.control_url.is_some(),
            system_tailnet_up: self.inputs.system_tailnet_up,
            app_active: self.inputs.app_active,
            woken: self.woken_until.is_some_and(|until| now < until),
            login_pending: self.phase() == NodePhase::NeedsLogin
                && self
                    .needs_login_since
                    .is_some_and(|since| now < since + self.cfg.login_hold),
            live_connections,
            node_started: self.started,
        })
    }

    async fn reconcile(&mut self) -> Option<Instant> {
        if self.rebind_at.is_some_and(|at| at <= Instant::now()) {
            self.rebind_at = None;
            if self.started {
                if let Err(error) = self.node.rebind().await {
                    tracing::warn!(kind = error_kind(&error), "tailnet rebind failed");
                }
                self.snapshot_at = Some(Instant::now());
            }
        }
        let now = Instant::now();
        if self.snapshot_at.is_some_and(|at| at <= now) {
            self.snapshot_at = None;
            if self.started {
                match self.node.snapshot().await {
                    Ok(snapshot) => {
                        if phase_of(&snapshot.state) == NodePhase::NeedsLogin {
                            self.needs_login_since.get_or_insert_with(Instant::now);
                        } else {
                            self.needs_login_since = None;
                        }
                        self.snapshot = Some(snapshot);
                    }
                    Err(error) => {
                        tracing::warn!(kind = error_kind(&error), "tailnet snapshot failed");
                    }
                }
            }
        }
        if self.retry_at.is_some_and(|at| at <= Instant::now()) {
            self.retry_at = None;
        }

        let now = Instant::now();
        let live_connections = self.live.any();
        let decision = self.decision(live_connections, now);
        self.report_route(decision.route);
        let mut stop_at = None;
        if decision.want_node {
            if self.started && self.url_in_use != self.inputs.control_url {
                self.stop(StopWhen::Now).await;
            }
            if !self.started && self.available && self.retry_at.is_none() {
                if let Some(url) = self.inputs.control_url.clone() {
                    self.report_readiness(Readiness::Starting);
                    self.stopped_after_grace = false;
                    match self.node.start(&url, &self.hostname).await {
                        Ok(()) => {
                            self.started = true;
                            self.url_in_use = Some(url);
                            self.snapshot_at = Some(Instant::now());
                        }
                        Err(CoreError::Unsupported { .. }) => self.available = false,
                        Err(error) => {
                            self.retry_at = Some(Instant::now() + self.cfg.start_retry);
                            tracing::warn!(kind = error_kind(&error), "tailnet start failed");
                        }
                    }
                }
            }
        } else if self.started {
            match decision.stop {
                StopWhen::Now => self.stop(StopWhen::Now).await,
                StopWhen::AfterGrace => {
                    let since = *self.background_since.get_or_insert_with(Instant::now);
                    let at = since + self.cfg.background_grace;
                    if Instant::now() >= at {
                        self.stop(StopWhen::AfterGrace).await;
                    } else {
                        stop_at = Some(at);
                    }
                }
                StopWhen::Never => {}
            }
        }

        // Unsupported changes routing immediately, without waiting for input.
        let route = self.decision(live_connections, Instant::now()).route;
        self.report_route(route);
        self.report_readiness(self.readiness());
        self.report_status(route);

        // System routing can also retain a node for existing connections while
        // foregrounded. Poll those connections so their closure releases it.
        if self.started
            && live_connections
            && (!self.inputs.app_active || route == TailnetRoute::System)
        {
            let now = Instant::now();
            if self.live_recheck_at.is_none_or(|at| at <= now) {
                self.live_recheck_at = Some(now + self.cfg.live_recheck);
            }
        } else {
            self.live_recheck_at = None;
        }
        [
            stop_at,
            self.retry_at,
            self.snapshot_at,
            self.live_recheck_at,
            self.rebind_at,
            self.woken_until.filter(|until| now < *until),
            self.needs_login_since
                .map(|since| since + self.cfg.login_hold)
                .filter(|until| now < *until),
        ]
        .into_iter()
        .flatten()
        .min()
    }

    async fn stop(&mut self, reason: StopWhen) {
        if let Err(error) = self.node.stop().await {
            tracing::warn!(kind = error_kind(&error), "tailnet stop failed");
        }
        self.started = false;
        self.url_in_use = None;
        self.snapshot = None;
        self.needs_login_since = None;
        self.rebind_at = None;
        self.snapshot_at = None;
        self.retry_at = None;
        self.stopped_after_grace = reason == StopWhen::AfterGrace;
    }

    fn readiness(&self) -> Readiness {
        if !self.available || self.inputs.control_url.is_none() {
            return Readiness::Down;
        }
        if self.started {
            match self.phase() {
                NodePhase::Running => {
                    if let (Some(snapshot), Some(dialer)) = (&self.snapshot, self.node.dialer()) {
                        return Readiness::Ready {
                            dialer,
                            magic_dns_suffix: snapshot.magic_dns_suffix.clone(),
                            peer_names: snapshot.peer_names.clone(),
                        };
                    }
                }
                NodePhase::Starting => return Readiness::Starting,
                NodePhase::NeedsLogin | NodePhase::Down => {}
            }
        } else if self.stopped_after_grace {
            return Readiness::Idle;
        }
        Readiness::Down
    }

    fn phase(&self) -> NodePhase {
        self.snapshot
            .as_ref()
            .map_or(NodePhase::Starting, |snapshot| phase_of(&snapshot.state))
    }

    fn report_route(&mut self, route: TailnetRoute) {
        if self.last_route != Some(route) {
            self.last_route = Some(route);
            self.sink.route_changed(route);
        }
    }

    fn report_readiness(&mut self, readiness: Readiness) {
        if !self
            .last_readiness
            .as_ref()
            .is_some_and(|last| same_readiness(last, &readiness))
        {
            self.last_readiness = Some(readiness.clone());
            self.sink.readiness_changed(readiness);
        }
    }

    fn report_status(&mut self, route: TailnetRoute) {
        let embedded = self.started.then(|| match &self.snapshot {
            Some(snapshot) => EmbeddedNodeSummary {
                state: snapshot.state.clone(),
                hostname: snapshot.hostname.clone(),
                ips: snapshot.ips.clone(),
                derp_region: snapshot.derp_region.clone(),
                derp_region_name: snapshot.derp_region_name.clone(),
            },
            None => EmbeddedNodeSummary {
                state: "starting".to_owned(),
                hostname: String::new(),
                ips: Vec::new(),
                derp_region: String::new(),
                derp_region_name: String::new(),
            },
        });
        let status = route_status(
            self.inputs.mode,
            route,
            self.available,
            self.inputs.control_url.is_some(),
            embedded,
        );
        if self.last_status.as_ref() != Some(&status) {
            // Release the lock before calling out: sinks may read status().
            *lock(&self.status) = status.clone();
            self.last_status = Some(status.clone());
            self.sink.status_changed(&status);
        }
    }
}

fn same_readiness(left: &Readiness, right: &Readiness) -> bool {
    match (left, right) {
        (Readiness::Down, Readiness::Down)
        | (Readiness::Idle, Readiness::Idle)
        | (Readiness::Starting, Readiness::Starting) => true,
        (
            Readiness::Ready {
                dialer: left_dialer,
                magic_dns_suffix: left_suffix,
                peer_names: left_peers,
            },
            Readiness::Ready {
                dialer: right_dialer,
                magic_dns_suffix: right_suffix,
                peer_names: right_peers,
            },
        ) => {
            Arc::ptr_eq(left_dialer, right_dialer)
                && left_suffix == right_suffix
                && left_peers == right_peers
        }
        _ => false,
    }
}

async fn wait_until(deadline: Option<Instant>) {
    match deadline {
        Some(at) => tokio::time::sleep_until(at).await,
        None => pending().await,
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

// Error messages can contain URLs or credentials supplied by the node backend.
fn error_kind(error: &CoreError) -> &'static str {
    match error {
        CoreError::Network { .. } => "network",
        CoreError::Http { .. } => "http",
        CoreError::Conflict { .. } => "conflict",
        CoreError::Unsupported { .. } => "unsupported",
        CoreError::Cancelled { .. } => "cancelled",
        CoreError::Internal { .. } => "internal",
    }
}
