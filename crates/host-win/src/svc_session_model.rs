//! Service session driver. OS operations are supplied by `ChildOps` so lifecycle
//! and pipe handling can be exercised with Tokio's paused clock on any platform.

use std::collections::{HashMap, VecDeque};
use std::future::{pending, Future};
use std::io;
use std::sync::Arc;
use std::time::Duration;

use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use tokio::time::{self, Instant};

use crate::pipe_proto::{check_hello, read_frame, write_frame, Ctl, PipeFrame, PIPE_PROTO_VERSION};
use crate::proxy::{PipePlatform, ServiceLocal};
use crate::supervisor::{
    ChildState, SessionChangeKind, SupAction, SupEvent, Supervisor, SupervisorConfig, NO_SESSION,
};

pub const SESSION_EXE: &str = "rs-host-session.exe";
pub const CONSOLE_POLL: Duration = Duration::from_secs(2);
pub const HELLO_TIMEOUT: Duration = Duration::from_secs(10);
pub const KILL_WAIT: Duration = Duration::from_secs(2);
pub const SHUTDOWN_WAIT: Duration = Duration::from_secs(5);

/// Resolve a sibling using either Windows or slash path separators.
pub fn sibling_exe(service_exe: &str, name: &str) -> String {
    match service_exe.rfind(['\\', '/']) {
        Some(index) => format!("{}{name}", &service_exe[..=index]),
        None => name.to_owned(),
    }
}

pub fn os_version_string(
    major: Option<u32>,
    minor: Option<u32>,
    build: Option<&str>,
    ubr: Option<u32>,
    display: Option<&str>,
) -> String {
    let Some(build) = build.and_then(|build| build.parse::<u32>().ok()) else {
        return "Windows".to_owned();
    };
    let product = if build >= 22000 {
        "Windows 11"
    } else {
        "Windows 10"
    };
    let mut version = format!("{}.{}.{build}", major.unwrap_or(10), minor.unwrap_or(0));
    if let Some(ubr) = ubr {
        version.push_str(&format!(".{ubr}"));
    }
    match display.filter(|display| !display.is_empty()) {
        Some(display) => format!("{product} {display} ({version})"),
        None => format!("{product} ({version})"),
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum HelloReject {
    NotHello,
    Version { got: u32 },
    Session { expected: u32, got: u32 },
    Pid { expected: u32, got: u32 },
}

pub fn verify_hello(ctl: &Ctl, session_id: u32, pid: u32) -> Result<(), HelloReject> {
    let Ctl::Hello {
        version,
        session_id: got_session,
        pid: got_pid,
    } = ctl
    else {
        return Err(HelloReject::NotHello);
    };
    check_hello(ctl).map_err(|_| HelloReject::Version { got: *version })?;
    if *got_session != session_id {
        return Err(HelloReject::Session {
            expected: session_id,
            got: *got_session,
        });
    }
    if *got_pid != pid {
        return Err(HelloReject::Pid {
            expected: pid,
            got: *got_pid,
        });
    }
    Ok(())
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SvcCommand {
    SessionChange {
        kind: SessionChangeKind,
        session_id: u32,
    },
    Shutdown,
}

pub trait ChildOps: Send + Sync + 'static {
    type Conn: AsyncRead + AsyncWrite + Unpin + Send + 'static;
    type Listener: Send + 'static;
    type Child: Send + Sync + 'static;
    fn console_session(&self) -> Option<u32>;
    fn listen(&self, session_id: u32) -> io::Result<Self::Listener>;
    fn accept(
        listener: &mut Self::Listener,
    ) -> impl Future<Output = io::Result<(Self::Conn, u32)>> + Send;
    fn spawn(&self, session_id: u32) -> Result<(u32, Self::Child), String>;
    fn wait(child: Arc<Self::Child>) -> impl Future<Output = u32> + Send + 'static;
    fn kill(&self, child: &Self::Child);
}

pub trait AttachTarget: Send + Sync + 'static {
    fn attach(&self, out: mpsc::UnboundedSender<PipeFrame>) -> u64;
    fn on_frame(&self, generation: u64, frame: PipeFrame);
    fn detach(&self, generation: u64);
}

impl<L: ServiceLocal> AttachTarget for PipePlatform<L> {
    fn attach(&self, out: mpsc::UnboundedSender<PipeFrame>) -> u64 {
        self.attach(out)
    }

    fn on_frame(&self, generation: u64, frame: PipeFrame) {
        self.on_frame(generation, frame);
    }

    fn detach(&self, generation: u64) {
        self.detach(generation);
    }
}

#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct DriverStats {
    pub spawned: u64,
    pub spawn_failed: u64,
    pub connected: u64,
    pub rejected: u64,
    pub exited: u64,
    pub killed: u64,
}

