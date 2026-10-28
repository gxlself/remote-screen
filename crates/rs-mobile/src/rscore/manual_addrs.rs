//! Addresses the App registered for a known computer ("add by address").
//! They live as `computer` entries in the `servers` namespace, whose last
//! value rs.db keeps under `SERVERS_SYNC_KV`.

use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
    sync::Arc,
};

use rsp_transport::dial::{Candidate, CandidateSource, IpNet, LocalNet};
use serde_json::Value;

use super::{
    api_ssh::SERVERS_SYNC_KV,
    candidates::{host_target, manual_candidate, HostTarget, TargetError},
    runtime::Core,
    store::{LastGoodRow, PairedHostRow},
};

/// `(hostId, address)` for each `computer` entry, with the same tolerance as
/// `sshbook::parse_servers_value`: a bad entry is skipped, not the value.
/// Hostnames are deliberately skipped, as for QR addresses; this layer
/// performs no DNS lookups.
pub fn parse_manual_computers(value: Option<&Value>) -> Vec<(String, SocketAddr)> {
    let Some(object) = value.and_then(Value::as_object) else {
        return Vec::new();
    };
    if object.get("v").and_then(Value::as_f64) != Some(1.0) {
        return Vec::new();
    }
    let Some(entries) = object.get("entries").and_then(Value::as_array) else {
        return Vec::new();
    };
    entries.iter().filter_map(parse_manual_computer).collect()
}

fn parse_manual_computer(value: &Value) -> Option<(String, SocketAddr)> {
    let entry = value.as_object()?;
    if entry.get("kind").and_then(Value::as_str) != Some("computer") {
        return None;
    }
    let host_id = entry.get("hostId")?.as_str().filter(|id| !id.is_empty())?;
    let port = entry
        .get("port")?
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)?;
    let host = entry.get("host")?.as_str()?.trim();
    let literal = host
        .strip_prefix('[')
        .and_then(|host| host.strip_suffix(']'))
        .unwrap_or(host);
    let ip = literal.parse::<IpAddr>().ok()?;
    Some((host_id.to_owned(), SocketAddr::new(ip, port)))
}

/// Manual candidates for `host_id`. An unreadable cache yields none rather
/// than failing the dial.
pub fn manual_candidates(core: &Arc<Core>, host_id: &str) -> Vec<Candidate> {
    let json = match core.store().kv_get(SERVERS_SYNC_KV) {
        Ok(json) => json,
        Err(error) => {
            tracing::debug!(%error, "reading manual addresses failed");
            None
        }
    };
    let value = json.and_then(|json| serde_json::from_str::<Value>(&json).ok());
    let mut seen = BTreeSet::new();
    parse_manual_computers(value.as_ref())
        .into_iter()
        .filter(|(id, addr)| id == host_id && seen.insert(*addr))
        .map(|(_, addr)| manual_candidate(addr))
        .collect()
}

/// `host_target` followed by the manual addresses not already present. A
/// paired host with no other address dials its manual addresses alone. They
/// add no trust: the pin is still the paired row's fingerprint.
pub fn host_target_with_manual(
    core: &Arc<Core>,
    host_id: &str,
    paired: Option<&PairedHostRow>,
    last_good: Option<&LastGoodRow>,
    local: &LocalNet,
    settings_cidrs: &[IpNet],
) -> Result<HostTarget, TargetError> {
    match host_target(host_id, paired, last_good, local, settings_cidrs) {
        Ok(mut target) => {
            for candidate in manual_candidates(core, host_id) {
                if !target
                    .candidates
                    .iter()
                    .any(|existing| existing.addr == candidate.addr)
                {
                    target.candidates.push(candidate);
                }
            }
            Ok(target)
        }
        Err(TargetError::NoAddress) => {
            let manual = manual_candidates(core, host_id);
            let Some(paired) = paired.filter(|_| !manual.is_empty()) else {
                return Err(TargetError::NoAddress);
            };
            // Let host_target keep the network settings.
            let mut row = paired.clone();
            row.addresses = manual.iter().map(|candidate| candidate.addr).collect();
            let mut target = host_target(host_id, Some(&row), last_good, local, settings_cidrs)?;
            for candidate in &mut target.candidates {
                candidate.source = CandidateSource::Manual;
            }
            Ok(target)
        }
        Err(error) => Err(error),
    }
}
