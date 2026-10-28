//! Resilient-shell tests against in-process russh servers on loopback ports.
use std::{
    collections::HashMap,
    future::Future,
    net::SocketAddr,
    pin::Pin,
    sync::{Arc, Mutex},
    time::{Duration, Instant},
};

use async_trait::async_trait;
use rs_ssh::reconnect::{
    backoff_delay, is_recoverable, Connector, ReconnectPolicy, ResilientShell, TerminalEvent,
    WriteOutcome,
};
use rs_ssh::{
    Credential, HostKeyStore, MemoryHostKeyStore, Prompter, PtyOpts, SshError, SshOptions,
    SshSession, SshTarget,
};
use russh::{
    server::{self, Auth},
    Channel, ChannelId, MethodKind, MethodSet, Pty,
};
use ssh_key::{private::Ed25519Keypair, PrivateKey};
use tokio::{
    io::copy_bidirectional,
    net::{TcpListener, TcpStream},
    sync::{watch, RwLock},
    time::timeout,
};
use zeroize::Zeroizing;

fn private(seed: u8) -> PrivateKey {
    PrivateKey::new(Ed25519Keypair::from_seed(&[seed; 32]).into(), "").unwrap()
}

fn options() -> SshOptions {
    SshOptions {
        connect_timeout: Duration::from_secs(2),
        auth_timeout: Duration::from_secs(2),
        request_timeout: Duration::from_secs(2),
        keepalive_interval: Duration::from_millis(100),
        keepalive_max: 3,
        ..Default::default()
    }
}

#[derive(Default)]
struct TestPrompter {
    confirmations: Mutex<Vec<(String, String, String)>>,
}

#[async_trait]
impl Prompter for TestPrompter {
    async fn confirm_new_host_key(&self, host: &str, fp: &str, algo: &str) -> bool {
        self.confirmations
            .lock()
            .unwrap()
            .push((host.into(), fp.into(), algo.into()));
        true
    }

    async fn keyboard_interactive(
        &self,
        _: &str,
        _: &str,
        _: &[(String, bool)],
    ) -> Option<Vec<String>> {
        None
    }

    fn auth_url(&self, _: &str) {}
}

#[derive(Default)]
struct Observed {
    ptys: Vec<(String, u32, u32)>,
    commands: Vec<String>,
    attach_counts: HashMap<String, usize>,
    connections: usize,
    open_connections: usize,
}

#[derive(Clone, Copy, Default)]
struct Scenario {
    tmux_missing: bool,
    exit_immediately: bool,
}

#[derive(Clone)]
struct ServerState {
    scenario: Scenario,
    keys: Arc<RwLock<Arc<PrivateKey>>>,
    observed: Arc<Mutex<Observed>>,
}

struct TestServer {
    address: SocketAddr,
    state: ServerState,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for TestServer {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl TestServer {
    async fn start(scenario: Scenario) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let state = ServerState {
            scenario,
            keys: Arc::new(RwLock::new(Arc::new(private(1)))),
            observed: Arc::new(Mutex::new(Observed::default())),
        };
        let task_state = state.clone();
        let task = tokio::spawn(async move {
            let mut peers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let config = Arc::new(server::Config {
                            keys: vec![task_state.keys.read().await.as_ref().clone()],
                            methods: MethodSet::from([MethodKind::PublicKey].as_slice()),
                            auth_rejection_time: Duration::from_millis(1),
                            auth_rejection_time_initial: Some(Duration::from_millis(1)),
                            max_auth_attempts: 20,
                            channel_buffer_size: 2,
                            event_buffer_size: 4,
                            nodelay: true,
                            ..Default::default()
                        });
                        {
                            let mut observed = task_state.observed.lock().unwrap();
                            observed.connections += 1;
                            observed.open_connections += 1;
                        }
                        let observed = task_state.observed.clone();
                        let handler = TestHandler {
                            scenario: task_state.scenario,
                            observed: observed.clone(),
                            channels: HashMap::new(),
                        };
                        peers.spawn(async move {
                            if let Ok(peer) = server::run_stream(config, stream, handler).await {
                                let _ = peer.await;
                            }
                            observed.lock().unwrap().open_connections -= 1;
                        });
                    }
                    _ = peers.join_next(), if !peers.is_empty() => {}
                }
            }
        });
        Self {
            address,
            state,
            task,
        }
    }

    async fn rotate_host_key(&self) {
        *self.state.keys.write().await = Arc::new(private(9));
    }
}

struct TestHandler {
    scenario: Scenario,
    observed: Arc<Mutex<Observed>>,
    channels: HashMap<ChannelId, Channel<server::Msg>>,
}

