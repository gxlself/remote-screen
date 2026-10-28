//! Line-delimited JSON-RPC client. The transport only depends on [`EventSink`];
//! the Tauri adapter at the end of this module handles application integration.

use std::collections::HashMap;
use std::io;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use serde::{Deserialize, Serialize};
use serde_json::{json, Value};
use tokio::io::{AsyncBufRead, AsyncBufReadExt, AsyncRead, AsyncWrite, AsyncWriteExt, BufReader};
#[cfg(unix)]
use tokio::net::UnixStream;
use tokio::sync::{mpsc, oneshot};
use tokio::task::JoinHandle;

use crate::pipe_trust::RejectLog;

const MAX_LINE_LENGTH: usize = 1024 * 1024;
const WRITE_QUEUE_CAPACITY: usize = 128;

pub trait EventSink: Send + Sync + 'static {
    fn event(&self, method: &str, params: Value);
    fn connection(&self, connected: bool);
}

pub struct BridgeConfig {
    /// Unix socket path; on Windows, this is a local named-pipe name.
    pub socket_path: PathBuf,
    pub call_timeout: Duration,
    pub backoff_min: Duration,
    pub backoff_max: Duration,
}

impl Default for BridgeConfig {
    fn default() -> Self {
        Self {
            socket_path: default_socket_path(),
            call_timeout: Duration::from_secs(15),
            backoff_min: Duration::from_millis(500),
            backoff_max: Duration::from_secs(5),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize, thiserror::Error)]
#[error("{message}")]
pub struct RpcError {
    pub code: i64,
    pub message: String,
    #[serde(skip_serializing_if = "Option::is_none")]
    pub data: Option<Value>,
}

impl RpcError {
    fn unavailable() -> Self {
        Self {
            code: -32099,
            message: "agent not connected".into(),
            data: Some(json!({"code": "agent_unavailable"})),
        }
    }

    fn timeout() -> Self {
        Self {
            code: -32098,
            message: "agent timeout".into(),
            data: Some(json!({"code": "agent_timeout"})),
        }
    }

    fn invalid_request() -> Self {
        Self {
            code: -32600,
            message: "invalid request".into(),
            data: None,
        }
    }
}

type RpcResult = Result<Value, RpcError>;

struct Outbound {
    id: u64,
    line: Vec<u8>,
}

#[derive(Default)]
struct ConnectionState {
    writer: Option<mpsc::Sender<Outbound>>,
    pending: HashMap<u64, oneshot::Sender<RpcResult>>,
    next_id: u64,
}

#[derive(Default)]
struct Shared {
    state: Mutex<ConnectionState>,
}

impl Shared {
    fn state(&self) -> MutexGuard<'_, ConnectionState> {
        self.state.lock().unwrap_or_else(|error| error.into_inner())
    }

    /// Clear the writer and drain waiters under the same lock as call admission.
    /// A request can never register with a connection that has been retired.
    fn disconnect(&self) -> bool {
        let (was_connected, pending) = {
            let mut state = self.state();
            (
                state.writer.take().is_some(),
                std::mem::take(&mut state.pending),
            )
        };
        for (_, reply) in pending {
            let _ = reply.send(Err(RpcError::unavailable()));
        }
        was_connected
    }
}

/// Owns the reconnect task. Dropping the bridge closes its socket and stops
/// reconnecting; the external agent is never started or stopped by this client.
pub struct Bridge {
    shared: Arc<Shared>,
    call_timeout: Duration,
    task: JoinHandle<()>,
}

impl Bridge {
    /// Must be called inside a Tokio runtime.
    pub fn spawn(config: BridgeConfig, sink: Arc<dyn EventSink>) -> Self {
        let shared = Arc::new(Shared::default());
        let task = tokio::spawn(connect_loop(
            config.socket_path,
            config.backoff_min,
            config.backoff_max,
            Arc::clone(&shared),
            sink,
        ));
        Self {
            shared,
            call_timeout: config.call_timeout,
            task,
        }
    }

