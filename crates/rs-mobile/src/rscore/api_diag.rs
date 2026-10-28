//! Per-address diagnostics, network snapshots, and native settings exports.

use std::net::{IpAddr, Ipv4Addr, Ipv6Addr, SocketAddr};
use std::sync::atomic::{AtomicBool, AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use std::time::{Duration, Instant};

use rsp_core::ids::id_from_fp;
use rsp_transport::dial::{
    plan, race, AttemptLog, AttemptResult, Candidate, DefaultDialer, DialOpts, DialPlan, IpNet,
    LocalNet, Skipped,
};
use rsp_transport::{CloseCode, TransportKind};

use super::api_sessions::sync_low_power;
use super::candidates::{manual_candidate, TargetError};
use super::dto_core::{
    id_str, net_path_from_dial, CoreEvent, NetPath, NetworkStatus, ProbeOutcome, Transport,
};
use super::error::CoreError;
use super::manual_addrs::host_target_with_manual;
use super::netinfo::{
    local_net_excluding, network_status_marked as snapshot_status, read_interfaces, RawIface,
};
use super::runtime::{core, Core};
use super::settings::{save_sync_servers, SettingsError, SettingsState, SYNC_SERVERS};

#[derive(Default)]
pub struct DiagState {
    pub next_id: AtomicU64,
}

impl DiagState {
    pub fn allocate(&self) -> Result<u64, CoreError> {
        self.next_id
            .fetch_update(Ordering::Relaxed, Ordering::Relaxed, |id| id.checked_add(1))
            .map(|id| id + 1)
            .map_err(|_| CoreError::internal("probe ids exhausted"))
    }
}

/// Counting and enqueueing share a lock, so `last` describes delivery order.
pub struct ProbeBatch {
    pub id: u64,
    pub remaining: Mutex<usize>,
}

impl ProbeBatch {
    pub fn emit(&self, core: &Arc<Core>, mut event: CoreEvent) {
        let mut remaining = self.remaining.lock().unwrap_or_else(|e| e.into_inner());
        *remaining -= 1;
        if let CoreEvent::ProbeResult { probe_id, last, .. } = &mut event {
            *probe_id = id_str(self.id);
            *last = *remaining == 0;
        }
        core.events().emit(&event);
    }
}

pub async fn probe(core: &Arc<Core>, host_id: &str) -> Result<u64, CoreError> {
    let raw = read_interfaces()?;
    let (pools, work_profile_ifaces) = {
        let state = core.state::<SettingsState>();
        let settings = state.0.lock().unwrap_or_else(|e| e.into_inner());
        (settings.vpn_pools(), settings.work_profile_ifaces.clone())
    };
    probe_with_net(
        core,
        host_id,
        local_net_excluding(&raw, &pools, &work_profile_ifaces),
    )
    .await
}

pub async fn probe_with_net(
    core: &Arc<Core>,
    host_id: &str,
    local: LocalNet,
) -> Result<u64, CoreError> {
    let (paired, last_good) = {
        let store = core.store();
        (store.paired_host(host_id)?, store.last_good(host_id)?)
    };
    let paired = paired.ok_or_else(|| CoreError::internal("unknown host"))?;
    // Validate even a host with no usable addresses before allocating its ID.
    core.local_identity()?;
    let cidrs = core
        .state::<SettingsState>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .intranet_cidrs();
    match host_target_with_manual(
        core,
        host_id,
        Some(&paired),
        last_good.as_ref(),
        &local,
        &cidrs,
    ) {
        Ok(target) => start_probe(
            core,
            target.candidates,
            target.fp,
            Some(host_id.to_owned()),
            Some((local, target.intranet_cidrs)),
        ),
        Err(TargetError::NoAddress) => {
            let id = core.state::<DiagState>().allocate()?;
            core.events().emit(&CoreEvent::ProbeResult {
                probe_id: id_str(id),
                host_id: Some(host_id.to_owned()),
                path: NetPath::Manual,
                addr: String::new(),
                port: 0,
                transport: Transport::Quic,
                outcome: ProbeOutcome::NoRoute,
                ms: None,
                reason: None,
                last: true,
            });
            Ok(id)
        }
        Err(error) => Err(CoreError::internal(error)),
    }
}

/// The pin for a Host whose fingerprint is not known yet. TLS accepts any
/// certificate and race rejects this impossible pin only after the handshake,
/// so an RSP peer's rejection carries the SPKI fingerprint it proved it holds.
const UNPINNED: [u8; 32] = [0; 32];

async fn address_candidates(
    core: &Arc<Core>,
    addr: &str,
    port: Option<u16>,
) -> Result<Vec<Candidate>, CoreError> {
    let addr = addr.trim();
    let port = port.unwrap_or(rsp_core::DEFAULT_PORT);
    let literal = addr
        .strip_prefix('[')
        .and_then(|value| value.strip_suffix(']'))
        .unwrap_or(addr);
    let addresses = if let Ok(ip) = literal.parse::<IpAddr>() {
        vec![SocketAddr::new(ip, port)]
    } else {
        let resolved =
            tokio::time::timeout(core.timeouts().probe, tokio::net::lookup_host((addr, port)))
                .await
                .map_err(|_| CoreError::internal("bad address"))?
                .map_err(|_| CoreError::internal("bad address"))?;
        let mut addresses = Vec::new();
        for address in resolved {
            if !addresses.contains(&address) {
                addresses.push(address);
            }
            if addresses.len() == 4 {
                break;
            }
        }
        addresses
    };
    if addresses.is_empty() {
        return Err(CoreError::internal("bad address"));
    }
    Ok(addresses.into_iter().map(manual_candidate).collect())
}

pub async fn probe_address(
    core: &Arc<Core>,
    addr: &str,
    port: Option<u16>,
) -> Result<u64, CoreError> {
    let candidates = address_candidates(core, addr, port).await?;
    start_probe(core, candidates, UNPINNED, None, None)
}

/// Dial a typed address as probe_address does, returning the first Host
/// fingerprint presented and every candidate that presented it. `None` means
/// nothing there completed an RSP TLS handshake.
pub async fn discover_address(
    core: &Arc<Core>,
    addr: &str,
    port: Option<u16>,
) -> Result<Option<(Vec<Candidate>, [u8; 32])>, CoreError> {
    let Ok(candidates) = address_candidates(core, addr, port).await else {
        return Ok(None);
    };
    let dialer = probe_dialer(core, &candidates)?;
    let plan = DialPlan {
        candidates,
        ..DialPlan::default()
    };
    let opts = DialOpts {
        timeout: core.timeouts().probe,
        ..DialOpts::default()
    };
    let attempts = match race(&plan, dialer.as_ref(), UNPINNED, &opts).await {
        Ok(result) => {
            result.primary.close(CloseCode::Normal, b"probe");
            if let Some((backup, _)) = &result.backup {
                backup.close(CloseCode::Normal, b"probe");
            }
            result.attempts
        }
        Err(failure) => failure.attempts,
    };
    let presented = |log: &AttemptLog| match log.result {
        AttemptResult::FingerprintMismatch { got } => Some(got),
        _ => None,
    };
    let Some(fp) = attempts.iter().find_map(presented) else {
        return Ok(None);
    };
    let mut found: Vec<Candidate> = Vec::new();
    for log in &attempts {
        if presented(log) == Some(fp) && !found.iter().any(|c| c.addr == log.candidate.addr) {
            found.push(log.candidate.clone());
        }
    }
    Ok(Some((found, fp)))
}

fn probe_dialer(
    core: &Arc<Core>,
    candidates: &[Candidate],
) -> Result<Arc<DefaultDialer>, CoreError> {
    let identity = core.local_identity()?;
    let bind = if candidates.iter().any(|c| c.addr.is_ipv6()) {
        SocketAddr::from((Ipv6Addr::UNSPECIFIED, 0))
    } else {
        SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0))
    };
    let net = core.net();
    let endpoint = net.shared_quic_endpoint(bind)?;
    Ok(Arc::new(
        DefaultDialer::with_net(net, endpoint, &identity).map_err(CoreError::internal)?,
    ))
}

