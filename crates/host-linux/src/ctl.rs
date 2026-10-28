//! Command-line control client for the host agent's local JSON-RPC socket.

use std::collections::{HashMap, VecDeque};
use std::path::{Path, PathBuf};
use std::time::Duration;

use host_core::ipc::{
    ApprovalRespondParams, PairingStartResult, Permission, StatusGetResult, MAX_LINE_LENGTH,
};
use serde_json::{json, Value};
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
use tokio::net::UnixStream;

#[derive(Debug, thiserror::Error)]
pub enum CtlError {
    #[error("cannot connect to the host agent ({0}); is remote-host.service running?")]
    Connect(String),
    #[error("I/O: {0}")]
    Io(String),
    #[error("{message} ({code})")]
    Rpc { code: String, message: String },
    #[error("bad message from the host agent: {0}")]
    Protocol(String),
    #[error("the host agent closed the connection")]
    Closed,
    #[error("timed out waiting for the host agent")]
    Timeout,
}

#[derive(Debug, Clone, PartialEq)]
pub struct IpcEvent {
    pub method: String,
    pub params: Value,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Incoming {
    Response {
        id: u64,
        result: Result<Value, (String, String)>,
    },
    Event(IpcEvent),
}

/// Build one newline-delimited JSON-RPC request.
pub fn request_line(id: u64, method: &str, params: Option<&Value>) -> String {
    let mut request = serde_json::Map::new();
    request.insert("jsonrpc".to_owned(), Value::String("2.0".to_owned()));
    request.insert("id".to_owned(), Value::Number(id.into()));
    request.insert("method".to_owned(), Value::String(method.to_owned()));
    if let Some(params) = params {
        request.insert("params".to_owned(), params.clone());
    }
    let mut line = Value::Object(request).to_string();
    line.push('\n');
    line
}

/// Parse one JSON-RPC response or event.
pub fn parse_line(line: &str) -> Result<Incoming, CtlError> {
    if line.trim_end_matches(['\r', '\n']).len() > MAX_LINE_LENGTH {
        return Err(CtlError::Protocol("JSON-RPC line is too long".to_owned()));
    }
    let value: Value = serde_json::from_str(line.trim_end_matches(['\r', '\n']))
        .map_err(|error| CtlError::Protocol(error.to_string()))?;
    let object = value
        .as_object()
        .ok_or_else(|| CtlError::Protocol("JSON-RPC message must be an object".to_owned()))?;
    if object.get("jsonrpc").and_then(Value::as_str) != Some("2.0") {
        return Err(CtlError::Protocol("missing jsonrpc 2.0 version".to_owned()));
    }

    if let Some(method) = object.get("method") {
        if object.contains_key("id")
            || object.contains_key("result")
            || object.contains_key("error")
        {
            return Err(CtlError::Protocol(
                "request messages are not expected".to_owned(),
            ));
        }
        let method = method
            .as_str()
            .filter(|method| !method.is_empty())
            .ok_or_else(|| CtlError::Protocol("event method is not a string".to_owned()))?;
        let params = object.get("params").cloned().unwrap_or_else(|| json!({}));
        return Ok(Incoming::Event(IpcEvent {
            method: method.to_owned(),
            params,
        }));
    }

    let id = object
        .get("id")
        .and_then(Value::as_u64)
        .ok_or_else(|| CtlError::Protocol("response id is not an unsigned integer".to_owned()))?;
    if object.contains_key("result") && object.contains_key("error") {
        return Err(CtlError::Protocol(
            "response contains both result and error".to_owned(),
        ));
    }
    if let Some(result) = object.get("result") {
        return Ok(Incoming::Response {
            id,
            result: Ok(result.clone()),
        });
    }
    if let Some(error) = object.get("error") {
        let error = error
            .as_object()
            .ok_or_else(|| CtlError::Protocol("response error is not an object".to_owned()))?;
        let numeric_code = error.get("code").and_then(Value::as_i64).ok_or_else(|| {
            CtlError::Protocol("response error code is not an integer".to_owned())
        })?;
        let message = error
            .get("message")
            .and_then(Value::as_str)
            .ok_or_else(|| CtlError::Protocol("response error message is not a string".to_owned()))?
            .to_owned();
        let code = error
            .get("data")
            .and_then(Value::as_object)
            .and_then(|data| data.get("code"))
            .and_then(Value::as_str)
            .map(ToOwned::to_owned)
            .unwrap_or_else(|| numeric_code.to_string());
        return Ok(Incoming::Response {
            id,
            result: Err((code, message)),
        });
    }
    Err(CtlError::Protocol(
        "response has neither result nor error".to_owned(),
    ))
}

pub fn default_socket() -> PathBuf {
    host_core::default_state_dir().join("agent.sock")
}

pub struct IpcClient {
    reader: BufReader<OwnedReadHalf>,
    writer: OwnedWriteHalf,
    next_id: u64,
    events: VecDeque<IpcEvent>,
    // Retained across cancelled next_event futures in tokio::select!.
    line: Vec<u8>,
    closed: bool,
}

impl IpcClient {
    pub async fn connect(path: &Path) -> Result<Self, CtlError> {
        let stream = UnixStream::connect(path)
            .await
            .map_err(|error| CtlError::Connect(error.to_string()))?;
        let (reader, writer) = stream.into_split();
        Ok(Self {
            reader: BufReader::new(reader),
            writer,
            next_id: 1,
            events: VecDeque::new(),
            line: Vec::new(),
            closed: false,
        })
    }