    pub async fn call(&self, method: &str, params: Value) -> RpcResult {
        if method.is_empty() || method.len() > 64 {
            return Err(RpcError::invalid_request());
        }

        let (reply, receiver) = oneshot::channel();
        let (id, writer) = {
            let mut state = self.shared.state();
            let writer = state.writer.clone().ok_or_else(RpcError::unavailable)?;
            // Preserve uniqueness even if the u64 counter eventually wraps.
            while state.pending.contains_key(&state.next_id) {
                state.next_id = state.next_id.wrapping_add(1);
            }
            let id = state.next_id;
            state.next_id = state.next_id.wrapping_add(1);
            state.pending.insert(id, reply);
            (id, writer)
        };
        let _pending = PendingCall {
            shared: &self.shared,
            id,
        };

        let mut request = json!({"jsonrpc": "2.0", "id": id, "method": method});
        if !params.is_null() {
            request["params"] = params;
        }
        let mut line = request.to_string().into_bytes();
        line.push(b'\n');

        // Include queue backpressure in the deadline, not just agent response time.
        match tokio::time::timeout(self.call_timeout, async {
            writer
                .send(Outbound { id, line })
                .await
                .map_err(|_| RpcError::unavailable())?;
            receiver
                .await
                .unwrap_or_else(|_| Err(RpcError::unavailable()))
        })
        .await
        {
            Ok(result) => result,
            Err(_) => Err(RpcError::timeout()),
        }
    }

    pub fn is_connected(&self) -> bool {
        self.shared.state().writer.is_some()
    }
}

impl Drop for Bridge {
    fn drop(&mut self) {
        self.task.abort();
        self.shared.disconnect();
    }
}

/// Also removes waiters when the caller cancels its future before the timeout.
struct PendingCall<'a> {
    shared: &'a Shared,
    id: u64,
}

impl Drop for PendingCall<'_> {
    fn drop(&mut self) {
        self.shared.state().pending.remove(&self.id);
    }
}

type Transport = (
    Box<dyn AsyncRead + Send + Unpin>,
    Box<dyn AsyncWrite + Send + Unpin>,
);

async fn open_transport(path: &Path, log: &mut RejectLog) -> Option<Transport> {
    #[cfg(unix)]
    {
        let _ = log;
        match UnixStream::connect(path).await {
            Ok(stream) => {
                let (reader, writer) = stream.into_split();
                Some((Box::new(reader), Box::new(writer)))
            }
            Err(_) => {
                tracing::debug!("IPC connection failed");
                None
            }
        }
    }
    #[cfg(windows)]
    {
        match crate::win_pipe::connect(&path.to_string_lossy()).await {
            Ok(client) => {
                log.reset();
                let (reader, writer) = tokio::io::split(client);
                Some((Box::new(reader), Box::new(writer)))
            }
            Err(crate::win_pipe::ConnectError::Untrusted(reason)) => {
                if log.should_warn(&reason) {
                    tracing::warn!(reason = %reason, "refusing untrusted agent pipe");
                }
                None
            }
            Err(crate::win_pipe::ConnectError::Io(error)) => {
                tracing::debug!(error = %error, "IPC connection failed");
                None
            }
        }
    }
    #[cfg(not(any(unix, windows)))]
    {
        let _ = (path, log);
        None
    }
}

async fn connect_loop(
    path: PathBuf,
    backoff_min: Duration,
    backoff_max: Duration,
    shared: Arc<Shared>,
    sink: Arc<dyn EventSink>,
) {
    let mut backoff = backoff_min;
    let mut log = RejectLog::default();
    loop {
        if let Some((reader, writer)) = open_transport(&path, &mut log).await {
            let (sender, receiver) = mpsc::channel(WRITE_QUEUE_CAPACITY);
            shared.state().writer = Some(sender);
            sink.connection(true);
            backoff = backoff_min;

            // Each connection owns exactly one reader and one writer future.
            // Dropping the other branch closes both halves on any I/O error.
            tokio::select! {
                _ = read_messages(reader, &shared, sink.as_ref()) => {},
                _ = write_messages(writer, receiver, &shared) => {},
            }
        }
        if shared.disconnect() {
            sink.connection(false);
        }
        tokio::time::sleep(backoff).await;
        backoff = backoff.saturating_mul(2).min(backoff_max);
    }
}

async fn write_messages<W: AsyncWrite + Unpin>(
    mut writer: W,
    mut receiver: mpsc::Receiver<Outbound>,
    shared: &Shared,
) -> io::Result<()> {
    while let Some(outbound) = receiver.recv().await {
        if shared.state().pending.contains_key(&outbound.id) {
            writer.write_all(&outbound.line).await?;
        }
    }
    Ok(())
}

