//! Session, pairing, SSH, and network DTOs for the RsCore JSON boundary.
//!
//! Serialization rules:
//! - TypeScript optional fields (`x?: T`) omit the key when the Rust value is
//!   `None`.
//! - TypeScript nullable fields (`x: T | null`) always emit the key and use
//!   JSON `null` when the Rust value is `None`.
//! - Enums are serialized as camelCase strings.
//! - Session, SSH, and probe IDs cross this boundary as decimal strings even
//!   though their native representation is `u64`.
//! - Map keys are data and are never renamed.

use std::collections::BTreeMap;

use super::dto_files::FileTransfer;

use rsp_core::pb;
use serde::{de::Error as _, Deserialize, Deserializer, Serialize};

/// Format a native ID for the uniffi JSON boundary.
pub fn id_str(id: u64) -> String {
    id.to_string()
}

/// Parse a decimal ID received from the uniffi JSON boundary.
pub fn parse_id(s: &str) -> Option<u64> {
    s.parse().ok()
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum NetPath {
    TailnetDirect,
    TailnetDerp,
    Intranet,
    Lan,
    Manual,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Transport {
    Quic,
    Tcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DevicePlatform {
    Ios,
    Ipados,
    Android,
    Macos,
    Windows,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Quality {
    Auto,
    Clear,
    Smooth,
    Eco,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Role {
    Controller,
    Viewer,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Permission {
    ViewOnly,
    Control,
    Unattended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PairStep {
    Dialing,
    Verifying,
    Exchanging,
    WaitingLocal,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionPhase {
    Connecting,
    Authenticating,
    PendingApproval,
    Streaming,
    Reconnecting,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum GoodbyeReason {
    UserClosed,
    HostClosedByLocalUser,
    IdleTimeout,
    Replaced,
    Revoked,
    Background,
    HostShutdown,
    HostUpdating,
    PermissionLost,
    ProtocolError,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionCombo {
    CmdTab,
    CmdSpace,
    MissionControl,
    AltTab,
    WinKey,
    CtrlSpace,
    CmdH,
    CmdShift4,
    WinSpace,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SessionActionKind {
    Lock,
    CtrlAltDel,
    LockOnEnd,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ProbeOutcome {
    Ok,
    Timeout,
    Refused,
    FingerprintMismatch,
    NoRoute,
    NeedsVpn,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SshAuth {
    Tailscale,
    DeviceKey,
    Password,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DiscoverOutcome {
    /// Port open and the protocol signature matched (or kind "tcp").
    Open,
    /// Port open but the signature did not match (something else listens).
    Mismatch,
    /// TCP RST: the computer is up, the service is off.
    Refused,
    /// Timed out, unreachable, or the name did not resolve.
    Timeout,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DiscoverProbe {
    pub addr: String,
    pub port: u16,
    pub outcome: DiscoverOutcome,
    pub ms: u32,
    /// VNC: the server's version line, e.g. "RFB 003.889".
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub banner: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub enum SshKeyKind {
    #[serde(rename = "se-p256")]
    SeP256,
    #[serde(rename = "ed25519")]
    Ed25519,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum OpenIntent {
    Control,
    View,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ViewMode {
    Trackpad,
    Touch,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum DialState {
    Trying,
    Ok,
    Failed,
    Skipped,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ReconnectReason {
    PathLost,
    HostUpdating,
    Resume,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SshState {
    Connecting,
    Authenticating,
    Open,
    Closed,
    Failed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum HostKeyPromptKind {
    First,
    Changed,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ClipboardDirection {
    FromHost,
    ToHost,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum Codec {
    Hevc,
    H264,
}

/// Source of an SSH server entry in the app's book.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum SshSource {
    Manual,
}

/// Alias for callers that prefer the more explicit name.
pub type SshServerSource = SshSource;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CoreConfig {
    /// None selects <data_dir>/files.
    #[serde(default)]
    pub files_dir: Option<String>,
    pub data_dir: String,
    pub app_version: String,
    pub platform: DevicePlatform,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Identity {
    pub device_id: String,
    pub fp: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct PairTarget {
    pub v: u32,
    pub host_id: String,
    pub fp: String,
    pub code: String,
    pub name: String,
    pub addrs: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairTargetWire {
    v: u32,
    host_id: String,
    fp: String,
    code: String,
    name: String,
    addrs: Vec<String>,
}

impl<'de> Deserialize<'de> for PairTarget {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let wire = PairTargetWire::deserialize(deserializer)?;
        if wire.v != 1 {
            return Err(D::Error::custom("pair target version must be 1"));
        }
        Ok(Self {
            v: wire.v,
            host_id: wire.host_id,
            fp: wire.fp,
            code: wire.code,
            name: wire.name,
            addrs: wire.addrs,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize)]
#[serde(untagged)]
pub enum PairStartArg {
    Target(PairTarget),
    #[serde(rename_all = "camelCase")]
    File {
        file_id: String,
    },
    #[serde(rename_all = "camelCase")]
    CodeAddr {
        addr: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        port: Option<u16>,
        code: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Deserialize)]
#[serde(rename_all = "camelCase")]
struct PairCodeAddrWire {
    addr: String,
    #[serde(default)]
    port: Option<u16>,
    code: String,
}

impl<'de> Deserialize<'de> for PairStartArg {
    fn deserialize<D>(deserializer: D) -> Result<Self, D::Error>
    where
        D: Deserializer<'de>,
    {
        let value = serde_json::Value::deserialize(deserializer)?;
        if value.get("v").is_some() {
            serde_json::from_value(value)
                .map(Self::Target)
                .map_err(D::Error::custom)
        } else if let Some(file_id) = value.get("fileId") {
            let file_id = String::deserialize(file_id).map_err(D::Error::custom)?;
            Ok(Self::File { file_id })
        } else {
            let wire =
                serde_json::from_value::<PairCodeAddrWire>(value).map_err(D::Error::custom)?;
            Ok(Self::CodeAddr {
                addr: wire.addr,
                port: wire.port,
                code: wire.code,
            })
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "status", rename_all = "camelCase")]
pub enum PairOutcome {
    #[serde(rename_all = "camelCase")]
    Ok {
        host_id: String,
        host_name: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        permission: Option<Permission>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<NetPath>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latency_ms: Option<u32>,
    },
    #[serde(rename_all = "camelCase")]
    BadCode {
        attempts_left: u32,
    },
    Denied,
    Expired,
    #[serde(rename_all = "camelCase")]
    Locked {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_s: Option<u32>,
    },
    MethodDisabled,
    DisabledOnPath,
    Mitm,
    Cancelled,
    Timeout,
    #[serde(rename_all = "camelCase")]
    Error {
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<u32>,
        message: String,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairFileSummary {
    pub file_id: String,
    pub host_id: String,
    pub host_name: String,
    pub fp: String,
    pub addrs: Vec<String>,
    pub expires_at: u64,
    pub expired: bool,
}

/// A display a paired Host reported. `host_seen` persists it as JSON, so the
/// name and serde shape are retained for native binding compatibility.
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct HubDisplay {
    pub id: i32,
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub primary: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PairedHost {
    pub host_id: String,
    pub fp: String,
    pub name: String,
    pub addresses: Vec<String>,
    pub paired_at: u64,
    #[serde(default)]
    pub os: Option<String>,
    #[serde(default)]
    pub os_version: Option<String>,
    #[serde(default)]
    pub host_version: Option<String>,
    #[serde(default)]
    pub features: Option<u64>,
    #[serde(default)]
    pub displays: Vec<HubDisplay>,
    #[serde(default)]
    pub last_seen_at: Option<u64>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenOpts {
    pub intent: OpenIntent,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub display_id: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub quality: Option<Quality>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub mode: Option<ViewMode>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub audio: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_fps: Option<u32>,
    /// 期望的最大编码宽度（像素），夹到 640–7680；省略时 1920。电脑端用（07 · 电脑端）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub max_width: Option<u32>,
    /// 唯一的解码能力（`ClientCaps.decode`）；省略时 HEVC。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub codec: Option<Codec>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionSummary {
    pub id: String,
    pub host_id: String,
    pub host_name: String,
    pub phase: SessionPhase,
    pub role: Option<Role>,
    pub path: Option<NetPath>,
    pub transport: Option<Transport>,
    pub started_at: u64,
    pub display_id: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct Participant {
    pub device_id: String,
    pub device_name: String,
    pub user: String,
    pub path: Option<NetPath>,
    pub since: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SessionDisplay {
    pub id: u32,
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub primary: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub refresh_hz: Option<u32>,
    #[serde(default, skip_serializing_if = "std::ops::Not::not")]
    pub is_virtual: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum VdisplayStatus {
    Ok,
    Unsupported,
    NotAllowed,
    Limit,
    BadRequest,
    Failed,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct DialAttempt {
    pub path: NetPath,
    pub addr: String,
    pub state: DialState,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub ms: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshServer {
    pub server_id: String,
    pub name: String,
    pub host: String,
    pub port: u16,
    pub user: String,
    pub jump: Option<String>,
    pub auth: SshAuth,
    pub tailscale_ssh: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub os: Option<String>,
    pub source: SshSource,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshKey {
    pub key_id: String,
    pub name: String,
    pub kind: SshKeyKind,
    pub fp: String,
    pub public_key: String,
    pub created_at: u64,
    pub installed_on: Vec<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TrustedHostKey {
    pub server_id: String,
    pub key_type: String,
    pub fp: String,
    pub added_at: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub jump: Option<String>,
    pub auth: SshAuth,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SshOpenOpts {
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tmux: Option<bool>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub target: Option<SshTarget>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetIface {
    pub name: String,
    pub addrs: Vec<String>,
    pub is_vpn: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub work_profile: Option<bool>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum TailnetRoute {
    System,
    Embedded,
}

/// App 内节点的摘要。没有 authUrl：它只经 `tailnet.state` 事件和 `tailnet_status()` 给 JS，
/// 不进网络状态（网络状态会出现在诊断导出里）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct EmbeddedNodeSummary {
    /// 内嵌节点的状态串：noState / needsLogin / needsMachineAuth / stopped / starting / running / unknown / unavailable
    pub state: String,
    pub hostname: String,
    pub ips: Vec<String>,
    pub derp_region: String,
    pub derp_region_name: String,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct TailnetRouteStatus {
    /// 设置 `tailnet.mode` 的当前值：system / embedded / auto
    pub mode: String,
    /// 新连接实际走哪条
    pub active: TailnetRoute,
    /// 本构建带 App 内 tailnet，且没有报过 Unsupported
    pub available: bool,
    pub control_url_set: bool,
    /// 节点已启动（含 starting / needsLogin）时为 Some；没启动为 None（序列化为 null）
    pub embedded: Option<EmbeddedNodeSummary>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct NetworkStatus {
    pub ifaces: Vec<NetIface>,
    /// 由 `refresh_network` 填充；None 时不序列化（旧快照、旧测试不受影响）。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub tailnet: Option<TailnetRouteStatus>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum CoreEvent {
    #[serde(rename = "discover.found", rename_all = "camelCase")]
    DiscoverFound {
        scan_id: String,
        addr: String,
        port: u16,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        banner: Option<String>,
    },
    #[serde(rename = "discover.done", rename_all = "camelCase")]
    DiscoverDone {
        scan_id: String,
        scanned: u32,
        cancelled: bool,
    },
    #[serde(rename = "file.transfer", rename_all = "camelCase")]
    FileTransfer { transfer: FileTransfer },
    #[serde(rename = "file.transferRemoved", rename_all = "camelCase")]
    FileTransferRemoved { transfer_id: String },
    #[serde(rename = "session.state", rename_all = "camelCase")]
    SessionState {
        session_id: String,
        host_id: String,
        phase: SessionPhase,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<NetPath>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        transport: Option<Transport>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        derp_region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        dial: Option<Vec<DialAttempt>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        approval_timeout_s: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        resumed: Option<bool>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reconnect_reason: Option<ReconnectReason>,
    },
    #[serde(rename = "session.role", rename_all = "camelCase")]
    SessionRole {
        session_id: String,
        role: Role,
        controller: Option<Participant>,
        viewers: Vec<Participant>,
        input_paused_local: bool,
    },
    #[serde(rename = "session.stats", rename_all = "camelCase")]
    SessionStats {
        session_id: String,
        rtt_ms: u32,
        fps: u32,
        bitrate_kbps: u32,
        decode_ms: f64,
        loss_pct: f64,
        width: u32,
        height: u32,
        codec: Codec,
        path: NetPath,
        transport: Transport,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        derp_region: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        rate_reason: Option<String>,
    },
    #[serde(rename = "session.error", rename_all = "camelCase")]
    SessionError {
        session_id: String,
        host_id: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        code: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<GoodbyeReason>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        detail: Option<BTreeMap<String, String>>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        retry_after_s: Option<u32>,
    },
    #[serde(rename = "session.displays", rename_all = "camelCase")]
    SessionDisplays {
        session_id: String,
        displays: Vec<SessionDisplay>,
        current_id: u32,
    },
    #[serde(rename = "session.vdisplay", rename_all = "camelCase")]
    SessionVdisplay {
        session_id: String,
        request_id: u32,
        status: VdisplayStatus,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        display: Option<SessionDisplay>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    #[serde(rename = "session.clipboard", rename_all = "camelCase")]
    SessionClipboard {
        session_id: String,
        direction: ClipboardDirection,
        text: String,
        at: u64,
    },
    #[serde(rename = "session.idleWarning", rename_all = "camelCase")]
    SessionIdleWarning {
        session_id: String,
        seconds_left: u32,
        lock_on_end: bool,
    },
    #[serde(rename = "session.controlRequest", rename_all = "camelCase")]
    SessionControlRequest {
        session_id: String,
        requester: Participant,
        timeout_s: u32,
    },
    #[serde(rename = "pairing.progress", rename_all = "camelCase")]
    PairingProgress {
        step: PairStep,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        host_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        path: Option<NetPath>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        latency_ms: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        fp: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        timeout_s: Option<u32>,
    },
    #[serde(rename = "pairing.done", rename_all = "camelCase")]
    PairingDone { outcome: PairOutcome },
    #[serde(rename = "probe.result", rename_all = "camelCase")]
    ProbeResult {
        probe_id: String,
        host_id: Option<String>,
        path: NetPath,
        addr: String,
        port: u16,
        transport: Transport,
        outcome: ProbeOutcome,
        ms: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        reason: Option<String>,
        last: bool,
    },
    #[serde(rename = "ssh.state", rename_all = "camelCase")]
    SshState {
        ssh_id: String,
        server_id: String,
        state: SshState,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        message: Option<String>,
    },
    #[serde(rename = "ssh.hostKeyPrompt", rename_all = "camelCase")]
    SshHostKeyPrompt {
        ssh_id: String,
        server_id: String,
        key_type: String,
        fp: String,
        kind: HostKeyPromptKind,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_fp: Option<String>,
    },
    #[serde(rename = "ssh.authUrl", rename_all = "camelCase")]
    SshAuthUrl {
        ssh_id: String,
        server_id: String,
        url: String,
        user: String,
    },
    #[serde(rename = "network.changed", rename_all = "camelCase")]
    NetworkChanged { status: NetworkStatus },
    #[serde(rename = "recording.state")]
    RecordingState(super::recording::dto::RecordingStateEvent),
    #[serde(rename = "playback.state")]
    PlaybackState(super::recording::dto::PlaybackStateEvent),
}

impl CoreEvent {
    pub fn event_type(&self) -> &'static str {
        match self {
            Self::DiscoverFound { .. } => "discover.found",
            Self::DiscoverDone { .. } => "discover.done",
            Self::FileTransfer { .. } => "file.transfer",
            Self::FileTransferRemoved { .. } => "file.transferRemoved",
            Self::SessionState { .. } => "session.state",
            Self::SessionRole { .. } => "session.role",
            Self::SessionStats { .. } => "session.stats",
            Self::SessionError { .. } => "session.error",
            Self::SessionDisplays { .. } => "session.displays",
            Self::SessionVdisplay { .. } => "session.vdisplay",
            Self::SessionClipboard { .. } => "session.clipboard",
            Self::SessionIdleWarning { .. } => "session.idleWarning",
            Self::SessionControlRequest { .. } => "session.controlRequest",
            Self::PairingProgress { .. } => "pairing.progress",
            Self::PairingDone { .. } => "pairing.done",
            Self::ProbeResult { .. } => "probe.result",
            Self::SshState { .. } => "ssh.state",
            Self::SshHostKeyPrompt { .. } => "ssh.hostKeyPrompt",
            Self::SshAuthUrl { .. } => "ssh.authUrl",
            Self::NetworkChanged { .. } => "network.changed",
            Self::RecordingState(_) => "recording.state",
            Self::PlaybackState(_) => "playback.state",
        }
    }
}

pub fn net_path_from_pb(path: pb::PathKind) -> Option<NetPath> {
    match path {
        pb::PathKind::PathUnspecified => None,
        pb::PathKind::TailnetDirect => Some(NetPath::TailnetDirect),
        pb::PathKind::TailnetDerp => Some(NetPath::TailnetDerp),
        pb::PathKind::Intranet => Some(NetPath::Intranet),
        pb::PathKind::Lan => Some(NetPath::Lan),
        pb::PathKind::Manual => Some(NetPath::Manual),
    }
}

pub fn net_path_from_dial(path: &rsp_transport::dial::PathKind) -> NetPath {
    use rsp_transport::dial::PathKind;

    match path {
        PathKind::LastGood(inner) => net_path_from_dial(inner),
        PathKind::Lan => NetPath::Lan,
        PathKind::Intranet => NetPath::Intranet,
        PathKind::Tailnet => NetPath::TailnetDirect,
        PathKind::Manual => NetPath::Manual,
    }
}

pub fn transport_from_pb(transport: pb::Transport) -> Option<Transport> {
    match transport {
        pb::Transport::Unspecified => None,
        pb::Transport::Quic => Some(Transport::Quic),
        pb::Transport::Tcp => Some(Transport::Tcp),
    }
}

pub fn role_from_pb(role: pb::Role) -> Option<Role> {
    match role {
        pb::Role::Unspecified => None,
        pb::Role::Controller => Some(Role::Controller),
        pb::Role::Viewer => Some(Role::Viewer),
    }
}

pub fn quality_to_pb(quality: Quality) -> pb::Quality {
    match quality {
        Quality::Auto => pb::Quality::Auto,
        Quality::Clear => pb::Quality::Clear,
        Quality::Smooth => pb::Quality::Smooth,
        Quality::Eco => pb::Quality::Eco,
    }
}

pub fn quality_from_pb(quality: pb::Quality) -> Quality {
    match quality {
        pb::Quality::Auto => Quality::Auto,
        pb::Quality::Clear => Quality::Clear,
        pb::Quality::Smooth => Quality::Smooth,
        pb::Quality::Eco => Quality::Eco,
    }
}

pub fn goodbye_reason_from_pb(reason: pb::goodbye::Reason) -> Option<GoodbyeReason> {
    match reason {
        pb::goodbye::Reason::Unspecified => None,
        pb::goodbye::Reason::UserClosed => Some(GoodbyeReason::UserClosed),
        pb::goodbye::Reason::HostClosedByLocalUser => Some(GoodbyeReason::HostClosedByLocalUser),
        pb::goodbye::Reason::IdleTimeout => Some(GoodbyeReason::IdleTimeout),
        pb::goodbye::Reason::Replaced => Some(GoodbyeReason::Replaced),
        pb::goodbye::Reason::Revoked => Some(GoodbyeReason::Revoked),
        pb::goodbye::Reason::Background => Some(GoodbyeReason::Background),
        pb::goodbye::Reason::HostShutdown => Some(GoodbyeReason::HostShutdown),
        pb::goodbye::Reason::HostUpdating => Some(GoodbyeReason::HostUpdating),
        pb::goodbye::Reason::PermissionLost => Some(GoodbyeReason::PermissionLost),
    }
}

pub fn action_kind_to_pb(kind: SessionActionKind) -> pb::session_action::Kind {
    match kind {
        SessionActionKind::Lock => pb::session_action::Kind::LockScreen,
        SessionActionKind::CtrlAltDel => pb::session_action::Kind::CtrlAltDel,
        SessionActionKind::LockOnEnd => pb::session_action::Kind::LockOnEnd,
    }
}

pub fn phase_from_state(state: rsp_client::SessionStateKind) -> SessionPhase {
    match state {
        rsp_client::SessionStateKind::Connecting => SessionPhase::Connecting,
        rsp_client::SessionStateKind::Authenticating => SessionPhase::Authenticating,
        rsp_client::SessionStateKind::PendingApproval => SessionPhase::PendingApproval,
        rsp_client::SessionStateKind::Streaming => SessionPhase::Streaming,
        rsp_client::SessionStateKind::Reconnecting => SessionPhase::Reconnecting,
        rsp_client::SessionStateKind::Ended => SessionPhase::Ended,
    }
}

pub fn codec_from_pb(codec: pb::Codec) -> Option<Codec> {
    match codec {
        pb::Codec::Unspecified => None,
        pb::Codec::Hevc => Some(Codec::Hevc),
        pb::Codec::H264 => Some(Codec::H264),
    }
}

impl From<&pb::Participant> for Participant {
    fn from(value: &pb::Participant) -> Self {
        Self {
            device_id: value.device_id.clone(),
            device_name: value.device_name.clone(),
            user: value.user.clone(),
            path: pb::PathKind::try_from(value.path)
                .ok()
                .and_then(net_path_from_pb),
            since: value.since_us,
        }
    }
}

impl From<&pb::Display> for SessionDisplay {
    fn from(value: &pb::Display) -> Self {
        Self {
            id: value.id,
            name: value.name.clone(),
            width_px: value.width_px,
            height_px: value.height_px,
            primary: value.primary,
            refresh_hz: (value.refresh_hz != 0).then_some(value.refresh_hz),
            is_virtual: value.is_virtual,
        }
    }
}

pub fn end_reason_to_error(kind: &rsp_client::EndReason) -> (Option<u32>, Option<GoodbyeReason>) {
    match kind {
        rsp_client::EndReason::Goodbye(reason) => (
            rsp_client::goodbye_reason_error(*reason).map(|code| u32::from(code.code())),
            goodbye_reason_from_pb(*reason),
        ),
        rsp_client::EndReason::Error(code) => (Some(u32::from(code.code())), None),
        rsp_client::EndReason::Close(code) => (
            rsp_client::close_code_to_error(*code).map(|code| u32::from(code.code())),
            None,
        ),
        rsp_client::EndReason::Protocol => (None, Some(GoodbyeReason::ProtocolError)),
        rsp_client::EndReason::ConnectionLost => (None, None),
    }
}
