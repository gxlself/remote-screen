//! All network peers here are in-process russh servers on loopback ephemeral ports.
use std::{
    collections::{HashMap, HashSet},
    net::SocketAddr,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use async_trait::async_trait;
use p256::{
    ecdsa::{signature::Signer, Signature, SigningKey},
    pkcs8::EncodePublicKey,
};
use rs_ssh::{
    public_key_openssh, Credential, HostKeyStore, MemoryHostKeyStore, Prompter, PtyOpts, SshError,
    SshEvent, SshOptions, SshSession, SshState, SshTarget,
};
use rsp_transport::identity::{ExternalSigner, SignError};
use russh::{
    server::{self, Auth},
    Channel, ChannelId, MethodKind, MethodSet, Pty,
};
use ssh_key::{private::Ed25519Keypair, PrivateKey, PublicKey};
use tokio::{net::TcpListener, sync::Notify, time::timeout};
use x509_parser::prelude::FromDer;
use zeroize::Zeroizing;

fn private(seed: u8) -> PrivateKey {
    PrivateKey::new(Ed25519Keypair::from_seed(&[seed; 32]).into(), "").unwrap()
}
fn ed25519(seed: u8) -> Credential {
    Credential::Ed25519 {
        secret: Zeroizing::new([seed; 32]),
    }
}
fn password(value: &str) -> Credential {
    Credential::Password(Zeroizing::new(value.into()))
}
fn options() -> SshOptions {
    SshOptions {
        connect_timeout: Duration::from_secs(3),
        auth_timeout: Duration::from_secs(3),
        request_timeout: Duration::from_secs(3),
        ..Default::default()
    }
}

#[derive(Debug, PartialEq, Eq)]
struct PromptRequest {
    name: String,
    instructions: String,
    prompts: Vec<(String, bool)>,
}

#[derive(Default)]
struct TestPrompter {
    accept: bool,
    confirmations: Mutex<Vec<(String, String, String)>>,
    urls: Mutex<Vec<String>>,
    rounds: AtomicUsize,
    cancel: bool,
    wrong_count: bool,
    password_answer: Option<&'static str>,
    requests: Mutex<Vec<PromptRequest>>,
}
impl TestPrompter {
    fn accepting() -> Arc<Self> {
        Arc::new(Self {
            accept: true,
            ..Default::default()
        })
    }
}
#[async_trait]
impl Prompter for TestPrompter {
    async fn confirm_new_host_key(&self, host: &str, fp: &str, algo: &str) -> bool {
        self.confirmations
            .lock()
            .unwrap()
            .push((host.into(), fp.into(), algo.into()));
        self.accept
    }
    async fn keyboard_interactive(
        &self,
        name: &str,
        instructions: &str,
        prompts: &[(String, bool)],
    ) -> Option<Vec<String>> {
        self.requests.lock().unwrap().push(PromptRequest {
            name: name.into(),
            instructions: instructions.into(),
            prompts: prompts.to_vec(),
        });
        if self.cancel {
            return None;
        }
        if self.wrong_count {
            return Some(vec![]);
        }
        if let Some(password) = self.password_answer {
            assert!(name.is_empty());
            assert!(instructions.is_empty());
            assert_eq!(prompts, &[("Password:".into(), false)]);
            return Some(vec![password.into()]);
        }
        let round = self.rounds.fetch_add(1, Ordering::SeqCst);
        assert_eq!(name, "verification");
        assert_eq!(instructions, "answer the test prompt");
        assert_eq!(prompts, &[(format!("round {}", round + 1), round == 0)]);
        Some(vec![format!("answer{}", round + 1)])
    }
    fn auth_url(&self, url: &str) {
        self.urls.lock().unwrap().push(url.into());
    }
}

struct SoftwareSigner {
    key: SigningKey,
    calls: AtomicUsize,
    fail: bool,
    creator: std::thread::ThreadId,
}
impl SoftwareSigner {
    fn new() -> Self {
        Self {
            key: SigningKey::from_bytes((&[42u8; 32]).into()).unwrap(),
            calls: AtomicUsize::new(0),
            fail: false,
            creator: std::thread::current().id(),
        }
    }
}
impl ExternalSigner for SoftwareSigner {
    fn sign(&self, msg: &[u8]) -> Result<Vec<u8>, SignError> {
        // spawn_blocking must keep slow device work off a tokio async worker.
        assert_ne!(std::thread::current().id(), self.creator);
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.fail {
            return Err(SignError("test failure".into()));
        }
        let signature: Signature = self.key.sign(msg);
        Ok(signature.to_der().as_bytes().to_vec())
    }
    fn spki_der(&self) -> Vec<u8> {
        self.key
            .verifying_key()
            .to_public_key_der()
            .unwrap()
            .as_bytes()
            .to_vec()
    }
}

#[derive(Default)]
struct Observed {
    attempts: Vec<&'static str>,
    verified: Vec<PublicKey>,
    ptys: Vec<(String, u32, u32)>,
    sizes: Vec<(u32, u32)>,
    commands: Vec<String>,
    shells: usize,
    forwards: Vec<(String, u32)>,
    rounds: usize,
    produced: usize,
}
#[derive(Clone)]
struct Scenario {
    host_seed: u8,
    accepted_key: Option<PublicKey>,
    accepted_password: Option<&'static str>,
    keyboard: bool,
    methods: Vec<MethodKind>,
    banner: Option<String>,
    none_success: bool,
    auth_delay: Duration,
    tmux_missing: bool,
    reject_pty: bool,
    reject_exec: bool,
    stall_on_data: bool,
    bulk: bool,
    no_status: bool,
    observed: Arc<Mutex<Observed>>,
    changed: Arc<Notify>,
    stalled: Arc<Notify>,
}
impl Default for Scenario {
    fn default() -> Self {
        Self {
            host_seed: 1,
            accepted_key: None,
            accepted_password: Some("correct"),
            keyboard: false,
            methods: vec![
                MethodKind::PublicKey,
                MethodKind::Password,
                MethodKind::KeyboardInteractive,
            ],
            banner: None,
            none_success: false,
            auth_delay: Duration::ZERO,
            tmux_missing: false,
            reject_pty: false,
            reject_exec: false,
            stall_on_data: false,
            bulk: false,
            no_status: false,
            observed: Arc::new(Mutex::new(Observed::default())),
            changed: Arc::new(Notify::new()),
            stalled: Arc::new(Notify::new()),
        }
    }
}
struct TestServer {
    address: SocketAddr,
    scenario: Scenario,
    host_key: Arc<Mutex<PrivateKey>>,
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
        Self::on_listener(listener, scenario)
    }
    fn on_listener(listener: TcpListener, scenario: Scenario) -> Self {
        let address = listener.local_addr().unwrap();
        let host_key = Arc::new(Mutex::new(private(scenario.host_seed)));
        let task_host_key = host_key.clone();
        let task_scenario = scenario.clone();
        let task = tokio::spawn(async move {
            let mut peers = tokio::task::JoinSet::new();
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let Ok((stream, _)) = accepted else { break; };
                        let config = Arc::new(server::Config {
                            keys: vec![task_host_key.lock().unwrap().clone()],
                            methods: MethodSet::from(task_scenario.methods.as_slice()),
                            auth_rejection_time: Duration::from_millis(1), auth_rejection_time_initial: Some(Duration::from_millis(1)),
                            max_auth_attempts: 20, channel_buffer_size: 2, event_buffer_size: 4, nodelay: true,
                            ..Default::default()
                        });
                        let handler = TestHandler { scenario: task_scenario.clone(), channels: HashMap::new(), forwarded: HashSet::new(), round: 0 };
                        peers.spawn(async move {
                            if let Ok(peer) = server::run_stream(config, stream, handler).await { let _ = peer.await; }
                        });
                    }
                    _ = peers.join_next(), if !peers.is_empty() => {}
                }
            }
        });
        Self {
            address,
            scenario,
            host_key,
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
    async fn connect(
        &self,
        creds: Vec<Credential>,
        store: Arc<MemoryHostKeyStore>,
        prompter: Arc<TestPrompter>,
    ) -> Result<SshSession, SshError> {
        SshSession::connect(self.target(), creds, store, prompter, options()).await
    }
}

