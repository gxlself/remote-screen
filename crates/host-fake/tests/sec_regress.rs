use async_trait::async_trait;
use host_core::{
    pairing::{PairApproval, PairApprovalRequest, PairApprover},
    platform::Nic,
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
    security: Arc<host_core::session::handshake::HandshakeContext>,
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
        let host = HostRuntime::new(FakePlatform::new(), options)
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
        Self {
            addr,
            security,
            task: tokio::spawn(host.run()),
        }
    }

    async fn client(&self, identity: Option<&LocalIdentity>, intent: pb::Intent) -> Client {
        self.client_from(identity, intent, "127.0.0.1:0").await
    }

    async fn client_from(
        &self,
        identity: Option<&LocalIdentity>,
        intent: pb::Intent,
        bind: &str,
    ) -> Client {
        let device = identity.unwrap_or(common::device());
        let expected_device_id =
            identity.map(|identity| rsp_core::ids::id_from_fp(&identity.fingerprint()));
        let tls =
            client_config(device, ServerPin::Fingerprint(self.security.identity.fp())).unwrap();
        Client::connect(self.addr, tls, intent, expected_device_id, bind).await
    }
}

impl Drop for Host {
    fn drop(&mut self) {
        self.task.abort();
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
        bind: &str,
    ) -> Self {
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
        let mut config = quinn::ClientConfig::new(Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        transport.keep_alive_interval(Some(Duration::from_millis(200)));
        config.transport_config(Arc::new(transport));
        let mut endpoint = quinn::Endpoint::client(bind.parse().unwrap()).unwrap();
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
                assert_eq!(state.your_role, auth.role);
                if auth.role == pb::Role::Controller as i32 {
                    let controller = state
                        .controller
                        .as_ref()
                        .expect("controller SessionState names the controller");
                    if let Some(expected) = &self.expected_device_id {
                        assert_eq!(controller.device_id, *expected);
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
}

impl Drop for Client {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"test done");
    }
}

struct TestWhoIs(&'static str);

#[async_trait]
impl host_core::whois::WhoIsSource for TestWhoIs {
    async fn whois(
        &self,
        _: SocketAddr,
    ) -> Result<host_core::whois::WhoIs, host_core::whois::WhoIsError> {
        Ok(host_core::whois::WhoIs {
            login: self.0.into(),
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
async fn sec04_disallowed_tailnet_user_denied_130_and_audited() {
    let mut options = RuntimeOptions {
        store_dir: common::state_dir(),
        ..Default::default()
    };
    options.listen_policy.port = 0;
    options.auth_policy.allowed_users = vec!["alice@example.com".into()];
    let audit = host_core::audit::AuditQueue::open(options.store_dir.join("audit")).unwrap();
    let platform = FakePlatform::with_nics(vec![Nic {
        name: "test-tailnet".into(),
        addresses: vec!["127.0.0.1".parse().unwrap()],
        networks: vec![],
        mac: None,
        tailnet: true,
    }]);
    let mut bound = HostRuntime::new(platform, options)
        .with_store(common::paired_store())
        .with_audit(audit)
        .with_connection_approver(Arc::new(Approval(Some(true))))
        .with_pairing_approver(Arc::new(Approval(Some(true))))
        .bind()
        .await
        .unwrap();
    Arc::get_mut(&mut bound.security).unwrap().whois = Arc::new(TestWhoIs("mallory@example.com"));
    let addr = bound
        .listener
        .bound()
        .into_iter()
        .find(|(_, _, transport)| *transport == rsp_transport::TransportKind::Quic)
        .unwrap()
        .0;
    let security = bound.security.clone();
    let _task = tokio::spawn(bound.run());

    let tls = client_config(
        common::device(),
        ServerPin::Fingerprint(security.identity.fp()),
    )
    .unwrap();
    let mut client = Client::connect(
        addr,
        tls,
        pb::Intent::Control,
        Some(rsp_core::ids::id_from_fp(&common::device().fingerprint())),
        "127.0.0.1:0",
    )
    .await;
    client.denied(130).await;

    let audit = security
        .audit
        .as_ref()
        .expect("audit queue")
        .batch(10)
        .unwrap();
    assert_eq!(audit.items.len(), 1);
    let item = &audit.items[0];
    assert_eq!(item.event, "auth.denied");
    assert_eq!(item.user.as_deref(), Some("mallory@example.com"));
    assert_eq!(item.path, Some(host_core::audit::AuditPath::TailnetDerp));
    assert_eq!(
        item.detail,
        Some(serde_json::json!({
            "code": 130,
            "reason": "tailnet user is not allowed",
        }))
    );
}

struct StartedPairing {
    client: Client,
    pairing: rsp_core::pairing::AppPairing,
    confirm: pb::PairConfirm,
}

async fn start_pairing(host: &Host, device: &LocalIdentity, code: &str) -> StartedPairing {
    let mut client = host.client(Some(device), pb::Intent::Pair).await;
    client.hello().await;
    let mut pairing = rsp_core::pairing::AppPairing::new(
        code,
        device.fingerprint(),
        host.security.identity.fp(),
        "new app",
        pb::Platform::Ios,
    )
    .unwrap();
    client.send(pairing.start()).await;
    let spake = match client.next().await {
        pb::envelope::Msg::PairSpake(spake) => spake,
        other => panic!("{other:?}"),
    };
    let (spake, confirm) = pairing.on_spake(spake).unwrap();
    client.send(spake).await;
    StartedPairing {
        client,
        pairing,
        confirm,
    }
}

#[tokio::test]
async fn sec12_replayed_pair_confirm_is_rejected() {
    let host = Host::start(RuntimeOptions::default(), Some(true)).await;

    let first_device = LocalIdentity::generate_software(CertKind::App).unwrap();
    let first_ticket = host
        .security
        .pairing
        .start(rsp_core::clock::unix_micros())
        .unwrap();
    let mut first = start_pairing(&host, &first_device, &first_ticket.code).await;
    let captured_confirm = first.confirm.clone();
    first.client.send(first.confirm).await;
    match first.client.next().await {
        pb::envelope::Msg::PairConfirm(confirm) => {
            first.pairing.on_confirm(confirm).unwrap();
        }
        other => panic!("{other:?}"),
    }
    for expected in [
        pb::pair_result::Status::WaitingLocal,
        pb::pair_result::Status::Ok,
    ] {
        match first.client.next().await {
            pb::envelope::Msg::PairResult(result) => {
                assert_eq!(result.status, expected as i32);
                first.pairing.on_result(result);
            }
            other => panic!("{other:?}"),
        }
    }
    assert!(host
        .security
        .store
        .get(&first_device.fingerprint())
        .unwrap()
        .is_some());

    let second_device = LocalIdentity::generate_software(CertKind::App).unwrap();
    let second_ticket = host
        .security
        .pairing
        .start(rsp_core::clock::unix_micros())
        .unwrap();
    let mut second = start_pairing(&host, &second_device, &second_ticket.code).await;
    second.client.send(captured_confirm).await;
    let result = match second.client.next().await {
        pb::envelope::Msg::PairResult(result) => result,
        other => panic!("{other:?}"),
    };
    assert_eq!(result.status, pb::pair_result::Status::BadCode as i32);
    assert_eq!(result.attempts_left, 4);
    assert!(host
        .security
        .store
        .get(&second_device.fingerprint())
        .unwrap()
        .is_none());
    let audit = host.security.audit.as_ref().unwrap().batch(10).unwrap();
    let denied = audit
        .items
        .iter()
        .find(|item| item.event == "pairing.denied")
        .expect("pairing denial audit");
    assert_eq!(
        denied.detail.as_ref().unwrap()["status"],
        pb::pair_result::Status::BadCode as i32
    );
    assert_eq!(
        denied.detail.as_ref().unwrap()["method"],
        serde_json::Value::String("code".into())
    );
}
