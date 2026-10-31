#![cfg(feature = "testkit")]

use std::fmt;
use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironrdp_cliprdr::backend::{ClipboardMessage, CliprdrBackend, CliprdrBackendFactory};
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp_server::{CliprdrServerFactory, ServerEvent, ServerEventSender};
use rs_rdp::client::{connect, RdpConnection};
use rs_rdp::testkit::{FakeInput, FakeRdpServer, FakeSecurity};
use rs_rdp::{CertCheck, RdpCommand, RdpConfig, RdpInput, RdpSink, Rect};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use tracing::instrument::WithSubscriber;

const UNICODE: ClipboardFormatId = ClipboardFormatId::CF_UNICODETEXT;
const LIMIT: usize = 1024 * 1024;

async fn wall_clock(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("clipboard test exceeded its 20-second wall-clock budget");
}

struct LogRecorder(Arc<Mutex<String>>);

impl tracing::Subscriber for LogRecorder {
    fn enabled(&self, _: &tracing::Metadata<'_>) -> bool {
        true
    }

    fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
        tracing::span::Id::from_u64(1)
    }

    fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
    fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
    fn enter(&self, _: &tracing::span::Id) {}
    fn exit(&self, _: &tracing::span::Id) {}

    fn event(&self, event: &tracing::Event<'_>) {
        struct Visitor<'a>(&'a mut String);
        impl tracing::field::Visit for Visitor<'_> {
            fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn fmt::Debug) {
                use std::fmt::Write as _;
                writeln!(self.0, "{value:?}").unwrap();
            }
        }
        event.record(&mut Visitor(&mut self.0.lock().unwrap()));
    }
}

#[derive(Default)]
struct RecSink {
    text: Mutex<Vec<String>>,
    other: AtomicUsize,
    changed: Notify,
}

impl RecSink {
    async fn received(&self, count: usize) -> Vec<String> {
        loop {
            let notified = self.changed.notified();
            let text = self.text.lock().unwrap().clone();
            if text.len() >= count {
                return text;
            }
            notified.await;
        }
    }
}

impl RdpSink for RecSink {
    fn resize(&self, _: u16, _: u16) {}
    fn update(&self, _: &[u8], _: usize, _: Rect) {}
    fn commit(&self) {}
    fn pointer_bitmap(&self, _: u16, _: u16, _: u16, _: u16, _: &[u8]) {}
    fn pointer_hidden(&self) {}
    fn pointer_default(&self) {}
    fn pointer_pos(&self, _: u16, _: u16) {}

    fn clipboard(&self, text: &str) {
        self.text.lock().unwrap().push(text.to_owned());
        self.changed.notify_one();
    }

    fn clipboard_other(&self) {
        self.other.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
enum ServerCall {
    Negotiated,
    Ready,
    Formats(Vec<ClipboardFormatId>),
    Requested(ClipboardFormatId),
    Accepted(bool),
    Text(String),
    DataError,
}

struct ServerClipboard {
    sender: Mutex<Option<mpsc::UnboundedSender<ServerEvent>>>,
    data: Mutex<OwnedFormatDataResponse>,
    calls: Mutex<Vec<ServerCall>>,
    changed: Notify,
}

impl ServerClipboard {
    fn new() -> Self {
        Self {
            sender: Mutex::new(None),
            data: Mutex::new(FormatDataResponse::new_error()),
            calls: Mutex::new(Vec::new()),
            changed: Notify::new(),
        }
    }

    fn record(&self, call: ServerCall) {
        self.calls.lock().unwrap().push(call);
        self.changed.notify_one();
    }

    fn send(&self, message: ClipboardMessage) {
        self.sender
            .lock()
            .unwrap()
            .as_ref()
            .expect("server event sender installed before startup")
            .send(ServerEvent::Clipboard(message))
            .unwrap();
    }

    fn announce(&self, formats: &[ClipboardFormatId]) {
        self.send(ClipboardMessage::SendInitiateCopy(
            formats.iter().copied().map(ClipboardFormat::new).collect(),
        ));
    }

    fn copy(&self, text: &str) {
        *self.data.lock().unwrap() = FormatDataResponse::new_unicode_string(text);
        self.announce(&[UNICODE]);
    }

    async fn until(&self, predicate: impl Fn(&[ServerCall]) -> bool) -> Vec<ServerCall> {
        loop {
            let notified = self.changed.notified();
            let calls = self.calls.lock().unwrap().clone();
            if predicate(&calls) {
                return calls;
            }
            notified.await;
        }
    }

    async fn initialized(&self) {
        self.until(|calls| calls.contains(&ServerCall::Formats(Vec::new())))
            .await;
    }

    fn format_lists(&self) -> Vec<Vec<ClipboardFormatId>> {
        self.calls
            .lock()
            .unwrap()
            .iter()
            .filter_map(|call| match call {
                ServerCall::Formats(formats) => Some(formats.clone()),
                _ => None,
            })
            .collect()
    }
}

struct ServerBackend(Arc<ServerClipboard>);

impl fmt::Debug for ServerBackend {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("ServerBackend").finish_non_exhaustive()
    }
}

ironrdp_core::impl_as_any!(ServerBackend);

impl CliprdrBackend for ServerBackend {
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {
        self.0.record(ServerCall::Ready);
    }

