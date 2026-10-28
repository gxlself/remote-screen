#![cfg(feature = "testkit")]

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use ironrdp_core::encode_vec;
use ironrdp_pdu::nego::{ConnectionConfirm, FailureCode, ResponseFlags, SecurityProtocol};
use ironrdp_pdu::x224::X224;
use ironrdp_tokio::{FramedWrite as _, TokioFramed};
use rs_rdp::client::{connect, AsyncReadWrite, RdpConnection};
use rs_rdp::testkit::{FakeInput, FakeRdpServer, FakeSecurity};
use rs_rdp::tls::{fingerprint_hex, server_public_key};
use rs_rdp::{
    CertCheck, MouseButton, RdpAudioSink, RdpCommand, RdpConfig, RdpCredentials, RdpError,
    RdpInput, RdpSink, Rect,
};
use tokio::net::TcpStream;
use tokio::sync::{mpsc, Notify};
use zeroize::Zeroizing;

async fn wall_clock(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("client test exceeded its 20-second wall-clock budget");
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    Resize(u16, u16),
    Update(Rect, Vec<u8>),
    Commit,
    PointerBitmap(u16, u16, u16, u16, Vec<u8>),
    PointerHidden,
    PointerDefault,
    PointerPos(u16, u16),
}

#[derive(Default)]
struct RecSink {
    calls: Mutex<Vec<Call>>,
    changed: Notify,
}

impl RecSink {
    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
        self.changed.notify_one();
    }

    async fn until(&self, predicate: impl Fn(&[Call]) -> bool) -> Vec<Call> {
        loop {
            let notified = self.changed.notified();
            {
                let calls = self.calls.lock().unwrap();
                if predicate(&calls) {
                    return calls.clone();
                }
            }
            notified.await;
        }
    }
}

impl RdpSink for RecSink {
    fn resize(&self, w: u16, h: u16) {
        self.record(Call::Resize(w, h));
    }

    fn update(&self, image: &[u8], stride: usize, rect: Rect) {
        let mut pixels = Vec::new();
        for y in usize::from(rect.y)..usize::from(rect.y) + usize::from(rect.h) {
            let start = y * stride + usize::from(rect.x) * 4;
            pixels.extend_from_slice(&image[start..start + usize::from(rect.w) * 4]);
        }
        self.record(Call::Update(rect, pixels));
    }

    fn commit(&self) {
        self.record(Call::Commit);
    }

    fn pointer_bitmap(&self, w: u16, h: u16, hot_x: u16, hot_y: u16, rgba: &[u8]) {
        self.record(Call::PointerBitmap(w, h, hot_x, hot_y, rgba.to_vec()));
    }

    fn pointer_hidden(&self) {
        self.record(Call::PointerHidden);
    }

    fn pointer_default(&self) {
        self.record(Call::PointerDefault);
    }

    fn pointer_pos(&self, x: u16, y: u16) {
        self.record(Call::PointerPos(x, y));
    }
}

struct AcceptAll;
struct RejectAll;

#[derive(Default)]
struct Recorder(Mutex<Vec<[u8; 32]>>);

#[async_trait]
impl CertCheck for AcceptAll {
    async fn check(&self, _: [u8; 32]) -> bool {
        true
    }
}

#[async_trait]
impl CertCheck for RejectAll {
    async fn check(&self, _: [u8; 32]) -> bool {
        false
    }
}

#[async_trait]
impl CertCheck for Recorder {
    async fn check(&self, fp: [u8; 32]) -> bool {
        self.0.lock().unwrap().push(fp);
        true
    }
}

fn config() -> RdpConfig {
    RdpConfig {
        server_name: "127.0.0.1".into(),
        credentials: None,
        // Deliberately different from the server's negotiated dimensions.
        width: 800,
        height: 600,
        scale_percent: 100,
        client_name: "rdp-test".into(),
        cert: Arc::new(AcceptAll),
        gfx: false,
        monitors: 1,
        audio: None,
        clipboard: false,
    }
}

