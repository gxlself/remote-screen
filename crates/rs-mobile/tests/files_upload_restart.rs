mod support;

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;
use std::time::Duration;

use host_core::config::HostConfig;
use host_fake::faultnet::{Dir, FaultProxy, UdpRule};
use host_fake::harness::{TestHost, TestHostOptions};
use ring::digest::{digest, SHA256};
use rs_mobile::rscore::{
    api_files,
    dto_files::{FileTransfer, FileTransferState as State},
    files_upload as uploads,
    runtime::{install_core, take_core, Core, CoreOptions},
    store::PairedHostRow,
    xfer::{self, Transfers},
};
use rsp_core::trust::Permission;
use rsp_files::recv::partial_info;
use tokio::time::{sleep, timeout};

const MIB: usize = 1024 * 1024;

async fn until(predicate: impl Fn() -> bool) {
    timeout(Duration::from_secs(40), async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

fn transfer(core: &Arc<Core>, id: &str) -> FileTransfer {
    core.state::<Transfers>()
        .get(core, id)
        .expect("transfer exists")
}

fn pair_row(core: &Arc<Core>, host: &TestHost, address: SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: host.fp,
            name: "Upload restart host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .expect("pair row");
}

fn content(size: usize, salt: u8) -> Vec<u8> {
    (0..size)
        .map(|index| index.wrapping_add(salt as usize) as u8)
        .collect()
}

fn sha256(bytes: &[u8]) -> Vec<u8> {
    digest(&SHA256, bytes).as_ref().to_vec()
}

fn upload_json(core: &Arc<Core>) -> serde_json::Map<String, serde_json::Value> {
    let path = core.data_dir().join("uploads.json");
    if !path.exists() {
        return serde_json::Map::new();
    }
    serde_json::from_slice(&std::fs::read(path).expect("uploads registry"))
        .expect("valid uploads registry")
}

struct Fixture {
    core: Arc<Core>,
    signer: Arc<support::TestSigner>,
    host: TestHost,
    proxy: FaultProxy,
    root: PathBuf,
    source: PathBuf,
}

impl Fixture {
    async fn new() -> Self {
        let root = std::fs::canonicalize(support::temp_dir("upload-restart-host"))
            .expect("host directory");
        let core = support::test_core();
        let signer = support::with_identity(&core, "Upload restart device");
        let mut config = HostConfig::default();
        config.files.from_phone = true;
        config.files.browse_roots = vec![root.to_string_lossy().into_owned()];
        let host = TestHost::start_with(TestHostOptions {
            downloads_dir: Some(root.clone()),
            config: Some(config),
            ..Default::default()
        })
        .await;
        host.pair(
            &core.local_identity().expect("local identity"),
            Permission::Control,
        );
        let proxy = FaultProxy::start(host.quic_addr, None, 0x5a11_5eed)
            .await
            .expect("fault proxy");
        proxy.set_udp(
            Dir::ToHost,
            UdpRule {
                rate_kbps: Some(12_000),
                delay_ms: 5,
                ..Default::default()
            },
        );
        pair_row(&core, &host, proxy.addr());
        let source = core.data_dir().join("upload.bin");
        install_core(core.clone());
        Self {
            core,
            signer,
            host,
            proxy,
            root,
            source,
        }
    }

    fn write(&self, bytes: &[u8]) {
        std::fs::write(&self.source, bytes).expect("source file");
    }

    async fn upload(&self) -> String {
        uploads::upload(
            &self.core,
            &self.host.host_id,
            self.source.to_str().expect("source path"),
            None,
        )
        .await
        .expect("queue upload")
    }

    async fn pause_at_two_mib(&self, id: &str) -> u64 {
        until(|| transfer(&self.core, id).done >= 2 * MIB as u64).await;
        xfer::pause(&self.core, id).expect("pause upload");
        until(|| {
            partial_info(&self.root, id)
                .expect("partial info")
                .is_some_and(|partial| partial.received > 0 && partial.received < partial.size)
        })
        .await;
        let done = transfer(&self.core, id).done;
        assert!(done >= 2 * MIB as u64, "paused at {done} bytes");
        done
    }

    fn reopen(&mut self) {
        let data_dir = self.core.data_dir();
        let old = take_core().expect("installed core");
        old.shutdown();
        drop(old);
        drop(std::mem::replace(
            &mut self.core,
            Core::new(
                rs_mobile::rscore::dto_core::CoreConfig {
                    files_dir: None,
                    data_dir: data_dir.to_string_lossy().into_owned(),
                    app_version: "test".into(),
                    platform: rs_mobile::rscore::dto_core::DevicePlatform::Ios,
                },
                CoreOptions {
                    init_logging: false,
                    ..CoreOptions::default()
                },
            )
            .expect("reopen core"),
        ));
        self.core.set_signer(Some(self.signer.clone()));
        pair_row(&self.core, &self.host, self.proxy.addr());
        api_files::register_driver(&self.core);
        install_core(self.core.clone());
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        let _ = take_core();
        self.core.shutdown();
        self.host.abort();
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_resumes_after_core_restart() {
    let mut fixture = Fixture::new().await;
    let bytes = content(8 * MIB, 7);
    fixture.write(&bytes);
    let id = fixture.upload().await;
    let paused_done = fixture.pause_at_two_mib(&id).await;
    let received = partial_info(&fixture.root, &id)
        .expect("partial info")
        .expect("host partial");
    assert!(received.received > 0);
    let host_received_before_restart = received.received;

    fixture.reopen();
    let events = support::EventCollector::new();
    fixture.core.events().subscribe(events.clone());
    fixture.proxy.restore();
    fixture.proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            rate_kbps: Some(1_000),
            delay_ms: 5,
            ..Default::default()
        },
    );
    xfer::resume(&fixture.core, &id)
        .await
        .expect("resume upload");
    until(|| transfer(&fixture.core, &id).state == State::Running).await;
    sleep(Duration::from_millis(100)).await;
    until(|| {
        partial_info(&fixture.root, &id)
            .expect("partial info")
            .is_some_and(|partial| partial.received >= host_received_before_restart)
    })
    .await;
    assert!(
        transfer(&fixture.core, &id).done >= paused_done,
        "restart reset progress to {} from {paused_done}",
        transfer(&fixture.core, &id).done
    );
    fixture.proxy.restore();
    until(|| transfer(&fixture.core, &id).state == State::Completed).await;

    let first_done = events
        .of_type("file.transfer")
        .into_iter()
        .filter(|event| event["transfer"]["transferId"] == id)
        .map(|event| event["transfer"]["done"].as_u64().expect("done"))
        .next()
        .expect("progress event after restart");
    assert!(
        first_done >= paused_done,
        "first post-restart progress was {first_done}, paused at {paused_done}"
    );
    assert_eq!(
        std::fs::read(fixture.root.join("upload.bin")).expect("received file"),
        bytes
    );
    assert_eq!(
        sha256(&std::fs::read(fixture.root.join("upload.bin")).expect("received file")),
        sha256(&bytes)
    );
    let files: Vec<_> = std::fs::read_dir(&fixture.root)
        .expect("receive directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .filter(|name| !name.to_string_lossy().starts_with(".rs-"))
        .collect();
    assert_eq!(files, vec![std::ffi::OsString::from("upload.bin")]);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_source_after_restart_restarts_from_zero() {
    let mut fixture = Fixture::new().await;
    let original = content(8 * MIB, 11);
    fixture.write(&original);
    let id = fixture.upload().await;
    fixture.pause_at_two_mib(&id).await;
    fixture.reopen();
    let replacement = content(8 * MIB, 83);
    sleep(Duration::from_millis(20)).await;
    fixture.write(&replacement);
    fixture.proxy.restore();
    fixture.proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            rate_kbps: Some(1_000),
            delay_ms: 5,
            ..Default::default()
        },
    );
    xfer::resume(&fixture.core, &id)
        .await
        .expect("resume changed upload");
    until(|| {
        transfer(&fixture.core, &id).state == State::Running
            && transfer(&fixture.core, &id).done == 0
    })
    .await;
    fixture.proxy.restore();
    until(|| transfer(&fixture.core, &id).state == State::Completed).await;
    assert_eq!(
        std::fs::read(fixture.root.join("upload.bin")).expect("received file"),
        replacement
    );
    assert!(!fixture.root.join("upload (1).bin").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn terminal_transfer_removes_fingerprint() {
    let fixture = Fixture::new().await;
    let bytes = content(2 * MIB, 19);
    fixture.write(&bytes);
    let completed = fixture.upload().await;
    until(|| transfer(&fixture.core, &completed).state == State::Completed).await;
    assert!(!upload_json(&fixture.core).contains_key(&completed));

    fixture.write(&content(8 * MIB, 29));
    let cancelled = fixture.upload().await;
    fixture.pause_at_two_mib(&cancelled).await;
    xfer::cancel(&fixture.core, &cancelled).expect("cancel upload");
    until(|| transfer(&fixture.core, &cancelled).state == State::Cancelled).await;
    assert!(!upload_json(&fixture.core).contains_key(&cancelled));
}
