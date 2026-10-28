mod sshserver;
mod support;

use std::{future::Future, path::PathBuf, sync::Arc, time::Duration};

use rs_mobile::rscore::{
    api_diag::set_setting,
    api_ssh::{
        connect_once, credentials, lock, CoreHostKeyStore, CorePrompter, PasswordCache,
        PromptRelay, SshBookCache,
    },
    api_sshkeys::{install_key, keys},
    dto_core::{SshAuth, SshServer, SshSource},
    error::CoreError,
    runtime::Core,
    sshbook::ResolvedTarget,
    tables::{SshTable, TermTap},
};
use rs_ssh::{Credential, HostKeyEntry, HostKeyStore, SshTarget};
use serde_json::json;
use ssh_key::HashAlg;
use sshserver::{ServerCfg, SshTestServer};
use zeroize::Zeroizing;

const LIMIT: Duration = Duration::from_secs(8);
const SERVER_ID: &str = "book-server";

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(LIMIT, future)
        .await
        .expect("SSH auth fallback test exceeded its deadline")
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    home: PathBuf,
}

impl Fixture {
    fn new() -> Self {
        let core = support::test_core_with(|_, opts| {
            opts.timeouts.hostkey_prompt = Duration::from_secs(2);
        });
        support::with_identity(&core, "SSH fallback test");
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        let home = core.data_dir().join("ssh-home");
        std::fs::create_dir_all(&home).unwrap();
        Self { core, events, home }
    }

    fn pin_server(&self, server: &SshTestServer) {
        let public = server.cfg.host_key.public_key();
        CoreHostKeyStore {
            core: self.core.clone(),
            server_id: SERVER_ID.into(),
        }
        .save(
            "127.0.0.1",
            server.addr.port(),
            HostKeyEntry {
                algorithm: public.algorithm().to_string(),
                key: public.to_bytes().unwrap(),
                fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
            },
        );
    }

    fn cache_server(&self, server: &SshTestServer) {
        *lock(&self.core.state::<SshBookCache>().0) = vec![SshServer {
            server_id: SERVER_ID.into(),
            name: "Book server".into(),
            host: "127.0.0.1".into(),
            port: server.addr.port(),
            user: "ops".into(),
            jump: None,
            auth: SshAuth::DeviceKey,
            tailscale_ssh: None,
            os: None,
            source: SshSource::Manual,
        }];
        self.pin_server(server);
    }

    fn cache_password(&self) {
        self.core.state::<PasswordCache>().put(
            SERVER_ID,
            vec![Zeroizing::new("pw".into())],
            self.core.now_us(),
        );
    }

    fn assert_installed(&self) {
        let device_key = keys(&self.core).unwrap().remove(0);
        let authorized = std::fs::read_to_string(self.home.join(".ssh/authorized_keys")).unwrap();
        assert_eq!(
            authorized.lines().collect::<Vec<_>>(),
            [device_key.public_key]
        );
        assert_eq!(device_key.installed_on, [SERVER_ID]);
    }

