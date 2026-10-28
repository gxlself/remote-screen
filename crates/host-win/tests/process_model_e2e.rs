use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use host_core::platform::{ConsoleState, HostInfo, KeyStore, Nic, Result as PlatformResult};
use host_fake::harness::{app_identity, direct_params, open, wait_frames, FrameReport, TestHost};
use host_fake::FakePlatform;
use host_win::pipe_proto::{read_frame, write_frame, Ctl, PipeFrame, PIPE_PROTO_VERSION};
use host_win::proxy::{PipePlatform, ProxyTimeouts, ServiceLocal};
use host_win::session_server::SessionServer;
use host_win::supervisor::{SessionChangeKind, SupervisorConfig};
use host_win::svc_session_model::{supervise, ChildOps, DriverStats, SvcCommand};
use rsp_core::pb;
use rsp_core::trust::Permission;
use tokio::io::DuplexStream;
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, ReadBuf};
use tokio::sync::{mpsc, watch};
use tokio::time::{sleep, timeout};

type PipeStream = DuplexStream;

fn pipe_pair() -> io::Result<(PipeStream, PipeStream)> {
    Ok(tokio::io::duplex(64 * 1024))
}

// A lease remains live until both the listener and accepted service endpoint
// are dropped, modeling first_pipe_instance rather than merely listener life.
struct FakeConn {
    stream: PipeStream,
    _lease: Arc<()>,
}

impl AsyncRead for FakeConn {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_read(cx, buf)
    }
}

impl AsyncWrite for FakeConn {
    fn poll_write(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(&mut self.stream).poll_write(cx, buf)
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_flush(cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(&mut self.stream).poll_shutdown(cx)
    }
}

struct FakeListener {
    incoming: mpsc::UnboundedReceiver<io::Result<(FakeConn, u32)>>,
    _lease: Arc<()>,
}

struct FakeChild {
    pid: u32,
    exit: watch::Sender<Option<u32>>,
}

type Incoming = mpsc::UnboundedSender<io::Result<(FakeConn, u32)>>;

#[derive(Default)]
struct OpsState {
    console: Option<u32>,
    next_pid: u32,
    leases: HashMap<u32, Weak<()>>,
    incoming: HashMap<u32, Incoming>,
    spawned: Vec<(u32, u32)>,
    killed: Vec<u32>,
    exits: HashMap<u32, watch::Sender<Option<u32>>>,
    platforms: HashMap<u32, FakePlatform>,
    in_use: usize,
}

struct FakeOps {
    state: Mutex<OpsState>,
    stats: Arc<Mutex<DriverStats>>,
}

impl FakeOps {
    fn new(console: Option<u32>) -> Arc<Self> {
        Arc::new(Self {
            state: Mutex::new(OpsState {
                console,
                next_pid: 100,
                ..OpsState::default()
            }),
            stats: Arc::new(Mutex::new(DriverStats::default())),
        })
    }

    fn snapshot(&self) -> DriverStats {
        *self
            .stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn spawned(&self) -> Vec<(u32, u32)> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spawned
            .clone()
    }

    fn killed(&self) -> Vec<u32> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .killed
            .clone()
    }

    fn connected(&self) -> u64 {
        self.snapshot().connected
    }

    fn console(&self, id: Option<u32>) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .console = id;
    }

    fn crash(&self, pid: u32, code: u32) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if let Some(exit) = state.exits.get(&pid) {
            exit.send_replace(Some(code));
        }
    }

    fn platform(&self, pid: u32) -> FakePlatform {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .platforms
            .get(&pid)
            .cloned()
            .unwrap_or_else(|| panic!("missing FakePlatform for pid {pid}"))
    }

    fn no_live_pipes(&self) {
        let state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        assert_eq!(
            state.in_use, 0,
            "new listen ran before old pipe destruction"
        );
        assert!(state.leases.values().all(|lease| lease.upgrade().is_none()));
    }
}

impl ChildOps for FakeOps {
    type Conn = FakeConn;
    type Listener = FakeListener;
    type Child = FakeChild;

