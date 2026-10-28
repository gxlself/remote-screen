//! Pure Linux system information models.
//!
//! The functions in this module deliberately take their filesystem roots as
//! arguments.  That keeps the parsing and classification logic independent of
//! the machine on which the host process happens to run.

use std::collections::BTreeMap;
use std::fs;
use std::net::IpAddr;
use std::path::{Path, PathBuf};

use host_core::nic_model::{self, AdapterRecord};
use host_core::platform::{ConsoleState, HostInfo, Nic};
use rsp_core::pb;

/// The desktop protocol selected for the current session.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum DesktopSession {
    Wayland,
    X11,
    None,
}

/// Select a desktop session from the usual XDG environment values.
pub fn detect_session(
    session_type: Option<&str>,
    wayland_display: Option<&str>,
    display: Option<&str>,
) -> DesktopSession {
    match session_type
        .map(str::trim)
        .map(str::to_ascii_lowercase)
        .as_deref()
    {
        Some("wayland") => DesktopSession::Wayland,
        Some("x11") => DesktopSession::X11,
        _ if wayland_display.is_some_and(|value| !value.trim().is_empty()) => {
            DesktopSession::Wayland
        }
        _ if display.is_some_and(|value| !value.trim().is_empty()) => DesktopSession::X11,
        _ => DesktopSession::None,
    }
}

/// Fields read from `/etc/os-release`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct OsRelease {
    pub name: String,
    pub pretty_name: Option<String>,
    pub version_id: Option<String>,
    pub id: Option<String>,
}

/// Parse the subset of the os-release format needed by host information.
pub fn parse_os_release(text: &str) -> OsRelease {
    let mut name = None;
    let mut pretty_name = None;
    let mut version_id = None;
    let mut id = None;

    for line in text.lines() {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            continue;
        }
        let Some((key, value)) = line.split_once('=') else {
            continue;
        };
        let value = parse_release_value(value);
        match key.trim() {
            "NAME" => name = Some(value),
            "PRETTY_NAME" => pretty_name = Some(value),
            "VERSION_ID" => version_id = Some(value),
            "ID" => id = Some(value),
            _ => {}
        }
    }

    OsRelease {
        name: name.unwrap_or_else(|| "Linux".to_owned()),
        pretty_name,
        version_id,
        id,
    }
}

fn parse_release_value(raw: &str) -> String {
    let value = raw.trim();
    let bytes = value.as_bytes();
    if bytes.len() >= 2
        && ((bytes[0] == b'"' && bytes[bytes.len() - 1] == b'"')
            || (bytes[0] == b'\'' && bytes[bytes.len() - 1] == b'\''))
    {
        let inner = &value[1..value.len() - 1];
        if bytes[0] == b'\'' {
            return inner.to_owned();
        }
        return unescape_double_quoted(inner);
    }
    value.to_owned()
}

fn unescape_double_quoted(value: &str) -> String {
    let mut result = String::with_capacity(value.len());
    let mut chars = value.chars();
    while let Some(character) = chars.next() {
        if character != '\\' {
            result.push(character);
            continue;
        }
        match chars.next() {
            Some(next @ ('"' | '\\' | '$' | '`')) => result.push(next),
            Some(next) => {
                result.push('\\');
                result.push(next);
            }
            None => result.push('\\'),
        }
    }
    result
}

/// Build the user-visible OS version string.
pub fn os_version_string(os: &OsRelease, kernel_release: &str) -> String {
    let mut result = match &os.pretty_name {
        Some(pretty_name) => pretty_name.clone(),
        None => match &os.version_id {
            Some(version_id) => format!("{} {}", os.name, version_id),
            None => os.name.clone(),
        },
    };
    let kernel_release = kernel_release.trim();
    if !kernel_release.is_empty() {
        result.push_str(" (Linux ");
        result.push_str(kernel_release);
        result.push(')');
    }
    result
}

/// Read `/etc/os-release`, falling back to `/usr/lib/os-release`.
pub fn read_os_release(root: &Path) -> OsRelease {
    let etc_path = root.join("etc/os-release");
    let usr_path = root.join("usr/lib/os-release");
    let text = fs::read_to_string(&etc_path)
        .or_else(|error| {
            if error.kind() == std::io::ErrorKind::NotFound {
                fs::read_to_string(&usr_path)
            } else {
                Err(error)
            }
        })
        .unwrap_or_default();
    if text.is_empty() {
        // An empty file is still a valid os-release file, but parsing it gives
        // the required Linux default and keeps fallback behavior deterministic.
        return parse_os_release("");
    }
    parse_os_release(&text)
}

