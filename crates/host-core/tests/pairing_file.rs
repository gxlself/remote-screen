use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};

use async_trait::async_trait;
use data_encoding::BASE64URL_NOPAD;
use host_core::pairfile_store::{PairFileError, PairFileState, PairFileStore};
use host_core::pairing::{
    default_export_dir, ExportFileRequest, ExportedFile, PairApproval, PairApprovalRequest,
    PairApprover, PairingConfirmOutcome, PairingError, PairingManager, PairingReply,
    PairingSession,
};
use host_core::store::{PairingPath, Store};
use host_core::whois::WhoIs;
use rsp_core::pairfile::{file_name, parse, PairFile, PAIRFILE_LIFETIME_S};
use rsp_core::pairing::AppPairing;
use rsp_core::pb::{self, pair_result::Status};
use rsp_core::trust::Permission;

const NOW: u64 = 1_792_465_134_123_456;
const HOST_FP: [u8; 32] = [11; 32];
const DEVICE_FP: [u8; 32] = [12; 32];

struct TestDir(PathBuf);
impl TestDir {
    fn new() -> Self {
        static NEXT: AtomicU64 = AtomicU64::new(0);
        let path = std::env::temp_dir().join(format!(
            "e{}-{}",
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

#[derive(Default)]
struct CountApprover(AtomicUsize);
#[async_trait]
impl PairApprover for CountApprover {
    async fn request(&self, _: PairApprovalRequest) -> PairApproval {
        self.0.fetch_add(1, Ordering::Relaxed);
        PairApproval {
            allow: true,
            permission: Permission::Control,
        }
    }
}

struct Fixture {
    manager: PairingManager,
    store: Arc<Store>,
    approver: Arc<CountApprover>,
    now: Arc<AtomicU64>,
    dir: TestDir,
}
impl Fixture {
    fn new() -> Self {
        let dir = TestDir::new();
        let store = Arc::new(Store::open(dir.0.join("state")).unwrap());
        let files = Arc::new(PairFileStore::open(&dir.0.join("state")).unwrap());
        let approver = Arc::new(CountApprover::default());
        let now = Arc::new(AtomicU64::new(NOW));
        let clock = now.clone();
        let manager = PairingManager::new(
            store.clone(),
            HOST_FP,
            "Host 示例",
            (1..=10)
                .map(|i| format!("100.64.0.{i}:47990").parse().unwrap())
                .collect(),
        )
        .with_files(files)
        .with_approver(approver.clone())
        .with_clock(Arc::new(move || clock.load(Ordering::Relaxed)));
        Self {
            manager,
            store,
            approver,
            now,
            dir,
        }
    }
    fn export(&self, permission: Permission) -> (ExportedFile, PairFile) {
        let exported = self
            .manager
            .export_file(
                self.now.load(Ordering::Relaxed),
                ExportFileRequest {
                    permission,
                    label: Some("IT 分发".into()),
                    dir: self.dir.0.clone(),
                },
            )
            .unwrap();
        let file = parse(&fs::read_to_string(&exported.path).unwrap()).unwrap();
        (exported, file)
    }
    fn session(&self) -> PairingSession {
        self.manager.session_with_whois(
            DEVICE_FP,
            vec![1, 2, 3],
            Some(WhoIs {
                login: "user@example.com".into(),
                stable_id: "node-phone".into(),
                node_name: "phone.hs.example.com".into(),
            }),
            PairingPath::Lan,
        )
    }
}

fn result(reply: PairingReply) -> pb::PairResult {
    let PairingReply::Result(result) = reply else {
        panic!("expected result")
    };
    result
}
fn assert_status(result: &pb::PairResult, status: Status, attempts_left: u32) {
    assert_eq!(result.status, status as i32);
    assert_eq!(result.attempts_left, attempts_left);
}
fn start(file_id: Vec<u8>) -> pb::PairStart {
    pb::PairStart {
        device_name: "phone".into(),
        pair_file_id: file_id,
        ..Default::default()
    }
}
fn expired(f: &Fixture, id: Vec<u8>) {
    let before = f.store.paired().unwrap();
    let mut session = f.session();
    assert_status(
        &result(session.on_start(start(id), true).unwrap()),
        Status::Expired,
        0,
    );
    assert_eq!(session.method(), "file");
    assert_eq!(f.store.paired().unwrap(), before);
}
fn exchange(
    f: &Fixture,
    file: &PairFile,
    secret: &[u8; 32],
) -> (PairingSession, AppPairing, pb::PairConfirm) {
    let mut session = f.session();
    let mut app = AppPairing::new_file(
        file.file_id,
        secret,
        DEVICE_FP,
        HOST_FP,
        "phone",
        pb::Platform::Ios,
    );
    let PairingReply::Spake(spake) = session.on_start(app.try_start().unwrap(), true).unwrap()
    else {
        panic!("expected SPAKE")
    };
    let (spake, confirm) = app.on_spake(spake).unwrap();
    assert_eq!(session.on_spake(spake).unwrap(), PairingReply::Continue);
    (session, app, confirm)
}
fn pair(f: &Fixture, file: &PairFile) -> pb::PairResult {
    let (mut session, mut app, confirm) = exchange(f, file, &file.secret);
    let PairingConfirmOutcome::Paired { confirm, result } =
        session.on_confirm_outcome(confirm).unwrap()
    else {
        panic!("file must pair immediately without WAITING_LOCAL")
    };
    app.on_confirm(confirm).unwrap();
    assert_status(&result, Status::Ok, 0);
    result
}

#[test]
fn exports_private_parseable_files_with_unique_names_and_limits() {
    let f = Fixture::new();
    let (exported, file) = f.export(Permission::ViewOnly);
    let content = fs::read_to_string(&exported.path).unwrap();
    assert!(content.ends_with('\n'));
    assert_eq!(content.lines().count(), 1);
    assert_eq!(file.host_id, rsp_core::ids::id_from_fp(&HOST_FP));
    assert_eq!(file.fp, HOST_FP);
    assert_eq!(file.name, "Host 示例");
    assert_eq!(file.addrs.len(), 8);
    assert_eq!(
        file.addrs,
        (1..=8)
            .map(|i| format!("100.64.0.{i}:47990").parse().unwrap())
            .collect::<Vec<std::net::SocketAddr>>()
    );
    assert_eq!(file.expires_at_s, NOW / 1_000_000 + PAIRFILE_LIFETIME_S);
    assert_eq!(
        exported.expires_at_us,
        NOW + PAIRFILE_LIFETIME_S * 1_000_000
    );
    assert_eq!(exported.file_id, BASE64URL_NOPAD.encode(&file.file_id));
    let name = file_name("Host 示例", NOW / 1_000_000);
    assert_eq!(exported.file_name, name);
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(
            fs::metadata(&exported.path).unwrap().permissions().mode() & 0o777,
            0o600
        );
    }
    let (second, _) = f.export(Permission::Unattended);
    assert_eq!(
        second.file_name,
        format!("{} (2).rspair", name.strip_suffix(".rspair").unwrap())
    );
    assert_eq!(fs::read_to_string(exported.path).unwrap(), content);
    for _ in 2..50 {
        f.export(Permission::Control);
    }
    assert!(matches!(
        f.manager.export_file(
            NOW,
            ExportFileRequest {
                permission: Permission::Control,
                label: None,
                dir: f.dir.0.clone(),
            }
        ),
        Err(PairingError::PairFile(PairFileError::TooMany))
    ));
    assert!(!format!("{:?}", f.manager).contains(&BASE64URL_NOPAD.encode(file.secret.as_ref())));
}

#[test]
fn invalid_export_paths_and_unavailable_store() {
    let f = Fixture::new();
    let plain_file = f.dir.0.join("regular-file");
    fs::write(&plain_file, b"keep").unwrap();
    for dir in [
        PathBuf::from("relative"),
        f.dir.0.join("missing"),
        plain_file,
    ] {
        assert!(matches!(
            f.manager.export_file(
                NOW,
                ExportFileRequest {
                    permission: Permission::Control,
                    label: None,
                    dir,
                }
            ),
            Err(PairingError::InvalidPath)
        ));
    }
    assert!(f.manager.list_files(NOW).unwrap().is_empty());
    let manager = PairingManager::new(f.store.clone(), HOST_FP, "host", vec![]);
    assert!(!manager.has_files());
    assert!(matches!(
        manager.export_file(
            NOW,
            ExportFileRequest {
                permission: Permission::Control,
                label: None,
                dir: f.dir.0.clone(),
            }
        ),
        Err(PairingError::FilesUnavailable)
    ));
    assert!(matches!(
        manager.list_files(NOW),
        Err(PairingError::FilesUnavailable)
    ));
    assert!(matches!(
        manager.revoke_file("unknown"),
        Err(PairingError::FilesUnavailable)
    ));
    assert_status(
        &result(
            manager
                .session(DEVICE_FP, vec![], None, "lan")
                .on_start(start(vec![1; 16]), true)
                .unwrap(),
        ),
        Status::Expired,
        0,
    );
}

#[test]
fn default_directory_is_created_private() {
    let dir = TestDir::new();
    #[cfg(not(target_os = "macos"))]
    {
        let path = default_export_dir(&dir.0);
        assert_eq!(path, dir.0.join("exports"));
        assert!(path.is_dir());
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            assert_eq!(
                fs::metadata(path).unwrap().permissions().mode() & 0o777,
                0o700
            );
        }
    }
}

#[test]
fn file_pairs_without_approval_and_preserves_exported_permissions_and_identity() {
    for permission in [Permission::ViewOnly, Permission::Unattended] {
        let f = Fixture::new();
        let (_, file) = f.export(permission);
        pair(&f, &file);
        assert_eq!(f.approver.0.load(Ordering::Relaxed), 0);
        let device = f.store.get(&DEVICE_FP).unwrap().unwrap();
        assert_eq!(device.permission, permission);
        assert_eq!(device.name, "phone");
        assert_eq!(device.cert_der, vec![1, 2, 3]);
        assert_eq!(device.paired_via, PairingPath::Lan);
        assert_eq!(device.whois_user.as_deref(), Some("user@example.com"));
        assert_eq!(device.whois_node.as_deref(), Some("node-phone"));
        assert_eq!(
            device.whois_node_name.as_deref(),
            Some("phone.hs.example.com")
        );
        assert_eq!(device.paired_at_us, NOW);
        let files = f.manager.list_files(NOW).unwrap();
        assert_eq!(files[0].state, PairFileState::Used);
        assert_eq!(files[0].permission, permission);
        assert_eq!(files[0].used_by_device_id, Some(device.device_id));
        assert_eq!(files[0].used_at_us, Some(NOW));
        expired(&f, file.file_id.to_vec());
    }
}

#[test]
fn fifth_wrong_secret_fails_file_and_bad_spake_counts_too() {
    let f = Fixture::new();
    let (_, file) = f.export(Permission::Control);
    for attempt in 1..=5 {
        let (mut session, _, confirm) = exchange(&f, &file, &[0; 32]);
        let PairingConfirmOutcome::Result(result) = session.on_confirm_outcome(confirm).unwrap()
        else {
            panic!("expected failure")
        };
        assert_status(
            &result,
            if attempt == 5 {
                Status::Expired
            } else {
                Status::BadCode
            },
            5 - attempt,
        );
        assert!(f.store.paired().unwrap().is_empty());
    }
    assert_eq!(
        f.manager.list_files(NOW).unwrap()[0].state,
        PairFileState::Failed
    );
    expired(&f, file.file_id.to_vec());
    let (_, file) = f.export(Permission::Control);
    let mut session = f.session();
    assert!(matches!(
        session
            .on_start(start(file.file_id.to_vec()), true)
            .unwrap(),
        PairingReply::Spake(_)
    ));
    assert_status(
        &result(session.on_spake(pb::PairSpake { msg: vec![] }).unwrap()),
        Status::BadCode,
        4,
    );
    assert!(f.store.paired().unwrap().is_empty());
}

#[test]
fn revoked_expired_malformed_and_unknown_files_are_expired() {
    let f = Fixture::new();
    let (exported, file) = f.export(Permission::Control);
    f.manager.revoke_file(&exported.file_id).unwrap();
    assert_eq!(
        f.manager.list_files(NOW).unwrap()[0].state,
        PairFileState::Revoked
    );
    expired(&f, file.file_id.to_vec());
    expired(&f, vec![1; 15]);
    expired(&f, vec![1; 17]);
    expired(&f, vec![0; 16]);
    let (_, file) = f.export(Permission::Control);
    f.now
        .store(NOW + PAIRFILE_LIFETIME_S * 1_000_000, Ordering::Relaxed);
    expired(&f, file.file_id.to_vec());
}

#[test]
fn global_limit_and_path_policy_apply_without_a_code() {
    let f = Fixture::new();
    for _ in 0..20 {
        expired(&f, vec![0; 16]);
    }
    assert_status(
        &result(f.session().on_start(start(vec![0; 16]), true).unwrap()),
        Status::Locked,
        0,
    );
    f.now.store(NOW + 300_000_000, Ordering::Relaxed);
    assert_status(
        &result(f.session().on_start(start(vec![0; 16]), false).unwrap()),
        Status::DisabledOnPath,
        0,
    );
    assert!(f.store.paired().unwrap().is_empty());
}

#[test]
fn global_limit_is_shared_with_six_digit_codes() {
    let f = Fixture::new();
    f.manager.start(NOW).unwrap();
    for _ in 0..10 {
        expired(&f, vec![0; 16]);
        assert!(matches!(
            f.session()
                .on_start_at(NOW, pb::PairStart::default(), true)
                .unwrap(),
            PairingReply::Spake(_)
        ));
    }
    assert_status(
        &result(f.session().on_start(start(vec![0; 16]), true).unwrap()),
        Status::Locked,
        0,
    );
}

#[test]
fn file_is_checked_again_at_confirm_and_revoked_devices_are_denied() {
    for action in ["consume", "revoke", "expire", "device-revoke"] {
        let f = Fixture::new();
        let (exported, file) = f.export(Permission::Control);
        let (mut session, _, confirm) = exchange(&f, &file, &file.secret);
        match action {
            "consume" => {
                pair(&f, &file);
            }
            "revoke" => f.manager.revoke_file(&exported.file_id).unwrap(),
            "expire" => f
                .now
                .store(NOW + PAIRFILE_LIFETIME_S * 1_000_000, Ordering::Relaxed),
            "device-revoke" => f.store.revoke(DEVICE_FP, NOW).unwrap(),
            _ => unreachable!(),
        }
        let before = f.store.paired().unwrap();
        let PairingConfirmOutcome::Result(result) = session.on_confirm_outcome(confirm).unwrap()
        else {
            panic!("expected denial")
        };
        assert_status(
            &result,
            if action == "device-revoke" {
                Status::Denied
            } else {
                Status::Expired
            },
            0,
        );
        assert_eq!(f.store.paired().unwrap(), before);
        assert_eq!(f.approver.0.load(Ordering::Relaxed), 0);
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

#[test]
fn success_and_failure_tracing_redact_secret_and_uri() {
    let log = Log::default();
    let writer = log.clone();
    let subscriber = tracing_subscriber::fmt()
        .without_time()
        .with_ansi(false)
        .with_max_level(tracing::Level::TRACE)
        .with_writer(move || writer.clone())
        .finish();
    let _guard = tracing::subscriber::set_default(subscriber);
    let f = Fixture::new();
    let (_, file) = f.export(Permission::Control);
    let (mut session, _, confirm) = exchange(&f, &file, &[0; 32]);
    assert!(matches!(
        session.on_confirm_outcome(confirm).unwrap(),
        PairingConfirmOutcome::Result(_)
    ));
    pair(&f, &file);
    let logs = String::from_utf8(log.0.lock().unwrap().clone()).unwrap();
    assert!(logs.contains("pairing file exported"), "{logs}");
    assert!(logs.contains("pairing completed"), "{logs}");
    assert!(!logs.contains(&BASE64URL_NOPAD.encode(file.secret.as_ref())));
    assert!(!logs.contains("rsp://pairfile"));
    assert!(!format!("{:?}", f.manager).contains(&BASE64URL_NOPAD.encode(file.secret.as_ref())));
}

#[cfg(unix)]
mod ipc_events {
    use super::*;
    use host_core::ipc::*;
    use rsp_core::trust::Permission;
    use std::time::Duration;
    use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
    use tokio::net::UnixStream;
    struct Backend;
    impl IpcBackend for Backend {
        async fn status_get(&self) -> Result<StatusGetResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn sessions_list(&self) -> Result<SessionsListResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn sessions_disconnect(
            &self,
            _params: SessionsDisconnectParams,
        ) -> Result<SessionsDisconnectResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn sessions_set_view_only(
            &self,
            _params: SessionsSetViewOnlyParams,
        ) -> Result<SessionsSetViewOnlyResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn pairing_start(&self) -> Result<PairingStartResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn pairing_cancel(
            &self,
            _params: PairingCancelParams,
        ) -> Result<PairingCancelResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn devices_list(&self) -> Result<DevicesListResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn devices_update(
            &self,
            _params: DevicesUpdateParams,
        ) -> Result<DevicesUpdateResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn devices_revoke(
            &self,
            _params: DevicesRevokeParams,
        ) -> Result<DevicesRevokeResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn settings_get(&self) -> Result<Value, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn settings_set(&self, _settings: Value) -> Result<Value, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn network_list_nics(&self) -> Result<NetworkListNicsResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn network_firewall_status(&self) -> Result<NetworkFirewallStatusResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn network_it_note(&self) -> Result<NetworkItNoteResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn approval_respond(
            &self,
            _params: ApprovalRespondParams,
        ) -> Result<ApprovalRespondResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn update_status(&self) -> Result<UpdateStatusResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn update_install_now(&self) -> Result<UpdateInstallNowResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn update_set_channel(
            &self,
            _params: UpdateSetChannelParams,
        ) -> Result<UpdateSetChannelResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn logs_export(
            &self,
            _params: LogsExportParams,
        ) -> Result<LogsExportResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
        async fn audit_list(&self, _params: AuditListParams) -> Result<AuditListResult, IpcError> {
            Err(IpcError::business("unsupported", "unused"))
        }
    }

    #[tokio::test]
    async fn result_events_include_file_and_code_method_and_device_name() {
        let mut f = Fixture::new();
        let socket = f.dir.0.join("ipc");
        let server = IpcServer::bind(&socket, Backend).unwrap();
        f.manager = f.manager.with_notifier(server.notifier());
        let task = tokio::spawn(async move { server.serve().await.unwrap() });
        let stream = UnixStream::connect(socket).await.unwrap();
        let (read, mut write) = stream.into_split();
        let mut lines = BufReader::new(read).lines();
        // A round trip ensures the server has subscribed before pairing emits.
        write
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"status.get\"}\n")
            .await
            .unwrap();
        tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let (_, file) = f.export(Permission::Control);
        pair(&f, &file);
        let event = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event: Value = serde_json::from_str(&event).unwrap();
        assert_eq!(event["method"], "event.pairingResult");
        assert_eq!(event["params"]["method"], "file");
        assert_eq!(event["params"]["device_name"], "phone");
        assert_eq!(event["params"]["status"], Status::Ok as i32);
        assert!(!event
            .to_string()
            .contains(&BASE64URL_NOPAD.encode(file.secret.as_ref())));

        let ticket = f.manager.start(NOW).unwrap();
        let mut app = AppPairing::new(
            ticket.code,
            [13; 32],
            HOST_FP,
            "code phone",
            pb::Platform::Ios,
        )
        .unwrap();
        let mut session = f.manager.session([13; 32], vec![], None, "lan");
        let PairingReply::Spake(spake) = session.on_start(app.try_start().unwrap(), true).unwrap()
        else {
            panic!("SPAKE")
        };
        let (spake, confirm) = app.on_spake(spake).unwrap();
        session.on_spake(spake).unwrap();
        let PairingConfirmOutcome::WaitingLocal { confirm, result } =
            session.on_confirm_outcome(confirm).unwrap()
        else {
            panic!("codes require local approval")
        };
        assert_status(&result, Status::WaitingLocal, 5);
        app.on_confirm(confirm).unwrap();
        assert_status(&session.approve().await.unwrap(), Status::Ok, 0);
        let event = tokio::time::timeout(Duration::from_secs(2), lines.next_line())
            .await
            .unwrap()
            .unwrap()
            .unwrap();
        let event: Value = serde_json::from_str(&event).unwrap();
        assert_eq!(event["params"]["method"], "code");
        assert_eq!(event["params"]["device_name"], "code phone");
        task.abort();
        let _ = task.await;
    }
}

#[test]
fn failed_write_revokes_record() {
    use host_core::pairing::{OsRandom, RandomSource};
    struct RemoveDir(PathBuf);
    impl RandomSource for RemoveDir {
        fn fill(&self, bytes: &mut [u8]) -> Result<(), getrandom::Error> {
            if self.0.is_dir() {
                fs::remove_dir(&self.0).unwrap();
                fs::write(&self.0, b"directory replaced").unwrap();
            }
            OsRandom.fill(bytes)
        }
    }
    let f = Fixture::new();
    let exports = f.dir.0.join("exports");
    fs::create_dir(&exports).unwrap();
    let files = Arc::new(
        PairFileStore::open(&f.dir.0.join("write-error"))
            .unwrap()
            .with_rng(Arc::new(RemoveDir(exports.clone()))),
    );
    let manager = f.manager.with_files(files);
    assert!(matches!(
        manager.export_file(
            NOW,
            ExportFileRequest {
                permission: Permission::ViewOnly,
                label: None,
                dir: exports,
            }
        ),
        Err(PairingError::Export(_))
    ));
    let files = manager.list_files(NOW).unwrap();
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].state, PairFileState::Revoked);
}