impl server::Handler for TestHandler {
    type Error = russh::Error;

    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        _: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn auth_publickey(
        &mut self,
        _: &str,
        _: &ssh_key::PublicKey,
    ) -> Result<Auth, Self::Error> {
        Ok(Auth::Accept)
    }

    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let _ = channel;
        reply.accept().await;
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    async fn pty_request(
        &mut self,
        channel: ChannelId,
        term: &str,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &[(Pty, u32)],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.observed
            .lock()
            .unwrap()
            .ptys
            .push((term.into(), cols, rows));
        session.channel_success(channel)
    }

    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.channel_success(channel)?;
        session.data(channel, &b"fallback\r\n"[..])?;
        Ok(())
    }

    async fn window_change_request(
        &mut self,
        _: ChannelId,
        _: u32,
        _: u32,
        _: u32,
        _: u32,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        Ok(())
    }

    async fn exec_request(
        &mut self,
        channel: ChannelId,
        cmd: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        let cmd = String::from_utf8_lossy(cmd).into_owned();
        self.observed.lock().unwrap().commands.push(cmd.clone());
        session.channel_success(channel)?;
        if cmd.starts_with("tmux ") {
            if self.scenario.tmux_missing {
                session.extended_data(channel, 1, &b"tmux: command not found\r\n"[..])?;
                session.exit_status_request(channel, 127)?;
                session.eof(channel)?;
                session.close(channel)?;
            } else {
                let id = cmd.split(" -s rs-").nth(1).unwrap_or("unknown").to_owned();
                let (attaches, cols, rows) = {
                    let mut observed = self.observed.lock().unwrap();
                    let count = observed.attach_counts.entry(id.clone()).or_default();
                    *count += 1;
                    let count = *count;
                    let (cols, rows) = observed
                        .ptys
                        .last()
                        .map(|(_, cols, rows)| (*cols, *rows))
                        .unwrap_or_default();
                    (count, cols, rows)
                };
                let banner = format!("[attach rs-{id} #{attaches} {cols}x{rows}]\r\n");
                session.data(channel, banner.into_bytes())?;
                if self.scenario.exit_immediately {
                    session.exit_status_request(channel, 0)?;
                    session.eof(channel)?;
                    session.close(channel)?;
                }
            }
        } else {
            session.data(channel, &b"stdout\0bytes"[..])?;
        }
        Ok(())
    }

    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        session.data(channel, data.to_vec())?;
        Ok(())
    }

    async fn channel_close(
        &mut self,
        channel: ChannelId,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.channels.remove(&channel);
        Ok(())
    }
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ProxyMode {
    Forwarding,
    Cut,
    Blackhole,
}

struct CutProxy {
    address: SocketAddr,
    mode: watch::Sender<ProxyMode>,
    task: tokio::task::JoinHandle<()>,
}

impl Drop for CutProxy {
    fn drop(&mut self) {
        self.task.abort();
    }
}

impl CutProxy {
    async fn start(upstream: SocketAddr) -> Self {
        let listener = TcpListener::bind(("127.0.0.1", 0)).await.unwrap();
        let address = listener.local_addr().unwrap();
        let (mode, _) = watch::channel(ProxyMode::Forwarding);
        let task_mode = mode.clone();
        let task = tokio::spawn(async move {
            let mut peers = tokio::task::JoinSet::new();
            loop {
                let accepted = listener.accept().await;
                let Ok((mut client, _)) = accepted else {
                    break;
                };
                let mut mode = task_mode.subscribe();
                peers.spawn(async move {
                    let mut server = None;
                    loop {
                        let current = *mode.borrow_and_update();
                        match current {
                            ProxyMode::Cut => return,
                            ProxyMode::Blackhole => {
                                // Keep both TCP legs open, but do not move bytes.
                                if mode.changed().await.is_err() {
                                    return;
                                }
                            }
                            ProxyMode::Forwarding => {
                                if server.is_none() {
                                    match TcpStream::connect(upstream).await {
                                        Ok(stream) => server = Some(stream),
                                        Err(_) => return,
                                    }
                                }
                                let upstream = server.as_mut().expect("upstream connected");
                                let changed = tokio::select! {
                                    _ = copy_bidirectional(&mut client, upstream) => return,
                                    changed = mode.changed() => changed,
                                };
                                if changed.is_err() {
                                    return;
                                }
                            }
                        }
                    }
                });
            }
        });
        Self {
            address,
            mode,
            task,
        }
    }

    fn target(&self) -> SshTarget {
        SshTarget {
            host: "127.0.0.1".into(),
            port: self.address.port(),
            user: "test".into(),
            jump: None,
        }
    }