pub fn start_probe(
    core: &Arc<Core>,
    candidates: Vec<Candidate>,
    expected_fp: [u8; 32],
    host_id: Option<String>,
    planning: Option<(LocalNet, Vec<IpNet>)>,
) -> Result<u64, CoreError> {
    let dialer = probe_dialer(core, &candidates)?;
    let id = core.state::<DiagState>().allocate()?;
    let c = core.clone();
    core.spawn(async move {
        let plan = match planning {
            Some((local, cidrs)) => plan(candidates, &local, &cidrs),
            None => DialPlan {
                candidates,
                ..DialPlan::default()
            },
        };
        dispatch_probe(&c, plan, id, dialer, expected_fp, host_id);
    });
    Ok(id)
}

pub fn dispatch_probe(
    core: &Arc<Core>,
    plan: DialPlan,
    id: u64,
    dialer: Arc<DefaultDialer>,
    expected_fp: [u8; 32],
    host_id: Option<String>,
) {
    let batch = Arc::new(ProbeBatch {
        id,
        remaining: Mutex::new(plan.candidates.len() + plan.skipped.len()),
    });
    for (candidate, skipped) in plan.skipped {
        batch.emit(
            core,
            CoreEvent::ProbeResult {
                probe_id: String::new(),
                host_id: host_id.clone(),
                path: net_path_from_dial(&candidate.kind),
                addr: candidate.addr.ip().to_string(),
                port: candidate.addr.port(),
                transport: Transport::Quic,
                outcome: match skipped {
                    Skipped::NeedsVpn => ProbeOutcome::NeedsVpn,
                    Skipped::NotOnLan => ProbeOutcome::Skipped,
                },
                ms: None,
                reason: Some(format!("{skipped:?}")),
                last: false,
            },
        );
    }
    for candidate in plan.candidates {
        let c = core.clone();
        let dialer = dialer.clone();
        let batch = batch.clone();
        let host_id = host_id.clone();
        core.spawn(async move {
            let event = probe_candidate(&c, candidate, &dialer, expected_fp, host_id).await;
            batch.emit(&c, event);
        });
    }
}

