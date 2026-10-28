//! Connect Core settings, network observations and node events to tailnet policy.

use std::{
    collections::BTreeSet,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, Weak,
    },
    time::Duration,
};

use rsp_transport::NetDialer;
use serde_json::Value;

#[cfg(unix)]
use super::{
    api_diag,
    dto_core::TailnetRoute,
    netinfo::read_interfaces,
    tables::{SessionTable, SshTable},
    tailnet_ctl::{LiveConnections, RouteSink},
    tailnet_policy::node_hostname,
    tailnet_split::Readiness,
};
use super::{
    api_tailnet,
    dto_core::TailnetRouteStatus,
    error::CoreError,
    netinfo::{has_tailnet, RawIface},
    runtime::Core,
    settings::SettingsState,
    tailnet_ctl::{
        ControllerConfig, EmbeddedNode, Inputs, NodeFuture, NodeSnapshot, TailnetController,
    },
    tailnet_policy::TailnetMode,
    tailnet_split::{SplitNetDialer, DEFAULT_WAIT},
};

#[derive(Clone)]
pub struct WireOptions {
    pub controller: ControllerConfig,
    pub split_wait: Duration,
    pub disable_netns: bool,
}

impl Default for WireOptions {
    fn default() -> Self {
        Self {
            controller: ControllerConfig::default(),
            split_wait: DEFAULT_WAIT,
            disable_netns: false,
        }
    }
}

/// Installed lazily in `core.state::<TailnetWire>()`.
#[derive(Default)]
pub struct TailnetWire {
    wired: Mutex<Option<Wired>>,
}

pub struct Wired {
    pub ctl: Arc<TailnetController>,
    pub split: Arc<SplitNetDialer>,
    pub base_net: Arc<dyn NetDialer>,
    pub system_tailnet_up: AtomicBool,
}

pub fn install(core: &Arc<Core>) -> Result<(), CoreError> {
    let opts = WireOptions::default();
    install_with(core, real_node(core, opts.disable_netns), opts)
}

#[cfg(unix)]
pub fn install_with(
    core: &Arc<Core>,
    node: Arc<dyn EmbeddedNode>,
    opts: WireOptions,
) -> Result<(), CoreError> {
    if installed(core) {
        return Ok(());
    }
    let raw = read_interfaces().ok();
    // Seed the comparison before installing, so the first controller status is
    // observable even when network.status has never been called. No wire lock
    // may be held here: refresh_network calls back into on_network.
    if let Some(raw) = &raw {
        api_diag::refresh_network(core, raw);
    }
    let state = core.state::<TailnetWire>();
    let mut wired = lock(&state.wired);
    if wired.is_some() {
        return Ok(());
    }
    let system_tailnet_up = raw.as_deref().is_some_and(has_tailnet);
    let base_net = core.net();
    let split = SplitNetDialer::new(base_net.clone(), opts.split_wait);
    let sink = Arc::new(CoreSink {
        core: Arc::downgrade(core),
        split: split.clone(),
        base_net: base_net.clone(),
    });
    let live = Arc::new(CoreLiveConnections {
        core: Arc::downgrade(core),
    });
    let hostname = node_hostname(core.config().platform, core.device_id().ok().as_deref());
    // Read settings without reacquiring wired: the initial interface value has
    // not been published yet. Spawned callbacks wait until Wired is installed.
    let ctl = TailnetController::spawn(
        &core.handle(),
        node,
        sink,
        live,
        hostname,
        opts.controller,
        settings_inputs(core, system_tailnet_up),
    );
    let weak_ctl = Arc::downgrade(&ctl);
    split.set_wake(Arc::new(move || {
        if let Some(ctl) = weak_ctl.upgrade() {
            ctl.wake();
        }
    }));
    let weak_ctl = Arc::downgrade(&ctl);
    core.add_shutdown_hook(Box::new(move || {
        if let Some(ctl) = weak_ctl.upgrade() {
            ctl.shutdown();
        }
    }));
    *wired = Some(Wired {
        ctl,
        split,
        base_net,
        system_tailnet_up: AtomicBool::new(system_tailnet_up),
    });
    Ok(())
}

#[cfg(not(unix))]
pub fn install_with(
    _core: &Arc<Core>,
    _node: Arc<dyn EmbeddedNode>,
    _opts: WireOptions,
) -> Result<(), CoreError> {
    Err(CoreError::unsupported(
        "embedded tailnet is not available on this platform",
    ))
}

