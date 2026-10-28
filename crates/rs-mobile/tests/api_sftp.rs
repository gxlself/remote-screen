//! File-page SFTP wiring, using the shared process-local SSH/SFTP fixture.
#[allow(dead_code)]
#[path = "../../rs-ssh/tests/support/sftp_server.rs"]
mod sftp_server;
mod support;

use std::collections::HashMap;
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::Duration;

use async_trait::async_trait;
use rs_mobile::rscore::api_sftp::{self, SftpConnector};
use rs_mobile::rscore::dto_files::{FileEntryKind, FileTransfer, FileTransferState};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::{self, Core};
use rs_mobile::rscore::xfer::{self, Transfers};
use rs_ssh::{SshError, SshSession};
use sftp_server::SftpTestServer;
use support::EventCollector;

const MIB: usize = 1024 * 1024;
const SIZE: usize = 8 * MIB;
const WAIT: Duration = Duration::from_secs(35);

struct TestConnector {
    servers: HashMap<String, Arc<SftpTestServer>>,
    calls: AtomicUsize,
    auth_required: AtomicBool,
    disconnect_each: AtomicBool,
}

impl TestConnector {
    fn new(servers: HashMap<String, Arc<SftpTestServer>>) -> Self {
        Self {
            servers,
            calls: AtomicUsize::new(0),
            auth_required: AtomicBool::new(false),
            disconnect_each: AtomicBool::new(false),
        }
    }
}

#[async_trait]
impl SftpConnector for TestConnector {
    async fn connect(&self, _: &Arc<Core>, server_id: &str) -> Result<SshSession, CoreError> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        if self.auth_required.load(Ordering::SeqCst) {
            return Err(CoreError::Http {
                status: 401,
                hub_code: None,
                message: "sftp.authRequired: connect in terminal first".into(),
            });
        }
        let server = &self.servers[server_id];
        let session = server.connect().await;
        server.delay_io_responses(Some(Duration::from_millis(10)));
        if self.disconnect_each.load(Ordering::SeqCst) {
            server.fail_after_read_bytes(Some(MIB as u64));
        }
        Ok(session)
    }
}

struct Fixture {
    core: Arc<Core>,
    local: PathBuf,
    server: Arc<SftpTestServer>,
    connector: Arc<TestConnector>,
    events: Arc<EventCollector>,
}

impl Fixture {
    fn new() -> Self {
        Self::with_sftp(true)
    }

    fn with_sftp(enabled: bool) -> Self {
        let local = support::temp_dir("mobile-sftp-files");
        let core = support::test_core_with(|cfg, _| {
            cfg.files_dir = Some(local.to_string_lossy().into_owned());
        });
        let server = Arc::new(core.block_on(async {
            if enabled {
                SftpTestServer::start_temporary("mobile-sftp-server").await
            } else {
                SftpTestServer::without_sftp(sftp_server::unique_temp_dir("mobile-no-sftp")).await
            }
        }));
        let connector = Arc::new(TestConnector::new(HashMap::from([(
            "server-a".into(),
            server.clone(),
        )])));
        api_sftp::set_connector(&core, connector.clone());
        let events = EventCollector::new();
        core.events().subscribe(events.clone());
        Self {
            core,
            local,
            server,
            connector,
            events,
        }
    }

    fn source(&self, name: &str) -> Vec<u8> {
        let bytes: Vec<_> = (0..SIZE)
            .map(|n| (n.wrapping_mul(37) % 251) as u8)
            .collect();
        fs::write(self.server.root().join(name), &bytes).unwrap();
        bytes
    }

    fn local_source(&self, name: &str) -> (String, Vec<u8>) {
        let bytes: Vec<_> = (0..SIZE)
            .map(|n| (n.wrapping_mul(19) % 241) as u8)
            .collect();
        let path = self.local.join(name);
        fs::write(&path, &bytes).unwrap();
        (path.to_string_lossy().into_owned(), bytes)
    }

    fn transfer(&self, id: &str) -> FileTransfer {
        self.core.state::<Transfers>().get(&self.core, id).unwrap()
    }

