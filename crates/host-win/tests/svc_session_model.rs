use std::collections::HashMap;
use std::io;
use std::pin::Pin;
use std::sync::{Arc, Mutex, Weak};
use std::task::{Context, Poll};
use std::time::Duration;

use host_core::platform::{
    ConsoleState, HostInfo, KeyStore, Nic, Platform, Result as PlatformResult,
};
use host_fake::FakePlatform;
use host_win::pipe_proto::{read_frame, write_frame, Ctl, PipeFrame};
use host_win::proxy::{PipePlatform, ServiceLocal};
use host_win::supervisor::{SessionChangeKind, SupervisorConfig, NO_SESSION};
use host_win::svc_session_model::{
    os_version_string, sibling_exe, supervise, verify_hello, AttachTarget, ChildOps, DriverStats,
    HelloReject, SvcCommand, CONSOLE_POLL, HELLO_TIMEOUT, KILL_WAIT, SESSION_EXE, SHUTDOWN_WAIT,
};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use tokio::time::{advance, Instant};

#[test]
fn sibling_paths() {
    assert_eq!(
        sibling_exe(r"C:\Program Files\Remote Host\rs-host.exe", SESSION_EXE),
        r"C:\Program Files\Remote Host\rs-host-session.exe"
    );
    assert_eq!(
        sibling_exe("C:/Remote Host/rs-host.exe", SESSION_EXE),
        "C:/Remote Host/rs-host-session.exe"
    );
    assert_eq!(sibling_exe("rs-host.exe", SESSION_EXE), SESSION_EXE);
}

fn hello(version: u32, session_id: u32, pid: u32) -> Ctl {
    Ctl::Hello {
        version,
        session_id,
        pid,
    }
}

#[test]
fn hello_validation_order() {
    assert_eq!(verify_hello(&hello(1, 1, 100), 1, 100), Ok(()));
    assert_eq!(
        verify_hello(&Ctl::Ping { n: 0 }, 1, 100),
        Err(HelloReject::NotHello)
    );
    assert_eq!(
        verify_hello(&hello(2, 2, 200), 1, 100),
        Err(HelloReject::Version { got: 2 })
    );
    assert_eq!(
        verify_hello(&hello(1, 2, 200), 1, 100),
        Err(HelloReject::Session {
            expected: 1,
            got: 2
        })
    );
    assert_eq!(
        verify_hello(&hello(1, 1, 200), 1, 100),
        Err(HelloReject::Pid {
            expected: 100,
            got: 200
        })
    );
}

#[test]
fn windows_versions() {
    assert_eq!(
        os_version_string(Some(10), Some(0), Some("22631"), Some(4317), Some("23H2")),
        "Windows 11 23H2 (10.0.22631.4317)"
    );
    assert_eq!(
        os_version_string(None, None, Some("19045"), None, None),
        "Windows 10 (10.0.19045)"
    );
    assert_eq!(os_version_string(None, None, None, None, None), "Windows");
    assert_eq!(
        os_version_string(Some(10), Some(0), Some("invalid"), None, Some("23H2")),
        "Windows"
    );
    assert_eq!(
        os_version_string(None, None, Some("22000"), Some(0), Some("")),
        "Windows 11 (10.0.22000.0)"
    );
}

// A lease remains live until BOTH the listener and accepted service endpoint
// are dropped, modeling first_pipe_instance rather than merely listener life.
struct FakeConn {
    stream: DuplexStream,
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

struct Session {
    id: u32,
    pid: u32,
    conn: DuplexStream,
    exit: watch::Sender<Option<u32>>,
    spoof: Option<DuplexStream>,
}

impl Session {
    async fn handshake(&mut self) {
        self.send(hello(1, self.id, self.pid)).await;
        assert_eq!(
            read_frame(&mut self.conn).await.unwrap(),
            Some(PipeFrame::Ctl(Ctl::Welcome { version: 1 }))
        );
        settle().await;
    }

    async fn send(&mut self, ctl: Ctl) {
        write_frame(&mut self.conn, &PipeFrame::Ctl(ctl))
            .await
            .unwrap();
    }

