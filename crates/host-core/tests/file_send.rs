use std::fs;
use std::future::Future;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};

use host_core::file_recv::{partial_received, receive, FileSource, ReceiveConfig, ReceiveOutcome};
use host_core::file_send::{prepare, send, FileSink, PreparedFile, SendError, SendOutcome};
use ring::digest;
use rsp_core::file::{build_header, encode_stream_prefix, FileStreamReceiver, StreamEnd};
use rsp_core::file_name::sanitize;
use rsp_core::file_progress::{CancelFlag, Direction, TransferProgress, TransferState};
use rsp_core::pb::FileHeader;
use rsp_transport::ConnError;
use tokio::sync::mpsc;

const MIB: usize = 1024 * 1024;
const KIB: usize = 1024;
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
            "rs-file-send-{}-{nanos}-{serial}",
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

fn sha256(data: &[u8]) -> [u8; 32] {
    digest::digest(&digest::SHA256, data)
        .as_ref()
        .try_into()
        .expect("SHA-256 is 32 bytes")
}

fn mtime_us(metadata: &fs::Metadata) -> u64 {
    metadata
        .modified()
        .expect("modification time is available")
        .duration_since(UNIX_EPOCH)
        .expect("modification time is after the epoch")
        .as_micros()
        .try_into()
        .expect("modification time fits in u64")
}

fn prefix_len(file: &PreparedFile, offset: u64) -> usize {
    let header = build_header(
        file.transfer_id.clone(),
        sanitize(&file.name),
        file.size,
        offset,
        file.sha256,
        file.mtime_us,
    )
    .expect("test header is valid");
    encode_stream_prefix(&header)
        .expect("test header encodes")
        .len()
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
    assert_eq!(
        receiver.finish().expect("stream finishes"),
        StreamEnd::Complete {
            header: header.clone()
        }
    );
    (header, data)
}

fn parse_interrupted_stream(stream: &[u8]) -> (FileHeader, Vec<u8>) {
    let mut receiver = FileStreamReceiver::new();
    let fed = receiver.feed(stream).expect("stream parses");
    let header = fed.header.expect("stream has a header");
    let mut data = Vec::new();
    if let Some(span) = fed.data {
        assert_eq!(span.offset, header.offset);
        data.extend_from_slice(span.bytes);
    }
    match receiver.finish().expect("stream finishes") {
        StreamEnd::Truncated { header, position } => {
            assert_eq!(position, header.offset + data.len() as u64);
            (header, data)
        }
        other => panic!("expected Truncated, got {other:?}"),
    }
}

fn running_count(events: &[TransferProgress]) -> usize {
    events
        .iter()
        .take_while(|event| event.state == TransferState::Running)
        .count()
}

#[derive(Default)]
struct MemorySink {
    stream: Vec<u8>,
    prefix_seen: bool,
    data_bytes: usize,
    finish_calls: usize,
    fail_after_data: Option<usize>,
    cancel_after_data: Option<(usize, CancelFlag)>,
}

impl MemorySink {
    fn failing_after(data_bytes: usize) -> Self {
        Self {
            fail_after_data: Some(data_bytes),
            ..Self::default()
        }
    }

    fn cancelling_after(data_bytes: usize, cancel: CancelFlag) -> Self {
        Self {
            cancel_after_data: Some((data_bytes, cancel)),
            ..Self::default()
        }
    }
}

