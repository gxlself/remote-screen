mod sshserver;
mod support;

use std::collections::BTreeSet;
use std::future::Future;
use std::sync::Arc;
use std::time::Duration;

use rs_mobile::rscore::api_diag::set_setting;
use rs_mobile::rscore::api_ssh::{CoreHostKeyStore, PasswordCache, SshBookCache};
use rs_mobile::rscore::api_sshkeys::{create_key, install_key, keys};
use rs_mobile::rscore::dto_core::{SshAuth, SshKeyKind, SshServer, SshSource};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::Core;
use rs_ssh::{HostKeyEntry, HostKeyStore};
use serde_json::{json, Value};
use ssh_key::HashAlg;
use sshserver::{ServerCfg, SshTestServer};
use support::TestSigner;
use zeroize::Zeroizing;

const LIMIT: Duration = Duration::from_secs(8);
const SERVER_ID: &str = "manual";

async fn bounded<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(LIMIT, future)
        .await
        .expect("SSH key test exceeded its deadline")
}

fn manual_server(host: &str, port: u16, auth: &str) -> Value {
    json!({
        "v": 1,
        "entries": [{
            "kind": "ssh",
            "id": SERVER_ID,
            "name": "Manual server",
            "host": host,
            "port": port,
            "user": "ops",
            "auth": auth
        }]
    })
}

/// The App pushes its server list through `sync.servers`; `servers()` reads it back.
fn push_server(core: &Arc<Core>, server: &SshTestServer, auth: &str) {
    set_setting(
        core,
        "sync.servers",
        &manual_server("127.0.0.1", server.addr.port(), auth).to_string(),
    )
    .unwrap();
}

fn cache_server(core: &Arc<Core>, server: &SshTestServer, auth: SshAuth) {
    *core.state::<SshBookCache>().0.lock().unwrap() = vec![SshServer {
        server_id: SERVER_ID.to_owned(),
        name: "Manual server".to_owned(),
        host: "127.0.0.1".to_owned(),
        port: server.addr.port(),
        user: "ops".to_owned(),
        jump: None,
        auth,
        tailscale_ssh: None,
        os: None,
        source: SshSource::Manual,
    }];
}

fn pin_server(core: &Arc<Core>, server: &SshTestServer) {
    let public = server.cfg.host_key.public_key();
    let entry = HostKeyEntry {
        algorithm: public.algorithm().to_string(),
        key: public.to_bytes().unwrap(),
        fingerprint: public.fingerprint(HashAlg::Sha256).to_string(),
    };
    CoreHostKeyStore {
        core: core.clone(),
        server_id: SERVER_ID.to_owned(),
    }
    .save("127.0.0.1", server.addr.port(), entry);
}

fn cache_password(core: &Arc<Core>) {
    core.state::<PasswordCache>().put(
        SERVER_ID,
        vec![Zeroizing::new("pw".to_owned())],
        core.now_us(),
    );
}

