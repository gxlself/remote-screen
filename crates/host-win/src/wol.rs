//! Windows Wake-on-LAN entry points.
//!
//! The packet and socket implementation lives in `host-core`; this module
//! only chooses the route from the selected NIC and adapts its errors.

use host_core::platform::{Nic, PlatformError};
use host_core::wol::{route_of, send_magic_udp};
use tracing::debug;

/// The conventional Wake-on-LAN UDP port.
pub const WOL_PORT: u16 = 9;

/// Sends one Wake-on-LAN packet on the conventional UDP port.
pub fn send_wol(mac: [u8; 6], via: &Nic) -> Result<(), PlatformError> {
    send_wol_to_port(mac, via, WOL_PORT)
}

/// Sends one Wake-on-LAN packet using `via`'s first IPv4 route.
pub fn send_wol_to_port(mac: [u8; 6], via: &Nic, port: u16) -> Result<(), PlatformError> {
    let Some((source, broadcast)) = route_of(via) else {
        return Err(PlatformError::Platform(
            "WOL requires an IPv4 network with a broadcast address".to_owned(),
        ));
    };

    send_magic_udp(mac, source, broadcast, port)?;
    debug!(nic = %via.name, %broadcast, "wol packet sent");
    Ok(())
}
