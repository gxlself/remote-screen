use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use ring::digest;
use rsp_core::file::{build_header, encode_stream_prefix, FileError};
use rsp_core::file_name::{partial_meta_name, partial_name};
use rsp_core::file_progress::{CancelFlag, TransferProgress};
use rsp_core::pb::{self, FileHeader};
use rsp_files::recv::{
    discard_partial, partial_info, partial_received, receive_with, AcceptFn, FileSource,
    ReceiveConfig, ReceiveOutcome, RecvError,
};
use rsp_transport::ConnError;

const ID: &str = "00112233445566778899aabbccddeeff";
const MTIME_US: u64 = 1_735_689_600_000_000;

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
            "rs-recv-with-{}-{nanos}-{serial}",
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

fn make_header(name: &str, size: u64, offset: u64, sha256: [u8; 32]) -> FileHeader {
    build_header(ID, name, size, offset, sha256, MTIME_US).expect("test header is valid")
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

struct ScriptedSource {
    stream: Vec<u8>,
    cursor: usize,
    stop_at: Option<usize>,
    stop_with_error: bool,
}

impl ScriptedSource {
    fn new(stream: Vec<u8>) -> Self {
        Self {
            stream,
            cursor: 0,
            stop_at: None,
            stop_with_error: false,
        }
    }

    fn stop_after(mut self, offset: usize, stop_with_error: bool) -> Self {
        self.stop_at = Some(offset);
        self.stop_with_error = stop_with_error;
        self
    }

    fn next_read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, ConnError> {
        if let Some(stop_at) = self.stop_at {
            if self.cursor >= stop_at {
                self.stop_at = None;
                return if self.stop_with_error {
                    Err(ConnError::Closed)
                } else {
                    Ok(None)
                };
            }
        }

        if self.cursor == self.stream.len() {
            return Ok(None);
        }
        if buf.is_empty() {
            return Ok(Some(0));
        }

        let mut count = buf.len().min(self.stream.len() - self.cursor);
        if let Some(stop_at) = self.stop_at {
            count = count.min(stop_at - self.cursor);
        }
        buf[..count].copy_from_slice(&self.stream[self.cursor..self.cursor + count]);
        self.cursor += count;
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

async fn receive_with_accept(
    source: &mut ScriptedSource,
    cfg: &ReceiveConfig,
    accept: &mut AcceptFn<'_>,
) -> Result<ReceiveOutcome, RecvError> {
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || MTIME_US;
    receive_with(source, cfg, accept, &mut progress, &cancel, &now).await
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
async fn rejection_leaves_no_partial_files() {
    let dir = TestDir::new();
    let data = b"rejected payload";
    let header = make_header("rejected.bin", data.len() as u64, 0, sha256(data));
    let mut source = ScriptedSource::new(stream(&header, data));
    let mut accept = |_header: &FileHeader| Err(RecvError::Rejected(pb::FileError::Disabled));

    let error = receive_with_accept(&mut source, &config(dir.path()), &mut accept)
        .await
        .expect_err("the receiver rejects the file");

    assert!(matches!(
        error,
        RecvError::Rejected(pb::FileError::Disabled)
    ));
    assert!(names(dir.path()).is_empty());
}

#[tokio::test]
async fn callback_destination_receives_the_final_file() {
    let dir = TestDir::new();
    let other = dir.path().join("other");
    fs::create_dir(&other).expect("callback destination is created");
    let data = b"callback destination payload";
    let header = make_header("other.bin", data.len() as u64, 0, sha256(data));
    let mut source = ScriptedSource::new(stream(&header, data));

    let other_for_accept = other.clone();
    let mut accept = move |_header: &FileHeader| Ok(Some(other_for_accept.clone()));
    let (_, path, size) = completed(
        receive_with_accept(&mut source, &config(dir.path()), &mut accept)
            .await
            .expect("receive succeeds"),
    );

    assert_eq!(path.parent(), Some(other.as_path()));
    assert_eq!(size, data.len() as u64);
    assert_eq!(fs::read(&path).expect("received file is readable"), data);
    assert_eq!(names(dir.path()), vec!["other".to_owned()]);
    assert!(!dir.path().join("other.bin").exists());
    assert!(!has_partial_leftovers(&other));
}

#[tokio::test]
async fn missing_callback_destination_is_rejected_before_creation() {
    let root = TestDir::new();
    let dest = root.path().join("downloads");
    fs::create_dir(&dest).expect("receive directory is created");
    let other = root.path().join("missing");
    let data = b"missing destination";
    let header = make_header("missing.bin", data.len() as u64, 0, sha256(data));
    let mut source = ScriptedSource::new(stream(&header, data));

    let other_for_accept = other.clone();
    let mut accept = move |_header: &FileHeader| Ok(Some(other_for_accept.clone()));
    let error = receive_with_accept(&mut source, &config(&dest), &mut accept)
        .await
        .expect_err("a missing callback destination is rejected");

    assert!(matches!(error, RecvError::DestMissing));
    assert!(names(&dest).is_empty());
    assert!(!other.exists());
}

#[tokio::test]
async fn callback_destination_does_not_overwrite_an_existing_name() {
    let root = TestDir::new();
    let dest = root.path().join("downloads");
    let other = root.path().join("other");
    fs::create_dir(&dest).expect("receive directory is created");
    fs::create_dir(&other).expect("callback destination is created");
    let existing = other.join("report.pdf");
    fs::write(&existing, b"old").expect("existing file is written");
    let data = b"new payload";
    let header = make_header("report.pdf", data.len() as u64, 0, sha256(data));
    let mut source = ScriptedSource::new(stream(&header, data));

    let other_for_accept = other.clone();
    let mut accept = move |_header: &FileHeader| Ok(Some(other_for_accept.clone()));
    let (_, path, _) = completed(
        receive_with_accept(&mut source, &config(&dest), &mut accept)
            .await
            .expect("receive succeeds"),
    );

    assert_eq!(
        path.file_name().and_then(|name| name.to_str()),
        Some("report (1).pdf")
    );
    assert_eq!(fs::read(&path).expect("new file is readable"), data);
    assert_eq!(fs::read(&existing).expect("old file is readable"), b"old");
    assert!(names(&dest).is_empty());
}

#[tokio::test]
async fn callback_runs_once_with_the_sender_header() {
    let dir = TestDir::new();
    let data = b"callback header payload";
    let header = make_header("header.bin", data.len() as u64, 0, sha256(data));
    let mut source = ScriptedSource::new(stream(&header, data));
    let mut calls = 0_usize;
    let mut seen: Option<(String, u64)> = None;
    let mut accept = |header: &FileHeader| {
        calls += 1;
        seen = Some((header.transfer_id.clone(), header.size));
        Ok(None)
    };

    completed(
        receive_with_accept(&mut source, &config(dir.path()), &mut accept)
            .await
            .expect("receive succeeds"),
    );

    assert_eq!(calls, 1);
    assert_eq!(seen, Some((ID.to_owned(), data.len() as u64)));
}

#[tokio::test]
async fn interrupted_receive_resumes_into_the_callback_destination() {
    let root = TestDir::new();
    let dest = root.path().join("downloads");
    let other = root.path().join("other");
    fs::create_dir(&dest).expect("receive directory is created");
    fs::create_dir(&other).expect("callback destination is created");
    let data: Vec<u8> = (0_u32..256 * 1024)
        .map(|index| (index.wrapping_mul(31) % 251) as u8)
        .collect();
    let header = make_header("resume.bin", data.len() as u64, 0, sha256(&data));
    let stop = stream_prefix_len(&header) + 64 * 1024;
    let mut first = ScriptedSource::new(stream(&header, &data)).stop_after(stop, true);
    let mut accept_none = |_header: &FileHeader| Ok(None);

    let (_, received) = interrupted(
        receive_with_accept(&mut first, &config(&dest), &mut accept_none)
            .await
            .expect("interruption is not an error"),
    );
    assert_eq!(received, 64 * 1024);
    assert_eq!(
        partial_received(&dest, ID).expect("partial state is readable"),
        Some(received)
    );

    let resumed = make_header("resume.bin", data.len() as u64, received, sha256(&data));
    let mut second = ScriptedSource::new(stream(&resumed, &data[received as usize..]));
    let other_for_accept = other.clone();
    let mut accept_other = move |_header: &FileHeader| Ok(Some(other_for_accept.clone()));
    let (_, path, size) = completed(
        receive_with_accept(&mut second, &config(&dest), &mut accept_other)
            .await
            .expect("resume succeeds"),
    );

    assert_eq!(size, data.len() as u64);
    assert_eq!(path.parent(), Some(other.as_path()));
    assert_eq!(fs::read(&path).expect("received file is readable"), data);
    assert!(names(&dest).is_empty());
}

#[tokio::test]
async fn discard_partial_removes_both_files_then_reports_missing() {
    let dir = TestDir::new();
    let data = b"partial payload to discard";
    let header = make_header("discard.bin", data.len() as u64, 0, sha256(data));
    let stop = stream_prefix_len(&header);
    let mut source = ScriptedSource::new(stream(&header, data)).stop_after(stop, true);
    let mut accept = |_header: &FileHeader| Ok(None);

    interrupted(
        receive_with_accept(&mut source, &config(dir.path()), &mut accept)
            .await
            .expect("interruption is not an error"),
    );
    assert!(dir
        .path()
        .join(partial_name(ID).expect("valid test id"))
        .exists());
    assert!(dir
        .path()
        .join(partial_meta_name(ID).expect("valid test id"))
        .exists());

    assert!(discard_partial(dir.path(), ID).expect("partial files are removed"));
    assert!(!dir
        .path()
        .join(partial_name(ID).expect("valid test id"))
        .exists());
    assert!(!dir
        .path()
        .join(partial_meta_name(ID).expect("valid test id"))
        .exists());
    assert!(!discard_partial(dir.path(), ID).expect("missing partials are not an error"));
    assert!(matches!(
        discard_partial(dir.path(), "xyz"),
        Err(RecvError::Codec(FileError::InvalidTransferId))
    ));
}

#[tokio::test]
async fn partial_info_matches_the_received_bytes_and_header() {
    let dir = TestDir::new();
    let data: Vec<u8> = (0_u32..96 * 1024)
        .map(|index| (index.wrapping_mul(17) % 239) as u8)
        .collect();
    let header = make_header("info.bin", data.len() as u64, 0, sha256(&data));
    let stop = stream_prefix_len(&header) + 64 * 1024;
    let mut source = ScriptedSource::new(stream(&header, &data)).stop_after(stop, true);
    let mut accept = |_header: &FileHeader| Ok(None);

    interrupted(
        receive_with_accept(&mut source, &config(dir.path()), &mut accept)
            .await
            .expect("interruption is not an error"),
    );

    let received = partial_received(dir.path(), ID)
        .expect("partial state is readable")
        .expect("partial file exists");
    let info = partial_info(dir.path(), ID)
        .expect("partial info is readable")
        .expect("partial info exists");
    assert_eq!(info.received, received);
    assert_eq!(info.size, data.len() as u64);
    assert_eq!(info.sha256, sha256(&data));
    assert_eq!(info.mtime_us, MTIME_US);
    assert_eq!(info.name, "info.bin");

    let missing = TestDir::new();
    assert_eq!(
        partial_info(missing.path(), ID).expect("missing partial info is not an error"),
        None
    );

    fs::write(
        dir.path()
            .join(partial_meta_name(ID).expect("valid test id")),
        b"{}",
    )
    .expect("malformed metadata is written");
    assert_eq!(
        partial_info(dir.path(), ID).expect("malformed partial info is not an error"),
        None
    );
}
