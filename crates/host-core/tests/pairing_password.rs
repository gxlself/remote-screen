use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use host_core::config::{apply_patch, load, ConfigError, HostConfig};
use host_core::pairing::{
    PairingError, PairingManager, PairingReply, PairingSession, PasswordPolicy, PermissionName,
};
use host_core::pwpair::PasswordLock;
use host_core::store::{DevicePlatform, PairingPath, Store};
use host_core::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};
use host_core::whois::WhoIs;
use rsp_core::ids::id_from_fp;
use rsp_core::pb::{self, pair_result::Status};
use rsp_core::trust::Permission;
use serde_json::{json, Value};
use tokio::time::Instant;

const NOW: u64 = 1_792_465_134_123_456;
const HOST_FP: [u8; 32] = [11; 32];
const DEVICE_FP: [u8; 32] = [12; 32];
const PASSWORD: &str = "pwwire-test-password!";
const UNKNOWN: &str = "private-unknown-account";

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        logs();
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "pw-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).unwrap();
        Self(path)
    }
}
impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[derive(Clone, Default)]
struct Log(Arc<Mutex<Vec<u8>>>);
impl Write for Log {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }
    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}
fn logs() -> &'static Log {
    static LOG: OnceLock<Log> = OnceLock::new();
    LOG.get_or_init(|| {
        let log = Log::default();
        let writer = log.clone();
        tracing::subscriber::set_global_default(
            tracing_subscriber::fmt()
                .without_time()
                .with_ansi(false)
                .with_max_level(tracing::Level::TRACE)
                .with_writer(move || writer.clone())
                .finish(),
        )
        .unwrap();
        log
    })
}

#[derive(Default)]
struct Auth {
    calls: AtomicUsize,
    unavailable: AtomicBool,
}
impl SystemAuthenticator for Auth {
    fn lookup(&self, name: &str) -> Result<Option<SystemAccount>, SysAuthError> {
        self.calls.fetch_add(1, Ordering::Relaxed);
        if self.unavailable.load(Ordering::Relaxed) {
            return Err(SysAuthError::Unavailable("test backend unavailable".into()));
        }
        Ok(match name {
            "alice" | "Alice Liu" => Some(SystemAccount {
                short_name: "alice".into(),
                full_name: "Alice Liu".into(),
                is_admin: true,
                is_console_user: false,
            }),
            "bob" => Some(SystemAccount {
                short_name: "bob".into(),
                full_name: "Bob".into(),
                is_admin: false,
                is_console_user: false,
            }),
            "console" => Some(SystemAccount {
                short_name: "console".into(),
                full_name: "Console".into(),
                is_admin: false,
                is_console_user: true,
            }),
            _ => None,
        })
    }
    fn verify(&self, name: &str, password: &[u8]) -> Result<(), SysAuthError> {
        if matches!(name, "alice" | "bob" | "console") && password == PASSWORD.as_bytes() {
            Ok(())
        } else {
            Err(SysAuthError::WrongPassword)
        }
    }
}

struct Fixture {
    manager: PairingManager,
    store: Arc<Store>,
    auth: Arc<Auth>,
    dir: TestDir,
}
impl Fixture {
    fn new() -> Self {
        let dir = TestDir::new();
        let store = Arc::new(Store::open(&dir.0).unwrap());
        let manager = PairingManager::new(store.clone(), HOST_FP, "host", vec![])
            .with_password_lock(Arc::new(PasswordLock::open(&dir.0, NOW)))
            .with_clock(Arc::new(|| NOW));
        Self {
            manager,
            store,
            auth: Arc::new(Auth::default()),
            dir,
        }
    }
    fn policy(&self) -> PasswordPolicy {
        PasswordPolicy {
            enabled: true,
            permission: Permission::Control,
            authenticator: Some(self.auth.clone()),
        }
    }
    fn session(&self) -> PairingSession {
        self.manager.session_with_whois(
            DEVICE_FP,
            vec![1, 2, 3],
            Some(WhoIs {
                login: "user@hs.example.com".into(),
                stable_id: "node-id".into(),
                node_name: "phone.hs.example.com".into(),
            }),
            PairingPath::Tailnet,
        )
    }
    async fn attempt(&self, account: &str, password: &str) -> pb::PairResult {
        timed(
            &mut self.session(),
            NOW,
            message(account, password),
            self.policy(),
        )
        .await
    }
    fn state(&self) -> Value {
        serde_json::from_slice(&fs::read(self.dir.0.join("password_lock.json")).unwrap()).unwrap()
    }
}
fn message(account: &str, password: &str) -> pb::PairPassword {
    pb::PairPassword {
        device_name: "phone".into(),
        platform: pb::Platform::Ios as i32,
        account: account.into(),
        password: password.as_bytes().to_vec(),
    }
}
async fn timed(
    session: &mut PairingSession,
    now: u64,
    msg: pb::PairPassword,
    policy: PasswordPolicy,
) -> pb::PairResult {
    let start = Instant::now();
    let result = session.on_password(now, msg, policy).await.unwrap();
    if result.status == Status::Ok as i32 {
        assert!(start.elapsed() < Duration::from_secs(1));
    } else {
        assert_eq!(
            start.elapsed(),
            Duration::from_secs(1),
            "failure delay must not stack"
        );
        assert!(result.host_id.is_empty());
        assert!(result.host_name.is_empty());
    }
    result
}
fn assert_status(result: &pb::PairResult, status: Status, left: u32, retry: u32) {
    assert_eq!(
        (result.status, result.attempts_left, result.retry_after_s),
        (status as i32, left, retry)
    );
}

