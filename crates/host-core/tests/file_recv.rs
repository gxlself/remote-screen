use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use host_core::file_recv::{
    cleanup_stale, partial_received, receive, FileSource, ReceiveConfig, ReceiveOutcome, RecvError,
};
use ring::digest;
use rsp_core::file::{build_header, encode_stream_prefix, FileError};
use rsp_core::file_name::{partial_meta_name, partial_name};
use rsp_core::file_progress::{CancelFlag, Direction, TransferProgress, TransferState};
use rsp_core::pb::FileHeader;
use rsp_transport::ConnError;

const ID: &str = "00112233445566778899aabbccddeeff";
const OTHER_ID: &str = "ffeeddccbbaa99887766554433221100";
const MTIME_US: u64 = 1_735_689_600_000_000;
const MIB: usize = 1024 * 1024;

static NEXT_TEMP_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir {
    path: PathBuf,
}

impl TestDir {
    fn new() -> Self {
        let serial = NEXT_TEMP_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock is after the epoch")
            .as_nanos();
        let path = std::env::temp_dir().join(format!(
            "rs-file-recv-{}-{nanos}-{serial}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("test directory is created");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn sha256(data: &[u8]) -> [u8; 32] {
    digest::digest(&digest::SHA256, data)
        .as_ref()
        .try_into()
        .expect("SHA-256 is 32 bytes")
}

fn make_header(name: &str, size: u64, offset: u64, sha256: [u8; 32], mtime_us: u64) -> FileHeader {
    build_header(ID, name, size, offset, sha256, mtime_us).expect("test header is valid")
}

fn stream(header: &FileHeader, bytes: &[u8]) -> Vec<u8> {
    let mut stream = encode_stream_prefix(header).expect("test header encodes");
    stream.extend_from_slice(bytes);
    stream
}

fn stream_prefix_len(header: &FileHeader) -> usize {
    let prefix = encode_stream_prefix(header).expect("test header encodes");
    let header_len = u32::from_be_bytes(
        prefix[..4]
            .try_into()
            .expect("the stream starts with a four-byte length"),
    ) as usize;
    4 + header_len
}

fn config(dest_dir: &Path) -> ReceiveConfig {
    ReceiveConfig {
        dest_dir: dest_dir.to_owned(),
        ..ReceiveConfig::default()
    }
}

fn names(path: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(path)
        .expect("directory can be listed")
        .map(|entry| {
            entry
                .expect("directory entry can be read")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

fn has_partial_leftovers(path: &Path) -> bool {
    names(path).iter().any(|name| name.starts_with(".rs-"))
}

fn random_data(len: usize, seed: u64) -> Vec<u8> {
    let mut state = seed;
    let mut out = Vec::with_capacity(len);
    while out.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        out.extend_from_slice(&state.to_le_bytes());
    }
    out.truncate(len);
    out
}

fn random_chunks(seed: u64, count: usize) -> Vec<usize> {
    let mut state = seed;
    (0..count)
        .map(|_| {
            state ^= state << 13;
            state ^= state >> 7;
            state ^= state << 17;
            1 + (state as usize % (96 * 1024))
        })
        .collect()
}

#[derive(Clone, Copy)]
enum StopBehavior {
    Error,
    Fin,
    Cancel,
}

struct ScriptedSource {
    stream: Vec<u8>,
    cursor: usize,
    chunks: Vec<usize>,
    next_chunk: usize,
    stop_at: Option<usize>,
    behavior: Option<StopBehavior>,
    cancel: Option<CancelFlag>,
    triggered: bool,
}

impl ScriptedSource {
    fn new(stream: Vec<u8>) -> Self {
        Self {
            stream,
            cursor: 0,
            chunks: vec![64 * 1024],
            next_chunk: 0,
            stop_at: None,
            behavior: None,
            cancel: None,
            triggered: false,
        }
    }

    fn with_chunks(mut self, chunks: Vec<usize>) -> Self {
        assert!(!chunks.is_empty());
        self.chunks = chunks;
        self
    }

    fn stop_after(mut self, offset: usize, behavior: StopBehavior) -> Self {
        self.stop_at = Some(offset);
        self.behavior = Some(behavior);
        self
    }

    fn cancel_after(mut self, offset: usize, cancel: CancelFlag) -> Self {
        self.stop_at = Some(offset);
        self.behavior = Some(StopBehavior::Cancel);
        self.cancel = Some(cancel);
        self
    }

    fn next_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, ConnError> {
        if let Some(stop_at) = self.stop_at {
            if self.cursor >= stop_at && !self.triggered {
                match self.behavior {
                    Some(StopBehavior::Error) => {
                        self.triggered = true;
                        return Err(ConnError::Closed);
                    }
                    Some(StopBehavior::Fin) => {
                        self.triggered = true;
                        return Ok(None);
                    }
                    Some(StopBehavior::Cancel) | None => {}
                }
            }
        }

        if self.cursor == self.stream.len() {
            return Ok(None);
        }
        if buf.is_empty() {
            return Ok(Some(0));
        }

        let mut available = buf.len().min(self.stream.len() - self.cursor);
        if let Some(stop_at) = self.stop_at {
            if self.cursor < stop_at {
                available = available.min(stop_at - self.cursor);
            }
        }
        if available == 0 {
            if matches!(self.behavior, Some(StopBehavior::Cancel)) {
                if let Some(cancel) = self.cancel.as_ref() {
                    cancel.cancel();
                }
            }
            return Ok(Some(0));
        }

        let requested = self.chunks[self.next_chunk % self.chunks.len()];
        self.next_chunk += 1;
        let count = requested.min(available).max(1);
        buf[..count].copy_from_slice(&self.stream[self.cursor..self.cursor + count]);
        self.cursor += count;

        if matches!(self.behavior, Some(StopBehavior::Cancel)) {
            if let (Some(stop_at), Some(cancel)) = (self.stop_at, self.cancel.as_ref()) {
                if self.cursor >= stop_at {
                    cancel.cancel();
                }
            }
        }
        Ok(Some(count))
    }
}

impl FileSource for ScriptedSource {
    fn read(
        &mut self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<Option<usize>, ConnError>> + Send {
        std::future::ready(self.next_read(buf))
    }
}

async fn receive_plain(
    source: &mut ScriptedSource,
    cfg: &ReceiveConfig,
) -> Result<ReceiveOutcome, RecvError> {
    let cancel = CancelFlag::new();
    receive_with_cancel(source, cfg, &cancel).await
}

async fn receive_with_cancel(
    source: &mut ScriptedSource,
    cfg: &ReceiveConfig,
    cancel: &CancelFlag,
) -> Result<ReceiveOutcome, RecvError> {
    let mut progress = |_event: TransferProgress| {};
    let now = || MTIME_US;
    receive(source, cfg, &mut progress, cancel, &now).await
}

fn completed(outcome: ReceiveOutcome) -> (String, PathBuf, u64) {
    match outcome {
        ReceiveOutcome::Completed {
            transfer_id,
            path,
            size,
        } => (transfer_id, path, size),
        other => panic!("expected Completed, got {other:?}"),
    }
}

fn interrupted(outcome: ReceiveOutcome) -> (String, u64) {
    match outcome {
        ReceiveOutcome::Interrupted {
            transfer_id,
            received,
        } => (transfer_id, received),
        other => panic!("expected Interrupted, got {other:?}"),
    }
}

#[tokio::test]
async fn completes_large_random_file_with_metadata() {
    let dir = TestDir::new();
    let data = random_data(3 * MIB, 0x1234_5678_9abc_def0);
    let header = make_header("报告.pdf", data.len() as u64, 0, sha256(&data), MTIME_US);
    let mut source = ScriptedSource::new(stream(&header, &data)).with_chunks(random_chunks(7, 128));
    let cfg = config(dir.path());

    let (transfer_id, path, size) = completed(
        receive_plain(&mut source, &cfg)
            .await
            .expect("receive succeeds"),
    );

    assert_eq!(transfer_id, ID);
    assert_eq!(size, data.len() as u64);
    assert_eq!(path, dir.path().join("报告.pdf"));
    assert_eq!(fs::read(&path).expect("received file is readable"), data);
    let metadata = fs::metadata(&path).expect("received file has metadata");
    assert_eq!(
        metadata
            .modified()
            .expect("modification time is available")
            .duration_since(UNIX_EPOCH)
            .expect("modification time is after the epoch")
            .as_secs(),
        MTIME_US / 1_000_000
    );
    assert!(!has_partial_leftovers(dir.path()));
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        assert_eq!(metadata.permissions().mode() & 0o777, 0o644);
    }
}

#[tokio::test]
async fn collision_uses_numbered_names_without_overwriting() {
    let dir = TestDir::new();
    let existing = dir.path().join("report.pdf");
    fs::write(&existing, b"old").expect("existing file is written");

    for (index, data) in [b"new one".as_slice(), b"new two".as_slice()]
        .into_iter()
        .enumerate()
    {
        let header = make_header("report.pdf", data.len() as u64, 0, sha256(data), MTIME_US);
        let mut source = ScriptedSource::new(stream(&header, data));
        let (_, path, _) = completed(
            receive_plain(&mut source, &config(dir.path()))
                .await
                .expect("receive succeeds"),
        );
        let expected = format!("report ({}).pdf", index + 1);
        assert_eq!(
            path.file_name().and_then(|name| name.to_str()),
            Some(expected.as_str())
        );
        assert_eq!(fs::read(&path).expect("new file is readable"), data);
    }

    assert_eq!(fs::read(&existing).expect("old file is readable"), b"old");
}

#[tokio::test]
async fn path_traversal_names_are_sanitized_inside_destination() {
    let root = TestDir::new();
    let dest = root.path().join("downloads");
    fs::create_dir(&dest).expect("download directory is created");
    let cases = [
        ("../../etc/passwd", "passwd"),
        ("/abs/x.txt", "x.txt"),
        ("..\\..\\win.ini", "win.ini"),
        ("..", "file"),
        (".ssh", "_.ssh"),
        ("con.txt", "_con.txt"),
    ];

    for (name, expected) in cases {
        let data = format!("payload for {name}").into_bytes();
        let header = make_header(name, data.len() as u64, 0, sha256(&data), MTIME_US);
        let mut source = ScriptedSource::new(stream(&header, &data));
        let (_, path, _) = completed(
            receive_plain(&mut source, &config(&dest))
                .await
                .expect("receive succeeds"),
        );

        assert_eq!(path.parent(), Some(dest.as_path()));
        assert_eq!(
            path.file_name().and_then(|component| component.to_str()),
            Some(expected)
        );
        assert_eq!(fs::read(&path).expect("received file is readable"), data);
    }

    assert_eq!(names(root.path()), vec!["downloads"]);
    assert!(!has_partial_leftovers(&dest));
}

#[tokio::test]
async fn transport_error_interrupts_and_resumes_from_received_offset() {
    let dir = TestDir::new();
    let data = random_data(3 * MIB, 0x1111_2222_3333_4444);
    let header = make_header("resume.bin", data.len() as u64, 0, sha256(&data), MTIME_US);
    let mut first = ScriptedSource::new(stream(&header, &data))
        .with_chunks(vec![17, 4096, 100_000])
        .stop_after(stream_prefix_len(&header) + MIB, StopBehavior::Error);

    let (transfer_id, received) = interrupted(
        receive_plain(&mut first, &config(dir.path()))
            .await
            .expect("interruption is not an error"),
    );
    assert_eq!(transfer_id, ID);
    assert_eq!(received, MIB as u64);

    let part = dir.path().join(partial_name(ID).expect("valid test id"));
    let meta = dir
        .path()
        .join(partial_meta_name(ID).expect("valid test id"));
    assert!(part.is_file());
    assert!(meta.is_file());
    assert_eq!(fs::metadata(&part).expect("part exists").len(), MIB as u64);
    assert_eq!(
        partial_received(dir.path(), ID).expect("partial state is readable"),
        Some(MIB as u64)
    );

    let resumed = make_header(
        "resume.bin",
        data.len() as u64,
        MIB as u64,
        sha256(&data),
        MTIME_US,
    );
    let mut second = ScriptedSource::new(stream(&resumed, &data[MIB..]));
    let (_, path, size) = completed(
        receive_plain(&mut second, &config(dir.path()))
            .await
            .expect("resume succeeds"),
    );

    assert_eq!(size, data.len() as u64);
    assert_eq!(fs::read(&path).expect("received file is readable"), data);
    assert!(!has_partial_leftovers(dir.path()));
}

#[tokio::test]
async fn premature_fin_interrupts_and_can_resume() {
    let dir = TestDir::new();
    let data = random_data(2 * MIB, 0x5555_6666_7777_8888);
    let header = make_header(
        "truncated.bin",
        data.len() as u64,
        0,
        sha256(&data),
        MTIME_US,
    );
    let mut first = ScriptedSource::new(stream(&header, &data))
        .with_chunks(vec![8192, 64 * 1024])
        .stop_after(stream_prefix_len(&header) + MIB, StopBehavior::Fin);

    let (_, received) = interrupted(
        receive_plain(&mut first, &config(dir.path()))
            .await
            .expect("truncated FIN is not an error"),
    );
    assert_eq!(received, MIB as u64);
    assert_eq!(
        partial_received(dir.path(), ID).expect("partial state is readable"),
        Some(MIB as u64)
    );

    let resumed = make_header(
        "truncated.bin",
        data.len() as u64,
        MIB as u64,
        sha256(&data),
        MTIME_US,
    );
    let mut second = ScriptedSource::new(stream(&resumed, &data[MIB..]));
    let (_, path, _) = completed(
        receive_plain(&mut second, &config(dir.path()))
            .await
            .expect("resume succeeds"),
    );
    assert_eq!(fs::read(path).expect("received file is readable"), data);
}

#[tokio::test]
async fn resume_mismatch_reports_have_and_leaves_part_unchanged() {
    let dir = TestDir::new();
    let data = random_data(3 * MIB, 0x9999_aaaa_bbbb_cccc);
    let header = make_header(
        "mismatch.bin",
        data.len() as u64,
        0,
        sha256(&data),
        MTIME_US,
    );
    let mut first = ScriptedSource::new(stream(&header, &data))
        .stop_after(stream_prefix_len(&header) + MIB, StopBehavior::Error);
    interrupted(
        receive_plain(&mut first, &config(dir.path()))
            .await
            .expect("interruption is not an error"),
    );

    let part = dir.path().join(partial_name(ID).expect("valid test id"));
    let before = fs::read(&part).expect("partial bytes are readable");
    let too_far = make_header(
        "mismatch.bin",
        data.len() as u64,
        (2 * MIB) as u64,
        sha256(&data),
        MTIME_US,
    );
    let mut second = ScriptedSource::new(stream(&too_far, &data[2 * MIB..]));
    assert!(matches!(
        receive_plain(&mut second, &config(dir.path())).await,
        Err(RecvError::ResumeMismatch { have }) if have == MIB as u64
    ));
    assert_eq!(fs::read(&part).expect("partial bytes are readable"), before);

    let mut wrong_sha = sha256(&data);
    wrong_sha[0] ^= 0x80;
    let changed_source = make_header(
        "mismatch.bin",
        data.len() as u64,
        MIB as u64,
        wrong_sha,
        MTIME_US,
    );
    let mut third = ScriptedSource::new(stream(&changed_source, &data[MIB..]));
    assert!(matches!(
        receive_plain(&mut third, &config(dir.path())).await,
        Err(RecvError::ResumeMismatch { have: 0 })
    ));
    assert_eq!(fs::read(&part).expect("partial bytes are readable"), before);
}

#[tokio::test]
async fn resume_truncates_part_that_is_longer_than_offset() {
    let dir = TestDir::new();
    let data = random_data(4 * MIB, 0xdddd_eeee_ffff_0001);
    let header = make_header(
        "long-part.bin",
        data.len() as u64,
        0,
        sha256(&data),
        MTIME_US,
    );
    let mut first = ScriptedSource::new(stream(&header, &data))
        .with_chunks(vec![64 * 1024])
        .stop_after(stream_prefix_len(&header) + 3 * MIB, StopBehavior::Error);
    interrupted(
        receive_plain(&mut first, &config(dir.path()))
            .await
            .expect("interruption is not an error"),
    );
    assert_eq!(
        partial_received(dir.path(), ID).expect("partial state is readable"),
        Some((3 * MIB) as u64)
    );

    let resumed = make_header(
        "long-part.bin",
        data.len() as u64,
        (2 * MIB) as u64,
        sha256(&data),
        MTIME_US,
    );
    let mut second = ScriptedSource::new(stream(&resumed, &data[2 * MIB..]));
    let (_, path, _) = completed(
        receive_plain(&mut second, &config(dir.path()))
            .await
            .expect("resume succeeds"),
    );
    assert_eq!(fs::read(path).expect("received file is readable"), data);
    assert!(!has_partial_leftovers(dir.path()));
}

#[tokio::test]
async fn offset_zero_replaces_an_old_partial() {
    let dir = TestDir::new();
    let part = dir.path().join(partial_name(ID).expect("valid test id"));
    fs::write(&part, b"stale bytes").expect("old partial is written");
    let data = random_data(128 * 1024, 0x0002_0003_0004_0005);
    let header = make_header("fresh.bin", data.len() as u64, 0, sha256(&data), MTIME_US);
    let mut source = ScriptedSource::new(stream(&header, &data));

    let (_, path, _) = completed(
        receive_plain(&mut source, &config(dir.path()))
            .await
            .expect("fresh receive succeeds"),
    );
    assert_eq!(fs::read(path).expect("received file is readable"), data);
    assert!(!has_partial_leftovers(dir.path()));
}

#[tokio::test]
async fn sha_mismatch_removes_partial_metadata() {
    let dir = TestDir::new();
    let data = random_data(256 * 1024, 0x0006_0007_0008_0009);
    let header = make_header("corrupt.bin", data.len() as u64, 0, [0_u8; 32], MTIME_US);
    let mut source = ScriptedSource::new(stream(&header, &data));

    assert!(matches!(
        receive_plain(&mut source, &config(dir.path())).await,
        Err(RecvError::Sha256Mismatch)
    ));
    assert!(!dir.path().join("corrupt.bin").exists());
    assert!(!dir
        .path()
        .join(partial_name(ID).expect("valid test id"))
        .exists());
    assert!(!dir
        .path()
        .join(partial_meta_name(ID).expect("valid test id"))
        .exists());
    assert!(names(dir.path()).is_empty());
}

#[tokio::test]
async fn cancellation_after_progress_removes_partial_files() {
    let dir = TestDir::new();
    let data = random_data(2 * MIB, 0x000a_000b_000c_000d);
    let header = make_header("cancel.bin", data.len() as u64, 0, sha256(&data), MTIME_US);
    let cancel = CancelFlag::new();
    let mut source = ScriptedSource::new(stream(&header, &data))
        .with_chunks(vec![32 * 1024])
        .cancel_after(stream_prefix_len(&header) + MIB, cancel.clone());

    assert!(matches!(
        receive_with_cancel(&mut source, &config(dir.path()), &cancel).await,
        Ok(ReceiveOutcome::Cancelled { transfer_id }) if transfer_id == ID
    ));
    assert_eq!(source.cursor, stream_prefix_len(&header) + MIB);
    assert!(!dir
        .path()
        .join(partial_name(ID).expect("valid test id"))
        .exists());
    assert!(!dir
        .path()
        .join(partial_meta_name(ID).expect("valid test id"))
        .exists());
    assert!(names(dir.path()).is_empty());
}

#[tokio::test]
async fn size_limit_and_missing_destination_are_rejected_before_creation() {
    let root = TestDir::new();
    let dest = root.path().join("downloads");
    fs::create_dir(&dest).expect("download directory is created");
    let data = vec![7_u8; 2048];
    let header = make_header("too-big.bin", data.len() as u64, 0, sha256(&data), MTIME_US);
    let mut source = ScriptedSource::new(stream(&header, &data));
    let limited = ReceiveConfig {
        dest_dir: dest.clone(),
        max_file_bytes: 1024,
        buf_size: 4096,
    };

    assert!(matches!(
        receive_plain(&mut source, &limited).await,
        Err(RecvError::FileTooLarge {
            size: 2048,
            limit: 1024
        })
    ));
    assert!(names(&dest).is_empty());

    let missing = root.path().join("does-not-exist");
    let mut second = ScriptedSource::new(stream(&header, &data));
    assert!(matches!(
        receive_plain(&mut second, &config(&missing)).await,
        Err(RecvError::DestMissing)
    ));
    assert!(!missing.exists());

    let not_directory = root.path().join("plain-file");
    fs::write(&not_directory, b"x").expect("plain file is written");
    let mut third = ScriptedSource::new(stream(&header, &data));
    assert!(matches!(
        receive_plain(&mut third, &config(&not_directory)).await,
        Err(RecvError::DestMissing)
    ));
}

#[tokio::test]
async fn progress_is_monotonic_and_ends_completed() {
    let dir = TestDir::new();
    let data = random_data(2 * MIB, 0x000e_000f_0010_0011);
    let header = make_header(
        "progress.bin",
        data.len() as u64,
        0,
        sha256(&data),
        MTIME_US,
    );
    let mut source =
        ScriptedSource::new(stream(&header, &data)).with_chunks(random_chunks(23, 256));
    let cfg = config(dir.path());
    let cancel = CancelFlag::new();
    let clock_value = Arc::new(AtomicU64::new(0));
    let now = {
        let clock_value = Arc::clone(&clock_value);
        move || clock_value.fetch_add(10_000, Ordering::Relaxed) + 10_000
    };
    let mut events = Vec::new();

    {
        let mut progress = |event: TransferProgress| events.push(event);
        let outcome = receive(&mut source, &cfg, &mut progress, &cancel, &now)
            .await
            .expect("receive succeeds");
        completed(outcome);
    }

    assert!(events.len() > 3);
    assert_eq!(events[0].state, TransferState::Running);
    assert_eq!(events[0].done, 0);
    assert!(events
        .iter()
        .all(|event| event.direction == Direction::Receive));
    assert!(events.windows(2).all(|pair| pair[0].done <= pair[1].done));
    let last = events.last().expect("there is at least one event");
    assert_eq!(last.state, TransferState::Completed);
    assert_eq!(last.done, data.len() as u64);
    assert!(clock_value.load(Ordering::Relaxed) >= 10_000);
}

#[tokio::test]
async fn pre_header_errors_do_not_create_files() {
    let dir = TestDir::new();
    let cfg = config(dir.path());

    let mut empty = ScriptedSource::new(Vec::new());
    assert!(matches!(
        receive_plain(&mut empty, &cfg).await,
        Err(RecvError::Codec(FileError::EmptyStream))
    ));

    let mut truncated = ScriptedSource::new(vec![0, 0]);
    assert!(matches!(
        receive_plain(&mut truncated, &cfg).await,
        Err(RecvError::Codec(FileError::HeaderTruncated {
            declared: 4,
            received: 2
        }))
    ));

    let mut transport = ScriptedSource::new(vec![0, 0, 0, 1]).stop_after(0, StopBehavior::Error);
    assert!(matches!(
        receive_plain(&mut transport, &cfg).await,
        Err(RecvError::Transport(ConnError::Closed))
    ));
    assert!(names(dir.path()).is_empty());
}

#[test]
fn cleanup_stale_only_removes_old_partial_files() {
    let dir = TestDir::new();
    let old = dir.path().join(partial_name(ID).expect("valid test id"));
    let fresh = dir
        .path()
        .join(partial_name(OTHER_ID).expect("valid test id"));
    fs::write(&old, b"old").expect("old partial is written");
    fs::write(&fresh, b"fresh").expect("fresh partial is written");
    fs::write(dir.path().join("a.txt"), b"keep me").expect("ordinary file is written");

    let old_time = SystemTime::now() - Duration::from_secs(8 * 24 * 60 * 60);
    fs::File::options()
        .write(true)
        .open(&old)
        .expect("old partial opens")
        .set_modified(old_time)
        .expect("old partial timestamp is set");

    let removed = cleanup_stale(
        dir.path(),
        Duration::from_secs(7 * 24 * 60 * 60),
        SystemTime::now(),
    )
    .expect("stale cleanup succeeds");

    assert_eq!(removed, 1);
    assert!(!old.exists());
    assert!(fresh.exists());
    assert!(dir.path().join("a.txt").exists());
}

#[test]
fn partial_received_rejects_missing_or_malformed_metadata() {
    let dir = TestDir::new();
    let part = dir.path().join(partial_name(ID).expect("valid test id"));
    let meta = dir
        .path()
        .join(partial_meta_name(ID).expect("valid test id"));
    fs::write(&part, b"partial").expect("partial is written");
    assert_eq!(
        partial_received(dir.path(), ID).expect("missing metadata is not an I/O error"),
        None
    );

    fs::write(&meta, b"{}").expect("malformed metadata is written");
    assert_eq!(
        partial_received(dir.path(), ID).expect("malformed metadata is not an I/O error"),
        None
    );
}

#[test]
fn cleanup_stale_ignores_directories_and_foreign_partial_names() {
    let dir = TestDir::new();
    let nested = dir.path().join(format!(".rs-{ID}.part"));
    fs::create_dir(&nested).expect("directory with partial-like name is created");
    let foreign = dir.path().join(".rs-not-a-transfer-id.part");
    fs::write(&foreign, b"keep").expect("foreign file is written");
    let old_time = SystemTime::now() - Duration::from_secs(30 * 24 * 60 * 60);
    fs::File::options()
        .write(true)
        .open(&foreign)
        .expect("foreign file opens")
        .set_modified(old_time)
        .expect("foreign timestamp is set");

    let removed = cleanup_stale(
        dir.path(),
        Duration::from_secs(7 * 24 * 60 * 60),
        SystemTime::now(),
    )
    .expect("stale cleanup succeeds");

    assert_eq!(removed, 0);
    assert!(nested.is_dir());
    assert!(foreign.is_file());
}
