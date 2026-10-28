mod support;

use std::net::{SocketAddr, UdpSocket};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_core::config::HostConfig;
use host_fake::faultnet::{Dir, FaultProxy, UdpRule};
use host_fake::harness::{TestHost, TestHostOptions};
use rs_mobile::rscore::{
    api_files::{self as files, FilesConns},
    dto_files::{FileEntryKind, FileTransfer, FileTransferFail, FileTransferState as State},
    error::CoreError,
    runtime::Core,
    store::PairedHostRow,
    xfer::{self, Transfers},
};
use rsp_core::trust::Permission;
use rsp_files::recv::partial_info;
use tokio::time::{sleep, timeout, Instant};

fn pair_row(core: &Arc<Core>, host: &TestHost, address: SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: host.fp,
            name: "Files test host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

async fn until(predicate: impl Fn() -> bool) {
    timeout(Duration::from_secs(25), async {
        while !predicate() {
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("condition before deadline");
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    host: TestHost,
    root: PathBuf,
    local: PathBuf,
    data: PathBuf,
}

impl Fixture {
    async fn new(browse: bool) -> Self {
        let root = std::fs::canonicalize(support::temp_dir("files-host")).unwrap();
        let local = support::temp_dir("files-local").join("received");
        let core = support::test_core_with(|cfg, _| {
            cfg.files_dir = Some(local.to_string_lossy().into_owned())
        });
        support::with_identity(&core, "Files test device");
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        let mut config = HostConfig::default();
        config.files.browse = browse;
        config.files.browse_roots = vec![root.to_string_lossy().into_owned()];
        let host = TestHost::start_with(TestHostOptions {
            tcp: false,
            downloads_dir: Some(root.clone()),
            config: Some(config),
            ..Default::default()
        })
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        pair_row(&core, &host, host.quic_addr);
        let data = core.data_dir();
        Self {
            core,
            events,
            host,
            root,
            local,
            data,
        }
    }

    fn transfer(&self, id: &str) -> FileTransfer {
        self.core.state::<Transfers>().get(&self.core, id).unwrap()
    }

    async fn download(&self, name: &str) -> String {
        files::download(
            &self.core,
            &self.host.host_id,
            self.root.join(name).to_str().unwrap(),
        )
        .await
        .unwrap()
    }

    async fn completed(&self, id: &str) -> FileTransfer {
        until(|| matches!(self.transfer(id).state, State::Completed | State::Failed)).await;
        let t = self.transfer(id);
        assert_eq!(t.state, State::Completed, "{t:?}");
        t
    }

    async fn slow(&self) -> FaultProxy {
        let proxy = FaultProxy::start(self.host.quic_addr, None, 42)
            .await
            .unwrap();
        proxy.set_udp(
            Dir::ToClient,
            UdpRule {
                rate_kbps: Some(12_000),
                delay_ms: 5,
                ..Default::default()
            },
        );
        pair_row(&self.core, &self.host, proxy.addr());
        proxy
    }

    fn write(&self, name: &str, size: usize) -> Vec<u8> {
        let content: Vec<u8> = (0..size).map(|n| (n % 251) as u8).collect();
        std::fs::write(self.root.join(name), &content).unwrap();
        content
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.core.shutdown();
        self.host.abort();
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(self.local.parent().unwrap());
        let _ = std::fs::remove_dir_all(&self.data);
    }
}

fn no_partials(dir: &Path) -> bool {
    !std::fs::read_dir(dir)
        .unwrap()
        .any(|e| e.unwrap().file_name().to_string_lossy().starts_with(".rs-"))
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lists_root_and_maps_entries_and_paths() {
    let f = Fixture::new(true).await;
    std::fs::create_dir(f.root.join("folder")).unwrap();
    f.write("file.bin", 17);
    let listing = files::list(&f.core, &f.host.host_id, None).await.unwrap();
    assert_eq!(listing.path, f.root.to_str().unwrap());
    assert_eq!(listing.parent, None);
    assert!(!listing.truncated);
    assert!(listing.net_path.is_some());
    assert_eq!(listing.entries.len(), 2);
    let dir = listing.entries.iter().find(|e| e.name == "folder").unwrap();
    assert_eq!(dir.kind, FileEntryKind::Dir);
    assert_eq!(dir.size, None);
    assert_eq!(dir.child_count, Some(0));
    let file = listing
        .entries
        .iter()
        .find(|e| e.name == "file.bin")
        .unwrap();
    assert_eq!(file.size, Some(17));
    assert!(file.mtime_us.is_some());
    assert_eq!(file.child_count, None);
    let sub = files::list(
        &f.core,
        &f.host.host_id,
        Some(f.root.join("folder").to_string_lossy().into_owned()),
    )
    .await
    .unwrap();
    assert_eq!(sub.parent.as_deref(), f.root.to_str());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn paginates_2500_and_truncates_12000_at_ten_pages() {
    let f = Fixture::new(true).await;
    for n in 0..2500 {
        std::fs::write(f.root.join(format!("file-{n:05}")), []).unwrap();
    }
    let listing = files::list(&f.core, &f.host.host_id, None).await.unwrap();
    assert_eq!(listing.entries.len(), 2500);
    assert!(!listing.truncated);
    for n in 2500..12000 {
        std::fs::write(f.root.join(format!("file-{n:05}")), []).unwrap();
    }
    let listing = files::list(&f.core, &f.host.host_id, None).await.unwrap();
    assert_eq!(listing.entries.len(), 10000);
    assert!(listing.truncated);
    let names: std::collections::HashSet<_> = listing.entries.iter().map(|e| &e.name).collect();
    assert_eq!(names.len(), 10000);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn maps_outside_root_missing_disabled_and_unpaired_errors() {
    let f = Fixture::new(true).await;
    let outside = f.root.parent().unwrap().to_string_lossy().into_owned();
    assert!(
        matches!(files::list(&f.core, &f.host.host_id, Some(outside)).await, Err(CoreError::Http { status: 403, message, .. }) if message.starts_with("files."))
    );
    assert!(
        matches!(files::list(&f.core, &f.host.host_id, Some(f.root.join("missing").to_string_lossy().into_owned())).await, Err(CoreError::Http { status: 404, message, .. }) if message.starts_with("files.notFound"))
    );
    assert!(
        matches!(files::list(&f.core, "unpaired", None).await, Err(CoreError::Http { status: 403, message, .. }) if message.starts_with("files.notAllowed"))
    );
    let f = Fixture::new(false).await;
    assert!(
        matches!(files::list(&f.core, &f.host.host_id, None).await, Err(CoreError::Unsupported { message }) if message.starts_with("files.disabled"))
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn offline_list_finishes_within_dial_deadline() {
    let f = Fixture::new(true).await;
    let socket = UdpSocket::bind(("127.0.0.1", 0)).unwrap();
    let addr = socket.local_addr().unwrap();
    drop(socket);
    pair_row(&f.core, &f.host, addr);
    let started = Instant::now();
    assert!(
        matches!(files::list(&f.core, &f.host.host_id, None).await, Err(CoreError::Network { message }) if message.starts_with("files.offline"))
    );
    assert!(started.elapsed() < Duration::from_secs(5));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn downloads_8_mib_and_emits_ordered_events_without_partial_files() {
    let f = Fixture::new(true).await;
    let content = f.write("download.bin", 8 * 1024 * 1024);
    let id = f.download("download.bin").await;
    let t = f.completed(&id).await;
    let path = PathBuf::from(t.local_path.unwrap());
    assert!(path.starts_with(&f.local));
    assert_eq!(std::fs::read(path).unwrap(), content);
    assert_eq!(t.done, t.size);
    assert!(no_partials(&f.local));
    until(|| {
        f.events
            .of_type("file.transfer")
            .iter()
            .any(|e| e["transfer"]["transferId"] == id && e["transfer"]["state"] == "completed")
    })
    .await;
    let mut states: Vec<_> = f
        .events
        .of_type("file.transfer")
        .into_iter()
        .filter(|e| e["transfer"]["transferId"] == id)
        .map(|e| e["transfer"]["state"].as_str().unwrap().to_owned())
        .collect();
    states.dedup();
    assert_eq!(&states[..2], ["queued", "running"]);
    assert_eq!(states.last().unwrap(), "completed");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_keeps_partial_and_resume_completes() {
    let f = Fixture::new(true).await;
    let proxy = f.slow().await;
    let content = f.write("pause.bin", 8 * 1024 * 1024);
    let id = f.download("pause.bin").await;
    until(|| f.transfer(&id).done > 128 * 1024).await;
    xfer::pause(&f.core, &id).unwrap();
    assert_eq!(f.transfer(&id).state, State::Paused);
    sleep(Duration::from_millis(200)).await;
    let partial = partial_info(&f.local, &id).unwrap().unwrap();
    assert!(partial.received > 0 && partial.received < partial.size);
    let paused_done = f.transfer(&id).done;
    sleep(Duration::from_millis(200)).await;
    assert_eq!(f.transfer(&id).done, paused_done);
    proxy.restore();
    xfer::resume(&f.core, &id).await.unwrap();
    let t = f.completed(&id).await;
    assert_eq!(std::fs::read(t.local_path.unwrap()).unwrap(), content);
    assert!(no_partials(&f.local));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancelling_receiving_or_paused_download_removes_partials() {
    let f = Fixture::new(true).await;
    let _proxy = f.slow().await;
    f.write("cancel.bin", 8 * 1024 * 1024);
    for paused in [false, true] {
        let id = f.download("cancel.bin").await;
        until(|| f.transfer(&id).done > 128 * 1024).await;
        if paused {
            xfer::pause(&f.core, &id).unwrap();
            sleep(Duration::from_millis(200)).await;
        }
        xfer::cancel(&f.core, &id).unwrap();
        assert_eq!(f.transfer(&id).state, State::Cancelled);
        until(|| no_partials(&f.local)).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnected_download_retries_and_resumes_on_a_new_connection() {
    let f = Fixture::new(true).await;
    let proxy = f.slow().await;
    let content = f.write("retry.bin", 8 * 1024 * 1024);
    let id = f.download("retry.bin").await;
    until(|| f.transfer(&id).done > 128 * 1024).await;
    let conn = f
        .core
        .state::<FilesConns>()
        .get(&f.core, &f.host.host_id)
        .await
        .unwrap();
    proxy.cut();
    // Signal the transport loss immediately; QUIC can otherwise hide a 3 s outage by retransmitting.
    conn.handle.close();
    until(|| f.transfer(&id).state == State::Interrupted).await;
    sleep(Duration::from_secs(3)).await;
    proxy.restore();
    let t = f.completed(&id).await;
    assert_eq!(std::fs::read(t.local_path.unwrap()).unwrap(), content);
    assert!(no_partials(&f.local));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn nonexistent_download_fails_without_retrying() {
    let f = Fixture::new(true).await;
    let id = f.download("missing.bin").await;
    until(|| f.transfer(&id).state == State::Failed).await;
    assert_eq!(f.transfer(&id).fail, Some(FileTransferFail::NotFound));
    sleep(Duration::from_millis(2200)).await;
    let states: Vec<_> = f
        .events
        .of_type("file.transfer")
        .into_iter()
        .filter(|e| e["transfer"]["transferId"] == id)
        .collect();
    let mut transitions: Vec<_> = states
        .iter()
        .map(|e| e["transfer"]["state"].as_str().unwrap())
        .collect();
    transitions.dedup();
    assert_eq!(transitions, ["queued", "running", "failed"]);
    assert!(!states
        .iter()
        .any(|e| e["transfer"]["state"] == "interrupted"));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_callers_share_one_host_connection_and_two_hosts_are_independent() {
    let f = Fixture::new(true).await;
    let g = Fixture::new(true).await;
    g.host
        .pair(&f.core.local_identity().unwrap(), Permission::Control);
    pair_row(&f.core, &g.host, g.host.quic_addr);
    let pool = f.core.state::<FilesConns>();
    let (a, b) = tokio::join!(
        pool.get(&f.core, &f.host.host_id),
        pool.get(&f.core, &f.host.host_id)
    );
    assert!(Arc::ptr_eq(&a.unwrap(), &b.unwrap()));
    let left = f.write("left.bin", 8 * 1024 * 1024);
    let right = g.write("right.bin", 8 * 1024 * 1024);
    let right_path = g.root.join("right.bin");
    let (id1, id2) = tokio::join!(
        f.download("left.bin"),
        files::download(&f.core, &g.host.host_id, right_path.to_str().unwrap())
    );
    let id2 = id2.unwrap();
    let (t1, t2) = tokio::join!(f.completed(&id1), f.completed(&id2));
    assert_eq!(std::fs::read(t1.local_path.unwrap()).unwrap(), left);
    assert_eq!(std::fs::read(t2.local_path.unwrap()).unwrap(), right);
    let (a, b) = tokio::join!(
        pool.get(&f.core, &f.host.host_id),
        pool.get(&f.core, &g.host.host_id)
    );
    assert!(!Arc::ptr_eq(&a.unwrap(), &b.unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn idle_closes_after_60_seconds_but_active_guard_keeps_connection() {
    let f = Fixture::new(true).await;
    let clock = Arc::new(AtomicU64::new(1_000_000));
    let now = clock.clone();
    let c = support::test_core_with(|_, opts| {
        opts.clock = Some(Arc::new(move || now.load(Ordering::Relaxed)))
    });
    support::with_identity(&c, "Idle test device");
    f.host
        .pair(&c.local_identity().unwrap(), Permission::Control);
    pair_row(&c, &f.host, f.host.quic_addr);
    let pool = c.state::<FilesConns>();
    let first = pool.get(&c, &f.host.host_id).await.unwrap();
    let active = first.activity(&c);
    clock.fetch_add(61_000_000, Ordering::Relaxed);
    sleep(Duration::from_millis(1100)).await;
    assert!(Arc::ptr_eq(
        &first,
        &pool.get(&c, &f.host.host_id).await.unwrap()
    ));
    drop(active);
    clock.fetch_add(61_000_000, Ordering::Relaxed);
    sleep(Duration::from_millis(1100)).await;
    let second = pool.get(&c, &f.host.host_id).await.unwrap();
    assert!(!Arc::ptr_eq(&first, &second));
    assert_eq!(first.handle.state(), rsp_client::SessionStateKind::Ended);
    c.shutdown();
    let _ = std::fs::remove_dir_all(c.data_dir());
}

// A small wire peer exposes request counters/offsets that TestHost deliberately keeps private.
use prost::Message;
use rsp_core::{
    features,
    file::{build_header, encode_stream_prefix},
    pb,
};
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    Conn, ControlStream, QuicListener, StreamKind,
};

async fn send(control: &ControlStream, msg: impl Into<pb::Envelope>) {
    control.send(&msg.into().encode_to_vec()).await.unwrap();
}

async fn next(control: &ControlStream) -> pb::envelope::Msg {
    loop {
        let msg = pb::Envelope::decode(control.recv().await.unwrap().unwrap())
            .unwrap()
            .msg
            .unwrap();
        if let pb::envelope::Msg::Ping(p) = msg {
            send(
                control,
                pb::Pong {
                    t1_us: p.t1_us,
                    t2_us: 1,
                },
            )
            .await;
        } else {
            return msg;
        }
    }
}

struct ScriptHost {
    core: Arc<Core>,
    _listener: QuicListener,
    wire: Conn,
    control: ControlStream,
    conn: Arc<files::FilesConn>,
}

impl ScriptHost {
    async fn new() -> Self {
        let identity = LocalIdentity::generate_software(CertKind::Host).unwrap();
        let listener = QuicListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            server_config(&identity).unwrap(),
        )
        .await
        .unwrap();
        let core = support::test_core();
        support::with_identity(&core, "Wire test device");
        core.store()
            .upsert_paired_host(&PairedHostRow {
                host_id: "scripted".into(),
                fp: identity.fingerprint(),
                name: "Wire test host".into(),
                addresses: vec![listener.local_addr().unwrap()],
                paired_at_us: core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
        let pool = core.state::<FilesConns>();
        let (conn, (wire, control)) = tokio::join!(pool.get(&core, "scripted"), async {
            let wire = listener.accept().await.unwrap().accept().await.unwrap();
            let control = wire.accept_control().await.unwrap();
            let pb::envelope::Msg::ClientHello(hello) = next(&control).await else {
                panic!("client hello")
            };
            assert_eq!(hello.intent, pb::Intent::Files as i32);
            assert_eq!(hello.required_features, features::FILE_BROWSE);
            send(
                &control,
                pb::ServerHello {
                    protocol: features::PROTOCOL_VERSION,
                    features: rsp_client::files::FILES_FEATURES,
                    ..Default::default()
                },
            )
            .await;
            send(
                &control,
                pb::AuthResult {
                    status: pb::auth_result::Status::Ok as i32,
                    role: pb::Role::Viewer as i32,
                    ..Default::default()
                },
            )
            .await;
            (wire, control)
        });
        Self {
            core,
            _listener: listener,
            wire,
            control,
            conn: conn.unwrap(),
        }
    }

    async fn download(&self) -> String {
        files::download(&self.core, "scripted", "/download.bin")
            .await
            .unwrap()
    }

    async fn request(&self) -> pb::FileRequest {
        timeout(Duration::from_secs(20), async {
            loop {
                match next(&self.control).await {
                    pb::envelope::Msg::FileRequest(r) => return r,
                    pb::envelope::Msg::FileCancel(_) | pb::envelope::Msg::FileStatus(_) => {}
                    _ => panic!("unexpected control message"),
                }
            }
        })
        .await
        .unwrap()
    }

    fn transfer(&self, id: &str) -> FileTransfer {
        self.core.state::<Transfers>().get(&self.core, id).unwrap()
    }

    async fn file(&self, header: &pb::FileHeader, body: &[u8]) {
        let mut stream = self.wire.open_uni(StreamKind::File).await.unwrap();
        stream
            .write_raw(&encode_stream_prefix(header).unwrap())
            .await
            .unwrap();
        stream
            .write_raw(&body[header.offset as usize..])
            .await
            .unwrap();
        stream.finish().await.unwrap();
    }

    async fn failed(&self, request: &pb::FileRequest, error: pb::FileError) {
        send(
            &self.control,
            pb::FileStatus {
                request_id: request.request_id,
                transfer_id: request.transfer_id.clone(),
                state: pb::file_status::State::Failed as i32,
                error: error as i32,
                ..Default::default()
            },
        )
        .await;
    }
}

impl Drop for ScriptHost {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

fn header(id: &str, body: &[u8]) -> pb::FileHeader {
    let hash: [u8; 32] = ring::digest::digest(&ring::digest::SHA256, body)
        .as_ref()
        .try_into()
        .unwrap();
    build_header(id, "download.bin", body.len() as u64, 0, hash, 123).unwrap()
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_resume_has_nonzero_offset_and_matching_expectations() {
    let h = ScriptHost::new().await;
    let id = h.download().await;
    let request = h.request().await;
    assert_eq!(request.transfer_id, id);
    assert_eq!(request.offset, 0);
    assert!(request.expect_sha256.is_empty());
    let body = vec![37; 8 * 1024 * 1024];
    let mut hdr = header(&id, &body);
    let mut stream = h.wire.as_quic().unwrap().open_uni().await.unwrap();
    stream.write_all(&[StreamKind::File as u8]).await.unwrap();
    stream
        .write_all(&encode_stream_prefix(&hdr).unwrap())
        .await
        .unwrap();
    stream.write_all(&body[..512 * 1024]).await.unwrap();
    let dir = files::files_dir(&h.core).unwrap();
    until(|| {
        partial_info(&dir, &id)
            .unwrap()
            .is_some_and(|p| p.received >= 512 * 1024)
    })
    .await;
    xfer::pause(&h.core, &id).unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), stream.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(2u32.into())
    );
    xfer::resume(&h.core, &id).await.unwrap();
    let resumed = h.request().await;
    assert!(resumed.offset > 0);
    assert_eq!(resumed.expect_size, hdr.size);
    assert_eq!(resumed.expect_sha256, hdr.sha256);
    assert_eq!(resumed.expect_mtime_us, hdr.mtime_us);
    hdr.offset = resumed.offset;
    h.file(&hdr, &body).await;
    until(|| h.transfer(&id).state == State::Completed).await;
    assert_eq!(
        std::fs::read(h.transfer(&id).local_path.unwrap()).unwrap(),
        body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_failure_is_correlated_and_not_found_sends_only_one_request() {
    let h = ScriptHost::new().await;
    let id = h.download().await;
    let request = h.request().await;
    h.failed(&request, pb::FileError::NotFound).await;
    until(|| h.transfer(&id).state == State::Failed).await;
    assert_eq!(h.transfer(&id).fail, Some(FileTransferFail::NotFound));
    assert!(timeout(Duration::from_millis(2200), h.request())
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn source_changed_restarts_once_and_sha_mismatch_fails() {
    let h = ScriptHost::new().await;
    let id = h.download().await;
    let first = h.request().await;
    h.failed(&first, pb::FileError::SourceChanged).await;
    let second = h.request().await;
    assert_eq!(second.offset, 0);
    assert!(second.expect_sha256.is_empty());
    h.failed(&second, pb::FileError::SourceChanged).await;
    until(|| h.transfer(&id).state == State::Failed).await;
    assert_eq!(h.transfer(&id).fail, Some(FileTransferFail::SourceChanged));
    let id = h.download().await;
    h.request().await;
    let body = vec![5; 4096];
    let mut hdr = header(&id, &body);
    hdr.sha256 = vec![0; 32];
    h.file(&hdr, &body).await;
    until(|| h.transfer(&id).state == State::Failed).await;
    assert_eq!(h.transfer(&id).fail, Some(FileTransferFail::Sha256Mismatch));
    assert!(no_partials(&files::files_dir(&h.core).unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn status_waiters_receive_unsolicited_results_and_fail_on_disconnect() {
    let h = ScriptHost::new().await;
    let rx = h.conn.wait_status("upload-id");
    send(
        &h.control,
        pb::FileStatus {
            transfer_id: "upload-id".into(),
            state: pb::file_status::State::Completed as i32,
            ..Default::default()
        },
    )
    .await;
    assert_eq!(
        timeout(Duration::from_secs(2), rx)
            .await
            .unwrap()
            .unwrap()
            .transfer_id,
        "upload-id"
    );
    let rx = h.conn.wait_status("pending-id");
    h.conn.handle.close();
    assert!(timeout(Duration::from_secs(2), rx).await.unwrap().is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn unsolicited_host_push_is_stopped_without_creating_a_file() {
    let h = ScriptHost::new().await;
    let body = vec![7; 4096];
    let hdr = header("11111111111111111111111111111111", &body);
    let mut stream = h.wire.as_quic().unwrap().open_uni().await.unwrap();
    stream.write_all(&[StreamKind::File as u8]).await.unwrap();
    stream
        .write_all(&encode_stream_prefix(&hdr).unwrap())
        .await
        .unwrap();
    assert_eq!(
        timeout(Duration::from_secs(2), stream.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(6u32.into())
    );
    assert!(no_partials(&files::files_dir(&h.core).unwrap()));
    assert!(xfer::list(&h.core).is_empty());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn busy_retries_three_times_with_2_5_10_delays_then_manual_resume_resets_budget() {
    let h = ScriptHost::new().await;
    let id = h.download().await;
    let mut request = h.request().await;
    for delay in [2, 5, 10] {
        let started = Instant::now();
        h.failed(&request, pb::FileError::Busy).await;
        until(|| h.transfer(&id).state == State::Interrupted).await;
        request = h.request().await;
        assert!(started.elapsed() >= Duration::from_secs(delay));
    }
    h.failed(&request, pb::FileError::Busy).await;
    until(|| h.transfer(&id).state == State::Interrupted).await;
    assert!(timeout(Duration::from_millis(200), h.request())
        .await
        .is_err());
    xfer::resume(&h.core, &id).await.unwrap();
    let request = h.request().await;
    h.failed(&request, pb::FileError::Busy).await;
    let request = h.request().await;
    assert_eq!(request.transfer_id, id);
    let body = vec![42; 4096];
    h.file(&header(&id, &body), &body).await;
    until(|| h.transfer(&id).state == State::Completed).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_and_cancel_interrupt_retry_waits_and_manual_resume_replaces_waiter() {
    let h = ScriptHost::new().await;
    let id = h.download().await;
    let request = h.request().await;
    h.failed(&request, pb::FileError::Busy).await;
    until(|| h.transfer(&id).state == State::Interrupted).await;
    xfer::pause(&h.core, &id).unwrap();
    assert!(timeout(Duration::from_millis(2200), h.request())
        .await
        .is_err());
    assert_eq!(h.transfer(&id).state, State::Paused);
    xfer::resume(&h.core, &id).await.unwrap();
    let request = h.request().await;
    h.failed(&request, pb::FileError::Busy).await;
    until(|| h.transfer(&id).state == State::Interrupted).await;
    // Resume immediately during an automatic backoff, without first pausing.
    xfer::resume(&h.core, &id).await.unwrap();
    let request = timeout(Duration::from_millis(500), h.request())
        .await
        .unwrap();
    h.failed(&request, pb::FileError::Busy).await;
    until(|| h.transfer(&id).state == State::Interrupted).await;
    xfer::cancel(&h.core, &id).unwrap();
    assert!(timeout(Duration::from_millis(2200), h.request())
        .await
        .is_err());
    assert_eq!(h.transfer(&id).state, State::Cancelled);
    assert!(no_partials(&files::files_dir(&h.core).unwrap()));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn local_resume_mismatch_discards_partial_and_restarts_from_zero_once() {
    let h = ScriptHost::new().await;
    let id = h.download().await;
    h.request().await;
    let body = vec![9; 1024 * 1024];
    let hdr = header(&id, &body);
    let mut stream = h.wire.as_quic().unwrap().open_uni().await.unwrap();
    stream.write_all(&[StreamKind::File as u8]).await.unwrap();
    stream
        .write_all(&encode_stream_prefix(&hdr).unwrap())
        .await
        .unwrap();
    stream.write_all(&body[..512 * 1024]).await.unwrap();
    let dir = files::files_dir(&h.core).unwrap();
    until(|| {
        partial_info(&dir, &id)
            .unwrap()
            .is_some_and(|p| p.received >= 512 * 1024)
    })
    .await;
    xfer::pause(&h.core, &id).unwrap();
    timeout(Duration::from_secs(2), stream.stopped())
        .await
        .unwrap()
        .unwrap();
    xfer::resume(&h.core, &id).await.unwrap();
    let request = h.request().await;
    assert!(request.offset > 0);
    let mut wrong = hdr.clone();
    wrong.offset = request.offset;
    wrong.mtime_us += 1;
    let mut rejected = h.wire.open_uni(StreamKind::File).await.unwrap();
    rejected
        .write_raw(&encode_stream_prefix(&wrong).unwrap())
        .await
        .unwrap();
    let fresh = h.request().await;
    assert_eq!(fresh.offset, 0);
    assert_eq!(fresh.expect_size, 0);
    assert!(fresh.expect_sha256.is_empty());
    assert!(no_partials(&dir));
    h.file(&hdr, &body).await;
    until(|| h.transfer(&id).state == State::Completed).await;
    assert_eq!(
        std::fs::read(h.transfer(&id).local_path.unwrap()).unwrap(),
        body
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn host_transfer_error_codes_map_to_stable_failures() {
    let h = ScriptHost::new().await;
    for (error, fail) in [
        (
            pb::FileError::PermissionDenied,
            FileTransferFail::PermissionDenied,
        ),
        (
            pb::FileError::OutsideRoot,
            FileTransferFail::PermissionDenied,
        ),
        (pb::FileError::NotAllowed, FileTransferFail::NotAllowed),
        (pb::FileError::Disabled, FileTransferFail::Disabled),
        (pb::FileError::NoSpace, FileTransferFail::NoSpace),
        (pb::FileError::TooLarge, FileTransferFail::TooLarge),
        (pb::FileError::Io, FileTransferFail::Io),
        (pb::FileError::Protocol, FileTransferFail::Protocol),
    ] {
        let id = h.download().await;
        let request = h.request().await;
        h.failed(&request, error).await;
        until(|| h.transfer(&id).state == State::Failed).await;
        assert_eq!(h.transfer(&id).fail, Some(fail));
    }
}