async fn read_messages<R: AsyncRead + Unpin>(
    reader: R,
    shared: &Shared,
    sink: &dyn EventSink,
) -> io::Result<()> {
    let mut reader = BufReader::new(reader);
    let mut line = Vec::new();
    while read_line_limited(&mut reader, &mut line).await? {
        match serde_json::from_slice::<Value>(&line) {
            Ok(message) => dispatch_message(message, shared, sink),
            Err(_) => tracing::debug!(length = line.len(), "bad IPC line"),
        }
    }
    Ok(())
}

/// Bound allocation while reading, including a peer that never sends a newline.
async fn read_line_limited<R: AsyncBufRead + Unpin>(
    reader: &mut R,
    line: &mut Vec<u8>,
) -> io::Result<bool> {
    line.clear();
    loop {
        let available = reader.fill_buf().await?;
        if available.is_empty() {
            return Ok(false);
        }
        let newline = available.iter().position(|byte| *byte == b'\n');
        let length = newline.unwrap_or(available.len());
        if line.len() + length > MAX_LINE_LENGTH {
            tracing::debug!("IPC line exceeds maximum length");
            return Err(io::Error::new(
                io::ErrorKind::InvalidData,
                "IPC line too long",
            ));
        }
        line.extend_from_slice(&available[..length]);
        reader.consume(length + usize::from(newline.is_some()));
        if newline.is_some() {
            return Ok(true);
        }
    }
}

fn dispatch_message(message: Value, shared: &Shared, sink: &dyn EventSink) {
    let Some(object) = message.as_object() else {
        return;
    };
    if let Some(id) = object.get("id") {
        let Some(id) = id.as_u64() else { return };
        let result = match (object.get("result"), object.get("error")) {
            (Some(result), None) => Ok(result.clone()),
            (None, Some(error)) => {
                let Ok(error) = serde_json::from_value::<RpcError>(error.clone()) else {
                    return;
                };
                Err(error)
            }
            _ => return,
        };
        let reply = shared.state().pending.remove(&id);
        if let Some(reply) = reply {
            let _ = reply.send(result);
        }
    } else if let Some(method) = object.get("method").and_then(Value::as_str) {
        sink.event(method, object.get("params").cloned().unwrap_or(Value::Null));
    }
}

#[cfg(unix)]
pub fn default_socket_path() -> PathBuf {
    if let Some(path) = std::env::var_os("RS_AGENT_SOCK") {
        return PathBuf::from(path);
    }
    PathBuf::from(std::env::var_os("HOME").unwrap_or_default())
        .join("Library/Application Support/Remote Host/agent.sock")
}

#[cfg(windows)]
pub fn default_socket_path() -> PathBuf {
    use crate::pipe_trust::{pipe_name, valid_pipe_name, PIPE_ENV};

    static INVALID_PIPE_WARNING: std::sync::Once = std::sync::Once::new();
    let value = std::env::var_os(PIPE_ENV);
    let name = value.as_deref().and_then(std::ffi::OsStr::to_str);
    if value.is_some() && !name.is_some_and(valid_pipe_name) {
        INVALID_PIPE_WARNING.call_once(|| {
            tracing::warn!("invalid RS_AGENT_PIPE; using default agent pipe");
        });
    }
    PathBuf::from(pipe_name(name))
}

#[cfg(not(any(unix, windows)))]
pub fn default_socket_path() -> PathBuf {
    PathBuf::new()
}

// Tauri adapter. No application handle or window is needed by the transport.
use tauri::{Emitter, Manager};

struct TauriSink(tauri::AppHandle);

impl EventSink for TauriSink {
    fn event(&self, method: &str, params: Value) {
        let _ = self
            .0
            .emit("ipc://event", json!({"method": method, "params": params}));
    }

    fn connection(&self, connected: bool) {
        let _ = self
            .0
            .emit("ipc://connection", json!({"connected": connected}));
    }
}

#[tauri::command]
pub async fn ipc_call(
    state: tauri::State<'_, Bridge>,
    method: String,
    params: Option<Value>,
) -> Result<Value, RpcError> {
    state.call(&method, params.unwrap_or(Value::Null)).await
}

#[tauri::command]
pub fn ipc_connected(state: tauri::State<'_, Bridge>) -> bool {
    state.is_connected()
}

