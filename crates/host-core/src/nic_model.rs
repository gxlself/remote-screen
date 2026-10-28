//! Platform-independent network adapter conversion.

use std::collections::BTreeMap;
use std::net::IpAddr;

use crate::platform::Nic;
use ipnet::IpNet;

/// Windows' software loopback interface type.
pub const IF_TYPE_SOFTWARE_LOOPBACK: u32 = 24;

/// The subset of an adapter returned by the Windows API that is needed by the
/// platform model.  Keeping this type independent from Win32 makes all of the
/// filtering and address normalization testable on every target.
#[derive(Debug, Clone, PartialEq)]
pub struct AdapterRecord {
    pub friendly_name: String,
    pub description: String,
    pub if_type: u32,
    pub oper_up: bool,
    pub mac: Vec<u8>,
    pub unicast: Vec<(IpAddr, u8)>,
}

/// Converts raw adapter records into the stable public NIC representation.
pub fn to_nics(records: &[AdapterRecord]) -> Vec<Nic> {
    let mut by_name = BTreeMap::<String, Nic>::new();

    for record in records {
        if record.if_type == IF_TYPE_SOFTWARE_LOOPBACK
            || !record.oper_up
            || record.unicast.is_empty()
        {
            continue;
        }

        let name = if record.friendly_name.is_empty() {
            record.description.clone()
        } else {
            record.friendly_name.clone()
        };

        let mut addresses = record
            .unicast
            .iter()
            .map(|(address, _)| *address)
            .collect::<Vec<_>>();
        addresses.sort_by(ip_order);
        addresses.dedup();

        let mut unicast = record.unicast.clone();
        unicast.sort_by(|(left, left_prefix), (right, right_prefix)| {
            ip_order(left, right).then(left_prefix.cmp(right_prefix))
        });

        let mut networks = Vec::new();
        for (address, prefix_len) in unicast {
            let Some(network) = network_string(address, prefix_len) else {
                continue;
            };
            if !networks.contains(&network) {
                networks.push(network);
            }
        }

        let mac = if record.mac.len() == 6 && record.mac.iter().any(|byte| *byte != 0) {
            let mut value = [0_u8; 6];
            value.copy_from_slice(&record.mac);
            Some(value)
        } else {
            None
        };

        let nic = Nic {
            name: name.clone(),
            tailnet: addresses.iter().copied().any(is_tailnet_address),
            addresses,
            networks,
            mac,
        };

        // Adapter enumeration can contain duplicate names.  Preserve the
        // first usable record, then let BTreeMap provide deterministic name
        // ordering for the result.
        by_name.entry(name).or_insert(nic);
    }

    by_name.into_values().collect()
}

/// Whether an address belongs to one of the Tailscale tailnet ranges.
pub fn is_tailnet_address(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(address) => (u32::from(address) & 0xffc0_0000) == 0x6440_0000,
        IpAddr::V6(address) => address.octets()[..6] == [0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0],
    }
}

fn network_string(address: IpAddr, prefix_len: u8) -> Option<String> {
    let max_prefix_len = match address {
        IpAddr::V4(_) => 32,
        IpAddr::V6(_) => 128,
    };
    if u32::from(prefix_len) > max_prefix_len {
        return None;
    }

    IpNet::new(address, prefix_len)
        .ok()
        .map(|network| network.trunc().to_string())
}

fn ip_order(left: &IpAddr, right: &IpAddr) -> std::cmp::Ordering {
    match (left, right) {
        (IpAddr::V4(left), IpAddr::V4(right)) => left.cmp(right),
        (IpAddr::V6(left), IpAddr::V6(right)) => left.cmp(right),
        (IpAddr::V4(_), IpAddr::V6(_)) => std::cmp::Ordering::Less,
        (IpAddr::V6(_), IpAddr::V4(_)) => std::cmp::Ordering::Greater,
    }
}
