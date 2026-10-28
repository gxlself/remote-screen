//! Local interface snapshots for network status and dial planning.

use std::{collections::BTreeMap, io, net::IpAddr};

use rsp_transport::dial::{IfaceInfo, IpNet, LocalNet};

use super::dto_core::{NetIface, NetworkStatus};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RawIface {
    pub name: String,
    pub addrs: Vec<IpNet>,
}

/// Read one snapshot, retaining host addresses and their interface prefixes.
pub fn read_interfaces() -> io::Result<Vec<RawIface>> {
    let mut interfaces: BTreeMap<String, Vec<IpNet>> = BTreeMap::new();
    for iface in if_addrs::get_if_addrs()? {
        let ip = iface.ip();
        let link_local = match ip {
            IpAddr::V4(ip) => ip.is_link_local(),
            IpAddr::V6(ip) => ip.is_unicast_link_local(),
        };
        if iface.is_loopback() || link_local {
            continue;
        }
        let prefix = match iface.addr {
            if_addrs::IfAddr::V4(addr) => u32::from(addr.netmask).leading_ones() as u8,
            if_addrs::IfAddr::V6(addr) => u128::from(addr.netmask).leading_ones() as u8,
        };
        let net = IpNet::new(ip, prefix)
            .map_err(|err| io::Error::new(io::ErrorKind::InvalidData, err))?;
        let addrs = interfaces.entry(iface.name).or_default();
        if !addrs.contains(&net) {
            addrs.push(net);
        }
    }
    Ok(interfaces
        .into_iter()
        .map(|(name, addrs)| RawIface { name, addrs })
        .collect())
}

pub fn is_tailnet_ip(ip: IpAddr) -> bool {
    match ip {
        IpAddr::V4(ip) => {
            let octets = ip.octets();
            octets[0] == 100 && (64..128).contains(&octets[1])
        }
        IpAddr::V6(ip) => ip.segments()[..3] == [0xfd7a, 0x115c, 0xa1e0],
    }
}

/// Registered pools take precedence over the interface-name heuristic. A
/// tailnet-only tunnel is not considered a company VPN by its name alone.
pub fn is_vpn(iface: &RawIface, vpn_pools: &[IpNet]) -> bool {
    if iface
        .addrs
        .iter()
        .any(|addr| vpn_pools.iter().any(|pool| pool.contains(addr.addr())))
    {
        return true;
    }
    ["utun", "ipsec", "ppp", "tun", "tap", "wg"]
        .iter()
        .any(|prefix| iface.name.starts_with(prefix))
        && !iface.addrs.iter().all(|addr| is_tailnet_ip(addr.addr()))
}

pub fn local_net(raw: &[RawIface], vpn_pools: &[IpNet]) -> LocalNet {
    local_net_excluding(raw, vpn_pools, &[])
}

/// Removes interfaces that belong to another device profile, such as an
/// Android work-profile VPN, from local routing and VPN detection.
pub fn local_net_excluding(raw: &[RawIface], vpn_pools: &[IpNet], excluded: &[String]) -> LocalNet {
    LocalNet {
        interfaces: raw
            .iter()
            .filter(|iface| !excluded.iter().any(|name| name == &iface.name))
            .map(|iface| IfaceInfo {
                name: iface.name.clone(),
                addrs: iface.addrs.clone(),
                is_vpn: is_vpn(iface, vpn_pools),
            })
            .collect(),
    }
}

pub fn network_status(raw: &[RawIface], vpn_pools: &[IpNet]) -> NetworkStatus {
    network_status_marked(raw, vpn_pools, &[])
}

/// Marks interfaces supplied by the App as belonging to the work profile.
pub fn network_status_marked(
    raw: &[RawIface],
    vpn_pools: &[IpNet],
    work_profile: &[String],
) -> NetworkStatus {
    let mut ifaces: Vec<_> = raw
        .iter()
        .map(|iface| NetIface {
            name: iface.name.clone(),
            addrs: iface
                .addrs
                .iter()
                .map(|addr| format!("{}/{}", addr.addr(), addr.prefix_len()))
                .collect(),
            is_vpn: is_vpn(iface, vpn_pools),
            work_profile: work_profile
                .iter()
                .any(|name| name == &iface.name)
                .then_some(true),
        })
        .collect();
    ifaces.sort_by(|left, right| left.name.cmp(&right.name));
    NetworkStatus {
        ifaces,
        tailnet: None,
    }
}

pub fn has_tailnet(raw: &[RawIface]) -> bool {
    raw.iter()
        .flat_map(|iface| &iface.addrs)
        .any(|addr| is_tailnet_ip(addr.addr()))
}
