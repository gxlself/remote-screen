mod sshserver;
mod support;

use std::{future::Future, io, net::SocketAddr, sync::Arc, sync::Mutex, time::Duration};

use host_core::platform::InputEvent;
use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_diag, api_sessions,
    api_ssh::{connect_once, CoreHostKeyStore, CorePrompter, PromptRelay},
    dto_core::{OpenIntent, OpenOpts, SessionPhase, SshAuth},
    runtime::Core,
    sshbook::ResolvedTarget,
    store::PairedHostRow,
    tables::SessionTable,
};
use rs_ssh::{HostKeyEntry, HostKeyStore, SshTarget};
use rsp_core::{hid::InputMsg, trust::Permission};
use rsp_transport::{net::NetFuture, NetDialer, NetStream, SystemNetDialer};
use support::{test_core_with, with_identity, EventCollector};
use tokio::time::{sleep, timeout};

#[derive(Debug, Clone, PartialEq, Eq)]
enum Call {
    SharedQuicEndpoint(SocketAddr),
    DialUdp(SocketAddr),
    DialTcp(SocketAddr),
    DialTcpHost(String, u16),
}

#[derive(Debug, Default)]
struct RecordingNet {
    system: SystemNetDialer,
    calls: Mutex<Vec<Call>>,
}

impl RecordingNet {
    fn record(&self, call: Call) {
        self.calls.lock().unwrap().push(call);
    }

    fn calls(&self) -> Vec<Call> {
        self.calls.lock().unwrap().clone()
    }
}

impl NetDialer for RecordingNet {
    fn shared_quic_endpoint(&self, bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        self.record(Call::SharedQuicEndpoint(bind));
        self.system.shared_quic_endpoint(bind)
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        self.record(Call::DialUdp(peer));
        self.system.dial_udp(peer)
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        self.record(Call::DialTcp(peer));
        self.system.dial_tcp(peer)
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        self.record(Call::DialTcpHost(host.to_owned(), port));
        self.system.dial_tcp_host(host, port)
    }
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<EventCollector>,
}

impl Fixture {
    fn new(net: Option<Arc<dyn NetDialer>>) -> Self {
        let core = test_core_with(|_, opts| opts.net = net);
        with_identity(&core, "Network selection test");
        let events = EventCollector::new();
        core.events().subscribe(events.clone());
        Self { core, events }
    }