#[tokio::test(start_paused = true)]
async fn disabled_unsupported_and_missing_lock_do_not_consume_attempts() {
    for mode in ["disabled", "unsupported", "no-lock"] {
        let f = Fixture::new();
        let manager = if mode == "no-lock" {
            PairingManager::new(f.store.clone(), HOST_FP, "host", vec![])
        } else {
            f.manager.clone()
        };
        for _ in 0..21 {
            let mut policy = f.policy();
            policy.enabled = mode != "disabled";
            if mode == "unsupported" {
                policy.authenticator = None;
            }
            let mut session = manager.session(DEVICE_FP, vec![], None, "lan");
            assert_status(
                &timed(&mut session, NOW, message("alice", PASSWORD), policy).await,
                Status::MethodDisabled,
                0,
                0,
            );
        }
        // Twenty subsequent code starts fit into the shared window.
        for _ in 0..20 {
            assert!(
                matches!(manager.session(DEVICE_FP, vec![], None, "lan").on_start_at(NOW, pb::PairStart::default(), true).unwrap(), PairingReply::Result(result) if result.status == Status::Expired as i32)
            );
        }
        assert_eq!(f.auth.calls.load(Ordering::Relaxed), 0);
    }
}

#[tokio::test(start_paused = true)]
async fn success_persists_identity_permission_path_and_whois() {
    for (account, permission) in [
        ("alice", Permission::Control),
        ("Alice Liu", Permission::ViewOnly),
        ("console", Permission::Unattended),
    ] {
        let f = Fixture::new();
        let mut policy = f.policy();
        policy.permission = permission;
        let mut session = f.session();
        let result = timed(&mut session, NOW, message(account, PASSWORD), policy).await;
        assert_status(&result, Status::Ok, 0, 0);
        assert_eq!(result.host_id, id_from_fp(&HOST_FP));
        assert_eq!(result.host_name, "host");
        assert_eq!(session.method(), "password");
        let device = f.store.get(&DEVICE_FP).unwrap().unwrap();
        assert_eq!(device.permission, permission);
        assert_eq!(device.name, "phone");
        assert_eq!(device.platform, DevicePlatform::Ios);
        assert_eq!(device.device_id, id_from_fp(&DEVICE_FP));
        assert_eq!(device.cert_der, [1, 2, 3]);
        assert_eq!(device.paired_at_us, NOW);
        assert_eq!(device.paired_via, PairingPath::Tailnet);
        assert_eq!(device.whois_user.as_deref(), Some("user@hs.example.com"));
        assert_eq!(device.whois_node.as_deref(), Some("node-id"));
        assert_eq!(
            device.whois_node_name.as_deref(),
            Some("phone.hs.example.com")
        );
    }
}

#[tokio::test(start_paused = true)]
async fn fifth_failure_locks_and_locked_correct_password_skips_authenticator() {
    let f = Fixture::new();
    for n in 1..=5 {
        let result = f.attempt("alice", "wrong-password-private").await;
        if n == 5 {
            assert_status(&result, Status::Locked, 0, 900);
        } else {
            assert_status(&result, Status::BadCode, 5 - n, 0);
        }
    }
    assert_status(&f.attempt("alice", PASSWORD).await, Status::Locked, 0, 900);
    assert_eq!(f.auth.calls.load(Ordering::Relaxed), 5);
    assert!(f.store.paired().unwrap().is_empty());
}