    fn exit(&self, code: u32) {
        self.exit.send_replace(Some(code));
    }
}

type Incoming = mpsc::UnboundedSender<io::Result<(FakeConn, u32)>>;

struct OpsState {
    console: Option<u32>,
    next_pid: u32,
    listen_fail: usize,
    spawn_fail: usize,
    accept_fail: bool,
    wrong_client: bool,
    ignore_kill: bool,
    leases: HashMap<u32, Weak<()>>,
    incoming: HashMap<u32, Incoming>,
    attempts: Vec<(u32, Instant)>,
    spawned: Vec<(u32, u32)>,
    killed: Vec<u32>,
    in_use: usize,
}

struct FakeOps {
    state: Mutex<OpsState>,
    sessions: mpsc::UnboundedSender<Session>,
}

impl FakeOps {
    fn new(console: Option<u32>) -> (Arc<Self>, mpsc::UnboundedReceiver<Session>) {
        let (sessions, rx) = mpsc::unbounded_channel();
        (
            Arc::new(Self {
                state: Mutex::new(OpsState {
                    console,
                    next_pid: 100,
                    listen_fail: 0,
                    spawn_fail: 0,
                    accept_fail: false,
                    wrong_client: false,
                    ignore_kill: false,
                    leases: HashMap::new(),
                    incoming: HashMap::new(),
                    attempts: vec![],
                    spawned: vec![],
                    killed: vec![],
                    in_use: 0,
                }),
                sessions,
            }),
            rx,
        )
    }

    fn spawned(&self) -> Vec<(u32, u32)> {
        self.state.lock().unwrap().spawned.clone()
    }

    fn killed(&self) -> Vec<u32> {
        self.state.lock().unwrap().killed.clone()
    }

    fn console(&self, id: Option<u32>) {
        self.state.lock().unwrap().console = id;
    }

    fn no_live_pipes(&self) {
        let state = self.state.lock().unwrap();
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
        self.state.lock().unwrap().console
    }

    fn listen(&self, id: u32) -> io::Result<FakeListener> {
        let mut state = self.state.lock().unwrap();
        state.attempts.push((id, Instant::now()));
        if state.listen_fail > 0 {
            state.listen_fail -= 1;
            return Err(io::Error::other("injected listen failure"));
        }
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
        let mut state = self.state.lock().unwrap();
        if state.spawn_fail > 0 {
            state.spawn_fail -= 1;
            return Err("injected spawn failure".to_owned());
        }
        let pid = state.next_pid;
        state.next_pid += 1;
        state.spawned.push((id, pid));
        let lease = state.leases[&id].upgrade().unwrap();
        let tx = &state.incoming[&id];
        let spoof = if state.wrong_client {
            let (server, client) = tokio::io::duplex(4096);
            tx.send(Ok((
                FakeConn {
                    stream: server,
                    _lease: Arc::clone(&lease),
                },
                pid + 1000,
            )))
            .ok()
            .unwrap();
            Some(client)
        } else {
            None
        };
        if state.accept_fail {
            tx.send(Err(io::Error::other("injected accept failure")))
                .ok()
                .unwrap();
        }
        let (server, conn) = tokio::io::duplex(4096);
        tx.send(Ok((
            FakeConn {
                stream: server,
                _lease: lease,
            },
            pid,
        )))
        .ok()
        .unwrap();
        let (exit, _) = watch::channel(None);
        self.sessions
            .send(Session {
                id,
                pid,
                conn,
                exit: exit.clone(),
                spoof,
            })
            .ok()
            .unwrap();
        Ok((pid, FakeChild { pid, exit }))
    }

    async fn wait(child: Arc<FakeChild>) -> u32 {
        let mut exit = child.exit.subscribe();
        loop {
            if let Some(code) = *exit.borrow_and_update() {
                return code;
            }
            exit.changed().await.unwrap();
        }
    }

    fn kill(&self, child: &FakeChild) {
        let mut state = self.state.lock().unwrap();
        state.killed.push(child.pid);
        if !state.ignore_kill {
            child.exit.send_replace(Some(1));
        }
    }
}

#[derive(Default)]
struct TargetState {
    attached: Vec<u64>,
    detached: Vec<u64>,
    frames: Vec<(u64, PipeFrame)>,
    out: Option<mpsc::UnboundedSender<PipeFrame>>,
}

#[derive(Default)]
struct FakeTarget(Mutex<TargetState>);

impl AttachTarget for FakeTarget {
    fn attach(&self, out: mpsc::UnboundedSender<PipeFrame>) -> u64 {
        let mut state = self.0.lock().unwrap();
        let generation = state.attached.len() as u64 + 1;
        state.attached.push(generation);
        state.out = Some(out);
        generation
    }

