use std::collections::BTreeMap;
use std::net::SocketAddr;
use std::sync::Arc;
use std::time::{Duration, Instant};

use host_core::{HostRuntime, RuntimeOptions};
use host_fake::FakePlatform;
use quinn::crypto::rustls::QuicClientConfig;
use rsp_core::control::{encode, EnvelopeDecoder, MAX_CONTROL_FRAME};
use rsp_core::errors::ErrorCode;
use rsp_core::features;
use rsp_core::pb;
use tokio::task::JoinHandle;
mod common;

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

async fn wait_for(mut predicate: impl FnMut() -> bool) {
    tokio::time::timeout(Duration::from_secs(2), async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition becomes true within timeout");
}

struct RunningHost {
    endpoint: quinn::Endpoint,
    task: JoinHandle<()>,
    addr: SocketAddr,
    platform: FakePlatform,
}

impl RunningHost {
    fn start(view_only: bool) -> Self {
        let (endpoint, _) = host_core::listen::bind("127.0.0.1:0".parse().expect("loopback"))
            .expect("bind fake host");
        let addr = endpoint.local_addr().expect("host address");
        let platform = FakePlatform::new();
        let runtime = HostRuntime::new(
            platform.clone(),
            RuntimeOptions {
                bind: Some(addr),
                view_only,
                dump: None,
                ..RuntimeOptions::default()
            },
        )
        .with_store(common::paired_store());
        let serve_endpoint = endpoint.clone();
        let task = tokio::spawn(async move {
            if let Err(error) = runtime.serve(serve_endpoint).await {
                panic!("fake host failed: {error}");
            }
        });
        Self {
            endpoint,
            task,
            addr,
            platform,
        }
    }

    async fn connect(&self) -> RawClient {
        RawClient::connect(self.addr).await
    }

    async fn stop(mut self) {
        self.endpoint.close(0u32.into(), b"test complete");
        tokio::time::timeout(Duration::from_secs(3), &mut self.task)
            .await
            .expect("host stops")
            .expect("host task joins");
    }
}

impl Drop for RunningHost {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"test dropped");
        self.task.abort();
    }
}

struct RawClient {
    conn: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    decoder: EnvelopeDecoder,
    endpoint: quinn::Endpoint,
    _input: Option<quinn::SendStream>,
}

#[derive(Debug)]
struct CloseObservation {
    messages: Vec<ObservedControl>,
    close: quinn::ApplicationClose,
}

#[derive(Debug, PartialEq)]
enum ObservedControl {
    Error {
        code: u16,
        detail: BTreeMap<String, String>,
    },
    Goodbye(pb::Goodbye),
}