pub fn installed(core: &Arc<Core>) -> bool {
    lock(&core.state::<TailnetWire>().wired).is_some()
}

pub fn route_status(core: &Arc<Core>) -> Option<TailnetRouteStatus> {
    lock(&core.state::<TailnetWire>().wired)
        .as_ref()
        .map(|wired| wired.ctl.status())
}

pub fn on_setting(core: &Arc<Core>, key: &str) -> Result<(), CoreError> {
    if !matches!(
        key,
        "tailnet.mode" | "tailnet.controlUrl" | "device.appActive"
    ) {
        return Ok(());
    }
    install(core)?;
    let ctl = lock(&core.state::<TailnetWire>().wired)
        .as_ref()
        .map(|wired| wired.ctl.clone());
    if let Some(ctl) = ctl {
        ctl.update(inputs_from(core));
    }
    Ok(())
}

pub fn on_network(core: &Arc<Core>, raw: &[RawIface], ifaces_changed: bool) {
    let ctl = lock(&core.state::<TailnetWire>().wired)
        .as_ref()
        .map(|wired| {
            wired
                .system_tailnet_up
                .store(has_tailnet(raw), Ordering::Release);
            wired.ctl.clone()
        });
    if let Some(ctl) = ctl {
        ctl.update(inputs_from(core));
        if ifaces_changed {
            ctl.network_changed();
        }
    }
}

pub fn node_changed(core: &Arc<Core>) {
    if let Some(wired) = lock(&core.state::<TailnetWire>().wired).as_ref() {
        wired.ctl.node_changed();
    }
}

pub fn inputs_from(core: &Arc<Core>) -> Inputs {
    let system_tailnet_up = lock(&core.state::<TailnetWire>().wired)
        .as_ref()
        .is_some_and(|wired| wired.system_tailnet_up.load(Ordering::Acquire));
    settings_inputs(core, system_tailnet_up)
}

fn settings_inputs(core: &Arc<Core>, system_tailnet_up: bool) -> Inputs {
    let state = core.state::<SettingsState>();
    let settings = lock(&state.0);
    Inputs {
        mode: TailnetMode::parse(&settings.tailnet_mode).unwrap_or(TailnetMode::System),
        control_url: settings.tailnet_control_url.clone(),
        app_active: settings.app_active,
        system_tailnet_up,
    }
}

pub struct CoreNode {
    pub core: Weak<Core>,
    pub disable_netns: bool,
}

pub fn real_node(core: &Arc<Core>, disable_netns: bool) -> Arc<dyn EmbeddedNode> {
    Arc::new(CoreNode {
        core: Arc::downgrade(core),
        disable_netns,
    })
}

impl CoreNode {
    fn core(&self) -> Result<Arc<Core>, CoreError> {
        self.core
            .upgrade()
            .ok_or_else(|| CoreError::internal("core dropped"))
    }
}

impl EmbeddedNode for CoreNode {
    fn available(&self) -> bool {
        api_tailnet::available()
    }

    fn start<'a>(&'a self, control_url: &'a str, hostname: &'a str) -> NodeFuture<'a, ()> {
        Box::pin(async move {
            api_tailnet::start(
                &self.core()?,
                api_tailnet::TailnetStartOptions {
                    control_url: control_url.to_owned(),
                    hostname: hostname.to_owned(),
                    auth_key: None,
                    disable_netns: self.disable_netns,
                },
            )
            .await
        })
    }

    fn stop(&self) -> NodeFuture<'_, ()> {
        Box::pin(async move { api_tailnet::stop(&self.core()?).await })
    }

    fn rebind(&self) -> NodeFuture<'_, ()> {
        Box::pin(async move { api_tailnet::rebind(&self.core()?).await })
    }

    fn snapshot(&self) -> NodeFuture<'_, NodeSnapshot> {
        Box::pin(async move {
            let value: Value =
                serde_json::from_str(&api_tailnet::status_json(&self.core()?).await?)?;
            Ok(snapshot_from(&value))
        })
    }

    fn dialer(&self) -> Option<Arc<dyn NetDialer>> {
        self.core
            .upgrade()
            .and_then(|core| api_tailnet::net_dialer(&core))
    }
}

