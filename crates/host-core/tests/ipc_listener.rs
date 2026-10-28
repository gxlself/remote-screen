use std::io;
use std::sync::Arc;

use host_core::ipc::{
    ApprovalRespondParams, AuditListParams, AuditListResult, DeviceListResult, EmptyResult, Event,
    IpcBackend, IpcError, LogsExportParams, LogsExportResult, NetworkFirewallStatusResult,
    NetworkItNoteResult, NetworkListNicsResult, SessionSummary, SessionsDisconnectParams,
    SessionsListResult, SessionsSetViewOnlyParams, StatusGetResult, UpdateSetChannelParams,
    UpdateStatusResult, Value, MAX_LINE_LENGTH,
};
use host_core::ipc_listener::{IpcListener, StreamServer};
use serde_json::json;
use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader, DuplexStream};
use tokio::sync::mpsc;

#[derive(Default)]
struct Backend;

fn unsupported<T>() -> Result<T, IpcError> {
    Err(IpcError::business(
        "unsupported",
        "method is not used by the stream listener test",
    ))
}

#[allow(refining_impl_trait)]
impl IpcBackend for Backend {
    async fn status_get(&self) -> Result<StatusGetResult, IpcError> {
        Ok(StatusGetResult {
            online: true,
            tailnet_addresses: vec!["100.64.0.2".to_owned()],
            version: "0.1.0".to_owned(),
            permissions: json!({"screen_recording": true}),
            listening: true,
            sessions: vec![],
        })
    }

    async fn sessions_list(&self) -> Result<SessionsListResult, IpcError> {
        Ok(SessionsListResult {
            sessions: vec![SessionSummary {
                session_id: "s-1".to_owned(),
                device_id: "d-1".to_owned(),
                device_name: "test phone".to_owned(),
                ..Default::default()
            }],
        })
    }

    async fn sessions_disconnect(
        &self,
        _params: SessionsDisconnectParams,
    ) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn sessions_set_view_only(
        &self,
        _params: SessionsSetViewOnlyParams,
    ) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn pairing_start(&self) -> Result<host_core::ipc::PairingStartResult, IpcError> {
        unsupported()
    }

    async fn pairing_cancel(
        &self,
        _params: host_core::ipc::PairingCancelParams,
    ) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn devices_list(&self) -> Result<DeviceListResult, IpcError> {
        unsupported()
    }

    async fn devices_update(
        &self,
        _params: host_core::ipc::DevicesUpdateParams,
    ) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn devices_revoke(
        &self,
        _params: host_core::ipc::DevicesRevokeParams,
    ) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn settings_get(&self) -> Result<Value, IpcError> {
        unsupported()
    }

    async fn settings_set(&self, _settings: Value) -> Result<Value, IpcError> {
        unsupported()
    }

    async fn network_list_nics(&self) -> Result<NetworkListNicsResult, IpcError> {
        unsupported()
    }

    async fn network_firewall_status(&self) -> Result<NetworkFirewallStatusResult, IpcError> {
        unsupported()
    }

    async fn network_it_note(&self) -> Result<NetworkItNoteResult, IpcError> {
        unsupported()
    }

    async fn approval_respond(
        &self,
        _params: ApprovalRespondParams,
    ) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn update_status(&self) -> Result<UpdateStatusResult, IpcError> {
        unsupported()
    }

    async fn update_install_now(&self) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn update_set_channel(
        &self,
        _params: UpdateSetChannelParams,
    ) -> Result<EmptyResult, IpcError> {
        unsupported()
    }

    async fn logs_export(&self, _params: LogsExportParams) -> Result<LogsExportResult, IpcError> {
        unsupported()
    }

    async fn audit_list(&self, _params: AuditListParams) -> Result<AuditListResult, IpcError> {
        unsupported()
    }
}

struct DuplexListener {
    receiver: mpsc::Receiver<DuplexStream>,
}

#[allow(refining_impl_trait)]
impl IpcListener for DuplexListener {
    type Stream = DuplexStream;

    async fn accept(&mut self) -> io::Result<Self::Stream> {
        self.receiver.recv().await.ok_or_else(|| {
            io::Error::new(io::ErrorKind::BrokenPipe, "duplex listener channel closed")
        })
    }
}

fn listener() -> (DuplexListener, mpsc::Sender<DuplexStream>) {
    let (sender, receiver) = mpsc::channel(8);
    (DuplexListener { receiver }, sender)
}

async fn connect(sender: &mpsc::Sender<DuplexStream>) -> DuplexStream {
    let (server, client) = tokio::io::duplex(64 * 1024);
    sender.send(server).await.expect("queue duplex stream");
    client
}

