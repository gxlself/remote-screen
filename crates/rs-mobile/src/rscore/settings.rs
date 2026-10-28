//! In-memory native settings. MMKV remains the source of truth in the app.

use std::{net::IpAddr, sync::Mutex, time::Duration};

use rsp_transport::dial::IpNet;
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::api_ssh::SERVERS_SYNC_KV;
use super::dto_core::Quality;
use super::store::{Store, StoreError};

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkEntry {
    pub id: String,
    pub name: String,
    pub cidrs: Vec<String>,
    pub vpn_pools: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Settings {
    pub quality_default: Quality,
    pub tailnet_mode: String,
    /// 用户填的 Headscale 地址（App 的 `tailnet.controlUrl`）。None = 没填。
    pub tailnet_control_url: Option<String>,
    /// 运行时提示：App 是否在前台。App 按 AppState 推送，不持久化。默认 true。
    pub app_active: bool,
    pub prewarm_enabled: bool,
    pub networks: Vec<NetworkEntry>,
    /// Runtime-only power hint pushed by the App; never persisted here.
    pub low_power: bool,
    /// Runtime-only background hold hint pushed by the App; never persisted here.
    pub background_hold: bool,
    /// Runtime-only work-profile interface names pushed by the App; never persisted here.
    pub work_profile_ifaces: Vec<String>,
    /// 录制总占用上限（MB）：1024 / 2048 / 4096 / 8192。App 的 `recording.maxTotalMb`。
    pub recording_max_total_mb: u64,
    /// 录制保留天数：7 / 30 / 90；None = 永久。App 的 `recording.retentionDays`。
    pub recording_retention_days: Option<u32>,
}

impl Default for Settings {
    fn default() -> Self {
        Self {
            quality_default: Quality::Auto,
            tailnet_mode: "system".into(),
            tailnet_control_url: None,
            app_active: true,
            prewarm_enabled: true,
            networks: Vec::new(),
            low_power: false,
            background_hold: false,
            work_profile_ifaces: Vec::new(),
            recording_max_total_mb: 4096,
            recording_retention_days: Some(30),
        }
    }
}

impl Settings {
    /// Validate before changing anything; unsupported app-only keys are ignored.
    pub fn set(&mut self, key: &str, value: Value) -> Result<bool, SettingsError> {
        let bad_value = || SettingsError(format!("bad value for {key}"));
        match key {
            "quality.default" => {
                let quality = match value.as_str() {
                    Some("auto") => Quality::Auto,
                    Some("clear") => Quality::Clear,
                    Some("smooth") => Quality::Smooth,
                    Some("eco") => Quality::Eco,
                    _ => return Err(bad_value()),
                };
                Ok(replace_if_changed(&mut self.quality_default, quality))
            }
            "tailnet.mode" => {
                let mode = match value.as_str() {
                    Some(mode @ ("system" | "embedded" | "auto")) => mode,
                    _ => return Err(bad_value()),
                };
                Ok(replace_if_changed(&mut self.tailnet_mode, mode.into()))
            }
            "tailnet.controlUrl" => {
                let next = match &value {
                    Value::Null => None,
                    Value::String(s) if s.trim().is_empty() => None,
                    Value::String(s) => Some(normalize_control_url(s).ok_or_else(bad_value)?),
                    _ => return Err(bad_value()),
                };
                Ok(replace_if_changed(&mut self.tailnet_control_url, next))
            }
            "device.appActive" => Ok(replace_if_changed(
                &mut self.app_active,
                value.as_bool().ok_or_else(bad_value)?,
            )),
            "prewarm.enabled" => Ok(replace_if_changed(
                &mut self.prewarm_enabled,
                value.as_bool().ok_or_else(bad_value)?,
            )),
            "networks" => {
                let networks = if value.is_null() {
                    Vec::new()
                } else {
                    serde_json::from_value::<Vec<NetworkEntry>>(value).map_err(|_| bad_value())?
                };
                Ok(replace_if_changed(&mut self.networks, networks))
            }
            "recording.maxTotalMb" => {
                let mb = value
                    .as_u64()
                    .filter(|mb| [1024, 2048, 4096, 8192].contains(mb))
                    .ok_or_else(bad_value)?;
                Ok(replace_if_changed(&mut self.recording_max_total_mb, mb))
            }
            "recording.retentionDays" => {
                let days = if value.is_null() {
                    None
                } else {
                    let days = value
                        .as_u64()
                        .filter(|days| [7, 30, 90].contains(days))
                        .ok_or_else(bad_value)?;
                    Some(days as u32)
                };
                Ok(replace_if_changed(&mut self.recording_retention_days, days))
            }
            "device.lowPower" => Ok(replace_if_changed(
                &mut self.low_power,
                value.as_bool().ok_or_else(bad_value)?,
            )),
            "device.backgroundHold" => Ok(replace_if_changed(
                &mut self.background_hold,
                value.as_bool().ok_or_else(bad_value)?,
            )),
            "device.workProfileIfaces" => {
                let mut ifaces = if value.is_null() {
                    Vec::new()
                } else {
                    serde_json::from_value::<Vec<String>>(value).map_err(|_| bad_value())?
                };
                if ifaces.len() > 32
                    || ifaces
                        .iter()
                        .any(|iface| iface.is_empty() || iface.len() > 64)
                {
                    return Err(bad_value());
                }
                ifaces.sort();
                ifaces.dedup();
                Ok(replace_if_changed(&mut self.work_profile_ifaces, ifaces))
            }
            _ => {
                tracing::debug!(key, "ignoring unknown native setting");
                Ok(false)
            }
        }
    }

    pub fn intranet_cidrs(&self) -> Vec<IpNet> {
        parsed_networks(self.networks.iter().flat_map(|network| &network.cidrs))
    }

    pub fn vpn_pools(&self) -> Vec<IpNet> {
        parsed_networks(self.networks.iter().flat_map(|network| &network.vpn_pools))
    }

    /// 给以后接 BackgroundTimeline 的卡用：`BackgroundTimeline::with_grace(settings.background_grace())`。
    pub fn background_grace(&self) -> Option<Duration> {
        if self.background_hold {
            None
        } else {
            Some(rsp_client::recovery::background::PAUSE_GRACE)
        }
    }
}

fn replace_if_changed<T: PartialEq>(current: &mut T, next: T) -> bool {
    if *current == next {
        false
    } else {
        *current = next;
        true
    }
}

fn parsed_networks<'a>(networks: impl Iterator<Item = &'a String>) -> Vec<IpNet> {
    networks
        .filter_map(|value| {
            let parsed = parse_cidr(value);
            if parsed.is_none() {
                tracing::warn!(cidr = %value, "skipping invalid network range");
            }
            parsed
        })
        .collect()
}

