//! Requires nextest's one-process-per-case isolation: Go node and ABI loader are
//! process-global. Missing test library/helper is SKIP unless RS_REQUIRE_TAILNET=1.
#![cfg(unix)]

mod sshserver;
mod support;

use std::{
    io,
    net::SocketAddr,
    path::Path,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_diag::set_setting,
    api_sessions as sessions,
    api_ssh::{connect_once, CoreHostKeyStore, CorePrompter, PasswordCache, PromptRelay},
    dto_core::{OpenIntent, OpenOpts, SessionPhase, SshAuth, Transport},
    runtime::Core,
    sshbook::ResolvedTarget,
    store::PairedHostRow,
    tables::{SessionEntry, SessionTable, VideoConsumer},
    tailnet_ctl::{ControllerConfig, EmbeddedNode, NodeFuture, NodeSnapshot},
    tailnet_wire::{install_with, real_node, WireOptions},
};
use rs_ssh::{HostKeyEntry, HostKeyStore, SshTarget};
use rs_tailnet::testkit::{TestNet, TestNetOptions, TestPaths};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::{
    hid::{InputMsg, Mods},
    pb,
    trust::Permission,
    DEFAULT_PORT,
};
use rsp_transport::{net::NetFuture, system_net, NetDialer, NetStream};
use serde_json::Value;
use ssh_key::HashAlg;
use sshserver::{ServerCfg, SshTestServer};
use tokio::time::{sleep, timeout};
use zeroize::Zeroizing;

use support::{test_core_with, with_identity, EventCollector};

const LIMIT: Duration = Duration::from_secs(15);

#[derive(Clone, Debug, PartialEq, Eq)]
enum DialCall {
    Udp(SocketAddr),
    Tcp(SocketAddr),
    Host(String, u16),
}

type DialCalls = Arc<Mutex<Vec<DialCall>>>;

// Both observers delegate to real networks and record only successful dials.
// Observing EmbeddedNode::dialer keeps the production controller and split
// dialer responsible for choosing each connection's path.
#[derive(Debug)]
struct RecordingNet {
    inner: Arc<dyn NetDialer>,
    calls: DialCalls,
}

impl NetDialer for RecordingNet {
    fn shared_quic_endpoint(&self, bind: SocketAddr) -> io::Result<Option<quinn::Endpoint>> {
        self.inner.shared_quic_endpoint(bind)
    }

    fn dial_udp(&self, peer: SocketAddr) -> NetFuture<'_, Arc<dyn quinn::AsyncUdpSocket>> {
        Box::pin(async move {
            let socket = self.inner.dial_udp(peer).await?;
            self.calls.lock().unwrap().push(DialCall::Udp(peer));
            Ok(socket)
        })
    }

    fn dial_tcp(&self, peer: SocketAddr) -> NetFuture<'_, NetStream> {
        Box::pin(async move {
            let stream = self.inner.dial_tcp(peer).await?;
            self.calls.lock().unwrap().push(DialCall::Tcp(peer));
            Ok(stream)
        })
    }

    fn dial_tcp_host<'a>(&'a self, host: &'a str, port: u16) -> NetFuture<'a, NetStream> {
        Box::pin(async move {
            let stream = self.inner.dial_tcp_host(host, port).await?;
            self.calls
                .lock()
                .unwrap()
                .push(DialCall::Host(host.into(), port));
            Ok(stream)
        })
    }
}

struct RecordingNode {
    inner: Arc<dyn EmbeddedNode>,
    calls: DialCalls,
}