impl FileSink for MemorySink {
    fn write(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send {
        if !self.prefix_seen {
            self.prefix_seen = true;
            self.stream.extend_from_slice(bytes);
            return std::future::ready(Ok(()));
        }

        if self
            .fail_after_data
            .is_some_and(|limit| self.data_bytes >= limit)
        {
            return std::future::ready(Err(ConnError::Closed));
        }

        self.stream.extend_from_slice(bytes);
        self.data_bytes += bytes.len();
        if let Some((limit, cancel)) = self.cancel_after_data.as_ref() {
            if self.data_bytes >= *limit {
                cancel.cancel();
            }
        }
        std::future::ready(Ok(()))
    }

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.finish_calls += 1;
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn prepare_hashes_metadata_and_rejects_directories() {
    let dir = TestDir::new();
    let data = random_data(MIB, 0x1234_5678_9abc_def0);
    let path = dir.path().join("报告 100%.bin");
    fs::write(&path, &data).expect("source file is written");
    let metadata = fs::metadata(&path).expect("source metadata is available");

    let prepared = prepare(&path, None)
        .await
        .expect("regular file prepares successfully");

    assert_eq!(prepared.path, path);
    assert_eq!(prepared.name, sanitize("报告 100%.bin"));
    assert_eq!(prepared.size, data.len() as u64);
    assert_eq!(prepared.mtime_us, mtime_us(&metadata));
    assert_eq!(prepared.sha256, sha256(&data));
    assert_eq!(prepared.transfer_id.len(), 32);
    assert!(prepared
        .transfer_id
        .bytes()
        .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte)));

    let supplied_id = "00112233445566778899aabbccddeeff".to_owned();
    let resumed = prepare(&path, Some(supplied_id.clone()))
        .await
        .expect("regular file prepares with a supplied transfer id");
    assert_eq!(resumed.transfer_id, supplied_id);

    let error = prepare(dir.path(), None)
        .await
        .expect_err("a directory cannot be sent");
    assert!(
        matches!(error, SendError::NotAFile),
        "unexpected error: {error}"
    );
}

