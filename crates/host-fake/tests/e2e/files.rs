//! F1 acceptance: large-file transfer survives a network cut in both
//! directions, resumes from the peer's confirmed offset, and verifies SHA-256.

use std::future::Future;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, UNIX_EPOCH};

use host_core::clock::Clock;
use host_core::config::HostConfig;
use host_core::file_recv::{self, ReceiveConfig, ReceiveOutcome, RecvError};
use host_core::file_send::{self, PreparedFile, SendOutcome};
use host_fake::faultnet::{Dir, FaultProxy, UdpRule};
use host_fake::harness::{
    app_identity, open, params, wait_end, wait_frames, TestHost, TestHostOptions,
};
use rsp_client::files::{FileCtl, FileReplies, FileReply, FILES_FEATURES};
use rsp_client::session::{
    EndReason, EventStream, IncomingFile, SessionEvent, SessionHandle, SessionParams,
    SessionStateKind,
};
use rsp_core::file_progress::{CancelFlag, TransferProgress};
use rsp_core::{clock::unix_micros, features, pb, trust::Permission};
use rsp_transport::identity::LocalIdentity;
use tokio::sync::{mpsc, Notify};
use tokio::time::{sleep, timeout, Instant};

const MIB: u64 = 1024 * 1024;
const DEFAULT_FILE_MB: u64 = 64;
const CUT_AT_PERCENT: u64 = 40;
const CONNECTION_IDLE_TIMEOUT: Duration = Duration::from_secs(10);
const CONNECTION_IDLE_MARGIN: Duration = Duration::from_secs(5);
const FILES_IDLE_TIMEOUT: Duration = Duration::from_secs(10 * 60);
const QUERY_BUDGET: Duration = Duration::from_secs(15);
const REQUEST_BUDGET: Duration = Duration::from_secs(15);
const PARTIAL_BUDGET: Duration = Duration::from_secs(8);
const FILE_RATE_KBPS: u32 = 1_500_000;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

fn file_mb() -> u64 {
    match std::env::var("RS_E2E_FILE_MB") {
        Ok(value) => value
            .parse::<u64>()
            .unwrap_or_else(|error| panic!("RS_E2E_FILE_MB={value:?} is not a file size: {error}")),
        Err(std::env::VarError::NotPresent) => DEFAULT_FILE_MB,
        Err(error) => panic!("read RS_E2E_FILE_MB: {error}"),
    }
}

fn file_size() -> u64 {
    let mb = file_mb();
    assert!(mb > 0, "RS_E2E_FILE_MB must be positive");
    mb.checked_mul(MIB).expect("RS_E2E_FILE_MB fits u64")
}

fn budget() -> Duration {
    let size = file_size();
    if size > 256 * MIB {
        Duration::from_secs_f64(size as f64 / (20.0 * MIB as f64) + 60.0)
    } else {
        Duration::from_secs(60)
    }
}