fn hybrid() -> FakeSecurity {
    FakeSecurity::Hybrid {
        user: "alice".into(),
        password: "pw-1".into(),
        domain: None,
    }
}

fn credentials(password: &str) -> RdpCredentials {
    RdpCredentials {
        user: "alice".into(),
        password: Zeroizing::new(password.into()),
        domain: None,
    }
}

async fn open(
    server: &FakeRdpServer,
    cfg: RdpConfig,
    sink: Arc<RecSink>,
) -> Result<RdpConnection, RdpError> {
    let stream = TcpStream::connect(server.addr()).await.unwrap();
    stream.set_nodelay(true).unwrap();
    // The trait object is Send but not Sync, as permitted by connect's contract.
    let stream: Box<dyn AsyncReadWrite> = Box::new(stream);
    require_send(connect(stream, cfg, sink)).await
}

fn require_send<T: Future + Send>(future: T) -> T {
    future
}

async fn session<F, Fut>(connection: RdpConnection, action: F)
where
    F: FnOnce(mpsc::Sender<RdpCommand>) -> Fut,
    Fut: Future<Output = ()>,
{
    let (tx, rx) = mpsc::channel(32);
    let (result, ()) = tokio::join!(require_send(connection.run(rx)), action(tx));
    assert_eq!(result, Ok(()));
}

async fn assert_bitmap(sink: &RecSink) {
    let calls = sink.until(|calls| calls.contains(&Call::Commit)).await;
    assert_eq!(calls.first(), Some(&Call::Resize(1024, 768)));
    let (index, rect, pixels) = calls
        .iter()
        .enumerate()
        .find_map(|(index, call)| match call {
            Call::Update(rect, pixels)
                if rect.x == 0 && rect.y == 0 && rect.w >= 16 && rect.h >= 16 =>
            {
                Some((index, rect, pixels))
            }
            _ => None,
        })
        .expect("no update covered the injected bitmap");
    let pixel = (8 * usize::from(rect.w) + 8) * 4;
    assert_eq!(&pixels[pixel..pixel + 4], &[255, 0, 0, 255]);
    assert_eq!(calls.get(index + 1), Some(&Call::Commit));
    assert_eq!(
        calls.iter().filter(|call| **call == Call::Commit).count(),
        1
    );
}

#[tokio::test]
async fn tls_only_bitmap() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let sink = Arc::new(RecSink::default());
        let connection = open(&server, config(), sink.clone()).await.unwrap();
        assert_eq!(connection.desktop_size(), (1024, 768));
        session(connection, |tx| async {
            sink.until(|calls| calls.contains(&Call::Resize(1024, 768)))
                .await;
            server.push_bitmap(0, 0, 16, 16, [0, 0, 255, 255].repeat(16 * 16));
            assert_bitmap(&sink).await;
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn hybrid_ok() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        let mut cfg = config();
        cfg.credentials = Some(credentials("pw-1"));
        let connection = open(&server, cfg, Arc::new(RecSink::default()))
            .await
            .unwrap();
        assert_eq!(connection.desktop_size(), (1024, 768));
    })
    .await;
}

#[tokio::test]
async fn hybrid_bad_password() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        let mut cfg = config();
        cfg.credentials = Some(credentials("wrong"));
        assert!(matches!(
            open(&server, cfg, Arc::new(RecSink::default())).await,
            Err(RdpError::AuthFailed)
        ));
    })
    .await;
}

#[tokio::test]
async fn nla_required() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        assert!(matches!(
            open(&server, config(), Arc::new(RecSink::default())).await,
            Err(RdpError::NlaRequired)
        ));
    })
    .await;
}

