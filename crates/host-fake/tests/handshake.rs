use async_trait::async_trait;
use host_core::{
    pairing::{PairApproval, PairApprovalRequest, PairApprover},
    platform::Platform,
    session::handshake::{ConnectionApprovalRequest, ConnectionApprover},
    HostRuntime, RuntimeOptions,
};
use host_fake::FakePlatform;
use rsp_core::{
    control::{encode, EnvelopeDecoder},
    features, pb,
};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    tls::{client_config, ServerPin},
};
use std::{net::SocketAddr, sync::Arc, time::Duration};
mod common;

struct Approval(Option<bool>);
#[async_trait]
impl ConnectionApprover for Approval {
    async fn request(&self, _: ConnectionApprovalRequest) -> bool {
        match self.0 {
            Some(allow) => allow,
            None => std::future::pending().await,
        }
    }
}
#[async_trait]
impl PairApprover for Approval {
    async fn request(&self, _: PairApprovalRequest) -> PairApproval {
        PairApproval {
            allow: self.0.unwrap_or(false),
            permission: rsp_core::trust::Permission::Control,
        }
    }
}

struct Host {
    addr: SocketAddr,
    tcp_addr: SocketAddr,
    security: Arc<host_core::session::handshake::HandshakeContext>,
    platform: FakePlatform,
    task: tokio::task::JoinHandle<host_core::RuntimeResult<()>>,
}
impl Host {
    async fn start(options: RuntimeOptions, approval: Option<bool>) -> Self {
        Self::with_approver(options, Arc::new(Approval(approval))).await
    }
    async fn with_approver(
        mut options: RuntimeOptions,
        approval: Arc<dyn ConnectionApprover>,
    ) -> Self {
        options.bind = Some("127.0.0.1:0".parse().unwrap());
        options.store_dir = common::state_dir();
        let audit = host_core::audit::AuditQueue::open(&options.store_dir).unwrap();
        let platform = FakePlatform::new();
        let host = HostRuntime::new(platform.clone(), options)
            .with_store(common::paired_store())
            .with_audit(audit)
            .with_connection_approver(approval)
            .with_pairing_approver(Arc::new(Approval(Some(true))))
            .bind()
            .await
            .unwrap();
        let addr = host
            .listener
            .bound()
            .into_iter()
            .find(|(_, _, transport)| *transport == rsp_transport::TransportKind::Quic)
            .unwrap()
            .0;
        let security = host.security.clone();
        let tcp_addr = host
            .listener
            .bound()
            .into_iter()
            .find(|(_, _, transport)| *transport == rsp_transport::TransportKind::Tcp)
            .unwrap()
            .0;
        let task = tokio::spawn(host.run());
        Self {
            addr,
            tcp_addr,
            security,
            platform,
            task,
        }
    }
    async fn client(&self, identity: Option<&LocalIdentity>, intent: pb::Intent) -> Client {
        let device = identity.unwrap_or(common::device());
        let expected_device_id =
            identity.map(|identity| rsp_core::ids::id_from_fp(&identity.fingerprint()));
        let mut tls =
            client_config(device, ServerPin::Fingerprint(self.security.identity.fp())).unwrap();
        if identity.is_none() {
            tls.client_auth_cert_resolver = Arc::new(NoCertificate);
        }
        Client::connect(self.addr, tls, intent, expected_device_id).await
    }
}
impl Drop for Host {
    fn drop(&mut self) {
        self.task.abort();
    }
}

#[derive(Debug)]
struct NoCertificate;
impl rustls::client::ResolvesClientCert for NoCertificate {
    fn resolve(
        &self,
        _: &[&[u8]],
        _: &[rustls::SignatureScheme],
    ) -> Option<Arc<rustls::sign::CertifiedKey>> {
        None
    }
    fn has_certs(&self) -> bool {
        false
    }
}