impl EmbeddedNode for RecordingNode {
    fn available(&self) -> bool {
        self.inner.available()
    }
    fn start<'a>(&'a self, control_url: &'a str, hostname: &'a str) -> NodeFuture<'a, ()> {
        self.inner.start(control_url, hostname)
    }
    fn stop(&self) -> NodeFuture<'_, ()> {
        self.inner.stop()
    }
    fn rebind(&self) -> NodeFuture<'_, ()> {
        self.inner.rebind()
    }
    fn snapshot(&self) -> NodeFuture<'_, NodeSnapshot> {
        self.inner.snapshot()
    }
    fn dialer(&self) -> Option<Arc<dyn NetDialer>> {
        self.inner.dialer().map(|inner| {
            Arc::new(RecordingNet {
                inner,
                calls: self.calls.clone(),
            }) as Arc<dyn NetDialer>
        })
    }
}

struct RunningCore {
    core: Arc<Core>,
    events: Arc<EventCollector>,
    testnet: TestNet,
    system: DialCalls,
    embedded: DialCalls,
}

impl Drop for RunningCore {
    fn drop(&mut self) {
        self.core.shutdown();
    }
}

async fn wait_event(
    events: &EventCollector,
    after: u64,
    seconds: u64,
    predicate: impl Fn(&Value) -> bool,
) -> Value {
    timeout(Duration::from_secs(seconds), async {
        loop {
            if let Some((_, event)) = events
                .events()
                .into_iter()
                .find(|(seq, event)| *seq > after && predicate(event))
            {
                return event;
            }
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await
    .expect("expected event before deadline")
}

fn running(event: &Value) -> bool {
    event["type"] == "network.changed"
        && event["status"]["tailnet"]["active"] == "embedded"
        && event["status"]["tailnet"]["embedded"]["state"] == "running"
}

async fn running_core(paths: &TestPaths, mut options: TestNetOptions) -> RunningCore {
    options.require_auth = true;
    let testnet = TestNet::spawn(paths, &options).expect("start local testnet");
    let system = DialCalls::default();
    let embedded = DialCalls::default();
    let core = test_core_with(|_, opts| {
        opts.init_logging = true;
        opts.net = Some(Arc::new(RecordingNet {
            inner: system_net(),
            calls: system.clone(),
        }));
    });
    with_identity(&core, "phone");
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    install_with(
        &core,
        Arc::new(RecordingNode {
            inner: real_node(&core, true),
            calls: embedded.clone(),
        }),
        WireOptions {
            disable_netns: true,
            controller: ControllerConfig {
                background_grace: Duration::from_secs(2),
                ..ControllerConfig::default()
            },
            ..WireOptions::default()
        },
    )
    .unwrap();
    set_setting(&core, "tailnet.mode", "\"embedded\"").unwrap();
    set_setting(
        &core,
        "tailnet.controlUrl",
        &serde_json::to_string(&testnet.control_url).unwrap(),
    )
    .unwrap();
    let login = wait_event(&events, 0, 15, |event| {
        event["type"] == "tailnet.state"
            && event["state"] == "needsLogin"
            && event["authUrl"]
                .as_str()
                .is_some_and(|url| url.contains("/auth/"))
    })
    .await;
    testnet
        .complete(login["authUrl"].as_str().unwrap())
        .expect("complete interactive login");
    wait_event(&events, 0, 30, running).await;
    RunningCore {
        core,
        events,
        testnet,
        system,
        embedded,
    }
}

fn assert_no_key(bytes: &[u8], secret: &[u8]) {
    // Interactive login normally has an empty authKey; also reject the prefix.
    assert!(
        !bytes
            .windows(b"tskey-".len())
            .any(|window| window == b"tskey-"),
        "auth key prefix leaked"
    );
    if !secret.is_empty() {
        assert!(
            !bytes.windows(secret.len()).any(|window| window == secret),
            "auth key leaked"
        );
    }
}

fn check_logs(path: &Path, secret: &[u8]) -> usize {
    let mut files = 0;
    for entry in std::fs::read_dir(path).unwrap() {
        let path = entry.unwrap().path();
        if path.is_dir() {
            files += check_logs(&path, secret);
        } else {
            let bytes = std::fs::read(path).unwrap();
            assert_no_key(&bytes, secret);
            files += 1;
        }
    }
    files
}

async fn finish(fixture: &RunningCore) {
    for entry in fixture.core.state::<SessionTable>().list() {
        sessions::close(&fixture.core, entry.id);
    }
    until(LIMIT, || fixture.core.state::<SessionTable>().is_empty()).await;
    assert!(fixture.core.events().flush(Duration::from_secs(2)));
    let node = rs_tailnet::Tailnet::global(rs_tailnet::load_default().unwrap());
    fixture.core.shutdown();
    assert_eq!(
        node.status().unwrap().state,
        rs_tailnet::BackendState::Stopped
    );
    assert_no_key(
        &serde_json::to_vec(&fixture.events.events()).unwrap(),
        fixture.testnet.auth_key.as_bytes(),
    );
    assert!(check_logs(&fixture.core.log_dir(), fixture.testnet.auth_key.as_bytes()) > 0);
    for event in fixture.events.of_type("network.changed") {
        assert!(event["status"]["tailnet"]["embedded"]
            .get("authUrl")
            .is_none());
    }
}

fn pair_row(core: &Arc<Core>, host_id: &str, fp: [u8; 32], address: SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host_id.into(),
            fp,
            name: "Paired test host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

fn opts(intent: OpenIntent) -> OpenOpts {
    OpenOpts {
        intent,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

async fn until(limit: Duration, predicate: impl Fn() -> bool) {
    timeout(limit, async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

#[derive(Default)]
struct VideoRecorder {
    frames: Mutex<Vec<(Instant, bool)>>,
    rtts: Mutex<Vec<u64>>,
}

impl VideoRecorder {
    fn ready(&self) -> bool {
        let frames = self.frames.lock().unwrap();
        frames.len() >= 10 && frames.iter().any(|(_, keyframe)| *keyframe)
    }

    fn assert_continuous(&self, start: Instant, end: Instant, minimum: Duration) {
        let frames = self.frames.lock().unwrap();
        let times: Vec<_> = frames
            .iter()
            .map(|(at, _)| *at)
            .filter(|at| *at >= start && *at <= end)
            .collect();
        // Include both boundaries, so a stalled stream cannot pass using old frames.
        let mut previous = start;
        let mut max_gap = Duration::ZERO;
        for &at in &times {
            max_gap = max_gap.max(at.duration_since(previous));
            previous = at;
        }
        max_gap = max_gap.max(end.duration_since(previous));
        let span = times
            .first()
            .zip(times.last())
            .map_or(Duration::ZERO, |(first, last)| last.duration_since(*first));
        eprintln!(
            "video window: frames={} span={span:?} max_gap={max_gap:?}",
            times.len()
        );
        assert!(span >= minimum, "video span {span:?} < {minimum:?}");
        assert!(
            max_gap < Duration::from_millis(500),
            "video max_gap={max_gap:?}"
        );
    }
}

impl VideoConsumer for VideoRecorder {
    fn frame(&self, frame: &ReceivedFrame) {
        self.frames
            .lock()
            .unwrap()
            .push((Instant::now(), frame.frame.keyframe));
    }
    fn stream_config(&self, _: &pb::StreamConfig) {}
    fn cursor(&self, _: &CursorEvent) {}
    fn cursor_msg(&self, _: &pb::CursorMsg) {}
    fn clock(&self, rtt_us: u64, _: i64) {
        self.rtts.lock().unwrap().push(rtt_us);
    }
    fn closed(&self, _: &str) {}
}

struct SessionFixture {
    running: RunningCore,
    host_a: TestHost,
    host_b: TestHost,
    platform_a: FakePlatform,
    a_addr: SocketAddr,
}

async fn session_fixture(paths: &TestPaths) -> SessionFixture {
    let platform_a = FakePlatform::new();
    let host_a = TestHost::start_with_platform(
        platform_a.clone(),
        TestHostOptions {
            tcp: true,
            ..Default::default()
        },
    )
    .await;
    let host_b = TestHost::start_with_platform(
        FakePlatform::new(),
        TestHostOptions {
            tcp: true,
            ..Default::default()
        },
    )
    .await;
    let running = running_core(
        paths,
        TestNetOptions {
            forward_udp: vec![(DEFAULT_PORT, host_a.quic_addr)],
            forward_tcp: vec![(DEFAULT_PORT, host_a.tcp_addr.unwrap())],
            ..Default::default()
        },
    )
    .await;
    let identity = running.core.local_identity().unwrap();
    host_a.pair(&identity, Permission::Control);
    host_b.pair(&identity, Permission::Control);
    let a_addr = SocketAddr::new(running.testnet.peer_ip4().into(), DEFAULT_PORT);
    pair_row(&running.core, &host_a.host_id, host_a.fp, a_addr);
    pair_row(&running.core, &host_b.host_id, host_b.fp, host_b.quic_addr);
    SessionFixture {
        running,
        host_a,
        host_b,
        platform_a,
        a_addr,
    }
}

async fn open_streaming(
    fixture: &RunningCore,
    host_id: &str,
    intent: OpenIntent,
) -> (Arc<SessionEntry>, Arc<VideoRecorder>) {
    let id = sessions::open(&fixture.core, host_id, opts(intent)).unwrap();
    let entry = fixture.core.state::<SessionTable>().get(id).unwrap();
    let recorder = Arc::new(VideoRecorder::default());
    assert!(entry.video.attach(recorder.clone()));
    let streaming = timeout(LIMIT, async {
        loop {
            match entry.summary().phase {
                SessionPhase::Streaming => break,
                SessionPhase::Ended => panic!(
                    "session ended: {:?}",
                    fixture.events.of_type("session.error")
                ),
                _ => sleep(Duration::from_millis(5)).await,
            }
        }
    })
    .await;
    assert!(
        streaming.is_ok(),
        "session did not stream within 15 s: {:?}",
        fixture.events.of_type("session.error")
    );
    until(LIMIT, || {
        recorder.ready() && entry.summary().transport == Some(Transport::Quic)
    })
    .await;
    assert_eq!(entry.summary().transport, Some(Transport::Quic));
    (entry, recorder)
}

fn assert_embedded_session(fixture: &SessionFixture) {
    let embedded = fixture.running.embedded.lock().unwrap().clone();
    let system = fixture.running.system.lock().unwrap().clone();
    assert!(
        embedded.contains(&DialCall::Udp(fixture.a_addr)),
        "Host A must dial UDP through embedded tsnet: embedded={embedded:?}, system={system:?}"
    );
    assert!(!system.contains(&DialCall::Udp(fixture.a_addr)));
    assert!(!system.contains(&DialCall::Udp(fixture.host_a.quic_addr)));
}

async fn close_session(core: &Arc<Core>, id: u64) {
    sessions::close(core, id);
    until(LIMIT, || core.state::<SessionTable>().get(id).is_none()).await;
}

fn median(values: &[u64]) -> u64 {
    let mut values = values.to_vec();
    values.sort_unstable();
    let middle = values.len() / 2;
    if values.len().is_multiple_of(2) {
        ((u128::from(values[middle - 1]) + u128::from(values[middle])) / 2) as u64
    } else {
        values[middle]
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn session_over_embedded_tailnet() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("session_over_embedded_tailnet") else {
        return;
    };
    let fixture = session_fixture(&paths).await;
    let (entry, _) = open_streaming(
        &fixture.running,
        &fixture.host_a.host_id,
        OpenIntent::Control,
    )
    .await;
    assert_embedded_session(&fixture);
    sessions::key(&fixture.running.core, entry.id, 0x04, true, 0).unwrap();
    sessions::key(&fixture.running.core, entry.id, 0x04, false, 0).unwrap();
    sessions::combo(&fixture.running.core, entry.id, "cmdTab").unwrap();
    let text = "\u{4f60}\u{597d}";
    sessions::text(&fixture.running.core, entry.id, text).unwrap();
    until(LIMIT, || fixture.platform_a.injected_msgs().len() >= 7).await;
    assert_eq!(
        fixture.platform_a.injected_msgs(),
        [
            InputMsg::Key {
                usage: 0x04,
                down: true,
                mods: Mods::NONE
            },
            InputMsg::Key {
                usage: 0x04,
                down: false,
                mods: Mods::NONE
            },
            InputMsg::Key {
                usage: 0xe3,
                down: true,
                mods: Mods::LMETA
            },
            InputMsg::Key {
                usage: 0x2b,
                down: true,
                mods: Mods::LMETA
            },
            InputMsg::Key {
                usage: 0x2b,
                down: false,
                mods: Mods::LMETA
            },
            InputMsg::Key {
                usage: 0xe3,
                down: false,
                mods: Mods::NONE
            },
            InputMsg::Text(text.into()),
        ]
    );
    finish(&fixture.running).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn ssh_over_embedded_tailnet() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("ssh_over_embedded_tailnet") else {
        return;
    };
    let home = support::temp_dir("tailnet-ssh");
    let server = SshTestServer::start(ServerCfg {
        password: Some("tailnet-test-password".into()),
        exec_with_sh: Some(home.clone()),
        ..Default::default()
    })
    .await;
    let fixture = running_core(
        &paths,
        TestNetOptions {
            forward_tcp: vec![(22, server.addr)],
            ..Default::default()
        },
    )
    .await;
    let target = ResolvedTarget {
        server_id: "embedded-ssh".into(),
        auth: SshAuth::Password,
        tailscale_ssh: None,
        target: SshTarget {
            host: fixture.testnet.peer_hostname.clone(),
            port: 22,
            user: "test".into(),
            jump: None,
        },
    };
    let public = server.cfg.host_key.public_key();
    CoreHostKeyStore {
        core: fixture.core.clone(),
        server_id: target.server_id.clone(),
    }
    .save(
        &target.target.host,
        22,
        HostKeyEntry {
            algorithm: public.algorithm().to_string(),
            key: public.to_bytes().unwrap(),
            fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
        },
    );
    fixture.core.state::<PasswordCache>().put(
        &target.server_id,
        vec![Zeroizing::new(server.cfg.password.clone().unwrap())],
        fixture.core.now_us(),
    );
    let prompter = Arc::new(CorePrompter::new(
        fixture.core.clone(),
        None,
        target.server_id.clone(),
        target.target.user.clone(),
        None,
        Arc::new(PromptRelay::default()),
    ));
    let session = timeout(LIMIT, connect_once(&fixture.core, &target, prompter))
        .await
        .expect("SSH connection deadline")
        .expect("authenticate SSH through tsnet");
    let command = "printf embedded-ssh";
    let output = timeout(LIMIT, session.exec(command))
        .await
        .unwrap()
        .unwrap();
    assert_eq!(output.stdout, b"embedded-ssh");
    assert!(output.stderr.is_empty());
    assert_eq!(output.status, Some(0));
    let call = DialCall::Host(fixture.testnet.peer_hostname.clone(), 22);
    assert!(fixture.embedded.lock().unwrap().contains(&call));
    assert!(!fixture.system.lock().unwrap().contains(&call));
    assert!(server.observed().attempts.contains(&"password"));
    assert_eq!(server.observed().commands, [command]);
    timeout(LIMIT, session.close()).await.unwrap();
    finish(&fixture).await;
    server.stop();
    std::fs::remove_dir_all(home).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_tailnet_and_direct() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("concurrent_tailnet_and_direct") else {
        return;
    };
    let fixture = session_fixture(&paths).await;
    let (a, a_video) =
        open_streaming(&fixture.running, &fixture.host_a.host_id, OpenIntent::View).await;
    let (b, b_video) =
        open_streaming(&fixture.running, &fixture.host_b.host_id, OpenIntent::View).await;
    assert_embedded_session(&fixture);
    let direct_call = DialCall::Udp(fixture.host_b.quic_addr);
    assert!(fixture
        .running
        .system
        .lock()
        .unwrap()
        .contains(&direct_call));
    assert!(!fixture
        .running
        .embedded
        .lock()
        .unwrap()
        .contains(&direct_call));
    let start = Instant::now();
    sleep(Duration::from_millis(3_200)).await;
    let end = Instant::now();
    a_video.assert_continuous(start, end, Duration::from_secs(3));
    b_video.assert_continuous(start, end, Duration::from_secs(3));
    assert_eq!(a.summary().phase, SessionPhase::Streaming);
    assert_eq!(b.summary().phase, SessionPhase::Streaming);
    let close_start = Instant::now();
    close_session(&fixture.running.core, a.id).await;
    sleep(Duration::from_millis(1_200)).await;
    b_video.assert_continuous(close_start, Instant::now(), Duration::from_secs(1));
    assert_eq!(b.summary().phase, SessionPhase::Streaming);
    let a_dials = fixture
        .running
        .embedded
        .lock()
        .unwrap()
        .iter()
        .filter(|call| **call == DialCall::Udp(fixture.a_addr))
        .count();
    let (_, reopened) =
        open_streaming(&fixture.running, &fixture.host_a.host_id, OpenIntent::View).await;
    assert!(reopened.ready());
    let new_dials = fixture
        .running
        .embedded
        .lock()
        .unwrap()
        .iter()
        .filter(|call| **call == DialCall::Udp(fixture.a_addr))
        .count();
    assert!(
        new_dials > a_dials,
        "reopening A must dial embedded tsnet again"
    );
    b_video.assert_continuous(close_start, Instant::now(), Duration::from_secs(1));
    assert_eq!(b.summary().phase, SessionPhase::Streaming);
    finish(&fixture.running).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn rtt_delta_within_3ms() {
    let Some(paths) = rs_tailnet::testkit::paths_or_skip("rtt_delta_within_3ms") else {
        return;
    };
    let fixture = session_fixture(&paths).await;
    let (_, a_video) =
        open_streaming(&fixture.running, &fixture.host_a.host_id, OpenIntent::View).await;
    let (_, b_video) =
        open_streaming(&fixture.running, &fixture.host_b.host_id, OpenIntent::View).await;
    assert_embedded_session(&fixture);
    assert!(fixture
        .running
        .system
        .lock()
        .unwrap()
        .contains(&DialCall::Udp(fixture.host_b.quic_addr)));
    // Native clock callbacks deliver one microsecond-resolution sample per Pong;
    // polling session.stats would repeat samples or truncate them to milliseconds.
    a_video.rtts.lock().unwrap().clear();
    b_video.rtts.lock().unwrap().clear();
    let sampled = timeout(Duration::from_secs(30), async {
        while a_video.rtts.lock().unwrap().len() < 20 || b_video.rtts.lock().unwrap().len() < 20 {
            sleep(Duration::from_millis(20)).await;
        }
    })
    .await;
    let a_samples = a_video.rtts.lock().unwrap().clone();
    let b_samples = b_video.rtts.lock().unwrap().clone();
    assert!(
        sampled.is_ok(),
        "need 20 RTT samples per session: A={a_samples:?}, B={b_samples:?}"
    );
    let a_samples = &a_samples[..20];
    let b_samples = &b_samples[..20];
    let a_median = median(a_samples);
    let b_median = median(b_samples);
    let delta = a_median.abs_diff(b_median);
    eprintln!(
        "RTT us: A={a_samples:?}, B={b_samples:?}, medians=({a_median}, {b_median}), delta={delta}"
    );
    assert!(
        delta <= 3_000,
        "RTT delta {delta} us exceeds 3000 us: A={a_samples:?}, B={b_samples:?}"
    );
    finish(&fixture.running).await;
}