#[tokio::test]
async fn cert_fp_passed() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid(), 1024, 768).await.unwrap();
        let recorder = Arc::new(Recorder::default());
        let mut cfg = config();
        cfg.server_name = "fake-rdp".into();
        cfg.credentials = Some(credentials("pw-1"));
        cfg.cert = recorder.clone();
        let connection = open(&server, cfg, Arc::new(RecSink::default()))
            .await
            .unwrap();
        assert_eq!(*recorder.0.lock().unwrap(), vec![server.cert_sha256()]);
        drop(connection);
        let mut cfg = config();
        cfg.credentials = Some(credentials("wrong"));
        cfg.cert = Arc::new(RejectAll);
        // Rejection must precede CredSSP, even when the password is wrong.
        assert!(matches!(
            open(&server, cfg, Arc::new(RecSink::default())).await,
            Err(RdpError::CertRejected)
        ));
    })
    .await;
}

#[tokio::test]
async fn inputs_reach_server() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let connection = open(&server, config(), Arc::new(RecSink::default()))
            .await
            .unwrap();
        session(connection, |tx| async {
            let inputs = [
                (
                    RdpInput::Key {
                        scancode: 0x1e,
                        extended: false,
                        down: true,
                    },
                    FakeInput::Key {
                        scancode: 0x1e,
                        extended: false,
                        down: true,
                    },
                ),
                (
                    RdpInput::Key {
                        scancode: 0x4d,
                        extended: true,
                        down: false,
                    },
                    FakeInput::Key {
                        scancode: 0x4d,
                        extended: true,
                        down: false,
                    },
                ),
                (
                    RdpInput::Unicode {
                        code: 0x4e2d,
                        down: true,
                    },
                    FakeInput::Unicode {
                        code: 0x4e2d,
                        down: true,
                    },
                ),
                (
                    RdpInput::MouseMove { x: 10, y: 20 },
                    FakeInput::MouseMove { x: 10, y: 20 },
                ),
                (
                    RdpInput::MouseButton {
                        button: MouseButton::Left,
                        down: true,
                        x: 10,
                        y: 20,
                    },
                    FakeInput::Button {
                        button: 0,
                        down: true,
                    },
                ),
                (
                    RdpInput::MouseButton {
                        button: MouseButton::X1,
                        down: true,
                        x: 10,
                        y: 20,
                    },
                    FakeInput::Button {
                        button: 3,
                        down: true,
                    },
                ),
                (
                    RdpInput::Wheel {
                        horizontal: false,
                        units: 120,
                    },
                    FakeInput::Wheel {
                        horizontal: false,
                        units: 120,
                    },
                ),
                (
                    RdpInput::Wheel {
                        horizontal: true,
                        units: -120,
                    },
                    FakeInput::Wheel {
                        horizontal: true,
                        units: -120,
                    },
                ),
                (
                    RdpInput::Unicode {
                        code: 0x4e2d,
                        down: false,
                    },
                    FakeInput::Unicode {
                        code: 0x4e2d,
                        down: false,
                    },
                ),
                (
                    RdpInput::MouseButton {
                        button: MouseButton::Right,
                        down: false,
                        x: 10,
                        y: 20,
                    },
                    FakeInput::Button {
                        button: 1,
                        down: false,
                    },
                ),
                (
                    RdpInput::MouseButton {
                        button: MouseButton::Middle,
                        down: true,
                        x: 10,
                        y: 20,
                    },
                    FakeInput::Button {
                        button: 2,
                        down: true,
                    },
                ),
                (
                    RdpInput::MouseButton {
                        button: MouseButton::X2,
                        down: false,
                        x: 10,
                        y: 20,
                    },
                    FakeInput::Button {
                        button: 4,
                        down: false,
                    },
                ),
            ];
            for (input, expected) in inputs {
                tx.send(RdpCommand::Input(input)).await.unwrap();
                assert_eq!(
                    server.next_input(Duration::from_secs(1)).await,
                    Some(expected)
                );
            }
            tx.send(RdpCommand::Input(RdpInput::ReleaseAll))
                .await
                .unwrap();
            tx.send(RdpCommand::Input(RdpInput::MouseMove { x: 1, y: 2 }))
                .await
                .unwrap();
            assert_eq!(
                server.next_input(Duration::from_secs(1)).await,
                Some(FakeInput::MouseMove { x: 1, y: 2 })
            );
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn pointer_events() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let sink = Arc::new(RecSink::default());
        let connection = open(&server, config(), sink.clone()).await.unwrap();
        session(connection, |tx| async {
            // Distinct rows catch channel swaps, vertical flips and row padding.
            // Partial alpha catches accidental extra (un)premultiplication.
            let rgba = vec![1, 2, 3, 255, 90, 80, 70, 255, 15, 30, 60, 128, 4, 5, 6, 255];
            let bitmap = Call::PointerBitmap(2, 2, 1, 1, rgba.clone());
            server.push_pointer_bitmap(2, 2, 1, 1, rgba);
            sink.until(|calls| calls.contains(&bitmap)).await;
            server.push_pointer_hidden();
            let calls = sink
                .until(|calls| calls.contains(&Call::PointerHidden))
                .await;
            let bitmap_at = calls.iter().position(|call| call == &bitmap).unwrap();
            let hidden_at = calls
                .iter()
                .position(|call| call == &Call::PointerHidden)
                .unwrap();
            assert!(bitmap_at < hidden_at);
            server.push_pointer_default();
            sink.until(|calls| calls.contains(&Call::PointerDefault))
                .await;
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn input_closed_is_ok() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let connection = open(&server, config(), Arc::new(RecSink::default()))
            .await
            .unwrap();
        let (tx, rx) = mpsc::channel(1);
        drop(tx);
        assert_eq!(
            tokio::time::timeout(Duration::from_secs(5), connection.run(rx))
                .await
                .unwrap(),
            Ok(())
        );
        // The listener remains usable after a graceful client shutdown.
        let _connection = open(&server, config(), Arc::new(RecSink::default()))
            .await
            .unwrap();
        assert_eq!(server.connections(), 2);
    })
    .await;
}

#[tokio::test]
async fn server_disconnect() {
    wall_clock(async {
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let connection = open(&server, config(), Arc::new(RecSink::default()))
            .await
            .unwrap();
        let (tx, rx) = mpsc::channel(1);
        server.disconnect();
        assert!(matches!(connection.run(rx).await, Err(RdpError::Closed(_))));
        drop(tx);
    })
    .await;
}

#[tokio::test]
async fn fingerprint_hex_format() {
    wall_clock(async {
        assert_eq!(fingerprint_hex(&[0xab; 32]), "ab".repeat(32));
        assert_eq!(fingerprint_hex(&[0; 32]), "0".repeat(64));
    })
    .await;
}

#[tokio::test]
async fn server_public_key_matches() {
    wall_clock(async {
        let key = rcgen::KeyPair::generate_for(&rcgen::PKCS_ECDSA_P256_SHA256).unwrap();
        let cert = rcgen::CertificateParams::new(vec!["hs.example.com".into()])
            .unwrap()
            .self_signed(&key)
            .unwrap();
        let public_key = server_public_key(cert.der()).unwrap();
        assert_eq!(public_key, key.public_key_raw());
        assert_eq!(public_key.len(), 65);
        assert_eq!(public_key[0], 0x04);
        assert_eq!(
            server_public_key(b"bad certificate"),
            Err(RdpError::Tls("bad certificate".into()))
        );
    })
    .await;
}

#[tokio::test]
async fn map_connector_error_table() {
    wall_clock(async {
        // Construct actual negotiation PDUs; the unit table in client.rs also
        // covers nested IO/TLS/SSPI errors without making the mapper public.
        for code in [
            FailureCode::SSL_REQUIRED_BY_SERVER,
            FailureCode::SSL_NOT_ALLOWED_BY_SERVER,
            FailureCode::SSL_CERT_NOT_ON_SERVER,
            FailureCode::INCONSISTENT_FLAGS,
            FailureCode::HYBRID_REQUIRED_BY_SERVER,
            FailureCode::SSL_WITH_USER_AUTH_REQUIRED_BY_SERVER,
        ] {
            let result = negotiation_response(ConnectionConfirm::Failure { code }).await;
            if code == FailureCode::HYBRID_REQUIRED_BY_SERVER {
                assert!(matches!(result, Err(RdpError::NlaRequired)));
            } else {
                assert!(matches!(result, Err(RdpError::UnsupportedSecurity(reason)) if reason == format!("0x{:08x}", u32::from(code))));
            }
        }
        for protocol in [SecurityProtocol::RDSTLS, SecurityProtocol::SSL | SecurityProtocol::RDSTLS, SecurityProtocol::empty()] {
            let result = negotiation_response(ConnectionConfirm::Response {
                flags: ResponseFlags::empty(),
                protocol,
            }).await;
            assert!(matches!(result, Err(RdpError::UnsupportedSecurity(_))));
        }
    }).await;
}

async fn negotiation_response(response: ConnectionConfirm) -> Result<RdpConnection, RdpError> {
    let (client, server) = tokio::io::duplex(4096);
    let (result, ()) = tokio::join!(
        connect(client, config(), Arc::new(RecSink::default())),
        async {
            let mut framed = TokioFramed::new(server);
            framed.read_pdu().await.unwrap();
            framed
                .write_all(&encode_vec(&X224(response)).unwrap())
                .await
                .unwrap();
        }
    );
    result
}

#[derive(Default)]
struct RecAudio(Mutex<Vec<&'static str>>);

impl RdpAudioSink for RecAudio {
    fn format(&self, _: u32, _: u16) {
        self.0.lock().unwrap().push("format");
    }

    fn samples(&self, _: &[i16]) {
        self.0.lock().unwrap().push("samples");
    }

    fn stop(&self) {
        self.0.lock().unwrap().push("stop");
    }
}

#[tokio::test]
async fn commands_with_stub_hooks() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let sink = Arc::new(RecSink::default());
        let audio = Arc::new(RecAudio::default());
        let mut cfg = config();
        cfg.gfx = true;
        cfg.clipboard = true;
        cfg.audio = Some(audio.clone());
        let connection = open(&server, cfg, sink.clone()).await.unwrap();
        session(connection, |tx| async {
            server.push_bitmap(0, 0, 16, 16, [0, 0, 255, 255].repeat(16 * 16));
            assert_bitmap(&sink).await;
            tx.send(RdpCommand::Clipboard("x".into())).await.unwrap();
            tx.send(RdpCommand::Resize {
                width: 800,
                height: 600,
                scale_percent: 100,
            })
            .await
            .unwrap();
            tx.send(RdpCommand::Input(RdpInput::MouseMove { x: 1, y: 2 }))
                .await
                .unwrap();
            assert_eq!(
                server.next_input(Duration::from_secs(1)).await,
                Some(FakeInput::MouseMove { x: 1, y: 2 })
            );
            assert!(audio.0.lock().unwrap().is_empty());
            drop(tx);
        })
        .await;
    })
    .await;
}