struct TestHandler {
    scenario: Scenario,
    channels: HashMap<ChannelId, Channel<server::Msg>>,
    forwarded: HashSet<ChannelId>,
    round: usize,
}
impl server::Handler for TestHandler {
    type Error = russh::Error;
    async fn auth_none(&mut self, _: &str) -> Result<Auth, Self::Error> {
        if self.scenario.none_success {
            tokio::time::sleep(self.scenario.auth_delay).await;
            Ok(Auth::Accept)
        } else {
            Ok(Auth::reject())
        }
    }
    async fn authentication_banner(&mut self) -> Result<Option<String>, Self::Error> {
        Ok(self.scenario.banner.clone())
    }
    async fn auth_password(&mut self, _: &str, password: &str) -> Result<Auth, Self::Error> {
        self.scenario
            .observed
            .lock()
            .unwrap()
            .attempts
            .push("password");
        tokio::time::sleep(self.scenario.auth_delay).await;
        Ok(if self.scenario.accepted_password == Some(password) {
            Auth::Accept
        } else {
            Auth::Reject {
                proceed_with_methods: Some(MethodSet::from(self.scenario.methods.as_slice())),
                partial_success: false,
            }
        })
    }
    async fn auth_publickey_offered(
        &mut self,
        _: &str,
        key: &PublicKey,
    ) -> Result<Auth, Self::Error> {
        self.scenario
            .observed
            .lock()
            .unwrap()
            .attempts
            .push("publickey");
        Ok(
            if self.scenario.accepted_key.as_ref().map(PublicKey::key_data) == Some(key.key_data())
            {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }
    async fn auth_publickey(&mut self, _: &str, key: &PublicKey) -> Result<Auth, Self::Error> {
        // russh invokes this only AFTER validating the SSH signature.
        self.scenario
            .observed
            .lock()
            .unwrap()
            .verified
            .push(key.clone());
        Ok(
            if self.scenario.accepted_key.as_ref().map(PublicKey::key_data) == Some(key.key_data())
            {
                Auth::Accept
            } else {
                Auth::reject()
            },
        )
    }
    async fn auth_keyboard_interactive(
        &mut self,
        _: &str,
        _: &str,
        response: Option<server::Response<'_>>,
    ) -> Result<Auth, Self::Error> {
        if !self.scenario.keyboard {
            return Ok(Auth::reject());
        }
        if let Some(response) = response {
            let answers: Vec<_> = response.map(|a| a.to_vec()).collect();
            assert_eq!(answers, vec![format!("answer{}", self.round).into_bytes()]);
            self.scenario.observed.lock().unwrap().rounds += 1;
            if self.round == 2 {
                return Ok(Auth::Accept);
            }
        } else {
            self.scenario
                .observed
                .lock()
                .unwrap()
                .attempts
                .push("keyboard-interactive");
        }
        self.round += 1;
        Ok(Auth::Partial {
            name: "verification".into(),
            instructions: "answer the test prompt".into(),
            prompts: vec![(format!("round {}", self.round).into(), self.round == 1)].into(),
        })
    }
    async fn channel_open_session(
        &mut self,
        channel: Channel<server::Msg>,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.scenario.bulk {
            self.channels.insert(channel.id(), channel);
        }
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
        self.scenario
            .observed
            .lock()
            .unwrap()
            .ptys
            .push((term.into(), cols, rows));
        if self.scenario.reject_pty {
            session.channel_failure(channel)
        } else {
            session.channel_success(channel)
        }
    }
    async fn window_change_request(
        &mut self,
        _: ChannelId,
        cols: u32,
        rows: u32,
        _: u32,
        _: u32,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.scenario
            .observed
            .lock()
            .unwrap()
            .sizes
            .push((cols, rows));
        self.scenario.changed.notify_one();
        Ok(())
    }
    async fn shell_request(
        &mut self,
        channel: ChannelId,
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.scenario.observed.lock().unwrap().shells += 1;
        session.channel_success(channel)?;
        session.data(channel, &b"ready\r\n"[..])?;
        if self.scenario.bulk {
            let channel = self.channels.remove(&channel).unwrap();
            let observed = self.scenario.observed.clone();
            tokio::spawn(async move {
                for index in 0..512usize {
                    let block: Vec<_> = (0..16 * 1024)
                        .map(|i| ((index * 16 * 1024 + i) % 251) as u8)
                        .collect();
                    channel.data(&block[..]).await.unwrap();
                    observed.lock().unwrap().produced += 1;
                }
                channel.eof().await.unwrap();
                channel.exit_status(0).await.unwrap();
                channel.close().await.unwrap();
            });
        }
        Ok(())
    }
    async fn data(
        &mut self,
        channel: ChannelId,
        data: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        if self.forwarded.contains(&channel) {
            return Ok(());
        }
        if self.scenario.stall_on_data {
            self.scenario.stalled.notify_one();
            std::future::pending::<()>().await;
        }
        if data == b"exit" {
            session.eof(channel)?;
            if !self.scenario.no_status {
                session.exit_status_request(channel, 23)?;
            }
            session.close(channel)?;
        } else {
            session.data(channel, data.to_vec())?;
        }
        Ok(())
    }
    async fn exec_request(
        &mut self,
        channel: ChannelId,
        cmd: &[u8],
        session: &mut server::Session,
    ) -> Result<(), Self::Error> {
        self.scenario
            .observed
            .lock()
            .unwrap()
            .commands
            .push(String::from_utf8_lossy(cmd).into_owned());
        if self.scenario.reject_exec {
            return session.channel_failure(channel);
        }
        session.channel_success(channel)?;
        if cmd.starts_with(b"tmux ") {
            if self.scenario.tmux_missing {
                session.extended_data(channel, 1, &b"tmux: command not found\r\n"[..])?;
                session.exit_status_request(channel, 127)?;
                session.eof(channel)?;
                session.close(channel)?;
            } else {
                session.data(channel, &b"tmux-ready\r\n"[..])?;
            }
        } else {
            session.data(channel, &b"stdout\0bytes"[..])?;
            session.extended_data(channel, 1, &b"stderr"[..])?;
            // Exercise EOF before exit-status (common for OpenSSH).
            session.eof(channel)?;
            session.exit_status_request(channel, 7)?;
            session.close(channel)?;
        }
        Ok(())
    }
    #[allow(clippy::too_many_arguments)]
    async fn channel_open_direct_tcpip(
        &mut self,
        channel: Channel<server::Msg>,
        host: &str,
        port: u32,
        _: &str,
        _: u32,
        reply: server::ChannelOpenHandle,
        _: &mut server::Session,
    ) -> Result<(), Self::Error> {
        assert_eq!(host, "127.0.0.1");
        self.forwarded.insert(channel.id());
        self.scenario
            .observed
            .lock()
            .unwrap()
            .forwards
            .push((host.into(), port));
        let mut stream = tokio::net::TcpStream::connect((host, port as u16)).await?;
        reply.accept().await;
        tokio::spawn(async move {
            let mut channel = channel.into_stream();
            let _ = tokio::io::copy_bidirectional(&mut channel, &mut stream).await;
        });
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

#[tokio::test]
async fn ed25519_auth_and_exec() {
    let server = TestServer::start(Scenario {
        accepted_key: Some(private(7).public_key().clone()),
        ..Default::default()
    })
    .await;
    let session = server
        .connect(
            vec![ed25519(7)],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .unwrap();
    assert_eq!(session.state(), SshState::Connected);
    assert_eq!(
        server.scenario.observed.lock().unwrap().verified,
        vec![private(7).public_key().clone()]
    );
    let out = session.exec("test-command").await.unwrap();
    assert_eq!(out.status, Some(7));
    assert_eq!(out.stdout, b"stdout\0bytes");
    assert_eq!(out.stderr, b"stderr");
    session.close().await;
}

#[tokio::test]
async fn external_p256_signature_is_verified_by_server() {
    let signer = Arc::new(SoftwareSigner::new());
    let credential = Credential::External(signer.clone());
    let key = PublicKey::from_openssh(&public_key_openssh(&credential, "").unwrap()).unwrap();
    let server = TestServer::start(Scenario {
        accepted_key: Some(key.clone()),
        ..Default::default()
    })
    .await;
    let session = server
        .connect(
            vec![credential],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .unwrap();
    assert_eq!(signer.calls.load(Ordering::SeqCst), 1);
    assert_eq!(server.scenario.observed.lock().unwrap().verified, vec![key]);
    session.close().await;
}

#[tokio::test]
async fn password_success_failure_and_ordered_fallback() {
    let server = TestServer::start(Scenario::default()).await;
    let store = Arc::new(MemoryHostKeyStore::default());
    let prompter = TestPrompter::accepting();
    let error = server
        .connect(vec![password("incorrect")], store.clone(), prompter.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(
        error,
        SshError::AuthFailed {
            tried: vec!["password"]
        }
    );
    server.scenario.observed.lock().unwrap().attempts.clear();
    let session = server
        .connect(
            vec![ed25519(8), password("correct")],
            store.clone(),
            prompter.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["publickey", "password"]
    );
    session.close().await;
    // Two credentials for the same method must still be tried in order.
    let session = server
        .connect(
            vec![password("incorrect"), password("correct")],
            store,
            prompter,
        )
        .await
        .unwrap();
    session.close().await;
}

#[tokio::test]
async fn unsupported_server_methods_are_skipped() {
    let server = TestServer::start(Scenario {
        methods: vec![MethodKind::Password],
        ..Default::default()
    })
    .await;
    let session = server
        .connect(
            vec![
                ed25519(8),
                Credential::KeyboardInteractive,
                password("correct"),
            ],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .unwrap();
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["password"]
    );
    session.close().await;
}

#[tokio::test]
async fn password_only_server_uses_interactive_password_prompt() {
    let server = TestServer::start(Scenario {
        methods: vec![MethodKind::Password],
        ..Default::default()
    })
    .await;
    let prompter = Arc::new(TestPrompter {
        accept: true,
        password_answer: Some("correct"),
        ..Default::default()
    });
    let session = server
        .connect(
            vec![Credential::KeyboardInteractive],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .unwrap();
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["password"]
    );
    assert_eq!(
        *prompter.requests.lock().unwrap(),
        vec![PromptRequest {
            name: String::new(),
            instructions: String::new(),
            prompts: vec![("Password:".into(), false)],
        }]
    );
    session.close().await;
}

#[tokio::test]
async fn password_only_prompt_can_be_cancelled() {
    let server = TestServer::start(Scenario {
        methods: vec![MethodKind::Password],
        ..Default::default()
    })
    .await;
    let prompter = Arc::new(TestPrompter {
        accept: true,
        cancel: true,
        ..Default::default()
    });
    let error = server
        .connect(
            vec![Credential::KeyboardInteractive],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error, SshError::Cancelled);
    assert_eq!(prompter.requests.lock().unwrap().len(), 1);
    assert!(server.scenario.observed.lock().unwrap().attempts.is_empty());
}

#[tokio::test]
async fn password_only_wrong_answer_is_tried_once() {
    let server = TestServer::start(Scenario {
        methods: vec![MethodKind::Password],
        ..Default::default()
    })
    .await;
    let prompter = Arc::new(TestPrompter {
        accept: true,
        password_answer: Some("incorrect"),
        ..Default::default()
    });
    let error = server
        .connect(
            vec![Credential::KeyboardInteractive],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        error,
        SshError::AuthFailed {
            tried: vec!["password"],
        }
    );
    assert_eq!(prompter.requests.lock().unwrap().len(), 1);
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["password"]
    );
}

#[tokio::test]
async fn password_only_prompt_requires_one_answer() {
    let server = TestServer::start(Scenario {
        methods: vec![MethodKind::Password],
        ..Default::default()
    })
    .await;
    let prompter = Arc::new(TestPrompter {
        accept: true,
        wrong_count: true,
        ..Default::default()
    });
    let error = server
        .connect(
            vec![Credential::KeyboardInteractive],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        error,
        SshError::InvalidOptions("keyboard-interactive answer count does not match prompts")
    );
    assert_eq!(prompter.requests.lock().unwrap().len(), 1);
    assert!(server.scenario.observed.lock().unwrap().attempts.is_empty());
}

#[tokio::test]
async fn password_only_server_does_not_prompt_for_key_only_credentials() {
    let server = TestServer::start(Scenario {
        methods: vec![MethodKind::Password],
        ..Default::default()
    })
    .await;
    let prompter = TestPrompter::accepting();
    let error = server
        .connect(
            vec![ed25519(8)],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error, SshError::AuthFailed { tried: vec![] });
    assert!(prompter.requests.lock().unwrap().is_empty());
    assert!(server.scenario.observed.lock().unwrap().attempts.is_empty());
}

#[tokio::test]
async fn password_prompt_is_deferred_until_after_later_keys() {
    let server = TestServer::start(Scenario {
        methods: vec![MethodKind::PublicKey, MethodKind::Password],
        accepted_key: Some(private(7).public_key().clone()),
        ..Default::default()
    })
    .await;
    let prompter = TestPrompter::accepting();
    let session = server
        .connect(
            vec![Credential::KeyboardInteractive, ed25519(7)],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .unwrap();
    assert!(prompter.requests.lock().unwrap().is_empty());
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["publickey"]
    );
    session.close().await;
}

#[tokio::test]
async fn rejected_keyboard_interactive_does_not_prompt_for_password() {
    let server = TestServer::start(Scenario::default()).await;
    let prompter = TestPrompter::accepting();
    let error = server
        .connect(
            vec![Credential::KeyboardInteractive],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(
        error,
        SshError::AuthFailed {
            tried: vec!["keyboard-interactive"],
        }
    );
    assert!(prompter.requests.lock().unwrap().is_empty());
    assert!(server.scenario.observed.lock().unwrap().attempts.is_empty());
}

#[tokio::test]
async fn advertised_keyboard_interactive_uses_only_server_prompts() {
    let server = TestServer::start(Scenario {
        keyboard: true,
        ..Default::default()
    })
    .await;
    let prompter = TestPrompter::accepting();
    let session = server
        .connect(
            vec![Credential::KeyboardInteractive],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .unwrap();
    assert_eq!(prompter.requests.lock().unwrap().len(), 2);
    assert_eq!(prompter.rounds.load(Ordering::SeqCst), 2);
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["keyboard-interactive"]
    );
    session.close().await;
}

#[tokio::test]
async fn keyboard_interactive_two_rounds() {
    let server = TestServer::start(Scenario {
        keyboard: true,
        ..Default::default()
    })
    .await;
    let prompter = TestPrompter::accepting();
    let session = server
        .connect(
            vec![Credential::KeyboardInteractive],
            Arc::new(MemoryHostKeyStore::default()),
            prompter.clone(),
        )
        .await
        .unwrap();
    assert_eq!(prompter.rounds.load(Ordering::SeqCst), 2);
    assert_eq!(server.scenario.observed.lock().unwrap().rounds, 2);
    session.close().await;
}

#[tokio::test]
async fn keyboard_interactive_cancel_and_answer_count() {
    let server = TestServer::start(Scenario {
        keyboard: true,
        ..Default::default()
    })
    .await;
    for (cancel, wrong_count, expected) in [
        (true, false, SshError::Cancelled),
        (
            false,
            true,
            SshError::InvalidOptions("keyboard-interactive answer count does not match prompts"),
        ),
    ] {
        let prompter = Arc::new(TestPrompter {
            accept: true,
            cancel,
            wrong_count,
            ..Default::default()
        });
        let error = server
            .connect(
                vec![Credential::KeyboardInteractive],
                Arc::new(MemoryHostKeyStore::default()),
                prompter,
            )
            .await
            .err()
            .unwrap();
        assert_eq!(error, expected);
    }
}

#[tokio::test]
async fn known_hosts_first_confirmation_refusal_and_matching_pin() {
    let server = TestServer::start(Scenario::default()).await;
    let store = Arc::new(MemoryHostKeyStore::default());
    let rejecting = Arc::new(TestPrompter::default());
    assert_eq!(
        server
            .connect(vec![password("correct")], store.clone(), rejecting.clone())
            .await
            .err()
            .unwrap(),
        SshError::HostKeyRejected
    );
    assert!(store.lookup("127.0.0.1", server.address.port()).is_none());
    assert!(server.scenario.observed.lock().unwrap().attempts.is_empty());
    let prompter = TestPrompter::accepting();
    let session = server
        .connect(vec![password("correct")], store.clone(), prompter.clone())
        .await
        .unwrap();
    let entry = store.lookup("127.0.0.1", server.address.port()).unwrap();
    let key = private(1);
    assert_eq!(entry.key, key.public_key().to_bytes().unwrap());
    assert_eq!(
        entry.fingerprint,
        key.public_key()
            .fingerprint(ssh_key::HashAlg::Sha256)
            .to_string()
    );
    assert!(entry.fingerprint.starts_with("SHA256:") && !entry.fingerprint.ends_with('='));
    assert_eq!(
        prompter.confirmations.lock().unwrap().as_slice(),
        &[(
            "127.0.0.1".into(),
            entry.fingerprint.clone(),
            "ssh-ed25519".into()
        )]
    );
    session.close().await;
    let session = server
        .connect(vec![password("correct")], store.clone(), prompter.clone())
        .await
        .unwrap();
    assert_eq!(prompter.confirmations.lock().unwrap().len(), 1);
    assert_eq!(
        store.lookup("127.0.0.1", server.address.port()),
        Some(entry)
    );
    session.close().await;
}

#[tokio::test]
async fn changed_host_key_is_blocked_without_prompt_or_overwrite() {
    let server = TestServer::start(Scenario::default()).await;
    let store = Arc::new(MemoryHostKeyStore::default());
    let prompter = TestPrompter::accepting();
    let session = server
        .connect(vec![password("correct")], store.clone(), prompter.clone())
        .await
        .unwrap();
    session.close().await;
    let target = server.target();
    let old = store.lookup(&target.host, target.port).unwrap();
    // Rotate the in-process server key while retaining its ephemeral listener.
    *server.host_key.lock().unwrap() = private(2);
    let error = server
        .connect(vec![password("correct")], store.clone(), prompter.clone())
        .await
        .err()
        .unwrap();
    assert_eq!(
        error,
        SshError::HostKeyChanged {
            expected: old.fingerprint.clone(),
            got: private(2)
                .public_key()
                .fingerprint(ssh_key::HashAlg::Sha256)
                .to_string()
        }
    );
    assert_eq!(store.lookup(&target.host, target.port), Some(old));
    assert_eq!(prompter.confirmations.lock().unwrap().len(), 1);
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["password"]
    );
}

#[tokio::test]
async fn pty_echo_resize_and_exit_status_after_eof() {
    let server = TestServer::start(Scenario::default()).await;
    let session = server
        .connect(
            vec![password("correct")],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .unwrap();
    let mut shell = session.open_shell(PtyOpts::default()).await.unwrap();
    assert_eq!(shell.output().recv().await.unwrap(), b"ready\r\n");
    let input = b"\0\x1b[31mhello\r\n\xff";
    shell.write(input).await.unwrap();
    assert_eq!(shell.output().recv().await.unwrap(), input);
    shell.resize(132, 43).await.unwrap();
    timeout(Duration::from_secs(1), server.scenario.changed.notified())
        .await
        .unwrap();
    assert_eq!(
        server.scenario.observed.lock().unwrap().sizes,
        vec![(132, 43)]
    );
    assert_eq!(
        server.scenario.observed.lock().unwrap().ptys,
        vec![("xterm-256color".into(), 80, 24)]
    );
    shell.write(b"exit").await.unwrap();
    assert_eq!(shell.closed().await, Some(23));
    session.close().await;
}

#[tokio::test]
async fn pty_eight_mebibytes_are_lossless_and_backpressure_producer() {
    let server = TestServer::start(Scenario {
        bulk: true,
        ..Default::default()
    })
    .await;
    let session = SshSession::connect(
        server.target(),
        vec![password("correct")],
        Arc::new(MemoryHostKeyStore::default()),
        TestPrompter::accepting(),
        SshOptions {
            output_buffer: 1,
            ..options()
        },
    )
    .await
    .unwrap();
    let mut shell = session.open_shell(PtyOpts::default()).await.unwrap();
    assert_eq!(shell.output().recv().await.unwrap(), b"ready\r\n");
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert!(
        server.scenario.observed.lock().unwrap().produced < 512,
        "producer must stop when bounded queues fill"
    );
    let length = timeout(Duration::from_secs(20), async {
        let mut length = 0;
        while let Some(bytes) = shell.output().recv().await {
            for byte in bytes {
                assert_eq!(byte, (length % 251) as u8);
                length += 1;
            }
        }
        length
    })
    .await
    .unwrap();
    assert_eq!(length, 8 * 1024 * 1024);
    assert_eq!(shell.closed().await, Some(0));
    session.close().await;
}

#[tokio::test]
async fn keepalive_unresponsive_peer_reports_timeout() {
    let server = TestServer::start(Scenario {
        stall_on_data: true,
        ..Default::default()
    })
    .await;
    let session = SshSession::connect(
        server.target(),
        vec![password("correct")],
        Arc::new(MemoryHostKeyStore::default()),
        TestPrompter::accepting(),
        SshOptions {
            keepalive_interval: Duration::from_millis(100),
            keepalive_max: 3,
            ..options()
        },
    )
    .await
    .unwrap();
    let mut shell = session.open_shell(PtyOpts::default()).await.unwrap();
    shell.output().recv().await.unwrap();
    shell.write(b"stall").await.unwrap();
    timeout(Duration::from_secs(1), server.scenario.stalled.notified())
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), session.closed())
            .await
            .unwrap(),
        Err(SshError::Timeout)
    );
    assert_eq!(session.state(), SshState::Failed(SshError::Timeout));
    assert_eq!(shell.result().await, Err(SshError::Timeout));
    assert_eq!(shell.write(b"after-timeout").await, Err(SshError::Timeout));
    session.close().await;
}

#[tokio::test]
async fn proxy_jump_authenticates_and_pins_both_legs() {
    let jump = TestServer::start(Scenario {
        host_seed: 3,
        accepted_password: Some("jump"),
        ..Default::default()
    })
    .await;
    let target = TestServer::start(Scenario {
        host_seed: 4,
        accepted_key: Some(private(9).public_key().clone()),
        accepted_password: None,
        ..Default::default()
    })
    .await;
    let mut address = target.target();
    address.jump = Some(Box::new(jump.target()));
    let store = Arc::new(MemoryHostKeyStore::default());
    let prompter = TestPrompter::accepting();
    let session = SshSession::connect(
        address,
        vec![ed25519(9)],
        store.clone(),
        prompter.clone(),
        SshOptions {
            jump_credentials: Some(vec![password("jump")]),
            ..options()
        },
    )
    .await
    .unwrap();
    assert!(store.lookup("127.0.0.1", jump.address.port()).is_some());
    assert!(store.lookup("127.0.0.1", target.address.port()).is_some());
    assert_eq!(prompter.confirmations.lock().unwrap().len(), 2);
    assert_eq!(
        jump.scenario.observed.lock().unwrap().attempts,
        vec!["password"]
    );
    assert_eq!(target.scenario.observed.lock().unwrap().verified.len(), 1);
    assert_eq!(
        jump.scenario.observed.lock().unwrap().forwards,
        vec![("127.0.0.1".into(), u32::from(target.address.port()))]
    );
    assert_eq!(session.exec("through-jump").await.unwrap().status, Some(7));
    session.close().await;
}

#[tokio::test]
async fn nested_proxy_jump_is_rejected_before_connecting() {
    let server = TestServer::start(Scenario::default()).await;
    let mut target = server.target();
    let mut jump = server.target();
    jump.jump = Some(Box::new(server.target()));
    target.jump = Some(Box::new(jump));
    assert_eq!(
        SshSession::connect(
            target,
            vec![],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
            options()
        )
        .await
        .err()
        .unwrap(),
        SshError::Unsupported("only one ProxyJump hop is supported")
    );
}

#[tokio::test]
async fn tmux_command_and_invalid_ids() {
    let server = TestServer::start(Scenario::default()).await;
    let session = server
        .connect(
            vec![password("correct")],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .unwrap();
    for id in [
        "",
        "a;id",
        "$(id)",
        "a\nb",
        "a b",
        "'",
        "é",
        "abcdefghijklmnopqrstuvwxyz1234567",
    ] {
        assert!(matches!(
            session
                .open_shell(PtyOpts {
                    tmux: Some(id.into()),
                    ..Default::default()
                })
                .await,
            Err(SshError::InvalidOptions(_))
        ));
    }
    let mut shell = session
        .open_shell(PtyOpts {
            tmux: Some("abc_-123".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    assert_eq!(shell.output().recv().await.unwrap(), b"tmux-ready\r\n");
    assert_eq!(
        server.scenario.observed.lock().unwrap().commands,
        vec!["tmux new -A -s rs-abc_-123"]
    );
    shell.write(b"exit").await.unwrap();
    assert_eq!(shell.closed().await, Some(23));
    session.close().await;
}

#[tokio::test]
async fn tmux_127_reopens_plain_pty_and_emits_event() {
    let server = TestServer::start(Scenario {
        tmux_missing: true,
        ..Default::default()
    })
    .await;
    let session = server
        .connect(
            vec![password("correct")],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .unwrap();
    let mut events = session.events();
    let mut shell = session
        .open_shell(PtyOpts {
            tmux: Some("persist".into()),
            ..Default::default()
        })
        .await
        .unwrap();
    let ready = timeout(Duration::from_secs(3), async {
        let mut output = Vec::new();
        while !output.ends_with(b"ready\r\n") {
            output.extend(shell.output().recv().await.unwrap());
        }
        output
    })
    .await
    .unwrap();
    assert!(ready.starts_with(b"tmux: command not found"));
    assert_eq!(events.recv().await.unwrap(), SshEvent::TmuxUnavailable);
    assert_eq!(server.scenario.observed.lock().unwrap().ptys.len(), 2);
    assert_eq!(server.scenario.observed.lock().unwrap().shells, 1);
    shell.resize(90, 30).await.unwrap();
    shell.write(b"fallback").await.unwrap();
    assert_eq!(shell.output().recv().await.unwrap(), b"fallback");
    shell.write(b"exit").await.unwrap();
    assert_eq!(shell.closed().await, Some(23));
    session.close().await;
}

#[tokio::test]
async fn tailscale_check_banner_waits_for_auth_and_filters_urls() {
    let server = TestServer::start(Scenario {
        banner: Some(concat!(
            "Visit https://login.tailscale.com/a/xxxx to approve. ",
            "(https://hs.example.com/check?id=abc) ",
            "https://evil.example.com/a https://login.tailscale.com.evil.example.com/a ",
            "http://login.tailscale.com/a https://evil.example.com/?next=https://login.tailscale.com/a ",
            "https://user@login.tailscale.com/a https://login.tailscale.com@evil.example.com/a"
        ).into()),
        none_success: true, auth_delay: Duration::from_millis(150), ..Default::default()
    }).await;
    let prompter = TestPrompter::accepting();
    let session = SshSession::connect(
        server.target(),
        vec![password("correct")],
        Arc::new(MemoryHostKeyStore::default()),
        prompter.clone(),
        SshOptions {
            headscale_domains: vec!["hs.example.com".into()],
            ..options()
        },
    )
    .await
    .unwrap();
    assert_eq!(
        &*prompter.urls.lock().unwrap(),
        &[
            "https://login.tailscale.com/a/xxxx",
            "https://hs.example.com/check?id=abc",
        ]
    );
    assert_eq!(session.state(), SshState::Connected);
    session.close().await;
}

#[tokio::test]
async fn authentication_total_timeout_includes_browser_approval() {
    let server = TestServer::start(Scenario {
        banner: Some("https://login.tailscale.com/a/xxxx".into()),
        none_success: true,
        auth_delay: Duration::from_secs(1),
        ..Default::default()
    })
    .await;
    let prompter = TestPrompter::accepting();
    let error = SshSession::connect(
        server.target(),
        vec![],
        Arc::new(MemoryHostKeyStore::default()),
        prompter.clone(),
        SshOptions {
            auth_timeout: Duration::from_millis(100),
            ..options()
        },
    )
    .await
    .err()
    .unwrap();
    assert_eq!(error, SshError::Timeout);
    assert_eq!(
        &*prompter.urls.lock().unwrap(),
        &["https://login.tailscale.com/a/xxxx"]
    );
}

#[tokio::test]
async fn public_keys_roundtrip_and_rsa_user_key_is_unsupported() {
    let software = SoftwareSigner::new();
    for credential in [ed25519(7), Credential::External(Arc::new(software))] {
        let line = public_key_openssh(&credential, "device@example.com").unwrap();
        let key = PublicKey::from_openssh(&line).unwrap();
        assert_eq!(key.comment().as_str().unwrap(), "device@example.com");
        let expected = match credential {
            Credential::Ed25519 { .. } => private(7).public_key().key_data().clone(),
            Credential::External(ref signer) => {
                let der = signer.spki_der();
                let (_, spki) = x509_parser::prelude::SubjectPublicKeyInfo::from_der(&der).unwrap();
                ssh_key::public::EcdsaPublicKey::from_sec1_bytes(&spki.subject_public_key.data)
                    .unwrap()
                    .into()
            }
            _ => unreachable!(),
        };
        assert_eq!(key.key_data(), &expected);
    }
    assert!(matches!(
        public_key_openssh(&password("secret"), "comment"),
        Err(SshError::Unsupported(_))
    ));
    let imported =
        Credential::from_openssh(&private(7).to_openssh(ssh_key::LineEnding::LF).unwrap()).unwrap();
    assert_eq!(
        public_key_openssh(&imported, "").unwrap(),
        public_key_openssh(&ed25519(7), "").unwrap()
    );
    // Tiny, deliberately non-cryptographic RSA components exercise format import
    // without enabling RSA code or adding an RSA dependency.
    let pem = unsupported_rsa()
        .to_openssh(ssh_key::LineEnding::LF)
        .unwrap();
    assert_eq!(
        Credential::from_openssh(&pem).err().unwrap(),
        SshError::Unsupported("RSA user keys are not supported")
    );
    assert!(!format!("{:?}", password("do-not-log-this")).contains("do-not-log-this"));
    assert!(format!("{:?}", ed25519(7)).contains("REDACTED"));
}

#[tokio::test]
async fn pty_and_exec_request_rejections_are_reported() {
    for scenario in [
        Scenario {
            reject_pty: true,
            ..Default::default()
        },
        Scenario {
            reject_exec: true,
            ..Default::default()
        },
    ] {
        let server = TestServer::start(scenario.clone()).await;
        let session = server
            .connect(
                vec![password("correct")],
                Arc::new(MemoryHostKeyStore::default()),
                TestPrompter::accepting(),
            )
            .await
            .unwrap();
        if scenario.reject_pty {
            assert!(matches!(
                session.open_shell(PtyOpts::default()).await,
                Err(SshError::RequestRejected)
            ));
        } else {
            assert_eq!(
                session.exec("rejected").await,
                Err(SshError::RequestRejected)
            );
        }
        session.close().await;
    }
}

#[tokio::test]
async fn shell_close_without_status_returns_none() {
    let server = TestServer::start(Scenario {
        no_status: true,
        ..Default::default()
    })
    .await;
    let session = server
        .connect(
            vec![password("correct")],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .unwrap();
    let mut shell = session.open_shell(PtyOpts::default()).await.unwrap();
    shell.output().recv().await.unwrap();
    shell.write(b"exit").await.unwrap();
    assert_eq!(shell.result().await, Ok(None));
    session.close().await;
}

fn unsupported_rsa() -> PrivateKey {
    let mp = |v: u8| ssh_key::Mpint::from_bytes(&[v]).unwrap();
    PrivateKey::new(
        ssh_key::private::RsaKeypair::new(
            ssh_key::public::RsaPublicKey::new(mp(3), mp(15)).unwrap(),
            ssh_key::private::RsaPrivateKey::new(mp(3), mp(2), mp(3), mp(5)).unwrap(),
        )
        .unwrap()
        .into(),
        "",
    )
    .unwrap()
}

#[tokio::test]
async fn rsa_host_key_is_unsupported_without_enabling_rsa() {
    let server = TestServer::start(Scenario::default()).await;
    *server.host_key.lock().unwrap() = unsupported_rsa();
    let store = Arc::new(MemoryHostKeyStore::default());
    let prompter = TestPrompter::accepting();
    assert_eq!(
        server
            .connect(vec![password("correct")], store.clone(), prompter.clone())
            .await
            .err()
            .unwrap(),
        SshError::Unsupported("RSA host keys are not supported")
    );
    assert!(store.lookup("127.0.0.1", server.address.port()).is_none());
    assert!(prompter.confirmations.lock().unwrap().is_empty());
}

#[tokio::test]
async fn external_signer_error_is_reported_without_falling_back() {
    let mut signer = SoftwareSigner::new();
    signer.fail = true;
    let credential = Credential::External(Arc::new(signer));
    let key = PublicKey::from_openssh(&public_key_openssh(&credential, "").unwrap()).unwrap();
    let server = TestServer::start(Scenario {
        accepted_key: Some(key),
        ..Default::default()
    })
    .await;
    let error = server
        .connect(
            vec![credential, password("correct")],
            Arc::new(MemoryHostKeyStore::default()),
            TestPrompter::accepting(),
        )
        .await
        .err()
        .unwrap();
    assert_eq!(error, SshError::SigningFailed);
    assert!(server.scenario.observed.lock().unwrap().verified.is_empty());
    assert_eq!(
        server.scenario.observed.lock().unwrap().attempts,
        vec!["publickey"]
    );
}

#[tokio::test]
async fn proxy_jump_changed_key_blocks_each_leg_and_preserves_pins() {
    let jump = TestServer::start(Scenario::default()).await;
    let target = TestServer::start(Scenario {
        host_seed: 4,
        ..Default::default()
    })
    .await;
    let mut address = target.target();
    address.jump = Some(Box::new(jump.target()));
    let store = Arc::new(MemoryHostKeyStore::default());
    let prompter = TestPrompter::accepting();
    let connect = || {
        SshSession::connect(
            address.clone(),
            vec![password("correct")],
            store.clone(),
            prompter.clone(),
            options(),
        )
    };
    connect().await.unwrap().close().await;
    let jump_pin = store.lookup("127.0.0.1", jump.address.port()).unwrap();
    let target_pin = store.lookup("127.0.0.1", target.address.port()).unwrap();
    *target.host_key.lock().unwrap() = private(5);
    assert!(
        matches!(connect().await, Err(SshError::HostKeyChanged { expected, .. }) if expected == target_pin.fingerprint)
    );
    *target.host_key.lock().unwrap() = private(4);
    *jump.host_key.lock().unwrap() = private(6);
    let forwarded = jump.scenario.observed.lock().unwrap().forwards.len();
    assert!(
        matches!(connect().await, Err(SshError::HostKeyChanged { expected, .. }) if expected == jump_pin.fingerprint)
    );
    assert_eq!(
        jump.scenario.observed.lock().unwrap().forwards.len(),
        forwarded
    );
    assert_eq!(
        store.lookup("127.0.0.1", jump.address.port()),
        Some(jump_pin)
    );
    assert_eq!(
        store.lookup("127.0.0.1", target.address.port()),
        Some(target_pin)
    );
    assert_eq!(prompter.confirmations.lock().unwrap().len(), 2);
}

#[tokio::test]
async fn invalid_options_are_rejected_before_network_or_prompts() {
    let server = TestServer::start(Scenario::default()).await;
    for opts in [
        SshOptions {
            output_buffer: 0,
            ..options()
        },
        SshOptions {
            keepalive_max: 0,
            ..options()
        },
        SshOptions {
            auth_timeout: Duration::ZERO,
            ..options()
        },
        SshOptions {
            headscale_domains: vec!["https://hs.example.com".into()],
            ..options()
        },
        SshOptions {
            headscale_domains: vec!["*.example.com".into()],
            ..options()
        },
    ] {
        assert!(matches!(
            SshSession::connect(
                server.target(),
                vec![],
                Arc::new(MemoryHostKeyStore::default()),
                TestPrompter::accepting(),
                opts
            )
            .await,
            Err(SshError::InvalidOptions(_))
        ));
    }
}