#[tokio::test]
async fn offset_zero_writes_prefix_data_and_fin() {
    let dir = TestDir::new();
    let data = random_data(MIB, 0x0ddc_0ffe_e15e_beef);
    let path = dir.path().join("complete.bin");
    fs::write(&path, &data).expect("source file is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let mut sink = MemorySink::default();
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || 0_u64;

    let outcome = send(&prepared, 0, &mut sink, &mut progress, &cancel, &now)
        .await
        .expect("send succeeds");

    assert_eq!(
        outcome,
        SendOutcome::Completed {
            sent: prepared.size
        }
    );
    assert_eq!(sink.finish_calls, 1);
    let (header, received) = parse_stream(&sink.stream);
    assert_eq!(header.transfer_id, prepared.transfer_id);
    assert_eq!(header.name, prepared.name);
    assert_eq!(header.size, prepared.size);
    assert_eq!(header.offset, 0);
    assert_eq!(header.sha256, prepared.sha256);
    assert_eq!(header.mtime_us, prepared.mtime_us);
    assert_eq!(received, data);
}

#[tokio::test]
async fn offsets_resume_mid_file_and_allow_an_empty_tail() {
    let dir = TestDir::new();
    let data = random_data(MIB, 0x55aa_1234_9876_fedc);
    let path = dir.path().join("resume.bin");
    fs::write(&path, &data).expect("source file is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || 0_u64;

    let mut middle = MemorySink::default();
    let outcome = send(&prepared, 1000, &mut middle, &mut progress, &cancel, &now)
        .await
        .expect("send from the middle succeeds");
    assert_eq!(
        outcome,
        SendOutcome::Completed {
            sent: prepared.size
        }
    );
    let (header, received) = parse_stream(&middle.stream);
    assert_eq!(header.offset, 1000);
    assert_eq!(received, data[1000..]);
    assert_eq!(middle.finish_calls, 1);

    let mut tail = MemorySink::default();
    let outcome = send(
        &prepared,
        prepared.size,
        &mut tail,
        &mut progress,
        &cancel,
        &now,
    )
    .await
    .expect("send from EOF succeeds");
    assert_eq!(
        outcome,
        SendOutcome::Completed {
            sent: prepared.size
        }
    );
    let (header, received) = parse_stream(&tail.stream);
    assert_eq!(header.offset, prepared.size);
    assert!(received.is_empty());
    assert_eq!(tail.stream.len(), prefix_len(&prepared, prepared.size));
    assert_eq!(tail.finish_calls, 1);

    let mut beyond = MemorySink::default();
    let error = send(
        &prepared,
        prepared.size + 1,
        &mut beyond,
        &mut progress,
        &cancel,
        &now,
    )
    .await
    .expect_err("an offset beyond EOF is rejected");
    assert!(matches!(
        error,
        SendError::OffsetBeyondSize { offset, size }
            if offset == prepared.size + 1 && size == prepared.size
    ));
    assert!(beyond.stream.is_empty());
    assert_eq!(beyond.finish_calls, 0);
}

#[tokio::test]
async fn source_change_is_detected_before_writing_anything() {
    let dir = TestDir::new();
    let path = dir.path().join("changing.bin");
    fs::write(&path, random_data(256 * KIB, 0xfeed_face_cafe_beef)).expect("source is written");
    let prepared = prepare(&path, None).await.expect("file prepares");

    let mut appended = fs::OpenOptions::new()
        .append(true)
        .open(&path)
        .expect("source is open for append");
    use std::io::Write;
    std::thread::sleep(std::time::Duration::from_millis(2));
    appended.write_all(b"changed").expect("source is appended");
    drop(appended);

    let mut sink = MemorySink::default();
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || 0_u64;
    let result = send(&prepared, 0, &mut sink, &mut progress, &cancel, &now).await;

    assert!(matches!(result, Err(SendError::SourceChanged)));
    assert!(sink.stream.is_empty());
    assert_eq!(sink.finish_calls, 0);
}

#[tokio::test]
async fn short_read_is_reported_as_a_source_change() {
    let dir = TestDir::new();
    let path = dir.path().join("short.bin");
    fs::write(&path, random_data(256 * KIB, 0x1357_9bdf_2468_ace0)).expect("source is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let mut sink = TruncatingSink {
        path: path.clone(),
        stream: Vec::new(),
        finish_calls: 0,
    };
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || 0_u64;
    let result = send(&prepared, 0, &mut sink, &mut progress, &cancel, &now).await;

    assert!(matches!(result, Err(SendError::SourceChanged)));
    assert_eq!(sink.finish_calls, 0);
    assert_eq!(sink.stream.len(), prefix_len(&prepared, 0));
}

struct TruncatingSink {
    path: PathBuf,
    stream: Vec<u8>,
    finish_calls: usize,
}

impl FileSink for TruncatingSink {
    fn write(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send {
        if self.stream.is_empty() {
            fs::OpenOptions::new()
                .write(true)
                .open(&self.path)
                .expect("source is open for truncation")
                .set_len(0)
                .expect("source is truncated during the send");
        }
        self.stream.extend_from_slice(bytes);
        std::future::ready(Ok(()))
    }

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.finish_calls += 1;
        std::future::ready(Ok(()))
    }
}

#[tokio::test]
async fn sink_failure_interrupts_at_the_last_accepted_data_offset() {
    let dir = TestDir::new();
    let path = dir.path().join("interrupted.bin");
    fs::write(&path, random_data(MIB, 0x1111_2222_3333_4444)).expect("source is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let mut sink = MemorySink::failing_after(300 * KIB);
    let cancel = CancelFlag::new();
    let mut progress = |_event: TransferProgress| {};
    let now = || 0_u64;

    let outcome = send(&prepared, 1000, &mut sink, &mut progress, &cancel, &now)
        .await
        .expect("a sink failure is reported as an outcome");

    assert_eq!(
        outcome,
        SendOutcome::Interrupted {
            sent: 1000 + sink.data_bytes as u64
        }
    );
    assert!(sink.data_bytes >= 300 * KIB);
    assert_eq!(sink.finish_calls, 0);
    assert_eq!(
        sink.stream.len(),
        prefix_len(&prepared, 1000) + sink.data_bytes
    );
    let (header, received) = parse_interrupted_stream(&sink.stream);
    assert_eq!(header.offset, 1000);
    assert_eq!(received.len(), sink.data_bytes);
}

#[tokio::test]
async fn cancellation_stops_without_finishing_the_stream() {
    let dir = TestDir::new();
    let path = dir.path().join("cancelled.bin");
    fs::write(&path, random_data(MIB, 0xabcd_ef01_2345_6789)).expect("source is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let cancel = CancelFlag::new();
    let mut sink = MemorySink::cancelling_after(128 * KIB, cancel.clone());
    let mut events = Vec::new();
    let mut progress = |event: TransferProgress| events.push(event);
    let now = || 0_u64;

    let outcome = send(&prepared, 0, &mut sink, &mut progress, &cancel, &now)
        .await
        .expect("cancellation is reported as an outcome");

    assert_eq!(outcome, SendOutcome::Cancelled);
    assert_eq!(sink.finish_calls, 0);
    assert!(sink.data_bytes >= 128 * KIB);
    assert!(sink.data_bytes < prepared.size as usize);
    assert_eq!(
        events.last().map(|event| &event.state),
        Some(&TransferState::Cancelled)
    );
}

#[tokio::test]
async fn progress_is_monotonic_and_completes_at_the_declared_size() {
    let dir = TestDir::new();
    let path = dir.path().join("progress.bin");
    fs::write(&path, random_data(MIB, 0x2468_ace0_1357_9bdf)).expect("source is written");
    let prepared = prepare(&path, None).await.expect("file prepares");
    let mut sink = MemorySink::default();
    let cancel = CancelFlag::new();
    let mut events = Vec::new();
    let mut progress = |event: TransferProgress| events.push(event);
    let clock = AtomicU64::new(MTIME_US);
    let now = || clock.fetch_add(10_000, Ordering::Relaxed);

    let outcome = send(&prepared, 0, &mut sink, &mut progress, &cancel, &now)
        .await
        .expect("send succeeds");

    assert_eq!(
        outcome,
        SendOutcome::Completed {
            sent: prepared.size
        }
    );
    assert!(events.len() > 2);
    assert!(events
        .iter()
        .all(|event| event.direction == Direction::Send));
    assert!(events.windows(2).all(|pair| pair[0].done <= pair[1].done));
    assert_eq!(events[0].state, TransferState::Running);
    assert_eq!(events[0].done, 0);
    let last = events.last().expect("at least one progress event exists");
    assert_eq!(last.transfer_id, prepared.transfer_id);
    assert_eq!(last.direction, Direction::Send);
    assert_eq!(last.state, TransferState::Completed);
    assert_eq!(last.done, prepared.size);
    assert_eq!(last.size, prepared.size);
    assert_eq!(last.fail, None);
    assert!(running_count(&events) >= 2);
}

struct PipeSink {
    tx: Option<mpsc::Sender<Vec<u8>>>,
    prefix_seen: bool,
    data_bytes: usize,
    cutoff: usize,
    finish_calls: usize,
}

impl PipeSink {
    fn new(tx: mpsc::Sender<Vec<u8>>, cutoff: usize) -> Self {
        Self {
            tx: Some(tx),
            prefix_seen: false,
            data_bytes: 0,
            cutoff,
            finish_calls: 0,
        }
    }
}

impl FileSink for PipeSink {
    fn write(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send {
        let tx = if !self.prefix_seen {
            self.prefix_seen = true;
            self.tx.clone()
        } else if self.data_bytes >= self.cutoff {
            self.tx.take();
            None
        } else {
            self.data_bytes += bytes.len();
            self.tx.clone()
        };
        let payload = bytes.to_vec();
        async move {
            let Some(tx) = tx else {
                return Err(ConnError::Closed);
            };
            tx.send(payload).await.map_err(|_| ConnError::Closed)
        }
    }

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.finish_calls += 1;
        self.tx.take();
        std::future::ready(Ok(()))
    }
}

struct PipeSource {
    rx: mpsc::Receiver<Vec<u8>>,
    pending: Vec<u8>,
    position: usize,
}

impl PipeSource {
    fn new(rx: mpsc::Receiver<Vec<u8>>) -> Self {
        Self {
            rx,
            pending: Vec::new(),
            position: 0,
        }
    }
}

impl FileSource for PipeSource {
    async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, ConnError> {
        loop {
            if self.position < self.pending.len() {
                let count = buf.len().min(self.pending.len() - self.position);
                buf[..count].copy_from_slice(&self.pending[self.position..self.position + count]);
                self.position += count;
                if self.position == self.pending.len() {
                    self.pending.clear();
                    self.position = 0;
                }
                return Ok(Some(count));
            }

            match self.rx.recv().await {
                Some(chunk) => {
                    self.pending = chunk;
                    self.position = 0;
                }
                None => return Ok(None),
            }
        }
    }
}

fn destination_names(path: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(path)
        .expect("destination directory is readable")
        .map(|entry| {
            entry
                .expect("destination entry is readable")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect();
    names.sort();
    names
}

#[tokio::test]
async fn interrupted_pipe_resumes_with_the_receiver_reported_offset() {
    let dir = TestDir::new();
    let source_path = dir.path().join("source.bin");
    let data = random_data(8 * MIB, 0x9876_5432_10fe_dcba);
    fs::write(&source_path, &data).expect("source file is written");
    let prepared = prepare(&source_path, None).await.expect("file prepares");

    let dest = dir.path().join("received");
    fs::create_dir(&dest).expect("destination directory is created");
    let cfg = ReceiveConfig {
        dest_dir: dest.clone(),
        ..ReceiveConfig::default()
    };

    let (first_tx, first_rx) = mpsc::channel(8);
    let mut first_sink = PipeSink::new(first_tx, 3 * MIB);
    let mut first_source = PipeSource::new(first_rx);
    let first_cancel = CancelFlag::new();
    let first_receive_cancel = CancelFlag::new();
    let mut send_progress = |_event: TransferProgress| {};
    let mut receive_progress = |_event: TransferProgress| {};
    let now = || 0_u64;

    let (first_send, first_receive) = tokio::join!(
        send(
            &prepared,
            0,
            &mut first_sink,
            &mut send_progress,
            &first_cancel,
            &now,
        ),
        receive(
            &mut first_source,
            &cfg,
            &mut receive_progress,
            &first_receive_cancel,
            &now,
        ),
    );

    assert!(matches!(
        first_send,
        Ok(SendOutcome::Interrupted { sent }) if sent == 3 * MIB as u64
    ));
    assert!(matches!(
        first_receive,
        Ok(ReceiveOutcome::Interrupted { received, .. }) if received == 3 * MIB as u64
    ));
    assert_eq!(first_sink.finish_calls, 0);

    let received = partial_received(&dest, &prepared.transfer_id)
        .expect("partial state is readable")
        .expect("the interrupted transfer has a partial file");
    assert_eq!(received, 3 * MIB as u64);

    let (second_tx, second_rx) = mpsc::channel(8);
    let mut second_sink = PipeSink::new(second_tx, usize::MAX);
    let mut second_source = PipeSource::new(second_rx);
    let second_cancel = CancelFlag::new();
    let second_receive_cancel = CancelFlag::new();
    let (second_send, second_receive) = tokio::join!(
        send(
            &prepared,
            received,
            &mut second_sink,
            &mut send_progress,
            &second_cancel,
            &now,
        ),
        receive(
            &mut second_source,
            &cfg,
            &mut receive_progress,
            &second_receive_cancel,
            &now,
        ),
    );

    assert_eq!(
        second_send.expect("resumed send succeeds"),
        SendOutcome::Completed {
            sent: prepared.size
        }
    );
    let path = match second_receive.expect("resumed receive succeeds") {
        ReceiveOutcome::Completed {
            transfer_id,
            path,
            size,
        } => {
            assert_eq!(transfer_id, prepared.transfer_id);
            assert_eq!(size, prepared.size);
            path
        }
        other => panic!("expected Completed, got {other:?}"),
    };
    assert_eq!(second_sink.finish_calls, 1);
    assert_eq!(fs::read(&path).expect("received file is readable"), data);
    assert_eq!(destination_names(&dest), vec!["source.bin".to_owned()]);
    assert!(!destination_names(&dest)
        .iter()
        .any(|name| name.starts_with(".rs-")));
}
