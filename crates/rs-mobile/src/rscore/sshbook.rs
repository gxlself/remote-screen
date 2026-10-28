//! SSH server book: manual-server parsing, target resolution, pinned-host
//! conversion, and tmux session naming.

use std::collections::BTreeMap;

use rs_ssh::{HostKeyEntry, SshTarget};
use serde_json::Value;
use tracing::warn;

use crate::rscore::dto_core::{
    SshAuth, SshServer, SshSource, SshTarget as SshTargetDto, TrustedHostKey,
};
use crate::rscore::store::KnownHostRow;

const DEFAULT_JUMP_PORT: u16 = 22;

/// A manually configured SSH server from `sync/servers`.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ManualServer {
    pub id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub auth: SshAuth,
    pub jump: Option<String>,
    pub network_id: Option<String>,
}

/// The SSH-relevant portion of `sync/servers`.
#[derive(Debug, Clone, Default, PartialEq, Eq)]
pub struct ServersValue {
    pub manual: Vec<ManualServer>,
    pub tailscale_ssh: BTreeMap<String, bool>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum BookError {
    #[error("unknown server {0}")]
    UnknownServer(String),
    #[error("bad jump host {0}")]
    BadJump(String),
    #[error("bad target: {0}")]
    BadTarget(&'static str),
}

/// A server-book entry resolved to the SSH transport's target shape.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ResolvedTarget {
    pub server_id: String,
    pub target: SshTarget,
    pub auth: SshAuth,
    pub tailscale_ssh: Option<bool>,
}

/// Parse the `sync/servers` value with the same tolerance as the App parser.
pub fn parse_servers_value(v: Option<&Value>) -> ServersValue {
    let Some(object) = v.and_then(Value::as_object) else {
        return ServersValue::default();
    };
    if object.get("v").and_then(Value::as_f64) != Some(1.0) {
        return ServersValue::default();
    }
    let Some(entries) = object.get("entries").and_then(Value::as_array) else {
        return ServersValue::default();
    };

    let manual = entries.iter().filter_map(parse_manual_server).collect();
    let mut tailscale_ssh = BTreeMap::new();
    if let Some(values) = object.get("tailscaleSsh").and_then(Value::as_object) {
        for (node, enabled) in values {
            if let Some(enabled) = enabled.as_bool() {
                tailscale_ssh.insert(node.clone(), enabled);
            }
        }
    }

    ServersValue {
        manual,
        tailscale_ssh,
    }
}

fn parse_manual_server(value: &Value) -> Option<ManualServer> {
    let entry = value.as_object()?;
    if entry.get("kind").and_then(Value::as_str) != Some("ssh") {
        return None;
    }

    let id = entry.get("id")?.as_str()?.to_owned();
    let name = entry.get("name")?.as_str()?.to_owned();
    let host = entry.get("host")?.as_str()?.to_owned();
    let user = entry.get("user")?.as_str()?.to_owned();
    let port = entry
        .get("port")?
        .as_u64()
        .and_then(|port| u16::try_from(port).ok())
        .filter(|port| *port != 0)?;
    let auth = parse_auth(entry.get("auth")?.as_str()?)?;
    let jump = optional_string(entry.get("jump"))?;
    let network_id = optional_string(entry.get("networkId"))?;

    Some(ManualServer {
        id,
        name,
        host,
        port,
        user,
        auth,
        jump,
        network_id,
    })
}

fn parse_auth(value: &str) -> Option<SshAuth> {
    match value {
        "tailscale" => Some(SshAuth::Tailscale),
        "deviceKey" => Some(SshAuth::DeviceKey),
        "password" => Some(SshAuth::Password),
        _ => None,
    }
}

/// Decode a nullable string field. A missing or null field is `None`; any
/// other JSON type makes the entry malformed.
fn optional_string(value: Option<&Value>) -> Option<Option<String>> {
    match value {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value.clone())),
        Some(_) => None,
    }
}

/// The manually configured SSH servers, in their `sync/servers` order.
pub fn ssh_servers(servers: &ServersValue) -> Vec<SshServer> {
    servers
        .manual
        .iter()
        .map(|manual| SshServer {
            server_id: manual.id.clone(),
            name: manual.name.clone(),
            host: manual.host.clone(),
            port: manual.port,
            user: manual.user.clone(),
            jump: manual.jump.clone(),
            auth: manual.auth,
            tailscale_ssh: None,
            os: None,
            source: SshSource::Manual,
        })
        .collect()
}

/// Resolve a book entry or an explicit `ssh.open` target to one SSH hop with
/// an optional single jump host.
pub fn resolve(
    server_id: &str,
    all: &[SshServer],
    override_target: Option<&SshTargetDto>,
) -> Result<ResolvedTarget, BookError> {
    if let Some(target) = override_target {
        validate_target(&target.host, target.port, &target.user)?;
        let jump = match target.jump.as_deref() {
            Some(jump) => resolve_jump(jump, &target.user, all)?,
            None => None,
        };
        return Ok(ResolvedTarget {
            server_id: server_id.to_owned(),
            target: SshTarget {
                host: target.host.clone(),
                port: target.port,
                user: target.user.clone(),
                jump,
            },
            auth: target.auth,
            tailscale_ssh: None,
        });
    }

    let server = all
        .iter()
        .find(|server| server.server_id == server_id)
        .ok_or_else(|| BookError::UnknownServer(server_id.to_owned()))?;
    validate_target(&server.host, server.port, &server.user)?;
    let jump = match server.jump.as_deref() {
        Some(jump) => resolve_jump(jump, &server.user, all)?,
        None => None,
    };

    Ok(ResolvedTarget {
        server_id: server.server_id.clone(),
        target: SshTarget {
            host: server.host.clone(),
            port: server.port,
            user: server.user.clone(),
            jump,
        },
        auth: server.auth,
        tailscale_ssh: server.tailscale_ssh,
    })
}

