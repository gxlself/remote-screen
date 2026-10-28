//! Large SFTP transfers across repeated disconnects, plus partial cleanup.

mod support;

use std::{
    fs,
    future::Future,
    path::{Path, PathBuf},
    time::{Duration, Instant},
};

use rs_ssh::{
    sftp::Sftp,
    sftp_transfer::{
        discard_download_partials, discard_upload_partials, download, upload, Cancel,
        TransferError, TransferOpts, TransferOutcome,
    },
    SshSession,
};
use ssh_key::sha2::{Digest as _, Sha256};
use support::sftp_server::{unique_temp_dir, SftpTestServer};
use tokio::{
    fs::File,
    io::{AsyncReadExt as _, AsyncWriteExt as _},
};

const MIB: u64 = 1024 * 1024;
const DEFAULT_SIZE_MIB: u64 = 64;
const DEFAULT_TIMEOUT: Duration = Duration::from_secs(120);
const DISCONNECT_PERCENT: [u64; 3] = [25, 50, 75];
const UPLOAD_WINDOW: u64 = 16;
const PATTERN_SEED: u64 = 0x5f37_1d29_b4c8_6e13;
const CLEANUP_SIZE: usize = 4 * 1024 * 1024;

struct Pattern(u64);

impl Pattern {
    fn new() -> Self {
        Self(PATTERN_SEED)
    }

    fn fill(&mut self, buffer: &mut [u8]) {
        for word in buffer.chunks_mut(8) {
            self.0 ^= self.0 << 13;
            self.0 ^= self.0 >> 7;
            self.0 ^= self.0 << 17;
            let bytes = self.0.to_le_bytes();
            word.copy_from_slice(&bytes[..word.len()]);
        }
    }
}

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        Self(unique_temp_dir(label))
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn test_size() -> u64 {
    let size_mib = match std::env::var("RS_SFTP_TEST_MB") {
        Ok(value) => value
            .parse::<u64>()
            .unwrap_or_else(|error| panic!("invalid RS_SFTP_TEST_MB={value:?}: {error}")),
        Err(std::env::VarError::NotPresent) => DEFAULT_SIZE_MIB,
        Err(error) => panic!("cannot read RS_SFTP_TEST_MB: {error}"),
    };
    assert!(size_mib > 0, "RS_SFTP_TEST_MB must be positive");
    size_mib * MIB
}

fn test_timeout(size: u64) -> Duration {
    if size > 256 * MIB {
        Duration::from_secs(size / (10 * MIB) + 60)
    } else {
        DEFAULT_TIMEOUT
    }
}

async fn with_timeout<F>(label: &str, timeout: Duration, future: F)
where
    F: Future<Output = ()>,
{
    tokio::time::timeout(timeout, future)
        .await
        .unwrap_or_else(|_| panic!("{label} exceeded {timeout:?}"));
}

async fn write_pattern(path: &Path, size: u64) -> [u8; 32] {
    let mut file = File::create(path).await.unwrap();
    let mut pattern = Pattern::new();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; MIB as usize];
    let mut remaining = size;
    while remaining > 0 {
        let count = remaining.min(buffer.len() as u64) as usize;
        let chunk = &mut buffer[..count];
        pattern.fill(chunk);
        hasher.update(&chunk[..]);
        file.write_all(chunk).await.unwrap();
        remaining -= count as u64;
    }
    file.flush().await.unwrap();
    hasher.finalize().into()
}

async fn file_sha256(path: &Path) -> [u8; 32] {
    let mut file = File::open(path).await.unwrap();
    let mut hasher = Sha256::new();
    let mut buffer = vec![0; MIB as usize];
    loop {
        let count = file.read(&mut buffer).await.unwrap();
        if count == 0 {
            break;
        }
        hasher.update(&buffer[..count]);
    }
    hasher.finalize().into()
}

async fn open(server: &SftpTestServer) -> (SshSession, Sftp) {
    let session = server.connect().await;
    let sftp = session.sftp().await.unwrap();
    (session, sftp)
}