enum Event {
    Connected { pid: u32 },
    Rejected { pid: u32, reason: String },
    PipeEnded { pid: u32 },
    Exited { pid: u32, code: u32 },
}

struct Child<C> {
    session_id: u32,
    process: Arc<C>,
    conn: Option<JoinHandle<()>>,
    wait: JoinHandle<()>,
    kill_requested: bool,
}

impl<C> Child<C> {
    async fn close_pipe(&mut self) {
        if let Some(conn) = self.conn.take() {
            conn.abort();
            let _ = conn.await;
        }
    }
}

impl<C> Drop for Child<C> {
    fn drop(&mut self) {
        if let Some(conn) = &self.conn {
            conn.abort();
        }
        self.wait.abort();
    }
}

struct Driver<O: ChildOps, T> {
    ops: Arc<O>,
    target: Arc<T>,
    supervisor: Supervisor,
    actions: VecDeque<SupAction>,
    children: HashMap<u32, Child<O::Child>>,
    events_tx: mpsc::UnboundedSender<Event>,
    events: mpsc::UnboundedReceiver<Event>,
    stats: DriverStats,
}

impl<O: ChildOps, T: AttachTarget> Driver<O, T> {
    fn feed(&mut self, event: SupEvent) {
        self.actions
            .extend(self.supervisor.handle(event, Instant::now().into_std()));
    }

    fn kill(&mut self, pid: u32) {
        if let Some(child) = self.children.get_mut(&pid) {
            if let Some(conn) = &child.conn {
                conn.abort();
            }
            self.ops.kill(&child.process);
            child.kill_requested = true;
            self.stats.killed += 1;
        }
    }

    fn next_deadline(&self) -> Option<std::time::Instant> {
        if let ChildState::Starting { pid, .. } = self.supervisor.state() {
            if self
                .children
                .get(pid)
                .is_some_and(|child| child.kill_requested)
            {
                // Rejection/pipe failure already requested termination. Its
                // Exited event, not a simultaneous connect timeout, starts the
                // backoff. Do not repeatedly tick an already expired deadline.
                return None;
            }
        }
        self.supervisor.next_deadline()
    }

    async fn event(&mut self, event: Event) {
        match event {
            Event::Connected { pid } => {
                self.stats.connected += 1;
                self.feed(SupEvent::Connected { pid });
            }
            Event::Rejected { pid, reason } => {
                self.stats.rejected += 1;
                tracing::info!(pid, reason, "session handshake rejected");
                self.kill(pid);
            }
            Event::PipeEnded { pid } => self.kill(pid),
            Event::Exited { pid, code } => {
                if let Some(mut child) = self.children.remove(&pid) {
                    // Await cancellation even on exit: otherwise a subsequent
                    // listen can race destruction of the last pipe handle.
                    child.close_pipe().await;
                    self.stats.exited += 1;
                    tracing::info!(pid, code, "session process exited");
                    self.feed(SupEvent::Exited { pid, code });
                }
            }
        }
    }