impl RawClient {
    async fn connect(addr: SocketAddr) -> Self {
        let mut tls =
            rsp_transport::tls::client_config(common::device(), rsp_transport::tls::ServerPin::Any)
                .expect("TLS");
        tls.alpn_protocols = vec![rsp_core::ALPN.to_vec()];
        let crypto = QuicClientConfig::try_from(tls).expect("QUIC client config");
        let mut client = quinn::ClientConfig::new(Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_millis(500)));
        client.transport_config(Arc::new(transport));

        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().expect("client bind"))
            .expect("client endpoint");
        endpoint.set_default_client_config(client);
        let conn = tokio::time::timeout(
            TEST_TIMEOUT,
            endpoint
                .connect(addr, rsp_core::SERVER_NAME)
                .expect("client connect"),
        )
        .await
        .expect("QUIC handshake within timeout")
        .expect("QUIC handshake succeeds");
        let (send, recv) = conn.open_bi().await.expect("open control stream");
        let input = conn.open_uni().await.ok();
        Self {
            conn,
            send,
            recv,
            decoder: EnvelopeDecoder::new(),
            endpoint,
            _input: input,
        }
    }

    async fn send(&mut self, envelope: impl Into<pb::Envelope>) {
        self.send_bytes(&encode(&envelope.into())).await;
    }

    async fn send_bytes(&mut self, bytes: &[u8]) {
        tokio::time::timeout(TEST_TIMEOUT, self.send.write_all(bytes))
            .await
            .expect("control write within timeout")
            .expect("control write succeeds");
    }

    async fn next(&mut self) -> pb::Envelope {
        tokio::time::timeout(TEST_TIMEOUT, async {
            let mut buf = [0u8; 4096];
            loop {
                if let Some(envelope) = self.decoder.next().expect("control frame decodes") {
                    return envelope;
                }
                let n = self
                    .recv
                    .read(&mut buf)
                    .await
                    .expect("control read")
                    .unwrap_or(0);
                assert_ne!(n, 0, "host closed before the next control message");
                self.decoder.push(&buf[..n]);
            }
        })
        .await
        .expect("control message within timeout")
    }

    async fn hello(&mut self, protocol: u32) -> (pb::Envelope, pb::Envelope) {
        self.hello_with_version(protocol, "0.3.1").await
    }

    async fn hello_with_version(
        &mut self,
        protocol: u32,
        app_version: &str,
    ) -> (pb::Envelope, pb::Envelope) {
        self.send(pb::ClientHello {
            protocol,
            app_version: app_version.into(),
            features: features::FEC_RS | features::MULTI_DISPLAY,
            required_features: 0,
            device_name: "raw-control-test".into(),
            platform: pb::Platform::Linux as i32,
            intent: pb::Intent::Control as i32,
            resume_token: Vec::new(),
            caps: None,
            path: pb::PathKind::Lan as i32,
            transport: pb::Transport::Quic as i32,
        })
        .await;
        let hello = self.next().await;
        let server_hello = expect_server_hello(hello.clone());
        let auth = self.next().await;
        if matches!(
            auth.msg,
            Some(pb::envelope::Msg::AuthResult(ref result))
                if result.status == pb::auth_result::Status::Ok as i32
        ) {
            let state = self.next().await;
            expect_session_state(state, &auth, &server_hello);
        }
        (hello, auth)
    }

    async fn next_skipping_session_state<T>(
        &mut self,
        expected: &str,
        mut f: impl FnMut(&pb::envelope::Msg) -> Option<T>,
    ) -> T {
        tokio::time::timeout(TEST_TIMEOUT, async {
            loop {
                let message = self
                    .next()
                    .await
                    .msg
                    .expect("control envelope carries a message");
                if matches!(message, pb::envelope::Msg::SessionState(_)) {
                    continue;
                }
                if let Some(value) = f(&message) {
                    return value;
                }
                panic!("expected {expected}, got {message:?}");
            }
        })
        .await
        .unwrap_or_else(|_| panic!("{expected} within timeout"))
    }

    async fn start_stream(&mut self) -> pb::StreamConfig {
        self.send(pb::StartStream {
            display_id: 1,
            codec: pb::Codec::Hevc as i32,
            max_width: 1280,
            max_height: 720,
            fps: 60,
            bitrate_kbps: 2000,
            quality: pb::Quality::Auto as i32,
        })
        .await;
        self.next_skipping_session_state("StreamConfig", |message| match message {
            pb::envelope::Msg::StreamConfig(config) => Some(*config),
            _ => None,
        })
        .await
    }

    async fn ping(&mut self, t1_us: u64) -> pb::Pong {
        self.send(pb::Ping { t1_us }).await;
        self.next_skipping_session_state("Pong", |message| match message {
            pb::envelope::Msg::Pong(pong) => Some(*pong),
            _ => None,
        })
        .await
    }

    async fn closed(&self) -> Option<quinn::ConnectionError> {
        self.closed_within(TEST_TIMEOUT).await
    }

    async fn application_close(&self) -> quinn::ApplicationClose {
        match self.closed().await.expect("connection closes") {
            quinn::ConnectionError::ApplicationClosed(close) => close,
            other => panic!("expected an application close, got {other:?}"),
        }
    }

    async fn closed_within(&self, wait: Duration) -> Option<quinn::ConnectionError> {
        tokio::time::timeout(wait, self.conn.closed()).await.ok()
    }

    async fn next_optional(&mut self, wait: Duration) -> Option<pb::Envelope> {
        tokio::time::timeout(wait, async {
            let mut buf = [0u8; 4096];
            loop {
                if let Ok(Some(envelope)) = self.decoder.next() {
                    return Some(envelope);
                }
                match self.recv.read(&mut buf).await {
                    Ok(Some(n)) => self.decoder.push(&buf[..n]),
                    Ok(None) | Err(_) => return None,
                }
            }
        })
        .await
        .ok()
        .flatten()
    }

    async fn collect_until_close(&mut self) -> CloseObservation {
        let mut messages = Vec::new();
        while let Some(envelope) = self.next_optional(Duration::from_secs(2)).await {
            match envelope.msg {
                Some(pb::envelope::Msg::Error(error)) => {
                    messages.push(ObservedControl::Error {
                        code: error.code.min(u32::from(u16::MAX)) as u16,
                        detail: error.detail.into_iter().collect(),
                    });
                }
                Some(pb::envelope::Msg::Goodbye(goodbye)) => {
                    messages.push(ObservedControl::Goodbye(goodbye));
                }
                _ => {}
            }
        }
        CloseObservation {
            messages,
            close: self.application_close().await,
        }
    }

    fn close(&self) {
        self.conn.close(0u32.into(), b"test done");
        self.endpoint.close(0u32.into(), b"test done");
    }
}