    fn console_session(&self) -> Option<u32> {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .console
    }

    fn listen(&self, id: u32) -> io::Result<FakeListener> {
        let mut state = self
            .state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if state.leases.get(&id).and_then(Weak::upgrade).is_some() {
            state.in_use += 1;
            return Err(io::ErrorKind::AddrInUse.into());
        }
        let lease = Arc::new(());
        let (tx, incoming) = mpsc::unbounded_channel();
        state.leases.insert(id, Arc::downgrade(&lease));
        state.incoming.insert(id, tx);
        Ok(FakeListener {
            incoming,
            _lease: lease,
        })
    }

    async fn accept(listener: &mut FakeListener) -> io::Result<(FakeConn, u32)> {
        listener
            .incoming
            .recv()
            .await
            .unwrap_or_else(|| Err(io::ErrorKind::BrokenPipe.into()))
    }

    fn spawn(&self, id: u32) -> Result<(u32, FakeChild), String> {
        let (pid, conn, exit, platform, lease) = {
            let mut state = self
                .state
                .lock()
                .unwrap_or_else(|poisoned| poisoned.into_inner());
            let pid = state.next_pid;
            state.next_pid = state.next_pid.saturating_add(1);
            let lease = state
                .leases
                .get(&id)
                .and_then(Weak::upgrade)
                .ok_or_else(|| "session pipe lease missing".to_owned())?;
            let tx = state
                .incoming
                .get(&id)
                .ok_or_else(|| "session pipe listener missing".to_owned())?;
            let (server, conn) = pipe_pair().map_err(|error| error.to_string())?;
            tx.send(Ok((
                FakeConn {
                    stream: server,
                    _lease: Arc::clone(&lease),
                },
                pid,
            )))
            .map_err(|_| "session pipe listener closed".to_owned())?;
            let (exit, _) = watch::channel(None);
            let platform = FakePlatform::new();
            state.spawned.push((id, pid));
            state.exits.insert(pid, exit.clone());
            state.platforms.insert(pid, platform.clone());
            (pid, conn, exit, platform, lease)
        };

        let stats = Arc::clone(&self.stats);
        tokio::spawn(session_process(
            conn,
            id,
            pid,
            exit.subscribe(),
            platform,
            stats,
        ));
        drop(lease);
        self.stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .spawned += 1;
        Ok((pid, FakeChild { pid, exit }))
    }

    async fn wait(child: Arc<FakeChild>) -> u32 {
        let mut exit = child.exit.subscribe();
        loop {
            if let Some(code) = *exit.borrow_and_update() {
                return code;
            }
            if exit.changed().await.is_err() {
                return 1;
            }
        }
    }

    fn kill(&self, child: &FakeChild) {
        self.state
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .killed
            .push(child.pid);
        child.exit.send_replace(Some(1));
        self.stats
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .killed += 1;
    }
}

async fn session_process(
    mut conn: PipeStream,
    session_id: u32,
    pid: u32,
    mut exit: watch::Receiver<Option<u32>>,
    platform: FakePlatform,
    stats: Arc<Mutex<DriverStats>>,
) {
    if exit.borrow().is_some()
        || write_frame(
            &mut conn,
            &PipeFrame::Ctl(Ctl::Hello {
                version: PIPE_PROTO_VERSION,
                session_id,
                pid,
            }),
        )
        .await
        .is_err()
        || conn.flush().await.is_err()
    {
        return;
    }
    let welcome = timeout(Duration::from_secs(5), read_frame(&mut conn)).await;
    if !matches!(
        welcome,
        Ok(Ok(Some(PipeFrame::Ctl(Ctl::Welcome {
            version: PIPE_PROTO_VERSION
        }))))
    ) {
        return;
    }
    stats
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .connected += 1;

    let (mut reader, mut writer) = tokio::io::split(conn);
    let (in_tx, in_rx) = mpsc::unbounded_channel();
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut server = SessionServer::new(Arc::new(platform), out_tx);
    server.start();

    let mut server_task = tokio::spawn(server.run(in_rx));
    let mut reader_task = tokio::spawn(async move {
        while let Ok(Some(frame)) = read_frame(&mut reader).await {
            if in_tx.send(frame).is_err() {
                break;
            }
        }
    });
    let mut writer_task = tokio::spawn(async move {
        loop {
            let frame = out_rx.recv().await;
            let Some(frame) = frame else {
                break;
            };
            if write_frame(&mut writer, &frame).await.is_err() || writer.flush().await.is_err() {
                break;
            }
        }
    });

    tokio::select! {
        _ = exit.changed() => {}
        _ = &mut server_task => {}
        _ = &mut reader_task => {}
        _ = &mut writer_task => {}
    }
    server_task.abort();
    reader_task.abort();
    writer_task.abort();
    let _ = server_task.await;
    let _ = reader_task.await;
    let _ = writer_task.await;
}