#[tokio::test(start_paused = true)]
async fn wrong_password_policy_rejection_unknown_and_invalid_inputs_are_indistinguishable() {
    for (account, password) in [
        ("alice", "wrong-password-private"),
        ("bob", PASSWORD),
        (UNKNOWN, PASSWORD),
        ("", PASSWORD),
        ("alice", ""),
    ] {
        let f = Fixture::new();
        assert_status(&f.attempt(account, password).await, Status::BadCode, 4, 0);
        assert_eq!(f.state()["failures"], 1);
        if !account.is_empty() {
            assert!(!fs::read_to_string(f.dir.0.join("password_lock.json"))
                .unwrap()
                .contains(account));
        }
        assert!(f.store.paired().unwrap().is_empty());
    }
    let captured = String::from_utf8(logs().0.lock().unwrap().clone()).unwrap();
    assert!(captured.contains("pairing.password"));
    for secret in [PASSWORD, "wrong-password-private", UNKNOWN, "bob", "alice"] {
        assert!(
            !captured.contains(secret),
            "sensitive value leaked: {secret}"
        );
    }
}

#[tokio::test(start_paused = true)]
async fn unavailable_does_not_increment_password_failures() {
    let f = Fixture::new();
    assert_status(&f.attempt("alice", "wrong").await, Status::BadCode, 4, 0);
    let before = f.state();
    f.auth.unavailable.store(true, Ordering::Relaxed);
    assert_status(&f.attempt(UNKNOWN, PASSWORD).await, Status::Denied, 0, 0);
    assert_eq!(f.state(), before);
    f.auth.unavailable.store(false, Ordering::Relaxed);
    assert_status(&f.attempt("alice", "wrong").await, Status::BadCode, 3, 0);
}

#[tokio::test(start_paused = true)]
async fn global_code_limit_precedes_password_verification_and_rounds_up_retry() {
    let f = Fixture::new();
    f.manager.start(NOW).unwrap();
    for _ in 0..20 {
        assert!(matches!(
            f.session()
                .on_start_at(NOW, pb::PairStart::default(), true)
                .unwrap(),
            PairingReply::Spake(_)
        ));
    }
    let mut session = f.session();
    assert_status(
        &timed(
            &mut session,
            NOW + 1,
            message("alice", PASSWORD),
            f.policy(),
        )
        .await,
        Status::Locked,
        0,
        300,
    );
    assert_eq!(f.auth.calls.load(Ordering::Relaxed), 0);
    assert_status(
        &timed(
            &mut f.session(),
            NOW + 299_000_001,
            message("alice", PASSWORD),
            f.policy(),
        )
        .await,
        Status::Locked,
        0,
        1,
    );
    assert_status(
        &timed(
            &mut f.session(),
            NOW + 300_000_000,
            message("alice", PASSWORD),
            f.policy(),
        )
        .await,
        Status::Ok,
        0,
        0,
    );
}

#[tokio::test(start_paused = true)]
async fn password_attempts_share_the_code_limit_in_the_other_direction() {
    let f = Fixture::new();
    f.auth.unavailable.store(true, Ordering::Relaxed);
    for _ in 0..20 {
        assert_status(&f.attempt("alice", PASSWORD).await, Status::Denied, 0, 0);
    }
    let PairingReply::Result(result) = f
        .session()
        .on_start_at(NOW, pb::PairStart::default(), true)
        .unwrap()
    else {
        panic!("global limit")
    };
    assert_status(&result, Status::Locked, 0, 300);
}

#[tokio::test(start_paused = true)]
async fn connections_accept_only_one_pairing_method_and_one_password_message() {
    let f = Fixture::new();
    let mut session = f.session();
    let _ = session
        .on_start_at(NOW, pb::PairStart::default(), true)
        .unwrap();
    assert!(matches!(
        session
            .on_password(NOW, message("alice", PASSWORD), f.policy())
            .await,
        Err(PairingError::Protocol(_))
    ));
    for enabled in [true, false] {
        let mut session = f.session();
        let mut policy = f.policy();
        policy.enabled = enabled;
        timed(&mut session, NOW, message("alice", PASSWORD), policy).await;
        assert!(matches!(
            session
                .on_password(NOW, message("alice", PASSWORD), f.policy())
                .await,
            Err(PairingError::Protocol(_))
        ));
        assert!(matches!(
            session.on_start_at(NOW, pb::PairStart::default(), true),
            Err(PairingError::Protocol(_))
        ));
    }
}

#[tokio::test(start_paused = true)]
async fn revoked_device_is_denied_with_the_same_minimum_delay() {
    let f = Fixture::new();
    f.store.revoke(DEVICE_FP, NOW).unwrap();
    let before = f.store.paired().unwrap();
    assert_status(&f.attempt("alice", PASSWORD).await, Status::Denied, 0, 0);
    assert_eq!(f.store.paired().unwrap(), before);
}