pub fn transport_dto(kind: TransportKind) -> Transport {
    match kind {
        TransportKind::Quic => Transport::Quic,
        TransportKind::Tcp => Transport::Tcp,
    }
}

/// Prefer authenticated identity evidence, then refusal, timeout, TLS/I/O.
pub fn failure_priority(result: &AttemptResult) -> u8 {
    match result {
        AttemptResult::FingerprintMismatch { .. } => 0,
        AttemptResult::Refused => 1,
        AttemptResult::TimedOut => 2,
        AttemptResult::TlsFailed | AttemptResult::Failed => 3,
        _ => 4,
    }
}

pub fn attempt_ms(attempt: Option<&AttemptLog>, elapsed: Duration) -> u32 {
    let micros = attempt
        .and_then(|log| Some(log.finished_at_us?.saturating_sub(log.started_at_us?)))
        .unwrap_or_else(|| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX));
    u32::try_from(micros / 1_000).unwrap_or(u32::MAX)
}

pub async fn probe_candidate(
    core: &Arc<Core>,
    candidate: Candidate,
    dialer: &DefaultDialer,
    expected_fp: [u8; 32],
    mut host_id: Option<String>,
) -> CoreEvent {
    let discover = host_id.is_none();
    let single = DialPlan {
        candidates: vec![candidate.clone()],
        ..DialPlan::default()
    };
    let opts = DialOpts {
        timeout: core.timeouts().probe,
        ..DialOpts::default()
    };
    let started = Instant::now();
    let (outcome, transport, ms, reason) = match race(&single, dialer, expected_fp, &opts).await {
        Ok(result) => {
            result.primary.close(CloseCode::Normal, b"probe");
            if let Some((backup, _)) = &result.backup {
                backup.close(CloseCode::Normal, b"probe");
            }
            if discover {
                host_id = Some(id_from_fp(&expected_fp));
            }
            let attempt = result.attempts.iter().find(|log| {
                log.result == AttemptResult::Connected && log.transport == Some(result.transport)
            });
            (
                ProbeOutcome::Ok,
                transport_dto(result.transport),
                attempt_ms(attempt, started.elapsed()),
                None,
            )
        }
        Err(failure) => {
            let attempt = failure
                .attempts
                .iter()
                .min_by_key(|log| failure_priority(&log.result));
            let result = attempt.map_or(&AttemptResult::Failed, |log| &log.result);
            let outcome = match result {
                // An authenticated TLS peer exposes its SPKI before RSP pairing.
                AttemptResult::FingerprintMismatch { got } if discover => {
                    host_id = Some(id_from_fp(got));
                    ProbeOutcome::Ok
                }
                AttemptResult::FingerprintMismatch { .. } => ProbeOutcome::FingerprintMismatch,
                AttemptResult::Refused => ProbeOutcome::Refused,
                AttemptResult::TimedOut => ProbeOutcome::Timeout,
                _ => ProbeOutcome::NoRoute,
            };
            let reason = if outcome == ProbeOutcome::Ok {
                None
            } else if matches!(result, AttemptResult::FingerprintMismatch { .. }) {
                Some("FingerprintMismatch".to_owned())
            } else {
                Some(format!("{result:?}"))
            };
            (
                outcome,
                transport_dto(
                    attempt
                        .and_then(|log| log.transport)
                        .unwrap_or(TransportKind::Quic),
                ),
                attempt_ms(attempt, started.elapsed()),
                reason,
            )
        }
    };
    CoreEvent::ProbeResult {
        probe_id: String::new(),
        host_id,
        path: net_path_from_dial(&candidate.kind),
        addr: candidate.addr.ip().to_string(),
        port: candidate.addr.port(),
        transport,
        outcome,
        ms: Some(ms),
        reason,
        last: false,
    }
}