fn partial_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| {
            name.rsplit_once(".rspart-").is_some_and(|(_, tag)| {
                tag.len() == 8 && tag.bytes().all(|byte| byte.is_ascii_hexdigit())
            })
        })
        .collect()
}

fn only_partial_len(dir: &Path) -> u64 {
    let names = partial_names(dir);
    assert_eq!(names.len(), 1, "unexpected partials: {names:?}");
    fs::metadata(dir.join(&names[0])).unwrap().len()
}

fn completion(outcome: TransferOutcome) -> (u64, u64) {
    match outcome {
        TransferOutcome::Completed {
            bytes,
            resumed_from,
        } => (bytes, resumed_from),
        TransferOutcome::Cancelled => panic!("transfer was unexpectedly cancelled"),
    }
}

fn upload_resume_offset(partial_len: u64, size: u64) -> u64 {
    if partial_len > size {
        return 0;
    }
    let chunk = TransferOpts::default().chunk as u64;
    partial_len.saturating_sub(UPLOAD_WINDOW * chunk) / chunk * chunk
}

fn report_transfer(label: &str, size: u64, elapsed: Duration) {
    let seconds = elapsed.as_secs_f64();
    let mib_per_second = size as f64 / MIB as f64 / seconds;
    eprintln!("{label}: {seconds:.2} s, {mib_per_second:.1} MiB/s");
}

#[tokio::test]
async fn large_download_survives_three_disconnects() {
    let size = test_size();
    with_timeout(
        "large download",
        test_timeout(size),
        download_survives_three_disconnects(size),
    )
    .await;
}

