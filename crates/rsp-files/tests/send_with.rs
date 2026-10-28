use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use rsp_core::file::{FileError, FileStreamReceiver};
use rsp_core::file_progress::{CancelFlag, TransferProgress};
use rsp_core::pb::FileHeader;
use rsp_files::send::{prepare, send, send_with, FileSink, SendError, SendOutcome};
use rsp_transport::ConnError;

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
            "rs-send-with-{}-{nanos}-{serial}",
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

fn parse_stream(stream: &[u8]) -> (FileHeader, Vec<u8>) {
    let mut receiver = FileStreamReceiver::new();
    let fed = receiver.feed(stream).expect("stream parses");
    let header = fed.header.expect("stream has a header");
    let mut data = Vec::new();
    if let Some(span) = fed.data {
        assert_eq!(span.offset, header.offset);
        data.extend_from_slice(span.bytes);
    }
    receiver.finish().expect("stream finishes");
    (header, data)
}

#[derive(Default)]
struct MemorySink {
    stream: Vec<u8>,
    finish_calls: usize,
}

impl FileSink for MemorySink {
    fn write(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.stream.extend_from_slice(bytes);
        std::future::ready(Ok(()))
    }

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.finish_calls += 1;
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn send_with_sets_dest_dir_and_writes_the_same_data() {
    let dir = TestDir::new();
    let data = random_data(320 * 1024, 0x1234_5678_9abc_def0);
    let path = dir.path().join("upload.bin");
    fs::write(&path, &data).expect("source file is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || 0_u64;

    let mut with_dest = MemorySink::default();
    let outcome = send_with(
        &prepared,
        0,
        "/srv/in",
        &mut with_dest,
        &mut progress,
        &cancel,
        &now,
    )
    .await
    .expect("send_with succeeds");
    assert_eq!(
        outcome,
        SendOutcome::Completed {
            sent: prepared.size
        }
    );
    let (header, received) = parse_stream(&with_dest.stream);
    assert_eq!(header.dest_dir, "/srv/in");
    assert_eq!(header.transfer_id, prepared.transfer_id);
    assert_eq!(header.name, prepared.name);
    assert_eq!(header.size, prepared.size);
    assert_eq!(header.sha256, prepared.sha256);
    assert_eq!(header.mtime_us, prepared.mtime_us);
    assert_eq!(received, data);
    assert_eq!(with_dest.finish_calls, 1);

    let mut without_dest = MemorySink::default();
    send(
        &prepared,
        0,
        &mut without_dest,
        &mut progress,
        &cancel,
        &now,
    )
    .await
    .expect("send succeeds");
    let (plain_header, plain_data) = parse_stream(&without_dest.stream);
    assert_eq!(plain_header.dest_dir, "");
    assert_eq!(plain_data, received);
}

#[tokio::test]
async fn oversized_dest_dir_is_rejected_before_writing() {
    let dir = TestDir::new();
    let data = random_data(1024, 0x0ddc_0ffe_e15e_beef);
    let path = dir.path().join("rejected.bin");
    fs::write(&path, &data).expect("source file is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || 0_u64;
    let mut sink = MemorySink::default();

    let error = send_with(
        &prepared,
        0,
        &"a".repeat(3000),
        &mut sink,
        &mut progress,
        &cancel,
        &now,
    )
    .await
    .expect_err("an oversized dest_dir is rejected");

    assert!(matches!(
        error,
        SendError::Codec(FileError::InvalidDestDir(_))
    ));
    assert!(sink.stream.is_empty());
    assert_eq!(sink.finish_calls, 0);
}