fn snapshot_from(value: &Value) -> NodeSnapshot {
    let string = |value: &Value| value.as_str().unwrap_or_default().to_owned();
    let node = &value["self"];
    let mut peer_names = BTreeSet::new();
    for peer in value["peers"].as_array().into_iter().flatten() {
        for name in [
            peer["hostname"].as_str(),
            peer["dnsName"]
                .as_str()
                .and_then(|name| name.split('.').next()),
        ]
        .into_iter()
        .flatten()
        .filter(|name| !name.is_empty())
        {
            peer_names.insert(name.to_lowercase());
        }
    }
    NodeSnapshot {
        state: string(&value["state"]),
        hostname: string(&node["hostname"]),
        ips: node["ips"]
            .as_array()
            .into_iter()
            .flatten()
            .filter_map(Value::as_str)
            .map(str::to_owned)
            .collect(),
        derp_region: string(&node["derpRegion"]),
        derp_region_name: string(&node["derpRegionName"]),
        magic_dns_suffix: string(&value["magicDnsSuffix"]),
        peer_names: peer_names.into_iter().collect(),
    }
}

#[cfg(unix)]
pub struct CoreSink {
    pub core: Weak<Core>,
    pub split: Arc<SplitNetDialer>,
    pub base_net: Arc<dyn NetDialer>,
}

#[cfg(unix)]
impl RouteSink for CoreSink {
    fn route_changed(&self, route: TailnetRoute) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        match route {
            TailnetRoute::Embedded => core.set_net(self.split.clone()),
            TailnetRoute::System => core.set_net(self.base_net.clone()),
        }
    }

    fn readiness_changed(&self, readiness: Readiness) {
        self.split.set_readiness(readiness);
    }

    fn status_changed(&self, _status: &TailnetRouteStatus) {
        let Some(core) = self.core.upgrade() else {
            return;
        };
        if let Ok(raw) = read_interfaces() {
            api_diag::refresh_network(&core, &raw);
        }
    }
}

#[cfg(unix)]
pub struct CoreLiveConnections {
    pub core: Weak<Core>,
}

#[cfg(unix)]
impl LiveConnections for CoreLiveConnections {
    fn any(&self) -> bool {
        self.core.upgrade().is_some_and(|core| {
            !core.state::<SessionTable>().is_empty() || !core.state::<SshTable>().is_empty()
        })
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[cfg(test)]
mod tests {
    use super::snapshot_from;
    use serde_json::json;

    #[test]
    fn snapshot_extracts_peer_aliases_without_login_url() {
        let snapshot = snapshot_from(&json!({
            "state": "running",
            "authUrl": "https://hs.example.com/auth/private-token",
            "self": {
                "hostname": "rs-ios-test", "ips": ["100.64.0.21"],
                "derpRegion": "test", "derpRegionName": "Local test"
            },
            "magicDnsSuffix": "ts.example.com",
            "peers": [
                {"hostname": "STUDIO-MAC", "dnsName": "studio-mac.ts.example.com."},
                {"hostname": "Office", "dnsName": "ALIAS.ts.example.com"},
                {"hostname": "office", "dnsName": ""},
                {}
            ]
        }));
        assert_eq!(snapshot.state, "running");
        assert_eq!(snapshot.hostname, "rs-ios-test");
        assert_eq!(snapshot.ips, ["100.64.0.21"]);
        assert_eq!(snapshot.derp_region, "test");
        assert_eq!(snapshot.derp_region_name, "Local test");
        assert_eq!(snapshot.magic_dns_suffix, "ts.example.com");
        assert_eq!(snapshot.peer_names, ["alias", "office", "studio-mac"]);
        assert!(!format!("{snapshot:?}").contains("private-token"));
    }

    #[test]
    fn snapshot_accepts_null_self_while_waiting_for_login() {
        let snapshot = snapshot_from(&json!({"state":"needsLogin", "self":null, "peers":[]}));
        assert_eq!(snapshot.state, "needsLogin");
        assert!(snapshot.hostname.is_empty());
        assert!(snapshot.ips.is_empty());
        assert!(snapshot.derp_region.is_empty());
        assert!(snapshot.derp_region_name.is_empty());
        assert!(snapshot.magic_dns_suffix.is_empty());
        assert!(snapshot.peer_names.is_empty());
    }
}