fn normalized_model_value(value: Option<&str>) -> Option<String> {
    let value = value?.trim();
    let folded = value.to_ascii_lowercase();
    let placeholders = [
        "",
        "to be filled by o.e.m.",
        "system product name",
        "system manufacturer",
        "default string",
        "none",
        "not applicable",
        "o.e.m.",
    ];
    if placeholders
        .iter()
        .any(|placeholder| *placeholder == folded)
    {
        None
    } else {
        Some(value.to_owned())
    }
}

/// Return a useful machine model while filtering common firmware placeholders.
pub fn model_name(sys_vendor: Option<&str>, product_name: Option<&str>) -> String {
    let vendor = normalized_model_value(sys_vendor);
    let product = normalized_model_value(product_name);
    match (vendor, product) {
        (Some(vendor), Some(product)) => {
            if product
                .to_ascii_lowercase()
                .starts_with(&vendor.to_ascii_lowercase())
            {
                product
            } else {
                format!("{vendor} {product}")
            }
        }
        (Some(vendor), None) => vendor,
        (None, Some(product)) => product,
        (None, None) => "Linux PC".to_owned(),
    }
}

/// Assemble the public host information record.
pub fn host_info(
    hostname: &str,
    os: &OsRelease,
    kernel_release: &str,
    sys_vendor: Option<&str>,
    product_name: Option<&str>,
    arch: &str,
) -> HostInfo {
    let name = hostname.trim();
    HostInfo {
        name: if name.is_empty() {
            "linux".to_owned()
        } else {
            name.to_owned()
        },
        os: pb::Platform::Linux,
        os_version: os_version_string(os, kernel_release),
        model: model_name(sys_vendor, product_name),
        arch: arch.to_owned(),
    }
}

pub const VENDOR_NVIDIA: u32 = 0x10DE;
pub const VENDOR_AMD: u32 = 0x1002;
pub const VENDOR_INTEL: u32 = 0x8086;

/// A DRM render node and the vendor/driver information discovered beside it.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct RenderNode {
    pub minor: u32,
    pub dev_path: PathBuf,
    pub vendor: u32,
    pub driver: Option<String>,
}

/// Parse a hexadecimal u32, with optional C-style prefix.
pub fn parse_hex_u32(text: &str) -> Option<u32> {
    let value = text.trim();
    let value = value
        .strip_prefix("0x")
        .or_else(|| value.strip_prefix("0X"))
        .unwrap_or(value);
    if value.is_empty() {
        return None;
    }
    u32::from_str_radix(value, 16).ok()
}

/// Scan DRM render nodes below a caller-provided sysfs root.
pub fn render_nodes(sys_root: &Path) -> Vec<RenderNode> {
    let drm_root = sys_root.join("class/drm");
    let Ok(entries) = fs::read_dir(drm_root) else {
        return Vec::new();
    };
    let mut nodes = Vec::new();
    for entry in entries.flatten() {
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        let Some(minor) = file_name
            .strip_prefix("renderD")
            .filter(|suffix| !suffix.is_empty())
            .and_then(|suffix| suffix.parse::<u32>().ok())
        else {
            continue;
        };

        let node_root = entry.path();
        let Ok(vendor_text) = fs::read_to_string(node_root.join("device/vendor")) else {
            continue;
        };
        let Some(vendor) = parse_hex_u32(&vendor_text) else {
            continue;
        };
        let driver = fs::read_link(node_root.join("device/driver"))
            .ok()
            .and_then(|path| {
                path.file_name()
                    .and_then(|name| name.to_str())
                    .map(str::to_owned)
            });
        nodes.push(RenderNode {
            minor,
            dev_path: PathBuf::from(format!("/dev/dri/renderD{minor}")),
            vendor,
            driver,
        });
    }
    nodes.sort_by_key(|node| node.minor);
    nodes
}

/// A logind session snapshot used to derive the host console state.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LogindSession {
    pub id: String,
    pub uid: u32,
    pub class: String,
    pub active: bool,
    pub locked_hint: bool,
}