impl Drop for RawClient {
    fn drop(&mut self) {
        self.conn.close(0u32.into(), b"test done");
        self.endpoint.close(0u32.into(), b"test done");
    }
}

fn expect_server_hello(envelope: pb::Envelope) -> pb::ServerHello {
    match envelope.msg {
        Some(pb::envelope::Msg::ServerHello(hello)) => hello,
        other => panic!("expected ServerHello, got {other:?}"),
    }
}

fn expect_auth_result(envelope: pb::Envelope) -> pb::AuthResult {
    match envelope.msg {
        Some(pb::envelope::Msg::AuthResult(auth)) => auth,
        other => panic!("expected AuthResult, got {other:?}"),
    }
}

fn expect_session_state(
    envelope: pb::Envelope,
    auth: &pb::Envelope,
    server: &pb::ServerHello,
) -> pb::SessionState {
    let Some(pb::envelope::Msg::AuthResult(auth)) = auth.msg.as_ref() else {
        panic!("expected AuthResult before SessionState, got {auth:?}");
    };
    match envelope.msg {
        Some(pb::envelope::Msg::SessionState(state)) => {
            assert_eq!(
                state.your_role, auth.role,
                "SessionState role differs from AuthResult"
            );
            if auth.role == pb::Role::Controller as i32 {
                let controller = state
                    .controller
                    .as_ref()
                    .expect("controller SessionState names the controller");
                let peer = server
                    .peer
                    .as_ref()
                    .expect("ServerHello identifies the connected device");
                assert_eq!(
                    controller.device_id, peer.device_id,
                    "controller SessionState names another device"
                );
            }
            state
        }
        other => panic!("expected SessionState after AuthResult, got {other:?}"),
    }
}

#[tokio::test]
async fn handshake_roles_and_stream_config_follow_03_section_4_1() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        for (view_only, expected_role) in [
            (false, pb::Role::Controller as i32),
            (true, pb::Role::Viewer as i32),
        ] {
            let host = RunningHost::start(view_only);
            let mut client = host.connect().await;
            let (server_hello, auth) = client.hello(features::PROTOCOL_VERSION).await;
            let server_hello = expect_server_hello(server_hello);
            assert_eq!(server_hello.protocol, features::PROTOCOL_VERSION);
            assert_eq!(
                server_hello.features & (features::FEC_RS | features::MULTI_DISPLAY),
                features::FEC_RS | features::MULTI_DISPLAY
            );
            assert!(!server_hello.displays.is_empty());
            assert!(server_hello.displays.iter().any(|display| display.primary));

            let auth = expect_auth_result(auth);
            assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
            assert_eq!(auth.role, expected_role);
            assert_eq!(auth.session_token.len(), 16);
            assert!(!auth.resumed);

            let config = client.start_stream().await;
            assert_eq!(config.epoch, 1);
            assert_eq!(config.display_id, server_hello.displays[0].id);
            assert_eq!(config.codec, pb::Codec::Hevc as i32);
            assert!(config.width > 0 && config.height > 0);
            client.close();
            host.stop().await;
        }
    })
    .await
    .expect("handshake case within timeout");
}

#[tokio::test]
async fn ping_is_echoed_with_a_monotonic_host_timestamp() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut client = host.connect().await;
        client.hello(features::PROTOCOL_VERSION).await;
        let t1 = 1_726_000_000_123_456;
        let first = client.ping(t1).await;
        assert_eq!(first.t1_us, t1);
        let second = client.ping(t1 + 1).await;
        assert_eq!(second.t1_us, t1 + 1);
        assert!(second.t2_us >= first.t2_us, "host time went backwards");
        assert!(first.t2_us > first.t1_us);
        client.close();
        drop(host);
    })
    .await
    .expect("ping case within timeout");
}

