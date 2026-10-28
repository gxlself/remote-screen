//! Wake-on-LAN packet construction and directed broadcast delivery.

use std::ffi::CString;
use std::net::{Ipv4Addr, SocketAddrV4, UdpSocket};
use std::os::fd::AsRawFd;
use std::time::Duration;

use host_core::platform::{Nic, PlatformError, Result};

pub const WOL_PORT: u16 = 9;
#[deprecated(note = "host-core repeats; send() sends one packet")]
pub const WOL_REPEATS: usize = 3;
#[deprecated(note = "host-core repeats; send() sends one packet")]
pub const WOL_INTERVAL: Duration = Duration::from_millis(100);

/// Build the 102-byte Wake-on-LAN magic packet.
pub fn magic_packet(mac: [u8; 6]) -> [u8; 102] {
    let mut packet = [0xff; 102];
    for repeat in 0..16 {
        let start = 6 + repeat * 6;
        packet[start..start + 6].copy_from_slice(&mac);
    }
    packet
}

/// Alias returning an owned vector for socket callers.
pub fn build_magic_packet(mac: [u8; 6]) -> Vec<u8> {
    magic_packet(mac).to_vec()
}

/// Return a directed IPv4 broadcast. `/31` and `/32` have no broadcast host
/// by definition (RFC 3021 and host routes), so they return `None`.
pub fn broadcast_address(address: Ipv4Addr, prefix: u8) -> Option<Ipv4Addr> {
    if prefix > 30 {
        return None;
    }
    let bits = u32::from(address);
    let mask = if prefix == 0 {
        0
    } else {
        u32::MAX << (32 - u32::from(prefix))
    };
    Some(Ipv4Addr::from((bits & mask) | !mask))
}

pub fn broadcast_for_cidr(cidr: &str) -> Option<Ipv4Addr> {
    let (network, prefix) = cidr.split_once('/')?;
    let address = network.parse::<Ipv4Addr>().ok()?;
    let prefix = prefix.parse::<u8>().ok()?;
    if prefix > 32 {
        return None;
    }
    broadcast_address(address, prefix)
}

/// Send one magic packet from `source` through `interface` to `dest:port`.
///
/// `interface` is optional so callers can use this helper for loopback tests
/// without binding the socket to a device.
pub fn send_to(
    mac: [u8; 6],
    source: Ipv4Addr,
    dest: Ipv4Addr,
    port: u16,
    interface: Option<&str>,
) -> Result<()> {
    let socket = UdpSocket::bind(SocketAddrV4::new(source, 0))?;

    // `dest` may be a directed broadcast (for example, 192.168.31.255), so its
    // broadcast nature cannot be determined from the address alone without a
    // network prefix. Enabling this option is harmless for unicast test
    // destinations and is required for all WoL broadcast destinations.
    socket.set_broadcast(true)?;

    if let Some(interface) = interface {
        let interface_name = CString::new(interface)
            .map_err(|_| PlatformError::Platform("invalid interface name".into()))?;
        // SAFETY: `interface_name` is a valid, NUL-terminated C string that
        // remains alive for the duration of the libc call.
        let index = unsafe { libc::if_nametoindex(interface_name.as_ptr()) };
        if index == 0 {
            return Err(PlatformError::Platform(format!(
                "WOL interface not found: {interface}"
            )));
        }

        let socket_fd = socket.as_raw_fd();
        let option_len = std::mem::size_of::<libc::c_uint>() as libc::socklen_t;
        // SAFETY: `socket_fd` belongs to the live `socket`; `&index` points to
        // an initialized c_uint and `option_len` matches its size.
        let result = unsafe {
            libc::setsockopt(
                socket_fd,
                libc::IPPROTO_IP,
                libc::IP_BOUND_IF,
                (&index as *const libc::c_uint).cast(),
                option_len,
            )
        };
        if result != 0 {
            let errno = std::io::Error::last_os_error();
            return Err(PlatformError::Platform(format!(
                "IP_BOUND_IF failed: {errno}"
            )));
        }
    }

    let packet = magic_packet(mac);
    let sent = socket.send_to(&packet, SocketAddrV4::new(dest, port))?;
    if sent != packet.len() {
        return Err(PlatformError::Platform(
            "incomplete wake-on-lan packet".into(),
        ));
    }
    Ok(())
}

pub fn send(mac: [u8; 6], via: &Nic) -> Result<()> {
    let (source, broadcast) = host_core::wol::route_of(via).ok_or_else(|| {
        PlatformError::Platform("WOL requires an IPv4 network with a broadcast address".into())
    })?;
    send_to(mac, source, broadcast, WOL_PORT, Some(&via.name))?;
    tracing::debug!(nic = %via.name, %broadcast, "wol packet sent");
    Ok(())
}