#[tokio::test]
async fn tls_errors_and_ssl_credential_fallback() {
    wall_clock(async {
        let (stream, _peer) = tokio::io::duplex(1024);
        assert!(matches!(
            rs_rdp::tls::upgrade(stream, "bad server name!", &AcceptAll).await,
            Err(RdpError::Tls(reason)) if reason == "bad server name"
        ));
        let (stream, peer) = tokio::io::duplex(1024);
        drop(peer);
        assert!(matches!(
            rs_rdp::tls::upgrade(stream, "::1", &AcceptAll).await,
            Err(RdpError::Tls(_))
        ));
        let server = FakeRdpServer::start(FakeSecurity::Tls, 1024, 768)
            .await
            .unwrap();
        let mut cfg = config();
        cfg.credentials = Some(credentials("pw-1"));
        let connection = open(&server, cfg, Arc::new(RecSink::default()))
            .await
            .unwrap();
        session(connection, |tx| async { drop(tx) }).await;
    })
    .await;
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
            fn record_debug(&mut self, _: &tracing::field::Field, value: &dyn std::fmt::Debug) {
                use std::fmt::Write as _;
                writeln!(self.0, "{value:?}").unwrap();
            }
        }
        event.record(&mut Visitor(&mut self.0.lock().unwrap()));
    }
}

