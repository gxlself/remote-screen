//! SFTP upload/download resume, progress and cancellation.

mod support;

use std::{
    fs,
    future::{poll_fn, Future as _},
    path::{Path, PathBuf},
    sync::{mpsc, Mutex},
    task::Poll,
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use rs_ssh::sftp::{DirEntry, EntryKind, SftpError};
use rs_ssh::sftp_transfer::{
    download, upload, Cancel, Progress, TransferError, TransferOpts, TransferOutcome,
};
use support::sftp_server::{unique_temp_dir, SftpTestServer};

const MIB: usize = 1024 * 1024;
const BIG: usize = 8 * MIB;
const FAIL_AT: u64 = 3 * MIB as u64;

fn bytes(seed: u64, len: usize) -> Vec<u8> {
    let mut state = seed ^ 0x9e37_79b9_7f4a_7c15;
    let mut data = Vec::with_capacity(len);
    while data.len() < len {
        state ^= state << 13;
        state ^= state >> 7;
        state ^= state << 17;
        data.extend_from_slice(&state.to_le_bytes());
    }
    data.truncate(len);
    data
}

fn local_dir(label: &str) -> PathBuf {
    unique_temp_dir(label)
}

fn write(path: &Path, data: &[u8]) -> u64 {
    fs::write(path, data).unwrap();
    seconds(fs::metadata(path).unwrap().modified().unwrap())
}

fn seconds(time: SystemTime) -> u64 {
    time.duration_since(UNIX_EPOCH).unwrap().as_secs()
}

async fn open(server: &SftpTestServer) -> (rs_ssh::SshSession, rs_ssh::sftp::Sftp) {
    let session = server.connect().await;
    let sftp = session.sftp().await.unwrap();
    (session, sftp)
}

fn partial_names(dir: &Path) -> Vec<String> {
    fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().to_string_lossy().into_owned())
        .filter(|name| name.contains(".rspart-"))
        .collect()
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

fn assert_progress(progress: &[Progress], resumed_from: u64, total: u64) {
    assert_eq!(
        progress.first().copied(),
        Some(Progress {
            done: resumed_from,
            total
        })
    );
    assert!(progress.windows(2).all(|pair| pair[0].done <= pair[1].done));
    assert_eq!(
        progress.last().copied(),
        Some(Progress { done: total, total })
    );
}

fn entry<'a>(entries: &'a [DirEntry], name: &str) -> &'a DirEntry {
    entries.iter().find(|entry| entry.name == name).unwrap()
}

async fn advance_after_return() {
    tokio::time::pause();
    tokio::time::advance(Duration::from_millis(500)).await;
    tokio::time::resume();
}