    async fn read_incoming(&mut self) -> Result<Incoming, CtlError> {
        if self.closed {
            return Err(CtlError::Closed);
        }
        loop {
            let available = self.reader.fill_buf().await.map_err(io_error)?;
            if available.is_empty() {
                self.closed = true;
                return Err(CtlError::Closed);
            }
            let end = available.iter().position(|byte| *byte == b'\n');
            let count = end.unwrap_or(available.len());
            if self.line.len().saturating_add(count) > MAX_LINE_LENGTH {
                self.closed = true;
                return Err(CtlError::Protocol("JSON-RPC line is too long".to_owned()));
            }
            self.line.extend_from_slice(&available[..count]);
            self.reader.consume(count + usize::from(end.is_some()));
            if end.is_some() {
                let line = std::mem::take(&mut self.line);
                let line = std::str::from_utf8(&line)
                    .map_err(|_| CtlError::Protocol("JSON-RPC line is not UTF-8".to_owned()))?;
                return parse_line(line);
            }
        }
    }

    /// Send a request and wait for its response, retaining interleaved events.
    pub async fn call(
        &mut self,
        method: &str,
        params: Option<Value>,
        timeout: Duration,
    ) -> Result<Value, CtlError> {
        if self.closed {
            return Err(CtlError::Closed);
        }
        let id = self.next_id;
        self.next_id = self
            .next_id
            .checked_add(1)
            .ok_or_else(|| CtlError::Protocol("JSON-RPC request ids exhausted".to_owned()))?;
        let line = request_line(id, method, params.as_ref());
        let wait = async {
            self.writer
                .write_all(line.as_bytes())
                .await
                .map_err(io_error)?;
            loop {
                match self.read_incoming().await? {
                    Incoming::Event(event) => self.events.push_back(event),
                    Incoming::Response {
                        id: response_id,
                        result,
                    } if response_id == id => {
                        return result.map_err(|(code, message)| CtlError::Rpc { code, message });
                    }
                    Incoming::Response { .. } => {
                        return Err(CtlError::Protocol(
                            "response id does not match request".to_owned(),
                        ));
                    }
                }
            }
        };
        match tokio::time::timeout(timeout, wait).await {
            Ok(result) => result,
            Err(_) => {
                // A write may have been interrupted or a late reply may arrive.
                // Reconnect instead of reusing a partially completed call.
                self.closed = true;
                Err(CtlError::Timeout)
            }
        }
    }

    pub async fn next_event(&mut self) -> Result<IpcEvent, CtlError> {
        if let Some(event) = self.events.pop_front() {
            return Ok(event);
        }
        match self.read_incoming().await? {
            Incoming::Event(event) => Ok(event),
            Incoming::Response { .. } => Err(CtlError::Protocol(
                "unexpected response while waiting for an event".to_owned(),
            )),
        }
    }

    pub async fn status(&mut self) -> Result<StatusGetResult, CtlError> {
        let value = self
            .call("status.get", None, Duration::from_secs(10))
            .await?;
        serde_json::from_value(value).map_err(|error| CtlError::Protocol(error.to_string()))
    }

    pub async fn pairing_start(&mut self) -> Result<PairingStartResult, CtlError> {
        let value = self
            .call("pairing.start", None, Duration::from_secs(10))
            .await?;
        serde_json::from_value(value).map_err(|error| CtlError::Protocol(error.to_string()))
    }

    pub async fn pairing_cancel(&mut self) -> Result<(), CtlError> {
        self.call("pairing.cancel", None, Duration::from_secs(10))
            .await
            .map(|_| ())
    }