#[tokio::test]
async fn credentials_are_absent_from_trace_logs() {
    use tracing::instrument::WithSubscriber as _;

    wall_clock(async {
        let log = Arc::new(Mutex::new(String::new()));
        let recorded = log.clone();
        async move {
            let mut server = FakeRdpServer::start(
                FakeSecurity::Hybrid {
                    user: "private-test-user".into(),
                    password: "private-test-password".into(),
                    domain: Some("PRIVATE-DOMAIN".into()),
                },
                1024,
                768,
            )
            .await
            .unwrap();
            let mut cfg = config();
            cfg.credentials = Some(RdpCredentials {
                user: "private-test-user".into(),
                password: Zeroizing::new("private-test-password".into()),
                domain: Some("PRIVATE-DOMAIN".into()),
            });
            let connection = open(&server, cfg, Arc::new(RecSink::default()))
                .await
                .unwrap();
            session(connection, |tx| async {
                tx.send(RdpCommand::Input(RdpInput::ReleaseAll))
                    .await
                    .unwrap();
                tx.send(RdpCommand::Input(RdpInput::MouseMove { x: 1, y: 2 }))
                    .await
                    .unwrap();
                assert_eq!(
                    server.next_input(Duration::from_secs(1)).await,
                    Some(FakeInput::MouseMove { x: 1, y: 2 })
                );
                drop(tx);
            })
            .await;
        }
        .with_subscriber(LogRecorder(recorded))
        .await;
        let log = log.lock().unwrap();
        assert!(log.contains("RDP release-all marker"));
        for secret in [
            "private-test-user",
            "private-test-password",
            "PRIVATE-DOMAIN",
        ] {
            assert!(!log.contains(secret), "trace output contained credentials");
        }
    })
    .await;
}