/// Occupies the sole blocking worker until released. Dropping the sender also
/// releases it on assertion failure, so runtime shutdown cannot get stuck.
#[derive(Default)]
struct LocalWriteGate {
    release: Option<mpsc::Sender<()>>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl LocalWriteGate {
    fn block(&mut self) {
        let (started_tx, started_rx) = mpsc::channel();
        let (release_tx, release_rx) = mpsc::channel();
        self.release = Some(release_tx);
        self.task = Some(tokio::task::spawn_blocking(move || {
            started_tx.send(()).unwrap();
            let _ = release_rx.recv();
        }));
        // The previous file write finishes before this worker starts. The
        // next write is guaranteed to queue behind this gate.
        started_rx.recv().unwrap();
    }

    fn release(&mut self) -> tokio::task::JoinHandle<()> {
        self.release.take().unwrap().send(()).unwrap();
        self.task.take().unwrap()
    }
}

#[test]
fn delayed_download_error_waits_for_local_write_before_resume() {
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .max_blocking_threads(1)
        .build()
        .unwrap()
        .block_on(async {
            let server = SftpTestServer::start_temporary("download-write-barrier").await;
            let opts = TransferOpts::default();
            let chunk = opts.chunk as u64;
            let disconnect_at = 3 * chunk;
            let data = bytes(18, 8 * opts.chunk);
            write(&server.root().join("remote.bin"), &data);
            let dir = local_dir("rs-transfer-write-barrier");
            let local = dir.join("copy.bin");
            let (first_session, first_sftp) = open(&server).await;
            let faults = server.connection_faults()[0].clone();
            faults.delay_io_responses(Some(Duration::from_millis(5)));
            faults.hold_reads_from(disconnect_at);
            let gate = Mutex::new(LocalWriteGate::default());
            let mut queued = false;
            let failure = {
                let mut progress = |p: Progress| {
                    if p.done == disconnect_at - chunk {
                        gate.lock().unwrap().block();
                    }
                    if p.done == disconnect_at {
                        queued = true;
                        faults.disconnect();
                    }
                };
                let cancel = Cancel::default();
                let transfer = download(
                    &first_sftp,
                    "/home/test/remote.bin",
                    &local,
                    &opts,
                    &mut progress,
                    &cancel,
                );
                tokio::pin!(transfer);
                tokio::select! {
                    result = &mut transfer => panic!("returned before the blocked write finished: {result:?}"),
                    _ = first_session.closed() => {}
                }
                // With the connection closed the only remaining work is local
                // I/O. The old implementation returns an error here, leaving
                // its last write queued behind our blocking worker.
                poll_fn(|cx| {
                    assert!(transfer.as_mut().poll(cx).is_pending(), "returned with a local write still pending");
                    Poll::Ready(())
                }).await;
                let worker = gate.lock().unwrap().release();
                worker.await.unwrap();
                transfer.await.unwrap_err()
            };
            assert!(queued);
            assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
            let partials = partial_names(&dir);
            assert_eq!(partials.len(), 1);
            let partial = dir.join(&partials[0]);
            let returned_len = fs::metadata(&partial).unwrap().len();
            assert_eq!(returned_len, disconnect_at);
            advance_after_return().await;
            assert_eq!(fs::metadata(&partial).unwrap().len(), returned_len);
            first_sftp.close().await;
            first_session.close().await;

            let (second_session, second_sftp) = open(&server).await;
            let outcome = download(
                &second_sftp,
                "/home/test/remote.bin",
                &local,
                &opts,
                &mut |_| {},
                &Cancel::default(),
            )
            .await
            .unwrap();
            assert_eq!(completion(outcome), (data.len() as u64, returned_len));
            assert_eq!(server.read_requests().first().unwrap().offset, returned_len);
            assert_eq!(fs::read(&local).unwrap(), data);
            assert!(partial_names(&dir).is_empty());
            second_sftp.close().await;
            second_session.close().await;
        });
}

#[tokio::test]
async fn download_eight_mib_preserves_content_and_mtime() {
    let server = SftpTestServer::start_temporary("download").await;
    let data = bytes(1, BIG);
    let remote_mtime = write(&server.root().join("big.bin"), &data);
    let dir = local_dir("rs-transfer-download");
    let local = dir.join("copy.bin");
    let (session, sftp) = open(&server).await;
    let mut progress = Vec::new();

    let outcome = download(
        &sftp,
        "/home/test/big.bin",
        &local,
        &TransferOpts::default(),
        &mut |p| progress.push(p),
        &Cancel::default(),
    )
    .await
    .unwrap();

    assert_eq!(completion(outcome), (BIG as u64, 0));
    assert_eq!(fs::read(&local).unwrap(), data);
    assert_eq!(
        seconds(fs::metadata(&local).unwrap().modified().unwrap()),
        remote_mtime
    );
    assert!(partial_names(&dir).is_empty());
    assert_progress(&progress, 0, BIG as u64);

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn zero_chunk_is_rejected_before_any_transfer_io() {
    let server = SftpTestServer::start_temporary("zero-chunk").await;
    let data = bytes(17, 64 * 1024);
    write(&server.root().join("remote.bin"), &data);
    let dir = local_dir("rs-transfer-zero-chunk");
    let local = dir.join("copy.bin");
    let source = dir.join("source.bin");
    write(&source, &data);
    let (session, sftp) = open(&server).await;
    let opts = TransferOpts {
        chunk: 0,
        overwrite: false,
    };

    let result = download(
        &sftp,
        "/home/test/remote.bin",
        &local,
        &opts,
        &mut |_| {},
        &Cancel::default(),
    )
    .await;
    assert!(matches!(
        result,
        Err(TransferError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidInput
    ));

    let result = upload(
        &sftp,
        &source,
        "/home/test/uploaded.bin",
        &opts,
        &mut |_| {},
        &Cancel::default(),
    )
    .await;
    assert!(matches!(
        result,
        Err(TransferError::Io(error)) if error.kind() == std::io::ErrorKind::InvalidInput
    ));

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn upload_eight_mib_preserves_content_and_mtime() {
    let server = SftpTestServer::start_temporary("upload").await;
    let data = bytes(2, BIG);
    let dir = local_dir("rs-transfer-upload");
    let local = dir.join("source.bin");
    let local_mtime = write(&local, &data);
    let (session, sftp) = open(&server).await;
    let mut progress = Vec::new();

    let outcome = upload(
        &sftp,
        &local,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |p| progress.push(p),
        &Cancel::default(),
    )
    .await
    .unwrap();

    assert_eq!(completion(outcome), (BIG as u64, 0));
    assert_eq!(fs::read(server.root().join("uploaded.bin")).unwrap(), data);
    assert_eq!(
        seconds(
            fs::metadata(server.root().join("uploaded.bin"))
                .unwrap()
                .modified()
                .unwrap()
        ),
        local_mtime
    );
    assert!(partial_names(server.root()).is_empty());
    assert_progress(&progress, 0, BIG as u64);

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn download_resumes_from_local_partial() {
    let server = SftpTestServer::start_temporary("download-resume").await;
    let data = bytes(3, BIG);
    write(&server.root().join("big.bin"), &data);
    let dir = local_dir("rs-transfer-download-resume");
    let local = dir.join("copy.bin");

    let (first_session, first_sftp) = open(&server).await;
    let mut fault_armed = false;
    let failure = download(
        &first_sftp,
        "/home/test/big.bin",
        &local,
        &TransferOpts::default(),
        &mut |progress| {
            if !fault_armed && progress.done >= FAIL_AT {
                fault_armed = true;
                // Arm the fixture from the local progress callback. Pipelined
                // reads can be acknowledged before their bytes reach disk, so
                // this makes "partial is at least FAIL_AT" deterministic.
                server.fail_after_read_bytes(Some(0));
            }
        },
        &Cancel::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
    let partial = partial_names(&dir);
    assert_eq!(partial.len(), 1, "{partial:?}");
    let partial_len = fs::metadata(dir.join(&partial[0])).unwrap().len();
    assert!(partial_len >= FAIL_AT && partial_len < BIG as u64);
    advance_after_return().await;
    assert_eq!(
        fs::metadata(dir.join(&partial[0])).unwrap().len(),
        partial_len
    );
    first_sftp.close().await;
    first_session.close().await;

    let (second_session, second_sftp) = open(&server).await;
    let mut progress = Vec::new();
    let outcome = download(
        &second_sftp,
        "/home/test/big.bin",
        &local,
        &TransferOpts::default(),
        &mut |p| progress.push(p),
        &Cancel::default(),
    )
    .await
    .unwrap();
    assert_eq!(completion(outcome), (BIG as u64, partial_len));
    let requests = server.read_requests();
    assert_eq!(requests.first().unwrap().offset, partial_len);
    assert_eq!(fs::read(&local).unwrap(), data);
    assert!(partial_names(&dir).is_empty());
    assert_progress(&progress, partial_len, BIG as u64);

    second_sftp.close().await;
    second_session.close().await;
}

#[tokio::test]
async fn upload_resumes_from_remote_partial() {
    let server = SftpTestServer::start_temporary("upload-resume").await;
    let data = bytes(4, BIG);
    let dir = local_dir("rs-transfer-upload-resume");
    let local = dir.join("source.bin");
    write(&local, &data);

    let (first_session, first_sftp) = open(&server).await;
    server.fail_after_write_bytes(Some(FAIL_AT));
    let failure = upload(
        &first_sftp,
        &local,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
    let partials = partial_names(server.root());
    assert_eq!(partials.len(), 1, "{partials:?}");
    let partial_len = fs::metadata(server.root().join(&partials[0]))
        .unwrap()
        .len();
    assert!(partial_len >= FAIL_AT && partial_len < BIG as u64);
    assert_eq!(
        partials[0],
        format!(
            "uploaded.bin.rspart-{}",
            fingerprint_for(
                &local,
                BIG as u64,
                fs::metadata(&local)
                    .unwrap()
                    .modified()
                    .unwrap()
                    .duration_since(UNIX_EPOCH)
                    .unwrap()
                    .as_secs()
            )
        )
    );
    first_sftp.close().await;
    first_session.close().await;

    let (second_session, second_sftp) = open(&server).await;
    let mut progress = Vec::new();
    let outcome = upload(
        &second_sftp,
        &local,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |p| progress.push(p),
        &Cancel::default(),
    )
    .await
    .unwrap();
    let expected_resume = expected_upload_resume(partial_len);
    assert_eq!(completion(outcome), (BIG as u64, expected_resume));
    assert_eq!(
        server.write_requests().first().unwrap().offset,
        expected_resume
    );
    assert_eq!(fs::read(server.root().join("uploaded.bin")).unwrap(), data);
    assert!(partial_names(server.root()).is_empty());
    assert_progress(&progress, expected_resume, BIG as u64);

    second_sftp.close().await;
    second_session.close().await;
}

#[cfg(unix)]
#[tokio::test]
async fn upload_resume_name_is_stable_across_symlinked_paths() {
    use std::os::unix::fs::symlink;

    let server = SftpTestServer::start_temporary("upload-resume-symlink").await;
    let data = bytes(19, BIG);
    let dir = local_dir("rs-transfer-upload-resume-symlink");
    let real = dir.join("real");
    fs::create_dir(&real).unwrap();
    let source = real.join("source.bin");
    write(&source, &data);
    let link = dir.join("link");
    symlink("real", &link).unwrap();

    let (first_session, first_sftp) = open(&server).await;
    server.fail_after_write_bytes(Some(FAIL_AT));
    let failure = upload(
        &first_sftp,
        &link.join("source.bin"),
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
    let partials = partial_names(server.root());
    assert_eq!(partials.len(), 1, "{partials:?}");
    let partial_len = fs::metadata(server.root().join(&partials[0]))
        .unwrap()
        .len();
    assert!(partial_len >= FAIL_AT && partial_len < BIG as u64);
    first_sftp.close().await;
    first_session.close().await;

    let (second_session, second_sftp) = open(&server).await;
    let outcome = upload(
        &second_sftp,
        &source,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    let expected_resume = expected_upload_resume(partial_len);
    let (bytes, resumed_from) = completion(outcome);
    assert_eq!(bytes, BIG as u64);
    assert_eq!(resumed_from, expected_resume);
    assert!(resumed_from > 0, "second upload did not resume");
    assert_eq!(
        server.write_requests().first().unwrap().offset,
        resumed_from
    );
    assert_eq!(fs::read(server.root().join("uploaded.bin")).unwrap(), data);
    assert!(partial_names(server.root()).is_empty());

    second_sftp.close().await;
    second_session.close().await;
}

fn fingerprint_for(local: &Path, size: u64, mtime: u64) -> String {
    let local = fs::canonicalize(local).unwrap();
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in local
        .to_string_lossy()
        .as_bytes()
        .iter()
        .copied()
        .chain([0])
        .chain(size.to_string().bytes())
        .chain([0])
        .chain(mtime.to_string().bytes())
    {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x0000_0100_0000_01b3);
    }
    format!("{hash:016x}")[..8].to_owned()
}

fn expected_upload_resume(partial_len: u64) -> u64 {
    let chunk = TransferOpts::default().chunk as u64;
    let window = 16 * chunk;
    partial_len.saturating_sub(window) / chunk * chunk
}

#[tokio::test]
async fn changed_remote_restarts_download_and_removes_old_partial() {
    let server = SftpTestServer::start_temporary("remote-size-change").await;
    let old = bytes(5, 4 * MIB);
    let new = bytes(6, 5 * MIB);
    write(&server.root().join("changing.bin"), &old);
    let dir = local_dir("rs-transfer-size-change");
    let local = dir.join("copy.bin");

    let (first_session, first_sftp) = open(&server).await;
    server.fail_after_read_bytes(Some(FAIL_AT));
    let _ = download(
        &first_sftp,
        "/home/test/changing.bin",
        &local,
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await;
    assert_eq!(partial_names(&dir).len(), 1);
    first_sftp.close().await;
    first_session.close().await;

    fs::write(server.root().join("changing.bin"), &new).unwrap();
    let (second_session, second_sftp) = open(&server).await;
    let outcome = download(
        &second_sftp,
        "/home/test/changing.bin",
        &local,
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    assert_eq!(completion(outcome), (new.len() as u64, 0));
    assert_eq!(fs::read(&local).unwrap(), new);
    assert!(partial_names(&dir).is_empty());

    second_sftp.close().await;
    second_session.close().await;
}

#[tokio::test]
async fn remote_mtime_change_is_detected_and_leaves_no_target() {
    let server = SftpTestServer::start_temporary("remote-mtime-change").await;
    let data = bytes(7, 4 * MIB);
    let remote_mtime = write(&server.root().join("changing.bin"), &data);
    let dir = local_dir("rs-transfer-mtime-change");
    let local = dir.join("copy.bin");
    let (session, sftp) = open(&server).await;
    let mut calls = 0;
    let mut mtime_changed = false;

    let result = download(
        &sftp,
        "/home/test/changing.bin",
        &local,
        &TransferOpts::default(),
        &mut |_| {
            calls += 1;
            if calls == 2 && !mtime_changed {
                mtime_changed = true;
                server.set_file_mtime("/home/test/changing.bin", remote_mtime + 10);
            }
        },
        &Cancel::default(),
    )
    .await;
    assert!(matches!(result, Err(TransferError::RemoteChanged)));
    assert!(!local.exists());
    assert!(partial_names(&dir).is_empty());

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn existing_destination_checks_before_io_and_overwrites_when_allowed() {
    let server = SftpTestServer::start_temporary("overwrite").await;
    let data = bytes(8, 3 * MIB);
    write(&server.root().join("download.bin"), &data);
    let dir = local_dir("rs-transfer-overwrite");
    let local = dir.join("download.bin");
    fs::write(&local, b"old").unwrap();
    let (session, sftp) = open(&server).await;

    let result = download(
        &sftp,
        "/home/test/download.bin",
        &local,
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await;
    assert!(matches!(result, Err(TransferError::AlreadyExists)));
    assert!(server.read_requests().is_empty());

    let opts = TransferOpts {
        overwrite: true,
        ..TransferOpts::default()
    };
    let outcome = download(
        &sftp,
        "/home/test/download.bin",
        &local,
        &opts,
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    assert_eq!(completion(outcome), (data.len() as u64, 0));
    assert_eq!(fs::read(&local).unwrap(), data);

    let source = dir.join("upload.bin");
    let upload_data = bytes(9, 2 * MIB);
    write(&source, &upload_data);
    fs::write(server.root().join("upload.bin"), b"old").unwrap();
    let result = upload(
        &sftp,
        &source,
        "/home/test/upload.bin",
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await;
    assert!(matches!(result, Err(TransferError::AlreadyExists)));

    let outcome = upload(
        &sftp,
        &source,
        "/home/test/upload.bin",
        &opts,
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    assert_eq!(completion(outcome), (upload_data.len() as u64, 0));
    assert_eq!(
        fs::read(server.root().join("upload.bin")).unwrap(),
        upload_data
    );

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn cancellation_removes_partial_files() {
    let server = SftpTestServer::start_temporary("cancel").await;
    let data = bytes(10, 4 * MIB);
    write(&server.root().join("download.bin"), &data);
    let dir = local_dir("rs-transfer-cancel");
    let download_path = dir.join("download.bin");
    let (session, sftp) = open(&server).await;
    let cancel = Cancel::default();

    let outcome = download(
        &sftp,
        "/home/test/download.bin",
        &download_path,
        &TransferOpts::default(),
        &mut |p| {
            if p.done > 0 {
                cancel.cancel();
            }
        },
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(outcome, TransferOutcome::Cancelled);
    assert!(!download_path.exists());
    assert!(partial_names(&dir).is_empty());
    advance_after_return().await;
    assert!(!download_path.exists());
    assert!(partial_names(&dir).is_empty());

    let source = dir.join("upload.bin");
    let upload_data = bytes(11, 4 * MIB);
    write(&source, &upload_data);
    server.delay_io_responses(Some(Duration::from_millis(5)));
    let cancel = Cancel::default();
    let outcome = upload(
        &sftp,
        &source,
        "/home/test/upload.bin",
        &TransferOpts::default(),
        &mut |p| {
            if p.done > 0 {
                cancel.cancel();
            }
        },
        &cancel,
    )
    .await
    .unwrap();
    assert_eq!(outcome, TransferOutcome::Cancelled);
    assert!(!server.root().join("upload.bin").exists());
    assert!(partial_names(server.root()).is_empty());
    let writes_at_return = server.write_requests();
    advance_after_return().await;
    assert_eq!(server.write_requests(), writes_at_return);
    assert!(partial_names(server.root()).is_empty());

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn upload_honours_advertised_posix_rename() {
    let server = SftpTestServer::start_temporary("posix-rename").await;
    server.set_posix_rename_advertised(true);
    let data = bytes(12, 2 * MIB);
    let dir = local_dir("rs-transfer-posix-rename");
    let local = dir.join("source.bin");
    write(&local, &data);
    fs::write(server.root().join("target.bin"), b"old").unwrap();
    let (session, sftp) = open(&server).await;
    let opts = TransferOpts {
        overwrite: true,
        ..TransferOpts::default()
    };

    let outcome = upload(
        &sftp,
        &local,
        "/home/test/target.bin",
        &opts,
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();

    assert_eq!(completion(outcome), (data.len() as u64, 0));
    assert_eq!(fs::read(server.root().join("target.bin")).unwrap(), data);
    assert!(partial_names(server.root()).is_empty());

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn upload_reports_local_change_and_keeps_remote_partial() {
    let server = SftpTestServer::start_temporary("local-change").await;
    let data = bytes(13, 4 * MIB);
    let dir = local_dir("rs-transfer-local-change");
    let local = dir.join("source.bin");
    write(&local, &data);
    let (session, sftp) = open(&server).await;
    let mut calls = 0;

    let result = upload(
        &sftp,
        &local,
        "/home/test/target.bin",
        &TransferOpts::default(),
        &mut |_| {
            calls += 1;
            if calls == 2 {
                fs::write(&local, b"changed after the first chunk").unwrap();
            }
        },
        &Cancel::default(),
    )
    .await;

    assert!(matches!(result, Err(TransferError::LocalChanged)));
    assert_eq!(partial_names(server.root()).len(), 1);
    assert!(!server.root().join("target.bin").exists());
    let writes_at_return = server.write_requests();
    advance_after_return().await;
    assert_eq!(server.write_requests(), writes_at_return);

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn missing_or_non_file_sources_are_reported() {
    let server = SftpTestServer::start_temporary("not-a-file").await;
    fs::create_dir(server.root().join("dir")).unwrap();
    let dir = local_dir("rs-transfer-not-a-file");
    let (session, sftp) = open(&server).await;

    let directory = download(
        &sftp,
        "/home/test/dir",
        &dir.join("copy"),
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await;
    assert!(matches!(directory, Err(TransferError::NotAFile)));

    let missing = download(
        &sftp,
        "/home/test/missing",
        &dir.join("copy"),
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await;
    assert!(matches!(
        missing,
        Err(TransferError::Sftp(SftpError::NotFound))
    ));
    assert_eq!(
        entry(&sftp.list("/home/test").await.unwrap(), "dir").kind,
        EntryKind::Dir
    );

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn download_and_upload_keep_multiple_requests_in_flight() {
    let server = SftpTestServer::start_temporary("pipeline").await;
    let data = bytes(14, 4 * MIB);
    write(&server.root().join("remote.bin"), &data);
    let dir = local_dir("rs-transfer-pipeline");
    let downloaded = dir.join("downloaded.bin");
    let (session, sftp) = open(&server).await;
    server.delay_io_responses(Some(Duration::from_millis(5)));

    let outcome = download(
        &sftp,
        "/home/test/remote.bin",
        &downloaded,
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    assert_eq!(completion(outcome), (data.len() as u64, 0));
    assert!(
        server.max_in_flight_reads() >= 2,
        "download was not pipelined: {}",
        server.max_in_flight_reads()
    );

    let source = dir.join("upload.bin");
    write(&source, &data);
    let outcome = upload(
        &sftp,
        &source,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    assert_eq!(completion(outcome), (data.len() as u64, 0));
    assert!(
        server.max_in_flight_writes() >= 2,
        "upload was not pipelined: {}",
        server.max_in_flight_writes()
    );
    let writes_at_return = server.write_requests();
    advance_after_return().await;
    assert_eq!(server.write_requests(), writes_at_return);

    sftp.close().await;
    session.close().await;
}

#[tokio::test]
async fn download_disconnect_discards_unacknowledged_reads() {
    let server = SftpTestServer::start_temporary("download-in-flight").await;
    let data = bytes(15, BIG);
    write(&server.root().join("remote.bin"), &data);
    let dir = local_dir("rs-transfer-download-in-flight");
    let local = dir.join("downloaded.bin");

    let (first_session, first_sftp) = open(&server).await;
    server.fail_after_read_bytes(Some(FAIL_AT));
    let failure = download(
        &first_sftp,
        "/home/test/remote.bin",
        &local,
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
    let partial = partial_names(&dir);
    assert_eq!(partial.len(), 1, "{partial:?}");
    let partial_len = fs::metadata(dir.join(&partial[0])).unwrap().len();
    assert!(partial_len > 0 && partial_len < BIG as u64);
    assert!(
        server.connection_faults()[0].max_in_flight_reads() >= 2,
        "download did not have unacknowledged reads when it disconnected"
    );
    first_sftp.close().await;
    first_session.close().await;

    let (second_session, second_sftp) = open(&server).await;
    let outcome = download(
        &second_sftp,
        "/home/test/remote.bin",
        &local,
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap();
    assert_eq!(completion(outcome), (BIG as u64, partial_len));
    assert_eq!(server.read_requests().first().unwrap().offset, partial_len);
    assert_eq!(fs::read(&local).unwrap(), data);

    second_sftp.close().await;
    second_session.close().await;
}

#[tokio::test]
async fn upload_recovers_after_out_of_order_acknowledgements() {
    let server = SftpTestServer::start_temporary("upload-in-flight").await;
    let data = bytes(16, BIG);
    let dir = local_dir("rs-transfer-upload-in-flight");
    let local = dir.join("source.bin");
    write(&local, &data);

    let (first_session, first_sftp) = open(&server).await;
    server.drop_write_responses_after(16, 5);
    let failure = upload(
        &first_sftp,
        &local,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |_| {},
        &Cancel::default(),
    )
    .await
    .unwrap_err();
    assert!(matches!(failure, TransferError::Sftp(_)), "{failure:?}");
    assert!(
        server.write_responses_dropped() >= 5,
        "fixture did not drop write acknowledgements: {}",
        server.write_responses_dropped()
    );
    let partials = partial_names(server.root());
    assert_eq!(partials.len(), 1, "{partials:?}");
    let partial_len = fs::metadata(server.root().join(&partials[0]))
        .unwrap()
        .len();
    assert!(partial_len > 0 && partial_len < BIG as u64, "{partial_len}");
    assert!(
        server.connection_faults()[0].max_in_flight_writes() >= 2,
        "upload did not have unacknowledged writes when it disconnected"
    );
    first_sftp.close().await;
    first_session.close().await;

    let (second_session, second_sftp) = open(&server).await;
    let mut progress = Vec::new();
    let outcome = upload(
        &second_sftp,
        &local,
        "/home/test/uploaded.bin",
        &TransferOpts::default(),
        &mut |p| progress.push(p),
        &Cancel::default(),
    )
    .await
    .unwrap();
    let expected_resume = expected_upload_resume(partial_len);
    let (bytes, resumed_from) = completion(outcome);
    assert_eq!((bytes, resumed_from), (BIG as u64, expected_resume));
    assert!(expected_resume > 0, "second upload did not resume");
    assert_eq!(
        server.write_requests().first().unwrap().offset,
        expected_resume
    );
    assert_eq!(fs::read(server.root().join("uploaded.bin")).unwrap(), data);
    assert!(partial_names(server.root()).is_empty());
    assert_progress(&progress, expected_resume, BIG as u64);

    second_sftp.close().await;
    second_session.close().await;
}

/// Exercise the same two-worker scheduling as the production Core without the
/// mobile transfer driver or any retry loop. Request deadlines stay at 3 s.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn concurrent_upload_and_download_on_two_workers_do_not_timeout() {
    let server = SftpTestServer::start_temporary("two-worker-transfers").await;
    let dir = local_dir("rs-transfer-two-workers");
    let download_data = bytes(41, BIG);
    let upload_data = bytes(42, BIG);
    let local_source = dir.join("source.bin");
    write(&server.root().join("source.bin"), &download_data);
    write(&local_source, &upload_data);
    let (session, sftp) = open(&server).await;
    server.delay_io_responses(Some(Duration::from_millis(5)));

    for round in 0..3 {
        let local_target = dir.join(format!("download-{round}.bin"));
        let remote_target = format!("/home/test/upload-{round}.bin");
        let opts = TransferOpts::default();
        let download_cancel = Cancel::default();
        let upload_cancel = Cancel::default();
        let mut download_progress = Vec::new();
        let mut upload_progress = Vec::new();
        let mut on_download = |progress| download_progress.push(progress);
        let mut on_upload = |progress| upload_progress.push(progress);
        let (downloaded, uploaded) = tokio::join!(
            download(
                &sftp,
                "/home/test/source.bin",
                &local_target,
                &opts,
                &mut on_download,
                &download_cancel,
            ),
            upload(
                &sftp,
                &local_source,
                &remote_target,
                &opts,
                &mut on_upload,
                &upload_cancel,
            ),
        );
        assert_eq!(completion(downloaded.unwrap()), (BIG as u64, 0));
        assert_eq!(completion(uploaded.unwrap()), (BIG as u64, 0));
        assert!(
            fs::read(&local_target).unwrap() == download_data,
            "download contents differ in round {round}"
        );
        assert!(
            fs::read(server.root().join(format!("upload-{round}.bin"))).unwrap() == upload_data,
            "upload contents differ in round {round}"
        );
        assert_progress(&download_progress, 0, BIG as u64);
        assert_progress(&upload_progress, 0, BIG as u64);
        assert!(partial_names(&dir).is_empty());
        assert!(partial_names(server.root()).is_empty());
    }
    assert_eq!(
        server.connection_faults().len(),
        1,
        "transfers must not reconnect"
    );
    sftp.close().await;
    session.close().await;
    fs::remove_dir_all(dir).unwrap();
}