    fn pair(&self, host: &TestHost) {
        host.pair(&self.core.local_identity().unwrap(), Permission::Control);
        self.core
            .store()
            .upsert_paired_host(&PairedHostRow {
                host_id: host.host_id.clone(),
                fp: host.fp,
                name: "Test host".into(),
                addresses: vec![host.quic_addr],
                paired_at_us: self.core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            api_sessions::close(&self.core, entry.id);
        }
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    timeout(Duration::from_secs(5), future)
        .await
        .expect("local operation timed out")
}

async fn until(predicate: impl Fn() -> bool) {
    bounded(async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

#[test]
fn default_network_is_system() {
    let f = Fixture::new(None);
    assert!(format!("{:?}", f.core.net()).contains("SystemNetDialer"));
}

#[test]
fn options_inject_the_same_network_object() {
    let net = Arc::new(RecordingNet::default());
    let f = Fixture::new(Some(net.clone()));
    assert_eq!(
        Arc::as_ptr(&f.core.net()) as *const (),
        Arc::as_ptr(&net) as *const ()
    );
}

#[test]
fn set_net_replaces_the_selected_object() {
    let original = Arc::new(RecordingNet::default());
    let replacement = Arc::new(RecordingNet::default());
    let f = Fixture::new(Some(original.clone()));
    let snapshot = f.core.net();
    f.core.set_net(replacement.clone());
    assert_eq!(
        Arc::as_ptr(&f.core.net()) as *const (),
        Arc::as_ptr(&replacement) as *const ()
    );
    assert_eq!(
        Arc::as_ptr(&snapshot) as *const (),
        Arc::as_ptr(&original) as *const ()
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_uses_current_net_and_keeps_established_connection() {
    let net = Arc::new(RecordingNet::default());
    let f = Fixture::new(Some(net.clone()));
    let platform = FakePlatform::new();
    let host = TestHost::start_with_platform(
        platform.clone(),
        TestHostOptions {
            tcp: false,
            ..Default::default()
        },
    )
    .await;
    f.pair(&host);
    let opts = || OpenOpts {
        intent: OpenIntent::Control,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    };
    let id = api_sessions::open(&f.core, &host.host_id, opts()).unwrap();
    let entry = f.core.state::<SessionTable>().get(id).unwrap();
    until(|| entry.summary().phase == SessionPhase::Streaming && entry.handle().is_some()).await;
    assert_eq!(
        net.calls(),
        vec![Call::SharedQuicEndpoint(([127, 0, 0, 1], 0).into())]
    );

    let replacement = Arc::new(RecordingNet::default());
    f.core.set_net(replacement.clone());
    entry
        .handle()
        .unwrap()
        .send_input(InputMsg::MouseMove { x: 0.25, y: 0.75 })
        .unwrap();
    until(|| {
        platform
            .injected_events()
            .contains(&InputEvent::MouseMove { x: 0.25, y: 0.75 })
    })
    .await;
    assert!(replacement.calls().is_empty());
    api_sessions::close(&f.core, id);
    until(|| f.core.state::<SessionTable>().is_empty()).await;

    let id = api_sessions::open(&f.core, &host.host_id, opts()).unwrap();
    let entry = f.core.state::<SessionTable>().get(id).unwrap();
    until(|| entry.summary().phase == SessionPhase::Streaming).await;
    assert_eq!(
        replacement.calls(),
        vec![Call::SharedQuicEndpoint(([127, 0, 0, 1], 0).into())]
    );
    api_sessions::close(&f.core, id);
    until(|| f.core.state::<SessionTable>().is_empty()).await;
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn ssh_uses_current_net_and_keeps_established_connection() {
    let net = Arc::new(RecordingNet::default());
    let f = Fixture::new(Some(net.clone()));
    let server = sshserver::SshTestServer::start(sshserver::ServerCfg {
        allow_none: true,
        ..Default::default()
    })
    .await;
    let public = server.cfg.host_key.public_key();
    CoreHostKeyStore {
        core: f.core.clone(),
        server_id: "server".into(),
    }
    .save(
        "127.0.0.1",
        server.addr.port(),
        HostKeyEntry {
            algorithm: public.algorithm().to_string(),
            key: public.to_bytes().unwrap(),
            fingerprint: public.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
        },
    );
    let prompter = Arc::new(CorePrompter::new(
        f.core.clone(),
        None,
        "server".into(),
        "test".into(),
        None,
        Arc::new(PromptRelay::default()),
    ));
    let target = ResolvedTarget {
        server_id: "server".into(),
        auth: SshAuth::Tailscale,
        tailscale_ssh: None,
        target: SshTarget {
            host: "127.0.0.1".into(),
            port: server.addr.port(),
            user: "test".into(),
            jump: None,
        },
    };
    let session = bounded(connect_once(&f.core, &target, prompter.clone()))
        .await
        .unwrap();
    assert_eq!(
        net.calls(),
        vec![Call::DialTcpHost("127.0.0.1".into(), server.addr.port())]
    );
    let replacement = Arc::new(RecordingNet::default());
    f.core.set_net(replacement.clone());
    assert_eq!(
        bounded(session.exec("still-connected"))
            .await
            .unwrap()
            .status,
        Some(0)
    );
    assert!(replacement.calls().is_empty());
    session.close().await;
    let session = bounded(connect_once(&f.core, &target, prompter))
        .await
        .unwrap();
    assert_eq!(
        replacement.calls(),
        vec![Call::DialTcpHost("127.0.0.1".into(), server.addr.port())]
    );
    session.close().await;
    server.stop();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn probe_uses_selected_net() {
    let net = Arc::new(RecordingNet::default());
    let f = Fixture::new(Some(net.clone()));
    let host = TestHost::start().await;
    f.pair(&host);
    let id = bounded(api_diag::probe(&f.core, &host.host_id))
        .await
        .unwrap()
        .to_string();
    until(|| {
        f.events
            .of_type("probe.result")
            .iter()
            .any(|event| event["probeId"] == id && event["last"] == true)
    })
    .await;
    let results = f.events.of_type("probe.result");
    let result = results.iter().find(|event| event["probeId"] == id).unwrap();
    assert_eq!(result["outcome"], "ok");
    assert_eq!(result["transport"], "quic");
    // Diagnostics deliberately retains its wildcard bind even for loopback peers.
    assert_eq!(
        net.calls(),
        vec![Call::SharedQuicEndpoint(([0, 0, 0, 0], 0).into())]
    );
    host.stop().await;
}