pub fn start(app: &mut tauri::App) -> tauri::Result<()> {
    let runtime = tauri::async_runtime::handle();
    let _entered = runtime.inner().enter();
    let sink = Arc::new(TauriSink(app.handle().clone()));
    app.manage(Bridge::spawn(BridgeConfig::default(), sink));
    Ok(())
}

#[cfg(all(test, unix))]
mod tests {
    use super::*;
    use std::ffi::OsString;
    use tempfile::TempDir;
    use tokio::net::unix::{OwnedReadHalf, OwnedWriteHalf};
    use tokio::net::UnixListener;
    use tokio::time::timeout;

    const TEST_WAIT: Duration = Duration::from_secs(3);

    #[derive(Debug, PartialEq)]
    enum Signal {
        Connection(bool),
        Event(String, Value),
    }

    struct TestSink(mpsc::UnboundedSender<Signal>);

    impl EventSink for TestSink {
        fn event(&self, method: &str, params: Value) {
            let _ = self.0.send(Signal::Event(method.into(), params));
        }

        fn connection(&self, connected: bool) {
            let _ = self.0.send(Signal::Connection(connected));
        }
    }

    struct Fixture {
        directory: TempDir,
        listener: Option<UnixListener>,
        bridge: Arc<Bridge>,
        signals: mpsc::UnboundedReceiver<Signal>,
    }

    fn config(path: PathBuf, call_timeout: Duration) -> BridgeConfig {
        BridgeConfig {
            socket_path: path,
            call_timeout,
            backoff_min: Duration::from_millis(10),
            backoff_max: Duration::from_millis(40),
        }
    }

    async fn next_signal(signals: &mut mpsc::UnboundedReceiver<Signal>) -> Signal {
        timeout(TEST_WAIT, signals.recv()).await.unwrap().unwrap()
    }

    impl Fixture {
        async fn new(call_timeout: Duration) -> (Self, UnixStream) {
            // macOS Unix socket paths must fit sun_path, including under a long TMPDIR.
            let directory = tempfile::tempdir_in("/tmp").unwrap();
            let path = directory.path().join("agent.sock");
            let listener = UnixListener::bind(&path).unwrap();
            let (sender, mut signals) = mpsc::unbounded_channel();
            let bridge = Arc::new(Bridge::spawn(
                config(path, call_timeout),
                Arc::new(TestSink(sender)),
            ));
            let (stream, _) = timeout(TEST_WAIT, listener.accept())
                .await
                .unwrap()
                .unwrap();
            assert_eq!(next_signal(&mut signals).await, Signal::Connection(true));
            assert!(bridge.is_connected());
            (
                Self {
                    directory,
                    listener: Some(listener),
                    bridge,
                    signals,
                },
                stream,
            )
        }
    }

    async fn request(reader: &mut BufReader<OwnedReadHalf>) -> Value {
        let mut line = String::new();
        assert!(
            timeout(TEST_WAIT, reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap()
                > 0
        );
        assert!(line.ends_with('\n'));
        serde_json::from_str(&line).unwrap()
    }

    async fn send(writer: &mut OwnedWriteHalf, message: Value) {
        let mut line = serde_json::to_vec(&message).unwrap();
        line.push(b'\n');
        writer.write_all(&line).await.unwrap();
    }

    async fn respond(writer: &mut OwnedWriteHalf, id: &Value, result: Value) {
        send(
            writer,
            json!({"jsonrpc": "2.0", "id": id, "result": result}),
        )
        .await;
    }

    #[tokio::test]
    async fn round_trip_omits_null_params_and_uses_numeric_id() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (result, ()) = tokio::join!(fixture.bridge.call("status.get", Value::Null), async {
            let message = request(&mut reader).await;
            assert_eq!(message["jsonrpc"], "2.0");
            assert_eq!(message["method"], "status.get");
            assert!(message["id"].is_u64());
            assert!(message.get("params").is_none());
            respond(&mut writer, &message["id"], json!({"online": true})).await;
        });
        assert_eq!(result.unwrap(), json!({"online": true}));
    }

