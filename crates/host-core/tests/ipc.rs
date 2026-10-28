use std::collections::BTreeSet;
use std::future::{ready, Ready};
use std::os::unix::fs::PermissionsExt;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::ipc::{
    ApprovalPendingResult, ApprovalRespondParams, AuditListParams, AuditListResult,
    DeviceListResult, DevicesListResult, DevicesRevokeParams, DevicesUpdateParams, EmptyResult,
    Event, IpcBackend, IpcError, IpcServer, LogsExportParams, LogsExportResult,
    NetworkFirewallStatusResult, NetworkItNoteResult, NetworkListNicsResult, PairFileInfo,
    PairFileState, PairingCancelParams, PairingExportFileParams, PairingExportFileResult,
    PairingListFilesResult, PairingRevokeFileParams, PairingStartResult, Permission,
    SessionSummary, SessionsDisconnectParams, SessionsListResult, SessionsSetViewOnlyParams,
    StatusGetResult, UpdateChannel, UpdateSetChannelParams, UpdateStatusResult, Value,
    METHOD_NAMES,
};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
use tokio::net::UnixStream;

static NEXT_PATH: AtomicU64 = AtomicU64::new(1);

#[derive(Default)]
struct Backend;

#[allow(refining_impl_trait)]
impl IpcBackend for Backend {
    fn status_get(&self) -> Ready<Result<StatusGetResult, IpcError>> {
        ready(Ok(StatusGetResult {
            online: true,
            tailnet_addresses: vec!["100.64.0.2".to_owned()],
            version: "0.1.0".to_owned(),
            permissions: json!({"screen_recording": true}),
            listening: true,
            sessions: vec![],
        }))
    }

    fn sessions_list(&self) -> Ready<Result<SessionsListResult, IpcError>> {
        ready(Ok(SessionsListResult {
            sessions: vec![SessionSummary {
                session_id: "s-1".to_owned(),
                device_id: "d-1".to_owned(),
                device_name: "test phone".to_owned(),
                ..Default::default()
            }],
        }))
    }

    fn sessions_disconnect(
        &self,
        _params: SessionsDisconnectParams,
    ) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn sessions_set_view_only(
        &self,
        _params: SessionsSetViewOnlyParams,
    ) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn pairing_start(&self) -> Ready<Result<PairingStartResult, IpcError>> {
        ready(Ok(PairingStartResult {
            qr_url: "rs://pair/example".to_owned(),
            code: "123456".to_owned(),
            expires_at: 1_800_000_000_000_000,
        }))
    }

    fn pairing_cancel(&self, _params: PairingCancelParams) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn pairing_export_file(
        &self,
        _params: PairingExportFileParams,
    ) -> Ready<Result<PairingExportFileResult, IpcError>> {
        ready(Ok(PairingExportFileResult {
            file_id: "AAECAwQFBgcICQoLDA0ODw".to_owned(),
            path: "/Users/example/Downloads/Studio Mac mini-20261020-0258.rspair".to_owned(),
            file_name: "Studio Mac mini-20261020-0258.rspair".to_owned(),
            expires_at: 1_792_551_534_000_000,
        }))
    }

    fn pairing_list_files(&self) -> Ready<Result<PairingListFilesResult, IpcError>> {
        ready(Ok(PairingListFilesResult {
            files: vec![
                PairFileInfo {
                    file_id: "AAECAwQFBgcICQoLDA0ODw".to_owned(),
                    label: Some("IT 分发 10 月".to_owned()),
                    permission: Permission::Control,
                    created_at: 1_792_465_134_000_000,
                    expires_at: 1_792_551_534_000_000,
                    state: PairFileState::Active,
                    used_by_device_id: None,
                    used_at: None,
                },
                PairFileInfo {
                    file_id: "EBESExQVFhcYGRobHB0eHw".to_owned(),
                    label: Some("iPad 现场支持".to_owned()),
                    permission: Permission::ViewOnly,
                    created_at: 1_792_378_734_000_000,
                    expires_at: 1_792_465_134_000_000,
                    state: PairFileState::Used,
                    used_by_device_id: Some("AAECAwQFBgcICQoLDA0ODw".to_owned()),
                    used_at: Some(1_792_400_000_000_000),
                },
            ],
        }))
    }