    async fn wait(&self, id: &str, predicate: impl Fn(&FileTransfer) -> bool) -> FileTransfer {
        tokio::time::timeout(WAIT, async {
            loop {
                let transfer = self.transfer(id);
                if predicate(&transfer) {
                    return transfer;
                }
                assert_ne!(transfer.state, FileTransferState::Failed, "{transfer:?}");
                tokio::time::sleep(Duration::from_millis(5)).await;
            }
        })
        .await
        .unwrap_or_else(|_| panic!("transfer timed out: {:?}", self.transfer(id)))
    }

    async fn completed(&self, id: &str) -> FileTransfer {
        let transfer = self
            .wait(id, |transfer| {
                transfer.state == FileTransferState::Completed
            })
            .await;
        assert_eq!(transfer.done, transfer.size);
        assert_eq!(transfer.bytes_per_sec, 0);
        assert_eq!(transfer.fail, None);
        transfer
    }

    async fn downloading(&self) -> String {
        let id = api_sftp::download(&self.core, "server-a", "/home/test/source.bin")
            .await
            .unwrap();
        assert!(self.transfer(&id).local_path.is_some());
        self.wait(&id, |transfer| {
            transfer.done >= MIB as u64 && transfer.state == FileTransferState::Running
        })
        .await;
        id
    }

    async fn uploading(&self, path: &str) -> String {
        let id = api_sftp::upload(&self.core, "server-a", path, None)
            .await
            .unwrap();
        assert_eq!(self.transfer(&id).saved_name.as_deref(), Some("source.bin"));
        self.wait(&id, |transfer| {
            transfer.done >= 2 * MIB as u64 && transfer.state == FileTransferState::Running
        })
        .await;
        id
    }

    fn states(&self, id: &str) -> Vec<String> {
        assert!(self.core.events().flush(Duration::from_secs(2)));
        self.events
            .of_type("file.transfer")
            .into_iter()
            .filter(|event| event["transfer"]["transferId"] == id)
            .map(|event| event["transfer"]["state"].as_str().unwrap().to_owned())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = fs::remove_dir_all(&self.local);
        let _ = fs::remove_dir_all(self.core.data_dir());
    }
}

fn assert_http(error: CoreError, expected_status: u16, prefix: &str) {
    match error {
        CoreError::Http {
            status, message, ..
        } => {
            assert_eq!(status, expected_status);
            assert!(message.starts_with(prefix), "{message}");
        }
        error => panic!("expected HTTP {expected_status} {prefix}, got {error:?}"),
    }
}

fn partials(directory: &Path) -> Vec<PathBuf> {
    fs::read_dir(directory)
        .unwrap()
        .map(|entry| entry.unwrap().path())
        .filter(|path| {
            path.file_name()
                .unwrap()
                .to_string_lossy()
                .contains(".rspart-")
        })
        .collect()
}

