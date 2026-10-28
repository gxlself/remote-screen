//! SSH 断线自动重连并续上 tmux。
//!
//! [`ResilientShell`] keeps one terminal byte stream alive across transport
//! failures. A new authenticated session is opened with the same tmux id and
//! the latest PTY size; the output receiver and event subscriber stay stable.

use std::{
    collections::VecDeque,
    future::Future,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex, MutexGuard,
    },
    time::{Duration, Instant},
};

use tokio::{
    sync::{broadcast, mpsc, oneshot, watch, Notify},
    task::JoinHandle,
};

use crate::{PtyOpts, Shell, SshError, SshEvent, SshSession};

const DEFAULT_BACKOFF_SECS: [u64; 5] = [1, 2, 4, 8, 15];
const OUTPUT_BUFFER: usize = 16;
const EVENT_BUFFER: usize = 64;

const ACTIVE: u8 = 0;
const RECONNECTING: u8 = 1;
const FINISHED: u8 = 2;

/// Supplies a fresh authenticated session for every reconnect attempt.
///
/// The App should obtain credentials again inside this callback: the authentication layer drops
/// credentials after a connection is established.
pub trait Connector: Send + Sync {
    fn connect(&self) -> Pin<Box<dyn Future<Output = Result<SshSession, SshError>> + Send + '_>>;
}

/// Backoff schedule and the maximum time spent recovering one disconnect.
#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ReconnectPolicy {
    /// Delay before attempt 1, 2, ... The last entry repeats indefinitely.
    pub backoff: Vec<Duration>,
    /// Measured from the moment the previous transport failed.
    pub give_up_after: Duration,
}

impl Default for ReconnectPolicy {
    fn default() -> Self {
        Self {
            backoff: DEFAULT_BACKOFF_SECS
                .into_iter()
                .map(Duration::from_secs)
                .collect(),
            give_up_after: Duration::from_secs(10 * 60),
        }
    }
}

/// Delay before a one-based reconnect attempt.
///
/// An empty schedule means no delay. Values past the end of the schedule use
/// its final element.
pub fn backoff_delay(policy: &ReconnectPolicy, attempt: u32) -> Duration {
    let Some(last) = policy.backoff.last() else {
        return Duration::ZERO;
    };
    let index = usize::try_from(attempt.saturating_sub(1))
        .unwrap_or(usize::MAX)
        .min(policy.backoff.len() - 1);
    policy.backoff.get(index).copied().unwrap_or(*last)
}

/// Whether retrying the transport can plausibly restore the shell.
///
/// Authentication and host-key failures are terminal. In particular a changed
/// host key must stop so the App can show its interception page.
pub fn is_recoverable(error: &SshError) -> bool {
    matches!(
        error,
        SshError::Timeout | SshError::Closed | SshError::Transport(_)
    )
}

fn error_kind(error: &SshError) -> &'static str {
    match error {
        SshError::AuthFailed { .. } => "auth_failed",
        SshError::HostKeyChanged { .. } => "host_key_changed",
        SshError::HostKeyRejected => "host_key_rejected",
        SshError::Unsupported(_) => "unsupported",
        SshError::InvalidKey => "invalid_key",
        SshError::SigningFailed => "signing_failed",
        SshError::Timeout => "timeout",
        SshError::Cancelled => "cancelled",
        SshError::InvalidOptions(_) => "invalid_options",
        SshError::RequestRejected => "request_rejected",
        SshError::Closed => "closed",
        SshError::Transport(_) => "transport",
    }
}

/// Lifecycle and transport state for a resilient terminal.
#[derive(Clone, Debug, PartialEq, Eq)]
pub enum TerminalEvent {
    Connected {
        generation: u32,
    },
    Reconnecting {
        attempt: u32,
        delay: Duration,
        cause: SshError,
    },
    Reconnected {
        generation: u32,
    },
    /// tmux was requested but the server fell back to a plain shell.
    TmuxUnavailable,
    Closed {
        status: Option<u32>,
    },
    GaveUp(SshError),
}

/// Result of an attempted terminal write.
#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum WriteOutcome {
    Sent,
    Dropped,
}