    #[tokio::test]
    async fn passes_params_unchanged() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let params = json!({"session_id": "s-1"});
        let (result, ()) = tokio::join!(
            fixture.bridge.call("sessions.disconnect", params.clone()),
            async {
                let message = request(&mut reader).await;
                assert_eq!(message["method"], "sessions.disconnect");
                assert_eq!(message["params"], params);
                respond(&mut writer, &message["id"], json!({})).await;
            }
        );
        assert_eq!(result.unwrap(), json!({}));
    }

    #[tokio::test]
    async fn passes_rpc_errors_unchanged() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let error =
            json!({"code": -32000, "message": "session not found", "data": {"code": "not_found"}});
        let (result, ()) = tokio::join!(
            fixture
                .bridge
                .call("sessions.disconnect", json!({"session_id": "s-1"})),
            async {
                let message = request(&mut reader).await;
                send(
                    &mut writer,
                    json!({"jsonrpc": "2.0", "id": message["id"], "error": error}),
                )
                .await;
            }
        );
        assert_eq!(
            result.unwrap_err(),
            RpcError {
                code: -32000,
                message: "session not found".into(),
                data: Some(json!({"code": "not_found"})),
            }
        );
    }

    #[tokio::test]
    async fn matches_out_of_order_responses_by_id() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (a, b, ()) = tokio::join!(
            fixture.bridge.call("request.a", Value::Null),
            fixture.bridge.call("request.b", Value::Null),
            async {
                let a = request(&mut reader).await;
                let b = request(&mut reader).await;
                assert_eq!(a["method"], "request.a");
                assert_eq!(b["method"], "request.b");
                assert!(a["id"].as_u64().unwrap() < b["id"].as_u64().unwrap());
                respond(&mut writer, &b["id"], json!("b")).await;
                respond(&mut writer, &a["id"], json!("a")).await;
            }
        );
        assert_eq!(a.unwrap(), "a");
        assert_eq!(b.unwrap(), "b");
    }

    #[tokio::test]
    async fn forwards_notifications_and_defaults_missing_params_to_null() {
        let (mut fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (_, mut writer) = stream.into_split();
        send(
            &mut writer,
            json!({"jsonrpc": "2.0", "method": "event.handoff", "params": {"x": 1}}),
        )
        .await;
        send(
            &mut writer,
            json!({"jsonrpc": "2.0", "method": "event.permissionsChanged"}),
        )
        .await;
        assert_eq!(
            next_signal(&mut fixture.signals).await,
            Signal::Event("event.handoff".into(), json!({"x": 1}))
        );
        assert_eq!(
            next_signal(&mut fixture.signals).await,
            Signal::Event("event.permissionsChanged".into(), Value::Null)
        );
    }

    #[tokio::test]
    async fn ignores_bad_lines_unknown_ids_and_invalid_envelopes() {
        let (mut fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (result, ()) = tokio::join!(fixture.bridge.call("status.get", Value::Null), async {
            let message = request(&mut reader).await;
            writer.write_all(b"not json\n\xff\n[]\n{}\n").await.unwrap();
            send(&mut writer, json!({"id": u64::MAX, "result": "unknown"})).await;
            send(
                &mut writer,
                json!({"id": message["id"], "error": {"message": "no code"}}),
            )
            .await;
            send(&mut writer, json!({"id": message["id"], "result": 1, "error": {"code": -1, "message": "ambiguous"}})).await;
            send(
                &mut writer,
                json!({"id": null, "method": "not.a.notification"}),
            )
            .await;
            respond(&mut writer, &message["id"], json!({"online": true})).await;
        });
        assert_eq!(result.unwrap(), json!({"online": true}));
        assert!(fixture.signals.try_recv().is_err());
    }

    #[tokio::test]
    async fn unavailable_socket_returns_immediately_without_connection_event() {
        let directory = tempfile::tempdir_in("/tmp").unwrap();
        let (sender, mut signals) = mpsc::unbounded_channel();
        let bridge = Bridge::spawn(
            config(
                directory.path().join("missing.sock"),
                Duration::from_secs(1),
            ),
            Arc::new(TestSink(sender)),
        );
        let error = timeout(
            Duration::from_millis(50),
            bridge.call("status.get", Value::Null),
        )
        .await
        .unwrap()
        .unwrap_err();
        assert_eq!(error, RpcError::unavailable());
        assert_eq!(error.code, -32099);
        assert_eq!(error.data.unwrap()["code"], "agent_unavailable");
        assert!(!bridge.is_connected());
        tokio::time::sleep(Duration::from_millis(60)).await;
        assert!(signals.try_recv().is_err());
    }

    #[tokio::test]
    async fn disconnect_fails_pending_calls_then_reconnects() {
        let (mut fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (result, ()) = tokio::join!(fixture.bridge.call("status.get", Value::Null), async {
            let _ = request(&mut reader).await;
            drop(fixture.listener.take());
            drop(reader);
            drop(writer);
        });
        assert_eq!(result.unwrap_err(), RpcError::unavailable());
        assert_eq!(
            next_signal(&mut fixture.signals).await,
            Signal::Connection(false)
        );
        assert!(!fixture.bridge.is_connected());
        let path = fixture.directory.path().join("agent.sock");
        std::fs::remove_file(&path).unwrap();
        // Let several failed retries elapse, then restart on the same pathname.
        tokio::time::sleep(Duration::from_millis(65)).await;
        let listener = UnixListener::bind(path).unwrap();
        let (stream, _) = timeout(TEST_WAIT, listener.accept())
            .await
            .unwrap()
            .unwrap();
        assert_eq!(
            next_signal(&mut fixture.signals).await,
            Signal::Connection(true)
        );
        assert!(fixture.bridge.is_connected());
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (result, ()) = tokio::join!(fixture.bridge.call("status.get", Value::Null), async {
            let message = request(&mut reader).await;
            respond(&mut writer, &message["id"], json!({"online": true})).await;
        });
        assert_eq!(result.unwrap(), json!({"online": true}));
    }

    #[tokio::test]
    async fn timeout_removes_waiter_and_late_response_is_ignored() {
        let (fixture, stream) = Fixture::new(Duration::from_millis(60)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (result, first) = tokio::join!(
            fixture.bridge.call("status.get", Value::Null),
            request(&mut reader)
        );
        assert_eq!(result.unwrap_err(), RpcError::timeout());
        assert!(fixture.bridge.shared.state().pending.is_empty());
        respond(&mut writer, &first["id"], json!("late")).await;
        let (result, ()) = tokio::join!(fixture.bridge.call("status.get", Value::Null), async {
            let second = request(&mut reader).await;
            assert_ne!(first["id"], second["id"]);
            respond(&mut writer, &second["id"], json!("current")).await;
        });
        assert_eq!(result.unwrap(), "current");
        assert!(fixture.bridge.shared.state().pending.is_empty());
        assert!(fixture.bridge.is_connected());
    }

    #[tokio::test(flavor = "multi_thread", worker_threads = 2)]
    async fn fifty_concurrent_calls_keep_lines_and_responses_separate() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(2)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let mut calls = tokio::task::JoinSet::new();
        for index in 0..50 {
            let bridge = Arc::clone(&fixture.bridge);
            calls.spawn(async move {
                (
                    index,
                    bridge.call("test.echo", json!({"index": index})).await,
                )
            });
        }
        let mut messages = Vec::new();
        for _ in 0..50 {
            messages.push(request(&mut reader).await);
        }
        for message in messages.into_iter().rev() {
            respond(&mut writer, &message["id"], message["params"].clone()).await;
        }
        while let Some(result) = timeout(TEST_WAIT, calls.join_next()).await.unwrap() {
            let (index, result) = result.unwrap();
            assert_eq!(result.unwrap(), json!({"index": index}));
        }
        assert!(fixture.bridge.shared.state().pending.is_empty());
    }

    #[test]
    fn socket_path_honors_override_and_default_location() {
        // All other tests pass explicit paths; none reads or changes this variable.
        struct RestoreEnv(Option<OsString>);
        impl Drop for RestoreEnv {
            fn drop(&mut self) {
                match &self.0 {
                    Some(value) => std::env::set_var("RS_AGENT_SOCK", value),
                    None => std::env::remove_var("RS_AGENT_SOCK"),
                }
            }
        }
        let _restore = RestoreEnv(std::env::var_os("RS_AGENT_SOCK"));
        std::env::set_var("RS_AGENT_SOCK", "/tmp/x.sock");
        assert_eq!(default_socket_path(), PathBuf::from("/tmp/x.sock"));
        std::env::remove_var("RS_AGENT_SOCK");
        let expected = PathBuf::from(std::env::var_os("HOME").unwrap())
            .join("Library/Application Support/Remote Host/agent.sock");
        assert_eq!(default_socket_path(), expected);
        assert!(
            default_socket_path().ends_with("Library/Application Support/Remote Host/agent.sock")
        );
        let defaults = BridgeConfig::default();
        assert_eq!(defaults.call_timeout, Duration::from_secs(15));
        assert_eq!(defaults.backoff_min, Duration::from_millis(500));
        assert_eq!(defaults.backoff_max, Duration::from_secs(5));
    }

    #[tokio::test]
    async fn validates_method_byte_length_before_sending() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        for method in [String::new(), "a".repeat(65), "é".repeat(33)] {
            let error = fixture.bridge.call(&method, Value::Null).await.unwrap_err();
            assert_eq!(error, RpcError::invalid_request());
            assert_eq!(
                serde_json::to_value(error).unwrap(),
                json!({"code": -32600, "message": "invalid request"})
            );
        }
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let method = "a".repeat(64);
        let (result, ()) = tokio::join!(fixture.bridge.call(&method, Value::Null), async {
            let message = request(&mut reader).await;
            assert_eq!(message["method"], method);
            respond(&mut writer, &message["id"], Value::Null).await;
        });
        assert_eq!(result.unwrap(), Value::Null);
    }

    #[tokio::test]
    async fn oversized_unterminated_line_disconnects_and_fails_all_waiters() {
        let (mut fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (a, b, ()) = tokio::join!(
            fixture.bridge.call("request.a", Value::Null),
            fixture.bridge.call("request.b", Value::Null),
            async {
                let _ = request(&mut reader).await;
                let _ = request(&mut reader).await;
                // No newline: the cap must be enforced before read_line completes.
                let _ = writer.write_all(&vec![b'x'; MAX_LINE_LENGTH + 1]).await;
            }
        );
        assert_eq!(a.unwrap_err(), RpcError::unavailable());
        assert_eq!(b.unwrap_err(), RpcError::unavailable());
        assert_eq!(
            next_signal(&mut fixture.signals).await,
            Signal::Connection(false)
        );
        assert!(fixture.bridge.shared.state().pending.is_empty());
    }

    #[tokio::test]
    async fn accepts_exactly_one_mebibyte_before_the_newline() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let (result, expected) =
            tokio::join!(fixture.bridge.call("test.large", Value::Null), async {
                let message = request(&mut reader).await;
                let mut response = json!({"jsonrpc": "2.0", "id": message["id"], "result": ""});
                let length = MAX_LINE_LENGTH - response.to_string().len();
                let expected = "x".repeat(length);
                response["result"] = json!(expected);
                assert_eq!(response.to_string().len(), MAX_LINE_LENGTH);
                send(&mut writer, response).await;
                expected
            });
        assert_eq!(result.unwrap(), expected);
    }

    #[tokio::test]
    async fn cancelling_a_call_removes_its_waiter() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let (reader, mut writer) = stream.into_split();
        let mut reader = BufReader::new(reader);
        let bridge = Arc::clone(&fixture.bridge);
        let call = tokio::spawn(async move { bridge.call("status.get", Value::Null).await });
        let message = request(&mut reader).await;
        assert_eq!(fixture.bridge.shared.state().pending.len(), 1);
        call.abort();
        assert!(call.await.unwrap_err().is_cancelled());
        assert!(fixture.bridge.shared.state().pending.is_empty());
        respond(&mut writer, &message["id"], json!("cancelled")).await;
        let (result, ()) = tokio::join!(fixture.bridge.call("status.get", Value::Null), async {
            let message = request(&mut reader).await;
            respond(&mut writer, &message["id"], json!("active")).await;
        });
        assert_eq!(result.unwrap(), "active");
    }

    #[tokio::test]
    async fn dropping_bridge_closes_socket_and_leaves_agent_listener_running() {
        let (fixture, stream) = Fixture::new(Duration::from_secs(1)).await;
        let mut reader = BufReader::new(stream);
        drop(fixture.bridge);
        let mut line = String::new();
        assert_eq!(
            timeout(TEST_WAIT, reader.read_line(&mut line))
                .await
                .unwrap()
                .unwrap(),
            0
        );
        let client = UnixStream::connect(fixture.directory.path().join("agent.sock"))
            .await
            .unwrap();
        let (_, _) = timeout(TEST_WAIT, fixture.listener.unwrap().accept())
            .await
            .unwrap()
            .unwrap();
        drop(client);
    }
}
