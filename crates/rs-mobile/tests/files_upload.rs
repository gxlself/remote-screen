mod support;

use std::net::SocketAddr;
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::Duration;

use host_core::config::HostConfig;
use host_fake::faultnet::{Dir, FaultProxy, UdpRule};
use host_fake::harness::{TestHost, TestHostOptions};
use prost::Message;
use rs_mobile::rscore::{
    api_files::FilesConns,
    dto_files::{
        FileTransfer, FileTransferDirection, FileTransferFail as Fail, FileTransferState as State,
    },
    error::CoreError,
    files_upload::{self as uploads, files_upload},
    runtime::{install_core, take_core, Core},
    store::PairedHostRow,
    xfer::{self, Transfers},
};
use rsp_core::{features, file_ctl, pb, trust::Permission};
use rsp_files::recv::partial_info;
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    Conn, ControlStream, QuicListener, StreamKind,
};
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

async fn failed_within(
    core: &Arc<Core>,
    events: &support::EventCollector,
    id: &str,
    duration: Duration,
) -> FileTransfer {
    match timeout(duration, async {
        loop {
            let transfer = transfer(core, id);
            if transfer.state == State::Failed {
                return transfer;
            }
            sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    {
        Ok(transfer) => transfer,
        Err(_) => panic!(
            "transfer did not fail in time: transfer={:?}, states={:?}",
            transfer(core, id),
            states(events, id)
        ),
    }
}

fn transfer(core: &Arc<Core>, id: &str) -> FileTransfer {
    core.state::<Transfers>().get(core, id).unwrap()
}

fn states(events: &support::EventCollector, id: &str) -> Vec<String> {
    let mut states: Vec<_> = events
        .of_type("file.transfer")
        .into_iter()
        .filter(|event| event["transfer"]["transferId"] == id)
        .map(|event| event["transfer"]["state"].as_str().unwrap().to_owned())
        .collect();
    states.dedup();
    states
}

fn pair_row(core: &Arc<Core>, host: &TestHost, address: SocketAddr) {
    core.store()
        .upsert_paired_host(&PairedHostRow {
            host_id: host.host_id.clone(),
            fp: host.fp,
            name: "Upload host".into(),
            addresses: vec![address],
            paired_at_us: core.now_us(),
            permission: Some("control".into()),
        })
        .unwrap();
}

fn content(size: usize) -> Vec<u8> {
    (0..size).map(|index| (index % 251) as u8).collect()
}

fn no_partials(dir: &Path) -> bool {
    !std::fs::read_dir(dir).unwrap().any(|entry| {
        entry
            .unwrap()
            .file_name()
            .to_string_lossy()
            .starts_with(".rs-")
    })
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    host: TestHost,
    root: PathBuf,
    source: PathBuf,
}

impl Fixture {
    async fn new(from_phone: bool) -> Self {
        Self::start(from_phone, false).await
    }

    async fn start(from_phone: bool, tcp: bool) -> Self {
        let root = std::fs::canonicalize(support::temp_dir("upload-host")).unwrap();
        let core = support::test_core();
        support::with_identity(&core, "Upload device");
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        let mut config = HostConfig::default();
        config.files.from_phone = from_phone;
        config.files.browse_roots = vec![root.to_string_lossy().into_owned()];
        let host = TestHost::start_with(TestHostOptions {
            tcp,
            downloads_dir: Some(root.clone()),
            config: Some(config),
            ..Default::default()
        })
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        pair_row(&core, &host, host.quic_addr);
        let source = core.data_dir().join("upload.bin");
        Self {
            core,
            events,
            host,
            root,
            source,
        }
    }

    fn write(&self, size: usize) -> Vec<u8> {
        let bytes = content(size);
        std::fs::write(&self.source, &bytes).unwrap();
        bytes
    }

    async fn upload(&self, remote_dir: Option<String>) -> String {
        uploads::upload(
            &self.core,
            &self.host.host_id,
            self.source.to_str().unwrap(),
            remote_dir,
        )
        .await
        .unwrap()
    }

    async fn completed(&self, id: &str) -> FileTransfer {
        until(|| {
            matches!(
                transfer(&self.core, id).state,
                State::Completed | State::Failed
            )
        })
        .await;
        let transfer = transfer(&self.core, id);
        assert_eq!(transfer.state, State::Completed, "{transfer:?}");
        transfer
    }

    async fn slow(&self) -> FaultProxy {
        let proxy = FaultProxy::start(self.host.quic_addr, None, 42)
            .await
            .unwrap();
        proxy.set_udp(
            Dir::ToHost,
            UdpRule {
                rate_kbps: Some(12_000),
                delay_ms: 5,
                ..Default::default()
            },
        );
        pair_row(&self.core, &self.host, proxy.addr());
        proxy
    }

    async fn paused(&self) -> String {
        let id = self.upload(None).await;
        until(|| {
            std::fs::metadata(self.root.join(format!(".rs-{id}.part")))
                .is_ok_and(|metadata| metadata.len() > 128 * 1024)
        })
        .await;
        xfer::pause(&self.core, &id).unwrap();
        until(|| {
            partial_info(&self.root, &id)
                .unwrap()
                .is_some_and(|p| p.received > 0 && p.received < p.size)
        })
        .await;
        assert_eq!(transfer(&self.core, &id).state, State::Paused);
        id
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        self.core.shutdown();
        self.host.abort();
        let _ = std::fs::remove_dir_all(&self.root);
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn uploads_four_mib_and_waits_for_host_saved_name() {
    let f = Fixture::new(true).await;
    let bytes = f.write(4 * MIB);
    let id = f.upload(None).await;
    let t = f.completed(&id).await;
    assert_eq!(t.direction, FileTransferDirection::Upload);
    assert_eq!(t.saved_name.as_deref(), Some("upload.bin"));
    assert_eq!(t.local_path.as_deref(), f.source.to_str());
    assert_eq!(t.done, bytes.len() as u64);
    assert!(t.net_path.is_some());
    assert_eq!(std::fs::read(f.root.join("upload.bin")).unwrap(), bytes);
    assert_eq!(
        states(&f.events, &id),
        ["queued", "running", "verifying", "completed"]
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn duplicate_upload_gets_numbered_saved_name_and_empty_files_work() {
    let f = Fixture::new(true).await;
    let bytes = f.write(MIB);
    let first = f.upload(None).await;
    f.completed(&first).await;
    let second = f.upload(None).await;
    let t = f.completed(&second).await;
    let name = t.saved_name.unwrap();
    assert_eq!(name, "upload (1).bin");
    assert_eq!(std::fs::read(f.root.join(name)).unwrap(), bytes);
    f.write(0);
    let empty = f.upload(None).await;
    let t = f.completed(&empty).await;
    assert_eq!(
        std::fs::metadata(f.root.join(t.saved_name.unwrap()))
            .unwrap()
            .len(),
        0
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_directory_is_honored_and_outside_root_is_denied() {
    let f = Fixture::new(true).await;
    let bytes = f.write(4 * MIB);
    let sub = f.root.join("sub");
    std::fs::create_dir(&sub).unwrap();
    let id = f.upload(Some(sub.to_str().unwrap().into())).await;
    f.completed(&id).await;
    assert_eq!(std::fs::read(sub.join("upload.bin")).unwrap(), bytes);
    let id = f
        .upload(Some(f.core.data_dir().to_str().unwrap().into()))
        .await;
    until(|| transfer(&f.core, &id).state == State::Failed).await;
    assert_eq!(transfer(&f.core, &id).fail, Some(Fail::PermissionDenied));
    assert!(!f.root.join("upload.bin").exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pause_keeps_host_partial_and_resume_completes() {
    let f = Fixture::new(true).await;
    let proxy = f.slow().await;
    let bytes = f.write(8 * MIB);
    let id = f.paused().await;
    assert!(f.root.join(format!(".rs-{id}.part")).exists());
    let done = transfer(&f.core, &id).done;
    sleep(Duration::from_millis(100)).await;
    assert_eq!(transfer(&f.core, &id).done, done);
    proxy.restore();
    xfer::resume(&f.core, &id).await.unwrap();
    f.completed(&id).await;
    assert_eq!(std::fs::read(f.root.join("upload.bin")).unwrap(), bytes);
    assert!(no_partials(&f.root));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_active_or_paused_upload_deletes_host_partial_and_keeps_source() {
    for paused in [false, true] {
        let f = Fixture::new(true).await;
        let _proxy = f.slow().await;
        let bytes = f.write(8 * MIB);
        let id = if paused {
            f.paused().await
        } else {
            let id = f.upload(None).await;
            until(|| transfer(&f.core, &id).done > 128 * 1024).await;
            id
        };
        xfer::cancel(&f.core, &id).unwrap();
        assert_eq!(transfer(&f.core, &id).state, State::Cancelled);
        until(|| no_partials(&f.root)).await;
        sleep(Duration::from_millis(100)).await;
        assert!(no_partials(&f.root));
        assert!(!f.root.join("upload.bin").exists());
        assert_eq!(std::fs::read(&f.source).unwrap(), bytes);
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn changed_source_during_pause_replaces_host_partial() {
    let f = Fixture::new(true).await;
    let proxy = f.slow().await;
    f.write(8 * MIB);
    let id = f.paused().await;
    let bytes = f.write(5 * MIB);
    proxy.restore();
    xfer::resume(&f.core, &id).await.unwrap();
    let t = f.completed(&id).await;
    assert_eq!(t.size, bytes.len() as u64);
    assert_eq!(std::fs::read(f.root.join("upload.bin")).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disconnected_upload_automatically_retries_after_three_second_outage() {
    let f = Fixture::new(true).await;
    let proxy = f.slow().await;
    let bytes = f.write(8 * MIB);
    let id = f.upload(None).await;
    until(|| transfer(&f.core, &id).done > 128 * 1024).await;
    let conn = f
        .core
        .state::<FilesConns>()
        .get(&f.core, &f.host.host_id)
        .await
        .unwrap();
    proxy.cut();
    conn.handle.close();
    until(|| transfer(&f.core, &id).state == State::Interrupted).await;
    sleep(Duration::from_secs(3)).await;
    proxy.restore();
    f.completed(&id).await;
    assert!(states(&f.events, &id)
        .iter()
        .any(|state| state == "interrupted"));
    assert_eq!(std::fs::read(f.root.join("upload.bin")).unwrap(), bytes);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_host_fails_without_retrying() {
    let f = Fixture::new(false).await;
    f.write(8 * MIB);
    let id = f.upload(None).await;
    until(|| transfer(&f.core, &id).state == State::Failed).await;
    assert_eq!(transfer(&f.core, &id).fail, Some(Fail::Disabled));
    sleep(Duration::from_millis(2200)).await;
    assert_eq!(states(&f.events, &id), ["queued", "running", "failed"]);
    assert!(no_partials(&f.root));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_host_rejects_short_files_before_sending() {
    let f = Fixture::new(false).await;
    for size in [0, 1024, 64 * 1024] {
        f.write(size);
        let id = f.upload(None).await;
        let failed = failed_within(&f.core, &f.events, &id, Duration::from_secs(5)).await;
        assert_eq!(failed.fail, Some(Fail::Disabled), "{failed:?}");
        assert_eq!(failed.done, 0, "{failed:?}");
        assert_eq!(states(&f.events, &id), ["queued", "running", "failed"]);
        sleep(Duration::from_millis(2200)).await;
        assert_eq!(transfer(&f.core, &id).state, State::Failed);
        assert_eq!(transfer(&f.core, &id).fail, Some(Fail::Disabled));
        assert!(std::fs::read_dir(&f.root).unwrap().next().is_none());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn disabled_host_over_tcp_fails_before_sending() {
    let f = Fixture::start(false, true).await;
    let mut attempts = 0;
    let proxy = loop {
        attempts += 1;
        match FaultProxy::start(f.host.quic_addr, f.host.tcp_addr, 0x00f1_4b02).await {
            Ok(proxy) => break proxy,
            Err(error) if error.kind() == std::io::ErrorKind::AddrInUse && attempts < 8 => {
                tokio::task::yield_now().await;
            }
            Err(error) => panic!("start fault proxy after {attempts} attempts: {error}"),
        }
    };
    for dir in [Dir::ToHost, Dir::ToClient] {
        proxy.set_udp(
            dir,
            UdpRule {
                blackhole: true,
                ..Default::default()
            },
        );
    }
    pair_row(&f.core, &f.host, proxy.addr());
    for size in [1024, 8 * MIB] {
        f.write(size);
        let id = f.upload(None).await;
        let failed = failed_within(&f.core, &f.events, &id, Duration::from_secs(8)).await;
        assert!(f.core.events().flush(Duration::from_secs(1)));
        assert_eq!(failed.fail, Some(Fail::Disabled), "{failed:?}");
        assert_eq!(failed.done, 0, "{failed:?}");
        assert_eq!(states(&f.events, &id), ["queued", "running", "failed"]);
        sleep(Duration::from_millis(2200)).await;
        assert_eq!(transfer(&f.core, &id).state, State::Failed);
        assert_eq!(transfer(&f.core, &id).fail, Some(Fail::Disabled));
        assert!(std::fs::read_dir(&f.root).unwrap().next().is_none());
    }
    assert!(proxy.stats().tcp_accepted >= 1, "{:?}", proxy.stats());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn exported_upload_rejects_missing_nonfile_and_bad_path_without_records() {
    let core = support::test_core();
    install_core(core.clone());
    for path in [core.data_dir().join("missing"), core.data_dir()] {
        assert!(
            matches!(files_upload("host".into(), path.to_str().unwrap().into(), None).await,
            Err(CoreError::Http { status: 404, message, .. }) if message == "files.localNotFound")
        );
    }
    let source = core.data_dir().join("source");
    std::fs::write(&source, []).unwrap();
    for dir in ["é".repeat(1025), "bad\0path".into()] {
        assert!(
            matches!(files_upload("host".into(), source.to_str().unwrap().into(), Some(dir)).await,
            Err(CoreError::Http { status: 400, message, .. }) if message == "files.badPath")
        );
    }
    assert!(xfer::list(&core).is_empty());
    take_core();
    core.shutdown();
    let _ = std::fs::remove_dir_all(core.data_dir());
}

// A wire peer records exact stream offsets and injects status/connection failures.
async fn send(control: &ControlStream, msg: impl Into<pb::Envelope>) {
    control.send(&msg.into().encode_to_vec()).await.unwrap();
}

async fn next(control: &ControlStream) -> pb::envelope::Msg {
    timeout(Duration::from_secs(25), async {
        loop {
            let msg = pb::Envelope::decode(control.recv().await.unwrap().unwrap())
                .unwrap()
                .msg
                .unwrap();
            if let pb::envelope::Msg::Ping(ping) = msg {
                send(
                    control,
                    pb::Pong {
                        t1_us: ping.t1_us,
                        t2_us: 1,
                    },
                )
                .await;
            } else {
                return msg;
            }
        }
    })
    .await
    .unwrap()
}

async fn accept(listener: &QuicListener) -> (Conn, ControlStream) {
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
}

struct WireHost {
    core: Arc<Core>,
    listener: QuicListener,
    wire: Conn,
    control: ControlStream,
    source: PathBuf,
    bytes: Vec<u8>,
}

impl WireHost {
    async fn new() -> Self {
        let identity = LocalIdentity::generate_software(CertKind::Host).unwrap();
        let listener = QuicListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            server_config(&identity).unwrap(),
        )
        .await
        .unwrap();
        let core = support::test_core();
        support::with_identity(&core, "Wire upload device");
        core.store()
            .upsert_paired_host(&PairedHostRow {
                host_id: "wire".into(),
                fp: identity.fingerprint(),
                name: "Wire upload host".into(),
                addresses: vec![listener.local_addr().unwrap()],
                paired_at_us: core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
        let pool = core.state::<FilesConns>();
        let (conn, (wire, control)) = tokio::join!(pool.get(&core, "wire"), accept(&listener));
        conn.unwrap();
        let source = core.data_dir().join("wire.bin");
        let bytes = content(8 * MIB);
        std::fs::write(&source, &bytes).unwrap();
        Self {
            core,
            listener,
            wire,
            control,
            source,
            bytes,
        }
    }

    async fn upload(&self) -> String {
        uploads::upload(&self.core, "wire", self.source.to_str().unwrap(), None)
            .await
            .unwrap()
    }

    async fn query(&self) -> pb::FileQuery {
        loop {
            match next(&self.control).await {
                pb::envelope::Msg::FileQuery(query) => return query,
                pb::envelope::Msg::FileCancel(_) => {}
                _ => panic!("unexpected control message"),
            }
        }
    }

    async fn reply(
        &self,
        query: &pb::FileQuery,
        state: pb::file_status::State,
        received: u64,
        error: pb::FileError,
    ) {
        send(
            &self.control,
            pb::FileStatus {
                request_id: query.request_id,
                transfer_id: query.transfer_id.clone(),
                state: state as i32,
                received,
                error: error as i32,
                ..Default::default()
            },
        )
        .await;
    }

    async fn ready(&self, received: u64) {
        let query = self.query().await;
        self.reply(
            &query,
            if received == 0 {
                pb::file_status::State::Unknown
            } else {
                pb::file_status::State::Partial
            },
            received,
            pb::FileError::None,
        )
        .await;
    }

    async fn stream(&self) -> (pb::FileHeader, quinn::RecvStream) {
        loop {
            let mut stream = timeout(
                Duration::from_secs(10),
                self.wire.as_quic().unwrap().accept_uni(),
            )
            .await
            .unwrap()
            .unwrap();
            let mut kind = [0];
            if stream.read_exact(&mut kind).await.is_err() {
                continue;
            }
            assert_eq!(kind[0], StreamKind::File as u8);
            let mut length = [0; 4];
            // SourceChanged can reset a just-opened stream before writing its header.
            if stream.read_exact(&mut length).await.is_err() {
                continue;
            }
            let mut header = vec![0; u32::from_be_bytes(length) as usize];
            stream.read_exact(&mut header).await.unwrap();
            return (pb::FileHeader::decode(header.as_slice()).unwrap(), stream);
        }
    }

    async fn status(&self, id: &str, error: pb::FileError, received: u64) {
        send(
            &self.control,
            pb::FileStatus {
                transfer_id: id.into(),
                state: if error == pb::FileError::None {
                    pb::file_status::State::Completed
                } else {
                    pb::file_status::State::Failed
                } as i32,
                error: error as i32,
                received,
                saved_name: "wire.bin".into(),
                ..Default::default()
            },
        )
        .await;
    }

    async fn status_before_stream_is_opened(&self, id: &str, query: &pb::FileQuery) {
        // A status notification can overtake the query response on the
        // control stream. The client must retain it until the upload attempt
        // consumes it instead of losing it before opening the stream.
        self.status(id, pb::FileError::PermissionDenied, 0).await;
        self.reply(
            query,
            pb::file_status::State::Unknown,
            0,
            pb::FileError::None,
        )
        .await;
        // Depending on which queued event the client observes first, it may
        // consume the status before opening a stream, or open one and then
        // observe the reset. Both orderings must terminate the transfer.
        if let Ok(Ok(mut stream)) = timeout(
            Duration::from_secs(1),
            self.wire.as_quic().unwrap().accept_uni(),
        )
        .await
        {
            let _ = stream.stop(file_ctl::STREAM_REJECTED.into());
        }
    }

    async fn finish(&self, header: &pb::FileHeader, mut stream: quinn::RecvStream) {
        let bytes = stream.read_to_end(16 * MIB).await.unwrap();
        assert_eq!(bytes, self.bytes[header.offset as usize..]);
        until(|| transfer(&self.core, &header.transfer_id).state == State::Verifying).await;
        self.status(&header.transfer_id, pb::FileError::None, header.size)
            .await;
        until(|| transfer(&self.core, &header.transfer_id).state == State::Completed).await;
    }

    async fn paused(&self) -> String {
        let id = self.upload().await;
        self.ready(0).await;
        let (header, mut stream) = self.stream().await;
        assert_eq!(header.offset, 0);
        let mut prefix = vec![0; 128 * 1024];
        stream.read_exact(&mut prefix).await.unwrap();
        xfer::pause(&self.core, &id).unwrap();
        let mut chunk = vec![0; 64 * 1024];
        loop {
            match stream.read(&mut chunk).await {
                Ok(Some(_)) => {}
                Err(quinn::ReadError::Reset(code)) => {
                    assert_eq!(code.into_inner(), 2);
                    break;
                }
                other => panic!("expected PAUSED reset, got {other:?}"),
            }
        }
        id
    }
}

impl Drop for WireHost {
    fn drop(&mut self) {
        self.core.shutdown();
        let _ = std::fs::remove_dir_all(self.core.data_dir());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_resume_queries_host_offset_and_duplicate_resume_opens_one_stream() {
    let h = WireHost::new().await;
    let id = h.paused().await;
    let (first, second) = tokio::join!(xfer::resume(&h.core, &id), xfer::resume(&h.core, &id));
    first.unwrap();
    second.unwrap();
    h.ready(128 * 1024).await;
    let (header, stream) = h.stream().await;
    assert_eq!(header.offset, 128 * 1024);
    assert_eq!(
        header.sha256,
        ring::digest::digest(&ring::digest::SHA256, &h.bytes).as_ref()
    );
    h.finish(&header, stream).await;
    assert!(timeout(
        Duration::from_millis(200),
        h.wire.as_quic().unwrap().accept_uni()
    )
    .await
    .is_err());
    assert!(timeout(Duration::from_millis(200), h.query())
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_changed_size_or_mtime_forces_zero_offset() {
    for same_size in [false, true] {
        let mut h = WireHost::new().await;
        let id = h.paused().await;
        if same_size {
            h.bytes.fill(117);
        } else {
            h.bytes.truncate(5 * MIB);
        }
        std::fs::write(&h.source, &h.bytes).unwrap();
        xfer::resume(&h.core, &id).await.unwrap();
        h.ready(128 * 1024).await;
        let (header, stream) = h.stream().await;
        assert_eq!(header.offset, 0);
        assert_eq!(header.size, h.bytes.len() as u64);
        h.finish(&header, stream).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_cancel_resets_stream_and_sends_file_cancel() {
    let h = WireHost::new().await;
    let id = h.upload().await;
    h.ready(0).await;
    let (_, mut stream) = h.stream().await;
    xfer::cancel(&h.core, &id).unwrap();
    let mut chunk = vec![0; 64 * 1024];
    loop {
        match stream.read(&mut chunk).await {
            Ok(Some(_)) => {}
            Err(quinn::ReadError::Reset(code)) => {
                assert_eq!(code.into_inner(), 3);
                break;
            }
            other => panic!("expected CANCELLED reset, got {other:?}"),
        }
    }
    let pb::envelope::Msg::FileCancel(cancel) = next(&h.control).await else {
        panic!("FileCancel")
    };
    assert_eq!(cancel.transfer_id, id);
    assert_eq!(transfer(&h.core, &id).state, State::Cancelled);
    assert!(h.source.exists());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_disconnect_requeries_and_resumes_at_confirmed_offset() {
    let mut h = WireHost::new().await;
    let id = h.upload().await;
    h.ready(0).await;
    let (_, mut stream) = h.stream().await;
    stream.read_exact(&mut vec![0; 128 * 1024]).await.unwrap();
    h.core
        .state::<FilesConns>()
        .get(&h.core, "wire")
        .await
        .unwrap()
        .handle
        .close();
    until(|| transfer(&h.core, &id).state == State::Interrupted).await;
    (h.wire, h.control) = timeout(Duration::from_secs(10), accept(&h.listener))
        .await
        .unwrap();
    h.ready(128 * 1024).await;
    let (header, stream) = h.stream().await;
    assert_eq!(header.offset, 128 * 1024);
    h.finish(&header, stream).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_policy_failures_are_terminal_and_make_one_query() {
    for (error, fail) in [
        (pb::FileError::Disabled, Fail::Disabled),
        (pb::FileError::NotAllowed, Fail::NotAllowed),
    ] {
        let h = WireHost::new().await;
        let id = h.upload().await;
        let query = h.query().await;
        h.reply(&query, pb::file_status::State::Failed, 0, error)
            .await;
        until(|| transfer(&h.core, &id).state == State::Failed).await;
        assert_eq!(transfer(&h.core, &id).fail, Some(fail));
        assert!(timeout(Duration::from_millis(2200), h.query())
            .await
            .is_err());
        assert!(timeout(
            Duration::from_millis(50),
            h.wire.as_quic().unwrap().accept_uni()
        )
        .await
        .is_err());
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_upload_status_before_stream_is_opened_is_terminal() {
    let h = WireHost::new().await;
    let id = h.upload().await;
    let query = h.query().await;
    h.status_before_stream_is_opened(&id, &query).await;
    let failed = failed_within(
        &h.core,
        &support::EventCollector::new(),
        &id,
        Duration::from_secs(2),
    )
    .await;
    assert_eq!(failed.fail, Some(Fail::PermissionDenied), "{failed:?}");
    assert_eq!(failed.done, 0, "{failed:?}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_sha_mismatch_restarts_once_then_fails() {
    let h = WireHost::new().await;
    let id = h.upload().await;
    for attempt in 0..2 {
        h.ready(if attempt == 0 { 0 } else { 128 * 1024 }).await;
        let (header, mut stream) = h.stream().await;
        assert_eq!(header.offset, 0);
        stream.read_to_end(16 * MIB).await.unwrap();
        h.status(&id, pb::FileError::Sha256Mismatch, 0).await;
    }
    until(|| transfer(&h.core, &id).state == State::Failed).await;
    assert_eq!(transfer(&h.core, &id).fail, Some(Fail::Sha256Mismatch));
    assert!(timeout(Duration::from_millis(200), h.query())
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_resume_mismatch_uses_received_once() {
    let h = WireHost::new().await;
    let id = h.upload().await;
    h.ready(0).await;
    let (_, mut stream) = h.stream().await;
    stream.read_to_end(16 * MIB).await.unwrap();
    h.status(&id, pb::FileError::ResumeMismatch, 128 * 1024)
        .await;
    h.ready(256 * 1024).await;
    let (header, stream) = h.stream().await;
    assert_eq!(header.offset, 128 * 1024);
    h.finish(&header, stream).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_source_changed_between_prepare_and_send_restarts_once() {
    let mut h = WireHost::new().await;
    let id = h.upload().await;
    let query = h.query().await;
    h.bytes.truncate(MIB);
    std::fs::write(&h.source, &h.bytes).unwrap();
    h.reply(
        &query,
        pb::file_status::State::Unknown,
        0,
        pb::FileError::None,
    )
    .await;
    h.ready(128 * 1024).await;
    let (header, stream) = h.stream().await;
    assert_eq!(header.transfer_id, id);
    assert_eq!(header.offset, 0);
    assert_eq!(header.size, MIB as u64);
    h.finish(&header, stream).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_receiving_is_polled_three_times_and_pause_stops_retry_wait() {
    let h = WireHost::new().await;
    let id = h.upload().await;
    for _ in 0..3 {
        let query = h.query().await;
        h.reply(
            &query,
            pb::file_status::State::Receiving,
            0,
            pb::FileError::None,
        )
        .await;
    }
    until(|| transfer(&h.core, &id).state == State::Interrupted).await;
    xfer::pause(&h.core, &id).unwrap();
    assert!(timeout(Duration::from_millis(2200), h.query())
        .await
        .is_err());
    assert_eq!(transfer(&h.core, &id).state, State::Paused);
    xfer::resume(&h.core, &id).await.unwrap();
    h.ready(0).await;
    let (header, stream) = h.stream().await;
    h.finish(&header, stream).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_retries_three_times_then_manual_resume_starts_fresh() {
    let h = WireHost::new().await;
    let id = h.upload().await;
    let mut last = tokio::time::Instant::now();
    for delay in [0, 2, 5, 10] {
        let query = h.query().await;
        if delay > 0 {
            assert!(last.elapsed() + Duration::from_millis(100) >= Duration::from_secs(delay));
        }
        h.reply(
            &query,
            pb::file_status::State::Failed,
            0,
            pb::FileError::Busy,
        )
        .await;
        last = tokio::time::Instant::now();
    }
    until(|| transfer(&h.core, &id).state == State::Interrupted).await;
    assert!(timeout(Duration::from_millis(2200), h.query())
        .await
        .is_err());
    xfer::resume(&h.core, &id).await.unwrap();
    h.ready(0).await;
    let (header, stream) = h.stream().await;
    h.finish(&header, stream).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_repeated_source_change_is_terminal() {
    let mut h = WireHost::new().await;
    let id = h.upload().await;
    for size in [2 * MIB, MIB] {
        let query = h.query().await;
        h.bytes.truncate(size);
        std::fs::write(&h.source, &h.bytes).unwrap();
        h.reply(
            &query,
            pb::file_status::State::Unknown,
            0,
            pb::FileError::None,
        )
        .await;
    }
    until(|| transfer(&h.core, &id).state == State::Failed).await;
    assert_eq!(transfer(&h.core, &id).fail, Some(Fail::SourceChanged));
    assert!(timeout(Duration::from_millis(200), h.query())
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_pause_cancels_pending_query_and_resume_queries_again() {
    let h = WireHost::new().await;
    let id = h.upload().await;
    let first = h.query().await;
    xfer::pause(&h.core, &id).unwrap();
    xfer::resume(&h.core, &id).await.unwrap();
    let second = h.query().await;
    assert_ne!(second.request_id, first.request_id);
    h.reply(
        &second,
        pb::file_status::State::Unknown,
        0,
        pb::FileError::None,
    )
    .await;
    let (header, stream) = h.stream().await;
    h.finish(&header, stream).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_lost_completion_is_recovered_from_completed_query() {
    let mut h = WireHost::new().await;
    let id = h.upload().await;
    h.ready(0).await;
    let (_header, mut stream) = h.stream().await;
    let received = stream.read_to_end(16 * MIB).await.unwrap();
    assert_eq!(received, h.bytes);
    until(|| transfer(&h.core, &id).state == State::Verifying).await;
    h.core
        .state::<FilesConns>()
        .get(&h.core, "wire")
        .await
        .unwrap()
        .handle
        .close();
    until(|| transfer(&h.core, &id).state == State::Interrupted).await;
    (h.wire, h.control) = timeout(Duration::from_secs(10), accept(&h.listener))
        .await
        .unwrap();
    let query = h.query().await;
    assert_eq!(query.transfer_id, id);
    send(
        &h.control,
        pb::FileStatus {
            request_id: query.request_id,
            transfer_id: id.clone(),
            state: pb::file_status::State::Completed as i32,
            received: (8 * MIB) as u64,
            saved_name: "wire (2).bin".into(),
            ..Default::default()
        },
    )
    .await;
    until(|| transfer(&h.core, &id).state == State::Completed).await;
    let transfer = transfer(&h.core, &id);
    assert_eq!(transfer.saved_name.as_deref(), Some("wire (2).bin"));
    assert_eq!(transfer.done, transfer.size);
    assert!(timeout(
        Duration::from_millis(500),
        h.wire.as_quic().unwrap().accept_uni()
    )
    .await
    .is_err());
    assert!(timeout(Duration::from_millis(200), h.query())
        .await
        .is_err());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_completed_query_with_other_size_or_no_name_sends_from_zero() {
    for (received, saved_name) in [((8 * MIB - 1) as u64, "wire.bin"), ((8 * MIB) as u64, "")] {
        let h = WireHost::new().await;
        let id = h.upload().await;
        let query = h.query().await;
        send(
            &h.control,
            pb::FileStatus {
                request_id: query.request_id,
                transfer_id: id.clone(),
                state: pb::file_status::State::Completed as i32,
                received,
                saved_name: saved_name.into(),
                ..Default::default()
            },
        )
        .await;
        let (header, stream) = h.stream().await;
        assert_eq!(header.offset, 0);
        h.finish(&header, stream).await;
        let transfer = transfer(&h.core, &id);
        assert_eq!(transfer.saved_name.as_deref(), Some("wire.bin"));
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_local_change_ignores_completed_query() {
    let mut h = WireHost::new().await;
    let id = h.paused().await;
    h.bytes.fill(117);
    std::fs::write(&h.source, &h.bytes).unwrap();
    xfer::resume(&h.core, &id).await.unwrap();
    let query = h.query().await;
    send(
        &h.control,
        pb::FileStatus {
            request_id: query.request_id,
            transfer_id: id.clone(),
            state: pb::file_status::State::Completed as i32,
            received: (8 * MIB) as u64,
            saved_name: "wire.bin".into(),
            ..Default::default()
        },
    )
    .await;
    let (header, stream) = h.stream().await;
    assert_eq!(header.offset, 0);
    assert_eq!(header.size, (8 * MIB) as u64);
    h.finish(&header, stream).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wire_failed_status_on_open_stream_is_terminal_or_retried() {
    {
        let h = WireHost::new().await;
        let id = h.upload().await;
        h.ready(0).await;
        let (_header, _stream) = h.stream().await;
        h.status(&id, pb::FileError::Disabled, 0).await;
        until(|| transfer(&h.core, &id).state == State::Failed).await;
        assert_eq!(transfer(&h.core, &id).fail, Some(Fail::Disabled));
        assert!(timeout(Duration::from_millis(2500), h.query())
            .await
            .is_err());
    }
    {
        let h = WireHost::new().await;
        let id = h.upload().await;
        h.ready(0).await;
        let (_header, _stream) = h.stream().await;
        h.status(&id, pb::FileError::Busy, 0).await;
        until(|| transfer(&h.core, &id).state == State::Interrupted).await;
        h.ready(0).await;
        let (header, stream) = h.stream().await;
        h.finish(&header, stream).await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn lost_completion_receipt_does_not_duplicate_on_real_host() {
    let f = Fixture::new(true).await;
    let proxy = f.slow().await;
    let conn = f
        .core
        .state::<FilesConns>()
        .get(&f.core, &f.host.host_id)
        .await
        .unwrap();
    proxy.set_udp(
        Dir::ToHost,
        UdpRule {
            delay_ms: 1500,
            ..Default::default()
        },
    );
    let bytes = f.write(64 * 1024);
    let id = f.upload(None).await;
    until(|| transfer(&f.core, &id).state == State::Verifying).await;
    proxy.set_udp(
        Dir::ToClient,
        UdpRule {
            blackhole: true,
            ..Default::default()
        },
    );
    until(|| std::fs::read(f.root.join("upload.bin")).is_ok_and(|b| b == bytes)).await;
    conn.handle.close();
    until(|| transfer(&f.core, &id).state == State::Interrupted).await;
    proxy.set_udp(Dir::ToHost, UdpRule::default());
    proxy.set_udp(Dir::ToClient, UdpRule::default());
    let transfer = f.completed(&id).await;
    assert_eq!(transfer.saved_name.as_deref(), Some("upload.bin"));
    assert_eq!(transfer.done, 64 * 1024);
    let mut entries: Vec<_> = std::fs::read_dir(&f.root)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .collect();
    entries.sort();
    assert_eq!(entries, ["upload.bin"]);
    assert!(no_partials(&f.root));
    assert!(states(&f.events, &id).contains(&"verifying".into()));
    assert!(states(&f.events, &id).contains(&"interrupted".into()));
}
