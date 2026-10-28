//! Network interface enumeration on macOS.

use std::collections::BTreeMap;
use std::ffi::CStr;
use std::net::{IpAddr, Ipv4Addr, Ipv6Addr};

use host_core::platform::Nic;

/// Return the CIDR network obtained by masking an address with a netmask.
/// Non-contiguous masks are rejected because they cannot be represented as a
/// CIDR prefix.
pub fn cidr_network(address: IpAddr, netmask: IpAddr) -> Option<String> {
    match (address, netmask) {
        (IpAddr::V4(address), IpAddr::V4(mask)) => {
            let prefix = prefix_len(u128::from(u32::from(mask)), 32)?;
            let network = Ipv4Addr::from(u32::from(address) & u32::from(mask));
            Some(format!("{network}/{prefix}"))
        }
        (IpAddr::V6(address), IpAddr::V6(mask)) => {
            let prefix = prefix_len(u128::from_be_bytes(mask.octets()), 128)?;
            let network = Ipv6Addr::from(
                u128::from_be_bytes(address.octets()) & u128::from_be_bytes(mask.octets()),
            );
            Some(format!("{network}/{prefix}"))
        }
        _ => None,
    }
}

/// Whether an address belongs to the Tailscale CGNAT or ULA tailnet ranges.
pub fn is_tailnet_address(address: IpAddr) -> bool {
    match address {
        IpAddr::V4(address) => (u32::from(address) & 0xffc0_0000) == 0x6440_0000,
        IpAddr::V6(address) => address.octets()[..6] == [0xfd, 0x7a, 0x11, 0x5c, 0xa1, 0xe0],
    }
}

#[derive(Default)]
struct NicBuilder {
    addresses: Vec<IpAddr>,
    networks: Vec<String>,
    mac: Option<[u8; 6]>,
    tailnet: bool,
}

pub fn enumerate() -> Vec<Nic> {
    let mut first = std::ptr::null_mut();
    // SAFETY: getifaddrs initializes the output pointer and the linked list
    // is released exactly once below.
    let status = unsafe { libc::getifaddrs(&mut first) };
    if status != 0 {
        return Vec::new();
    }
    let mut builders = BTreeMap::<String, NicBuilder>::new();
    let mut current = first;
    while !current.is_null() {
        // SAFETY: current points into the list returned by getifaddrs until
        // freeifaddrs is called after this loop.
        let entry = unsafe { &*current };
        let name = if entry.ifa_name.is_null() {
            String::new()
        } else {
            // SAFETY: ifa_name is a NUL-terminated interface name from libc.
            unsafe { CStr::from_ptr(entry.ifa_name) }
                .to_string_lossy()
                .into_owned()
        };
        if !name.is_empty() && (entry.ifa_flags & libc::IFF_LOOPBACK as u32) == 0 {
            let builder = builders.entry(name).or_default();
            if entry.ifa_addr.is_null() {
                current = entry.ifa_next;
                continue;
            }
            // SAFETY: the address pointer is valid for the duration of the
            // getifaddrs list; the family read is common to all sockaddr forms.
            let family = unsafe { (*entry.ifa_addr).sa_family as i32 };
            if family == libc::AF_LINK {
                if builder.mac.is_none() {
                    // SAFETY: AF_LINK entries use sockaddr_dl on macOS.
                    builder.mac = unsafe { mac_from_sockaddr(entry.ifa_addr) };
                }
            } else if family == libc::AF_INET || family == libc::AF_INET6 {
                // SAFETY: family was checked before selecting the sockaddr type.
                if let Some((address, netmask)) =
                    unsafe { ip_from_sockaddr(entry.ifa_addr, entry.ifa_netmask) }
                {
                    if !builder.addresses.contains(&address) {
                        builder.addresses.push(address);
                        builder.tailnet |= is_tailnet_address(address);
                    }
                    if let Some(network) = netmask.and_then(|mask| cidr_network(address, mask)) {
                        if !builder.networks.contains(&network) {
                            builder.networks.push(network);
                        }
                    }
                }
            }
        }
        current = entry.ifa_next;
    }
    // SAFETY: first came from getifaddrs and has not been freed yet.
    unsafe { libc::freeifaddrs(first) };

    builders
        .into_iter()
        .filter(|(_, builder)| !builder.addresses.is_empty())
        .map(|(name, mut builder)| {
            builder.addresses.sort();
            builder.networks.sort();
            Nic {
                name,
                addresses: builder.addresses,
                networks: builder.networks,
                mac: builder.mac,
                tailnet: builder.tailnet,
            }
        })
        .collect()
}