struct Shared {
    mode: Mutex<u8>,
    waiting_backoff: AtomicBool,
    user_closed: AtomicBool,
    dropped_input_bytes: AtomicU64,
    dimensions: Mutex<(u32, u32)>,
    output: mpsc::Sender<Vec<u8>>,
    events: broadcast::Sender<TerminalEvent>,
    commands: mpsc::UnboundedSender<Command>,
    close: watch::Sender<bool>,
    reconnect_now: Notify,
}

impl Shared {
    fn set_mode(&self, mode: u8) {
        *self
            .mode
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = mode;
    }

    fn dimensions(&self) -> (u32, u32) {
        *self
            .dimensions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }

    fn set_dimensions(&self, cols: u32, rows: u32) {
        *self
            .dimensions
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner()) = (cols, rows);
    }

    fn emit(&self, event: TerminalEvent) {
        if !self.user_closed.load(Ordering::SeqCst) {
            let _ = self.events.send(event);
        }
    }

    fn drop_input(&self, bytes: usize) {
        self.dropped_input_bytes
            .fetch_add(bytes as u64, Ordering::Relaxed);
    }

    fn lock_mode(&self) -> MutexGuard<'_, u8> {
        self.mode
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
    }
}

enum Command {
    Write {
        bytes: Vec<u8>,
        reply: oneshot::Sender<Result<WriteOutcome, SshError>>,
    },
    Resize {
        cols: u32,
        rows: u32,
        reply: oneshot::Sender<Result<(), SshError>>,
    },
}

/// A terminal whose byte stream can survive transport reconnects.
pub struct ResilientShell {
    output: mpsc::Receiver<Vec<u8>>,
    initial_events: Mutex<Option<broadcast::Receiver<TerminalEvent>>>,
    shared: Arc<Shared>,
    task: Option<JoinHandle<()>>,
}

impl ResilientShell {
    /// Opens the first session. Initial connection and shell failures are
    /// returned directly; automatic recovery starts only after this succeeds.
    pub async fn open(
        connector: Arc<dyn Connector>,
        pty: PtyOpts,
        policy: ReconnectPolicy,
    ) -> Result<Self, SshError> {
        let session = connector.connect().await?;
        // Subscribe before opening the shell so the tmux fallback event
        // cannot race with us.
        let mut session_events = session.events();
        let shell = match session.open_shell(pty.clone()).await {
            Ok(shell) => shell,
            Err(error) => {
                session.close().await;
                return Err(error);
            }
        };

        let (output_tx, output) = mpsc::channel(OUTPUT_BUFFER);
        let (events, initial_events) = broadcast::channel(EVENT_BUFFER);
        let (commands, command_rx) = mpsc::unbounded_channel();
        let (close, close_rx) = watch::channel(false);
        let shared = Arc::new(Shared {
            mode: Mutex::new(ACTIVE),
            waiting_backoff: AtomicBool::new(false),
            user_closed: AtomicBool::new(false),
            dropped_input_bytes: AtomicU64::new(0),
            dimensions: Mutex::new((pty.cols, pty.rows)),
            output: output_tx,
            events,
            commands,
            close,
            reconnect_now: Notify::new(),
        });

        shared.emit(TerminalEvent::Connected { generation: 1 });
        tracing::debug!(generation = 1, "SSH terminal connected");

        let mut tmux_unavailable_sent = false;
        if pty.tmux.is_some() {
            while let Ok(event) = session_events.try_recv() {
                if matches!(event, SshEvent::TmuxUnavailable) && !tmux_unavailable_sent {
                    tmux_unavailable_sent = true;
                    shared.emit(TerminalEvent::TmuxUnavailable);
                }
            }
        }
        let reconnect_allowed = pty.tmux.is_some() && !tmux_unavailable_sent;

        let task_shared = shared.clone();
        let task = tokio::spawn(async move {
            run(
                connector,
                session,
                shell,
                session_events,
                pty,
                policy,
                reconnect_allowed,
                tmux_unavailable_sent,
                command_rx,
                close_rx,
                task_shared,
            )
            .await;
        });

        Ok(Self {
            output,
            initial_events: Mutex::new(Some(initial_events)),
            shared,
            task: Some(task),
        })
    }