    fn cut(&self) {
        self.mode.send_replace(ProxyMode::Cut);
    }

    fn blackhole(&self) {
        self.mode.send_replace(ProxyMode::Blackhole);
    }

    fn restore(&self) {
        self.mode.send_replace(ProxyMode::Forwarding);
    }
}

struct TestConnector {
    target: SshTarget,
    store: Arc<MemoryHostKeyStore>,
    prompter: Arc<TestPrompter>,
}

impl Connector for TestConnector {
    fn connect(&self) -> Pin<Box<dyn Future<Output = Result<SshSession, SshError>> + Send + '_>> {
        let target = self.target.clone();
        let store = self.store.clone();
        let prompter = self.prompter.clone();
        Box::pin(async move {
            SshSession::connect(
                target,
                vec![Credential::Ed25519 {
                    secret: Zeroizing::new([7u8; 32]),
                }],
                store,
                prompter,
                options(),
            )
            .await
        })
    }
}

fn policy(backoff: &[Duration]) -> ReconnectPolicy {
    ReconnectPolicy {
        backoff: backoff.to_vec(),
        give_up_after: Duration::from_secs(10),
    }
}

async fn next_event(events: &mut tokio::sync::broadcast::Receiver<TerminalEvent>) -> TerminalEvent {
    timeout(Duration::from_secs(3), events.recv())
        .await
        .unwrap()
        .unwrap()
}

async fn connect_via_proxy(
    server: &TestServer,
    store: Arc<MemoryHostKeyStore>,
    prompter: Arc<TestPrompter>,
) -> (CutProxy, Arc<TestConnector>) {
    let proxy = CutProxy::start(server.address).await;
    let connector = Arc::new(TestConnector {
        target: proxy.target(),
        store,
        prompter,
    });
    (proxy, connector)
}

async fn collect_until(output: &mut tokio::sync::mpsc::Receiver<Vec<u8>>, needle: &[u8]) -> String {
    let deadline = Instant::now() + Duration::from_secs(3);
    let mut collected = Vec::new();
    loop {
        let remaining = deadline.saturating_duration_since(Instant::now());
        let chunk = timeout(remaining, output.recv())
            .await
            .unwrap_or_else(|_| {
                panic!(
                    "output deadline waiting for {:?}; got {:?}",
                    String::from_utf8_lossy(needle),
                    String::from_utf8_lossy(&collected)
                )
            })
            .expect("output closed");
        collected.extend_from_slice(&chunk);
        if collected
            .windows(needle.len())
            .any(|window| window == needle)
        {
            return String::from_utf8_lossy(&collected).into_owned();
        }
    }
}

#[test]
fn backoff_delay_table_and_custom_policy() {
    let default = ReconnectPolicy::default();
    assert_eq!(
        (1..=7)
            .map(|attempt| backoff_delay(&default, attempt))
            .collect::<Vec<_>>(),
        vec![
            Duration::from_secs(1),
            Duration::from_secs(2),
            Duration::from_secs(4),
            Duration::from_secs(8),
            Duration::from_secs(15),
            Duration::from_secs(15),
            Duration::from_secs(15),
        ]
    );
    let custom = ReconnectPolicy {
        backoff: vec![Duration::from_millis(50), Duration::from_millis(100)],
        give_up_after: Duration::from_secs(10),
    };
    assert_eq!(backoff_delay(&custom, 1), Duration::from_millis(50));
    assert_eq!(backoff_delay(&custom, 2), Duration::from_millis(100));
    assert_eq!(backoff_delay(&custom, 3), Duration::from_millis(100));
    assert_eq!(backoff_delay(&custom, 0), Duration::from_millis(50));
}

#[test]
fn recoverability_covers_every_error_variant() {
    let recoverable = [
        SshError::Timeout,
        SshError::Closed,
        SshError::Transport("socket".into()),
    ];
    let terminal = [
        SshError::AuthFailed { tried: vec![] },
        SshError::HostKeyChanged {
            expected: "expected".into(),
            got: "got".into(),
        },
        SshError::HostKeyRejected,
        SshError::Unsupported("feature"),
        SshError::InvalidKey,
        SshError::SigningFailed,
        SshError::Cancelled,
        SshError::InvalidOptions("options"),
        SshError::RequestRejected,
    ];
    for error in recoverable {
        assert!(is_recoverable(&error), "{error:?}");
    }
    for error in terminal {
        assert!(!is_recoverable(&error), "{error:?}");
    }
}