    async fn release_session(&mut self, session_id: u32) {
        for child in self
            .children
            .values_mut()
            .filter(|child| child.session_id == session_id)
        {
            child.close_pipe().await;
        }
        let deadline = Instant::now() + KILL_WAIT;
        while self
            .children
            .values()
            .any(|child| child.session_id == session_id)
        {
            match time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(event)) => self.event(event).await,
                _ => break,
            }
        }
    }

    async fn actions(&mut self) {
        while let Some(action) = self.actions.pop_front() {
            match action {
                SupAction::Kill { pid } => self.kill(pid),
                SupAction::Spawn { session_id } => {
                    self.release_session(session_id).await;
                    let listener = match self.ops.listen(session_id) {
                        Ok(listener) => listener,
                        Err(error) => {
                            self.spawn_failed(session_id, error.to_string());
                            continue;
                        }
                    };
                    let (pid, process) = match self.ops.spawn(session_id) {
                        Ok(child) => child,
                        Err(error) => {
                            drop(listener);
                            self.spawn_failed(session_id, error);
                            continue;
                        }
                    };
                    let process = Arc::new(process);
                    let waiting = Arc::clone(&process);
                    let events = self.events_tx.clone();
                    let wait = tokio::spawn(async move {
                        let code = O::wait(waiting).await;
                        let _ = events.send(Event::Exited { pid, code });
                    });
                    let conn = tokio::spawn(connection::<O, T>(
                        listener,
                        session_id,
                        pid,
                        Arc::clone(&self.target),
                        self.events_tx.clone(),
                    ));
                    self.children.insert(
                        pid,
                        Child {
                            session_id,
                            process,
                            conn: Some(conn),
                            wait,
                            kill_requested: false,
                        },
                    );
                    self.stats.spawned += 1;
                    self.feed(SupEvent::Spawned { session_id, pid });
                }
            }
        }
    }

    fn spawn_failed(&mut self, session_id: u32, error: String) {
        self.stats.spawn_failed += 1;
        // OS errors may contain paths; only log lifecycle identifiers/counts.
        tracing::warn!(
            session_id,
            spawn_failed = self.stats.spawn_failed,
            "session spawn failed"
        );
        self.feed(SupEvent::SpawnFailed { session_id, error });
    }

    async fn shutdown(&mut self) {
        self.feed(SupEvent::Shutdown);
        self.actions().await;
        for pid in self.children.keys().copied().collect::<Vec<_>>() {
            self.kill(pid);
        }
        let deadline = Instant::now() + SHUTDOWN_WAIT;
        while !self.children.is_empty() {
            match time::timeout_at(deadline, self.events.recv()).await {
                Ok(Some(event)) => self.event(event).await,
                _ => break,
            }
        }
        for (_, mut child) in self.children.drain() {
            child.close_pipe().await;
            child.wait.abort();
            let _ = (&mut child.wait).await;
        }
    }
}

/// Run until Shutdown or command-channel closure, then reap children boundedly.
pub async fn supervise<O: ChildOps, T: AttachTarget>(
    ops: Arc<O>,
    target: Arc<T>,
    cfg: SupervisorConfig,
    mut commands: mpsc::UnboundedReceiver<SvcCommand>,
) -> DriverStats {
    let (events_tx, events) = mpsc::unbounded_channel();
    let mut driver = Driver {
        ops,
        target,
        supervisor: Supervisor::new(cfg),
        actions: VecDeque::new(),
        children: HashMap::new(),
        events_tx,
        events,
        stats: DriverStats::default(),
    };
    let mut console = console_session(&*driver.ops);
    driver.feed(SupEvent::ConsoleSession(console));
    let mut poll = time::interval_at(Instant::now() + CONSOLE_POLL, CONSOLE_POLL);
    poll.set_missed_tick_behavior(time::MissedTickBehavior::Skip);
    loop {
        driver.actions().await;
        let deadline = driver.next_deadline();
        tokio::select! {
            command = commands.recv() => match command {
                Some(SvcCommand::SessionChange { kind, session_id }) => {
                    driver.feed(SupEvent::SessionChange { kind, session_id });
                }
                Some(SvcCommand::Shutdown) | None => break,
            },
            Some(event) = driver.events.recv() => driver.event(event).await,
            _ = poll.tick() => {
                let next = console_session(&*driver.ops);
                if next != console {
                    console = next;
                    driver.feed(SupEvent::ConsoleSession(console));
                }
            }
            _ = deadline_elapsed(deadline) => {
                // The Hello timeout and supervisor deadline can coincide. Let
                // ready pipe tasks report their rejection before applying Tick.
                tokio::task::yield_now().await;
                while let Ok(event) = driver.events.try_recv() {
                    driver.event(event).await;
                }
                if driver.next_deadline().is_some_and(|deadline| deadline <= Instant::now().into_std()) {
                    driver.feed(SupEvent::Tick);
                }
            }
        }
    }
    driver.shutdown().await;
    driver.stats
}