#[tokio::test]
async fn client_goodbye_ends_the_session_and_allows_an_immediate_new_session() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut first = host.connect().await;
        first.hello(features::PROTOCOL_VERSION).await;
        let _ = first.start_stream().await;
        assert_eq!(host.platform.active_captures(), 1);
        first
            .send(pb::Goodbye {
                reason: pb::goodbye::Reason::UserClosed as i32,
                message: "test complete".into(),
            })
            .await;
        let close = first
            .closed()
            .await
            .expect("host closes after client Goodbye");
        assert!(
            matches!(close, quinn::ConnectionError::ApplicationClosed(ref close) if close.error_code == quinn::VarInt::from_u32(0)),
            "expected a normal close, got {close:?}"
        );
        wait_for(|| host.platform.active_captures() == 0 && host.platform.release_count() >= 1).await;
        assert_eq!(host.platform.active_captures(), 0);
        assert!(host.platform.release_count() >= 1);

        let mut second = host.connect().await;
        let (hello, auth) = second.hello(features::PROTOCOL_VERSION).await;
        expect_server_hello(hello);
        expect_auth_result(auth);
        let _ = second.start_stream().await;
        assert_eq!(host.platform.active_captures(), 1);
        assert!(host.platform.release_count() >= 1);
        second.close();
        host.stop().await;
    })
    .await
    .expect("goodbye case within timeout");
}

#[tokio::test]
async fn protocol_mismatch_returns_the_directional_error_code_and_goodbye() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        for (protocol, expected_error) in [
            (features::PROTOCOL_VERSION + 1, ErrorCode::HostTooOld.code()),
            (features::PROTOCOL_VERSION - 1, ErrorCode::AppTooOld.code()),
        ] {
            let host = RunningHost::start(false);
            let mut client = host.connect().await;
            client
                .send(pb::ClientHello {
                    protocol,
                    app_version: "9.9.9".into(),
                    features: 0,
                    required_features: 0,
                    device_name: "wrong-protocol".into(),
                    platform: pb::Platform::Android as i32,
                    intent: pb::Intent::Control as i32,
                    resume_token: Vec::new(),
                    caps: None,
                    path: pb::PathKind::TailnetDirect as i32,
                    transport: pb::Transport::Quic as i32,
                })
                .await;
            let observed = client.collect_until_close().await;
            assert_eq!(
                observed.messages,
                [
                    ObservedControl::Error {
                        code: expected_error,
                        detail: BTreeMap::new(),
                    },
                    ObservedControl::Goodbye(pb::Goodbye {
                        reason: pb::goodbye::Reason::HostUpdating as i32,
                        message: "protocol mismatch".into(),
                    }),
                ],
                "wrong farewell for protocol {protocol}"
            );
            assert_eq!(observed.close.error_code, quinn::VarInt::from_u32(4));
            assert_eq!(host.platform.active_captures(), 0);
            drop(host);
        }
    })
    .await
    .expect("version mismatch case within timeout");
}

#[tokio::test]
async fn unsupported_required_feature_returns_170_and_goodbye() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut client = host.connect().await;
        let host_features = host_core::session::handlers::host_features();
        assert_ne!(host_features, u64::MAX, "host advertises every feature bit");
        // Pick the highest bit the host does not advertise so future features cannot invalidate this case.
        let unsupported_required_feature = 1u64 << (!host_features).ilog2();
        client
            .send(pb::ClientHello {
                protocol: features::PROTOCOL_VERSION,
                app_version: "0.3.1".into(),
                features: features::FEC_RS,
                required_features: unsupported_required_feature,
                device_name: "needs-unsupported-feature".into(),
                platform: pb::Platform::Linux as i32,
                intent: pb::Intent::Control as i32,
                resume_token: Vec::new(),
                caps: None,
                path: pb::PathKind::Lan as i32,
                transport: pb::Transport::Quic as i32,
            })
            .await;
        let observed = client.collect_until_close().await;
        assert_eq!(
            observed.messages,
            [
                ObservedControl::Error {
                    code: ErrorCode::HostTooOld.code(),
                    detail: BTreeMap::from([
                        ("app_version".to_owned(), "0.3.1".to_owned()),
                        (
                            "host_version".to_owned(),
                            env!("CARGO_PKG_VERSION").to_owned()
                        ),
                    ]),
                },
                ObservedControl::Goodbye(pb::Goodbye {
                    reason: pb::goodbye::Reason::HostUpdating as i32,
                    message: "required feature mismatch".into(),
                }),
            ]
        );
        assert_eq!(observed.close.error_code, quinn::VarInt::from_u32(4));
        assert_eq!(observed.close.reason.as_ref(), b"host updating");
        host.stop().await;
    })
    .await
    .expect("required feature case within timeout");
}