#[derive(Default)]
pub struct SettingsState(pub Mutex<Settings>);

#[derive(Debug, Clone, PartialEq, thiserror::Error)]
#[error("{0}")]
pub struct SettingsError(pub String);

/// The App's local `servers` namespace . It is persisted for
/// SSH lookups, not held in `Settings`.
pub const SYNC_SERVERS: &str = "sync.servers";

/// Store the pushed JSON verbatim. Malformed JSON is ignored, keeping the
/// last good server list.
pub fn save_sync_servers(store: &Store, value_json: &str) -> Result<(), StoreError> {
    if serde_json::from_str::<Value>(value_json).is_err() {
        tracing::warn!("ignoring sync.servers value that is not JSON");
        return Ok(());
    }
    store.kv_set(SERVERS_SYNC_KV, value_json)
}

/// Accept a CIDR or a single host address (/32 for IPv4, /128 for IPv6).
pub fn parse_cidr(s: &str) -> Option<IpNet> {
    if s.contains('/') {
        s.parse().ok()
    } else {
        let addr: IpAddr = s.parse().ok()?;
        IpNet::new(addr, if addr.is_ipv4() { 32 } else { 128 }).ok()
    }
}

/// Normalize and validate a user-provided Headscale control URL.
pub fn normalize_control_url(s: &str) -> Option<String> {
    let trimmed = s.trim();
    if trimmed.is_empty() || trimmed.len() > 512 {
        return None;
    }
    let url = url::Url::parse(trimmed).ok()?;
    if !matches!(url.scheme(), "http" | "https")
        || url.host().is_none()
        || url.query().is_some()
        || url.fragment().is_some()
        || !url.username().is_empty()
        || url.password().is_some()
    {
        return None;
    }
    Some(url.as_str().trim_end_matches('/').to_owned())
}
