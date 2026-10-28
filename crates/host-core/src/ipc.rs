//! Local JSON-RPC 2.0 service used by host-ui.
//!
//! The wire format is deliberately small: one JSON-RPC object per line. The
//! service owns transport and dispatch only; host-core subsystems provide the
//! [`IpcBackend`] implementation.

use std::fmt;
use std::future::Future;
use std::path::Path;
#[cfg(unix)]
use std::path::PathBuf;
use std::sync::Arc;

use serde::{de::DeserializeOwned, Deserialize, Serialize};
use serde_json::json;
pub use serde_json::Value;
use tokio::io::{split, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
use tokio::sync::{broadcast, mpsc};

#[cfg(unix)]
use std::os::unix::fs::PermissionsExt;
#[cfg(unix)]
use std::os::unix::net::UnixStream as StdUnixStream;
#[cfg(unix)]
use tokio::net::UnixListener;

/// Maximum encoded JSON-RPC line, excluding the trailing newline.
pub const MAX_LINE_LENGTH: usize = 1024 * 1024;
const NOTIFICATION_CAPACITY: usize = 256;

/// Permission granted to a paired device.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum Permission {
    #[default]
    ViewOnly,
    Control,
    Unattended,
}

/// Compatibility aliases used by callers that refer to the device-level
/// permission rather than the IPC field name.
pub type DevicePermission = Permission;
pub type PermissionLevel = Permission;

/// Session role displayed by host-ui.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum SessionRole {
    #[default]
    Viewer,
    Controller,
}

/// Update channel used by `update.setChannel`.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

/// A connected remote session.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionSummary {
    pub session_id: String,
    pub device_id: String,
    #[serde(default)]
    pub device_name: String,
    #[serde(default)]
    pub role: SessionRole,
    #[serde(default)]
    pub connected_at_us: u64,
    /// M3 V5：这个会话的 App 正在本地录制（RecordingNotice）。
    #[serde(default)]
    pub recording: bool,
}

/// A paired device shown in the device list.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DeviceSummary {
    pub device_id: String,
    #[serde(default)]
    pub name: String,
    #[serde(default)]
    pub permission: Permission,
    #[serde(default)]
    pub paths: Vec<String>,
    #[serde(default)]
    pub last_connected_at_us: Option<u64>,
}

/// Result returned by `status.get`.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct StatusGetResult {
    pub online: bool,
    #[serde(default)]
    pub tailnet_addresses: Vec<String>,
    #[serde(default)]
    pub version: String,
    #[serde(default)]
    pub permissions: Value,
    pub listening: bool,
    #[serde(default)]
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionsListResult {
    pub sessions: Vec<SessionSummary>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionsDisconnectParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct SessionsSetViewOnlyParams {
    pub session_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PairingStartResult {
    pub qr_url: String,
    pub code: String,
    pub expires_at: u64,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PairingCancelParams {
    #[serde(default)]
    pub code: Option<String>,
}

/// `pairing.exportFile` 参数（06「本地 IPC」）。Host 自己把文件写到 `dir`，不经 IPC 返回口令。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PairingExportFileParams {
    /// 缺省 = `control`。
    #[serde(default)]
    pub permission: Option<Permission>,
    /// ≤ 64 字符。
    #[serde(default)]
    pub label: Option<String>,
    /// 绝对路径；缺省：Mac `~/Downloads`，其他平台 `<state_dir>/exports`。
    #[serde(default)]
    pub dir: Option<String>,
}

/// `pairing.exportFile` 结果；`expires_at` 为 unix µs。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PairingExportFileResult {
    pub file_id: String,
    pub path: String,
    pub file_name: String,
    pub expires_at: u64,
}

/// 配对文件状态（`pairfiles.json` 的 `state`；`expired` 读时按时间判）。
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize, Default)]
#[serde(rename_all = "snake_case")]
pub enum PairFileState {
    #[default]
    Active,
    Used,
    Failed,
    Revoked,
    Expired,
}