#[derive(Default)]
pub struct NetworkState {
    pub previous: Mutex<Option<NetworkStatus>>,
    pub polling_started: AtomicBool,
}

pub fn refresh_network(core: &Arc<Core>, raw: &[RawIface]) -> NetworkStatus {
    let (pools, work_profile_ifaces) = {
        let state = core.state::<SettingsState>();
        let settings = state.0.lock().unwrap_or_else(|e| e.into_inner());
        (settings.vpn_pools(), settings.work_profile_ifaces.clone())
    };
    let mut status = snapshot_status(raw, &pools, &work_profile_ifaces);
    status.tailnet = super::tailnet_wire::route_status(core);
    let state = core.state::<NetworkState>();
    let mut previous = state.previous.lock().unwrap_or_else(|e| e.into_inner());
    let ifaces_changed = previous
        .as_ref()
        .is_some_and(|old| old.ifaces != status.ifaces);
    if previous.as_ref().is_some_and(|old| old != &status) {
        core.events().emit(&CoreEvent::NetworkChanged {
            status: status.clone(),
        });
    }
    *previous = Some(status.clone());
    drop(previous);
    super::tailnet_wire::on_network(core, raw, ifaces_changed);
    status
}

pub async fn status(core: &Arc<Core>) -> Result<NetworkStatus, CoreError> {
    let status = refresh_network(core, &read_interfaces()?);
    if !core
        .state::<NetworkState>()
        .polling_started
        .swap(true, Ordering::AcqRel)
    {
        // Native network push has no ABI entry yet; poll without retaining Core.
        let weak = Arc::downgrade(core);
        let task = core.spawn(async move {
            let mut interval = tokio::time::interval_at(
                tokio::time::Instant::now() + Duration::from_secs(5),
                Duration::from_secs(5),
            );
            interval.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            loop {
                interval.tick().await;
                let Some(core) = weak.upgrade() else { break };
                match read_interfaces() {
                    Ok(raw) => {
                        refresh_network(&core, &raw);
                    }
                    Err(error) => tracing::warn!(%error, "network interface polling failed"),
                }
            }
        });
        core.add_shutdown_hook(Box::new(move || task.abort()));
    }
    Ok(status)
}

pub fn set_setting(core: &Arc<Core>, key: &str, value_json: &str) -> Result<(), CoreError> {
    if key == SYNC_SERVERS {
        return Ok(save_sync_servers(&core.store(), value_json)?);
    }
    let value = serde_json::from_str(value_json).map_err(|_| CoreError::internal("bad json"))?;
    let changed = core
        .state::<SettingsState>()
        .0
        .lock()
        .unwrap_or_else(|e| e.into_inner())
        .set(key, value)
        .map_err(|SettingsError(message)| CoreError::internal(message))?;
    if (key == "networks" || key == "device.workProfileIfaces") && changed {
        refresh_network(core, &read_interfaces()?);
    }
    if key == "device.lowPower" && changed {
        sync_low_power(core);
    }
    if matches!(
        key,
        "tailnet.mode" | "tailnet.controlUrl" | "device.appActive"
    ) {
        super::tailnet_wire::on_setting(core, key)?;
    }
    Ok(())
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn diag_probe(host_id: String) -> Result<u64, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { probe(&c, &host_id).await })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn diag_probe_address(addr: String, port: Option<u16>) -> Result<u64, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { probe_address(&c, &addr, port).await })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn network_status() -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&status(&c).await?)?) })
        .await
}

#[uniffi::export]
pub fn settings_set(key: String, value_json: String) -> Result<(), CoreError> {
    set_setting(&core()?, &key, &value_json)
}