async fn read_json(stream: &mut DuplexStream) -> Value {
    let mut reader = BufReader::new(stream);
    let mut line = String::new();
    reader.read_line(&mut line).await.expect("read response");
    serde_json::from_str(line.trim()).expect("response JSON")
}

async fn request(stream: &mut DuplexStream, value: Value) -> Value {
    stream
        .write_all(format!("{value}\n").as_bytes())
        .await
        .expect("write request");
    read_json(stream).await
}

async fn warm_up(stream: &mut DuplexStream) {
    let response = request(
        stream,
        json!({"jsonrpc":"2.0","id":1,"method":"status.get"}),
    )
    .await;
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["online"], true);
}

#[tokio::test]
async fn duplex_listener_serves_status_get() {
    let (listener, streams) = listener();
    let server = StreamServer::new(Backend);
    let serve_task = tokio::spawn(async move { server.serve(listener).await });
    let mut client = connect(&streams).await;

    let response = request(
        &mut client,
        json!({"jsonrpc":"2.0","id":1,"method":"status.get"}),
    )
    .await;

    assert_eq!(response["jsonrpc"], "2.0");
    assert_eq!(response["id"], 1);
    assert_eq!(response["result"]["online"], true);
    assert!(response.get("error").is_none());
    serve_task.abort();
    let _ = serve_task.await;
}

#[tokio::test]
async fn broadcasts_the_same_event_to_every_connection() {
    let (listener, streams) = listener();
    let server = StreamServer::new(Backend);
    let notifier = server.notifier();
    let serve_task = tokio::spawn(async move { server.serve(listener).await });
    let mut first = connect(&streams).await;
    let mut second = connect(&streams).await;
    warm_up(&mut first).await;
    warm_up(&mut second).await;

    let event = Event::SessionsChanged(json!({"sessions": []}));
    notifier.send(event.clone()).expect("broadcast event");
    let first_event = read_json(&mut first).await;
    let second_event = read_json(&mut second).await;

    assert_eq!(first_event["method"], "event.sessionsChanged");
    assert_eq!(second_event["method"], "event.sessionsChanged");
    assert_eq!(first_event["params"], second_event["params"]);
    assert_eq!(first_event["params"], json!({"sessions": []}));
    serve_task.abort();
    let _ = serve_task.await;
}

#[tokio::test]
async fn oversized_line_only_closes_its_own_connection() {
    let (listener, streams) = listener();
    let server = StreamServer::new(Backend);
    let serve_task = tokio::spawn(async move { server.serve(listener).await });
    let mut oversized = connect(&streams).await;
    let mut healthy = connect(&streams).await;
    warm_up(&mut healthy).await;

    let mut line = vec![b' '; MAX_LINE_LENGTH + 1];
    line.push(b'\n');
    oversized
        .write_all(&line)
        .await
        .expect("write oversized request");
    let response = read_json(&mut oversized).await;
    assert_eq!(response["error"]["code"], -32600);
    assert_eq!(response["error"]["message"], "request line too long");

    warm_up(&mut healthy).await;
    serve_task.abort();
    let _ = serve_task.await;
}

#[tokio::test]
async fn disconnected_client_does_not_stop_the_server() {
    let (listener, streams) = listener();
    let server = StreamServer::new(Backend);
    let serve_task = tokio::spawn(async move { server.serve(listener).await });
    let mut first = connect(&streams).await;
    let mut second = connect(&streams).await;
    warm_up(&mut first).await;
    warm_up(&mut second).await;
    drop(first);
    tokio::task::yield_now().await;

    warm_up(&mut second).await;
    assert!(!serve_task.is_finished());
    serve_task.abort();
    let _ = serve_task.await;
}

#[tokio::test]
async fn listener_error_is_returned_as_accept_failed() {
    let (listener, streams) = listener();
    drop(streams);
    let server = StreamServer::new(Backend);

    let error = server.serve(listener).await.expect_err("accept must fail");

    assert_eq!(error.code, "accept_failed");
}

#[tokio::test]
async fn spawn_connection_serves_an_existing_stream() {
    let server = Arc::new(StreamServer::new(Backend));
    let (server_stream, mut client) = tokio::io::duplex(64 * 1024);
    let connection_task = server.spawn_connection(server_stream);

    let response = request(
        &mut client,
        json!({"jsonrpc":"2.0","id":6,"method":"status.get"}),
    )
    .await;
    assert_eq!(response["id"], 6);
    assert_eq!(response["result"]["online"], true);

    drop(client);
    connection_task.await.expect("connection task");
}