    fn pairing_revoke_file(
        &self,
        _params: PairingRevokeFileParams,
    ) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn devices_list(&self) -> Ready<Result<DevicesListResult, IpcError>> {
        ready(Ok(DeviceListResult::default()))
    }

    fn devices_update(&self, _params: DevicesUpdateParams) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn devices_revoke(&self, _params: DevicesRevokeParams) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn settings_get(&self) -> Ready<Result<Value, IpcError>> {
        ready(Ok(json!({"access": {"allow_remote_control": true}})))
    }

    fn settings_set(&self, settings: Value) -> Ready<Result<Value, IpcError>> {
        ready(Ok(settings))
    }

    fn network_list_nics(&self) -> Ready<Result<NetworkListNicsResult, IpcError>> {
        ready(Ok(NetworkListNicsResult::default()))
    }

    fn network_firewall_status(&self) -> Ready<Result<NetworkFirewallStatusResult, IpcError>> {
        ready(Ok(NetworkFirewallStatusResult {
            detail: json!({}),
            ..Default::default()
        }))
    }

    fn network_it_note(&self) -> Ready<Result<NetworkItNoteResult, IpcError>> {
        ready(Ok(NetworkItNoteResult::default()))
    }

    fn approval_pending(&self) -> Ready<Result<ApprovalPendingResult, IpcError>> {
        ready(Ok(ApprovalPendingResult {
            requests: vec![json!({
                "request_id": "a-1", "kind": "connection", "device_name": "test phone",
                "device_id": "d-1", "platform": "ios", "whois_login": null,
                "path": "lan", "reasons": ["attended_mode"], "timeout_s": 30,
                "permission_choice": false, "device_fp": null,
            })],
        }))
    }

    fn approval_respond(
        &self,
        _params: ApprovalRespondParams,
    ) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn update_status(&self) -> Ready<Result<UpdateStatusResult, IpcError>> {
        ready(Ok(UpdateStatusResult::default()))
    }

    fn update_install_now(&self) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn update_set_channel(
        &self,
        _params: UpdateSetChannelParams,
    ) -> Ready<Result<EmptyResult, IpcError>> {
        ready(Ok(EmptyResult::default()))
    }

    fn logs_export(&self, _params: LogsExportParams) -> Ready<Result<LogsExportResult, IpcError>> {
        ready(Ok(LogsExportResult {
            path: "/tmp/remote-host-logs.zip".to_owned(),
        }))
    }

    fn audit_list(&self, _params: AuditListParams) -> Ready<Result<AuditListResult, IpcError>> {
        ready(Ok(AuditListResult::default()))
    }
}

fn socket_path() -> PathBuf {
    let number = NEXT_PATH.fetch_add(1, Ordering::Relaxed);
    std::env::temp_dir().join(format!(
        "remote-host-ipc-{}-{number}.sock",
        std::process::id()
    ))
}

async fn start() -> (
    PathBuf,
    host_core::ipc::Notifier,
    tokio::task::JoinHandle<()>,
) {
    let path = socket_path();
    let server = IpcServer::bind(&path, Backend).expect("bind test socket");
    // bind creates the socket synchronously; serving need not be scheduled yet.
    assert!(path.exists(), "test socket was not created: {path:?}");
    let notifier = server.notifier();
    let task = tokio::spawn(async move {
        let _ = server.serve().await;
    });
    (path, notifier, task)
}

async fn request(stream: &mut UnixStream, value: serde_json::Value) -> serde_json::Value {
    stream
        .write_all(format!("{value}\n").as_bytes())
        .await
        .expect("write request");
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read response");
    serde_json::from_str(line.trim()).expect("response JSON")
}