fn prefix_len(mask: u128, bits: u32) -> Option<u32> {
    let mut prefix = 0;
    let mut seen_zero = false;
    for shift in (0..bits).rev() {
        let bit = ((mask >> shift) & 1) != 0;
        if bit {
            if seen_zero {
                return None;
            }
            prefix += 1;
        } else {
            seen_zero = true;
        }
    }
    Some(prefix)
}

unsafe fn ip_from_sockaddr(
    address: *const libc::sockaddr,
    netmask: *const libc::sockaddr,
) -> Option<(IpAddr, Option<IpAddr>)> {
    let family = (*address).sa_family as i32;
    match family {
        libc::AF_INET => {
            let address = *(address.cast::<libc::sockaddr_in>());
            let address = IpAddr::V4(Ipv4Addr::from(address.sin_addr.s_addr.to_ne_bytes()));
            let netmask = if netmask.is_null() {
                None
            } else {
                let mask = *(netmask.cast::<libc::sockaddr_in>());
                Some(IpAddr::V4(Ipv4Addr::from(
                    mask.sin_addr.s_addr.to_ne_bytes(),
                )))
            };
            Some((address, netmask))
        }
        libc::AF_INET6 => {
            let address = *(address.cast::<libc::sockaddr_in6>());
            let address = IpAddr::V6(Ipv6Addr::from(address.sin6_addr.s6_addr));
            let netmask = if netmask.is_null() {
                None
            } else {
                let mask = *(netmask.cast::<libc::sockaddr_in6>());
                Some(IpAddr::V6(Ipv6Addr::from(mask.sin6_addr.s6_addr)))
            };
            Some((address, netmask))
        }
        _ => None,
    }
}

unsafe fn mac_from_sockaddr(address: *const libc::sockaddr) -> Option<[u8; 6]> {
    let link = &*address.cast::<libc::sockaddr_dl>();
    let offset = usize::from(link.sdl_nlen);
    if usize::from(link.sdl_alen) < 6 || offset.checked_add(6)? > link.sdl_data.len() {
        return None;
    }
    let mut mac = [0u8; 6];
    for (index, byte) in mac.iter_mut().enumerate() {
        *byte = link.sdl_data[offset + index] as u8;
    }
    Some(mac)
}

#[cfg(test)]
mod tests {
    use super::{cidr_network, is_tailnet_address};
    use std::net::{IpAddr, Ipv4Addr};

    #[test]
    fn cidr_masks_ipv4_and_ipv6() {
        assert_eq!(
            cidr_network(
                IpAddr::V4(Ipv4Addr::new(192, 168, 1, 42)),
                IpAddr::V4(Ipv4Addr::new(255, 255, 255, 0)),
            ),
            Some("192.168.1.0/24".into())
        );
        assert_eq!(
            cidr_network(
                IpAddr::V6("fd7a:115c:a1e0:1234::42".parse().expect("valid IPv6")),
                IpAddr::V6("ffff:ffff:ffff::".parse().expect("valid IPv6 mask")),
            ),
            Some("fd7a:115c:a1e0::/48".into())
        );
    }

    #[test]
    fn tailnet_ranges_are_exact() {
        assert!(is_tailnet_address(IpAddr::V4(Ipv4Addr::new(100, 64, 0, 1))));
        assert!(is_tailnet_address(IpAddr::V4(Ipv4Addr::new(
            100, 127, 255, 254
        ))));
        assert!(!is_tailnet_address(IpAddr::V4(Ipv4Addr::new(
            100, 128, 0, 1
        ))));
        assert!(is_tailnet_address(IpAddr::V6(
            "fd7a:115c:a1e0::1".parse().expect("valid IPv6")
        )));
        assert!(!is_tailnet_address(IpAddr::V6(
            "fd7a:115c:a1e1::1".parse().expect("valid IPv6")
        )));
    }
}