#[tokio::test]
async fn background_suppresses_output_and_input_then_delivers_a_full_frame() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(FakeSecurity::Tls, 320, 240).await.unwrap();
        let sink = Arc::new(RecSink::default());
        let connection = open(&server, config(), sink.clone()).await.unwrap();
        let (tx, rx) = mpsc::channel(16);
        let (visibility, background) = tokio::sync::watch::channel(false);
        let action = async {
            server.push_bitmap(0, 0, 320, 240, [1, 2, 3, 255].repeat(320 * 240));
            sink.until(|calls| calls.contains(&Call::Commit)).await;
            visibility.send_replace(true);
            visibility.send_replace(true);
            while server.display_suppressed() != Some(true) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            let before = sink.calls.lock().unwrap().clone();
            tx.send(RdpCommand::Input(RdpInput::MouseMove { x: 90, y: 80 })).await.unwrap();
            // The fake deliberately continues emitting, exercising in-flight filtering.
            server.push_bitmap(0, 0, 320, 240, [9, 8, 7, 255].repeat(320 * 240));
            server.push_pointer_default();
            assert_eq!(server.next_input(Duration::from_millis(100)).await, None);
            assert_eq!(*sink.calls.lock().unwrap(), before);
            visibility.send_replace(false);
            visibility.send_replace(false);
            while server.display_suppressed() != Some(false) {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
            server.push_bitmap(0, 0, 320, 240, [4, 5, 6, 255].repeat(320 * 240));
            sink.until(|calls| calls.iter().any(|call| matches!(call, Call::Update(rect, pixels) if rect.w == 320 && rect.h == 240 && pixels == &[6, 5, 4, 255].repeat(320 * 240)))).await;
            tx.send(RdpCommand::Input(RdpInput::MouseMove { x: 7, y: 9 })).await.unwrap();
            assert_eq!(server.next_input(Duration::from_secs(5)).await, Some(FakeInput::MouseMove { x: 7, y: 9 }));
            assert_eq!(server.connections(), 1);
            drop(tx);
        };
        let (result, ()) = tokio::join!(require_send(connection.run_with_background(rx, background, 123)), action);
        assert_eq!(result, Ok(()));
    }).await;
}
