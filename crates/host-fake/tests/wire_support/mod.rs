#![allow(dead_code)]
//! Shared loopback wire fixture, independent of the legacy common fixture.

use async_trait::async_trait;
use host_core::audit::HostAuditRecord;
use host_core::{
    audit::AuditQueue,
    clock::Clock,
    config::{ConfigStore, HostConfig},
    ipc,
    platform::Nic,
    session::{
        approval::PromptEvents,
        handshake::ConnectionApprover,
        manager::{ConnInfo, HostServices, SessionManager},
    },
    store::{DevicePlatform, PairedDevice, PairingPath, PathPermissions, Store},
    HostRuntime, RuntimeOptions,
};
use host_fake::FakePlatform;
use rsp_core::{
    control::{encode, EnvelopeDecoder},
    features, pb,
    trust::Permission,
};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    tls::{client_config, ServerPin},
};
use std::{
    collections::HashMap,
    net::SocketAddr,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, OnceLock,
    },
    time::Duration,
};
use tokio::{task::JoinHandle, time::timeout};

#[derive(Clone, Copy, Debug)]
pub enum Dev {
    A,
    B,
    C,
    D,
    E,
    Unpaired,
}
pub fn identity(d: Dev) -> &'static Arc<LocalIdentity> {
    static IDS: OnceLock<[Arc<LocalIdentity>; 6]> = OnceLock::new();
    &IDS.get_or_init(|| {
        std::array::from_fn(|_| Arc::new(LocalIdentity::generate_software(CertKind::App).unwrap()))
    })[d as usize]
}
pub fn device_id(d: Dev) -> String {
    rsp_core::ids::id_from_fp(&identity(d).fingerprint())
}
#[derive(Default)]
pub struct OffsetClock {
    fixed_epoch_us: Option<u64>,
    offset: AtomicU64,
}
impl OffsetClock {
    /// Start at the current Unix time and advance only when `skip` is called.
    pub fn manual() -> Self {
        Self {
            fixed_epoch_us: Some(rsp_core::clock::unix_micros()),
            ..Self::default()
        }
    }

    pub fn skip(&self, d: Duration) {
        self.offset
            .fetch_add(u64::try_from(d.as_micros()).unwrap(), Ordering::Relaxed);
    }
}
impl Clock for OffsetClock {
    fn now_micros(&self) -> u64 {
        self.fixed_epoch_us
            .unwrap_or_else(rsp_core::clock::unix_micros)
            .saturating_add(self.offset.load(Ordering::Relaxed))
    }
}
pub struct RecordingEvents {
    events: Mutex<Vec<ipc::Event>>,
    listening: AtomicBool,
}
impl Default for RecordingEvents {
    fn default() -> Self {
        Self {
            events: Mutex::new(Vec::new()),
            listening: AtomicBool::new(true),
        }
    }
}
impl RecordingEvents {
    pub fn events(&self) -> Vec<ipc::Event> {
        self.events.lock().unwrap().clone()
    }
    pub fn take(&self) -> Vec<ipc::Event> {
        std::mem::take(&mut *self.events.lock().unwrap())
    }
    pub fn set_listening(&self, on: bool) {
        self.listening.store(on, Ordering::Relaxed);
    }
}
impl PromptEvents for RecordingEvents {
    fn emit(&self, event: ipc::Event) -> bool {
        self.events.lock().unwrap().push(event);
        self.listening.load(Ordering::Relaxed)
    }
}