struct Client {
    conn: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    decoder: EnvelopeDecoder,
    endpoint: quinn::Endpoint,
    expected_device_id: Option<String>,
}
impl Client {
    async fn connect(
        addr: SocketAddr,
        tls: rustls::ClientConfig,
        intent: pb::Intent,
        expected_device_id: Option<String>,
    ) -> Self {
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
        let mut config = quinn::ClientConfig::new(Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_millis(200)));
        config.transport_config(Arc::new(transport));
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(config);
        let conn = endpoint
            .connect(addr, rsp_core::SERVER_NAME)
            .unwrap()
            .await
            .unwrap();
        let (send, recv) = conn.open_bi().await.unwrap();
        let mut client = Self {
            conn,
            send,
            recv,
            decoder: EnvelopeDecoder::new(),
            endpoint,
            expected_device_id,
        };
        client
            .send(pb::ClientHello {
                protocol: features::PROTOCOL_VERSION,
                intent: intent as i32,
                device_name: "test device".into(),
                ..Default::default()
            })
            .await;
        client
    }
    async fn send(&mut self, message: impl Into<pb::Envelope>) {
        self.send.write_all(&encode(&message.into())).await.unwrap();
    }
    async fn next(&mut self) -> pb::envelope::Msg {
        tokio::time::timeout(Duration::from_secs(5), async {
            let mut bytes = [0u8; 4096];
            loop {
                if let Some(message) = self.decoder.next().unwrap() {
                    return message.msg.unwrap();
                }
                let n = self
                    .recv
                    .read(&mut bytes)
                    .await
                    .unwrap()
                    .expect("control response");
                self.decoder.push(&bytes[..n]);
            }
        })
        .await
        .expect("response deadline")
    }
    async fn hello(&mut self) -> pb::ServerHello {
        match self.next().await {
            pb::envelope::Msg::ServerHello(hello) => hello,
            other => panic!("expected hello: {other:?}"),
        }
    }
    async fn auth(&mut self, status: pb::auth_result::Status) -> pb::AuthResult {
        match self.next().await {
            pb::envelope::Msg::AuthResult(result) => {
                assert_eq!(result.status, status as i32);
                if status == pb::auth_result::Status::Ok {
                    self.expect_session_state(&result).await;
                }
                result
            }
            other => panic!("expected auth: {other:?}"),
        }
    }
    async fn expect_session_state(&mut self, auth: &pb::AuthResult) {
        match self.next().await {
            pb::envelope::Msg::SessionState(state) => {
                assert_eq!(
                    state.your_role, auth.role,
                    "SessionState role differs from AuthResult"
                );
                if auth.role == pb::Role::Controller as i32 {
                    let controller = state
                        .controller
                        .as_ref()
                        .expect("controller SessionState names the controller");
                    if let Some(expected) = &self.expected_device_id {
                        assert_eq!(
                            controller.device_id, *expected,
                            "controller SessionState names another device"
                        );
                    } else {
                        assert!(
                            controller.device_id.starts_with("anon-"),
                            "unpaired controller has no anonymous identity: {controller:?}"
                        );
                    }
                }
            }
            other => panic!("expected SessionState after AuthResult, got {other:?}"),
        }
    }
    async fn denied(&mut self, code: u32) {
        let result = self.auth(pb::auth_result::Status::Denied).await;
        assert_eq!(result.error.unwrap().code, code);
        assert!(self.recv.read(&mut [0u8; 1]).await.unwrap().is_none());
        match tokio::time::timeout(Duration::from_secs(5), self.conn.closed())
            .await
            .unwrap()
        {
            quinn::ConnectionError::ApplicationClosed(close) => {
                assert_eq!(close.error_code.into_inner(), 2)
            }
            error => panic!("wrong close: {error:?}"),
        }
    }
    async fn next_skipping_session_state<T>(
        &mut self,
        expected: &str,
        mut f: impl FnMut(&pb::envelope::Msg) -> Option<T>,
    ) -> T {
        tokio::time::timeout(Duration::from_secs(5), async {
            loop {
                let message = self.next().await;
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
        .unwrap_or_else(|_| panic!("{expected} timed out"))
    }
    async fn frame(&mut self) {
        self.send(pb::StartStream {
            display_id: 1,
            codec: pb::Codec::Hevc as i32,
            max_width: 1280,
            fps: 30,
            bitrate_kbps: 2000,
            ..Default::default()
        })
        .await;
        self.next_skipping_session_state("StreamConfig", |message| {
            matches!(message, pb::envelope::Msg::StreamConfig(_)).then_some(())
        })
        .await;
        let frame = tokio::time::timeout(Duration::from_secs(3), self.conn.read_datagram())
            .await
            .unwrap()
            .unwrap();
        assert!(rsp_core::video::VideoHeader::parse(&frame).is_ok());
    }
}
impl Drop for Client {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"test done");
    }
}

