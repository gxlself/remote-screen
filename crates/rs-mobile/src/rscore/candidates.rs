//! Collect dial candidates and pre-dial observations without opening sockets.

use std::{
    collections::BTreeSet,
    net::{IpAddr, SocketAddr},
};

use rsp_core::errors::ErrorCode;
use rsp_transport::dial::{Candidate, CandidateSource, IpNet, LocalNet, Observations, PathKind};

use super::{
    netinfo::is_tailnet_ip,
    store::{LastGoodRow, PairedHostRow},
};

#[derive(Debug, Clone)]
pub struct HostTarget {
    pub host_id: String,
    pub host_name: String,
    pub fp: [u8; 32],
    pub candidates: Vec<Candidate>,
    pub observations: Observations,
    pub intranet_cidrs: Vec<IpNet>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum TargetError {
    #[error("host is not paired")]
    NotPaired,
    #[error("no address for host")]
    NoAddress,
}

impl TargetError {
    pub fn code(&self) -> ErrorCode {
        match self {
            Self::NotPaired => ErrorCode::NotPaired,
            Self::NoAddress => ErrorCode::Offline,
        }
    }
}

/// User-supplied addresses stay eligible even outside the current LAN.
pub fn addr_kind(ip: IpAddr) -> PathKind {
    if is_tailnet_ip(ip) {
        PathKind::Tailnet
    } else {
        PathKind::Manual
    }
}

pub fn host_target(
    host_id: &str,
    paired: Option<&PairedHostRow>,
    last_good: Option<&LastGoodRow>,
    local: &LocalNet,
    settings_cidrs: &[IpNet],
) -> Result<HostTarget, TargetError> {
    let paired = paired.ok_or(TargetError::NotPaired)?;

    let mut candidates = Vec::new();
    if let Some(row) = last_good {
        let kind = match row.path.as_str() {
            "tailnet" => PathKind::Tailnet,
            "intranet" => PathKind::Intranet,
            "lan" => PathKind::Lan,
            _ => PathKind::Manual,
        };
        candidates.push(Candidate {
            addr: row.addr,
            kind: PathKind::LastGood(Box::new(kind)),
            source: CandidateSource::Cache,
        });
    }
    candidates.extend(paired.addresses.iter().map(|addr| Candidate {
        addr: *addr,
        kind: addr_kind(addr.ip()),
        source: CandidateSource::Cache,
    }));
    let mut seen = BTreeSet::new();
    candidates.retain(|candidate| seen.insert(candidate.addr));
    if candidates.is_empty() {
        return Err(TargetError::NoAddress);
    }

    let mut intranet_cidrs = Vec::new();
    for net in settings_cidrs.iter().copied() {
        if !intranet_cidrs.contains(&net) {
            intranet_cidrs.push(net);
        }
    }

    Ok(HostTarget {
        host_id: host_id.to_owned(),
        host_name: paired.name.clone(),
        fp: paired.fp,
        candidates,
        observations: Observations {
            local: local.clone(),
            ..Observations::default()
        },
        intranet_cidrs,
    })
}

/// QR hostnames are deliberately skipped; this layer performs no DNS lookups.
pub fn qr_candidates(addrs: &[String]) -> Vec<Candidate> {
    addrs
        .iter()
        .filter_map(|address| match address.parse() {
            Ok(addr) => Some(manual_candidate(addr)),
            Err(_) => {
                tracing::warn!(
                    "skipping QR address: expected an IP literal and port; DNS is not resolved"
                );
                None
            }
        })
        .collect()
}

pub fn manual_candidate(addr: SocketAddr) -> Candidate {
    Candidate {
        addr,
        kind: addr_kind(addr.ip()),
        source: CandidateSource::Manual,
    }
}