async fn within_budget<T>(name: &str, test: impl Future<Output = T>) -> T {
    let limit = budget();
    match timeout(limit, test).await {
        Ok(result) => result,
        Err(_) => panic!("{name} exceeded its {limit:?} budget"),
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rsp-f1k-{label}-{}-{}-{}",
            std::process::id(),
            unix_micros(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        std::fs::create_dir_all(&path)
            .unwrap_or_else(|error| panic!("create {}: {error}", path.display()));
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct Fixture {
    host: TestHost,
    proxy: FaultProxy,
    app: Arc<LocalIdentity>,
    downloads: TempDir,
    root: TempDir,
    phone: TempDir,
}

impl Fixture {
    async fn start() -> Self {
        Self::start_with_clock(None).await
    }

    async fn start_with_clock(services_clock: Option<Arc<dyn Clock>>) -> Self {
        let downloads = TempDir::new("downloads");
        let root = TempDir::new("root");
        let phone = TempDir::new("phone");
        let mut config = HostConfig::default();
        config.files.browse_roots = vec![root.path().to_string_lossy().into_owned()];
        let host = TestHost::start_with(TestHostOptions {
            downloads_dir: Some(downloads.path().to_owned()),
            config: Some(config),
            services_clock,
            ..TestHostOptions::default()
        })
        .await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let mut attempts = 0;
        let proxy = loop {
            attempts += 1;
            match FaultProxy::start(host.quic_addr, host.tcp_addr, 0x00f1_4b01).await {
                Ok(proxy) => break proxy,
                Err(error) if error.kind() == io::ErrorKind::AddrInUse && attempts < 8 => {
                    tokio::task::yield_now().await;
                }
                Err(error) => panic!("start fault proxy after {attempts} attempts: {error}"),
            }
        };
        Self {
            host,
            proxy,
            app,
            downloads,
            root,
            phone,
        }
    }

    fn files_params(&self) -> SessionParams {
        self.files_params_for(self.app.clone())
    }

    fn files_params_for(&self, app: Arc<LocalIdentity>) -> SessionParams {
        let mut params = params(&self.host, app, &[self.proxy.addr()]);
        params.intent = pb::Intent::Files;
        params.features = FILES_FEATURES;
        params.required_features = features::FILE_BROWSE;
        params.dial_options.tcp_after = Duration::from_secs(30);
        params
    }

    fn control_params(&self, app: Arc<LocalIdentity>) -> SessionParams {
        let mut params = params(&self.host, app, &[self.proxy.addr()]);
        params.dial_options.tcp_after = Duration::from_secs(30);
        params
    }

    fn throttle(&self) {
        for dir in [Dir::ToHost, Dir::ToClient] {
            self.proxy.set_udp(
                dir,
                UdpRule {
                    rate_kbps: Some(FILE_RATE_KBPS),
                    ..UdpRule::default()
                },
            );
        }
    }

    async fn finish(self) {
        let Self { host, proxy, .. } = self;
        proxy.shutdown().await;
        host.stop().await;
    }
}

struct ManualClock {
    base_us: u64,
    offset_us: AtomicU64,
}

impl ManualClock {
    fn new() -> Self {
        Self {
            base_us: unix_micros(),
            offset_us: AtomicU64::new(0),
        }
    }

    fn advance(&self, duration: Duration) {
        let micros = u64::try_from(duration.as_micros()).expect("clock advance fits u64");
        self.offset_us.fetch_add(micros, Ordering::SeqCst);
    }
}

impl Clock for ManualClock {
    fn now_micros(&self) -> u64 {
        self.base_us
            .saturating_add(self.offset_us.load(Ordering::SeqCst))
    }
}

fn random_chunk(state: &mut u64, bytes: &mut [u8]) {
    for word in bytes.chunks_mut(8) {
        *state ^= *state << 13;
        *state ^= *state >> 7;
        *state ^= *state << 17;
        *state = state.wrapping_mul(0x2545_f491_4f6c_dd1d);
        word.copy_from_slice(&state.to_le_bytes()[..word.len()]);
    }
}

/// Create `size` deterministic pseudo-random bytes without holding the file in
/// memory, so the same helper covers the 64 MiB CI case and the manual 1 GiB run.
fn write_random(path: &Path, size: u64, seed: u64) -> io::Result<()> {
    if let Some(parent) = path.parent() {
        std::fs::create_dir_all(parent)?;
    }
    let mut file = std::fs::File::create(path)?;
    let mut state = if seed == 0 {
        0x9e37_79b9_7f4a_7c15
    } else {
        seed
    };
    let mut buffer = vec![0_u8; 64 * 1024];
    let mut remaining = size;
    while remaining > 0 {
        let count = usize::try_from(remaining.min(buffer.len() as u64))
            .expect("random chunk length fits usize");
        random_chunk(&mut state, &mut buffer[..count]);
        file.write_all(&buffer[..count])?;
        remaining -= count as u64;
    }
    file.sync_all()
}

async fn prepare(path: &Path) -> PreparedFile {
    file_send::prepare(path, None)
        .await
        .unwrap_or_else(|error| panic!("prepare {}: {error}", path.display()))
}

fn set_mtime(path: &Path, mtime_us: u64) {
    let file = std::fs::OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap_or_else(|error| panic!("open {} to set mtime: {error}", path.display()));
    file.set_modified(UNIX_EPOCH + Duration::from_micros(mtime_us))
        .unwrap_or_else(|error| panic!("set mtime on {}: {error}", path.display()));
}

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = std::fs::read_dir(dir)
        .unwrap_or_else(|error| panic!("list {}: {error}", dir.display()))
        .map(|entry| {
            entry
                .unwrap_or_else(|error| panic!("read {} entry: {error}", dir.display()))
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn assert_no_partials(dir: &Path) {
    let leftovers: Vec<_> = names(dir)
        .into_iter()
        .filter(|name| name.starts_with(".rs-"))
        .collect();
    assert!(
        leftovers.is_empty(),
        "{} still contains partial sidecars: {leftovers:?}",
        dir.display()
    );
}

async fn session_open(params: SessionParams) -> (SessionHandle, EventStream) {
    open(params).await.expect("open FILES session")
}

fn files_session_events(mut source: EventStream, replies: FileReplies) -> EventStream {
    let (tx, rx) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        while let Some(event) = source.recv().await {
            if replies.route(&event) {
                continue;
            }
            if tx.send(event).is_err() {
                break;
            }
        }
        replies.fail_all();
    });
    rx
}

async fn wait_streaming(events: &mut EventStream) {
    timeout(REQUEST_BUDGET, async {
        loop {
            match events.recv().await {
                Some(SessionEvent::StateChanged(SessionStateKind::Streaming)) => return,
                Some(SessionEvent::Ended { code, kind, reason }) => {
                    panic!("FILES ended before Streaming: {code:?} {kind:?} {reason}")
                }
                Some(_) => {}
                None => panic!("FILES event stream closed before Streaming"),
            }
        }
    })
    .await
    .expect("FILES reaches Streaming");
}

async fn files_session(params: SessionParams) -> (SessionHandle, EventStream, FileReplies) {
    let replies = FileReplies::default();
    let (session, events) = session_open(params).await;
    let mut events = files_session_events(events, replies.clone());
    // The client exposes the transport's "Connected" edge as the following
    // Streaming state after AuthResult; there is no separate public variant.
    wait_streaming(&mut events).await;
    (session, events, replies)
}

async fn query_status(
    session: &SessionHandle,
    replies: &FileReplies,
    transfer_id: &str,
) -> pb::FileStatus {
    let request_id = replies.next_id();
    let waiter = replies.wait(request_id);
    session.send_file_ctl(FileCtl::Query(pb::FileQuery {
        request_id,
        transfer_id: transfer_id.to_owned(),
    }));
    match timeout(QUERY_BUDGET, waiter)
        .await
        .expect("FileQuery reply")
        .expect("FileQuery reply channel")
    {
        FileReply::Status(status) => status,
        FileReply::Listing(_) => panic!("FileQuery returned a listing"),
    }
}

async fn wait_partial(
    session: &SessionHandle,
    replies: &FileReplies,
    transfer_id: &str,
    size: u64,
) -> u64 {
    let deadline = Instant::now() + PARTIAL_BUDGET;
    loop {
        let status = query_status(session, replies, transfer_id).await;
        match pb::file_status::State::try_from(status.state) {
            Ok(pb::file_status::State::Partial) => {
                assert!(
                    status.received > 0 && status.received < size,
                    "partial upload length {} is not inside 0..{size}",
                    status.received
                );
                assert_eq!(status.error, pb::FileError::None as i32, "{status:?}");
                return status.received;
            }
            Ok(pb::file_status::State::Receiving | pb::file_status::State::Unknown)
                if Instant::now() < deadline =>
            {
                sleep(Duration::from_millis(25)).await;
            }
            state => panic!(
                "expected FileQuery PARTIAL, got {state:?} received={} error={}",
                status.received, status.error
            ),
        }
    }
}

async fn listing(session: &SessionHandle, replies: &FileReplies, path: &str) -> pb::FileListing {
    let request_id = replies.next_id();
    let waiter = replies.wait(request_id);
    session.send_file_ctl(FileCtl::List(pb::FileList {
        request_id,
        path: path.to_owned(),
        offset: 0,
    }));
    match timeout(REQUEST_BUDGET, waiter)
        .await
        .expect("FileList reply")
        .expect("FileList reply channel")
    {
        FileReply::Listing(listing) => listing,
        FileReply::Status(_) => panic!("FileList returned a status"),
    }
}

async fn request_file(
    session: &SessionHandle,
    events: &mut EventStream,
    replies: &FileReplies,
    request: pb::FileRequest,
) -> IncomingFile {
    let request_id = request.request_id;
    let mut waiter = replies.wait(request_id);
    session.send_file_ctl(FileCtl::Request(request));
    loop {
        tokio::select! {
            reply = &mut waiter => match reply {
                Ok(FileReply::Status(status)) => panic!(
                    "FileRequest failed: state={} error={} received={}",
                    status.state, status.error, status.received
                ),
                Ok(FileReply::Listing(_)) => panic!("FileRequest returned a listing"),
                Err(_) => panic!("FileRequest reply channel closed"),
            },
            event = events.recv() => match event {
                Some(SessionEvent::FileStream(incoming)) => return incoming,
                Some(SessionEvent::Ended { code, kind, reason }) => {
                    panic!("session ended before FileRequest stream: {code:?} {kind:?} {reason}")
                }
                Some(_) => {}
                None => panic!("session event stream closed before FileRequest stream"),
            },
        }
    }
}

async fn send_file_at(
    session: &SessionHandle,
    prepared: &PreparedFile,
    offset: u64,
    proxy: &FaultProxy,
    cut_at: Option<u64>,
) -> (SendOutcome, Option<(u64, Duration)>) {
    let mut stream = session
        .open_file_stream()
        .await
        .expect("open upload file stream");
    let cancel = CancelFlag::new();
    let started = Instant::now();
    let mut cut = None;
    let result = {
        let mut progress = |event: TransferProgress| {
            if cut_at.is_some_and(|threshold| event.done >= threshold) && cut.is_none() {
                proxy.cut();
                cut = Some((event.done, started.elapsed()));
            }
        };
        file_send::send_with(
            prepared,
            offset,
            "",
            &mut stream,
            &mut progress,
            &cancel,
            &unix_micros,
        )
        .await
        .expect("send file stream")
    };
    (result, cut)
}

async fn wait_upload_status(
    events: &mut EventStream,
    transfer_id: &str,
    wanted: pb::file_status::State,
) -> pb::FileStatus {
    loop {
        match events.recv().await {
            Some(SessionEvent::FileStatus(status))
                if status.transfer_id == transfer_id
                    && pb::file_status::State::try_from(status.state) == Ok(wanted) =>
            {
                return status;
            }
            Some(SessionEvent::FileStatus(status)) if status.transfer_id == transfer_id => {
                panic!(
                    "upload status for {transfer_id}: state={} error={} received={}",
                    status.state, status.error, status.received
                );
            }
            Some(SessionEvent::Ended { code, kind, reason }) => {
                panic!("session ended while waiting for upload status: {code:?} {kind:?} {reason}")
            }
            Some(_) => {}
            None => panic!("session events closed while waiting for upload status"),
        }
    }
}

async fn expect_ended(events: &mut EventStream) -> EndReason {
    let limit = CONNECTION_IDLE_TIMEOUT + CONNECTION_IDLE_MARGIN;
    let (code, kind, reason) = wait_end(events, limit)
        .await
        .unwrap_or_else(|| panic!("session did not end within {limit:?}"));
    assert!(
        !matches!(kind, EndReason::Goodbye(_)),
        "network cut ended normally: {code:?} {kind:?} {reason}"
    );
    kind
}

fn report(name: &str, label: &str, bytes: u64, elapsed: Duration, from_offset: u64) {
    let mib = bytes as f64 / MIB as f64;
    let rate = if elapsed.is_zero() {
        0.0
    } else {
        mib / elapsed.as_secs_f64()
    };
    eprintln!(
        "{name}: {label} bytes={bytes} ({mib:.1} MiB) from_offset={from_offset} elapsed={elapsed:?} rate={rate:.2} MiB/s",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn upload_resumes_after_network_cut() {
    let name = "upload_resumes_after_network_cut";
    within_budget(name, async {
        let fixture = Fixture::start().await;
        let size = file_size();
        let source = fixture.phone.path().join("upload.bin");
        write_random(&source, size, 0x00f1_4b11).expect("write upload source");
        let prepared = prepare(&source).await;
        assert_eq!(prepared.size, size);

        let (session, mut events, _replies) = files_session(fixture.files_params()).await;
        fixture.throttle();
        let cut_at = size * CUT_AT_PERCENT / 100;
        let started = Instant::now();
        let (outcome, cut) =
            send_file_at(&session, &prepared, 0, &fixture.proxy, Some(cut_at)).await;
        let (cut_done, cut_elapsed) = cut.expect("upload progress crossed the cut threshold");
        assert!(
            matches!(outcome, SendOutcome::Interrupted { .. }),
            "upload should be interrupted, got {outcome:?}"
        );
        report(name, "first segment", cut_done, cut_elapsed, 0);
        expect_ended(&mut events).await;
        session.close();
        let total_first = started.elapsed();
        eprintln!("{name}: cut detected after {total_first:?}; waiting 10 s QUIC idle timeout");

        fixture.proxy.restore();
        let (resumed, mut resumed_events, resumed_replies) =
            files_session(fixture.files_params()).await;
        let received = wait_partial(&resumed, &resumed_replies, &prepared.transfer_id, size).await;
        eprintln!("{name}: resumed from confirmed offset {received} of {size}");

        let resumed_started = Instant::now();
        let (send, status) = tokio::join!(
            send_file_at(&resumed, &prepared, received, &fixture.proxy, None),
            wait_upload_status(
                &mut resumed_events,
                &prepared.transfer_id,
                pb::file_status::State::Completed
            ),
        );
        let (outcome, _) = send;
        assert!(
            matches!(outcome, SendOutcome::Completed { .. }),
            "resumed upload did not complete: {outcome:?}"
        );
        assert_eq!(status.error, pb::FileError::None as i32, "{status:?}");
        assert!(!status.saved_name.is_empty(), "COMPLETED has saved_name");
        report(
            name,
            "resumed segment",
            size - received,
            resumed_started.elapsed(),
            received,
        );

        let saved = fixture.downloads.path().join(&status.saved_name);
        let saved_prepared = prepare(&saved).await;
        assert_eq!(saved_prepared.sha256, prepared.sha256);
        assert_eq!(saved_prepared.size, size);
        assert_no_partials(fixture.downloads.path());
        resumed.close();
        fixture.finish().await;
    })
    .await;
}

async fn download_first_attempt(
    name: &str,
    fixture: &Fixture,
    source: &Path,
    local: &Path,
) -> (pb::FileHeader, u64, Duration) {
    let size = std::fs::metadata(source).expect("source metadata").len();
    let (session, mut events, replies) = files_session(fixture.files_params()).await;
    let listed = listing(&session, &replies, "").await;
    assert_eq!(listed.error, pb::FileError::None as i32, "{listed:?}");
    let canonical_root = fixture
        .root
        .path()
        .canonicalize()
        .expect("canonical browse root");
    assert_eq!(
        listed.path,
        canonical_root.to_string_lossy(),
        "empty FileList path uses the configured browse root"
    );
    assert!(
        listed.entries.iter().any(|entry| entry.name == "big.bin"),
        "listing does not contain big.bin: {:?}",
        listed
            .entries
            .iter()
            .map(|entry| entry.name.as_str())
            .collect::<Vec<_>>()
    );

    fixture.throttle();
    let request_id = replies.next_id();
    let incoming = request_file(
        &session,
        &mut events,
        &replies,
        pb::FileRequest {
            request_id,
            path: source.to_string_lossy().into_owned(),
            ..Default::default()
        },
    )
    .await;
    let mut incoming = incoming;
    let mut accepted = None;
    let mut cut = None;
    let cfg = ReceiveConfig {
        dest_dir: local.to_owned(),
        ..ReceiveConfig::default()
    };
    let outcome = {
        let started = Instant::now();
        let mut progress = |event: TransferProgress| {
            if event.done >= size * CUT_AT_PERCENT / 100 && cut.is_none() {
                fixture.proxy.cut();
                cut = Some((event.done, started.elapsed()));
            }
        };
        let mut accept = |header: &pb::FileHeader| -> Result<Option<PathBuf>, RecvError> {
            accepted = Some(header.clone());
            Ok(None)
        };
        file_recv::receive_with(
            &mut incoming.0,
            &cfg,
            &mut accept,
            &mut progress,
            &CancelFlag::new(),
            &unix_micros,
        )
        .await
        .expect("receive first download segment")
    };
    let header = accepted.expect("accept recorded the first FileHeader");
    assert_eq!(header.offset, 0);
    assert_eq!(header.size, size);
    assert!(
        matches!(outcome, ReceiveOutcome::Interrupted { .. }),
        "download should be interrupted, got {outcome:?}"
    );
    let (cut_done, cut_elapsed) = cut.expect("download progress crossed the cut threshold");
    report(name, "first segment", cut_done, cut_elapsed, 0);
    expect_ended(&mut events).await;
    session.close();
    (header, cut_done, cut_elapsed)
}

async fn receive_resumed(incoming: IncomingFile, local: &Path) -> (pb::FileHeader, ReceiveOutcome) {
    let mut incoming = incoming;
    let mut accepted = None;
    let cfg = ReceiveConfig {
        dest_dir: local.to_owned(),
        ..ReceiveConfig::default()
    };
    let outcome = {
        let mut progress = |_event: TransferProgress| {};
        let mut accept = |header: &pb::FileHeader| -> Result<Option<PathBuf>, RecvError> {
            accepted = Some(header.clone());
            Ok(None)
        };
        file_recv::receive_with(
            &mut incoming.0,
            &cfg,
            &mut accept,
            &mut progress,
            &CancelFlag::new(),
            &unix_micros,
        )
        .await
        .expect("receive resumed download")
    };
    (accepted.expect("accept recorded resumed header"), outcome)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_resumes_after_network_cut() {
    let name = "download_resumes_after_network_cut";
    within_budget(name, async {
        let fixture = Fixture::start().await;
        let size = file_size();
        let source = fixture.root.path().join("big.bin");
        write_random(&source, size, 0x00f1_4b21).expect("write download source");
        let expected = prepare(&source).await;
        let local = TempDir::new("local-download");

        let (header, cut_done, cut_elapsed) =
            download_first_attempt(name, &fixture, &source, local.path()).await;
        assert_eq!(header.transfer_id.len(), 32);
        assert_eq!(header.sha256, expected.sha256);
        eprintln!(
            "{name}: cut at {cut_done} bytes after {cut_elapsed:?}; waiting 10 s QUIC idle timeout"
        );

        fixture.proxy.restore();
        let (session, mut events, replies) = files_session(fixture.files_params()).await;
        let received = file_recv::partial_received(local.path(), &header.transfer_id)
            .expect("read local partial")
            .expect("local partial exists after interruption");
        assert!(
            received > 0 && received < size,
            "local partial {received}/{size}"
        );
        eprintln!("{name}: resumed from local offset {received} of {size}");

        let request_id = replies.next_id();
        let resumed_started = Instant::now();
        let incoming = request_file(
            &session,
            &mut events,
            &replies,
            pb::FileRequest {
                request_id,
                path: source.to_string_lossy().into_owned(),
                transfer_id: header.transfer_id.clone(),
                offset: received,
                expect_size: header.size,
                expect_mtime_us: header.mtime_us,
                expect_sha256: header.sha256.clone(),
            },
        )
        .await;
        let (resumed_header, outcome) = receive_resumed(incoming, local.path()).await;
        assert_eq!(resumed_header.offset, received);
        let path = match outcome {
            ReceiveOutcome::Completed {
                path, size: got, ..
            } => {
                assert_eq!(got, size);
                path
            }
            other => panic!("resumed download did not complete: {other:?}"),
        };
        report(
            name,
            "resumed segment",
            size - received,
            resumed_started.elapsed(),
            received,
        );
        let actual = prepare(&path).await;
        assert_eq!(actual.sha256, expected.sha256);
        assert_eq!(actual.size, size);
        assert_no_partials(local.path());

        session.send_file_ctl(FileCtl::Status(pb::FileStatus {
            transfer_id: header.transfer_id,
            state: pb::file_status::State::Completed as i32,
            received: size,
            ..Default::default()
        }));
        session.close();
        fixture.finish().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn download_restarts_when_source_changed() {
    let name = "download_restarts_when_source_changed";
    within_budget(name, async {
        let fixture = Fixture::start().await;
        let size = file_size();
        let source = fixture.root.path().join("big.bin");
        write_random(&source, size, 0x00f1_4b31).expect("write original source");
        let original = prepare(&source).await;
        let local = TempDir::new("local-changed");

        let (old_header, cut_done, cut_elapsed) =
            download_first_attempt(name, &fixture, &source, local.path()).await;
        eprintln!(
            "{name}: cut at {cut_done} bytes after {cut_elapsed:?}; replacing the source"
        );

        let replacement = fixture.root.path().join("big.replacement");
        write_random(&replacement, size, 0x00f1_4b32).expect("write replacement source");
        std::fs::rename(&replacement, &source).expect("replace download source");
        set_mtime(&source, original.mtime_us + 1_000_000);
        let changed = prepare(&source).await;
        assert_eq!(changed.size, size);
        assert_ne!(changed.sha256, original.sha256, "replacement bytes differ");
        assert_ne!(
            changed.mtime_us, original.mtime_us,
            "replacement mtime differs"
        );

        fixture.proxy.restore();
        let (session, mut events, replies) = files_session(fixture.files_params()).await;
        let received = file_recv::partial_received(local.path(), &old_header.transfer_id)
            .expect("read old partial")
            .expect("old partial survives the cut");
        assert!(received > 0 && received < size);

        let request_id = replies.next_id();
        let incoming = request_file(
            &session,
            &mut events,
            &replies,
            pb::FileRequest {
                request_id,
                path: source.to_string_lossy().into_owned(),
                transfer_id: old_header.transfer_id.clone(),
                offset: received,
                expect_size: old_header.size,
                expect_mtime_us: old_header.mtime_us,
                expect_sha256: old_header.sha256.clone(),
            },
        )
        .await;
        let (restarted_header, outcome) = receive_resumed(incoming, local.path()).await;
        assert_eq!(
            restarted_header.offset, 0,
            "changed source must restart instead of resuming"
        );
        assert_eq!(restarted_header.sha256, changed.sha256);
        let path = match outcome {
            ReceiveOutcome::Completed { path, size: got, .. } => {
                assert_eq!(got, size);
                path
            }
            other => panic!("restarted download did not complete: {other:?}"),
        };
        let actual = prepare(&path).await;
        assert_eq!(actual.sha256, changed.sha256);
        assert_ne!(actual.sha256, original.sha256);
        assert_no_partials(local.path());
        eprintln!(
            "{name}: cut={cut_done}, resume_requested_offset={received}, header_offset={}, final_sha256={:02x?}",
            restarted_header.offset,
            &actual.sha256[..4],
        );

        session.send_file_ctl(FileCtl::Status(pb::FileStatus {
            transfer_id: old_header.transfer_id,
            state: pb::file_status::State::Completed as i32,
            received: size,
            ..Default::default()
        }));
        session.close();
        fixture.finish().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_session_does_not_disturb_control() {
    let name = "files_session_does_not_disturb_control";
    within_budget(name, async {
        let fixture = Fixture::start().await;
        let size = file_size();
        let source = fixture.phone.path().join("control-upload.bin");
        write_random(&source, size, 0x00f1_4b41).expect("write control upload source");
        let prepared = prepare(&source).await;

        let control_app = app_identity();
        fixture.host.pair(&control_app, Permission::Control);
        let control_params = fixture.control_params(control_app);
        let (session, mut events, _replies) = files_session(fixture.files_params()).await;
        fixture.throttle();

        let started = Arc::new(Notify::new());
        let signal = started.clone();
        let mut progress = move |event: TransferProgress| {
            if event.done >= size / 10 {
                signal.notify_one();
            }
        };
        let cancel = CancelFlag::new();
        let mut stream = session
            .open_file_stream()
            .await
            .expect("open control upload stream");
        let upload = async {
            let outcome = file_send::send_with(
                &prepared,
                0,
                "",
                &mut stream,
                &mut progress,
                &cancel,
                &unix_micros,
            )
            .await
            .expect("send control upload");
            let status = wait_upload_status(
                &mut events,
                &prepared.transfer_id,
                pb::file_status::State::Completed,
            )
            .await;
            (outcome, status)
        };
        let control = async {
            started.notified().await;
            let (handle, mut control_events) = open(control_params)
                .await
                .expect("open CONTROL session during upload");
            let frames = wait_frames(&mut control_events, 3, Duration::from_secs(10)).await;
            handle.close();
            frames
        };
        let ((outcome, status), frames) = tokio::join!(upload, control);
        assert!(
            matches!(outcome, SendOutcome::Completed { .. }),
            "upload was disturbed by control: {outcome:?}"
        );
        assert_eq!(status.error, pb::FileError::None as i32);
        assert!(
            frames.frames >= 3 && frames.ended.is_none() && frames.errors.is_empty(),
            "control session did not receive frames during upload: {frames:?}"
        );
        let saved = fixture.downloads.path().join(status.saved_name);
        let actual = prepare(&saved).await;
        assert_eq!(actual.sha256, prepared.sha256);
        assert_no_partials(fixture.downloads.path());
        eprintln!(
            "{name}: upload bytes={size} control_frames={} first_frame={:?}",
            frames.frames, frames.first_frame
        );
        session.close();
        fixture.finish().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn files_session_idles_out() {
    let name = "files_session_idles_out";
    within_budget(name, async {
        let clock = Arc::new(ManualClock::new());
        let fixture = Fixture::start_with_clock(Some(clock.clone())).await;

        let (session, mut events, _replies) = files_session(fixture.files_params()).await;
        clock.advance(FILES_IDLE_TIMEOUT + Duration::from_secs(1));
        // RequestControl is valid on a FILES session but ignored by the Host.
        // It wakes the control loop so its zero-delay timer observes the jump.
        session.request_control();
        let (code, kind, reason) = timeout(
            Duration::from_secs(2),
            wait_end(&mut events, Duration::from_secs(2)),
        )
        .await
        .expect("Host closes the idle FILES session")
        .expect("idle close event");
        assert_eq!(
            kind,
            EndReason::Goodbye(pb::goodbye::Reason::IdleTimeout),
            "FILES idle close must be Goodbye IDLE_TIMEOUT: {code:?} {reason}"
        );
        assert_eq!(code, None, "idle timeout is a normal protocol ending");
        session.close();

        let (session, mut events, replies) = files_session(fixture.files_params()).await;
        clock.advance(FILES_IDLE_TIMEOUT - Duration::from_secs(60));
        let first = listing(&session, &replies, "").await;
        assert_eq!(first.error, pb::FileError::None as i32, "{first:?}");

        // Nine minutes of the second window have elapsed, but the FileList
        // above was activity. Another nine minutes is still below the limit.
        clock.advance(FILES_IDLE_TIMEOUT - Duration::from_secs(60));
        session.request_control();
        let ended = timeout(Duration::from_millis(250), async {
            loop {
                match events.recv().await {
                    Some(SessionEvent::Ended { code, kind, reason }) => {
                        return Some(format!("{code:?} {kind:?} {reason}"));
                    }
                    Some(_) => {}
                    None => return Some("event stream closed".into()),
                }
            }
        })
        .await;
        assert!(
            ended.is_err(),
            "FILES session closed despite activity within the idle window: {ended:?}"
        );
        let second = listing(&session, &replies, "").await;
        assert_eq!(second.error, pb::FileError::None as i32, "{second:?}");
        eprintln!(
            "{name}: closed after {FILES_IDLE_TIMEOUT:?} idle; activity reset the FileList deadline"
        );
        session.close();
        fixture.finish().await;
    })
    .await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn cancel_upload_removes_partial() {
    let name = "cancel_upload_removes_partial";
    within_budget(name, async {
        let fixture = Fixture::start().await;
        let size = file_size();
        let source = fixture.phone.path().join("cancel.bin");
        write_random(&source, size, 0x00f1_4b51).expect("write cancel upload source");
        let prepared = prepare(&source).await;

        let (session, mut events, _replies) = files_session(fixture.files_params()).await;
        fixture.throttle();
        let cut_at = size * CUT_AT_PERCENT / 100;
        let (outcome, cut) =
            send_file_at(&session, &prepared, 0, &fixture.proxy, Some(cut_at)).await;
        let (cut_done, cut_elapsed) = cut.expect("cancel upload crossed the cut threshold");
        assert!(
            matches!(outcome, SendOutcome::Interrupted { .. }),
            "upload should be interrupted before cancel: {outcome:?}"
        );
        report(name, "first segment", cut_done, cut_elapsed, 0);
        expect_ended(&mut events).await;
        session.close();

        fixture.proxy.restore();
        let (resumed, mut resumed_events, replies) = files_session(fixture.files_params()).await;
        let received = wait_partial(&resumed, &replies, &prepared.transfer_id, size).await;
        eprintln!("{name}: cancelling partial at {received} of {size}");
        resumed.send_file_ctl(FileCtl::Cancel(pb::FileCancel {
            transfer_id: prepared.transfer_id.clone(),
        }));
        timeout(QUERY_BUDGET, async {
            let status = wait_upload_status(
                &mut resumed_events,
                &prepared.transfer_id,
                pb::file_status::State::Cancelled,
            )
            .await;
            assert_eq!(status.error, pb::FileError::None as i32, "{status:?}");
        })
        .await
        .expect("FileCancel reply");
        assert_no_partials(fixture.downloads.path());
        assert!(
            !fixture.downloads.path().join(&prepared.name).exists(),
            "cancelled upload must not publish a file"
        );
        resumed.close();
        fixture.finish().await;
    })
    .await;
}