#[tokio::test]
async fn paired_device_gets_real_identity_ok_and_frames() {
    let host = Host::start(RuntimeOptions::default(), Some(true)).await;
    let mut client = host
        .client(Some(common::device()), pb::Intent::Control)
        .await;
    let hello = client.hello().await;
    assert_eq!(hello.host_id, host.security.identity.host_id());
    assert_eq!(
        hello.features & features::AUDIO_OPUS != 0,
        host.platform.audio_capture_supported()
    );
    let peer = hello.peer.unwrap();
    assert_eq!(
        peer.device_id,
        rsp_core::ids::id_from_fp(&common::device().fingerprint())
    );
    assert_eq!(peer.path, pb::PathKind::Lan as i32);
    let auth = client.auth(pb::auth_result::Status::Ok).await;
    assert_eq!(auth.role, pb::Role::Controller as i32);
    assert_eq!(auth.session_token.len(), 16);
    client.frame().await;
}

#[tokio::test]
async fn unknown_and_revoked_devices_are_denied_without_host_inventory() {
    let host = Host::start(RuntimeOptions::default(), Some(true)).await;
    let unknown = LocalIdentity::generate_software(CertKind::App).unwrap();
    host.client(Some(&unknown), pb::Intent::Control)
        .await
        .denied(181)
        .await;
    host.security
        .store
        .revoke(common::device().fingerprint(), 1)
        .unwrap();
    host.client(Some(common::device()), pb::Intent::Control)
        .await
        .denied(180)
        .await;
    let audit = host.security.audit.as_ref().unwrap().batch(10).unwrap();
    assert_eq!(audit.items.len(), 2);
    assert_eq!(audit.items[0].event, "auth.denied");
    assert_eq!(audit.items[0].detail.as_ref().unwrap()["code"], 181);
    assert_eq!(audit.items[1].detail.as_ref().unwrap()["code"], 180);
}

#[tokio::test]
async fn no_client_certificate_fails_tls() {
    let host = Host::start(RuntimeOptions::default(), Some(true)).await;
    let mut tls = client_config(common::device(), ServerPin::Any).unwrap();
    tls.client_auth_cert_resolver = Arc::new(NoCertificate);
    let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
    let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
    endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(crypto)));
    // TLS 1.3 clients may observe handshake success before the server rejects
    // their missing certificate; the final result must be a TLS transport error.
    if let Ok(conn) = endpoint
        .connect(host.addr, rsp_core::SERVER_NAME)
        .unwrap()
        .await
    {
        let error = tokio::time::timeout(Duration::from_secs(3), conn.closed())
            .await
            .unwrap();
        match error {
            quinn::ConnectionError::ConnectionClosed(close) => {
                assert_eq!(u64::from(close.error_code), 0x174)
            }
            other => panic!("expected TLS certificate_required alert, got {other:?}"),
        }
    }
}

#[tokio::test]
async fn probe_only_receives_minimal_hello_and_goodbye() {
    let host = Host::start(RuntimeOptions::default(), Some(true)).await;
    let unknown = LocalIdentity::generate_software(CertKind::App).unwrap();
    let mut client = host.client(Some(&unknown), pb::Intent::Probe).await;
    let hello = client.hello().await;
    assert_eq!(hello.features, 0);
    assert!(hello.displays.is_empty());
    assert!(hello.host_name.is_empty());
    assert_eq!(hello.os, 0);
    assert!(hello.os_version.is_empty());
    assert!(hello.peer.is_some());
    assert!(
        matches!(client.next().await, pb::envelope::Msg::Goodbye(goodbye) if goodbye.reason == pb::goodbye::Reason::UserClosed as i32)
    );
    assert!(client.recv.read(&mut [0u8; 1]).await.unwrap().is_none());
}