fn validate_target(host: &str, port: u16, user: &str) -> Result<(), BookError> {
    if host.is_empty() {
        return Err(BookError::BadTarget("host is empty"));
    }
    if port == 0 {
        return Err(BookError::BadTarget("port is zero"));
    }
    if user.is_empty() {
        return Err(BookError::BadTarget("user is empty"));
    }
    Ok(())
}

fn resolve_jump(
    jump: &str,
    target_user: &str,
    all: &[SshServer],
) -> Result<Option<Box<SshTarget>>, BookError> {
    let catalog = all
        .iter()
        .find(|server| server.server_id == jump)
        .or_else(|| all.iter().find(|server| server.name == jump));

    if let Some(server) = catalog {
        if server.jump.is_some() {
            warn!(
                jump = %server.server_id,
                "ignoring a jump host's own jump; only one hop is supported"
            );
        }
        validate_jump_target(&server.host, server.port, &server.user, jump)?;
        return Ok(Some(Box::new(SshTarget {
            host: server.host.clone(),
            port: server.port,
            user: server.user.clone(),
            jump: None,
        })));
    }

    let (host, port) = parse_host_port(jump, DEFAULT_JUMP_PORT)
        .ok_or_else(|| BookError::BadJump(jump.to_owned()))?;
    Ok(Some(Box::new(SshTarget {
        host,
        port,
        user: target_user.to_owned(),
        jump: None,
    })))
}

fn validate_jump_target(host: &str, port: u16, user: &str, jump: &str) -> Result<(), BookError> {
    if host.is_empty() || port == 0 || user.is_empty() {
        return Err(BookError::BadJump(jump.to_owned()));
    }
    Ok(())
}

/// Parse a host or `host:port`, including bracketed IPv6 addresses.
pub fn parse_host_port(s: &str, default_port: u16) -> Option<(String, u16)> {
    if s.is_empty() || s.chars().any(char::is_whitespace) {
        return None;
    }

    let (host, port) = if let Some(rest) = s.strip_prefix('[') {
        let (host, rest) = rest.split_once(']')?;
        let host_is_ipv6 = host.parse::<std::net::Ipv6Addr>().is_ok();
        if rest.is_empty() && host_is_ipv6 {
            (host, default_port)
        } else if host_is_ipv6 {
            let port = rest.strip_prefix(':')?;
            (host, parse_port(port)?)
        } else {
            return None;
        }
    } else if s.parse::<std::net::IpAddr>().is_ok() {
        (s, default_port)
    } else if let Some((host, port)) = s.split_once(':') {
        if host.contains(':') || host.contains(']') || host.contains('[') {
            return None;
        }
        (host, parse_port(port)?)
    } else {
        (s, default_port)
    };

    if port == 0 || !valid_host(host) {
        return None;
    }
    Some((host.to_owned(), port))
}

fn parse_port(value: &str) -> Option<u16> {
    value.parse::<u16>().ok().filter(|port| *port != 0)
}

fn valid_host(host: &str) -> bool {
    !host.is_empty() && !host.chars().any(char::is_whitespace)
}

pub fn row_to_entry(row: &KnownHostRow) -> HostKeyEntry {
    HostKeyEntry {
        algorithm: row.algorithm.clone(),
        key: row.key.clone(),
        fingerprint: row.fingerprint.clone(),
    }
}

pub fn entry_to_row(
    server_id: &str,
    host: &str,
    port: u16,
    entry: &HostKeyEntry,
    now_us: u64,
) -> KnownHostRow {
    KnownHostRow {
        server_id: server_id.to_owned(),
        host: host.to_owned(),
        port,
        algorithm: entry.algorithm.clone(),
        key: entry.key.clone(),
        fingerprint: entry.fingerprint.clone(),
        added_at_us: now_us,
    }
}

pub fn trusted_host_keys(rows: &[KnownHostRow]) -> Vec<TrustedHostKey> {
    let mut keys: Vec<_> = rows
        .iter()
        .map(|row| TrustedHostKey {
            server_id: row.server_id.clone(),
            key_type: row.algorithm.clone(),
            fp: row.fingerprint.clone(),
            added_at: row.added_at_us,
        })
        .collect();
    keys.sort_by(|left, right| left.server_id.cmp(&right.server_id));
    keys
}

/// Turn an arbitrary server ID into a valid tmux session name component.
pub fn tmux_id(server_id: &str) -> String {
    if server_id.is_empty() {
        return "server".to_owned();
    }

    let sanitized: String = server_id
        .chars()
        .map(|c| {
            if c.is_ascii_alphanumeric() || matches!(c, '_' | '-') {
                c
            } else {
                '-'
            }
        })
        .collect();
    if sanitized.len() <= 32 {
        return sanitized;
    }

    const HEX: &[u8; 16] = b"0123456789abcdef";
    let digest = ring::digest::digest(&ring::digest::SHA256, server_id.as_bytes());
    let mut suffix = String::with_capacity(8);
    for byte in digest.as_ref().iter().take(4) {
        suffix.push(HEX[usize::from(byte >> 4)] as char);
        suffix.push(HEX[usize::from(byte & 0x0f)] as char);
    }
    let prefix: String = sanitized.chars().take(23).collect();
    format!("{prefix}-{suffix}")
}