#[test]
fn legacy_config_defaults_and_security_patches() {
    let dir = TestDir::new();
    let path = dir.0.join("config.toml");
    fs::write(&path, "[security]\nconfirm_new_devices = false\n").unwrap();
    let config = load(&path).unwrap();
    assert!(!config.security.password_pairing);
    assert_eq!(
        config.security.password_pairing_permission,
        PermissionName::Control
    );
    let error = apply_patch(
        &config,
        &json!({"security":{"password_pairing_permission":"admin"}}),
    )
    .unwrap_err();
    assert!(
        matches!(error, ConfigError::Invalid {field, ..} if field == "security.password_pairing_permission")
    );
    assert!(
        apply_patch(&config, &json!({"security":{"password_pairing":true}}))
            .unwrap()
            .security
            .password_pairing
    );
    assert_eq!(
        apply_patch(
            &HostConfig::default(),
            &json!({"security":{"password_pairing_permission":"view_only"}})
        )
        .unwrap()
        .security
        .password_pairing_permission,
        PermissionName::ViewOnly
    );
}

#[cfg(unix)]
mod notifications {
    use super::*;
    use host_core::ipc::{self, IpcServer};
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;

    struct Backend;
    macro_rules! unused_methods {
    ($($name:ident($($arg:ident: $ty:ty),*) -> $out:ty;)*) => {
        $(async fn $name(&self $(, $arg: $ty)*) -> Result<$out, ipc::IpcError> {
            Err(ipc::IpcError::business("unsupported", "notification fixture"))
        })*
    };
}
    impl ipc::IpcBackend for Backend {
        unused_methods! {
            status_get() -> ipc::StatusGetResult;
            sessions_list() -> ipc::SessionsListResult;
            sessions_disconnect(_p: ipc::SessionsDisconnectParams) -> ipc::SessionsDisconnectResult;
            sessions_set_view_only(_p: ipc::SessionsSetViewOnlyParams) -> ipc::SessionsSetViewOnlyResult;
            pairing_start() -> ipc::PairingStartResult;
            pairing_cancel(_p: ipc::PairingCancelParams) -> ipc::PairingCancelResult;
            devices_list() -> ipc::DevicesListResult;
            devices_update(_p: ipc::DevicesUpdateParams) -> ipc::DevicesUpdateResult;
            devices_revoke(_p: ipc::DevicesRevokeParams) -> ipc::DevicesRevokeResult;
            settings_get() -> Value;
            settings_set(_p: Value) -> Value;
            network_list_nics() -> ipc::NetworkListNicsResult;
            network_firewall_status() -> ipc::NetworkFirewallStatusResult;
            network_it_note() -> ipc::NetworkItNoteResult;
            approval_respond(_p: ipc::ApprovalRespondParams) -> ipc::ApprovalRespondResult;
            update_status() -> ipc::UpdateStatusResult;
            update_install_now() -> ipc::UpdateInstallNowResult;
            update_set_channel(_p: ipc::UpdateSetChannelParams) -> ipc::UpdateSetChannelResult;
            logs_export(_p: ipc::LogsExportParams) -> ipc::LogsExportResult;
            audit_list(_p: ipc::AuditListParams) -> ipc::AuditListResult;
        }
    }

    #[tokio::test(start_paused = true)]
    async fn password_event_has_device_name_method_and_successful_short_name_only() {
        let mut f = Fixture::new();
        let directory = fs::File::open(&f.dir.0).unwrap();
        #[cfg(target_os = "linux")]
        let path = {
            use std::os::fd::AsRawFd;
            PathBuf::from(format!("/proc/self/fd/{}/ipc", directory.as_raw_fd()))
        };
        #[cfg(not(target_os = "linux"))]
        let path = f.dir.0.join("ipc");
        let server = IpcServer::bind(&path, Backend).unwrap();
        f.manager = f.manager.clone().with_notifier(server.notifier());
        let task = tokio::spawn(async move { server.serve().await.unwrap() });
        let mut client = BufReader::new(UnixStream::connect(&path).await.unwrap());
        client
            .get_mut()
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"status.get\"}\n")
            .await
            .unwrap();
        let mut line = String::new();
        client.read_line(&mut line).await.unwrap();
        assert_eq!(serde_json::from_str::<Value>(&line).unwrap()["id"], 1);
        for account in ["alice", "Alice Liu", UNKNOWN] {
            let result = f.attempt(account, PASSWORD).await;
            line.clear();
            client.read_line(&mut line).await.unwrap();
            let event: Value = serde_json::from_str(&line).unwrap();
            assert_eq!(event["method"], "event.pairingResult");
            let params = &event["params"];
            assert_eq!(params["method"], "password");
            assert_eq!(params["device_name"], "phone");
            assert_eq!(params["status"], result.status);
            if account == UNKNOWN {
                assert!(params.get("account").is_none());
                assert!(!line.contains(UNKNOWN));
            } else {
                assert_eq!(params["account"], "alice");
                assert_eq!(params["host_id"], id_from_fp(&HOST_FP));
            }
            assert!(!line.contains(PASSWORD));
        }
        task.abort();
        let _ = task.await;
        drop(directory);
    }
}