#[tokio::test]
async fn cut_reconnects_same_tmux_with_latest_size_and_echo() {
    let server = TestServer::start(Scenario::default()).await;
    let store = Arc::new(MemoryHostKeyStore::default());
    let (proxy, connector) =
        connect_via_proxy(&server, store, Arc::new(TestPrompter::default())).await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t1".into()),
            ..Default::default()
        },
        policy(&[Duration::from_millis(50), Duration::from_millis(100)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    collect_until(shell.output(), b"[attach rs-t1 #1 80x24]").await;
    assert_eq!(shell.write(b"abc").await.unwrap(), WriteOutcome::Sent);
    collect_until(shell.output(), b"abc").await;

    proxy.cut();
    assert!(matches!(
        next_event(&mut events).await,
        TerminalEvent::Reconnecting { attempt: 1, .. }
    ));
    assert_eq!(shell.write(b"lost").await.unwrap(), WriteOutcome::Dropped);
    assert_eq!(shell.dropped_input_bytes(), 4);
    shell.resize(120, 40).await.unwrap();
    proxy.restore();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Reconnected { generation: 2 }
    );
    collect_until(shell.output(), b"[attach rs-t1 #2 120x40]").await;
    assert_eq!(shell.write(b"xyz").await.unwrap(), WriteOutcome::Sent);
    collect_until(shell.output(), b"xyz").await;
    shell.close().await;
}