#[tokio::test]
async fn replacement_sends_goodbye_replaced_and_close_code_3() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut first = host.connect().await;
        first.hello(features::PROTOCOL_VERSION).await;
        let _ = first.start_stream().await;

        let mut second = host.connect().await;
        second.hello(features::PROTOCOL_VERSION).await;
        let _ = second.start_stream().await;

        let replaced_conn = first.conn.clone();
        let (goodbye, close) = tokio::join!(
            first.next_skipping_session_state("Goodbye", |message| match message {
                pb::envelope::Msg::Goodbye(goodbye) => Some(goodbye.clone()),
                _ => None,
            }),
            tokio::time::timeout(Duration::from_secs(2), replaced_conn.closed()),
        );
        let close = close.expect("replaced connection closes");
        assert_eq!(goodbye.reason, pb::goodbye::Reason::Replaced as i32);
        assert!(
            matches!(close, quinn::ConnectionError::ApplicationClosed(ref close) if close.error_code == quinn::VarInt::from_u32(3)),
            "expected Goodbye(REPLACED) and application close code 3, got {close:?}"
        );
        second.close();
        host.stop().await;
    })
    .await
    .expect("replacement case within timeout");
}

#[tokio::test]
async fn higher_app_version_with_the_same_protocol_is_accepted() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut client = host.connect().await;
        let (hello, auth) = client
            .hello_with_version(features::PROTOCOL_VERSION, "99.0.0")
            .await;
        expect_server_hello(hello);
        assert_eq!(
            expect_auth_result(auth).status,
            pb::auth_result::Status::Ok as i32
        );
        let _ = client.start_stream().await;
        client.close();
        host.stop().await;
    })
    .await
    .expect("newer app version case within timeout");
}

#[tokio::test]
async fn unknown_oneof_member_is_ignored_and_the_session_keeps_working() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut client = host.connect().await;
        client.hello(features::PROTOCOL_VERSION).await;

        let payload = [0xa2, 0x06, 0x00];
        let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&payload);
        client.send_bytes(&framed).await;

        let pong = client.ping(42).await;
        assert_eq!(pong.t1_us, 42);
        let config = client.start_stream().await;
        assert_eq!(config.epoch, 1);
        client.close();
        host.stop().await;
    })
    .await
    .expect("unknown member case within timeout");
}

#[tokio::test]
async fn malformed_protobuf_sends_error_info_then_closes_with_protocol_error() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut client = host.connect().await;
        client.hello(features::PROTOCOL_VERSION).await;

        let payload = [0xff];
        let mut framed = (payload.len() as u32).to_be_bytes().to_vec();
        framed.extend_from_slice(&payload);
        client.send_bytes(&framed).await;

        let observed = client.collect_until_close().await;
        assert_eq!(
            observed.messages,
            [ObservedControl::Error {
                code: ErrorCode::Other.code(),
                detail: BTreeMap::new(),
            }],
            "protocol errors send one ErrorInfo and no Goodbye"
        );
        assert_eq!(observed.close.error_code, quinn::VarInt::from_u32(1));
        assert_eq!(observed.close.reason.as_ref(), b"protocol error");
        host.stop().await;
    })
    .await
    .expect("malformed protobuf case within timeout");
}

#[tokio::test]
async fn oversized_length_header_closes_immediately_without_payload() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = RunningHost::start(false);
        let mut client = host.connect().await;
        client.hello(features::PROTOCOL_VERSION).await;

        let oversized = (MAX_CONTROL_FRAME as u32 + 1).to_be_bytes();
        let started = Instant::now();
        client.send_bytes(&oversized).await;
        let close = match client.closed().await.expect("connection closes") {
            quinn::ConnectionError::ApplicationClosed(close) => close,
            other => panic!("expected an application close, got {other:?}"),
        };
        assert!(
            started.elapsed() < Duration::from_secs(1),
            "host waited for payload instead of closing immediately ({:?})",
            started.elapsed()
        );
        assert_eq!(
            close.error_code,
            quinn::VarInt::from_u32(1),
            "oversized frames must close as a protocol error: {close:?}"
        );
        assert_eq!(close.reason.as_ref(), b"protocol error");
        host.stop().await;
    })
    .await
    .expect("oversized frame case within timeout");
}