#[tokio::test]
async fn dispatches_a_request_and_reports_protocol_errors() {
    let (path, _notifier, task) = start().await;
    let mut stream = UnixStream::connect(&path).await.expect("connect");
    let response = request(
        &mut stream,
        json!({"jsonrpc":"2.0","id":7,"method":"status.get"}),
    )
    .await;
    assert_eq!(response["id"], 7);
    assert_eq!(response["result"]["online"], true);

    let response = request(
        &mut stream,
        json!({"jsonrpc":"2.0","id":8,"method":"unknown.method"}),
    )
    .await;
    assert_eq!(response["error"]["code"], -32601);

    let response = request(
        &mut stream,
        json!({"jsonrpc":"2.0","id":9,"method":"status.get","params":[]}),
    )
    .await;
    assert_eq!(response["error"]["code"], -32602);

    stream
        .write_all(b"{bad json}\n")
        .await
        .expect("write malformed request");
    let mut reader = BufReader::new(&mut stream);
    let mut line = String::new();
    reader
        .read_line(&mut line)
        .await
        .expect("read parse response");
    let response: serde_json::Value = serde_json::from_str(line.trim()).expect("parse response");
    assert_eq!(response["error"]["code"], -32700);

    let mut batch = UnixStream::connect(&path).await.expect("batch connect");
    batch.write_all(b"[]\n").await.expect("write batch");
    let mut batch_reader = BufReader::new(&mut batch);
    let mut batch_line = String::new();
    batch_reader
        .read_line(&mut batch_line)
        .await
        .expect("read batch response");
    let batch_response: serde_json::Value =
        serde_json::from_str(batch_line.trim()).expect("batch response");
    assert_eq!(batch_response["error"]["code"], -32600);

    let mut oversized = UnixStream::connect(&path).await.expect("oversized connect");
    let mut oversized_line = vec![b' '; host_core::ipc::MAX_LINE_LENGTH + 1];
    oversized_line.push(b'\n');
    oversized
        .write_all(&oversized_line)
        .await
        .expect("write oversized request");
    let mut oversized_reader = BufReader::new(&mut oversized);
    let mut oversized_response = String::new();
    oversized_reader
        .read_line(&mut oversized_response)
        .await
        .expect("read oversized response");
    let oversized_response: serde_json::Value =
        serde_json::from_str(oversized_response.trim()).expect("oversized response");
    assert_eq!(oversized_response["error"]["code"], -32600);

    task.abort();
    let _ = task.await;
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn broadcasts_events_to_two_clients_and_sets_socket_mode() {
    let (path, notifier, task) = start().await;
    let metadata = std::fs::metadata(&path).expect("socket metadata");
    assert_eq!(metadata.permissions().mode() & 0o777, 0o600);

    let mut first = UnixStream::connect(&path).await.expect("first connect");
    let mut second = UnixStream::connect(&path).await.expect("second connect");
    // A response proves each accepted client has subscribed to notifications.
    // connect alone only queues the connection in the listener's backlog.
    for stream in [&mut first, &mut second] {
        let response = request(
            stream,
            json!({"jsonrpc":"2.0","id":1,"method":"status.get"}),
        )
        .await;
        assert_eq!(response["result"]["online"], true);
    }
    notifier
        .send(Event::SessionsChanged(json!({"sessions": []})))
        .expect("send event");

    let mut first_reader = BufReader::new(&mut first);
    let mut first_line = String::new();
    first_reader
        .read_line(&mut first_line)
        .await
        .expect("first event");
    let mut second_reader = BufReader::new(&mut second);
    let mut second_line = String::new();
    second_reader
        .read_line(&mut second_line)
        .await
        .expect("second event");
    let first_event: serde_json::Value =
        serde_json::from_str(first_line.trim()).expect("first event JSON");
    let second_event: serde_json::Value =
        serde_json::from_str(second_line.trim()).expect("second event JSON");
    assert_eq!(first_event["method"], "event.sessionsChanged");
    assert_eq!(second_event["method"], "event.sessionsChanged");

    drop(first);
    notifier
        .send(Event::Handoff(json!({"session_id": "s-1"})))
        .expect("send after disconnect");
    let mut line = String::new();
    second_reader
        .read_line(&mut line)
        .await
        .expect("second event after disconnect");
    let event: serde_json::Value = serde_json::from_str(line.trim()).expect("event JSON");
    assert_eq!(event["method"], "event.handoff");

    task.abort();
    let _ = task.await;
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn removes_an_unreachable_socket_path() {
    let path = socket_path();
    std::fs::write(&path, b"stale").expect("create stale path");
    let server = IpcServer::bind(&path, Backend).expect("rebind stale socket");
    assert!(path.exists());
    drop(server);
    assert!(!path.exists());
}

#[tokio::test]
async fn contract_samples_round_trip() {
    let (path, _notifier, task) = start().await;
    let mut stream = UnixStream::connect(&path).await.expect("connect");
    let directory = std::path::Path::new(env!("CARGO_MANIFEST_DIR")).join("tests/ipc");
    let mut requests: Vec<_> = std::fs::read_dir(&directory)
        .expect("sample directory")
        .map(|entry| entry.expect("sample entry").path())
        .filter(|path| path.to_string_lossy().ends_with(".request.json"))
        .collect();
    requests.sort();
    // 样例文件里的方法名集合 == 声明的方法名集合（`host_core::ipc::METHOD_NAMES`）。
    let sampled: BTreeSet<String> = requests
        .iter()
        .map(|path| {
            path.file_name()
                .expect("request filename")
                .to_string_lossy()
                .trim_end_matches(".request.json")
                .to_owned()
        })
        .collect();
    let declared: BTreeSet<String> = METHOD_NAMES
        .iter()
        .map(|method| (*method).to_owned())
        .collect();
    assert_eq!(
        sampled,
        declared,
        "samples without a declared method: {:?}; declared methods without samples: {:?}",
        sampled.difference(&declared).collect::<Vec<_>>(),
        declared.difference(&sampled).collect::<Vec<_>>(),
    );
    for request_path in requests {
        let response_path = request_path.with_file_name(
            request_path
                .file_name()
                .expect("request filename")
                .to_string_lossy()
                .replace(".request.json", ".response.json"),
        );
        let request_value: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(&request_path).expect("request sample"))
                .expect("request sample JSON");
        let expected: serde_json::Value =
            serde_json::from_str(&std::fs::read_to_string(response_path).expect("response sample"))
                .expect("response sample JSON");
        let mut actual = request(&mut stream, request_value).await;
        actual["id"] = serde_json::Value::Null;
        let mut expected = expected;
        expected["id"] = serde_json::Value::Null;
        assert_eq!(actual, expected, "sample {:?}", request_path);
    }
    task.abort();
    let _ = task.await;
    let _ = std::fs::remove_file(path);
}

#[allow(dead_code)]
fn _type_markers(_: Permission, _: UpdateChannel) {}

struct DefaultPendingBackend;

macro_rules! forward_backend {
    ($($method:ident($($param:ident: $ty:ty),*) -> $result:ty;)*) => {
        $(fn $method(&self, $($param: $ty),*) -> impl std::future::Future<Output = Result<$result, IpcError>> + Send {
            Backend.$method($($param),*)
        })*
    };
}

impl IpcBackend for DefaultPendingBackend {
    forward_backend! {
        status_get() -> StatusGetResult;
        sessions_list() -> SessionsListResult;
        sessions_disconnect(p: SessionsDisconnectParams) -> EmptyResult;
        sessions_set_view_only(p: SessionsSetViewOnlyParams) -> EmptyResult;
        pairing_start() -> PairingStartResult;
        pairing_cancel(p: PairingCancelParams) -> EmptyResult;
        devices_list() -> DevicesListResult;
        devices_update(p: DevicesUpdateParams) -> EmptyResult;
        devices_revoke(p: DevicesRevokeParams) -> EmptyResult;
        settings_get() -> Value;
        settings_set(p: Value) -> Value;
        network_list_nics() -> NetworkListNicsResult;
        network_firewall_status() -> NetworkFirewallStatusResult;
        network_it_note() -> NetworkItNoteResult;
        approval_respond(p: ApprovalRespondParams) -> EmptyResult;
        update_status() -> UpdateStatusResult;
        update_install_now() -> EmptyResult;
        update_set_channel(p: UpdateSetChannelParams) -> EmptyResult;
        logs_export(p: LogsExportParams) -> LogsExportResult;
        audit_list(p: AuditListParams) -> AuditListResult;
    }
}

#[tokio::test]
async fn default_approval_pending_is_unsupported() {
    let path = socket_path();
    let server = IpcServer::bind(&path, DefaultPendingBackend).expect("bind default backend");
    let task = tokio::spawn(async move { server.serve().await });
    let mut stream = UnixStream::connect(&path)
        .await
        .expect("connect default backend");
    let response = request(
        &mut stream,
        json!({"jsonrpc":"2.0","id":1,"method":"approval.pending"}),
    )
    .await;
    assert_eq!(response["error"]["data"]["code"], "unsupported");
    // The dispatcher rejects parameters before invoking the default method.
    let response = request(
        &mut stream,
        json!({"jsonrpc":"2.0","id":2,"method":"approval.pending","params":{"unexpected":true}}),
    )
    .await;
    assert_eq!(response["error"]["code"], -32602);
    task.abort();
    let _ = task.await;
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn every_declared_method_is_dispatched() {
    let unique: BTreeSet<_> = METHOD_NAMES.iter().collect();
    assert_eq!(unique.len(), METHOD_NAMES.len(), "duplicate method name");
    let path = socket_path();
    let server = IpcServer::bind(&path, DefaultPendingBackend).expect("bind default backend");
    let task = tokio::spawn(async move { server.serve().await });
    let mut stream = UnixStream::connect(&path)
        .await
        .expect("connect default backend");
    for (id, method) in METHOD_NAMES.iter().enumerate() {
        let response = request(
            &mut stream,
            json!({"jsonrpc":"2.0","id":id,"method":method}),
        )
        .await;
        assert_ne!(
            response["error"]["code"], -32601,
            "{method} is not dispatched"
        );
    }
    let response = request(
        &mut stream,
        json!({"jsonrpc":"2.0","id":99,"method":"pairing.unknown"}),
    )
    .await;
    assert_eq!(response["error"]["code"], -32601);
    task.abort();
    let _ = task.await;
    let _ = std::fs::remove_file(path);
}

#[tokio::test]
async fn default_pairing_file_methods_are_unsupported() {
    let path = socket_path();
    let server = IpcServer::bind(&path, DefaultPendingBackend).expect("bind default backend");
    let task = tokio::spawn(async move { server.serve().await });
    let mut stream = UnixStream::connect(&path)
        .await
        .expect("connect default backend");
    for (id, method, params) in [
        (
            1,
            "pairing.exportFile",
            json!({"permission": "control", "label": "IT"}),
        ),
        (2, "pairing.listFiles", Value::Null),
        (3, "pairing.revokeFile", json!({"file_id": "f-1"})),
    ] {
        let response = request(
            &mut stream,
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await;
        assert_eq!(response["error"]["data"]["code"], "unsupported", "{method}");
    }
    // 参数在调用默认实现之前校验。
    for (id, method, params) in [
        (4, "pairing.exportFile", json!({"permission": "admin"})),
        (5, "pairing.listFiles", json!({"unexpected": true})),
        (6, "pairing.revokeFile", json!({})),
    ] {
        let response = request(
            &mut stream,
            json!({"jsonrpc":"2.0","id":id,"method":method,"params":params}),
        )
        .await;
        assert_eq!(response["error"]["code"], -32602, "{method}");
    }
    task.abort();
    let _ = task.await;
    let _ = std::fs::remove_file(path);
}
