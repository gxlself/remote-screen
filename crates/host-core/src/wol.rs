//! Wake-on-LAN routing and packet delivery.
//!
//! [`select_routes`] picks every local, non-tailnet interface whose IPv4
//! network has a given directed broadcast address.
//!
//! [`Platform::send_wol`](crate::platform::Platform::send_wol) has no separate
//! broadcast argument. Its `via` value is narrowed by [`select_routes`] to one
//! matching network and the local IPv4 addresses in that network. A platform
//! implementation should therefore use [`route_of`] to recover the source and
//! broadcast addresses, then call [`send_magic_udp`]:
//!
//! ```text
//! let Some((source, broadcast)) = route_of(via) else {
//!     return Err(PlatformError::Platform("no ipv4 route".into()));
//! };
//! send_magic_udp(mac, source, broadcast, WOL_PORT)?;
//! ```

use std::net::{IpAddr, Ipv4Addr, SocketAddrV4, UdpSocket};

use ipnet::Ipv4Net;

use crate::platform::Nic;

pub const WOL_PORT: u16 = 9;

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum WolError {
    #[error("invalid mac")]
    Mac,
    #[error("invalid broadcast")]
    Broadcast,
}

/// Accepts `aa:bb:cc:dd:ee:ff` or `AA-BB-CC-DD-EE-FF`.
pub fn parse_mac(s: &str) -> std::result::Result<[u8; 6], WolError> {
    let bytes = s.as_bytes();
    if bytes.len() != 17 {
        return Err(WolError::Mac);
    }
    let separator = bytes[2];
    if separator != b':' && separator != b'-' {
        return Err(WolError::Mac);
    }

    let mut mac = [0_u8; 6];
    for (index, octet) in mac.iter_mut().enumerate() {
        let offset = index * 3;
        if index > 0 && bytes[offset - 1] != separator {
            return Err(WolError::Mac);
        }
        let high = hex_value(bytes[offset]).ok_or(WolError::Mac)?;
        let low = hex_value(bytes[offset + 1]).ok_or(WolError::Mac)?;
        *octet = (high << 4) | low;
    }
    Ok(mac)
}

fn hex_value(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

/// Six `0xff` bytes followed by the MAC address repeated 16 times.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xff_u8; 102];
    for chunk in packet[6..].chunks_exact_mut(mac.len()) {
        chunk.copy_from_slice(&mac);
    }
    packet
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct WakeRoute {
    /// The interface passed to `Platform::send_wol`.
    pub nic: Nic,
    pub source: Ipv4Addr,
    pub network: Ipv4Net,
    pub broadcast: Ipv4Addr,
}

/// Selects non-tailnet interfaces with an IPv4 network for `broadcast`.
pub fn select_routes(nics: &[Nic], broadcast: Ipv4Addr) -> Vec<WakeRoute> {
    if broadcast == Ipv4Addr::BROADCAST {
        return Vec::new();
    }
    let mut routes = nics
        .iter()
        .filter(|nic| !nic.tailnet)
        .filter_map(|nic| select_nic_route(nic, broadcast))
        .collect::<Vec<_>>();
    routes.sort_by(|left, right| left.nic.name.cmp(&right.nic.name));
    routes
}

fn select_nic_route(nic: &Nic, broadcast: Ipv4Addr) -> Option<WakeRoute> {
    for value in &nic.networks {
        let Ok(network) = value.parse::<Ipv4Net>() else {
            continue;
        };
        let network = network.trunc();
        if network.prefix_len() > 30 || network.broadcast() != broadcast {
            continue;
        }

        let mut ipv4_addresses = nic
            .addresses
            .iter()
            .copied()
            .filter(|address| match address {
                IpAddr::V4(address) => network.contains(address),
                IpAddr::V6(_) => false,
            })
            .filter_map(|address| match address {
                IpAddr::V4(address) => Some(address),
                IpAddr::V6(_) => None,
            })
            .collect::<Vec<_>>();
        ipv4_addresses.sort_unstable();
        let Some(&source) = ipv4_addresses.first() else {
            continue;
        };
        let addresses = ipv4_addresses
            .into_iter()
            .map(IpAddr::V4)
            .collect::<Vec<_>>();

        return Some(WakeRoute {
            nic: Nic {
                name: nic.name.clone(),
                addresses,
                networks: vec![network.to_string()],
                mac: nic.mac,
                tailnet: nic.tailnet,
            },
            source,
            network,
            broadcast,
        });
    }
    None
}

/// Returns the first IPv4 route and address from a NIC narrowed by [`select_routes`].
pub fn route_of(via: &Nic) -> Option<(Ipv4Addr, Ipv4Addr)> {
    for value in &via.networks {
        let Ok(network) = value.parse::<Ipv4Net>() else {
            continue;
        };
        let network = network.trunc();
        for address in &via.addresses {
            if let IpAddr::V4(address) = address {
                if network.contains(address) {
                    return Some((*address, network.broadcast()));
                }
            }
        }
    }
    None
}

/// Sends one magic packet from `source` to `broadcast:port` over UDP.
pub fn send_magic_udp(
    mac: [u8; 6],
    source: Ipv4Addr,
    broadcast: Ipv4Addr,
    port: u16,
) -> std::io::Result<()> {
    let socket = UdpSocket::bind(SocketAddrV4::new(source, 0))?;
    socket.set_broadcast(true)?;
    let packet = magic_packet(mac);
    let sent = socket.send_to(&packet, SocketAddrV4::new(broadcast, port))?;
    if sent != packet.len() {
        return Err(std::io::Error::new(
            std::io::ErrorKind::WriteZero,
            "incomplete wake-on-lan packet",
        ));
    }
    Ok(())
}