    /// Writes terminal input. Input during backoff is dropped and counted.
    pub async fn write(&self, bytes: &[u8]) -> Result<WriteOutcome, SshError> {
        let reply = {
            let mode = self.shared.lock_mode();
            match *mode {
                RECONNECTING => {
                    drop(mode);
                    self.shared.drop_input(bytes.len());
                    return Ok(WriteOutcome::Dropped);
                }
                FINISHED => return Err(SshError::Closed),
                _ => {
                    let (reply, result) = oneshot::channel();
                    self.shared
                        .commands
                        .send(Command::Write {
                            bytes: bytes.to_vec(),
                            reply,
                        })
                        .map_err(|_| SshError::Closed)?;
                    result
                }
            }
        };
        reply.await.map_err(|_| SshError::Closed)?
    }

    /// Updates the PTY size. During recovery this is only recorded and applied
    /// when the next shell is opened.
    pub async fn resize(&self, cols: u32, rows: u32) -> Result<(), SshError> {
        if cols == 0 || rows == 0 {
            return Err(SshError::InvalidOptions("PTY dimensions must be positive"));
        }
        self.shared.set_dimensions(cols, rows);
        let reply = {
            let mode = self.shared.lock_mode();
            match *mode {
                RECONNECTING => return Ok(()),
                FINISHED => return Err(SshError::Closed),
                _ => {
                    let (reply, result) = oneshot::channel();
                    self.shared
                        .commands
                        .send(Command::Resize { cols, rows, reply })
                        .map_err(|_| SshError::Closed)?;
                    result
                }
            }
        };
        reply.await.map_err(|_| SshError::Closed)?
    }

    /// The same bounded byte stream for every connection generation.
    pub fn output(&mut self) -> &mut mpsc::Receiver<Vec<u8>> {
        &mut self.output
    }

    /// The first subscriber also receives `Connected { generation: 1 }`, which
    /// is emitted before `open` returns. Later subscribers start at the tail.
    pub fn events(&self) -> broadcast::Receiver<TerminalEvent> {
        let mut initial = self
            .initial_events
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        match initial.take() {
            Some(receiver) => receiver,
            None => self.shared.events.subscribe(),
        }
    }

    /// Skips the remaining backoff delay. Outside a backoff this is a no-op.
    pub fn reconnect_now(&self) {
        let mode = self.shared.lock_mode();
        if *mode == RECONNECTING && self.shared.waiting_backoff.load(Ordering::SeqCst) {
            self.shared.reconnect_now.notify_one();
        }
    }

    /// Total bytes discarded by writes made while recovery was in progress.
    pub fn dropped_input_bytes(&self) -> u64 {
        self.shared.dropped_input_bytes.load(Ordering::Relaxed)
    }

    /// Stops recovery and closes the current transport. No event is emitted.
    pub async fn close(mut self) {
        self.shared.user_closed.store(true, Ordering::SeqCst);
        self.shared.set_mode(FINISHED);
        let _ = self.shared.close.send(true);
        self.output.close();
        if let Some(task) = self.task.take() {
            let _ = task.await;
        }
    }
}