    fn on_request_format_list(&mut self) {}

    fn on_format_list_response(&mut self, ok: bool) {
        self.0.record(ServerCall::Accepted(ok));
    }

    fn on_process_negotiated_capabilities(&mut self, flags: ClipboardGeneralCapabilityFlags) {
        assert!(!flags.intersects(
            ClipboardGeneralCapabilityFlags::STREAM_FILECLIP_ENABLED
                | ClipboardGeneralCapabilityFlags::CAN_LOCK_CLIPDATA
        ));
        self.0.record(ServerCall::Negotiated);
    }

    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        self.0.record(ServerCall::Formats(
            formats.iter().map(|format| format.id).collect(),
        ));
        if formats.iter().any(|format| format.id == UNICODE) {
            self.0.send(ClipboardMessage::SendInitiatePaste(UNICODE));
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        self.0.record(ServerCall::Requested(request.format));
        assert_eq!(request.format, UNICODE);
        let data = self.0.data.lock().unwrap().clone();
        self.0.send(ClipboardMessage::SendFormatData(data));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        if response.is_error() {
            self.0.record(ServerCall::DataError);
        } else {
            assert!(response.data().ends_with(&[0, 0]));
            // Decode with the peer's implementation, independently of clip.rs.
            self.0
                .record(ServerCall::Text(response.to_unicode_string().unwrap()));
        }
    }