    fn on_frame(&self, generation: u64, frame: PipeFrame) {
        self.0.lock().unwrap().frames.push((generation, frame));
    }

    fn detach(&self, generation: u64) {
        let mut state = self.0.lock().unwrap();
        state.detached.push(generation);
        if state.attached.last() == Some(&generation) {
            state.out = None;
        }
    }
}

struct Harness {
    ops: Arc<FakeOps>,
    sessions: mpsc::UnboundedReceiver<Session>,
    target: Arc<FakeTarget>,
    commands: mpsc::UnboundedSender<SvcCommand>,
    task: JoinHandle<DriverStats>,
}

impl Harness {
    fn new(console: Option<u32>) -> Self {
        Self::configured(console, SupervisorConfig::default(), |_| {})
    }

    fn configured(
        console: Option<u32>,
        cfg: SupervisorConfig,
        setup: impl FnOnce(&mut OpsState),
    ) -> Self {
        let (ops, sessions) = FakeOps::new(console);
        setup(&mut ops.state.lock().unwrap());
        let target = Arc::new(FakeTarget::default());
        let (commands, rx) = mpsc::unbounded_channel();
        let task = tokio::spawn(supervise(Arc::clone(&ops), Arc::clone(&target), cfg, rx));
        Self {
            ops,
            sessions,
            target,
            commands,
            task,
        }
    }

    async fn session(&mut self) -> Session {
        settle().await;
        self.sessions
            .try_recv()
            .expect("expected session at current virtual time")
    }

    fn change(&self, kind: SessionChangeKind, session_id: u32) {
        self.commands
            .send(SvcCommand::SessionChange { kind, session_id })
            .unwrap();
    }