fn assert_internal_contains<T: std::fmt::Debug>(result: Result<T, CoreError>, needle: &str) {
    match result {
        Err(CoreError::Internal { message }) => assert!(
            message.contains(needle),
            "unexpected internal error: {message}"
        ),
        other => panic!("expected Internal containing {needle:?}, got {other:?}"),
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn no_identity_has_no_keys_and_cannot_create_a_device_key() {
    let core = support::test_core();
    assert!(keys(&core).unwrap().is_empty());
    assert_internal_contains(create_key(&core, "se-p256", "x"), "identity not created");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn device_key_dto_matches_the_shared_contract() {
    let core = support::test_core();
    support::with_identity(&core, "Alice's iPhone");
    let device_id = core.device_id().unwrap();

    let keys = keys(&core).unwrap();
    assert_eq!(keys.len(), 1);
    let key = &keys[0];
    assert_eq!(key.key_id, "device");
    assert_eq!(key.name, "Alice's iPhone");
    assert_eq!(key.kind, SshKeyKind::SeP256);
    assert_eq!(
        key.created_at,
        core.store().identity().unwrap().unwrap().created_at_us
    );
    assert!(key.public_key.starts_with("ecdsa-sha2-nistp256 "));
    assert!(key.public_key.ends_with(&format!(" rs-device={device_id}")));
    assert!(key.fp.starts_with("SHA256:"));
    assert!(key.installed_on.is_empty());

    let value = serde_json::to_value(key).unwrap();
    let actual: BTreeSet<_> = value
        .as_object()
        .unwrap()
        .keys()
        .map(String::as_str)
        .collect();
    let expected = BTreeSet::from([
        "createdAt",
        "fp",
        "installedOn",
        "keyId",
        "kind",
        "name",
        "publicKey",
    ]);
    assert_eq!(actual, expected);
    assert_eq!(value["kind"], "se-p256");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn create_key_names_the_device_key_and_rejects_ed25519() {
    let core = support::test_core();
    support::with_identity(&core, "old name");

    let renamed = create_key(&core, "se-p256", "  小明的 iPhone  ").unwrap();
    assert_eq!(renamed.key_id, "device");
    assert_eq!(renamed.name, "小明的 iPhone");
    assert_eq!(keys(&core).unwrap()[0].name, "小明的 iPhone");

    let unchanged = create_key(&core, "se-p256", "   ").unwrap();
    assert_eq!(unchanged.name, "小明的 iPhone");
    assert!(matches!(
        create_key(&core, "ed25519", "k"),
        Err(CoreError::Unsupported { .. })
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn a_changed_signer_rebuilds_the_key_and_clears_installations() {
    let core = support::test_core();
    support::with_identity(&core, "reinstalled phone");
    let before = keys(&core).unwrap().remove(0);
    assert!(core
        .store()
        .add_key_installed_on("device", "old-server")
        .unwrap());
    assert_eq!(keys(&core).unwrap()[0].installed_on, ["old-server"]);

    core.set_signer(Some(TestSigner::new()));
    let after = keys(&core).unwrap().remove(0);
    assert_ne!(after.public_key, before.public_key);
    assert_ne!(after.fp, before.fp);
    assert!(after.installed_on.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_is_idempotent_and_records_the_server() {
    let core = support::test_core();
    support::with_identity(&core, "install phone");
    let device_id = core.device_id().unwrap();
    let home = support::temp_dir("sshkeys-install");
    let server = SshTestServer::start(ServerCfg {
        password: Some("pw".to_owned()),
        exec_with_sh: Some(home.clone()),
        ..Default::default()
    })
    .await;
    pin_server(&core, &server);
    push_server(&core, &server, "password");

    cache_password(&core);
    bounded(install_key(&core, SERVER_ID, "device"))
        .await
        .unwrap();
    let key = keys(&core).unwrap().remove(0);
    let authorized = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
    let lines: Vec<_> = authorized.lines().filter(|line| !line.is_empty()).collect();
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0], key.public_key);
    assert!(lines[0].ends_with(&format!(" rs-device={device_id}")));

    cache_password(&core);
    bounded(install_key(&core, SERVER_ID, "device"))
        .await
        .unwrap();
    let authorized = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
    assert_eq!(
        authorized.lines().filter(|line| !line.is_empty()).count(),
        1
    );
    assert_eq!(keys(&core).unwrap()[0].installed_on, [SERVER_ID]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_from_the_cached_book_writes_only_the_device_key() {
    let core = support::test_core();
    support::with_identity(&core, "cached book phone");
    let home = support::temp_dir("sshkeys-cached-book");
    let server = SshTestServer::start(ServerCfg {
        password: Some("pw".to_owned()),
        exec_with_sh: Some(home.clone()),
        ..Default::default()
    })
    .await;
    cache_server(&core, &server, SshAuth::Password);
    pin_server(&core, &server);
    cache_password(&core);

    bounded(install_key(&core, SERVER_ID, "device"))
        .await
        .unwrap();
    let device = keys(&core).unwrap().remove(0);
    let authorized = std::fs::read_to_string(home.join(".ssh/authorized_keys")).unwrap();
    assert_eq!(
        authorized
            .lines()
            .filter(|line| !line.is_empty())
            .collect::<Vec<_>>(),
        [device.public_key.as_str()]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_without_a_cached_password_points_to_the_terminal() {
    let core = support::test_core();
    support::with_identity(&core, "no password");
    let server = SshTestServer::start(ServerCfg {
        password: Some("pw".to_owned()),
        ..Default::default()
    })
    .await;
    pin_server(&core, &server);
    push_server(&core, &server, "password");

    let result = bounded(install_key(&core, SERVER_ID, "device")).await;
    assert_internal_contains(result, "open a terminal");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn install_with_an_untrusted_host_key_points_to_the_terminal() {
    let core = support::test_core();
    support::with_identity(&core, "untrusted host");
    let server = SshTestServer::start(ServerCfg {
        allow_none: true,
        ..Default::default()
    })
    .await;
    push_server(&core, &server, "password");

    let result = bounded(install_key(&core, SERVER_ID, "device")).await;
    assert_internal_contains(result, "open a terminal");
    assert!(server.observed().attempts.is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsupported_install_requests_never_connect() {
    let core = support::test_core();
    support::with_identity(&core, "unsupported installs");
    let server = SshTestServer::start(ServerCfg {
        allow_none: true,
        ..Default::default()
    })
    .await;
    push_server(&core, &server, "tailscale");

    assert!(matches!(
        install_key(&core, SERVER_ID, "device").await,
        Err(CoreError::Unsupported { .. })
    ));
    assert_internal_contains(install_key(&core, SERVER_ID, "other").await, "unknown key");
    assert!(server.observed().attempts.is_empty());
}