async fn wait_clean(directory: &Path) {
    tokio::time::timeout(WAIT, async {
        while !partials(directory).is_empty() {
            tokio::time::sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .expect("partials were not cleaned");
}

#[test]
fn listing_home_sort_parent_and_metadata() {
    let f = Fixture::new();
    fs::create_dir(f.server.root().join("z-folder")).unwrap();
    fs::write(f.server.root().join("Alpha.txt"), b"alpha").unwrap();
    fs::write(f.server.root().join("beta.txt"), b"beta").unwrap();
    f.core.block_on(async {
        let listing = api_sftp::list(&f.core, "server-a", None).await.unwrap();
        assert_eq!(listing.path, "/home/test");
        assert_eq!(listing.parent.as_deref(), Some("/home"));
        assert_eq!(
            listing
                .entries
                .iter()
                .map(|entry| entry.name.as_str())
                .collect::<Vec<_>>(),
            ["z-folder", "Alpha.txt", "beta.txt"]
        );
        assert_eq!(listing.entries[0].kind, FileEntryKind::Dir);
        assert_eq!(listing.entries[0].size, None);
        assert_eq!(listing.entries[1].size, Some(5));
        assert!(listing
            .entries
            .iter()
            .all(|entry| entry.child_count.is_none()
                && entry
                    .mtime_us
                    .is_some_and(|time| time > 0 && time % 1_000_000 == 0)));
        assert!(!listing.truncated);
        assert_eq!(listing.net_path, None);
        let nested = api_sftp::list(&f.core, "server-a", Some("/home/test/z-folder".into()))
            .await
            .unwrap();
        assert_eq!(nested.path, "/home/test/z-folder");
        assert_eq!(nested.parent.as_deref(), Some("/home/test"));
    });
}

#[test]
fn missing_list_and_download_return_not_found_without_record() {
    let f = Fixture::new();
    f.core.block_on(async {
        assert_http(
            api_sftp::list(&f.core, "server-a", Some("/home/test/missing".into()))
                .await
                .unwrap_err(),
            404,
            "sftp.notFound",
        );
        assert_http(
            api_sftp::download(&f.core, "server-a", "/home/test/missing")
                .await
                .unwrap_err(),
            404,
            "sftp.notFound",
        );
    });
    assert!(xfer::list(&f.core).is_empty());
}

#[test]
fn rejected_subsystem_returns_unavailable_and_does_not_cache_connection() {
    let f = Fixture::with_sftp(false);
    f.core.block_on(async {
        for _ in 0..2 {
            assert_http(
                api_sftp::list(&f.core, "server-a", None).await.unwrap_err(),
                501,
                "sftp.unavailable",
            );
        }
    });
    assert_eq!(f.connector.calls.load(Ordering::SeqCst), 2);
}

#[test]
fn downloads_eight_mib_without_overwriting_existing_name() {
    let f = Fixture::new();
    let expected = f.source("source.bin");
    fs::write(f.local.join("source.bin"), b"keep original").unwrap();
    f.core.block_on(async {
        let id = api_sftp::download(&f.core, "server-a", "/home/test/source.bin")
            .await
            .unwrap();
        let transfer = f.completed(&id).await;
        assert_eq!(transfer.size, SIZE as u64);
        assert_eq!(
            transfer.local_path.as_deref(),
            f.local.join("source (1).bin").to_str()
        );
        assert_eq!(fs::read(transfer.local_path.unwrap()).unwrap(), expected);
        assert_eq!(
            fs::read(f.local.join("source.bin")).unwrap(),
            b"keep original"
        );
        assert!(partials(&f.local).is_empty());
    });
}

#[test]
fn download_pause_resume_preserves_partial_and_uses_nonzero_offset() {
    let f = Fixture::new();
    let expected = f.source("source.bin");
    f.core.block_on(async {
        let id = f.downloading().await;
        let local = f.transfer(&id).local_path.unwrap();
        xfer::pause(&f.core, &id).unwrap();
        assert_eq!(f.transfer(&id).state, FileTransferState::Paused);
        tokio::time::sleep(Duration::from_millis(80)).await;
        let partial = partials(&f.local);
        assert_eq!(partial.len(), 1);
        assert!(fs::metadata(&partial[0]).unwrap().len() > 0);
        let before = f.server.read_requests().len();
        xfer::resume(&f.core, &id).await.unwrap();
        // Repeated resume must not start another task.
        xfer::resume(&f.core, &id).await.unwrap();
        let transfer = f.completed(&id).await;
        assert_eq!(transfer.local_path.as_deref(), Some(local.as_str()));
        assert_eq!(fs::read(local).unwrap(), expected);
        assert!(f.server.read_requests()[before].offset > 0);
        assert_eq!(f.connector.calls.load(Ordering::SeqCst), 1);
        assert!(partials(&f.local).is_empty());
        assert!(f.states(&id).iter().any(|state| state == "paused"));
    });
}

#[test]
fn download_cancel_after_pause_discards_partials() {
    let f = Fixture::new();
    f.source("source.bin");
    f.core.block_on(async {
        let id = f.downloading().await;
        xfer::pause(&f.core, &id).unwrap();
        assert!(!partials(&f.local).is_empty());
        xfer::cancel(&f.core, &id).unwrap();
        wait_clean(&f.local).await;
        assert_eq!(f.transfer(&id).state, FileTransferState::Cancelled);
        assert!(!f.local.join("source.bin").exists());
    });
}

#[test]
fn running_download_cancel_discards_partials() {
    let f = Fixture::new();
    f.source("source.bin");
    f.core.block_on(async {
        let id = f.downloading().await;
        xfer::cancel(&f.core, &id).unwrap();
        wait_clean(&f.local).await;
        assert_eq!(f.transfer(&id).state, FileTransferState::Cancelled);
        assert!(!f.local.join("source.bin").exists());
    });
}

#[test]
fn download_disconnect_emits_interrupted_and_automatically_reconnects() {
    let f = Fixture::new();
    let expected = f.source("source.bin");
    f.core.block_on(async {
        api_sftp::list(&f.core, "server-a", None).await.unwrap();
        f.server.fail_after_read_bytes(Some(MIB as u64));
        let id = api_sftp::download(&f.core, "server-a", "/home/test/source.bin")
            .await
            .unwrap();
        f.wait(&id, |entry| entry.state == FileTransferState::Interrupted)
            .await;
        let transfer = f.completed(&id).await;
        assert_eq!(fs::read(transfer.local_path.unwrap()).unwrap(), expected);
        assert_eq!(f.connector.calls.load(Ordering::SeqCst), 2);
        assert!(f.states(&id).iter().any(|state| state == "interrupted"));
        assert!(f.server.read_requests()[0].offset > 0);
    });
}

#[test]
fn uploads_eight_mib_home_and_numbers_duplicate_saved_name() {
    let f = Fixture::new();
    let (path, expected) = f.local_source("source.bin");
    f.core.block_on(async {
        let id = api_sftp::upload(&f.core, "server-a", &path, None)
            .await
            .unwrap();
        let transfer = f.completed(&id).await;
        assert_eq!(transfer.remote_path.as_deref(), Some("/home/test"));
        assert_eq!(transfer.saved_name.as_deref(), Some("source.bin"));
        assert_eq!(
            fs::read(f.server.root().join("source.bin")).unwrap(),
            expected
        );
        let second = api_sftp::upload(&f.core, "server-a", &path, None)
            .await
            .unwrap();
        assert_eq!(
            f.transfer(&second).saved_name.as_deref(),
            Some("source (1).bin")
        );
        f.completed(&second).await;
        assert_eq!(
            fs::read(f.server.root().join("source (1).bin")).unwrap(),
            expected
        );
        assert_eq!(
            fs::read(f.server.root().join("source.bin")).unwrap(),
            expected
        );
        assert!(partials(f.server.root()).is_empty());
    });
}

#[test]
fn upload_pause_resume_keeps_saved_name_and_rewrites_from_nonzero_offset() {
    let f = Fixture::new();
    let (path, expected) = f.local_source("source.bin");
    f.core.block_on(async {
        let id = f.uploading(&path).await;
        xfer::pause(&f.core, &id).unwrap();
        assert_eq!(f.transfer(&id).state, FileTransferState::Paused);
        tokio::time::sleep(Duration::from_millis(100)).await;
        assert_eq!(partials(f.server.root()).len(), 1);
        let before = f.server.write_requests().len();
        let initial = f.transfer(&id);
        xfer::resume(&f.core, &id).await.unwrap();
        let transfer = f.completed(&id).await;
        assert_eq!(transfer.saved_name, initial.saved_name);
        assert_eq!(transfer.remote_path, initial.remote_path);
        assert!(f.server.write_requests()[before].offset > 0);
        assert_eq!(
            fs::read(f.server.root().join("source.bin")).unwrap(),
            expected
        );
        assert!(partials(f.server.root()).is_empty());
    });
}

#[test]
fn running_upload_cancel_discards_partial_and_does_not_publish_destination() {
    let f = Fixture::new();
    let (path, _) = f.local_source("source.bin");
    f.core.block_on(async {
        let id = f.uploading(&path).await;
        assert!(!partials(f.server.root()).is_empty());
        xfer::cancel(&f.core, &id).unwrap();
        wait_clean(f.server.root()).await;
        assert_eq!(f.transfer(&id).state, FileTransferState::Cancelled);
        assert!(!f.server.root().join("source.bin").exists());
    });
}

#[test]
fn paused_upload_cancel_uses_remote_discard_helper() {
    let f = Fixture::new();
    let (path, _) = f.local_source("source.bin");
    f.core.block_on(async {
        let id = f.uploading(&path).await;
        xfer::pause(&f.core, &id).unwrap();
        assert!(!partials(f.server.root()).is_empty());
        xfer::cancel(&f.core, &id).unwrap();
        wait_clean(f.server.root()).await;
        assert_eq!(f.transfer(&id).state, FileTransferState::Cancelled);
        assert!(!f.server.root().join("source.bin").exists());
    });
}

#[test]
fn auth_required_export_errors_are_direct_and_create_no_records() {
    let f = Fixture::new();
    f.connector.auth_required.store(true, Ordering::SeqCst);
    runtime::install_core(f.core.clone());
    f.core.block_on(async {
        assert_http(
            api_sftp::sftp_list("server-a".into(), None)
                .await
                .unwrap_err(),
            401,
            "sftp.authRequired",
        );
        assert_http(
            api_sftp::sftp_download("server-a".into(), "/source.bin".into())
                .await
                .unwrap_err(),
            401,
            "sftp.authRequired",
        );
    });
    assert!(xfer::list(&f.core).is_empty());
    runtime::take_core();
}

#[test]
fn upload_local_not_found_is_direct_and_creates_no_record() {
    let f = Fixture::new();
    f.core.block_on(async {
        assert_http(
            api_sftp::upload(
                &f.core,
                "server-a",
                f.local.join("missing").to_str().unwrap(),
                None,
            )
            .await
            .unwrap_err(),
            404,
            "files.localNotFound",
        );
    });
    assert_eq!(f.connector.calls.load(Ordering::SeqCst), 0);
    assert!(xfer::list(&f.core).is_empty());
}

#[test]
fn concurrent_lists_share_one_connection() {
    let f = Fixture::new();
    f.core.block_on(async {
        let requests = (0..8)
            .map(|_| {
                let core = f.core.clone();
                f.core
                    .handle()
                    .spawn(async move { api_sftp::list(&core, "server-a", None).await })
            })
            .collect::<Vec<_>>();
        for request in requests {
            request.await.unwrap().unwrap();
        }
    });
    assert_eq!(f.connector.calls.load(Ordering::SeqCst), 1);
    assert_eq!(f.server.connection_faults().len(), 1);
}

#[test]
fn two_servers_download_concurrently_with_independent_connections() {
    let f = Fixture::new();
    let expected_a = f.source("source.bin");
    let second = Arc::new(
        f.core
            .block_on(SftpTestServer::start_temporary("mobile-sftp-second")),
    );
    let expected_b = vec![92; SIZE];
    fs::write(second.root().join("source.bin"), &expected_b).unwrap();
    let connector = Arc::new(TestConnector::new(HashMap::from([
        ("server-a".into(), f.server.clone()),
        ("server-b".into(), second.clone()),
    ])));
    api_sftp::set_connector(&f.core, connector.clone());
    f.core.block_on(async {
        let (a, b) = tokio::join!(
            api_sftp::download(&f.core, "server-a", "/home/test/source.bin"),
            api_sftp::download(&f.core, "server-b", "/home/test/source.bin"),
        );
        let (a, b) = (a.unwrap(), b.unwrap());
        let (a, b) = tokio::join!(f.completed(&a), f.completed(&b));
        assert_ne!(a.local_path, b.local_path);
        assert_eq!(fs::read(a.local_path.unwrap()).unwrap(), expected_a);
        assert_eq!(fs::read(b.local_path.unwrap()).unwrap(), expected_b);
    });
    assert_eq!(connector.calls.load(Ordering::SeqCst), 2);
    assert_eq!(f.server.connection_faults().len(), 1);
    assert_eq!(second.connection_faults().len(), 1);
}

#[test]
fn ssh_error_mapping_covers_every_variant() {
    use api_sftp::map_ssh_error;
    assert_http(
        map_ssh_error(&SshError::HostKeyRejected),
        401,
        "sftp.hostKeyUnknown",
    );
    assert_http(
        map_ssh_error(&SshError::HostKeyChanged {
            expected: "old".into(),
            got: "new".into(),
        }),
        401,
        "sftp.hostKeyChanged",
    );
    assert_http(
        map_ssh_error(&SshError::AuthFailed {
            tried: vec!["password"],
        }),
        401,
        "sftp.authRequired",
    );
    for error in [
        SshError::Timeout,
        SshError::Transport("offline".into()),
        SshError::Closed,
    ] {
        assert!(
            matches!(map_ssh_error(&error), CoreError::Network { message } if message.starts_with("sftp.offline"))
        );
    }
    for error in [
        SshError::Unsupported("test"),
        SshError::InvalidKey,
        SshError::SigningFailed,
        SshError::Cancelled,
        SshError::InvalidOptions("test"),
        SshError::RequestRejected,
    ] {
        assert!(
            matches!(map_ssh_error(&error), CoreError::Internal { message } if message.starts_with("sftp.internal"))
        );
    }
}

#[test]
fn retry_budget_exhausts_at_interrupted_and_manual_resume_resets_it() {
    let f = Fixture::new();
    let expected = f.source("source.bin");
    f.connector.disconnect_each.store(true, Ordering::SeqCst);
    f.core.block_on(async {
        let started = tokio::time::Instant::now();
        let id = api_sftp::download(&f.core, "server-a", "/home/test/source.bin")
            .await
            .unwrap();
        f.wait(&id, |transfer| {
            transfer.state == FileTransferState::Interrupted
                && f.connector.calls.load(Ordering::SeqCst) == 4
        })
        .await;
        assert!(started.elapsed() >= Duration::from_secs(17));
        tokio::time::sleep(Duration::from_millis(50)).await;
        assert_eq!(f.transfer(&id).state, FileTransferState::Interrupted);
        f.connector.disconnect_each.store(false, Ordering::SeqCst);
        xfer::resume(&f.core, &id).await.unwrap();
        let transfer = f.completed(&id).await;
        assert_eq!(fs::read(transfer.local_path.unwrap()).unwrap(), expected);
        assert_eq!(f.connector.calls.load(Ordering::SeqCst), 5);
    });
}

#[test]
fn upload_disconnect_reconnects_and_resumes_same_destination() {
    let f = Fixture::new();
    let (path, expected) = f.local_source("source.bin");
    f.core.block_on(async {
        api_sftp::list(&f.core, "server-a", None).await.unwrap();
        f.server.fail_after_write_bytes(Some(2 * MIB as u64));
        let id = api_sftp::upload(&f.core, "server-a", &path, None)
            .await
            .unwrap();
        f.wait(&id, |transfer| {
            transfer.state == FileTransferState::Interrupted
        })
        .await;
        let transfer = f.completed(&id).await;
        assert_eq!(transfer.saved_name.as_deref(), Some("source.bin"));
        assert_eq!(
            fs::read(f.server.root().join("source.bin")).unwrap(),
            expected
        );
        assert!(f.server.write_requests()[0].offset > 0);
        assert!(partials(f.server.root()).is_empty());
        assert_eq!(f.connector.calls.load(Ordering::SeqCst), 2);
    });
}

#[test]
fn removing_paused_download_still_cleans_partial_after_row_is_removed() {
    let f = Fixture::new();
    f.source("source.bin");
    f.core.block_on(async {
        let id = f.downloading().await;
        xfer::pause(&f.core, &id).unwrap();
        assert!(!partials(&f.local).is_empty());
        xfer::remove(&f.core, &id).unwrap();
        assert!(f.core.state::<Transfers>().get(&f.core, &id).is_none());
        wait_clean(&f.local).await;
        assert!(!f.local.join("source.bin").exists());
    });
}

#[test]
fn real_connector_reports_unknown_server_without_creating_transfer() {
    let f = Fixture::new();
    api_sftp::set_connector(&f.core, Arc::new(api_sftp::RealConnector));
    f.core.block_on(async {
        assert_http(
            api_sftp::list(&f.core, "missing-server", None)
                .await
                .unwrap_err(),
            404,
            "sftp.serverNotFound",
        );
    });
    assert!(xfer::list(&f.core).is_empty());
}