impl Drop for ResilientShell {
    fn drop(&mut self) {
        self.shared.user_closed.store(true, Ordering::SeqCst);
        self.shared.set_mode(FINISHED);
        let _ = self.shared.close.send(true);
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

enum ActiveEnd {
    Remote(Option<u32>),
    Transport(SshError),
    Stopped,
}

#[allow(clippy::too_many_arguments)]
async fn run(
    connector: Arc<dyn Connector>,
    mut session: SshSession,
    mut shell: Shell,
    mut session_events: broadcast::Receiver<SshEvent>,
    pty: PtyOpts,
    policy: ReconnectPolicy,
    mut reconnect_allowed: bool,
    mut tmux_unavailable_sent: bool,
    mut commands: mpsc::UnboundedReceiver<Command>,
    mut close: watch::Receiver<bool>,
    shared: Arc<Shared>,
) {
    let mut generation = 1u32;
    loop {
        let end = forward(
            &session,
            &mut shell,
            &mut session_events,
            &mut commands,
            &mut close,
            &shared,
            &pty,
            &mut reconnect_allowed,
            &mut tmux_unavailable_sent,
        )
        .await;

        let cause = match end {
            ActiveEnd::Remote(status) => {
                tracing::debug!(generation, "SSH terminal closed by remote");
                shared.set_mode(FINISHED);
                shared.emit(TerminalEvent::Closed { status });
                drop(shell);
                session.close().await;
                return;
            }
            ActiveEnd::Stopped => {
                shared.set_mode(FINISHED);
                drop(shell);
                drop(session);
                return;
            }
            ActiveEnd::Transport(error) => error,
        };

        drain_session_events(
            &mut session_events,
            pty.tmux.is_some(),
            &mut reconnect_allowed,
            &mut tmux_unavailable_sent,
            &shared,
        );
        drop(shell);
        if is_closing(&mut close) {
            shared.set_mode(FINISHED);
            drop(session);
            return;
        }

        if !is_recoverable(&cause) || !reconnect_allowed {
            tracing::warn!(
                generation,
                error = error_kind(&cause),
                "SSH terminal gave up"
            );
            shared.set_mode(FINISHED);
            shared.emit(TerminalEvent::GaveUp(cause));
            session.close().await;
            return;
        }

        // The old transport is already unusable; closing it would only add
        // another timeout. Dropping releases the socket immediately.
        drop(session);

        let disconnected_at = Instant::now();
        let mut attempt = 1u32;
        let mut last_error = cause;
        {
            // Hold the state lock while draining so a write sees either a
            // completed queue entry or the reconnecting state, never neither.
            let mut mode = shared.lock_mode();
            *mode = RECONNECTING;
            drop_pending_input(&mut commands, &shared);
        }

        let next = loop {
            let remaining = policy
                .give_up_after
                .saturating_sub(disconnected_at.elapsed());
            if remaining.is_zero() {
                shared.set_mode(FINISHED);
                tracing::warn!(
                    attempt,
                    error = error_kind(&last_error),
                    "SSH terminal gave up"
                );
                shared.emit(TerminalEvent::GaveUp(last_error));
                return;
            }
            let delay = backoff_delay(&policy, attempt);
            shared.waiting_backoff.store(true, Ordering::SeqCst);
            shared.emit(TerminalEvent::Reconnecting {
                attempt,
                delay,
                cause: last_error.clone(),
            });
            tracing::warn!(
                attempt,
                error = error_kind(&last_error),
                "SSH terminal reconnecting"
            );

            if !wait_backoff(&shared, &mut close, delay.min(remaining)).await {
                shared.set_mode(FINISHED);
                if !shared.user_closed.load(Ordering::SeqCst) {
                    tracing::warn!(
                        attempt,
                        error = error_kind(&last_error),
                        "SSH terminal gave up"
                    );
                    shared.emit(TerminalEvent::GaveUp(last_error));
                }
                return;
            }
            if is_closing(&mut close) || shared.user_closed.load(Ordering::SeqCst) {
                shared.set_mode(FINISHED);
                return;
            }
            drop_pending_input(&mut commands, &shared);

            let remaining = policy
                .give_up_after
                .saturating_sub(disconnected_at.elapsed());
            if remaining.is_zero() {
                shared.set_mode(FINISHED);
                tracing::warn!(
                    attempt,
                    error = error_kind(&last_error),
                    "SSH terminal gave up"
                );
                shared.emit(TerminalEvent::GaveUp(last_error));
                return;
            }
            let connected = tokio::select! {
                result = tokio::time::timeout(remaining, connector.connect()) => {
                    match result {
                        Ok(result) => result,
                        Err(_) => {
                            shared.set_mode(FINISHED);
                            tracing::warn!(
                                attempt,
                                error = error_kind(&last_error),
                                "SSH terminal gave up"
                            );
                            shared.emit(TerminalEvent::GaveUp(last_error));
                            return;
                        }
                    }
                },
                _ = wait_closed(&mut close) => {
                    shared.set_mode(FINISHED);
                    return;
                }
            };
            let next_session = match connected {
                Ok(session) => session,
                Err(error) => {
                    if is_closing(&mut close) {
                        shared.set_mode(FINISHED);
                        return;
                    }
                    if !is_recoverable(&error) {
                        tracing::warn!(attempt, error = error_kind(&error), "SSH terminal gave up");
                        shared.set_mode(FINISHED);
                        shared.emit(TerminalEvent::GaveUp(error));
                        return;
                    }
                    last_error = error;
                    attempt = attempt.saturating_add(1);
                    continue;
                }
            };

            let mut next_events = next_session.events();
            let (cols, rows) = shared.dimensions();
            let opts = PtyOpts {
                term: pty.term.clone(),
                cols,
                rows,
                tmux: pty.tmux.clone(),
            };
            let remaining = policy
                .give_up_after
                .saturating_sub(disconnected_at.elapsed());
            if remaining.is_zero() {
                drop(next_session);
                shared.set_mode(FINISHED);
                tracing::warn!(
                    attempt,
                    error = error_kind(&last_error),
                    "SSH terminal gave up"
                );
                shared.emit(TerminalEvent::GaveUp(last_error));
                return;
            }
            let tmux_requested = opts.tmux.is_some();
            let opened = tokio::select! {
                result = tokio::time::timeout(remaining, next_session.open_shell(opts)) => {
                    match result {
                        Ok(result) => result,
                        Err(_) => {
                            drop(next_session);
                            shared.set_mode(FINISHED);
                            tracing::warn!(
                                attempt,
                                error = error_kind(&last_error),
                                "SSH terminal gave up"
                            );
                            shared.emit(TerminalEvent::GaveUp(last_error));
                            return;
                        }
                    }
                },
                _ = wait_closed(&mut close) => {
                    shared.set_mode(FINISHED);
                    // The transport may no longer answer a graceful close.
                    // Dropping the session closes its socket immediately.
                    drop(next_session);
                    return;
                }
            };
            match opened {
                Ok(next_shell) => {
                    let mut unavailable = false;
                    if tmux_requested {
                        while let Ok(event) = next_events.try_recv() {
                            if matches!(event, SshEvent::TmuxUnavailable) {
                                unavailable = true;
                            }
                        }
                    }
                    if unavailable && !tmux_unavailable_sent {
                        tmux_unavailable_sent = true;
                        reconnect_allowed = false;
                        shared.emit(TerminalEvent::TmuxUnavailable);
                    }
                    generation = generation.saturating_add(1);
                    shared.set_mode(ACTIVE);
                    shared.emit(TerminalEvent::Reconnected { generation });
                    tracing::debug!(generation, "SSH terminal reconnected");
                    break (next_session, next_shell, next_events);
                }
                Err(error) => {
                    let remaining = policy
                        .give_up_after
                        .saturating_sub(disconnected_at.elapsed());
                    if is_closing(&mut close) {
                        shared.set_mode(FINISHED);
                        drop(next_session);
                        return;
                    }
                    if !remaining.is_zero() {
                        let _ = tokio::time::timeout(remaining, next_session.close()).await;
                    } else {
                        drop(next_session);
                    }
                    if !is_recoverable(&error) {
                        tracing::warn!(attempt, error = error_kind(&error), "SSH terminal gave up");
                        shared.set_mode(FINISHED);
                        shared.emit(TerminalEvent::GaveUp(error));
                        return;
                    }
                    last_error = error;
                    attempt = attempt.saturating_add(1);
                }
            }
        };

        (session, shell, session_events) = next;
    }
}

fn drain_session_events(
    session_events: &mut broadcast::Receiver<SshEvent>,
    tmux_requested: bool,
    reconnect_allowed: &mut bool,
    tmux_unavailable_sent: &mut bool,
    shared: &Shared,
) {
    while let Ok(event) = session_events.try_recv() {
        if matches!(event, SshEvent::TmuxUnavailable) && tmux_requested && !*tmux_unavailable_sent {
            *tmux_unavailable_sent = true;
            *reconnect_allowed = false;
            shared.emit(TerminalEvent::TmuxUnavailable);
        }
    }
}

#[allow(clippy::too_many_arguments)]
async fn forward(
    session: &SshSession,
    shell: &mut Shell,
    session_events: &mut broadcast::Receiver<SshEvent>,
    commands: &mut mpsc::UnboundedReceiver<Command>,
    close: &mut watch::Receiver<bool>,
    shared: &Shared,
    pty: &PtyOpts,
    reconnect_allowed: &mut bool,
    tmux_unavailable_sent: &mut bool,
) -> ActiveEnd {
    let tmux_requested = pty.tmux.is_some();
    let closed = session.closed();
    tokio::pin!(closed);

    loop {
        tokio::select! {
            _ = wait_closed(close) => return ActiveEnd::Stopped,
            command = commands.recv() => {
                let Some(command) = command else { return ActiveEnd::Stopped; };
                if !handle_command(command, shell, close).await {
                    return ActiveEnd::Stopped;
                }
            }
            event = session_events.recv() => {
                if let Ok(SshEvent::TmuxUnavailable) = event {
                    if tmux_requested && !*tmux_unavailable_sent {
                        *tmux_unavailable_sent = true;
                        *reconnect_allowed = false;
                        shared.emit(TerminalEvent::TmuxUnavailable);
                    }
                }
            }
            chunk = shell.output().recv() => {
                match chunk {
                    Some(bytes) => {
                        if !send_output(shared, close, bytes).await {
                            return ActiveEnd::Stopped;
                        }
                    }
                    None => {
                        let result = shell.result().await;
                        return match result {
                            Ok(status) => ActiveEnd::Remote(status),
                            Err(error) => ActiveEnd::Transport(error),
                        };
                    }
                }
            }
            result = &mut closed => {
                let error = match result {
                    Ok(()) => SshError::Closed,
                    Err(error) => error,
                };
                let result = tokio::time::timeout(Duration::from_secs(1), shell.result())
                    .await
                    .unwrap_or_else(|_| Err(error.clone()));
                drain_output(shared, close, shell).await;
                drain_session_events(
                    session_events,
                    tmux_requested,
                    reconnect_allowed,
                    tmux_unavailable_sent,
                    shared,
                );
                return match result {
                    Ok(status) => ActiveEnd::Remote(status),
                    Err(shell_error) => ActiveEnd::Transport(shell_error),
                };
            }
        }
    }
}

async fn handle_command(
    command: Command,
    shell: &mut Shell,
    close: &mut watch::Receiver<bool>,
) -> bool {
    match command {
        Command::Write { bytes, reply } => {
            let result = tokio::select! {
                result = shell.write(&bytes) => result,
                _ = wait_closed(close) => return false,
            };
            let _ = reply.send(result.map(|()| WriteOutcome::Sent));
        }
        Command::Resize { cols, rows, reply } => {
            let result = tokio::select! {
                result = shell.resize(cols, rows) => result,
                _ = wait_closed(close) => return false,
            };
            let _ = reply.send(result);
        }
    }
    true
}

async fn send_output(shared: &Shared, close: &mut watch::Receiver<bool>, bytes: Vec<u8>) -> bool {
    if is_closing(close) {
        return false;
    }
    tokio::select! {
        result = shared.output.send(bytes) => result.is_ok(),
        _ = wait_closed(close) => false,
    }
}

async fn drain_output(shared: &Shared, close: &mut watch::Receiver<bool>, shell: &mut Shell) {
    let mut pending = VecDeque::new();
    while let Ok(bytes) = shell.output().try_recv() {
        pending.push_back(bytes);
    }
    for bytes in pending {
        if !send_output(shared, close, bytes).await {
            break;
        }
    }
}

fn drop_pending_input(commands: &mut mpsc::UnboundedReceiver<Command>, shared: &Shared) {
    while let Ok(command) = commands.try_recv() {
        match command {
            Command::Write { bytes, reply } => {
                shared.drop_input(bytes.len());
                let _ = reply.send(Ok(WriteOutcome::Dropped));
            }
            Command::Resize { reply, .. } => {
                let _ = reply.send(Ok(()));
            }
        }
    }
}

async fn wait_backoff(shared: &Shared, close: &mut watch::Receiver<bool>, delay: Duration) -> bool {
    let result = tokio::select! {
        _ = tokio::time::sleep(delay) => true,
        _ = shared.reconnect_now.notified() => true,
        _ = wait_closed(close) => false,
    };
    shared.waiting_backoff.store(false, Ordering::SeqCst);
    result
}

fn is_closing(close: &mut watch::Receiver<bool>) -> bool {
    match close.has_changed() {
        Ok(_) => *close.borrow_and_update(),
        Err(_) => true,
    }
}

async fn wait_closed(close: &mut watch::Receiver<bool>) {
    while !is_closing(close) {
        if close.changed().await.is_err() {
            return;
        }
    }
}
