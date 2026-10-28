//! V10 native-first discovery: raw TCP probes for Screen Sharing (VNC) and
//! Remote Desktop (RDP), and a LAN scan.

use std::time::Duration;

use super::discover::probe::probe;
use super::discover::scan::{scan_targets, start};
use super::error::CoreError;
use super::netinfo::read_interfaces;
use super::runtime::core;
use super::settings::SettingsState;

/// Probe one address. `kind`: "vnc" | "rdp" | "tcp". Returns JSON
/// `DiscoverProbe` (dto_core.rs). Dials through `Core::net()` (system network
/// or the in-app tailnet).
#[uniffi::export(async_runtime = "tokio")]
pub async fn discover_probe(
    addr: String,
    port: u16,
    kind: String,
    timeout_ms: u32,
) -> Result<String, CoreError> {
    // Keep the pre-initialization behavior used by older native packages. The
    // real probe is selected as soon as a Core has installed its network.
    if !matches!(kind.as_str(), "vnc" | "rdp" | "tcp") {
        return Err(CoreError::internal("unknown probe kind"));
    }
    let c = match core() {
        Ok(core) => core,
        Err(CoreError::Internal { message }) if message == "core not initialized" => {
            return Err(CoreError::Unsupported {
                message: "discover: not built yet".into(),
            });
        }
        Err(error) => return Err(error),
    };
    c.clone()
        .run(async move {
            let result = probe(
                c.net().as_ref(),
                &addr,
                port,
                &kind,
                Duration::from_millis(u64::from(timeout_ms)),
            )
            .await?;
            Ok(serde_json::to_string(&result)?)
        })
        .await
}

/// Scan directly attached IPv4 LAN prefixes for `ports` (5900 / 3389).
/// Returns a scan id; results arrive as `discover.found` events, then one
/// `discover.done`.
#[uniffi::export(async_runtime = "tokio")]
pub async fn discover_scan_lan(ports: Vec<u16>) -> Result<u64, CoreError> {
    if ports.is_empty() || ports.len() > 4 {
        return Err(CoreError::Internal {
            message: "bad ports".into(),
        });
    }
    let c = match core() {
        Ok(core) => core,
        Err(CoreError::Internal { message }) if message == "core not initialized" => {
            return Err(CoreError::Unsupported {
                message: "discover: not built yet".into(),
            });
        }
        Err(error) => return Err(error),
    };
    c.clone()
        .run(async move {
            let vpn_pools = {
                let settings = c.state::<SettingsState>();
                let guard = settings.0.lock().unwrap_or_else(|error| error.into_inner());
                guard.vpn_pools()
            };
            let interfaces = read_interfaces().unwrap_or_default();
            let targets = scan_targets(&interfaces, &vpn_pools, None::<std::net::Ipv4Addr>);
            start(&c, ports, targets).await
        })
        .await
}

/// Stop a running scan; unknown ids are ignored.
#[uniffi::export]
pub fn discover_scan_cancel(scan_id: u64) {
    if let Ok(c) = core() {
        super::discover::scan::cancel(&c, scan_id);
    }
}
