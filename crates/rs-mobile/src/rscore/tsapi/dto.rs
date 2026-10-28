//! Contract DTOs (`TailscaleDevice`, `TailscaleApiStatus` in types.ts) and the
//! subsets of the Tailscale API and Headscale node JSON they are mapped from.
//! Nullable contract fields always emit the key (JSON `null`).

use std::net::Ipv4Addr;

use serde::{Deserialize, Deserializer, Serialize};
use serde_json::Value;
use time::{format_description::well_known::Rfc3339, OffsetDateTime};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TailscaleOs {
    Linux,
    Macos,
    Windows,
    Ios,
    Android,
    Other,
    /// Headscale reports no operating system.
    Unknown,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleDevice {
    pub id: String,
    pub name: String,
    pub dns_name: String,
    pub os: TailscaleOs,
    pub addresses: Vec<String>,
    pub online: Option<bool>,
    pub last_seen_at: Option<u64>,
    pub tags: Vec<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub enum TailscaleApiError {
    Auth,
    Network,
    Http,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct TailscaleApiStatus {
    pub configured: bool,
    /// Headscale control server; `None` for the official API.
    pub control_url: Option<String>,
    pub last_error: Option<TailscaleApiError>,
    pub last_ok_at: Option<u64>,
    pub device_count: Option<usize>,
}

/// `GET /api/v2/tailnet/-/devices` response body.
#[derive(Deserialize)]
pub(super) struct ApiDevices {
    #[serde(default)]
    devices: Vec<ApiDevice>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct ApiDevice {
    id: Option<String>,
    node_id: Option<String>,
    /// MagicDNS name with a trailing dot.
    name: Option<String>,
    hostname: Option<String>,
    os: Option<String>,
    addresses: Option<Vec<String>>,
    connected_to_control: Option<bool>,
    last_seen: Option<String>,
    tags: Option<Vec<String>>,
}

/// Headscale `GET /api/v1/node` response body (protobuf JSON). Unknown
/// fields are ignored and missing ones take defaults.
#[derive(Deserialize)]
pub(super) struct HeadscaleNodes {
    nodes: Option<Vec<HeadscaleNode>>,
}

#[derive(Deserialize)]
#[serde(rename_all = "camelCase")]
struct HeadscaleNode {
    /// uint64: a string in protobuf JSON, a number from some versions.
    #[serde(default, deserialize_with = "string_or_number")]
    id: Option<String>,
    name: Option<String>,
    given_name: Option<String>,
    ip_addresses: Option<Vec<String>>,
    online: Option<bool>,
    last_seen: Option<String>,
    valid_tags: Option<Vec<String>>,
    forced_tags: Option<Vec<String>>,
}

fn string_or_number<'de, D: Deserializer<'de>>(
    deserializer: D,
) -> Result<Option<String>, D::Error> {
    Ok(match Option::<Value>::deserialize(deserializer)? {
        Some(Value::String(id)) => Some(id),
        Some(Value::Number(id)) => Some(id.to_string()),
        _ => None,
    })
}

/// Map and sort by name (case-insensitive; dnsName and id break ties).
pub(super) fn map_devices(api: ApiDevices) -> Vec<TailscaleDevice> {
    sorted(api.devices.into_iter().map(map_device).collect())
}

/// Same order as `map_devices`.
pub(super) fn map_nodes(body: HeadscaleNodes) -> Vec<TailscaleDevice> {
    sorted(
        body.nodes
            .unwrap_or_default()
            .into_iter()
            .map(map_node)
            .collect(),
    )
}

fn sorted(mut devices: Vec<TailscaleDevice>) -> Vec<TailscaleDevice> {
    devices.sort_by_cached_key(|device| {
        (
            device.name.to_lowercase(),
            device.dns_name.clone(),
            device.id.clone(),
        )
    });
    devices
}

/// IPv4 first, otherwise in the given order.
fn ipv4_first(addresses: Vec<String>) -> Vec<String> {
    let (mut v4, rest): (Vec<_>, Vec<_>) = addresses
        .into_iter()
        .partition(|address| address.parse::<Ipv4Addr>().is_ok());
    v4.extend(rest);
    v4
}

fn map_device(device: ApiDevice) -> TailscaleDevice {
    let dns_name = device
        .name
        .unwrap_or_default()
        .trim_end_matches('.')
        .to_owned();
    let name = match dns_name.split('.').next() {
        Some(first) if !first.is_empty() => first.to_owned(),
        _ => device.hostname.unwrap_or_default(),
    };
    TailscaleDevice {
        id: device
            .node_id
            .filter(|id| !id.is_empty())
            .or(device.id)
            .unwrap_or_default(),
        name,
        dns_name,
        os: map_os(device.os.as_deref().unwrap_or_default()),
        addresses: ipv4_first(device.addresses.unwrap_or_default()),
        online: device.connected_to_control,
        last_seen_at: device.last_seen.as_deref().and_then(rfc3339_micros),
        tags: device.tags.unwrap_or_default(),
    }
}

/// No DNS name or system: the App classifies these by their open ports.
fn map_node(node: HeadscaleNode) -> TailscaleDevice {
    let mut tags: Vec<String> = Vec::new();
    for tag in node
        .valid_tags
        .into_iter()
        .chain(node.forced_tags)
        .flatten()
    {
        if !tags.contains(&tag) {
            tags.push(tag);
        }
    }
    TailscaleDevice {
        id: node.id.unwrap_or_default(),
        name: node
            .given_name
            .filter(|name| !name.is_empty())
            .or(node.name)
            .unwrap_or_default(),
        dns_name: String::new(),
        os: TailscaleOs::Unknown,
        addresses: ipv4_first(node.ip_addresses.unwrap_or_default()),
        online: node.online,
        last_seen_at: node.last_seen.as_deref().and_then(rfc3339_micros),
        tags,
    }
}

fn map_os(os: &str) -> TailscaleOs {
    match os.to_ascii_lowercase().as_str() {
        "linux" => TailscaleOs::Linux,
        "macos" => TailscaleOs::Macos,
        "windows" => TailscaleOs::Windows,
        "ios" | "ipados" => TailscaleOs::Ios,
        "android" => TailscaleOs::Android,
        _ => TailscaleOs::Other,
    }
}

/// Unix microseconds; empty, unparsable and pre-1970 times are `None`.
fn rfc3339_micros(value: &str) -> Option<u64> {
    let time = OffsetDateTime::parse(value, &Rfc3339).ok()?;
    u64::try_from(time.unix_timestamp_nanos() / 1_000).ok()
}