#[tokio::test]
async fn attended_approval_allow_and_reject() {
    for allow in [true, false] {
        let mut options = RuntimeOptions::default();
        options.auth_policy.attended_mode = true;
        let host = Host::start(options, Some(allow)).await;
        let mut client = host
            .client(Some(common::device()), pb::Intent::Control)
            .await;
        client.hello().await;
        assert_eq!(
            client
                .auth(pb::auth_result::Status::PendingApproval)
                .await
                .approval_timeout_s,
            30
        );
        if allow {
            client.auth(pb::auth_result::Status::Ok).await;
            client.frame().await;
        } else {
            client.denied(140).await;
        }
    }
}

#[tokio::test]
async fn attended_approval_timeout_uses_paused_time() {
    let mut options = RuntimeOptions::default();
    options.auth_policy.attended_mode = true;
    let host = Host::start(options, None).await;
    let mut client = host
        .client(Some(common::device()), pb::Intent::Control)
        .await;
    client.hello().await;
    client.auth(pb::auth_result::Status::PendingApproval).await;
    tokio::time::pause();
    tokio::time::sleep(Duration::from_secs(31)).await;
    tokio::time::resume();
    client.denied(140).await;
    assert!(host.platform.injected_events().is_empty());
}

struct DelayedApproval(tokio::sync::Notify);
#[async_trait]
impl ConnectionApprover for DelayedApproval {
    async fn request(&self, _: ConnectionApprovalRequest) -> bool {
        self.0.notified().await;
        true
    }
}