    async fn connect_for_auth_url(&self, server: &SshTestServer) {
        self.pin_server(server);
        let prompter = Arc::new(CorePrompter::new(
            self.core.clone(),
            Some(7),
            SERVER_ID.into(),
            "ops".into(),
            None,
            Arc::new(PromptRelay::default()),
        ));
        let session = bounded(connect_once(
            &self.core,
            &target(server, SshAuth::Tailscale),
            prompter,
        ))
        .await
        .unwrap();
        session.close().await;
        assert!(self.core.events().flush(LIMIT));
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

fn target(server: &SshTestServer, auth: SshAuth) -> ResolvedTarget {
    ResolvedTarget {
        server_id: SERVER_ID.into(),
        auth,
        tailscale_ssh: None,
        target: SshTarget {
            host: "127.0.0.1".into(),
            port: server.addr.port(),
            user: "ops".into(),
            jump: None,
        },
    }
}

fn password_server() -> ServerCfg {
    ServerCfg {
        password: Some("pw".into()),
        keyboard_interactive: true,
        ..Default::default()
    }
}

#[test]
fn device_key_credentials_end_with_interactive_auth_only_when_initialized() {
    let f = Fixture::new();
    let creds = credentials(&f.core, SshAuth::DeviceKey, SERVER_ID);
    assert!(matches!(creds.first(), Some(Credential::External(_))));
    assert!(matches!(
        creds.last(),
        Some(Credential::KeyboardInteractive)
    ));

    f.core.set_signer(None);
    assert!(credentials(&f.core, SshAuth::DeviceKey, SERVER_ID).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_key_install_uses_the_cached_password() {
    let f = Fixture::new();
    let server = SshTestServer::start(ServerCfg {
        exec_with_sh: Some(f.home.clone()),
        ..password_server()
    })
    .await;
    f.cache_server(&server);
    f.cache_password();

    bounded(install_key(&f.core, SERVER_ID, "device"))
        .await
        .unwrap();
    f.assert_installed();
    assert_eq!(
        server.observed().attempts,
        ["none", "publickey", "keyboard-interactive"]
    );
    assert!(server.observed().verified.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_key_install_without_a_cached_password_points_to_the_terminal() {
    let f = Fixture::new();
    let server = SshTestServer::start(password_server()).await;
    f.cache_server(&server);

    let result = bounded(install_key(&f.core, SERVER_ID, "device")).await;
    match result {
        Err(CoreError::Internal { message }) => {
            assert_eq!(
                message,
                "authentication failed; open a terminal to this server first"
            );
        }
        other => panic!("expected terminal guidance, got {other:?}"),
    }
    assert_eq!(
        server.observed().attempts,
        ["none", "publickey", "keyboard-interactive"]
    );
    assert!(server.observed().commands.is_empty());
    assert!(!f.home.join(".ssh/authorized_keys").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_password_login_enables_key_installation() {
    let f = Fixture::new();
    let server = SshTestServer::start(ServerCfg {
        exec_with_sh: Some(f.home.clone()),
        ..password_server()
    })
    .await;
    f.cache_server(&server);
    let relay = Arc::new(PromptRelay::default());
    let (term, _input) = TermTap::new();
    let entry = f.core.state::<SshTable>().insert(SERVER_ID, term);
    let prompter = Arc::new(CorePrompter::new(
        f.core.clone(),
        Some(entry.id),
        SERVER_ID.into(),
        "ops".into(),
        Some(entry),
        relay.clone(),
    ));
    let core = f.core.clone();
    let resolved = target(&server, SshAuth::DeviceKey);
    let connection = tokio::spawn(async move { connect_once(&core, &resolved, prompter).await });
    bounded(async {
        while lock(&relay.pending).is_none() {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await;
    assert!(relay.feed(b"pw\r"));
    let session = bounded(connection).await.unwrap().unwrap();
    session.close().await;
    assert!(f
        .core
        .state::<PasswordCache>()
        .take_fresh(SERVER_ID, f.core.now_us())
        .is_some());

    bounded(install_key(&f.core, SERVER_ID, "device"))
        .await
        .unwrap();
    f.assert_installed();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn headscale_control_url_allows_its_check_url() {
    let f = Fixture::new();
    set_setting(
        &f.core,
        "tailnet.controlUrl",
        &json!("https://hs.example.com").to_string(),
    )
    .unwrap();
    let url = "https://hs.example.com/a/xyz";
    let server = SshTestServer::start(ServerCfg {
        allow_none: true,
        auth_banner_url: Some(url.into()),
        ..Default::default()
    })
    .await;

    f.connect_for_auth_url(&server).await;
    let events = f.events.of_type("ssh.authUrl");
    assert_eq!(events.len(), 1, "Headscale check URL was not relayed");
    assert_eq!(events[0]["url"], url);
    assert_eq!(events[0]["sshId"], "7");
    assert_eq!(events[0]["serverId"], SERVER_ID);
    assert_eq!(events[0]["user"], "ops");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn only_the_control_dns_name_allows_check_urls() {
    let f = Fixture::new();
    set_setting(
        &f.core,
        "tailnet.controlUrl",
        &json!("https://HS.Example.com:8443").to_string(),
    )
    .unwrap();
    let urls = [
        "https://old-control.example.com/a/control",
        "https://hs.example.com/a/control",
    ];
    let server = SshTestServer::start(ServerCfg {
        allow_none: true,
        auth_banner_url: Some(urls.join("\n")),
        ..Default::default()
    })
    .await;

    f.connect_for_auth_url(&server).await;
    // Only the current control URL’s domain is allowed.
    let events = f.events.of_type("ssh.authUrl");
    assert_eq!(events.len(), 1);
    assert_eq!(events[0]["url"], urls[1]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn control_url_ip_does_not_allow_ip_auth_urls() {
    let f = Fixture::new();
    set_setting(
        &f.core,
        "tailnet.controlUrl",
        &json!("https://100.64.0.7").to_string(),
    )
    .unwrap();
    let server = SshTestServer::start(ServerCfg {
        allow_none: true,
        auth_banner_url: Some("https://100.64.0.7/a/xyz".into()),
        ..Default::default()
    })
    .await;

    f.connect_for_auth_url(&server).await;
    assert!(f.events.of_type("ssh.authUrl").is_empty());
}