/// Map the current user session and the active seat session to a public state.
pub fn console_state_from(
    own: Option<&LogindSession>,
    seat_active: Option<&LogindSession>,
) -> ConsoleState {
    let Some(own) = own else {
        return ConsoleState::Unknown;
    };
    if own.locked_hint {
        return ConsoleState::Locked;
    }
    if own.active {
        return ConsoleState::LoggedIn;
    }
    let Some(seat_active) = seat_active else {
        return ConsoleState::LoginWindow;
    };
    if seat_active.id == own.id {
        return ConsoleState::LoggedIn;
    }
    match seat_active.class.as_str() {
        "greeter" => ConsoleState::LoginWindow,
        "lock-screen" => ConsoleState::Locked,
        _ => ConsoleState::OtherUser,
    }
}

/// One interface address as returned by an if-addrs style enumerator.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IfAddrRecord {
    pub name: String,
    pub ip: IpAddr,
    pub prefix: u8,
    pub loopback: bool,
}

/// Convert a contiguous IPv4/IPv6 netmask into a prefix length.
pub fn prefix_from_netmask(mask: IpAddr) -> Option<u8> {
    let bytes = match mask {
        IpAddr::V4(address) => address.octets().to_vec(),
        IpAddr::V6(address) => address.octets().to_vec(),
    };
    let mut prefix = 0u8;
    let mut saw_zero = false;
    for byte in bytes {
        for bit in (0..8).rev() {
            if byte & (1 << bit) != 0 {
                if saw_zero {
                    return None;
                }
                prefix += 1;
            } else {
                saw_zero = true;
            }
        }
    }
    Some(prefix)
}

/// Parse a six-byte colon-separated MAC address.
pub fn parse_mac(text: &str) -> Option<[u8; 6]> {
    let parts = text.trim().split(':').collect::<Vec<_>>();
    if parts.len() != 6 || parts.iter().any(|part| part.len() != 2) {
        return None;
    }
    let mut mac = [0u8; 6];
    for (slot, part) in mac.iter_mut().zip(parts) {
        *slot = u8::from_str_radix(part, 16).ok()?;
    }
    Some(mac)
}

fn safe_interface_name(name: &str) -> bool {
    !name.contains('/') && !name.contains("..")
}

fn interface_oper_up(sys_root: &Path, name: &str) -> bool {
    let path = sys_root.join("class/net").join(name).join("operstate");
    match fs::read_to_string(path) {
        Ok(value) => {
            let value = value.trim();
            value.eq_ignore_ascii_case("up") || value.eq_ignore_ascii_case("unknown")
        }
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => true,
        Err(_) => false,
    }
}

fn interface_mac(sys_root: &Path, name: &str) -> Vec<u8> {
    fs::read_to_string(sys_root.join("class/net").join(name).join("address"))
        .ok()
        .and_then(|value| parse_mac(&value))
        .map_or_else(Vec::new, |mac| mac.to_vec())
}

/// Combine if-addrs records into the platform-independent adapter model.
pub fn adapter_records(addrs: &[IfAddrRecord], sys_root: &Path) -> Vec<AdapterRecord> {
    let mut records = BTreeMap::<String, AdapterRecord>::new();
    for address in addrs {
        if !safe_interface_name(&address.name) {
            continue;
        }
        let name = address.name.clone();
        let entry = records
            .entry(name.clone())
            .or_insert_with(|| AdapterRecord {
                friendly_name: name.clone(),
                description: name.clone(),
                if_type: if address.loopback {
                    nic_model::IF_TYPE_SOFTWARE_LOOPBACK
                } else {
                    6
                },
                oper_up: interface_oper_up(sys_root, &name),
                mac: interface_mac(sys_root, &name),
                unicast: Vec::new(),
            });
        if address.loopback {
            entry.if_type = nic_model::IF_TYPE_SOFTWARE_LOOPBACK;
        }
        entry.unicast.push((address.ip, address.prefix));
    }
    records.into_values().collect()
}

/// Convert if-addrs records directly to the public NIC representation.
pub fn nics(addrs: &[IfAddrRecord], sys_root: &Path) -> Vec<Nic> {
    nic_model::to_nics(&adapter_records(addrs, sys_root))
}