    async fn finish(self) -> DriverStats {
        self.commands.send(SvcCommand::Shutdown).unwrap();
        let stats = self.task.await.unwrap();
        self.ops.no_live_pipes();
        stats
    }
}

async fn settle() {
    // Stay runnable so Tokio cannot auto-advance past a deadline under test.
    for _ in 0..32 {
        tokio::task::yield_now().await;
    }
}

async fn elapse(duration: Duration) {
    advance(duration).await;
    settle().await;
}

#[tokio::test(start_paused = true)]
async fn startup_handshake_and_bidirectional_frames() {
    let mut h = Harness::new(Some(1));
    let mut session = h.session().await;
    assert_eq!(session.id, 1);
    session.handshake().await;
    assert_eq!(h.target.0.lock().unwrap().attached, [1]);
    h.target
        .0
        .lock()
        .unwrap()
        .out
        .as_ref()
        .unwrap()
        .send(PipeFrame::Ctl(Ctl::Ping { n: 42 }))
        .unwrap();
    assert_eq!(
        read_frame(&mut session.conn).await.unwrap(),
        Some(PipeFrame::Ctl(Ctl::Ping { n: 42 }))
    );
    session
        .send(Ctl::Desktop {
            name: "Winlogon".to_owned(),
        })
        .await;
    settle().await;
    assert_eq!(
        h.target.0.lock().unwrap().frames,
        [(
            1,
            PipeFrame::Ctl(Ctl::Desktop {
                name: "Winlogon".to_owned()
            })
        )]
    );
    let target = Arc::clone(&h.target);
    let stats = h.finish().await;
    assert_eq!(
        stats,
        DriverStats {
            spawned: 1,
            connected: 1,
            exited: 1,
            killed: 2,
            ..Default::default()
        }
    );
    assert_eq!(target.0.lock().unwrap().detached, [1]);
}

#[tokio::test(start_paused = true)]
async fn unexpected_client_pid_is_dropped_then_accept_continues() {
    let mut h = Harness::configured(Some(1), SupervisorConfig::default(), |state| {
        state.wrong_client = true
    });
    let mut session = h.session().await;
    assert!(read_frame(session.spoof.as_mut().unwrap())
        .await
        .unwrap()
        .is_none());
    session.handshake().await;
    assert!(h.ops.killed().is_empty());
    let stats = h.finish().await;
    assert_eq!((stats.connected, stats.rejected), (1, 0));
}

async fn rejected_hello(ctl: impl FnOnce(&Session) -> Ctl) {
    let mut h = Harness::new(Some(1));
    let mut session = h.session().await;
    session.send(ctl(&session)).await;
    assert!(
        read_frame(&mut session.conn).await.unwrap().is_none(),
        "no Welcome on rejection"
    );
    settle().await;
    assert_eq!(h.ops.killed(), [session.pid]);
    assert!(h.target.0.lock().unwrap().attached.is_empty());
    elapse(Duration::from_millis(999)).await;
    assert_eq!(h.ops.spawned().len(), 1);
    elapse(Duration::from_millis(1)).await;
    let _retry = h.session().await;
    let stats = h.finish().await;
    assert_eq!((stats.spawned, stats.rejected, stats.killed), (2, 1, 3));
    assert_eq!(stats.spawn_failed, 0);
}

#[tokio::test(start_paused = true)]
async fn hello_pid_mismatch_kills_and_retries_after_one_second() {
    rejected_hello(|session| hello(1, session.id, session.pid + 1)).await;
}

#[tokio::test(start_paused = true)]
async fn hello_version_mismatch_kills_and_retries_after_one_second() {
    rejected_hello(|session| hello(2, session.id, session.pid)).await;
}

#[tokio::test(start_paused = true)]
async fn hello_session_mismatch_rejects() {
    rejected_hello(|session| hello(1, session.id + 1, session.pid)).await;
}

#[tokio::test(start_paused = true)]
async fn non_hello_control_rejects() {
    rejected_hello(|_| Ctl::Ping { n: 1 }).await;
}

#[tokio::test(start_paused = true)]
async fn hello_timeout_discards_partial_frame() {
    let mut h = Harness::new(Some(1));
    let mut session = h.session().await;
    // A partially consumed length prefix cannot be reused after timeout.
    session.conn.write_all(&[64, 0]).await.unwrap();
    settle().await;
    elapse(HELLO_TIMEOUT - Duration::from_millis(1)).await;
    assert!(h.ops.killed().is_empty());
    elapse(Duration::from_millis(1)).await;
    assert!(read_frame(&mut session.conn).await.unwrap().is_none());
    assert_eq!(h.ops.killed(), [session.pid]);
    let stats = h.finish().await;
    assert_eq!((stats.rejected, stats.killed, stats.exited), (1, 1, 1));
}

#[tokio::test(start_paused = true)]
async fn silence_times_out_at_hello_deadline() {
    let mut h = Harness::new(Some(1));
    let _session = h.session().await;
    elapse(HELLO_TIMEOUT).await;
    let stats = h.finish().await;
    assert_eq!((stats.rejected, stats.killed), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn rejection_waits_for_exit_before_starting_backoff() {
    let mut h = Harness::configured(Some(1), SupervisorConfig::default(), |state| {
        state.ignore_kill = true;
    });
    let mut session = h.session().await;
    session.send(hello(2, session.id, session.pid)).await;
    settle().await;
    elapse(HELLO_TIMEOUT * 2).await;
    assert_eq!(h.ops.killed(), [session.pid]);
    assert_eq!(h.ops.spawned().len(), 1);
    session.exit(1);
    settle().await;
    elapse(Duration::from_millis(999)).await;
    assert_eq!(h.ops.spawned().len(), 1);
    elapse(Duration::from_millis(1)).await;
    let _retry = h.session().await;
    h.ops.state.lock().unwrap().ignore_kill = false;
    let stats = h.finish().await;
    assert_eq!((stats.spawned, stats.rejected, stats.killed), (2, 1, 3));
}

#[tokio::test(start_paused = true)]
async fn invalid_first_frames_never_attach_or_receive_welcome() {
    use host_win::pipe_proto::{BlobKind, BlobPacket};

    for malformed in [false, true] {
        let mut h = Harness::new(Some(1));
        let mut session = h.session().await;
        if malformed {
            session.conn.write_all(&[0, 0, 0, 0]).await.unwrap();
        } else {
            write_frame(
                &mut session.conn,
                &PipeFrame::Blob(BlobPacket {
                    req: 1,
                    kind: BlobKind::Text,
                    data: b"test".to_vec(),
                }),
            )
            .await
            .unwrap();
        }
        assert!(read_frame(&mut session.conn).await.unwrap().is_none());
        settle().await;
        assert!(h.target.0.lock().unwrap().attached.is_empty());
        let stats = h.finish().await;
        assert_eq!((stats.rejected, stats.killed, stats.exited), (1, 1, 1));
    }
}

#[tokio::test(start_paused = true)]
async fn eof_before_hello_rejects() {
    let mut h = Harness::new(Some(1));
    let session = h.session().await;
    drop(session);
    settle().await;
    let stats = h.finish().await;
    assert_eq!((stats.rejected, stats.connected, stats.killed), (1, 0, 1));
}

#[tokio::test(start_paused = true)]
async fn supervisor_connect_deadline_uses_tokio_clock() {
    let cfg = SupervisorConfig {
        connect_timeout: Duration::from_secs(1),
        ..Default::default()
    };
    let mut h = Harness::configured(Some(1), cfg, |_| {});
    let session = h.session().await;
    elapse(Duration::from_millis(999)).await;
    assert!(h.ops.killed().is_empty());
    elapse(Duration::from_millis(1)).await;
    assert_eq!(h.ops.killed(), [session.pid]);
    elapse(Duration::from_secs(1)).await;
    let _retry = h.session().await;
    let stats = h.finish().await;
    assert_eq!((stats.spawned, stats.rejected, stats.exited), (2, 0, 2));
}

#[tokio::test(start_paused = true)]
async fn child_exit_detaches_and_doubles_backoff_without_pipe_collision() {
    let mut h = Harness::new(Some(1));
    let mut first = h.session().await;
    first.handshake().await;
    first.exit(17);
    settle().await;
    assert_eq!(h.target.0.lock().unwrap().detached, [1]);
    elapse(Duration::from_millis(999)).await;
    assert_eq!(h.ops.spawned().len(), 1);
    elapse(Duration::from_millis(1)).await;
    let mut second = h.session().await;
    second.handshake().await;
    second.exit(18);
    settle().await;
    elapse(Duration::from_millis(1999)).await;
    assert_eq!(h.ops.spawned().len(), 2);
    elapse(Duration::from_millis(1)).await;
    let _third = h.session().await;
    let stats = h.finish().await;
    assert_eq!((stats.spawned, stats.spawn_failed, stats.exited), (3, 0, 3));
}

#[tokio::test(start_paused = true)]
async fn pipe_eof_kills_live_child_then_retries_without_pipe_collision() {
    let mut h = Harness::new(Some(1));
    let mut first = h.session().await;
    first.handshake().await;
    let pid = first.pid;
    drop(first);
    settle().await;
    assert_eq!(h.ops.killed(), [pid]);
    assert_eq!(h.target.0.lock().unwrap().detached, [1]);
    elapse(Duration::from_secs(1)).await;
    let mut second = h.session().await;
    second.handshake().await;
    let stats = h.finish().await;
    assert_eq!(
        (stats.spawned, stats.spawn_failed, stats.connected),
        (2, 0, 2)
    );
}

#[tokio::test(start_paused = true)]
async fn writer_completion_also_detaches_and_kills() {
    let mut h = Harness::new(Some(1));
    let mut session = h.session().await;
    session.handshake().await;
    h.target.0.lock().unwrap().out = None;
    settle().await;
    assert_eq!(h.ops.killed(), [session.pid]);
    assert_eq!(h.target.0.lock().unwrap().detached, [1]);
    let stats = h.finish().await;
    assert_eq!((stats.connected, stats.killed, stats.exited), (1, 1, 1));
}

#[tokio::test(start_paused = true)]
async fn console_connect_command_replaces_child() {
    let mut h = Harness::new(Some(1));
    let mut first = h.session().await;
    first.handshake().await;
    h.change(SessionChangeKind::ConsoleConnect, 2);
    let mut second = h.session().await;
    assert_eq!(second.id, 2);
    second.handshake().await;
    assert_eq!(h.ops.killed(), [first.pid]);
    assert_eq!(h.target.0.lock().unwrap().detached, [1]);
    let stats = h.finish().await;
    assert_eq!((stats.spawned, stats.connected, stats.exited), (2, 2, 2));
}

#[tokio::test(start_paused = true)]
async fn polling_detects_changes_none_and_session_zero() {
    let mut h = Harness::new(Some(1));
    let mut first = h.session().await;
    first.handshake().await;
    h.ops.console(Some(2));
    elapse(CONSOLE_POLL - Duration::from_millis(1)).await;
    assert_eq!(h.ops.spawned().len(), 1);
    elapse(Duration::from_millis(1)).await;
    let mut second = h.session().await;
    assert_eq!(second.id, 2);
    second.handshake().await;
    // Unchanged console must not undo a WTS command selecting another session.
    h.change(SessionChangeKind::ConsoleConnect, 3);
    let mut third = h.session().await;
    third.handshake().await;
    elapse(CONSOLE_POLL).await;
    assert_eq!(h.ops.spawned().len(), 3);
    h.ops.console(None);
    elapse(CONSOLE_POLL).await;
    assert!(h.ops.killed().contains(&third.pid));
    h.ops.console(Some(0));
    elapse(CONSOLE_POLL * 2).await;
    assert_eq!(h.ops.spawned().len(), 3);
    let stats = h.finish().await;
    assert_eq!((stats.spawned, stats.killed, stats.exited), (3, 3, 3));
}

#[tokio::test(start_paused = true)]
async fn no_initial_spawn_for_absent_or_service_console() {
    for console in [None, Some(0), Some(NO_SESSION)] {
        let h = Harness::new(console);
        settle().await;
        elapse(CONSOLE_POLL * 2).await;
        assert!(h.ops.spawned().is_empty());
        assert_eq!(h.finish().await, DriverStats::default());
    }
}

#[tokio::test(start_paused = true)]
async fn listen_failure_retries_after_one_second() {
    let mut h = Harness::configured(Some(1), SupervisorConfig::default(), |state| {
        state.listen_fail = 1
    });
    settle().await;
    elapse(Duration::from_millis(999)).await;
    assert!(h.ops.spawned().is_empty());
    elapse(Duration::from_millis(1)).await;
    let _session = h.session().await;
    let stats = h.finish().await;
    assert_eq!((stats.spawn_failed, stats.spawned), (1, 1));
}

#[tokio::test(start_paused = true)]
async fn spawn_failures_retry_after_one_then_two_seconds_and_release_listener() {
    let mut h = Harness::configured(Some(1), SupervisorConfig::default(), |state| {
        state.spawn_fail = 2
    });
    settle().await;
    elapse(Duration::from_secs(1)).await;
    assert_eq!(h.ops.state.lock().unwrap().attempts.len(), 2);
    elapse(Duration::from_millis(1999)).await;
    assert!(h.ops.spawned().is_empty());
    elapse(Duration::from_millis(1)).await;
    let _session = h.session().await;
    {
        let state = h.ops.state.lock().unwrap();
        assert_eq!(
            state.attempts[1].1 - state.attempts[0].1,
            Duration::from_secs(1)
        );
        assert_eq!(
            state.attempts[2].1 - state.attempts[1].1,
            Duration::from_secs(2)
        );
    }
    let stats = h.finish().await;
    assert_eq!((stats.spawn_failed, stats.spawned), (2, 1));
}

#[tokio::test(start_paused = true)]
async fn same_session_logon_drops_blocked_writer_before_listen() {
    let mut h = Harness::new(Some(1));
    let mut first = h.session().await;
    first.handshake().await;
    h.target
        .0
        .lock()
        .unwrap()
        .out
        .as_ref()
        .unwrap()
        .send(PipeFrame::Ctl(Ctl::Desktop {
            name: "x".repeat(8192),
        }))
        .unwrap();
    settle().await;
    h.change(SessionChangeKind::Logon, 1);
    let mut second = h.session().await;
    second.handshake().await;
    assert_eq!(h.ops.killed(), [first.pid]);
    assert_eq!(h.target.0.lock().unwrap().detached, [1]);
    let stats = h.finish().await;
    assert_eq!((stats.spawn_failed, stats.spawned), (0, 2));
}

#[tokio::test(start_paused = true)]
async fn same_session_replacement_waits_at_most_kill_wait() {
    let mut h = Harness::configured(Some(1), SupervisorConfig::default(), |state| {
        state.ignore_kill = true
    });
    let mut first = h.session().await;
    first.handshake().await;
    h.change(SessionChangeKind::Logoff, 1);
    settle().await;
    elapse(KILL_WAIT - Duration::from_millis(1)).await;
    assert_eq!(h.ops.spawned().len(), 1);
    elapse(Duration::from_millis(1)).await;
    let mut second = h.session().await;
    second.handshake().await;
    // Late exit of the old process must not detach or restart its replacement.
    first.exit(1);
    settle().await;
    assert_eq!(h.target.0.lock().unwrap().detached, [1]);
    assert!(h.target.0.lock().unwrap().out.is_some());
    h.ops.state.lock().unwrap().ignore_kill = false;
    let stats = h.finish().await;
    assert_eq!((stats.spawned, stats.spawn_failed, stats.exited), (2, 0, 2));
}

#[tokio::test(start_paused = true)]
async fn shutdown_returns_after_five_seconds_if_all_children_ignore_kill() {
    let mut h = Harness::configured(Some(1), SupervisorConfig::default(), |state| {
        state.ignore_kill = true
    });
    let mut first = h.session().await;
    first.handshake().await;
    h.change(SessionChangeKind::ConsoleConnect, 2);
    let mut second = h.session().await;
    second.handshake().await;
    h.commands.send(SvcCommand::Shutdown).unwrap();
    settle().await;
    assert!(h.ops.killed().contains(&first.pid));
    assert!(h.ops.killed().contains(&second.pid));
    elapse(SHUTDOWN_WAIT - Duration::from_millis(1)).await;
    assert!(!h.task.is_finished());
    elapse(Duration::from_millis(1)).await;
    assert!(h.task.is_finished());
    let stats = h.task.await.unwrap();
    assert_eq!((stats.spawned, stats.exited, stats.killed), (2, 0, 4));
    assert_eq!(h.target.0.lock().unwrap().detached, [1, 2]);
    h.ops.no_live_pipes();
    assert_eq!(first.exit.receiver_count(), 0, "old wait task aborted");
    assert_eq!(second.exit.receiver_count(), 0, "current wait task aborted");
}

#[tokio::test(start_paused = true)]
async fn closing_commands_is_shutdown() {
    let mut h = Harness::new(Some(1));
    let mut session = h.session().await;
    session.handshake().await;
    drop(h.commands);
    let stats = h.task.await.unwrap();
    assert_eq!(
        (stats.spawned, stats.connected, stats.killed, stats.exited),
        (1, 1, 2, 1)
    );
    h.ops.no_live_pipes();
    assert_eq!(h.target.0.lock().unwrap().detached, [1]);
}

#[tokio::test(start_paused = true)]
async fn accept_failure_kills_without_counting_a_rejected_hello() {
    let mut h = Harness::configured(Some(1), SupervisorConfig::default(), |state| {
        state.accept_fail = true
    });
    let session = h.session().await;
    assert_eq!(h.ops.killed(), [session.pid]);
    let stats = h.finish().await;
    assert_eq!((stats.rejected, stats.killed, stats.exited), (0, 1, 1));
}

#[tokio::test(start_paused = true)]
async fn stable_child_resets_backoff() {
    let mut h = Harness::new(Some(1));
    let first = h.session().await;
    first.exit(1);
    settle().await;
    elapse(Duration::from_secs(1)).await;
    let mut stable = h.session().await;
    stable.handshake().await;
    elapse(SupervisorConfig::default().stable_after).await;
    stable.exit(1);
    settle().await;
    elapse(Duration::from_secs(1)).await;
    let _third = h.session().await;
    assert_eq!(h.finish().await.spawned, 3);
}

#[derive(Clone, Default)]
struct FakeLocal(FakePlatform);

impl ServiceLocal for FakeLocal {
    fn info(&self) -> HostInfo {
        self.0.info()
    }
    fn console_state(&self) -> ConsoleState {
        self.0.console_state()
    }
    fn nics(&self) -> Vec<Nic> {
        self.0.nics()
    }
    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> PlatformResult<()> {
        self.0.send_wol(mac, via)
    }
    fn keystore(&self) -> &dyn KeyStore {
        self.0.keystore()
    }
    fn ctrl_alt_del(&self) -> PlatformResult<()> {
        Ok(())
    }
}

#[test]
fn platform_clones_share_connection_and_generation() {
    let original = PipePlatform::new(FakeLocal::default());
    let cloned = original.clone();
    let (out, _rx) = mpsc::unbounded_channel();
    let generation = AttachTarget::attach(&cloned, out);
    assert!(original.is_attached());
    AttachTarget::on_frame(
        &cloned,
        generation,
        PipeFrame::Ctl(Ctl::Desktop {
            name: "Winlogon".to_owned(),
        }),
    );
    assert_eq!(original.desktop().as_deref(), Some("Winlogon"));
    AttachTarget::detach(&original, generation);
    assert!(!original.is_attached());
    assert!(!cloned.is_attached());
}