#[derive(Default)]
struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for MemoryKeyStore {
    fn load(&self, name: &str) -> PlatformResult<Option<Vec<u8>>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .get(name)
            .cloned())
    }

    fn store(&self, name: &str, value: &[u8]) -> PlatformResult<()> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .insert(name.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> PlatformResult<()> {
        self.0
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .remove(name);
        Ok(())
    }
}

#[derive(Default)]
struct LocalCalls {
    ctrl_alt_del: u64,
}

#[derive(Clone)]
struct E2eLocal {
    console: Arc<Mutex<ConsoleState>>,
    calls: Arc<Mutex<LocalCalls>>,
    keys: Arc<MemoryKeyStore>,
}

impl E2eLocal {
    fn new(console: ConsoleState) -> Self {
        Self {
            console: Arc::new(Mutex::new(console)),
            calls: Arc::new(Mutex::new(LocalCalls::default())),
            keys: Arc::new(MemoryKeyStore::default()),
        }
    }

    fn set_console_state(&self, state: ConsoleState) {
        *self
            .console
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = state;
    }

    fn ctrl_alt_del_count(&self) -> u64 {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ctrl_alt_del
    }
}

impl ServiceLocal for E2eLocal {
    fn info(&self) -> HostInfo {
        HostInfo {
            name: "e2e-windows-host".into(),
            os: pb::Platform::Windows,
            os_version: "11".into(),
            model: "fake".into(),
            arch: "x86_64".into(),
        }
    }

