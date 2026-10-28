//! JSON types shared with `tailnet/internal/node`.

use std::{fmt, net::IpAddr, path::PathBuf};

use serde::{Deserialize, Serialize};

#[derive(Clone, Default, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TailnetConfig {
    pub dir: PathBuf,
    pub hostname: String,
    pub control_url: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub auth_key: Option<String>,
    pub ephemeral: bool,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub log_file: Option<PathBuf>,
    pub verbose: bool,
    pub disable_netns: bool,
    pub udp_idle_seconds: u32,
}

impl fmt::Debug for TailnetConfig {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TailnetConfig")
            .field("dir", &self.dir)
            .field("hostname", &self.hostname)
            .field("control_url", &self.control_url)
            .field("auth_key", &self.auth_key.as_ref().map(|_| "<redacted>"))
            .field("ephemeral", &self.ephemeral)
            .field("log_file", &self.log_file)
            .field("verbose", &self.verbose)
            .field("disable_netns", &self.disable_netns)
            .field("udp_idle_seconds", &self.udp_idle_seconds)
            .finish()
    }
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct TailnetStatus {
    pub state: BackendState,
    pub auth_url: Option<String>,
    #[serde(rename = "self")]
    pub self_node: Option<SelfNode>,
    pub magic_dns_suffix: String,
    pub peers: Vec<PeerInfo>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct SelfNode {
    pub hostname: String,
    pub dns_name: String,
    pub stable_id: String,
    pub ips: Vec<IpAddr>,
    pub online: bool,
    pub derp_region: String,
    pub derp_region_name: String,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default, rename_all = "camelCase")]
pub struct PeerInfo {
    pub hostname: String,
    pub dns_name: String,
    pub ips: Vec<IpAddr>,
    pub os: String,
    pub online: bool,
    pub tags: Vec<String>,
    pub direct: bool,
    pub cur_addr: String,
    pub relay: String,
    pub peer_relay: String,
    pub last_handshake_us: u64,
    pub rx_bytes: u64,
    pub tx_bytes: u64,
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq, Serialize, Deserialize)]
pub enum BackendState {
    #[default]
    NoState,
    NeedsLogin,
    NeedsMachineAuth,
    Stopped,
    Starting,
    Running,
    #[serde(other)]
    Unknown,
}

impl BackendState {
    pub fn as_camel(&self) -> &'static str {
        match self {
            Self::NoState => "noState",
            Self::NeedsLogin => "needsLogin",
            Self::NeedsMachineAuth => "needsMachineAuth",
            Self::Stopped => "stopped",
            Self::Starting => "starting",
            Self::Running => "running",
            Self::Unknown => "unknown",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "type", rename_all = "camelCase")]
pub enum TailnetEvent {
    State {
        #[serde(default)]
        state: BackendState,
        #[serde(default, rename = "authUrl")]
        auth_url: Option<String>,
    },
    AuthUrl {
        #[serde(default)]
        url: String,
    },
    Netmap,
    Error {
        #[serde(default)]
        message: String,
    },
    Stopped,
    #[serde(other)]
    Unknown,
}