#[tokio::test]
async fn blackhole_times_out_then_restores() {
    let server = TestServer::start(Scenario::default()).await;
    let (proxy, connector) = connect_via_proxy(
        &server,
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(TestPrompter::default()),
    )
    .await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t2".into()),
            ..Default::default()
        },
        policy(&[Duration::from_millis(50), Duration::from_millis(100)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    collect_until(shell.output(), b"[attach rs-t2 #1 80x24]").await;
    let started = Instant::now();
    proxy.blackhole();
    let reconnecting = next_event(&mut events).await;
    assert!(started.elapsed() >= Duration::from_millis(250));
    assert!(
        started.elapsed() <= Duration::from_millis(900),
        "keepalive took {:?}",
        started.elapsed()
    );
    assert!(matches!(
        reconnecting,
        TerminalEvent::Reconnecting {
            cause: SshError::Timeout,
            ..
        }
    ));
    proxy.restore();
    assert!(matches!(
        next_event(&mut events).await,
        TerminalEvent::Reconnected { generation: 2 }
    ));
    shell.close().await;
}

#[tokio::test]
async fn changed_host_key_gives_up_without_prompt_or_store_update() {
    let server = TestServer::start(Scenario::default()).await;
    let store = Arc::new(MemoryHostKeyStore::default());
    let prompter = Arc::new(TestPrompter::default());
    let (proxy, connector) = connect_via_proxy(&server, store.clone(), prompter.clone()).await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t3".into()),
            ..Default::default()
        },
        policy(&[Duration::from_millis(50)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    collect_until(shell.output(), b"[attach rs-t3 #1 80x24]").await;
    let pinned = store.lookup("127.0.0.1", proxy.address.port()).unwrap();
    let prompts = prompter.confirmations.lock().unwrap().len();
    server.rotate_host_key().await;
    proxy.cut();
    assert!(matches!(
        next_event(&mut events).await,
        TerminalEvent::Reconnecting { attempt: 1, .. }
    ));
    proxy.restore();
    let event = next_event(&mut events).await;
    assert!(
        matches!(
            event,
            TerminalEvent::GaveUp(SshError::HostKeyChanged { .. })
        ),
        "{event:?}"
    );
    assert_eq!(prompter.confirmations.lock().unwrap().len(), prompts);
    assert_eq!(
        store.lookup("127.0.0.1", proxy.address.port()).unwrap(),
        pinned
    );
    shell.close().await;
}

#[tokio::test]
async fn tmux_missing_emits_unavailable_and_never_reconnects() {
    let server = TestServer::start(Scenario {
        tmux_missing: true,
        ..Default::default()
    })
    .await;
    let (proxy, connector) = connect_via_proxy(
        &server,
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(TestPrompter::default()),
    )
    .await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t4".into()),
            ..Default::default()
        },
        policy(&[Duration::from_millis(50)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::TmuxUnavailable
    );
    collect_until(shell.output(), b"command not found").await;
    proxy.cut();
    let event = next_event(&mut events).await;
    assert!(matches!(event, TerminalEvent::GaveUp(_)), "{event:?}");
    shell.close().await;
}

#[tokio::test]
async fn plain_shell_does_not_reconnect() {
    let server = TestServer::start(Scenario::default()).await;
    let (proxy, connector) = connect_via_proxy(
        &server,
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(TestPrompter::default()),
    )
    .await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts::default(),
        policy(&[Duration::from_millis(50)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    // The task's parenthetical is explicit: a terminal opened without a tmux
    // id is marked non-reconnecting but does not emit TmuxUnavailable.
    // TmuxUnavailable is reserved for a requested tmux that fell back to 127.
    collect_until(shell.output(), b"fallback").await;
    proxy.cut();
    let event = next_event(&mut events).await;
    assert!(matches!(event, TerminalEvent::GaveUp(_)), "{event:?}");
    shell.close().await;
}

#[tokio::test]
async fn remote_exit_reports_closed_without_reconnecting() {
    let server = TestServer::start(Scenario {
        exit_immediately: true,
        ..Default::default()
    })
    .await;
    let (_proxy, connector) = connect_via_proxy(
        &server,
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(TestPrompter::default()),
    )
    .await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t5".into()),
            ..Default::default()
        },
        policy(&[Duration::from_millis(50)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    collect_until(shell.output(), b"[attach rs-t5 #1 80x24]").await;
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Closed { status: Some(0) }
    );
    assert_eq!(shell.write(b"after").await, Err(SshError::Closed));
    assert!(timeout(Duration::from_millis(300), events.recv())
        .await
        .is_err());
    shell.close().await;
}

#[tokio::test]
async fn reconnect_now_skips_long_backoff() {
    let server = TestServer::start(Scenario::default()).await;
    let (proxy, connector) = connect_via_proxy(
        &server,
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(TestPrompter::default()),
    )
    .await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t6".into()),
            ..Default::default()
        },
        policy(&[Duration::from_secs(30)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    collect_until(shell.output(), b"[attach rs-t6 #1 80x24]").await;
    proxy.cut();
    assert!(matches!(
        next_event(&mut events).await,
        TerminalEvent::Reconnecting { attempt: 1, .. }
    ));
    proxy.restore();
    let started = Instant::now();
    shell.reconnect_now();
    assert!(matches!(
        timeout(Duration::from_secs(2), events.recv())
            .await
            .unwrap()
            .unwrap(),
        TerminalEvent::Reconnected { generation: 2 }
    ));
    assert!(started.elapsed() < Duration::from_secs(2));
    shell.close().await;
}

#[tokio::test]
async fn give_up_deadline_then_write_fails() {
    let server = TestServer::start(Scenario::default()).await;
    let (proxy, connector) = connect_via_proxy(
        &server,
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(TestPrompter::default()),
    )
    .await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t7".into()),
            ..Default::default()
        },
        ReconnectPolicy {
            backoff: vec![Duration::from_millis(100)],
            give_up_after: Duration::from_millis(500),
        },
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    collect_until(shell.output(), b"[attach rs-t7 #1 80x24]").await;
    proxy.cut();
    loop {
        match next_event(&mut events).await {
            TerminalEvent::Reconnecting { .. } => {}
            TerminalEvent::GaveUp(error) => {
                assert!(is_recoverable(&error));
                break;
            }
            event => panic!("unexpected event {event:?}"),
        }
    }
    assert!(shell.write(b"after").await.is_err());
    shell.close().await;
}

#[tokio::test]
async fn close_during_reconnect_stops_events_and_server_connection() {
    let server = TestServer::start(Scenario::default()).await;
    let (proxy, connector) = connect_via_proxy(
        &server,
        Arc::new(MemoryHostKeyStore::default()),
        Arc::new(TestPrompter::default()),
    )
    .await;
    let mut shell = ResilientShell::open(
        connector,
        PtyOpts {
            tmux: Some("t8".into()),
            ..Default::default()
        },
        policy(&[Duration::from_secs(1)]),
    )
    .await
    .unwrap();
    let mut events = shell.events();
    assert_eq!(
        next_event(&mut events).await,
        TerminalEvent::Connected { generation: 1 }
    );
    collect_until(shell.output(), b"[attach rs-t8 #1 80x24]").await;
    assert_eq!(server.state.observed.lock().unwrap().open_connections, 1);
    proxy.cut();
    assert!(matches!(
        next_event(&mut events).await,
        TerminalEvent::Reconnecting { attempt: 1, .. }
    ));
    let started = Instant::now();
    shell.close().await;
    assert!(started.elapsed() < Duration::from_secs(1));
    assert!(matches!(
        timeout(Duration::from_secs(1), events.recv()).await,
        Err(_) | Ok(Err(tokio::sync::broadcast::error::RecvError::Closed))
    ));
    timeout(Duration::from_secs(1), async {
        while server.state.observed.lock().unwrap().open_connections != 0 {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("server connection must close");
    assert_eq!(server.state.observed.lock().unwrap().connections, 1);
}