    pub async fn approval_pending(&mut self) -> Result<Vec<ApprovalPrompt>, CtlError> {
        let value = self
            .call("approval.pending", None, Duration::from_secs(10))
            .await?;
        let requests = value
            .get("requests")
            .and_then(Value::as_array)
            .ok_or_else(|| {
                CtlError::Protocol("approval.pending result has no requests".to_owned())
            })?;
        Ok(requests.iter().filter_map(approval_from_value).collect())
    }

    pub async fn approval_respond(
        &mut self,
        params: &ApprovalRespondParams,
    ) -> Result<(), CtlError> {
        let value =
            serde_json::to_value(params).map_err(|error| CtlError::Protocol(error.to_string()))?;
        self.call("approval.respond", Some(value), Duration::from_secs(10))
            .await
            .map(|_| ())
    }
}

fn io_error(error: std::io::Error) -> CtlError {
    match error.kind() {
        std::io::ErrorKind::BrokenPipe | std::io::ErrorKind::ConnectionReset => CtlError::Closed,
        _ => CtlError::Io(error.to_string()),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ApprovalPrompt {
    pub request_id: String,
    pub kind: String,
    pub device_name: String,
    pub platform: Option<String>,
    pub intent: Option<String>,
    pub whois_login: Option<String>,
    pub timeout_s: u64,
    pub permission_choice: bool,
}

pub fn approval_from_value(v: &Value) -> Option<ApprovalPrompt> {
    let request_id = v.get("request_id")?.as_str()?;
    let kind = v.get("kind")?.as_str()?;
    if request_id.trim().is_empty() || kind.trim().is_empty() {
        return None;
    }
    let optional_string = |name: &str| {
        v.get(name)
            .and_then(Value::as_str)
            .map(str::trim)
            .filter(|value| !value.is_empty())
            .map(ToOwned::to_owned)
    };
    Some(ApprovalPrompt {
        request_id: request_id.to_owned(),
        kind: kind.to_owned(),
        device_name: v
            .get("device_name")
            .and_then(Value::as_str)
            .filter(|value| !value.is_empty())
            .unwrap_or("未知设备")
            .to_owned(),
        platform: optional_string("platform"),
        intent: optional_string("intent"),
        whois_login: optional_string("whois_login"),
        timeout_s: v.get("timeout_s").and_then(Value::as_u64).unwrap_or(30),
        permission_choice: v
            .get("permission_choice")
            .and_then(Value::as_bool)
            .unwrap_or(false),
    })
}

pub fn approval_title(p: &ApprovalPrompt) -> String {
    let suffix = match (p.kind.as_str(), p.intent.as_deref()) {
        ("connection", Some("control")) => "请求控制这台电脑",
        ("connection", Some("view")) => "请求查看屏幕",
        ("connection", Some("files")) => "请求传输文件",
        ("pairing", _) => "请求配对",
        ("takeover", _) => "请求接管当前会话",
        _ => "请求访问",
    };
    format!("{} {}", p.device_name, suffix)
}

pub fn approval_body(p: &ApprovalPrompt) -> String {
    let mut parts = Vec::new();
    if let Some(platform) = &p.platform {
        parts.push(format!("平台：{platform}"));
    }
    if let Some(login) = &p.whois_login {
        parts.push(format!("Tailscale 账号：{login}"));
    }
    parts.push(format!("{} 秒内不处理将自动拒绝", p.timeout_s));
    parts.join("；")
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Choice {
    Deny,
    Allow(Option<Permission>),
}

pub fn parse_choice(input: &str, permission_choice: bool) -> Option<Choice> {
    let input = input.trim().to_lowercase();
    if permission_choice {
        return match input.as_str() {
            "1" => Some(Choice::Allow(Some(Permission::ViewOnly))),
            "2" => Some(Choice::Allow(Some(Permission::Control))),
            "3" => Some(Choice::Allow(Some(Permission::Unattended))),
            "n" | "" => Some(Choice::Deny),
            _ => None,
        };
    }
    match input.as_str() {
        "y" | "yes" | "允许" => Some(Choice::Allow(None)),
        "n" | "no" | "" => Some(Choice::Deny),
        _ => None,
    }
}

pub fn respond_params(p: &ApprovalPrompt, c: Choice) -> ApprovalRespondParams {
    match c {
        Choice::Deny => ApprovalRespondParams {
            request_id: p.request_id.clone(),
            allow: false,
            permission: None,
        },
        Choice::Allow(permission) => ApprovalRespondParams {
            request_id: p.request_id.clone(),
            allow: true,
            permission,
        },
    }
}

pub fn notification_actions(p: &ApprovalPrompt) -> Vec<(&'static str, &'static str)> {
    if p.permission_choice {
        vec![
            ("allow_view", "允许查看"),
            ("allow_control", "允许控制"),
            ("deny", "拒绝"),
        ]
    } else {
        vec![("allow", "允许"), ("deny", "拒绝")]
    }
}

pub fn format_code(code: &str) -> String {
    if code.chars().count() != 6 {
        return code.to_owned();
    }
    let mut chars = code.chars();
    format!(
        "{} {}",
        chars.by_ref().take(3).collect::<String>(),
        chars.collect::<String>()
    )
}

pub fn qr_text(url: &str) -> Result<String, CtlError> {
    let code = qrcode::QrCode::new(url.as_bytes())
        .map_err(|error| CtlError::Protocol(format!("cannot encode QR URL: {error}")))?;
    Ok(code
        .render::<qrcode::render::unicode::Dense1x2>()
        .quiet_zone(true)
        .build())
}

pub fn pairing_text(r: &PairingStartResult, now_us: u64) -> String {
    let seconds = r.expires_at.saturating_sub(now_us) / 1_000_000;
    let qr = qr_text(&r.qr_url).unwrap_or_else(|_| "（二维码生成失败）".to_owned());
    format!(
        "配对码：{}\n剩余：{} 秒\n{}",
        format_code(&r.code),
        seconds,
        qr
    )
}

pub fn status_text(s: &StatusGetResult) -> String {
    let online = if s.online { "在线" } else { "离线" };
    let listening = if s.listening {
        "监听中"
    } else {
        "未监听"
    };
    format!(
        "状态：{online}\n版本：{}\n监听：{listening}\n会话：{}",
        s.version,
        s.sessions.len()
    )
}

pub fn pairing_result_text(params: &Value) -> String {
    let status = params.get("status").and_then(Value::as_u64).unwrap_or(0);
    let attempts = params.get("attempts_left").and_then(Value::as_u64);
    match status {
        1 => "配对成功".to_owned(),
        3 => "配对被拒绝".to_owned(),
        4 => attempts
            .map(|left| format!("配对码错误，还可尝试 {left} 次"))
            .unwrap_or_else(|| "配对码错误".to_owned()),
        5 => "配对码已过期".to_owned(),
        6 => "配对已锁定".to_owned(),
        _ => "配对失败".to_owned(),
    }
}

#[derive(Debug, Default)]
pub struct ApprovalBook {
    by_notification: HashMap<u32, ApprovalPrompt>,
    by_request: HashMap<String, u32>,
}

impl ApprovalBook {
    pub fn add(&mut self, p: ApprovalPrompt, notification_id: u32) {
        self.on_request_closed(&p.request_id);
        self.remove(notification_id);
        self.by_request
            .insert(p.request_id.clone(), notification_id);
        self.by_notification.insert(notification_id, p);
    }

    pub(crate) fn contains_request(&self, request_id: &str) -> bool {
        self.by_request.contains_key(request_id)
    }

    fn remove(&mut self, notification_id: u32) -> Option<ApprovalPrompt> {
        let prompt = self.by_notification.remove(&notification_id)?;
        self.by_request.remove(&prompt.request_id);
        Some(prompt)
    }

    pub fn on_action(
        &mut self,
        notification_id: u32,
        action: &str,
    ) -> Option<ApprovalRespondParams> {
        let choice = match action {
            "allow" => Choice::Allow(None),
            "allow_view" => Choice::Allow(Some(Permission::ViewOnly)),
            "allow_control" => Choice::Allow(Some(Permission::Control)),
            "deny" => Choice::Deny,
            _ => return None,
        };
        let prompt = self.remove(notification_id)?;
        Some(respond_params(&prompt, choice))
    }

    pub fn on_closed(
        &mut self,
        notification_id: u32,
        reason: u32,
    ) -> Option<ApprovalRespondParams> {
        let prompt = self.remove(notification_id)?;
        (reason == 2).then(|| respond_params(&prompt, Choice::Deny))
    }

    pub fn on_request_closed(&mut self, request_id: &str) -> Option<u32> {
        let notification_id = self.by_request.remove(request_id)?;
        self.by_notification.remove(&notification_id);
        Some(notification_id)
    }

    pub fn drain(&mut self) -> Vec<u32> {
        let mut ids: Vec<_> = self.by_notification.keys().copied().collect();
        ids.sort_unstable();
        self.by_notification.clear();
        self.by_request.clear();
        ids
    }

    pub fn len(&self) -> usize {
        self.by_notification.len()
    }

    pub fn is_empty(&self) -> bool {
        self.by_notification.is_empty()
    }
}