pub struct WireOptions {
    pub options: RuntimeOptions,
    pub platform: Option<FakePlatform>,
    pub clock: Option<Arc<OffsetClock>>,
    pub config: HostConfig,
    pub connection_approver: Option<Arc<dyn ConnectionApprover>>,
    pub listeners: usize,
    pub tailnet_users: Option<Vec<String>>,
    pub state_dir: Option<PathBuf>,
    pub audit: bool,
}
impl Default for WireOptions {
    fn default() -> Self {
        Self {
            options: RuntimeOptions::default(),
            platform: None,
            clock: None,
            config: HostConfig::default(),
            connection_approver: None,
            listeners: 1,
            tailnet_users: None,
            state_dir: None,
            audit: false,
        }
    }
}
#[derive(Default)]
struct FakeWhoIs {
    logins: Mutex<HashMap<u16, String>>,
}
#[async_trait]
impl host_core::whois::WhoIsSource for FakeWhoIs {
    async fn whois(
        &self,
        peer: SocketAddr,
    ) -> Result<host_core::whois::WhoIs, host_core::whois::WhoIsError> {
        let login = self
            .logins
            .lock()
            .unwrap()
            .get(&peer.port())
            .cloned()
            .unwrap_or_default();
        Ok(host_core::whois::WhoIs {
            login,
            stable_id: format!("node-{}", peer.port()),
            node_name: "wire-device".into(),
        })
    }
    async fn peer_path(
        &self,
        _: std::net::IpAddr,
    ) -> Result<host_core::whois::PeerPath, host_core::whois::WhoIsError> {
        Ok(host_core::whois::PeerPath {
            direct: true,
            relay: None,
            cur_addr: None,
        })
    }
}
pub struct WireHost {
    pub mgr: Arc<SessionManager<FakePlatform>>,
    pub platform: FakePlatform,
    pub events: Arc<RecordingEvents>,
    pub clock: Arc<OffsetClock>,
    pub config: ConfigStore,
    pub store: Arc<Store>,
    pub downloads: PathBuf,
    pub state_dir: PathBuf,
    pub audit: Option<AuditQueue>,
    pub addrs: Vec<SocketAddr>,
    pub tcp_addrs: Vec<Option<SocketAddr>>,
    pub host_fp: [u8; 32],
    tasks: Vec<JoinHandle<host_core::RuntimeResult<()>>>,
    whois: Arc<FakeWhoIs>,
    temporary: bool,
}
impl WireHost {
    pub async fn start() -> Self {
        Self::start_with(WireOptions::default()).await
    }
    pub async fn start_with(mut o: WireOptions) -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(1);
        let temporary = o.state_dir.is_none();
        let state_dir = o.state_dir.take().unwrap_or_else(|| {
            std::env::temp_dir().join(format!(
                "rs-wire-{}-{}-{}",
                std::process::id(),
                rsp_core::clock::unix_micros(),
                NEXT.fetch_add(1, Ordering::Relaxed)
            ))
        });
        let downloads = state_dir.join("downloads");
        std::fs::create_dir_all(&downloads).unwrap();
        let store = Arc::new(Store::open(state_dir.join("store")).unwrap());
        let clock = o.clock.take().unwrap_or_default();
        // Seeded devices predate Host startup, even when the clock is frozen.
        for d in [Dev::A, Dev::B, Dev::C, Dev::D, Dev::E] {
            pair(
                &store,
                d,
                Permission::Control,
                clock.now_micros().saturating_sub(1),
            );
        }
        let platform = o.platform.take().unwrap_or_else(|| {
            if o.tailnet_users.is_some() {
                FakePlatform::with_nics(vec![Nic {
                    name: "wire-tailnet".into(),
                    addresses: vec!["127.0.0.1".parse().unwrap()],
                    networks: vec![],
                    mac: None,
                    tailnet: true,
                }])
            } else {
                FakePlatform::new()
            }
        });
        let events = Arc::new(RecordingEvents::default());
        let config = ConfigStore::new(state_dir.join("config.toml"), o.config);
        let audit = o
            .audit
            .then(|| AuditQueue::open(state_dir.join("audit")).expect("open audit queue"));
        let mgr = SessionManager::new(
            Arc::new(platform.clone()),
            HostServices {
                events: events.clone(),
                config: config.clone(),
                audit: audit.clone(),
                downloads_dir: downloads.clone(),
                state_dir: Some(state_dir.clone()),
                clock: clock.clone(),
            },
        );
        let whois = Arc::new(FakeWhoIs::default());
        let mut tasks = Vec::new();
        let mut addrs = Vec::new();
        let mut tcp_addrs = Vec::new();
        let mut host_fp = [0; 32];
        assert!(o.listeners > 0);
        for _ in 0..o.listeners {
            let mut options = o.options.clone();
            options.store_dir = state_dir.join("store");
            if let Some(users) = &o.tailnet_users {
                options.bind = None;
                options.listen_policy.port = 0;
                options.listen_policy.tailnet = true;
                options.auth_policy.allowed_users = users.clone();
            } else {
                options.bind = Some("127.0.0.1:0".parse().unwrap());
            }
            let mut runtime = HostRuntime::new(platform.clone(), options)
                .with_store(store.clone())
                .with_manager(mgr.clone());
            if let Some(approver) = &o.connection_approver {
                runtime = runtime.with_connection_approver(approver.clone());
            }
            let mut bound = runtime.bind().await.unwrap();
            Arc::get_mut(&mut bound.security).unwrap().whois = whois.clone();
            host_fp = bound.security.identity.fp();
            addrs.push(
                bound
                    .listener
                    .bound()
                    .into_iter()
                    .find(|(_, _, kind)| *kind == rsp_transport::TransportKind::Quic)
                    .unwrap()
                    .0,
            );
            tcp_addrs.push(
                bound
                    .listener
                    .bound()
                    .into_iter()
                    .find(|(_, _, kind)| *kind == rsp_transport::TransportKind::Tcp)
                    .map(|(addr, _, _)| addr),
            );
            tasks.push(tokio::spawn(bound.run()));
        }
        Self {
            mgr,
            platform,
            events,
            clock,
            config,
            store,
            downloads,
            state_dir,
            audit,
            addrs,
            tcp_addrs,
            host_fp,
            tasks,
            whois,
            temporary,
        }
    }
    pub fn pair(&self, d: Dev, permission: Permission) {
        pair(&self.store, d, permission, self.clock.now_micros());
    }
    pub async fn connect(&self) -> WireClient {
        self.connect_as(Dev::A, 0).await
    }
    pub async fn connect_as(&self, d: Dev, listener: usize) -> WireClient {
        self.connect_inner(d, listener, None).await
    }
    pub async fn connect_user(&self, d: Dev, login: &str) -> WireClient {
        self.connect_inner(d, 0, Some(login)).await
    }
    async fn connect_inner(&self, d: Dev, listener: usize, login: Option<&str>) -> WireClient {
        let tls = client_config(identity(d), ServerPin::Fingerprint(self.host_fp)).unwrap();
        let crypto = quinn::crypto::rustls::QuicClientConfig::try_from(tls).unwrap();
        let mut endpoint = quinn::Endpoint::client("127.0.0.1:0".parse().unwrap()).unwrap();
        endpoint.set_default_client_config(quinn::ClientConfig::new(Arc::new(crypto)));
        if let Some(login) = login {
            self.whois
                .logins
                .lock()
                .unwrap()
                .insert(endpoint.local_addr().unwrap().port(), login.into());
        }
        let conn = timeout(
            Duration::from_secs(3),
            endpoint
                .connect(self.addrs[listener], rsp_core::SERVER_NAME)
                .unwrap(),
        )
        .await
        .unwrap()
        .unwrap();
        // Opening a bi stream sends nothing until hello(), so it is also a prewarm client.
        let (send, recv) = conn.open_bi().await.unwrap();
        WireClient {
            conn,
            send,
            recv,
            decoder: EnvelopeDecoder::new(),
            endpoint,
        }
    }
    pub async fn connect_tcp(&self, d: Dev, listener: usize) -> WireTcpClient {
        let tls = client_config(identity(d), ServerPin::Fingerprint(self.host_fp)).unwrap();
        let conn = rsp_transport::connect_tcp(
            self.tcp_addrs[listener].expect("TCP listener is available"),
            tls.into(),
        )
        .await
        .unwrap();
        let control = conn.open_control().await.unwrap();
        WireTcpClient { conn, control }
    }
    /// Send AttachInput as the first record of a separate TLS connection.
    /// A rejected token must close this socket without closing its parent.
    pub async fn reject_tcp_attach(&self, d: Dev, listener: usize, token: Vec<u8>) {
        let client = self.connect_tcp(d, listener).await;
        client
            .send(pb::AttachInput {
                session_token: token,
            })
            .await;
        timeout(Duration::from_secs(3), client.conn.closed())
            .await
            .expect("input attachment rejected");
    }
    pub fn conns(&self) -> Vec<ConnInfo> {
        self.mgr.conns()
    }
    /// Abort one listener task; its session tasks are cancelled without on_closed.
    pub fn abort_listener(&self, listener: usize) {
        self.tasks[listener].abort();
    }
    pub fn audit_events(&self) -> Vec<HostAuditRecord> {
        self.audit
            .as_ref()
            .map(|audit| audit.batch(10_000).expect("read audit queue").items)
            .unwrap_or_default()
    }
    pub async fn wait_until(&self, mut f: impl FnMut() -> bool) {
        timeout(Duration::from_secs(3), async {
            while !f() {
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .expect("wire condition timed out");
    }
}
impl Drop for WireHost {
    fn drop(&mut self) {
        for task in &self.tasks {
            task.abort();
        }
        if self.temporary {
            let _ = std::fs::remove_dir_all(&self.state_dir);
        }
    }
}
fn pair(store: &Store, d: Dev, permission: Permission, paired_at_us: u64) {
    let device = identity(d);
    store
        .upsert(PairedDevice {
            fp: device.fingerprint(),
            device_id: device_id(d),
            name: format!("wire-{d:?}"),
            cert_der: device.cert_der.as_ref().to_vec(),
            permission,
            paired_at_us,
            require_confirm: false,
            platform: DevicePlatform::Ios,
            paired_via: PairingPath::Lan,
            whois_user: None,
            whois_node: None,
            whois_node_name: None,
            path_permissions: PathPermissions::default(),
        })
        .unwrap();
}

pub struct WireClient {
    pub conn: quinn::Connection,
    send: quinn::SendStream,
    recv: quinn::RecvStream,
    decoder: EnvelopeDecoder,
    endpoint: quinn::Endpoint,
}
impl WireClient {
    pub async fn hello(&mut self) -> (pb::ServerHello, pb::AuthResult) {
        self.hello_with(client_hello()).await
    }
    pub async fn hello_with(&mut self, h: pb::ClientHello) -> (pb::ServerHello, pb::AuthResult) {
        self.send(h).await;
        let mut server = pb::ServerHello::default();
        loop {
            match self.next().await {
                pb::envelope::Msg::ServerHello(h) => server = h,
                pb::envelope::Msg::AuthResult(a) => {
                    if a.status == pb::auth_result::Status::Ok as i32 {
                        self.expect_session_state(&a, &server).await;
                    }
                    return (server, a);
                }
                other => panic!("unexpected handshake message: {other:?}"),
            }
        }
    }
    pub async fn expect_session_state(
        &mut self,
        auth: &pb::AuthResult,
        server: &pb::ServerHello,
    ) -> pb::SessionState {
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
    pub async fn send(&mut self, m: impl Into<pb::Envelope>) {
        self.send.write_all(&encode(&m.into())).await.unwrap();
    }
    pub async fn next(&mut self) -> pb::envelope::Msg {
        timeout(Duration::from_secs(3), async {
            loop {
                if let Some(env) = self.decoder.next().unwrap() {
                    if let Some(msg) = env.msg {
                        return msg;
                    }
                }
                let mut bytes = [0; 4096];
                let n = self
                    .recv
                    .read(&mut bytes)
                    .await
                    .unwrap()
                    .expect("control EOF");
                self.decoder.push(&bytes[..n]);
            }
        })
        .await
        .expect("control message timed out")
    }
    pub async fn next_matching<T>(
        &mut self,
        mut f: impl FnMut(&pb::envelope::Msg) -> Option<T>,
    ) -> T {
        timeout(Duration::from_secs(3), async {
            loop {
                if let Some(value) = f(&self.next().await) {
                    return value;
                }
            }
        })
        .await
        .expect("matching message timed out")
    }
    pub async fn next_skipping_session_state<T>(
        &mut self,
        expected: &str,
        mut f: impl FnMut(&pb::envelope::Msg) -> Option<T>,
    ) -> T {
        timeout(Duration::from_secs(3), async {
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
    pub async fn expect_none_matching(
        &mut self,
        d: Duration,
        mut f: impl FnMut(&pb::envelope::Msg) -> bool,
    ) {
        assert!(timeout(d, async {
            loop {
                let msg = self.next().await;
                assert!(!f(&msg), "unexpected message: {msg:?}");
            }
        })
        .await
        .is_err());
    }
    pub async fn start_stream(&mut self, s: pb::StartStream) -> pb::StreamConfig {
        self.send(s).await;
        self.next_skipping_session_state("StreamConfig", |m| match m {
            pb::envelope::Msg::StreamConfig(c) => Some(*c),
            _ => None,
        })
        .await
    }
    pub async fn open_typed(&self, kind: u8) -> quinn::SendStream {
        let mut s = self.conn.open_uni().await.unwrap();
        s.write_all(&[kind]).await.unwrap();
        s
    }
    pub async fn open_legacy_input(&self) -> quinn::SendStream {
        self.conn.open_uni().await.unwrap()
    }
    pub async fn accept_typed(&self) -> (u8, quinn::RecvStream) {
        timeout(Duration::from_secs(3), async {
            let mut r = self.conn.accept_uni().await.unwrap();
            let mut kind = [0];
            r.read_exact(&mut kind).await.unwrap();
            (kind[0], r)
        })
        .await
        .unwrap()
    }
    pub fn send_datagram(&self, bytes: &[u8]) {
        self.conn.send_datagram(bytes.to_vec().into()).unwrap();
    }
    pub async fn recv_datagram(&self, ty: u8) -> Vec<u8> {
        timeout(Duration::from_secs(3), async {
            loop {
                let bytes = self.conn.read_datagram().await.unwrap();
                if bytes.first() == Some(&ty) {
                    return bytes.to_vec();
                }
            }
        })
        .await
        .expect("datagram timed out")
    }
    pub async fn goodbye(&mut self, reason: pb::goodbye::Reason) {
        self.send(pb::Goodbye {
            reason: reason as i32,
            message: String::new(),
        })
        .await;
    }
    pub async fn close_code(&self) -> u64 {
        match timeout(Duration::from_secs(3), self.conn.closed())
            .await
            .unwrap()
        {
            quinn::ConnectionError::ApplicationClosed(c) => c.error_code.into_inner(),
            other => panic!("expected application close: {other:?}"),
        }
    }
}
impl Drop for WireClient {
    fn drop(&mut self) {
        self.endpoint.close(0u32.into(), b"wire client dropped");
    }
}
pub fn client_hello() -> pb::ClientHello {
    pb::ClientHello {
        protocol: features::PROTOCOL_VERSION,
        intent: pb::Intent::Control as i32,
        platform: pb::Platform::Ios as i32,
        device_name: "wire-device".into(),
        features: (1 << 19) - 1,
        ..Default::default()
    }
}
pub async fn write_record(s: &mut quinn::SendStream, payload: &[u8]) {
    s.write_all(&(payload.len() as u32).to_be_bytes())
        .await
        .unwrap();
    s.write_all(payload).await.unwrap();
}
pub async fn read_record(r: &mut quinn::RecvStream) -> Option<Vec<u8>> {
    let mut len = [0; 4];
    r.read_exact(&mut len).await.ok()?;
    let mut bytes = vec![0; u32::from_be_bytes(len) as usize];
    r.read_exact(&mut bytes).await.ok()?;
    Some(bytes)
}

/// TCP wire client: uses records and never the QUIC compatibility APIs.
pub struct WireTcpClient {
    pub conn: rsp_transport::Conn,
    pub control: rsp_transport::ControlStream,
}
impl WireTcpClient {
    pub async fn send(&self, m: impl Into<pb::Envelope>) {
        let frame = encode(&m.into());
        self.control.send(&frame[4..]).await.unwrap();
    }
    pub async fn next(&self) -> pb::envelope::Msg {
        timeout(Duration::from_secs(3), async {
            loop {
                let payload = self.control.recv().await.unwrap().expect("TCP control EOF");
                let mut decoder = EnvelopeDecoder::new();
                decoder.push(&(payload.len() as u32).to_be_bytes());
                decoder.push(&payload);
                if let Some(message) = decoder.next().unwrap().and_then(|e| e.msg) {
                    return message;
                }
            }
        })
        .await
        .expect("TCP control message timed out")
    }
    pub async fn next_matching<T>(&self, mut f: impl FnMut(&pb::envelope::Msg) -> Option<T>) -> T {
        timeout(Duration::from_secs(3), async {
            loop {
                if let Some(value) = f(&self.next().await) {
                    return value;
                }
            }
        })
        .await
        .expect("TCP matching message timed out")
    }
    pub async fn hello(&self) -> (pb::ServerHello, pb::AuthResult) {
        self.hello_with(client_hello()).await
    }
    pub async fn hello_with(&self, h: pb::ClientHello) -> (pb::ServerHello, pb::AuthResult) {
        self.send(h).await;
        let server = self
            .next_matching(|m| match m {
                pb::envelope::Msg::ServerHello(h) => Some(h.clone()),
                _ => None,
            })
            .await;
        let auth = self
            .next_matching(|m| match m {
                pb::envelope::Msg::AuthResult(a) => Some(a.clone()),
                _ => None,
            })
            .await;
        if auth.status == pb::auth_result::Status::Ok as i32 {
            self.conn
                .set_session_token(auth.session_token.clone())
                .unwrap();
        }
        (server, auth)
    }
    pub async fn start_stream(&self, start: pb::StartStream) -> pb::StreamConfig {
        self.send(start).await;
        self.next_matching(|m| match m {
            pb::envelope::Msg::StreamConfig(c) => Some(*c),
            pb::envelope::Msg::Error(e) => panic!("TCP stream failed: {e:?}"),
            _ => None,
        })
        .await
    }
    pub async fn ping(&self) {
        self.send(pb::Ping { t1_us: 42 }).await;
        let pong = self
            .next_matching(|m| match m {
                pb::envelope::Msg::Pong(p) => Some(*p),
                _ => None,
            })
            .await;
        assert_eq!(pong.t1_us, 42);
    }
    pub async fn video(&self) -> Vec<u8> {
        timeout(Duration::from_secs(3), async {
            loop {
                let (kind, payload) = self.conn.recv_datagram().await.unwrap();
                if kind == rsp_transport::DatagramKind::Video {
                    // Neutral transport strips the channel byte from the payload.
                    let mut packet = vec![kind as u8];
                    packet.extend_from_slice(&payload);
                    return packet;
                }
            }
        })
        .await
        .expect("TCP video timed out")
    }
}
impl Drop for WireTcpClient {
    fn drop(&mut self) {
        self.conn
            .close(rsp_transport::CloseCode::Normal, b"wire TCP client dropped");
    }
}