    fn on_file_contents_request(&mut self, _: FileContentsRequest) {
        panic!("text-only clipboard requested file contents");
    }
    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {
        panic!("text-only clipboard sent file contents");
    }
    fn on_lock(&mut self, _: LockDataId) {
        panic!("text-only clipboard locked file contents");
    }
    fn on_unlock(&mut self, _: LockDataId) {
        panic!("text-only clipboard unlocked file contents");
    }
}

struct ServerFactory(Arc<ServerClipboard>);

impl ServerEventSender for ServerFactory {
    fn set_sender(&mut self, sender: mpsc::UnboundedSender<ServerEvent>) {
        *self.0.sender.lock().unwrap() = Some(sender);
    }
}

impl CliprdrBackendFactory for ServerFactory {
    fn build_cliprdr_backend(&self) -> Box<dyn CliprdrBackend> {
        Box::new(ServerBackend(Arc::clone(&self.0)))
    }
}

impl CliprdrServerFactory for ServerFactory {}

struct AcceptAll;

#[async_trait]
impl CertCheck for AcceptAll {
    async fn check(&self, _: [u8; 32]) -> bool {
        true
    }
}

async fn open(
    clipboard: bool,
) -> (
    FakeRdpServer,
    Arc<ServerClipboard>,
    Arc<RecSink>,
    RdpConnection,
) {
    let peer = Arc::new(ServerClipboard::new());
    let factory = ServerFactory(Arc::clone(&peer));
    let server = FakeRdpServer::start_with(FakeSecurity::Tls, 800, 600, move |builder| {
        builder.with_cliprdr_factory(Some(Box::new(factory)))
    })
    .await
    .unwrap();
    let sink = Arc::new(RecSink::default());
    let config = RdpConfig {
        server_name: "127.0.0.1".into(),
        credentials: None,
        width: 800,
        height: 600,
        scale_percent: 100,
        client_name: "rdp-clipboard-test".into(),
        cert: Arc::new(AcceptAll),
        gfx: false,
        monitors: 1,
        audio: None,
        clipboard,
    };
    let stream = TcpStream::connect(server.addr()).await.unwrap();
    stream.set_nodelay(true).unwrap();
    let connection = connect(Box::new(stream), config, sink.clone())
        .await
        .unwrap();
    (server, peer, sink, connection)
}

async fn session<F, Fut>(connection: RdpConnection, action: F)
where
    F: FnOnce(mpsc::Sender<RdpCommand>) -> Fut,
    Fut: Future<Output = ()>,
{
    let (tx, rx) = mpsc::channel(16);
    let (result, ()) = tokio::join!(connection.run(rx), action(tx));
    assert_eq!(result, Ok(()));
}

// Verifies the session is alive and flushes earlier client-to-server PDUs.
async fn input_barrier(server: &mut FakeRdpServer, tx: &mpsc::Sender<RdpCommand>) {
    tx.send(RdpCommand::Input(RdpInput::MouseMove { x: 7, y: 9 }))
        .await
        .unwrap();
    assert_eq!(
        server.next_input(Duration::from_secs(5)).await,
        Some(FakeInput::MouseMove { x: 7, y: 9 })
    );
}

#[tokio::test]
async fn remote_to_phone() {
    wall_clock(async {
        let (_server, peer, sink, connection) = open(true).await;
        session(connection, |tx| async {
            peer.initialized().await;
            peer.copy("你好 RDP");
            assert_eq!(sink.received(1).await, ["你好 RDP"]);
            assert!(peer
                .calls
                .lock()
                .unwrap()
                .contains(&ServerCall::Requested(UNICODE)));
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn phone_to_remote() {
    wall_clock(async {
        let (_server, peer, sink, connection) = open(true).await;
        session(connection, |tx| async {
            peer.initialized().await;
            tx.send(RdpCommand::Clipboard("café ✓".into()))
                .await
                .unwrap();
            peer.until(|calls| calls.contains(&ServerCall::Text("café ✓".into())))
                .await;
            assert_eq!(peer.format_lists(), [vec![], vec![UNICODE]]);

            peer.copy("café ✓");
            peer.until(|calls| calls.contains(&ServerCall::Requested(UNICODE)))
                .await;
            assert!(
                tokio::time::timeout(Duration::from_millis(200), sink.received(1))
                    .await
                    .is_err()
            );

            // Sending an empty clipboard still advertises Unicode text.
            tx.send(RdpCommand::Clipboard(String::new())).await.unwrap();
            peer.until(|calls| calls.contains(&ServerCall::Text(String::new())))
                .await;
            assert_eq!(peer.format_lists(), [vec![], vec![UNICODE], vec![UNICODE]]);
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn non_text_reported_not_fetched() {
    wall_clock(async {
        let (mut server, peer, sink, connection) = open(true).await;
        session(connection, |tx| async {
            peer.initialized().await;
            peer.announce(&[ClipboardFormatId::CF_BITMAP]);
            peer.until(|calls| calls.contains(&ServerCall::Accepted(true)))
                .await;
            input_barrier(&mut server, &tx).await;
            assert_eq!(sink.other.load(Ordering::SeqCst), 1);
            assert!(
                tokio::time::timeout(Duration::from_millis(200), sink.received(1))
                    .await
                    .is_err()
            );
            assert!(!peer
                .calls
                .lock()
                .unwrap()
                .iter()
                .any(|call| matches!(call, ServerCall::Requested(_) | ServerCall::Text(_))));
            assert_eq!(peer.format_lists(), [vec![]]);
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn no_echo() {
    wall_clock(async {
        let (mut server, peer, sink, connection) = open(true).await;
        session(connection, |tx| async {
            peer.initialized().await;
            peer.copy("a");
            assert_eq!(sink.received(1).await, ["a"]);
            input_barrier(&mut server, &tx).await;
            // Only the empty initialization list; remote text is never advertised.
            assert_eq!(peer.format_lists(), [vec![]]);

            // An explicit user send of the same text must still reach the peer.
            tx.send(RdpCommand::Clipboard("a".into())).await.unwrap();
            peer.until(|calls| calls.contains(&ServerCall::Text("a".into())))
                .await;
            assert_eq!(peer.format_lists(), [vec![], vec![UNICODE]]);
            peer.copy("a");
            peer.until(|calls| {
                calls
                    .iter()
                    .filter(|call| **call == ServerCall::Requested(UNICODE))
                    .count()
                    == 2
            })
            .await;
            assert!(
                tokio::time::timeout(Duration::from_millis(200), sink.received(2))
                    .await
                    .is_err()
            );

            // A later genuine change is delivered, including a return to "a".
            peer.copy("b");
            assert_eq!(sink.received(2).await, ["a", "b"]);
            peer.copy("a");
            assert_eq!(sink.received(3).await, ["a", "b", "a"]);
            input_barrier(&mut server, &tx).await;
            assert_eq!(peer.format_lists(), [vec![], vec![UNICODE]]);
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn disabled() {
    wall_clock(async {
        let (mut server, peer, sink, connection) = open(false).await;
        session(connection, |tx| async {
            tx.send(RdpCommand::Clipboard("ignored".into()))
                .await
                .unwrap();
            input_barrier(&mut server, &tx).await;
            assert!(
                tokio::time::timeout(Duration::from_millis(200), sink.received(1))
                    .await
                    .is_err()
            );
            // No capability negotiation, channel readiness, formats or data at the server.
            assert!(peer.calls.lock().unwrap().is_empty());
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn send_before_clipboard_ready() {
    wall_clock(async {
        let (_server, peer, _sink, connection) = open(true).await;
        // Queue before run can process MonitorReady or FormatListResponse.
        let (tx, rx) = mpsc::channel(1);
        tx.send(RdpCommand::Clipboard("early 😀".into()))
            .await
            .unwrap();
        let (result, ()) = tokio::join!(connection.run(rx), async {
            peer.until(|calls| calls.contains(&ServerCall::Text("early 😀".into())))
                .await;
            drop(tx);
        });
        assert_eq!(result, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn oversized_text_is_bounded_both_ways() {
    wall_clock(async {
        let (_server, peer, sink, connection) = open(true).await;
        let logs = Arc::new(Mutex::new(String::new()));
        session(connection, |tx| async {
            peer.initialized().await;
            let expected = "a".repeat(LIMIT - 1);
            peer.copy(&format!("{expected}😀"));
            assert_eq!(sink.received(1).await, [expected]);

            let expected = "b".repeat(LIMIT - 2);
            tx.send(RdpCommand::Clipboard(format!("{expected}你")))
                .await
                .unwrap();
            peer.until(|calls| calls.contains(&ServerCall::Text(expected.clone())))
                .await;
            drop(tx);
        })
        .with_subscriber(LogRecorder(Arc::clone(&logs)))
        .await;
        let logs = logs.lock().unwrap();
        assert!(logs.contains("Truncating incoming RDP clipboard text"));
        assert!(logs.contains("Truncating outgoing RDP clipboard text"));
        assert!(!logs.contains("aaaaaaaa"));
        assert!(!logs.contains("bbbbbbbb"));
    })
    .await;
}

#[tokio::test]
async fn malformed_or_failed_data_does_not_disconnect() {
    wall_clock(async {
        let (mut server, peer, sink, connection) = open(true).await;
        session(connection, |tx| async {
            peer.initialized().await;
            for (index, data) in [
                FormatDataResponse::new_error(),
                FormatDataResponse::new_data(vec![0]),
                FormatDataResponse::new_data(vec![0x3d, 0xd8, 0, 0]),
            ]
            .into_iter()
            .enumerate()
            {
                *peer.data.lock().unwrap() = data;
                peer.announce(&[UNICODE]);
                peer.until(|calls| {
                    calls
                        .iter()
                        .filter(|call| **call == ServerCall::Requested(UNICODE))
                        .count()
                        > index
                })
                .await;
                assert!(
                    tokio::time::timeout(Duration::from_millis(100), sink.received(1))
                        .await
                        .is_err()
                );
            }
            peer.copy("recovered 😀");
            assert_eq!(sink.received(1).await, ["recovered 😀"]);
            // The peer must not get text when requesting an unadvertised format.
            tx.send(RdpCommand::Clipboard("text only".into()))
                .await
                .unwrap();
            peer.until(|calls| calls.contains(&ServerCall::Text("text only".into())))
                .await;
            peer.send(ClipboardMessage::SendInitiatePaste(
                ClipboardFormatId::CF_BITMAP,
            ));
            peer.until(|calls| calls.contains(&ServerCall::DataError))
                .await;
            input_barrier(&mut server, &tx).await;
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn clipboard_remains_bidirectional_while_display_starts_in_background() {
    wall_clock(async {
        let (server, peer, sink, connection) = open(true).await;
        let (tx, rx) = mpsc::channel(16);
        let (_visibility, background) = tokio::sync::watch::channel(true);
        let action = async {
            peer.initialized().await;
            while server.display_suppressed() != Some(true) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            peer.copy("remote while hidden");
            assert_eq!(sink.received(1).await, ["remote while hidden"]);
            tx.send(RdpCommand::Clipboard("local while hidden".into()))
                .await
                .unwrap();
            peer.until(|calls| calls.contains(&ServerCall::Text("local while hidden".into())))
                .await;
            assert_eq!(server.display_suppressed(), Some(true));
            assert_eq!(server.connections(), 1);
            drop(tx);
        };
        let (result, ()) =
            tokio::join!(connection.run_with_background(rx, background, 124), action);
        assert_eq!(result, Ok(()));
    })
    .await;
}