#[tokio::test]
async fn pending_input_is_discarded_and_viewers_cannot_inject() {
    use rsp_core::{input::InputEvent, wire::frame};
    let mut options = RuntimeOptions::default();
    options.auth_policy.attended_mode = true;
    let approver = Arc::new(DelayedApproval(tokio::sync::Notify::new()));
    let host = Host::with_approver(options, approver.clone()).await;
    let mut client = host
        .client(Some(common::device()), pb::Intent::Control)
        .await;
    client.hello().await;
    client.auth(pb::auth_result::Status::PendingApproval).await;
    let mut input = client.conn.open_uni().await.unwrap();
    let before = InputEvent::MouseMove { x: 0.1, y: 0.2 };
    input.write_all(&frame(&before.encode())).await.unwrap();
    let split_frame = frame(&before.encode());
    input.write_all(&split_frame[..5]).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert!(host.platform.injected_events().is_empty());
    approver.0.notify_one();
    client.auth(pb::auth_result::Status::Ok).await;
    input.write_all(&split_frame[5..]).await.unwrap();
    let after = InputEvent::MouseMove { x: 0.3, y: 0.4 };
    input.write_all(&frame(&after.encode())).await.unwrap();
    tokio::time::timeout(Duration::from_secs(2), async {
        while host.platform.injected_events().is_empty() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .unwrap();
    assert_eq!(host.platform.injected_events(), vec![after]);

    let viewer_device = LocalIdentity::generate_software(CertKind::App).unwrap();
    common::pair_device(&host.security.store, &viewer_device);
    let mut viewer = host.client(Some(&viewer_device), pb::Intent::View).await;
    viewer.hello().await;
    viewer.auth(pb::auth_result::Status::PendingApproval).await;
    approver.0.notify_one();
    assert_eq!(
        viewer.auth(pb::auth_result::Status::Ok).await.role,
        pb::Role::Viewer as i32
    );
    let mut input = viewer.conn.open_uni().await.unwrap();
    input.write_all(&frame(&before.encode())).await.unwrap();
    tokio::time::sleep(Duration::from_millis(50)).await;
    assert_eq!(host.platform.injected_events(), vec![after]);
}

#[tokio::test]
async fn pairing_then_same_device_connects_directly() {
    let host = Host::start(RuntimeOptions::default(), Some(true)).await;
    let ticket = host
        .security
        .pairing
        .start(rsp_core::clock::unix_micros())
        .unwrap();
    let device = LocalIdentity::generate_software(CertKind::App).unwrap();
    let mut client = host.client(Some(&device), pb::Intent::Pair).await;
    client.hello().await;
    let mut pairing = rsp_core::pairing::AppPairing::new(
        &ticket.code,
        device.fingerprint(),
        host.security.identity.fp(),
        "new app",
        pb::Platform::Ios,
    )
    .unwrap();
    client.send(pairing.start()).await;
    let spake = match client.next().await {
        pb::envelope::Msg::PairSpake(value) => value,
        other => panic!("{other:?}"),
    };
    let (spake, confirm) = pairing.on_spake(spake).unwrap();
    client.send(spake).await;
    client.send(confirm).await;
    match client.next().await {
        pb::envelope::Msg::PairConfirm(value) => {
            pairing.on_confirm(value).unwrap();
        }
        other => panic!("{other:?}"),
    }
    for expected in [
        pb::pair_result::Status::WaitingLocal,
        pb::pair_result::Status::Ok,
    ] {
        match client.next().await {
            pb::envelope::Msg::PairResult(value) => {
                assert_eq!(value.status, expected as i32);
                pairing.on_result(value);
            }
            other => panic!("{other:?}"),
        }
    }
    let stored = host
        .security
        .store
        .get(&device.fingerprint())
        .unwrap()
        .unwrap();
    assert_eq!(stored.cert_der, device.cert_der.as_ref());
    assert_eq!(stored.paired_via, host_core::store::PairingPath::Lan);
    let mut connected = host.client(Some(&device), pb::Intent::Control).await;
    connected.hello().await;
    connected.auth(pb::auth_result::Status::Ok).await;
    connected.frame().await;
}

#[tokio::test]
async fn dev_mode_without_certificate_receives_frames() {
    let options = RuntimeOptions {
        dev_allow_unpaired: true,
        ..Default::default()
    };
    let host = Host::start(options, Some(true)).await;
    let mut client = host.client(None, pb::Intent::Control).await;
    client.hello().await;
    client.auth(pb::auth_result::Status::Ok).await;
    client.frame().await;
}

#[tokio::test]
async fn dev_mode_requires_explicit_loopback_bind() {
    for bind in [Some("0.0.0.0:0".parse().unwrap()), None] {
        let options = RuntimeOptions {
            bind,
            dev_allow_unpaired: true,
            ..Default::default()
        };
        assert!(HostRuntime::new(FakePlatform::new(), options)
            .bind()
            .await
            .is_err());
    }
}

#[tokio::test]
async fn file_backed_host_identity_survives_runtime_restart() {
    let dir = common::state_dir();
    let options = RuntimeOptions {
        bind: Some("127.0.0.1:0".parse().unwrap()),
        store_dir: dir.clone(),
        ..Default::default()
    };
    let first = HostRuntime::new(
        FakePlatform::with_state_dir(dir.join("keys")),
        options.clone(),
    )
    .bind()
    .await
    .unwrap();
    let fp = first.security.identity.fp();
    let cert = first.security.identity.cert_der().to_vec();
    drop(first);
    let second = HostRuntime::new(FakePlatform::with_state_dir(dir.join("keys")), options)
        .bind()
        .await
        .unwrap();
    assert_eq!(second.security.identity.fp(), fp);
    assert_eq!(second.security.identity.cert_der(), cert);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        for entry in std::fs::read_dir(dir.join("keys")).unwrap() {
            assert_eq!(
                entry.unwrap().metadata().unwrap().permissions().mode() & 0o777,
                0o600
            );
        }
    }
    drop(second);
    std::fs::remove_dir_all(dir).unwrap();
}

#[tokio::test]
async fn tcp_listener_uses_the_same_identity_and_authorization() {
    let host = Host::start(RuntimeOptions::default(), Some(true)).await;
    let unknown = LocalIdentity::generate_software(CertKind::App).unwrap();
    for (device, intent, expected_status) in [
        (
            &unknown,
            pb::Intent::Control,
            Some(pb::auth_result::Status::Denied),
        ),
        (
            common::device(),
            pb::Intent::Control,
            Some(pb::auth_result::Status::Ok),
        ),
        (&unknown, pb::Intent::Probe, None),
    ] {
        let tls =
            client_config(device, ServerPin::Fingerprint(host.security.identity.fp())).unwrap();
        let conn = rsp_transport::connect_tcp(host.tcp_addr, Arc::new(tls))
            .await
            .unwrap();
        let stream = conn.open_control().await.unwrap();
        let hello = encode(
            &pb::ClientHello {
                protocol: features::PROTOCOL_VERSION,
                intent: intent as i32,
                ..Default::default()
            }
            .into(),
        );
        stream.send(&hello[4..]).await.unwrap();
        let mut decoder = EnvelopeDecoder::new();
        let mut messages = Vec::new();
        let expected_count = if expected_status == Some(pb::auth_result::Status::Denied) {
            1
        } else {
            2
        };
        for _ in 0..expected_count {
            let payload = tokio::time::timeout(Duration::from_secs(3), stream.recv())
                .await
                .unwrap()
                .unwrap()
                .unwrap();
            decoder.push(&(payload.len() as u32).to_be_bytes());
            decoder.push(&payload);
            messages.push(decoder.next().unwrap().unwrap().msg.unwrap());
        }
        match (messages.last().unwrap(), expected_status) {
            (pb::envelope::Msg::AuthResult(result), Some(status)) => {
                assert_eq!(result.status, status as i32);
                if status == pb::auth_result::Status::Denied {
                    assert_eq!(result.error.as_ref().unwrap().code, 181);
                }
            }
            (pb::envelope::Msg::Goodbye(goodbye), None) => {
                assert_eq!(goodbye.reason, pb::goodbye::Reason::UserClosed as i32)
            }
            other => panic!("unexpected TCP handshake {other:?}"),
        }
        conn.close(rsp_transport::CloseCode::Normal, b"test complete");
    }
}

struct TailnetWhoIs;
#[async_trait]
impl host_core::whois::WhoIsSource for TailnetWhoIs {
    async fn whois(
        &self,
        _: SocketAddr,
    ) -> Result<host_core::whois::WhoIs, host_core::whois::WhoIsError> {
        Ok(host_core::whois::WhoIs {
            login: "alice@example.com".into(),
            stable_id: "node-phone".into(),
            node_name: "phone".into(),
        })
    }
    async fn peer_path(
        &self,
        _: std::net::IpAddr,
    ) -> Result<host_core::whois::PeerPath, host_core::whois::WhoIsError> {
        Ok(host_core::whois::PeerPath {
            direct: false,
            relay: Some("test-region".into()),
            cur_addr: None,
        })
    }
}

#[tokio::test]
async fn discovered_tailnet_listener_populates_whois_and_derp_peer() {
    let platform = FakePlatform::with_nics(vec![host_core::platform::Nic {
        name: "test-tailnet".into(),
        addresses: vec!["127.0.0.1".parse().unwrap()],
        networks: vec![],
        mac: None,
        tailnet: true,
    }]);
    let mut options = RuntimeOptions {
        store_dir: common::state_dir(),
        ..Default::default()
    };
    options.listen_policy.port = 0;
    options.auth_policy.allowed_users = vec!["alice@example.com".into()];
    let mut bound = HostRuntime::new(platform.clone(), options)
        .with_store(common::paired_store())
        .bind()
        .await
        .unwrap();
    Arc::get_mut(&mut bound.security).unwrap().whois = Arc::new(TailnetWhoIs);
    let addresses = bound.listener.bound();
    let addr = addresses
        .iter()
        .find(|(_, _, kind)| *kind == rsp_transport::TransportKind::Quic)
        .unwrap()
        .0;
    let tcp_addr = addresses
        .iter()
        .find(|(_, _, kind)| *kind == rsp_transport::TransportKind::Tcp)
        .unwrap()
        .0;
    let security = bound.security.clone();
    let host = Host {
        addr,
        tcp_addr,
        security,
        platform,
        task: tokio::spawn(bound.run()),
    };
    let mut client = host
        .client(Some(common::device()), pb::Intent::Control)
        .await;
    let peer = client.hello().await.peer.unwrap();
    assert_eq!(peer.user, "alice@example.com");
    assert_eq!(peer.node, "phone");
    assert_eq!(peer.path, pb::PathKind::TailnetDerp as i32);
    assert_eq!(peer.derp_region, "test-region");
    client.auth(pb::auth_result::Status::Ok).await;
    client.frame().await;
}
