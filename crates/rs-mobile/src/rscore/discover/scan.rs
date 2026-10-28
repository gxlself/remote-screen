//! User-triggered LAN discovery and scan cancellation.

use std::collections::BTreeSet;
use std::net::{IpAddr, Ipv4Addr};
use std::sync::atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rsp_transport::dial::IpNet;
use tokio::sync::{Notify, OwnedSemaphorePermit, Semaphore};
use tokio::task::JoinSet;

use super::super::dto_core::{CoreEvent, DiscoverOutcome};
use super::super::error::CoreError;
use super::super::netinfo::{is_tailnet_ip, is_vpn, RawIface};
use super::super::runtime::Core;
use super::probe::probe;

const MAX_CONCURRENT_ADDRESSES: usize = 32;
const PROBE_TIMEOUT: Duration = Duration::from_millis(400);

/// A local address (or set of addresses) supplied to [`scan_targets`].
///
/// The generic argument keeps the pure helper convenient for callers that have
/// either a single address, an optional address, or a snapshot of addresses.
pub trait ScanMe {
    fn addresses(&self) -> Vec<Ipv4Addr>;
}

impl ScanMe for Ipv4Addr {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        vec![*self]
    }
}

impl ScanMe for &Ipv4Addr {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        vec![**self]
    }
}

impl ScanMe for IpAddr {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        match self {
            IpAddr::V4(address) => vec![*address],
            IpAddr::V6(_) => Vec::new(),
        }
    }
}

impl ScanMe for &IpAddr {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        match **self {
            IpAddr::V4(address) => vec![address],
            IpAddr::V6(_) => Vec::new(),
        }
    }
}

impl ScanMe for Option<Ipv4Addr> {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        self.iter().copied().collect()
    }
}

impl ScanMe for Option<&Ipv4Addr> {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        self.iter().map(|address| **address).collect()
    }
}

impl ScanMe for Option<IpAddr> {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        self.as_ref()
            .map_or_else(Vec::new, |address| address.addresses())
    }
}

impl ScanMe for Option<&IpAddr> {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        self.as_ref()
            .map_or_else(Vec::new, |address| match **address {
                IpAddr::V4(address) => vec![address],
                IpAddr::V6(_) => Vec::new(),
            })
    }
}

impl ScanMe for Vec<Ipv4Addr> {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        self.clone()
    }
}

impl ScanMe for &[Ipv4Addr] {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        self.to_vec()
    }
}

impl ScanMe for &[IpAddr] {
    fn addresses(&self) -> Vec<Ipv4Addr> {
        self.iter()
            .filter_map(|address| match address {
                IpAddr::V4(address) => Some(*address),
                IpAddr::V6(_) => None,
            })
            .collect()
    }
}

/// Return all usable IPv4 hosts on directly attached, non-VPN networks.
///
/// Interface addresses are retained in `RawIface`, so they are also used as
/// the local host address when a prefix is wider than `/24`. `me` adds an
/// explicit set of local addresses for callers whose interface snapshot does
/// not contain the complete local address list.
pub fn scan_targets<M: ScanMe>(ifaces: &[RawIface], vpn_pools: &[IpNet], me: M) -> Vec<Ipv4Addr> {
    let explicit_me = me.addresses();
    let local_addresses: BTreeSet<Ipv4Addr> = ifaces
        .iter()
        .flat_map(|iface| iface.addrs.iter())
        .filter_map(|addr| match addr.addr() {
            IpAddr::V4(address) => Some(address),
            IpAddr::V6(_) => None,
        })
        .chain(explicit_me.iter().copied())
        .collect();
    let mut targets = BTreeSet::new();

    for iface in ifaces {
        if is_vpn(iface, vpn_pools) {
            continue;
        }
        for net in &iface.addrs {
            let IpAddr::V4(address) = net.addr() else {
                continue;
            };
            if is_tailnet_ip(IpAddr::V4(address))
                || address.is_loopback()
                || address.is_link_local()
            {
                continue;
            }
            let prefix = net.prefix_len().min(32);
            let scan_prefix = if prefix < 24 { 24 } else { prefix };
            let mask = if scan_prefix == 0 {
                0
            } else {
                u32::MAX << (32 - scan_prefix)
            };
            let host = if prefix < 24 {
                if let [candidate] = explicit_me.as_slice() {
                    if net.contains(IpAddr::V4(*candidate)) {
                        *candidate
                    } else {
                        address
                    }
                } else {
                    address
                }
            } else {
                address
            };
            let network = u32::from(host) & mask;
            let broadcast = network | !mask;
            for value in network.saturating_add(1)..broadcast {
                let target = Ipv4Addr::from(value);
                if !local_addresses.contains(&target) {
                    targets.insert(target);
                }
            }
        }
    }
    targets.into_iter().collect()
}

/// Per-scan cancellation and completion state.
pub struct ScanControl {
    id: u64,
    cancelled: AtomicBool,
    finished: AtomicBool,
    scanned: AtomicU32,
    notify: Notify,
    /// Serializes the finished check with event emission. This keeps a found
    /// event from being queued after the cancellation done event.
    events: Mutex<()>,
}

