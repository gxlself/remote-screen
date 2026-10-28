mod sshserver;
mod support;

use std::future::Future;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rs_mobile::rscore::api_diag::set_setting;
use rs_mobile::rscore::api_ssh::*;
use rs_mobile::rscore::dto_core::{SshAuth, SshSource};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::Core;
use rs_mobile::rscore::sshbook::ResolvedTarget;
use rs_mobile::rscore::tables::{SshTable, TermInput, TermOutput, TermTap};
use rs_ssh::{
    Credential, HostKeyEntry, HostKeyStore, Prompter, PtyOpts, SshError, SshSession, SshTarget,
};
use serde_json::{json, Value};
use ssh_key::{private::Ed25519Keypair, HashAlg, PrivateKey};
use sshserver::{ServerCfg, SshTestServer};
use tokio::task::JoinHandle;
use zeroize::{Zeroize, Zeroizing};

const LIMIT: Duration = Duration::from_secs(3);

async fn bounded<T>(f: impl Future<Output = T>) -> T {
    tokio::time::timeout(LIMIT, f)
        .await
        .expect("operation exceeded test deadline")
}

async fn until(predicate: impl Fn() -> bool) {
    bounded(async {
        while !predicate() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
}

#[derive(Default)]
struct Output(Mutex<Vec<u8>>);
impl TermOutput for Output {
    fn output(&self, bytes: &[u8]) {
        self.0.lock().unwrap().extend_from_slice(bytes);
    }
    fn closed(&self) {}
}
impl Output {
    fn bytes(&self) -> Vec<u8> {
        self.0.lock().unwrap().clone()
    }
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    prompter: Arc<CorePrompter>,
    output: Arc<Output>,
    pump: JoinHandle<()>,
}
impl Fixture {
    fn new(ssh_id: Option<u64>, terminal: bool) -> Self {
        let core = support::test_core_with(|_, opts| {
            opts.timeouts.hostkey_prompt = Duration::from_secs(2);
        });
        support::with_identity(&core, "SSH test");
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        let output = Arc::new(Output::default());
        let (term, mut input) = TermTap::new();
        term.attach(output.clone());
        let entry = core.state::<SshTable>().insert("server", term);
        let relay = Arc::new(PromptRelay::default());
        let pump_relay = relay.clone();
        let pump = tokio::spawn(async move {
            while let Some(input) = input.recv().await {
                if let TermInput::Bytes(mut bytes) = input {
                    pump_relay.feed(&bytes);
                    bytes.zeroize();
                }
            }
        });
        let prompter = Arc::new(CorePrompter::new(
            core.clone(),
            ssh_id,
            "server".into(),
            "test".into(),
            terminal.then_some(entry),
            relay,
        ));
        Self {
            core,
            events,
            prompter,
            output,
            pump,
        }
    }

    fn store(&self) -> CoreHostKeyStore {
        CoreHostKeyStore {
            core: self.core.clone(),
            server_id: "server".into(),
        }
    }

    async fn event(&self, kind: &str) -> Value {
        until(|| !self.events.of_type(kind).is_empty()).await;
        self.events.of_type(kind).pop().unwrap()
    }

    async fn connect(&self, target: &ResolvedTarget) -> Result<SshSession, SshError> {
        bounded(connect_once(&self.core, target, self.prompter.clone())).await
    }
}
impl Drop for Fixture {
    fn drop(&mut self) {
        self.pump.abort();
        self.prompter.relay.cancel();
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

fn key(seed: u8) -> PrivateKey {
    PrivateKey::new(Ed25519Keypair::from_seed(&[seed; 32]).into(), "").unwrap()
}
fn pin(key: &PrivateKey) -> HostKeyEntry {
    let key = key.public_key();
    HostKeyEntry {
        algorithm: key.algorithm().to_string(),
        key: key.to_bytes().unwrap(),
        fingerprint: key.fingerprint(HashAlg::Sha256).to_string(),
    }
}
fn target(server: &SshTestServer, auth: SshAuth) -> ResolvedTarget {
    ResolvedTarget {
        server_id: "server".into(),
        auth,
        tailscale_ssh: None,
        target: SshTarget {
            host: "127.0.0.1".into(),
            port: server.addr.port(),
            user: "test".into(),
            jump: None,
        },
    }
}
fn pin_server(f: &Fixture, server: &SshTestServer) {
    f.store()
        .save("127.0.0.1", server.addr.port(), pin(&server.cfg.host_key));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_key_store_round_trip_and_forget() {
    let f = Fixture::new(Some(7), false);
    let entry = pin(&key(2));
    f.store().save("100.64.0.7", 22, entry.clone());
    assert_eq!(f.store().lookup("100.64.0.7", 22), Some(entry));
    let rows = f.core.store().known_hosts().unwrap();
    assert_eq!(rows[0].server_id, "server");
    assert_eq!(trusted_host_keys(&f.core).unwrap()[0].server_id, "server");
    f.core
        .store()
        .kv_set("ssh.servers.sync", "preserved configuration")
        .unwrap();
    f.core.state::<PasswordCache>().put(
        "server",
        vec![Zeroizing::new("pw".into())],
        f.core.now_us(),
    );
    lock(&f.core.state::<HostKeyPrompts>().changed).insert(
        "server".into(),
        vec![("100.64.0.7".into(), 22, "old".into(), "new".into())],
    );
    forget_host_key(&f.core, "server").unwrap();
    assert!(trusted_host_keys(&f.core).unwrap().is_empty());
    assert!(f
        .core
        .state::<PasswordCache>()
        .take_fresh("server", f.core.now_us())
        .is_none());
    assert!(lock(&f.core.state::<HostKeyPrompts>().changed).is_empty());
    assert_eq!(
        f.core
            .store()
            .kv_get("ssh.servers.sync")
            .unwrap()
            .as_deref(),
        Some("preserved configuration")
    );
    assert!(matches!(
        trust_host_key(&f.core, "server", "unknown"),
        Err(CoreError::Internal { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_echo_secret_cancel_and_timeout() {
    let relay = Arc::new(PromptRelay::default());
    assert!(!relay.feed(b"idle"));
    for echo in [true, false] {
        let (term, _input) = TermTap::new();
        let out = Arc::new(Output::default());
        term.attach(out.clone());
        let reader = {
            let relay = relay.clone();
            tokio::spawn(async move { relay.read_line(&term, ">", echo, LIMIT).await })
        };
        until(|| !out.bytes().is_empty()).await;
        assert!(relay.feed(b"ab\x7fc\r"));
        assert_eq!(
            bounded(reader)
                .await
                .unwrap()
                .as_deref()
                .map(String::as_str),
            Some("ac")
        );
        assert_eq!(
            out.bytes(),
            if echo {
                b">ab\x08 \x08c\r\n".to_vec()
            } else {
                b">\r\n".to_vec()
            }
        );
        assert!(!relay.feed(b"idle"));
    }
    for ctrl_c in [true, false] {
        let (term, _) = TermTap::new();
        let reader = {
            let relay = relay.clone();
            tokio::spawn(async move { relay.read_line(&term, "", false, LIMIT).await })
        };
        until(|| lock(&relay.pending).is_some()).await;
        if ctrl_c {
            assert!(relay.feed(b"\x03"));
        } else {
            relay.cancel();
        }
        assert!(bounded(reader).await.unwrap().is_none());
    }
    let (term, _) = TermTap::new();
    assert!(relay
        .read_line(&term, "", false, Duration::from_millis(10))
        .await
        .is_none());
    assert!(!relay.feed(b"idle"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn relay_utf8_limit_and_dropped_future() {
    let relay = Arc::new(PromptRelay::default());
    let (term, _) = TermTap::new();
    let reader = {
        let relay = relay.clone();
        tokio::spawn(async move { relay.read_line(&term, "", true, LIMIT).await })
    };
    until(|| lock(&relay.pending).is_some()).await;
    relay.feed(&[0xe4, 0xbd]);
    relay.feed(&[0xa0, 0x08, b'a']);
    relay.feed(&[b'b'; 1100]);
    relay.feed(b"\n");
    let line = bounded(reader).await.unwrap().unwrap();
    assert_eq!(line.len(), 1024);
    assert!(line.starts_with('a'));
    let (term, _) = TermTap::new();
    let reader = {
        let relay = relay.clone();
        tokio::spawn(async move { relay.read_line(&term, "", false, LIMIT).await })
    };
    until(|| lock(&relay.pending).is_some()).await;
    reader.abort();
    assert!(reader.await.unwrap_err().is_cancelled());
    assert!(!relay.feed(b"idle"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn first_host_key_terminal_password_and_reconnect_cache() {
    let f = Fixture::new(Some(7), true);
    let server = sshserver::start(ServerCfg {
        password: Some("pw".into()),
        keyboard_interactive: true,
        ..Default::default()
    })
    .await;
    let resolved = target(&server, SshAuth::Password);
    let connection = {
        let core = f.core.clone();
        let p = f.prompter.clone();
        let target = resolved.clone();
        tokio::spawn(async move { connect_once(&core, &target, p).await })
    };
    let event = f.event("ssh.hostKeyPrompt").await;
    assert_eq!(event["kind"], "first");
    assert_eq!(event["sshId"], "7");
    assert!(trust_host_key(&f.core, "wrong-server", event["fp"].as_str().unwrap()).is_err());
    assert!(trust_host_key(&f.core, "server", "wrong-fingerprint").is_err());
    trust_host_key(&f.core, "server", event["fp"].as_str().unwrap()).unwrap();
    until(|| f.output.bytes().ends_with(b"Password: ")).await;
    assert!(f.prompter.term.as_ref().unwrap().term.input(b"pw\r"));
    let session = bounded(connection).await.unwrap().unwrap();
    assert_eq!(trusted_host_keys(&f.core).unwrap()[0].fp, event["fp"]);
    let cached = f
        .core
        .state::<PasswordCache>()
        .take_fresh("server", f.core.now_us())
        .unwrap();
    assert_eq!(cached[0].as_str(), "pw");
    assert!(!f.output.bytes().windows(2).any(|bytes| bytes == b"pw"));
    session.close().await;
    f.connect(&resolved).await.unwrap().close().await;
    assert!(server.observed().attempts.contains(&"password"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejected_host_key_is_not_saved() {
    let f = Fixture::new(Some(7), true);
    let server = sshserver::start(ServerCfg::default()).await;
    let connection = {
        let core = f.core.clone();
        let p = f.prompter.clone();
        let target = target(&server, SshAuth::Password);
        tokio::spawn(async move { connect_once(&core, &target, p).await })
    };
    f.event("ssh.hostKeyPrompt").await;
    reject_prompts(&f.core, 7);
    assert!(matches!(
        bounded(connection).await.unwrap(),
        Err(SshError::HostKeyRejected)
    ));
    assert!(trusted_host_keys(&f.core).unwrap().is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_host_key_requires_explicit_trust_then_skips_first_prompt() {
    let f = Fixture::new(Some(7), false);
    let server = sshserver::start(ServerCfg {
        allow_none: true,
        ..Default::default()
    })
    .await;
    let old = pin(&key(2));
    f.store().save("127.0.0.1", server.addr.port(), old.clone());
    let resolved = target(&server, SshAuth::Tailscale);
    assert!(matches!(
        f.connect(&resolved).await,
        Err(SshError::HostKeyChanged { .. })
    ));
    let event = f.event("ssh.hostKeyPrompt").await;
    assert_eq!(event["kind"], "changed");
    assert_eq!(event["oldFp"], old.fingerprint);
    assert_eq!(event["keyType"], old.algorithm);
    trust_host_key(&f.core, "server", event["fp"].as_str().unwrap()).unwrap();
    assert!(f.store().lookup("127.0.0.1", server.addr.port()).is_none());
    f.connect(&resolved).await.unwrap().close().await;
    assert!(f.core.events().flush(LIMIT));
    assert_eq!(f.events.of_type("ssh.hostKeyPrompt").len(), 1);
    assert_eq!(
        f.store().lookup("127.0.0.1", server.addr.port()).unwrap(),
        pin(&server.cfg.host_key)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn background_unknown_host_is_rejected_without_event() {
    let f = Fixture::new(None, false);
    let server = sshserver::start(ServerCfg {
        allow_none: true,
        ..Default::default()
    })
    .await;
    assert!(matches!(
        f.connect(&target(&server, SshAuth::Tailscale)).await,
        Err(SshError::HostKeyRejected)
    ));
    assert!(f.core.events().flush(LIMIT));
    assert!(f.events.of_type("ssh.hostKeyPrompt").is_empty());
    f.prompter.auth_url("https://login.tailscale.com/a/xyz");
    assert!(f.core.events().flush(LIMIT));
    assert!(f.events.of_type("ssh.authUrl").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_key_authentication_uses_external_signer() {
    let f = Fixture::new(None, false);
    let key =
        rs_ssh::public_key_openssh(&Credential::External(f.core.signer().unwrap()), "t").unwrap();
    let public = ssh_key::PublicKey::from_openssh(&key).unwrap();
    let server = sshserver::start(ServerCfg {
        accept_keys: vec![public.clone()],
        ..Default::default()
    })
    .await;
    pin_server(&f, &server);
    f.connect(&target(&server, SshAuth::DeviceKey))
        .await
        .unwrap()
        .close()
        .await;
    assert!(server.observed().attempts.contains(&"publickey"));
    assert_eq!(server.observed().verified[0].key_data(), public.key_data());
    f.core.set_signer(None);
    assert!(credentials(&f.core, SshAuth::DeviceKey, "server").is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn tailscale_none_auth_relays_browser_url() {
    let f = Fixture::new(Some(7), false);
    let url = "https://login.tailscale.com/a/xyz";
    let server = sshserver::start(ServerCfg {
        allow_none: true,
        auth_banner_url: Some(url.into()),
        ..Default::default()
    })
    .await;
    pin_server(&f, &server);
    f.connect(&target(&server, SshAuth::Tailscale))
        .await
        .unwrap()
        .close()
        .await;
    let event = f.event("ssh.authUrl").await;
    assert_eq!(event["url"], url);
    assert_eq!(event["sshId"], "7");
    assert_eq!(event["serverId"], "server");
    assert_eq!(event["user"], "test");
    assert_eq!(server.observed().attempts, vec!["none"]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_url_hostname_allows_headscale_check_url() {
    let f = Fixture::new(Some(7), false);
    set_control_url(&f.core, "https://hs.example.com:8443/v1");
    let url = "https://hs.example.com/a/xyz";
    let server = sshserver::start(ServerCfg {
        allow_none: true,
        auth_banner_url: Some(url.into()),
        ..Default::default()
    })
    .await;
    pin_server(&f, &server);
    f.connect(&target(&server, SshAuth::Tailscale))
        .await
        .unwrap()
        .close()
        .await;
    assert_eq!(f.event("ssh.authUrl").await["url"], url);
    // An IP control URL adds no domain and does not break the connection.
    set_control_url(&f.core, "http://127.0.0.1:7300");
    f.connect(&target(&server, SshAuth::Tailscale))
        .await
        .unwrap()
        .close()
        .await;
}

fn set_control_url(core: &Arc<Core>, url: &str) {
    set_setting(core, "tailnet.controlUrl", &json!(url).to_string()).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn server_book_reads_the_persistent_local_copy() {
    let f = Fixture::new(None, false);
    set_setting(
        &f.core,
        "sync.servers",
        &json!({
            "v": 1, "entries": [{"kind": "ssh", "id": "manual", "name": "Manual",
                "host": "10.30.0.7", "port": 22, "user": "ops", "auth": "password"}]
        })
        .to_string(),
    )
    .unwrap();
    let first = bounded(servers(&f.core)).await.unwrap();
    assert_eq!(first.len(), 1);
    assert_eq!(first[0].source, SshSource::Manual);
    let resolved = bounded(resolve_server(&f.core, "manual", None))
        .await
        .unwrap();
    assert_eq!(resolved.target.host, "10.30.0.7");
    assert!(matches!(
        bounded(resolve_server(&f.core, "unknown", None)).await,
        Err(CoreError::Internal { .. })
    ));
    // Recreate Core to prove the copy is rs.db rather than process-local state.
    let cfg = f.core.config();
    let reopened = Core::new(
        cfg,
        rs_mobile::rscore::runtime::CoreOptions {
            init_logging: false,
            ..Default::default()
        },
    )
    .unwrap();
    assert_eq!(bounded(servers(&reopened)).await.unwrap(), first);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_cache_expires_and_only_first_keyboard_round_reuses_it() {
    let f = Fixture::new(None, false);
    let cache = f.core.state::<PasswordCache>();
    cache.put("server", vec![Zeroizing::new("pw".into())], 100);
    assert!(cache
        .take_fresh("server", 100 + PASSWORD_CACHE_TTL_US - 1)
        .is_some());
    assert!(cache
        .take_fresh("server", 100 + PASSWORD_CACHE_TTL_US)
        .is_none());
    cache.put("server", vec![Zeroizing::new("pw".into())], f.core.now_us());
    let prompts = vec![("Password: ".into(), false)];
    assert_eq!(
        f.prompter.keyboard_interactive("", "", &prompts).await,
        Some(vec!["pw".into()])
    );
    assert!(f
        .prompter
        .keyboard_interactive("", "", &prompts)
        .await
        .is_none());
    assert!(matches!(
        credentials(&f.core, SshAuth::Password, "server").as_slice(),
        [Credential::Password(_), Credential::KeyboardInteractive]
    ));
    cache.clear("server");
    assert!(matches!(
        credentials(&f.core, SshAuth::Password, "server").as_slice(),
        [Credential::KeyboardInteractive]
    ));
    cache.put("server", vec![Zeroizing::new("pw".into())], f.core.now_us());
    let different = Arc::new(CorePrompter::new(
        f.core.clone(),
        None,
        "server".into(),
        "test".into(),
        None,
        Arc::new(PromptRelay::default()),
    ));
    assert!(different
        .keyboard_interactive("", "", &[("a".into(), false), ("b".into(), false)])
        .await
        .is_none());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_key_prompt_timeout_and_cancellation_remove_pending_state() {
    let f = Fixture::new(Some(7), false);
    assert!(
        !bounded(
            f.prompter
                .confirm_new_host_key("100.64.0.7", "fp", "ssh-ed25519")
        )
        .await
    );
    assert!(lock(&f.core.state::<HostKeyPrompts>().first).is_empty());
    let p = f.prompter.clone();
    let prompt = tokio::spawn(async move {
        p.confirm_new_host_key("100.64.0.7", "fp", "ssh-ed25519")
            .await
    });
    until(|| !lock(&f.core.state::<HostKeyPrompts>().first).is_empty()).await;
    prompt.abort();
    assert!(prompt.await.unwrap_err().is_cancelled());
    assert!(lock(&f.core.state::<HostKeyPrompts>().first).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn jump_key_change_selects_the_pinned_jump_host() {
    let f = Fixture::new(Some(7), false);
    let jump = sshserver::start(ServerCfg {
        allow_none: true,
        host_key: key(3),
        ..Default::default()
    })
    .await;
    let server = sshserver::start(ServerCfg {
        allow_none: true,
        ..Default::default()
    })
    .await;
    pin_server(&f, &server);
    f.store().save("127.0.0.1", jump.addr.port(), pin(&key(4)));
    let mut resolved = target(&server, SshAuth::Tailscale);
    resolved.target.jump = Some(Box::new(target(&jump, SshAuth::Tailscale).target));
    assert!(matches!(
        f.connect(&resolved).await,
        Err(SshError::HostKeyChanged { .. })
    ));
    let event = f.event("ssh.hostKeyPrompt").await;
    trust_host_key(&f.core, "server", event["fp"].as_str().unwrap()).unwrap();
    assert!(f.store().lookup("127.0.0.1", jump.addr.port()).is_none());
    assert!(f.store().lookup("127.0.0.1", server.addr.port()).is_some());
    f.connect(&resolved).await.unwrap().close().await;
    assert_eq!(
        f.store().lookup("127.0.0.1", jump.addr.port()).unwrap(),
        pin(&jump.cfg.host_key)
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn shared_server_shell_exec_resize_stop_and_restart() {
    let f = Fixture::new(None, false);
    let home = support::temp_dir("ssh-exec");
    let mut server = sshserver::start(ServerCfg {
        allow_none: true,
        exec_with_sh: Some(home.clone()),
        ..Default::default()
    })
    .await;
    pin_server(&f, &server);
    let resolved = target(&server, SshAuth::Tailscale);
    let session = f.connect(&resolved).await.unwrap();
    let command = "printf shared > \"$HOME/check\"; printf stdout; printf stderr >&2; exit 7";
    let output = bounded(session.exec(command)).await.unwrap();
    assert_eq!(output.stdout, b"stdout");
    assert_eq!(output.stderr, b"stderr");
    assert_eq!(output.status, Some(7));
    assert_eq!(std::fs::read(home.join("check")).unwrap(), b"shared");
    let stdin = bounded(session.exec_with_stdin("/bin/sh -s", b"printf from-stdin"))
        .await
        .unwrap();
    assert_eq!(stdin.stdout, b"from-stdin");
    assert_eq!(stdin.status, Some(0));
    let mut shell = bounded(session.open_shell(PtyOpts::default()))
        .await
        .unwrap();
    assert_eq!(bounded(shell.output().recv()).await.unwrap(), b"ready\r\n");
    shell.write(b"hello\n").await.unwrap();
    assert_eq!(bounded(shell.output().recv()).await.unwrap(), b"hello\n");
    shell.resize(100, 40).await.unwrap();
    until(|| server.observed().sizes == vec![(100, 40)]).await;
    shell.write(b"exit\n").await.unwrap();
    assert_eq!(bounded(shell.closed()).await, Some(0));
    assert_eq!(server.observed().commands, vec![command, "/bin/sh -s"]);
    assert_eq!(
        server.observed().ptys,
        vec![("xterm-256color".into(), 80, 24)]
    );
    let addr = server.addr;
    server.stop();
    // A socket shutdown may be reported as either a clean close or a transport error.
    let _ = bounded(session.closed()).await;
    bounded(server.restart()).await.unwrap();
    assert_eq!(server.addr, addr);
    f.connect(&resolved).await.unwrap().close().await;
    server.stop();
    bounded(async {
        while !server.task.is_finished() {
            tokio::task::yield_now().await;
        }
    })
    .await;
    let occupied = tokio::net::TcpListener::bind(addr).await.unwrap();
    assert_eq!(
        server.restart().await.unwrap_err().kind(),
        std::io::ErrorKind::AddrInUse
    );
    drop(occupied);
    bounded(server.restart()).await.unwrap();
    f.connect(&resolved).await.unwrap().close().await;
    std::fs::remove_dir_all(home).unwrap();
}