fn console_session<O: ChildOps>(ops: &O) -> Option<u32> {
    ops.console_session()
        .filter(|id| *id != 0 && *id != NO_SESSION)
}

async fn deadline_elapsed(deadline: Option<std::time::Instant>) {
    match deadline {
        Some(deadline) => time::sleep_until(Instant::from_std(deadline)).await,
        None => pending().await,
    }
}

struct DetachGuard<T: AttachTarget> {
    target: Arc<T>,
    generation: u64,
}

impl<T: AttachTarget> Drop for DetachGuard<T> {
    fn drop(&mut self) {
        self.target.detach(self.generation);
    }
}

async fn connection<O: ChildOps, T: AttachTarget>(
    mut listener: O::Listener,
    session_id: u32,
    pid: u32,
    target: Arc<T>,
    events: mpsc::UnboundedSender<Event>,
) {
    let mut conn = loop {
        match O::accept(&mut listener).await {
            Ok((conn, client_pid)) if client_pid == pid => break conn,
            Ok((_, client_pid)) => {
                tracing::warn!(
                    pid,
                    client_pid,
                    session_id,
                    "discarding unexpected pipe client"
                );
            }
            Err(_) => {
                let _ = events.send(Event::PipeEnded { pid });
                return;
            }
        }
    };
    let rejection = match time::timeout(HELLO_TIMEOUT, read_frame(&mut conn)).await {
        Ok(Ok(Some(PipeFrame::Ctl(ctl)))) => verify_hello(&ctl, session_id, pid)
            .err()
            .map(|reason| format!("{reason:?}")),
        Ok(Ok(Some(_))) => Some("non-control first frame".to_owned()),
        Ok(Ok(None)) => Some("EOF before Hello".to_owned()),
        Ok(Err(_)) => Some("cannot read Hello".to_owned()),
        Err(_) => Some("Hello timeout".to_owned()),
    };
    if let Some(reason) = rejection {
        // read_frame is not cancellation-safe; this connection is discarded.
        let _ = events.send(Event::Rejected { pid, reason });
        return;
    }
    if write_frame(
        &mut conn,
        &PipeFrame::Ctl(Ctl::Welcome {
            version: PIPE_PROTO_VERSION,
        }),
    )
    .await
    .is_err()
        || conn.flush().await.is_err()
    {
        let _ = events.send(Event::PipeEnded { pid });
        return;
    }
    let (mut reader, mut writer) = tokio::io::split(conn);
    let (out, mut outgoing) = mpsc::unbounded_channel();
    let generation = target.attach(out);
    let guard = DetachGuard {
        target: Arc::clone(&target),
        generation,
    };
    let _ = events.send(Event::Connected { pid });
    // Keep both pump futures inside this task. Aborting AND awaiting it must
    // drop both halves, including a blocked writer, before another first pipe
    // instance can be created. A detached tokio::spawn writer cannot ensure it.
    let write = async {
        while let Some(frame) = outgoing.recv().await {
            if write_frame(&mut writer, &frame).await.is_err() {
                break;
            }
        }
    };
    let read = async {
        while let Ok(Some(frame)) = read_frame(&mut reader).await {
            target.on_frame(generation, frame);
        }
    };
    tokio::select! {
        _ = write => {},
        _ = read => {},
    }
    drop(guard);
    let _ = events.send(Event::PipeEnded { pid });
}
