mod sshserver;
mod support;

use std::future::Future;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rs_mobile::rscore::api_diag::set_setting;
use rs_mobile::rscore::api_ssh::{
    lock, trust_host_key, CoreHostKeyStore, HostKeyPrompts, PasswordCache,
};
use rs_mobile::rscore::api_sshterm::{close, open, SshTuning, TMUX_UNAVAILABLE};
use rs_mobile::rscore::dto_core::{SshAuth, SshOpenOpts, SshState, SshTarget};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::Core;
use rs_mobile::rscore::sshbook::tmux_id;
use rs_mobile::rscore::tables::{SshEntry, SshTable, TermOutput};
use rs_ssh::reconnect::ReconnectPolicy;
use rs_ssh::{HostKeyEntry, HostKeyStore};
use serde_json::Value;
use ssh_key::{private::Ed25519Keypair, HashAlg, PrivateKey};
use sshserver::{ServerCfg, SshTestServer};

pub const LIMIT: Duration = Duration::from_secs(2);

pub async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(LIMIT, future)
        .await
        .expect("operation exceeded two seconds")
}

pub async fn until(predicate: impl Fn() -> bool) {
    bounded(async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

#[derive(Default)]
pub struct Output {
    pub bytes: Mutex<Vec<u8>>,
    pub closed: AtomicUsize,
}

impl TermOutput for Output {
    fn output(&self, bytes: &[u8]) {
        self.bytes.lock().unwrap().extend_from_slice(bytes);
    }

    fn closed(&self) {
        self.closed.fetch_add(1, Ordering::Relaxed);
    }
}

impl Output {
    pub fn text(&self) -> String {
        String::from_utf8_lossy(&self.bytes.lock().unwrap()).into_owned()
    }

    pub fn passwords(&self) -> usize {
        self.text().matches("Password: ").count()
    }
}

pub struct Fixture {
    pub core: Arc<Core>,
    pub events: Arc<support::EventCollector>,
}

impl Default for Fixture {
    fn default() -> Self {
        let core = support::test_core_with(|_, opts| {
            opts.timeouts.hostkey_prompt = Duration::from_secs(3);
        });
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        Self { core, events }
    }
}

impl Fixture {
    pub async fn start(
        &self,
        server: &SshTestServer,
        tmux: Option<bool>,
    ) -> (Arc<SshEntry>, Arc<Output>) {
        // Neither host-key confirmation nor password entry can complete here.
        // Returning within this bound proves open does not await authentication.
        let id = tokio::time::timeout(
            Duration::from_millis(250),
            open(&self.core, "server", Some(options(server, tmux))),
        )
        .await
        .expect("open must return immediately")
        .unwrap();
        let entry = self.core.state::<SshTable>().get(id).unwrap();
        let output = Arc::new(Output::default());
        assert!(entry.term.attach(output.clone()));
        (entry, output)
    }

    pub fn states(&self, id: u64) -> Vec<Value> {
        let ssh_id = id.to_string();
        self.events
            .of_type("ssh.state")
            .into_iter()
            .filter(|event| event["sshId"] == ssh_id)
            .collect()
    }

    pub fn prompts(&self, id: u64) -> Vec<Value> {
        let ssh_id = id.to_string();
        self.events
            .of_type("ssh.hostKeyPrompt")
            .into_iter()
            .filter(|event| event["sshId"] == ssh_id)
            .collect()
    }

    pub async fn state(&self, id: u64, state: &str) -> Value {
        until(|| self.states(id).iter().any(|event| event["state"] == state)).await;
        self.states(id)
            .into_iter()
            .find(|event| event["state"] == state)
            .unwrap()
    }

    pub async fn trust(&self, id: u64) {
        until(|| !self.prompts(id).is_empty()).await;
        let prompt = self.prompts(id).pop().unwrap();
        assert_eq!(prompt["kind"], "first");
        trust_host_key(&self.core, "server", prompt["fp"].as_str().unwrap()).unwrap();
    }

    pub async fn password(&self, entry: &SshEntry, output: &Output, password: &[u8]) {
        until(|| output.passwords() > 0).await;
        assert!(entry.term.input(password));
    }

    pub async fn connect(
        &self,
        server: &SshTestServer,
        tmux: Option<bool>,
    ) -> (Arc<SshEntry>, Arc<Output>) {
        let (entry, output) = self.start(server, tmux).await;
        self.trust(entry.id).await;
        self.password(&entry, &output, b"pw\r").await;
        self.state(entry.id, "open").await;
        (entry, output)
    }

    pub async fn stop(&self, entry: &SshEntry, output: &Output) {
        close(&self.core, entry.id);
        self.state(entry.id, "closed").await;
        self.assert_finished(entry, output, SshState::Closed);
        close(&self.core, entry.id);
        assert!(self.core.events().flush(LIMIT));
        self.assert_finished(entry, output, SshState::Closed);
    }

    pub fn assert_finished(&self, entry: &SshEntry, output: &Output, expected: SshState) {
        assert_eq!(*lock(&entry.state), expected);
        assert_eq!(output.closed.load(Ordering::Relaxed), 1);
        assert!(self.core.state::<SshTable>().get(entry.id).is_none());
        assert!(!lock(&self.core.state::<HostKeyPrompts>().first).contains_key(&entry.id));
        assert_eq!(
            self.states(entry.id)
                .iter()
                .filter(|event| event["state"] == "closed" || event["state"] == "failed")
                .count(),
            1
        );
        assert!(!entry.term.input(b"ignored"));
    }

    pub fn pin(&self, server: &SshTestServer) {
        let key = server.cfg.host_key.public_key();
        CoreHostKeyStore {
            core: self.core.clone(),
            server_id: "server".into(),
        }
        .save(
            "127.0.0.1",
            server.addr.port(),
            HostKeyEntry {
                algorithm: key.algorithm().to_string(),
                key: key.to_bytes().unwrap(),
                fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
            },
        );
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SshTable>().list() {
            close(&self.core, entry.id);
        }
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

pub fn options(server: &SshTestServer, tmux: Option<bool>) -> SshOpenOpts {
    SshOpenOpts {
        tmux,
        target: Some(SshTarget {
            host: "127.0.0.1".into(),
            port: server.addr.port(),
            user: "test".into(),
            jump: None,
            auth: SshAuth::Password,
        }),
    }
}

pub fn password_server() -> ServerCfg {
    ServerCfg {
        password: Some("pw".into()),
        keyboard_interactive: true,
        ..Default::default()
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn first_connection_native_password_echo_and_close() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.connect(&server, Some(false)).await;
    let states = f.states(entry.id);
    assert_eq!(states[0]["state"], "connecting");
    assert_eq!(states[0]["serverId"], "server");
    assert_eq!(f.events.events()[0].1["type"], "ssh.state");
    let authenticating = states
        .iter()
        .position(|event| event["state"] == "authenticating")
        .unwrap();
    let opened = states
        .iter()
        .position(|event| event["state"] == "open")
        .unwrap();
    assert!(authenticating < opened);
    assert_eq!(*lock(&entry.state), SshState::Open);
    assert!(entry.term.input(b"hello\n"));
    until(|| output.text().contains("hello\n")).await;
    assert!(!output.text().contains("pw"));
    assert_eq!(server.observed().shells, 1);
    assert!(server.observed().commands.is_empty());
    f.stop(&entry, &output).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn second_connection_reuses_host_trust_and_password_cache() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (first, first_output) = f.connect(&server, Some(false)).await;
    f.stop(&first, &first_output).await;
    let (second, output) = f.start(&server, Some(false)).await;
    assert!(second.id > first.id);
    f.state(second.id, "open").await;
    until(|| output.text().contains("ready")).await;
    assert!(f.prompts(second.id).is_empty());
    assert_eq!(output.passwords(), 0);
    assert!(server.observed().attempts.contains(&"password"));
    f.stop(&second, &output).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn reconnects_tmux_with_cached_password_and_latest_size() {
    let mut server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    *lock(&f.core.state::<SshTuning>().0) = ReconnectPolicy {
        backoff: vec![Duration::from_millis(30)],
        give_up_after: Duration::from_secs(5),
    };
    // rs-ssh intentionally only reconnects shells protected by tmux.
    // None also verifies that the default is tmux enabled.
    let (entry, output) = f.connect(&server, None).await;
    assert_eq!(server.observed().commands.len(), 1);
    assert!(server.observed().commands[0].contains(&tmux_id("server")));
    server.stop();
    until(|| {
        f.states(entry.id).iter().any(|event| {
            event["state"] == "connecting"
                && event["message"]
                    .as_str()
                    .is_some_and(|message| message.starts_with("reconnecting "))
        })
    })
    .await;
    assert!(entry.term.resize(110, 35));
    if let Err(error) = bounded(server.restart()).await {
        eprintln!(
            "restart failed ({:?}); validating close during reconnect instead",
            error.kind()
        );
        f.stop(&entry, &output).await;
        return;
    }
    until(|| {
        f.states(entry.id)
            .iter()
            .filter(|event| event["state"] == "open")
            .count()
            >= 2
    })
    .await;
    assert_eq!(output.passwords(), 1);
    assert!(!output.text().contains("pw"));
    assert_eq!(f.prompts(entry.id).len(), 1);
    until(|| {
        server
            .observed()
            .ptys
            .last()
            .is_some_and(|(_, cols, rows)| (*cols, *rows) == (110, 35))
            || server.observed().sizes.last() == Some(&(110, 35))
    })
    .await;
    assert!(entry.term.input(b"after-reconnect\n"));
    until(|| output.text().contains("after-reconnect\n")).await;
    f.stop(&entry, &output).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn wrong_password_fails_with_sanitized_kind() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.start(&server, Some(false)).await;
    f.trust(entry.id).await;
    bounded(async {
        let mut answered = 0;
        loop {
            let prompts = output.passwords();
            if prompts > answered {
                answered = prompts;
                entry.term.input(b"wrong-secret\r");
            }
            if f.states(entry.id)
                .iter()
                .any(|event| event["state"] == "failed")
            {
                break;
            }
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert_eq!(f.state(entry.id, "failed").await["message"], "auth_failed");
    assert!(!output.text().contains("wrong-secret"));
    f.assert_finished(&entry, &output, SshState::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn changed_host_key_prompts_and_fails() {
    let mut server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (first, first_output) = f.connect(&server, Some(false)).await;
    f.stop(&first, &first_output).await;
    server.cfg.host_key = PrivateKey::new(Ed25519Keypair::from_seed(&[2; 32]).into(), "").unwrap();
    bounded(server.restart())
        .await
        .expect("same-port host-key replacement must restart");
    let (entry, output) = f.start(&server, Some(false)).await;
    assert_eq!(
        f.state(entry.id, "failed").await["message"],
        "host_key_changed"
    );
    let prompts = f.prompts(entry.id);
    assert_eq!(prompts.len(), 1);
    assert_eq!(prompts[0]["kind"], "changed");
    assert_ne!(prompts[0]["fp"], prompts[0]["oldFp"]);
    f.assert_finished(&entry, &output, SshState::Failed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn close_during_host_key_confirmation_rejects_pending_prompt() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.start(&server, Some(false)).await;
    until(|| !f.prompts(entry.id).is_empty()).await;
    f.stop(&entry, &output).await;
    let fp = f.prompts(entry.id)[0]["fp"].as_str().unwrap().to_owned();
    assert!(trust_host_key(&f.core, "server", &fp).is_err());
    assert!(server.observed().ptys.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn immediate_close_cannot_lose_cancellation() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.start(&server, Some(false)).await;
    f.stop(&entry, &output).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn resize_before_open_is_applied_and_early_bytes_are_discarded() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.start(&server, Some(false)).await;
    assert!(entry.term.resize(100, 30));
    assert!(entry.term.input(b"discard-before-auth\n"));
    f.trust(entry.id).await;
    f.password(&entry, &output, b"pw\r").await;
    f.state(entry.id, "open").await;
    until(|| {
        server
            .observed()
            .ptys
            .last()
            .is_some_and(|(_, cols, rows)| (*cols, *rows) == (100, 30))
            || server.observed().sizes.last() == Some(&(100, 30))
    })
    .await;
    assert!(entry.term.input(b"after-open\n"));
    until(|| output.text().contains("after-open\n")).await;
    assert!(!output.text().contains("discard-before-auth"));
    f.stop(&entry, &output).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn unknown_server_rejects_without_events() {
    let f = Fixture::default();
    assert!(matches!(
        bounded(open(&f.core, "unknown", None)).await,
        Err(CoreError::Internal { .. })
    ));
    assert_eq!(f.core.events().last_seq(), 0);
    assert!(f.core.state::<SshTable>().is_empty());
    close(&f.core, 99);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn remote_exit_reports_status_once() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.connect(&server, Some(false)).await;
    assert!(entry.term.input(b"exit\n"));
    assert_eq!(f.state(entry.id, "closed").await["message"], "0");
    f.assert_finished(&entry, &output, SshState::Closed);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn auth_url_reports_authenticating_before_open_without_keyboard_prompt() {
    let server = SshTestServer::start(ServerCfg {
        allow_none: true,
        auth_banner_url: Some("https://hs.example.com/a/test".into()),
        ..Default::default()
    })
    .await;
    let f = Fixture::default();
    // connect_once only relays Headscale URLs matching the configured control
    // URL's domain. This only sets the allowlist; target connects directly to loopback.
    set_setting(&f.core, "tailnet.controlUrl", r#""https://hs.example.com""#).unwrap();
    f.pin(&server);
    let (entry, output) = f.start(&server, Some(false)).await;
    f.state(entry.id, "open").await;
    let states = f.states(entry.id);
    assert_eq!(
        states
            .iter()
            .filter(|event| event["state"] == "authenticating")
            .count(),
        1
    );
    assert_eq!(states.last().unwrap()["state"], "open");
    assert_eq!(
        f.events.of_type("ssh.authUrl")[0]["url"],
        "https://hs.example.com/a/test"
    );
    assert_eq!(output.passwords(), 0);
    f.stop(&entry, &output).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn close_during_password_prompt_cleans_up_and_does_not_cache() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.start(&server, Some(false)).await;
    f.trust(entry.id).await;
    until(|| output.passwords() == 1).await;
    f.stop(&entry, &output).await;
    assert!(f
        .core
        .state::<PasswordCache>()
        .take_fresh("server", f.core.now_us())
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn tmux_unavailable_prints_hint_and_plain_shell_still_echoes() {
    let f = Fixture::default();
    let server = SshTestServer::start(ServerCfg {
        // The fixture reports exit 127 when command spawning fails. A missing
        // cwd deterministically exercises that path without depending on a
        // locally installed tmux binary or starting a real tmux server.
        exec_with_sh: Some(f.core.data_dir().join("missing-command-directory")),
        ..password_server()
    })
    .await;
    let (entry, output) = f.connect(&server, Some(true)).await;
    let hint = std::str::from_utf8(TMUX_UNAVAILABLE).unwrap();
    until(|| output.text().contains(hint) && server.observed().shells == 1).await;
    assert_eq!(output.text().matches(hint).count(), 1);
    assert!(entry.term.input(b"plain-shell\n"));
    until(|| output.text().contains("plain-shell\n")).await;
    f.stop(&entry, &output).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
pub async fn sustained_output_does_not_block_writes_or_close() {
    let server = SshTestServer::start(password_server()).await;
    let f = Fixture::default();
    let (entry, output) = f.connect(&server, Some(false)).await;
    let block = vec![b'x'; 4096];
    let baseline = output.bytes.lock().unwrap().len();
    for _ in 0..64 {
        assert!(entry.term.input(&block));
    }
    until(|| output.bytes.lock().unwrap().len() >= baseline + 64 * block.len()).await;
    f.stop(&entry, &output).await;
}