#[tokio::test]
async fn rsp_client_maps_host_goodbye_to_an_error_code() {
    tokio::time::timeout(TEST_TIMEOUT, async {
        let host = GoodbyeServer::start(pb::goodbye::Reason::Revoked);
        let (_client, mut events) = rsp_client::connect(rsp_client::ClientOptions {
            addr: host.addr,
            client_name: "goodbye-mapping".into(),
            start: rsp_client::StartParams::default(),
            pin_sha256: None,
        })
        .await
        .expect("rsp-client connects");

        let mapped = tokio::time::timeout(Duration::from_secs(2), async {
            let mut observed = Vec::new();
            while let Some(event) = events.recv().await {
                match event {
                    rsp_client::ClientEvent::HostError { code, message } => {
                        assert_eq!(
                            code,
                            ErrorCode::DeviceRevoked.code(),
                            "Goodbye(REVOKED) mapped to the wrong error: {message}"
                        );
                        observed.push(format!("HostError({code})"));
                    }
                    rsp_client::ClientEvent::Closed { reason } => {
                        observed.push(format!("Closed({reason})"));
                        if observed.first().is_some_and(|line| line == "HostError(180)") {
                            assert_eq!(reason, "REVOKED");
                            return observed;
                        }
                    }
                    other => observed.push(format!("{other:?}")),
                }
            }
            panic!(
                "rsp-client saw {} but never mapped Goodbye(REVOKED) to ErrorCode::DeviceRevoked (180)",
                observed.join(", ")
            );
        })
        .await;
        assert_eq!(
            mapped.expect("rsp-client maps Goodbye(REVOKED) within two seconds"),
            ["HostError(180)", "Closed(REVOKED)"]
        );
        drop(host);
    })
    .await
    .expect("rsp-client goodbye case within timeout");
}

struct GoodbyeServer {
    endpoint: quinn::Endpoint,
    task: JoinHandle<()>,
    addr: SocketAddr,
}

impl GoodbyeServer {
    fn start(reason: pb::goodbye::Reason) -> Self {
        // The compatibility wrapper intentionally has no device certificate.
        // Only this scripted loopback fixture uses optional client auth.
        let identity = rsp_transport::identity::LocalIdentity::generate_software(
            rsp_transport::identity::CertKind::Host,
        )
        .expect("test host identity");
        let tls = rsp_transport::tls::server_config_with_client_auth(
            &identity,
            rsp_transport::tls::ClientAuth::Optional,
        )
        .expect("development TLS");
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(tls).expect("QUIC TLS");
        let endpoint = quinn::Endpoint::server(
            quinn::ServerConfig::with_crypto(Arc::new(crypto)),
            "127.0.0.1:0".parse().expect("loopback"),
        )
        .expect("bind goodbye server");
        let addr = endpoint.local_addr().expect("goodbye server address");
        let task_endpoint = endpoint.clone();
        let task = tokio::spawn(async move {
            let Some(incoming) = task_endpoint.accept().await else {
                return;
            };
            let conn = incoming.await.expect("goodbye server handshake");
            let (mut send, mut recv) = conn.accept_bi().await.expect("control stream");
            let mut hello = Vec::new();
            let mut buf = [0u8; 1024];
            while hello.len() < 4 {
                let Some(n) = recv.read(&mut buf).await.expect("read client hello") else {
                    return;
                };
                hello.extend_from_slice(&buf[..n]);
            }
            send.write_all(&encode(
                &pb::Goodbye {
                    reason: reason as i32,
                    message: "device revoked".into(),
                }
                .into(),
            ))
            .await
            .expect("write host goodbye");
            tokio::time::sleep(Duration::from_millis(100)).await;
        });
        Self {
            endpoint,
            task,
            addr,
        }
    }
}

impl Drop for GoodbyeServer {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"test done");
        self.task.abort();
    }
}