impl ScanControl {
    fn new(id: u64) -> Self {
        Self {
            id,
            cancelled: AtomicBool::new(false),
            finished: AtomicBool::new(false),
            scanned: AtomicU32::new(0),
            notify: Notify::new(),
            events: Mutex::new(()),
        }
    }

    fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.notify.notify_waiters();
    }

    fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }

    async fn wait_cancelled(&self) {
        let notified = self.notify.notified();
        if self.is_cancelled() {
            return;
        }
        notified.await;
    }
}

#[derive(Default)]
pub struct ScanState {
    next_id: AtomicU64,
    active: Mutex<Option<Arc<ScanControl>>>,
}

/// Start a scan over an already computed target list.
pub async fn start(
    core: &Arc<Core>,
    ports: Vec<u16>,
    targets: Vec<Ipv4Addr>,
) -> Result<u64, CoreError> {
    if ports.is_empty() || ports.len() > 4 {
        return Err(CoreError::Internal {
            message: "bad ports".into(),
        });
    }
    let id = core
        .state::<ScanState>()
        .next_id
        .fetch_add(1, Ordering::Relaxed)
        .saturating_add(1);
    let control = Arc::new(ScanControl::new(id));
    let state = core.state::<ScanState>();
    let previous = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .replace(control.clone());
    if let Some(previous) = previous {
        previous.cancel();
        finish(
            core,
            &previous,
            previous.scanned.load(Ordering::Acquire),
            true,
        );
    }

    if targets.is_empty() {
        finish(core, &control, 0, false);
        return Ok(id);
    }

    let net = core.net();
    let core_for_task = Arc::clone(core);
    core.spawn(async move {
        run(&core_for_task, control, net, ports, targets).await;
    });
    Ok(id)
}

/// Cancel the active scan when its id matches. Unknown IDs are ignored.
pub fn cancel(core: &Arc<Core>, id: u64) {
    let control = core
        .state::<ScanState>()
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .as_ref()
        .filter(|control| control.id == id)
        .cloned();
    if let Some(control) = control {
        control.cancel();
        finish(
            core,
            &control,
            control.scanned.load(Ordering::Acquire),
            true,
        );
    }
}

async fn run(
    core: &Arc<Core>,
    control: Arc<ScanControl>,
    net: Arc<dyn rsp_transport::NetDialer>,
    ports: Vec<u16>,
    targets: Vec<Ipv4Addr>,
) {
    let semaphore = Arc::new(Semaphore::new(MAX_CONCURRENT_ADDRESSES));
    let mut jobs = JoinSet::new();
    for &address in &targets {
        let control = Arc::clone(&control);
        let semaphore = Arc::clone(&semaphore);
        let net = Arc::clone(&net);
        let core = Arc::clone(core);
        let ports = ports.clone();
        jobs.spawn(async move {
            let permit = acquire(&control, semaphore).await?;
            let mut completed = true;
            for port in ports {
                let kind = match port {
                    5900 => "vnc",
                    3389 => "rdp",
                    _ => "tcp",
                };
                let address = address.to_string();
                let result = tokio::select! {
                    _ = control.wait_cancelled() => {
                        completed = false;
                        break;
                    }
                    result = probe(
                        net.as_ref(),
                        &address,
                        port,
                        kind,
                        PROBE_TIMEOUT,
                    ) => result,
                };
                if control.is_cancelled() {
                    completed = false;
                    break;
                }
                if let Ok(result) = result {
                    if result.outcome == DiscoverOutcome::Open {
                        emit_found(&core, &control, &result.addr, result.port, result.banner);
                    }
                }
            }
            drop(permit);
            Some(completed)
        });
    }

    let mut scanned = 0_u32;
    while let Some(result) = jobs.join_next().await {
        if result.ok().flatten().unwrap_or(false) {
            scanned = scanned.saturating_add(1);
            control.scanned.store(scanned, Ordering::Release);
        }
    }
    finish(
        core,
        &control,
        if control.is_cancelled() {
            scanned
        } else {
            targets.len().min(u32::MAX as usize) as u32
        },
        control.is_cancelled(),
    );
}

async fn acquire(control: &ScanControl, semaphore: Arc<Semaphore>) -> Option<OwnedSemaphorePermit> {
    tokio::select! {
        _ = control.wait_cancelled() => None,
        permit = semaphore.acquire_owned() => permit.ok(),
    }
}

fn emit_found(
    core: &Arc<Core>,
    control: &ScanControl,
    addr: &str,
    port: u16,
    banner: Option<String>,
) {
    let _events = control
        .events
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if control.finished.load(Ordering::Acquire) {
        return;
    }
    core.events().emit(&CoreEvent::DiscoverFound {
        scan_id: control.id.to_string(),
        addr: addr.to_owned(),
        port,
        banner,
    });
}

fn finish(core: &Arc<Core>, control: &Arc<ScanControl>, scanned: u32, cancelled: bool) {
    let _events = control
        .events
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if control.finished.swap(true, Ordering::AcqRel) {
        return;
    }
    core.events().emit(&CoreEvent::DiscoverDone {
        scan_id: control.id.to_string(),
        scanned,
        cancelled,
    });
    let state = core.state::<ScanState>();
    let mut active = state
        .active
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if active
        .as_ref()
        .is_some_and(|current| Arc::ptr_eq(current, control))
    {
        active.take();
    }
}