async fn download_survives_three_disconnects(size: u64) {
    let started = Instant::now();
    let server = SftpTestServer::start_temporary("large-download").await;
    let expected = write_pattern(&server.root().join("source.bin"), size).await;
    let local_dir = TempDir::new("rs-sftp-large-download");
    let local = local_dir.path().join("copy.bin");
    let mut previous_resumed = 0;
    let mut max_in_flight_reads = 0;

    for (attempt, percent) in DISCONNECT_PERCENT.into_iter().enumerate() {
        let partial_len = if attempt == 0 {
            0
        } else {
            only_partial_len(local_dir.path())
        };
        let target = size * percent / 100;
        let bytes_before_target = target.saturating_sub(partial_len);
        let (session, sftp) = open(&server).await;
        server.delay_io_responses(Some(Duration::from_millis(5)));
        server.fail_after_read_bytes(Some(bytes_before_target));
        let mut resumed_from = None;
        let failure = download(
            &sftp,
            "/home/test/source.bin",
            &local,
            &TransferOpts::default(),
            &mut |progress| {
                resumed_from.get_or_insert(progress.done);
            },
            &Cancel::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
        let resumed_from = resumed_from.expect("download did not report initial progress");
        assert_eq!(resumed_from, partial_len);
        if attempt > 0 {
            assert!(resumed_from > 0, "attempt {attempt} did not resume");
            assert!(
                resumed_from >= previous_resumed,
                "resume regressed: {resumed_from} < {previous_resumed}"
            );
        }
        previous_resumed = resumed_from;
        max_in_flight_reads = max_in_flight_reads.max(server.max_in_flight_reads());
        sftp.close().await;
        session.close().await;
    }

    let partial_len = only_partial_len(local_dir.path());
    let (session, sftp) = open(&server).await;
    server.delay_io_responses(Some(Duration::from_millis(5)));
    let outcome = download(
        &sftp,
        "/home/test/source.bin",
        &local,
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    max_in_flight_reads = max_in_flight_reads.max(server.max_in_flight_reads());
    let (bytes, resumed_from) = completion(outcome);
    assert_eq!(bytes, size);
    assert!(resumed_from > 0, "final download did not resume");
    assert!(resumed_from >= previous_resumed);
    assert!(
        resumed_from >= partial_len,
        "final resume {resumed_from} is behind the visible partial {partial_len}"
    );
    assert_eq!(file_sha256(&local).await, expected);
    assert!(partial_names(local_dir.path()).is_empty());
    assert!(
        max_in_flight_reads > 1,
        "download was not pipelined: {max_in_flight_reads}"
    );

    sftp.close().await;
    session.close().await;
    report_transfer("large download", size, started.elapsed());
}

#[tokio::test]
async fn large_upload_survives_three_disconnects() {
    let size = test_size();
    with_timeout(
        "large upload",
        test_timeout(size),
        upload_survives_three_disconnects(size),
    )
    .await;
}

async fn upload_survives_three_disconnects(size: u64) {
    let started = Instant::now();
    let server = SftpTestServer::start_temporary("large-upload").await;
    let local_dir = TempDir::new("rs-sftp-large-upload");
    let local = local_dir.path().join("source.bin");
    let expected = write_pattern(&local, size).await;
    let mut previous_resumed = 0;
    let mut max_in_flight_writes = 0;

    for (attempt, percent) in DISCONNECT_PERCENT.into_iter().enumerate() {
        let partial_len = if attempt == 0 {
            0
        } else {
            only_partial_len(server.root())
        };
        let resumed_from = upload_resume_offset(partial_len, size);
        let target = size * percent / 100;
        let bytes_before_target = target.saturating_sub(resumed_from);
        let (session, sftp) = open(&server).await;
        server.delay_io_responses(Some(Duration::from_millis(5)));
        server.fail_after_write_bytes(Some(bytes_before_target));
        let mut reported_resumed_from = None;
        let failure = upload(
            &sftp,
            &local,
            "/home/test/uploaded.bin",
            &TransferOpts::default(),
            &mut |progress| {
                reported_resumed_from.get_or_insert(progress.done);
            },
            &Cancel::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
        assert_eq!(reported_resumed_from, Some(resumed_from));
        if attempt > 0 {
            assert!(resumed_from > 0, "attempt {attempt} did not resume");
            assert!(
                resumed_from >= previous_resumed,
                "resume regressed: {resumed_from} < {previous_resumed}"
            );
        }
        previous_resumed = resumed_from;
        max_in_flight_writes = max_in_flight_writes.max(server.max_in_flight_writes());
        sftp.close().await;
        session.close().await;
    }

    let partial_len = only_partial_len(server.root());
    let (session, sftp) = open(&server).await;
    server.delay_io_responses(Some(Duration::from_millis(5)));
    let outcome = upload(
        &sftp,
        &local,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    max_in_flight_writes = max_in_flight_writes.max(server.max_in_flight_writes());
    let (bytes, resumed_from) = completion(outcome);
    assert_eq!(bytes, size);
    assert!(resumed_from > 0, "final upload did not resume");
    assert!(resumed_from >= previous_resumed);
    assert_eq!(resumed_from, upload_resume_offset(partial_len, size));
    assert_eq!(
        file_sha256(&server.root().join("uploaded.bin")).await,
        expected
    );
    assert!(partial_names(server.root()).is_empty());
    assert!(
        max_in_flight_writes > 1,
        "upload was not pipelined: {max_in_flight_writes}"
    );
    assert!(
        max_in_flight_writes <= UPLOAD_WINDOW,
        "upload window exceeded {UPLOAD_WINDOW}: {max_in_flight_writes}"
    );

    sftp.close().await;
    session.close().await;
    report_transfer("large upload", size, started.elapsed());
}

#[tokio::test]
async fn discard_download_partials_removes_only_partials() {
    with_timeout("discard download partials", DEFAULT_TIMEOUT, async {
        let server = SftpTestServer::start_temporary("discard-download").await;
        let data = vec![0x5a; CLEANUP_SIZE];
        fs::write(server.root().join("source.bin"), &data).unwrap();
        let local_dir = TempDir::new("rs-sftp-discard-download");
        let local = local_dir.path().join("copy.bin");
        let target_lookalike = local_dir.path().join("copy.bin.backup");
        let unrelated = local_dir.path().join("keep.txt");
        let lookalike = local_dir.path().join("copy.bin.rspart-nothex");
        fs::write(&target_lookalike, b"other file").unwrap();
        fs::write(&unrelated, b"keep me").unwrap();

        let (session, sftp) = open(&server).await;
        server.fail_after_read_bytes(Some(CLEANUP_SIZE as u64 / 2));
        let failure = download(
            &sftp,
            "/home/test/source.bin",
            &local,
            &TransferOpts::default(),
            &mut |_| {},
            &Cancel::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
        assert_eq!(partial_names(local_dir.path()).len(), 1);
        assert!(!local.exists());
        fs::write(&lookalike, b"not a partial").unwrap();

        let removed = discard_download_partials(&local).await.unwrap();
        assert_eq!(removed, 1);
        assert!(partial_names(local_dir.path()).is_empty());
        assert!(!local.exists());
        assert_eq!(fs::read(&target_lookalike).unwrap(), b"other file");
        assert_eq!(fs::read(&unrelated).unwrap(), b"keep me");
        assert_eq!(fs::read(&lookalike).unwrap(), b"not a partial");
        assert_eq!(discard_download_partials(&local).await.unwrap(), 0);

        let missing = local_dir.path().join("missing").join("copy.bin");
        assert_eq!(discard_download_partials(&missing).await.unwrap(), 0);
        let directory_target = local_dir.path().join("directory");
        fs::create_dir(&directory_target).unwrap();
        let directory_partial = local_dir
            .path()
            .join(format!("directory.rspart-{:08x}", std::process::id()));
        fs::write(&directory_partial, b"not ours").unwrap();
        assert_eq!(
            discard_download_partials(&directory_target).await.unwrap(),
            0
        );
        assert_eq!(fs::read(&directory_partial).unwrap(), b"not ours");

        sftp.close().await;
        session.close().await;
    })
    .await;
}

#[tokio::test]
async fn discard_upload_partials_removes_only_partials() {
    with_timeout("discard upload partials", DEFAULT_TIMEOUT, async {
        let server = SftpTestServer::start_temporary("discard-upload").await;
        let local_dir = TempDir::new("rs-sftp-discard-upload");
        let source = local_dir.path().join("source.bin");
        fs::write(&source, vec![0xa5; CLEANUP_SIZE]).unwrap();
        let target = server.root().join("uploaded.bin");
        let target_lookalike = server.root().join("uploaded.bin.backup");
        let unrelated = server.root().join("keep.txt");
        let lookalike = server.root().join("uploaded.bin.rspart-nothex");
        fs::write(&target, b"original target").unwrap();
        fs::write(&target_lookalike, b"other file").unwrap();
        fs::write(&unrelated, b"keep me").unwrap();
        fs::write(&lookalike, b"not a partial").unwrap();

        let (session, sftp) = open(&server).await;
        server.fail_after_write_bytes(Some(CLEANUP_SIZE as u64 / 2));
        let failure = upload(
            &sftp,
            &source,
            "/home/test/uploaded.bin",
            &TransferOpts {
                overwrite: true,
                ..TransferOpts::default()
            },
            &mut |_| {},
            &Cancel::default(),
        )
        .await
        .unwrap_err();
        assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
        assert_eq!(partial_names(server.root()).len(), 1);
        sftp.close().await;
        session.close().await;

        let (session, sftp) = open(&server).await;
        let removed = discard_upload_partials(&sftp, "/home/test/uploaded.bin")
            .await
            .unwrap();
        assert_eq!(removed, 1);
        assert!(partial_names(server.root()).is_empty());
        assert_eq!(fs::read(&target).unwrap(), b"original target");
        assert_eq!(fs::read(&target_lookalike).unwrap(), b"other file");
        assert_eq!(fs::read(&unrelated).unwrap(), b"keep me");
        assert_eq!(fs::read(&lookalike).unwrap(), b"not a partial");
        assert_eq!(
            discard_upload_partials(&sftp, "/home/test/uploaded.bin")
                .await
                .unwrap(),
            0
        );
        assert_eq!(
            discard_upload_partials(&sftp, "/home/test/missing/uploaded.bin")
                .await
                .unwrap(),
            0
        );

        sftp.close().await;
        session.close().await;
    })
    .await;
}