    fn console_state(&self) -> ConsoleState {
        *self
            .console
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn nics(&self) -> Vec<Nic> {
        vec![Nic {
            name: "ethernet".into(),
            addresses: vec!["192.168.31.7".parse().expect("test address")],
            networks: vec!["192.168.31.0/24".into()],
            mac: Some([2, 0, 0, 0, 0, 7]),
            tailnet: false,
        }]
    }

    fn send_wol(&self, _mac: [u8; 6], _via: &Nic) -> PlatformResult<()> {
        Ok(())
    }

    fn keystore(&self) -> &dyn KeyStore {
        self.keys.as_ref()
    }

    fn ctrl_alt_del(&self) -> PlatformResult<()> {
        self.calls
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .ctrl_alt_del += 1;
        Ok(())
    }
}

async fn wait_until(step: &str, ops: &FakeOps, mut ready: impl FnMut() -> bool) {
    if timeout(Duration::from_secs(10), async {
        loop {
            if ready() {
                return;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .is_err()
    {
        panic!(
            "{step}: condition timed out; driver stats {:?}",
            ops.snapshot()
        );
    }
}

async fn assert_steady(
    step: &str,
    ops: &FakeOps,
    duration: Duration,
    mut condition: impl FnMut() -> bool,
) {
    let result = timeout(duration, async {
        loop {
            if !condition() {
                return false;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    if !matches!(result, Err(_) | Ok(true)) {
        panic!(
            "{step}: condition changed; driver stats {:?}",
            ops.snapshot()
        );
    }
}

fn require(step: &str, ops: &FakeOps, condition: bool, detail: impl std::fmt::Display) {
    assert!(
        condition,
        "{step}: {detail}; driver stats {:?}",
        ops.snapshot()
    );
}

fn require_report(step: &str, ops: &FakeOps, report: &FrameReport, frames: u64, keyframes: u64) {
    require(
        step,
        ops,
        report.ended.is_none(),
        format_args!("session ended: {report:?}"),
    );
    require(
        step,
        ops,
        report.frames >= frames,
        format_args!("expected at least {frames} frames, got {report:?}"),
    );
    require(
        step,
        ops,
        report.keyframes >= keyframes,
        format_args!("expected at least {keyframes} keyframes, got {report:?}"),
    );
}

async fn wait_frames_with_keyframe(
    events: &mut rsp_client::session::EventStream,
    frames: u64,
    keyframes: u64,
    limit: Duration,
) -> FrameReport {
    let deadline = std::time::Instant::now() + limit;
    let mut report = FrameReport::default();
    while report.frames < frames || report.keyframes < keyframes {
        let remaining = deadline.saturating_duration_since(std::time::Instant::now());
        if remaining.is_zero() {
            break;
        }
        let next = wait_frames(events, 1, remaining).await;
        report.frames += next.frames;
        report.keyframes += next.keyframes;
        if report.first_frame.is_none() {
            report.first_frame = next.first_frame;
        }
        report.max_gap = report.max_gap.max(next.max_gap);
        if report.ended.is_none() {
            report.ended = next.ended;
        }
        report.last_stats = next.last_stats.or(report.last_stats);
        report.stream = next.stream.or(report.stream);
        report.states.extend(next.states);
        report.errors.extend(next.errors);
        if next.frames == 0 || report.ended.is_some() {
            break;
        }
    }
    report
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn e2e20_login_screen_to_user_switch() {
    let local = E2eLocal::new(ConsoleState::Locked);
    let platform = Arc::new(PipePlatform::with_timeouts(
        local.clone(),
        ProxyTimeouts {
            request: Duration::from_secs(5),
            open: Duration::from_secs(5),
        },
    ));
    let host = TestHost::start_with_platform((*platform).clone(), Default::default()).await;
    let ops = FakeOps::new(Some(1));
    let (commands, command_rx) = mpsc::unbounded_channel();
    let service_ops = Arc::clone(&ops);
    let service_platform = Arc::clone(&platform);
    let supervisor = tokio::task::spawn_blocking(move || {
        let runtime = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
            .expect("build service runtime");
        runtime.block_on(supervise(
            service_ops,
            service_platform,
            SupervisorConfig {
                backoff_initial: Duration::from_millis(100),
                backoff_max: Duration::from_secs(1),
                connect_timeout: Duration::from_secs(5),
                stable_after: Duration::from_secs(60),
            },
            command_rx,
        ))
    });

    wait_until("login screen: child A spawned", &ops, || {
        !ops.spawned().is_empty()
    })
    .await;
    wait_until("login screen: child A connected", &ops, || {
        ops.connected() >= 1
    })
    .await;
    wait_until("login screen: pipe attached", &ops, || {
        platform.is_attached()
    })
    .await;

    let app = app_identity();
    host.pair(&app, Permission::Control);
    let (session, mut events) = open(direct_params(&host, app))
        .await
        .expect("login screen: open app session");
    let first = wait_frames_with_keyframe(&mut events, 3, 1, Duration::from_secs(10)).await;
    require_report("login screen", &ops, &first, 3, 1);
    let pid_a = ops.spawned()[0].1;
    let actions_before_ctrl_alt_del = ops.platform(pid_a).session_actions();

    session.action(pb::session_action::Kind::CtrlAltDel, true);
    wait_until("Ctrl+Alt+Del: service local call", &ops, || {
        local.ctrl_alt_del_count() >= 1
    })
    .await;
    require(
        "Ctrl+Alt+Del",
        &ops,
        ops.platform(pid_a).session_actions() == actions_before_ctrl_alt_del,
        format_args!(
            "child A received a new session action: before={actions_before_ctrl_alt_del:?}, after={:?}",
            ops.platform(pid_a).session_actions()
        ),
    );
    let ctrl_report = wait_frames(&mut events, 1, Duration::from_secs(5)).await;
    require_report("Ctrl+Alt+Del", &ops, &ctrl_report, 1, 0);

    local.set_console_state(ConsoleState::LoggedIn);
    commands
        .send(SvcCommand::SessionChange {
            kind: SessionChangeKind::Logon,
            session_id: 1,
        })
        .expect("send Logon");
    wait_until("login: child A killed", &ops, || {
        ops.killed().contains(&pid_a)
    })
    .await;
    wait_until("login: child B spawned", &ops, || ops.spawned().len() >= 2).await;
    wait_until("login: child B connected", &ops, || ops.connected() >= 2).await;
    wait_until("login: child B pipe attached", &ops, || {
        platform.is_attached()
    })
    .await;
    let logged_in = wait_frames_with_keyframe(&mut events, 1, 1, Duration::from_secs(10)).await;
    require_report("login", &ops, &logged_in, 1, 1);

    let pid_b = ops.spawned()[1].1;
    ops.crash(pid_b, 17);
    wait_until("crash restart: child C spawned", &ops, || {
        ops.spawned().len() >= 3
    })
    .await;
    wait_until("crash restart: child C connected", &ops, || {
        ops.connected() >= 3
    })
    .await;
    wait_until("crash restart: child C pipe attached", &ops, || {
        platform.is_attached()
    })
    .await;
    let restarted = wait_frames_with_keyframe(&mut events, 1, 1, Duration::from_secs(10)).await;
    require_report("crash restart", &ops, &restarted, 1, 1);

    let pid_c = ops.spawned()[2].1;
    let spawned_before_lock = ops.spawned().len();
    local.set_console_state(ConsoleState::Locked);
    commands
        .send(SvcCommand::SessionChange {
            kind: SessionChangeKind::Lock,
            session_id: 1,
        })
        .expect("send Lock");
    assert_steady(
        "lock screen: process remains running",
        &ops,
        Duration::from_millis(500),
        || ops.spawned().len() == spawned_before_lock,
    )
    .await;
    let locked = wait_frames(&mut events, 1, Duration::from_secs(5)).await;
    require_report("lock screen", &ops, &locked, 1, 0);

    ops.console(Some(2));
    commands
        .send(SvcCommand::SessionChange {
            kind: SessionChangeKind::ConsoleConnect,
            session_id: 2,
        })
        .expect("send ConsoleConnect");
    wait_until("user switch: child C killed", &ops, || {
        ops.killed().contains(&pid_c)
    })
    .await;
    wait_until("user switch: child D spawned", &ops, || {
        ops.spawned().len() >= 4
    })
    .await;
    wait_until("user switch: child D connected", &ops, || {
        ops.connected() >= 4
    })
    .await;
    wait_until("user switch: child D pipe attached", &ops, || {
        platform.is_attached()
    })
    .await;
    let switched = wait_frames_with_keyframe(&mut events, 1, 1, Duration::from_secs(10)).await;
    require_report("user switch", &ops, &switched, 1, 1);

    commands.send(SvcCommand::Shutdown).expect("send Shutdown");
    let stats = timeout(Duration::from_secs(10), supervisor)
        .await
        .expect("shutdown: supervise timeout")
        .expect("shutdown: supervise task failed");
    ops.no_live_pipes();
    require(
        "shutdown",
        &ops,
        stats.spawned == 4,
        format_args!("expected four children, got {stats:?}"),
    );
    require(
        "shutdown",
        &ops,
        stats.killed >= 2,
        format_args!("expected at least two kills, got {stats:?}"),
    );
    require(
        "shutdown",
        &ops,
        ops.killed().len() >= 2,
        format_args!("kill log too short: {:?}", ops.killed()),
    );

    session.close();
    host.stop().await;
}