/// `pairing.listFiles` 的一项；时间均为 unix µs，不含口令。
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PairFileInfo {
    pub file_id: String,
    #[serde(default)]
    pub label: Option<String>,
    pub permission: Permission,
    pub created_at: u64,
    pub expires_at: u64,
    pub state: PairFileState,
    #[serde(default)]
    pub used_by_device_id: Option<String>,
    #[serde(default)]
    pub used_at: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PairingListFilesResult {
    pub files: Vec<PairFileInfo>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct PairingRevokeFileParams {
    pub file_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DeviceListResult {
    pub devices: Vec<DeviceSummary>,
}

/// Alias using the plural spelling used in the method table.
pub type DevicesListResult = DeviceListResult;
pub type SettingsGetResult = Value;
pub type SettingsSetResult = Value;
pub type NicsListResult = NetworkListNicsResult;

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DevicesUpdateParams {
    pub device_id: String,
    pub permission: Permission,
    #[serde(default)]
    pub paths: Vec<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct DevicesRevokeParams {
    pub device_id: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NetworkListNicsResult {
    pub nics: Vec<Value>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NetworkFirewallStatusResult {
    pub enabled: bool,
    pub active: bool,
    #[serde(default)]
    pub detail: Value,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct NetworkItNoteResult {
    pub text: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct ApprovalRespondParams {
    pub request_id: String,
    pub allow: bool,
    #[serde(default)]
    pub permission: Option<Permission>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct UpdateStatusResult {
    pub state: String,
    #[serde(default)]
    pub version: Option<String>,
    #[serde(default)]
    pub notes: Option<String>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct UpdateSetChannelParams {
    pub channel: UpdateChannel,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LogsExportParams {
    pub mask: bool,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct LogsExportResult {
    pub path: String,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AuditListParams {
    #[serde(default)]
    pub limit: Option<u32>,
    #[serde(default)]
    pub before_us: Option<u64>,
}

#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct AuditListResult {
    pub entries: Vec<Value>,
}

/// Empty object returned by commands whose result is only an acknowledgement.
#[derive(Debug, Clone, Serialize, Deserialize, Default, PartialEq)]
pub struct EmptyResult {}

pub type SessionsDisconnectResult = EmptyResult;
pub type SessionsSetViewOnlyResult = EmptyResult;
pub type PairingCancelResult = EmptyResult;
pub type PairingRevokeFileResult = EmptyResult;
pub type DevicesUpdateResult = EmptyResult;
pub type DevicesRevokeResult = EmptyResult;
pub type ApprovalRespondResult = EmptyResult;
#[derive(Serialize, Deserialize, Debug, Clone, PartialEq)]
pub struct ApprovalPendingResult {
    pub requests: Vec<serde_json::Value>,
}
pub type UpdateInstallNowResult = EmptyResult;
pub type UpdateSetChannelResult = EmptyResult;

/// Errors returned by a backend. The short code is exposed in JSON-RPC's
/// `data.code`; its contents are safe for the UI to display and log.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct IpcError {
    pub code: String,
    pub message: String,
}

impl IpcError {
    pub fn new(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self {
            code: code.into(),
            message: message.into(),
        }
    }

    pub fn business(code: impl Into<String>, message: impl Into<String>) -> Self {
        Self::new(code, message)
    }

    fn internal(message: impl Into<String>) -> Self {
        Self::new("internal", message)
    }
}

impl fmt::Display for IpcError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        write!(f, "{}: {}", self.code, self.message)
    }
}

impl std::error::Error for IpcError {}

/// Backend contract for all methods in the host-ui IPC table.
pub trait IpcBackend: Send + Sync + 'static {
    fn status_get(&self) -> impl Future<Output = Result<StatusGetResult, IpcError>> + Send;
    fn sessions_list(&self) -> impl Future<Output = Result<SessionsListResult, IpcError>> + Send;
    fn sessions_disconnect(
        &self,
        params: SessionsDisconnectParams,
    ) -> impl Future<Output = Result<SessionsDisconnectResult, IpcError>> + Send;
    fn sessions_set_view_only(
        &self,
        params: SessionsSetViewOnlyParams,
    ) -> impl Future<Output = Result<SessionsSetViewOnlyResult, IpcError>> + Send;
    fn pairing_start(&self) -> impl Future<Output = Result<PairingStartResult, IpcError>> + Send;
    fn pairing_cancel(
        &self,
        params: PairingCancelParams,
    ) -> impl Future<Output = Result<PairingCancelResult, IpcError>> + Send;
    /// 业务错误码：`too_many_files`、`io`、`invalid_params`（06「本地 IPC」）。
    fn pairing_export_file(
        &self,
        _params: PairingExportFileParams,
    ) -> impl Future<Output = Result<PairingExportFileResult, IpcError>> + Send {
        async {
            Err(IpcError::business(
                "unsupported",
                "pairing.exportFile is not wired",
            ))
        }
    }
    fn pairing_list_files(
        &self,
    ) -> impl Future<Output = Result<PairingListFilesResult, IpcError>> + Send {
        async {
            Err(IpcError::business(
                "unsupported",
                "pairing.listFiles is not wired",
            ))
        }
    }
    /// 业务错误码：`not_found`。
    fn pairing_revoke_file(
        &self,
        _params: PairingRevokeFileParams,
    ) -> impl Future<Output = Result<PairingRevokeFileResult, IpcError>> + Send {
        async {
            Err(IpcError::business(
                "unsupported",
                "pairing.revokeFile is not wired",
            ))
        }
    }
    fn devices_list(&self) -> impl Future<Output = Result<DevicesListResult, IpcError>> + Send;
    fn devices_update(
        &self,
        params: DevicesUpdateParams,
    ) -> impl Future<Output = Result<DevicesUpdateResult, IpcError>> + Send;
    fn devices_revoke(
        &self,
        params: DevicesRevokeParams,
    ) -> impl Future<Output = Result<DevicesRevokeResult, IpcError>> + Send;
    fn settings_get(&self) -> impl Future<Output = Result<Value, IpcError>> + Send;
    fn settings_set(&self, settings: Value)
        -> impl Future<Output = Result<Value, IpcError>> + Send;
    fn network_list_nics(
        &self,
    ) -> impl Future<Output = Result<NetworkListNicsResult, IpcError>> + Send;
    fn network_firewall_status(
        &self,
    ) -> impl Future<Output = Result<NetworkFirewallStatusResult, IpcError>> + Send;
    fn network_it_note(&self)
        -> impl Future<Output = Result<NetworkItNoteResult, IpcError>> + Send;
    fn approval_pending(
        &self,
    ) -> impl Future<Output = Result<ApprovalPendingResult, IpcError>> + Send {
        async {
            Err(IpcError::business(
                "unsupported",
                "approval.pending is not wired",
            ))
        }
    }
    fn approval_respond(
        &self,
        params: ApprovalRespondParams,
    ) -> impl Future<Output = Result<ApprovalRespondResult, IpcError>> + Send;
    fn update_status(&self) -> impl Future<Output = Result<UpdateStatusResult, IpcError>> + Send;
    fn update_install_now(
        &self,
    ) -> impl Future<Output = Result<UpdateInstallNowResult, IpcError>> + Send;
    fn update_set_channel(
        &self,
        params: UpdateSetChannelParams,
    ) -> impl Future<Output = Result<UpdateSetChannelResult, IpcError>> + Send;
    fn logs_export(
        &self,
        params: LogsExportParams,
    ) -> impl Future<Output = Result<LogsExportResult, IpcError>> + Send;
    fn audit_list(
        &self,
        params: AuditListParams,
    ) -> impl Future<Output = Result<AuditListResult, IpcError>> + Send;
}

/// Event payloads are JSON values so producers can evolve their schemas
/// without changing this transport layer.
#[derive(Debug, Clone, PartialEq)]
pub enum Event {
    /// Connection requests also carry intent: "control" | "view" | "files".
    ApprovalRequested(Value),
    ApprovalClosed(Value),
    SessionsChanged(Value),
    /// `{status, host_id, host_name, attempts_left, device_fp}`，另有（06「本地 IPC」）：
    /// `method`（`"code"` / `"file"` / `"password"` / `"approve"`）、`device_name`；
    /// `method = "password"` 且 OK 时带 `account`（账户短名）。
    PairingResult(Value),
    UpdateState(Value),
    PermissionsChanged(Value),
    Handoff(Value),
}

impl Event {
    fn method(&self) -> &'static str {
        match self {
            Self::ApprovalRequested(_) => "event.approvalRequested",
            Self::ApprovalClosed(_) => "event.approvalClosed",
            Self::SessionsChanged(_) => "event.sessionsChanged",
            Self::PairingResult(_) => "event.pairingResult",
            Self::UpdateState(_) => "event.updateState",
            Self::PermissionsChanged(_) => "event.permissionsChanged",
            Self::Handoff(_) => "event.handoff",
        }
    }

    fn params(&self) -> &Value {
        match self {
            Self::ApprovalRequested(value)
            | Self::ApprovalClosed(value)
            | Self::SessionsChanged(value)
            | Self::PairingResult(value)
            | Self::UpdateState(value)
            | Self::PermissionsChanged(value)
            | Self::Handoff(value) => value,
        }
    }
}

/// Handle used by producers to broadcast events to connected clients.
#[derive(Clone)]
pub struct Notifier {
    sender: broadcast::Sender<Event>,
}

impl Notifier {
    pub(crate) fn from_sender(sender: broadcast::Sender<Event>) -> Self {
        Self { sender }
    }

    /// Queues an event for every connected client. A client that cannot keep
    /// up with 256 queued events is disconnected by its writer task.
    pub fn send(&self, event: Event) -> Result<usize, Event> {
        self.sender.send(event).map_err(|error| error.0)
    }
}

/// JSON-RPC server. It is generic over the backend so no `dyn` async trait is
/// needed and each application can keep its own concrete state.
pub struct IpcServer<B: IpcBackend> {
    #[cfg_attr(not(unix), allow(dead_code))]
    backend: Arc<B>,
    notifier: broadcast::Sender<Event>,
    #[cfg(unix)]
    listener: UnixListener,
    #[cfg(unix)]
    path: PathBuf,
}

impl<B: IpcBackend> IpcServer<B> {
    /// Binds the Unix socket and enforces owner-only permissions (0600).
    #[cfg(unix)]
    pub fn bind(path: impl AsRef<Path>, backend: B) -> Result<Self, IpcError> {
        let path = path.as_ref().to_path_buf();
        let listener = match UnixListener::bind(&path) {
            Ok(listener) => listener,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse => {
                // A stale socket is common after an unclean agent shutdown.
                // A live listener is never removed.
                match StdUnixStream::connect(&path) {
                    Ok(_) => {
                        return Err(IpcError::new(
                            "address_in_use",
                            "IPC socket is already serving",
                        ));
                    }
                    Err(_) => {
                        std::fs::remove_file(&path).map_err(|remove_error| {
                            IpcError::new("bind_failed", remove_error.to_string())
                        })?;
                        UnixListener::bind(&path).map_err(|bind_error| {
                            IpcError::new("bind_failed", bind_error.to_string())
                        })?
                    }
                }
            }
            Err(error) => {
                return Err(IpcError::new("bind_failed", error.to_string()));
            }
        };
        let mut permissions = std::fs::metadata(&path)
            .map_err(|error| IpcError::new("bind_failed", error.to_string()))?
            .permissions();
        permissions.set_mode(0o600);
        std::fs::set_permissions(&path, permissions)
            .map_err(|error| IpcError::new("bind_failed", error.to_string()))?;
        let (notifier, _) = broadcast::channel(NOTIFICATION_CAPACITY);
        Ok(Self {
            backend: Arc::new(backend),
            notifier,
            listener,
            path,
        })
    }

    pub fn notifier(&self) -> Notifier {
        Notifier {
            sender: self.notifier.clone(),
        }
    }

    /// Accepts clients until the listener fails. Each client has an
    /// independent task, so disconnects and backend errors are isolated.
    #[cfg(unix)]
    pub async fn serve(&self) -> Result<(), IpcError> {
        loop {
            let (stream, _) = self
                .listener
                .accept()
                .await
                .map_err(|error| IpcError::new("accept_failed", error.to_string()))?;
            let backend = Arc::clone(&self.backend);
            let events = self.notifier.subscribe();
            tokio::spawn(async move {
                if let Err(error) = handle_client(stream, backend, events).await {
                    tracing::debug!(error = %error, "IPC client disconnected");
                }
            });
        }
    }
}

#[cfg(unix)]
impl<B: IpcBackend> Drop for IpcServer<B> {
    fn drop(&mut self) {
        let _ = std::fs::remove_file(&self.path);
    }
}

#[derive(Debug, Deserialize)]
struct RequestEnvelope {
    #[serde(default)]
    jsonrpc: Option<String>,
    #[serde(default)]
    method: Option<String>,
    #[serde(default)]
    params: Option<Value>,
    #[serde(default)]
    id: Option<Value>,
}

#[derive(Debug, Serialize)]
struct ResponseEnvelope {
    jsonrpc: &'static str,
    #[serde(skip_serializing_if = "Option::is_none")]
    id: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    result: Option<Value>,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<ResponseError>,
}

#[derive(Debug, Serialize)]
struct ResponseError {
    code: i32,
    message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    data: Option<Value>,
}

fn response_result(id: Value, result: Value) -> ResponseEnvelope {
    ResponseEnvelope {
        jsonrpc: "2.0",
        id: Some(id),
        result: Some(result),
        error: None,
    }
}

fn response_error(id: Option<Value>, code: i32, message: impl Into<String>) -> ResponseEnvelope {
    response_error_data(id, code, message, None)
}

fn response_error_data(
    id: Option<Value>,
    code: i32,
    message: impl Into<String>,
    data: Option<Value>,
) -> ResponseEnvelope {
    ResponseEnvelope {
        jsonrpc: "2.0",
        id,
        result: None,
        error: Some(ResponseError {
            code,
            message: message.into(),
            data,
        }),
    }
}

fn encode_response(response: &ResponseEnvelope) -> Result<String, IpcError> {
    serde_json::to_string(response).map_err(|error| IpcError::internal(error.to_string()))
}

fn encode_event(event: &Event) -> Result<String, IpcError> {
    serde_json::to_string(&json!({
        "jsonrpc": "2.0",
        "method": event.method(),
        "params": event.params(),
    }))
    .map_err(|error| IpcError::internal(error.to_string()))
}

pub(crate) async fn handle_client<B, S>(
    stream: S,
    backend: Arc<B>,
    mut events: broadcast::Receiver<Event>,
) -> Result<(), IpcError>
where
    B: IpcBackend,
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (reader, writer) = split(stream);
    let (out_tx, mut out_rx) = mpsc::channel::<String>(NOTIFICATION_CAPACITY);
    let writer_task =
        tokio::spawn(async move { write_client(writer, &mut out_rx, &mut events).await });
    let mut reader = BufReader::new(reader);
    let mut line = String::new();
    loop {
        line.clear();
        let bytes = reader
            .read_line(&mut line)
            .await
            .map_err(|error| IpcError::new("read_failed", error.to_string()))?;
        if bytes == 0 {
            break;
        }
        if line.trim_end_matches(['\r', '\n']).len() > MAX_LINE_LENGTH {
            let response = encode_response(&response_error(
                Some(Value::Null),
                -32600,
                "request line too long",
            ))?;
            let _ = out_tx.send(format!("{response}\n")).await;
            break;
        }
        let request_value = match serde_json::from_str::<Value>(line.trim()) {
            Ok(value) => value,
            Err(_) => {
                let response =
                    encode_response(&response_error(Some(Value::Null), -32700, "parse error"))?;
                let _ = out_tx.send(format!("{response}\n")).await;
                continue;
            }
        };
        let request = if request_value.is_object() {
            match serde_json::from_value::<RequestEnvelope>(request_value) {
                Ok(request) => request,
                Err(_) => {
                    let response = encode_response(&response_error(
                        Some(Value::Null),
                        -32600,
                        "invalid request",
                    ))?;
                    let _ = out_tx.send(format!("{response}\n")).await;
                    continue;
                }
            }
        } else {
            let response = encode_response(&response_error(
                Some(Value::Null),
                -32600,
                "invalid request",
            ))?;
            let _ = out_tx.send(format!("{response}\n")).await;
            continue;
        };
        let mut request = request;
        if request.id.is_none()
            && (request.jsonrpc.as_deref() != Some("2.0") || request.method.is_none())
        {
            // An invalid request is not a JSON-RPC notification; report it
            // with the required null id even when the malformed object had no
            // usable id field.
            request.id = Some(Value::Null);
        }
        let id = request.id.clone();
        let response = dispatch_request(&*backend, request).await;
        if let Some(id) = id {
            let encoded = encode_response(&match response {
                Ok(value) => response_result(id, value),
                Err(error) => error,
            })?;
            if out_tx.send(format!("{encoded}\n")).await.is_err() {
                break;
            }
        }
    }
    drop(out_tx);
    match writer_task.await {
        Ok(result) => result,
        Err(error) => Err(IpcError::new("write_failed", error.to_string())),
    }
}

pub(crate) async fn write_client<W: AsyncWrite + Unpin>(
    mut writer: W,
    out_rx: &mut mpsc::Receiver<String>,
    events: &mut broadcast::Receiver<Event>,
) -> Result<(), IpcError> {
    loop {
        tokio::select! {
            response = out_rx.recv() => {
                match response {
                    Some(response) => writer.write_all(response.as_bytes()).await
                        .map_err(|error| IpcError::new("write_failed", error.to_string()))?,
                    None => return Ok(()),
                }
            }
            event = events.recv() => {
                let event = match event {
                    Ok(event) => event,
                    Err(broadcast::error::RecvError::Lagged(_)) => {
                        return Err(IpcError::new("client_too_slow", "notification queue exceeded 256 events"));
                    }
                    Err(broadcast::error::RecvError::Closed) => return Ok(()),
                };
                let encoded = encode_event(&event)?;
                writer.write_all(encoded.as_bytes()).await
                    .map_err(|error| IpcError::new("write_failed", error.to_string()))?;
                writer.write_all(b"\n").await
                    .map_err(|error| IpcError::new("write_failed", error.to_string()))?;
            }
        }
    }
}

/// `dispatch_request` 认识的全部方法名（06「本地 IPC」方法表）。加方法时同时改这里、
/// 分发分支和 `tests/ipc/` 样例；`tests/ipc.rs` 比较样例集合与本表。
pub const METHOD_NAMES: &[&str] = &[
    "status.get",
    "sessions.list",
    "sessions.disconnect",
    "sessions.setViewOnly",
    "pairing.start",
    "pairing.cancel",
    "pairing.exportFile",
    "pairing.listFiles",
    "pairing.revokeFile",
    "devices.list",
    "devices.update",
    "devices.revoke",
    "settings.get",
    "settings.set",
    "network.listNics",
    "network.firewallStatus",
    "network.itNote",
    "approval.pending",
    "approval.respond",
    "update.status",
    "update.installNow",
    "update.setChannel",
    "logs.export",
    "audit.list",
];

async fn dispatch_request<B: IpcBackend>(
    backend: &B,
    request: RequestEnvelope,
) -> Result<Value, ResponseEnvelope> {
    if request.jsonrpc.as_deref() != Some("2.0") {
        return Err(response_error(request.id, -32600, "invalid request"));
    }
    let method = match request.method {
        Some(method) => method,
        None => return Err(response_error(request.id, -32600, "invalid request")),
    };
    let params = request.params.unwrap_or(Value::Null);
    macro_rules! call {
        ($future:expr) => {
            match $future.await {
                Ok(value) => serde_json::to_value(value)
                    .map_err(|_| response_error(request.id.clone(), -32603, "internal error")),
                Err(error) => Err(response_error_data(
                    request.id.clone(),
                    -32000,
                    error.message,
                    Some(json!({"code": error.code})),
                )),
            }
        };
    }
    macro_rules! parse_and_call {
        ($type:ty, $method:ident) => {{
            let parsed = match parse_params::<$type>(params.clone()) {
                Ok(value) => value,
                Err(error) => return Err(response_error(request.id.clone(), -32602, error)),
            };
            call!(backend.$method(parsed))
        }};
    }
    macro_rules! no_params {
        () => {
            if !params.is_null() && !params.as_object().is_some_and(|object| object.is_empty()) {
                return Err(response_error(request.id.clone(), -32602, "invalid params"));
            }
        };
    }
    match method.as_str() {
        "status.get" => {
            no_params!();
            call!(backend.status_get())
        }
        "sessions.list" => {
            no_params!();
            call!(backend.sessions_list())
        }
        "sessions.disconnect" => parse_and_call!(SessionsDisconnectParams, sessions_disconnect),
        "sessions.setViewOnly" => {
            parse_and_call!(SessionsSetViewOnlyParams, sessions_set_view_only)
        }
        "pairing.start" => {
            no_params!();
            call!(backend.pairing_start())
        }
        "pairing.cancel" => parse_and_call!(PairingCancelParams, pairing_cancel),
        "pairing.exportFile" => parse_and_call!(PairingExportFileParams, pairing_export_file),
        "pairing.listFiles" => {
            no_params!();
            call!(backend.pairing_list_files())
        }
        "pairing.revokeFile" => parse_and_call!(PairingRevokeFileParams, pairing_revoke_file),
        "devices.list" => {
            no_params!();
            call!(backend.devices_list())
        }
        "devices.update" => parse_and_call!(DevicesUpdateParams, devices_update),
        "devices.revoke" => parse_and_call!(DevicesRevokeParams, devices_revoke),
        "settings.get" => {
            no_params!();
            call!(backend.settings_get())
        }
        "settings.set" => {
            if !params.is_object() {
                return Err(response_error(
                    request.id,
                    -32602,
                    "settings must be an object",
                ));
            }
            call!(backend.settings_set(params))
        }
        "network.listNics" => {
            no_params!();
            call!(backend.network_list_nics())
        }
        "network.firewallStatus" => {
            no_params!();
            call!(backend.network_firewall_status())
        }
        "network.itNote" => {
            no_params!();
            call!(backend.network_it_note())
        }
        "approval.pending" => {
            no_params!();
            call!(backend.approval_pending())
        }
        "approval.respond" => parse_and_call!(ApprovalRespondParams, approval_respond),
        "update.status" => {
            no_params!();
            call!(backend.update_status())
        }
        "update.installNow" => {
            no_params!();
            call!(backend.update_install_now())
        }
        "update.setChannel" => parse_and_call!(UpdateSetChannelParams, update_set_channel),
        "logs.export" => parse_and_call!(LogsExportParams, logs_export),
        "audit.list" => parse_and_call!(AuditListParams, audit_list),
        _ => Err(response_error(request.id, -32601, "method not found")),
    }
}

fn parse_params<T: DeserializeOwned>(params: Value) -> Result<T, String> {
    let params = if params.is_null() { json!({}) } else { params };
    serde_json::from_value(params).map_err(|_| "invalid params".to_owned())
}
