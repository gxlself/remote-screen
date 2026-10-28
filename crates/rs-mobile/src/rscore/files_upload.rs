//! Resumable phone-to-computer uploads over the shared FILES connection pool.

use std::collections::{HashMap, HashSet};
use std::path::Path;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rsp_client::files::{FileCtl, FileReply};
use rsp_core::file::{new_transfer_id, FileError};
use rsp_core::file_ctl::{
    STREAM_BUSY, STREAM_CANCELLED, STREAM_DISABLED, STREAM_NOT_ALLOWED, STREAM_PAUSED,
    STREAM_REJECTED,
};
use rsp_core::file_progress::CancelFlag;
use rsp_core::pb;
use rsp_files::send::{prepare, send_with, FileSink, PreparedFile, SendError, SendOutcome};
use rsp_transport::{ConnError, UniSend};
use serde::{Deserialize, Serialize};
use tokio::sync::{watch, Mutex as AsyncMutex};
use tokio::time::{sleep, timeout};

use super::api_files::{self, transfer_fail, FilesConn, FilesConns};
use super::dto_files::{
    FileSourceRef, FileTransfer, FileTransferDirection, FileTransferFail,
    FileTransferState as State,
};
use super::error::CoreError;
use super::runtime::{core, Core};
use super::xfer::Transfers;

const RETRY_SECONDS: [u64; 3] = [2, 5, 10];
const QUERY_TIMEOUT: Duration = Duration::from_secs(10);
const UPLOADS_FILE: &str = "uploads.json";
const FINGERPRINT_TTL_US: u64 = 7 * 24 * 3600 * 1_000_000;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Default)]
struct Uploads(Mutex<Inner>);

#[derive(Default)]
struct Inner {
    jobs: HashMap<String, Arc<Job>>,
    prepared: HashMap<String, Cached>,
    // Retain corrections across pause/resume until a new stream actually opens.
    offsets: HashMap<String, u64>,
    fingerprints_loaded: bool,
    fingerprints: HashMap<String, Fingerprint>,
}

#[derive(Clone, Serialize, Deserialize)]
struct Fingerprint {
    size: u64,
    mtime_us: u64,
    sha256_hex: String,
    saved_at_us: u64,
}

impl Inner {
    // The Uploads mutex serializes loading, pruning and atomic replacement so
    // concurrent transfers cannot overwrite a newer registry snapshot.
    fn load_fingerprints(&mut self, core: &Arc<Core>) {
        if self.fingerprints_loaded {
            return;
        }
        self.fingerprints_loaded = true;
        let path = core.data_dir().join(UPLOADS_FILE);
        let bytes = match std::fs::read(&path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => return,
            Err(_) => {
                tracing::warn!("could not read upload fingerprints; starting empty");
                return;
            }
        };
        self.fingerprints = match serde_json::from_slice(&bytes) {
            Ok(entries) => entries,
            Err(_) => {
                tracing::warn!("upload fingerprints are invalid; starting empty");
                if std::fs::rename(&path, core.data_dir().join("uploads.json.bad")).is_err() {
                    tracing::warn!("could not preserve invalid upload fingerprints");
                }
                return;
            }
        };
        let active: HashSet<_> = core
            .state::<Transfers>()
            .list(core)
            .into_iter()
            .filter(|transfer| {
                transfer.direction == FileTransferDirection::Upload
                    && !matches!(
                        transfer.state,
                        State::Completed | State::Failed | State::Cancelled
                    )
            })
            .map(|transfer| transfer.transfer_id)
            .collect();
        let now_us = core.now_us();
        let before = self.fingerprints.len();
        self.fingerprints.retain(|id, entry| {
            active.contains(id) && now_us.saturating_sub(entry.saved_at_us) < FINGERPRINT_TTL_US
        });
        if self.fingerprints.len() != before {
            self.save_fingerprints(core);
        }
    }

    fn save_fingerprints(&self, core: &Arc<Core>) {
        let directory = core.data_dir();
        let temporary = directory.join("uploads.json.tmp");
        let result = serde_json::to_vec(&self.fingerprints)
            .map_err(std::io::Error::other)
            .and_then(|bytes| std::fs::write(&temporary, bytes))
            .and_then(|()| std::fs::rename(&temporary, directory.join(UPLOADS_FILE)));
        if result.is_err() {
            let _ = std::fs::remove_file(&temporary);
            tracing::warn!("could not persist upload fingerprints");
        }
    }

    fn forget_fingerprint(&mut self, core: &Arc<Core>, id: &str) {
        self.load_fingerprints(core);
        if self.fingerprints.remove(id).is_some() {
            self.save_fingerprints(core);
        }
    }
}

struct Cached {
    file: Arc<PreparedFile>,
    modified: Option<SystemTime>,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Run,
    Pause,
    Cancel,
}

struct Job {
    command: watch::Sender<Command>,
    cancel: CancelFlag,
    gate: Arc<AsyncMutex<()>>,
}

/// Local validation and queueing, shared by the UniFFI facade and Rust callers.
pub async fn upload(
    core: &Arc<Core>,
    host_id: &str,
    local_path: &str,
    remote_dir: Option<String>,
) -> Result<String, CoreError> {
    api_files::register_driver(core);
    if remote_dir
        .as_ref()
        .is_some_and(|dir| dir.len() > 2048 || dir.contains('\0'))
    {
        return Err(http(400, "files.badPath"));
    }
    let path = Path::new(local_path);
    let metadata = tokio::fs::metadata(path)
        .await
        .map_err(|_| http(404, "files.localNotFound"))?;
    if !metadata.is_file() {
        return Err(http(404, "files.localNotFound"));
    }
    let name = path
        .file_name()
        .ok_or_else(|| http(404, "files.localNotFound"))?
        .to_string_lossy()
        .into_owned();
    let id = new_transfer_id().map_err(|_| CoreError::internal("files.random"))?;
    core.state::<Transfers>().create(
        core,
        FileTransfer {
            transfer_id: id.clone(),
            direction: FileTransferDirection::Upload,
            source: FileSourceRef::Computer {
                host_id: host_id.to_owned(),
            },
            name,
            remote_path: remote_dir,
            local_path: Some(local_path.to_owned()),
            size: metadata.len(),
            done: 0,
            bytes_per_sec: 0,
            state: State::Queued,
            fail: None,
            net_path: None,
            saved_name: None,
            started_at: core.now_us(),
            updated_at: core.now_us(),
        },
    );
    start(core, &id);
    Ok(id)
}

fn http(status: u16, message: &str) -> CoreError {
    CoreError::Http {
        status,
        hub_code: None,
        message: message.to_owned(),
    }
}

pub fn pause(core: &Arc<Core>, id: &str) {
    if let Some(job) = lock(&core.state::<Uploads>().0).jobs.get(id) {
        job.cancel.cancel();
        job.command.send_replace(Command::Pause);
    }
}

pub fn cancel(core: &Arc<Core>, id: &str) {
    let uploads = core.state::<Uploads>();
    let mut inner = lock(&uploads.0);
    let gate = inner.jobs.get(id).map(|job| job.gate.clone());
    if let Some(job) = inner.jobs.get(id) {
        job.cancel.cancel();
        job.command.send_replace(Command::Cancel);
    }
    inner.prepared.remove(id);
    inner.offsets.remove(id);
    inner.forget_fingerprint(core, id);
    drop(inner);
    // Use an existing connection only: cancelling never initiates a dial.
    if let Some(FileTransfer {
        source: FileSourceRef::Computer { host_id },
        ..
    }) = core.state::<Transfers>().get(core, id)
    {
        let pool = core.state::<FilesConns>();
        let slot = lock(&pool.slots).get(&host_id).cloned();
        if let Some(slot) = slot {
            let id = id.to_owned();
            let c = core.clone();
            core.spawn(async move {
                let _gate = match gate {
                    Some(gate) => Some(gate.lock_owned().await),
                    None => None,
                };
                let conn = slot.entry.lock().await.clone();
                if let Some(conn) = conn {
                    let _activity = conn.activity(&c);
                    send_cancel(&conn, &id);
                    // RESET can make the receiver choose interruption while an
                    // earlier FileCancel still sees its active entry. Wait for
                    // its partial flush, then cancel again to remove that pair.
                    let _ = query(&conn, &id).await;
                    send_cancel(&conn, &id);
                }
            });
        }
    }
}

fn send_cancel(conn: &FilesConn, id: &str) {
    conn.handle.send_file_ctl(FileCtl::Cancel(pb::FileCancel {
        transfer_id: id.to_owned(),
    }));
}

pub async fn resume(core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
    start(core, id);
    Ok(())
}

fn start(core: &Arc<Core>, id: &str) {
    let uploads = core.state::<Uploads>();
    let mut inner = lock(&uploads.0);
    let Some(transfer) = core.state::<Transfers>().get(core, id) else {
        return;
    };
    if matches!(transfer.state, State::Completed | State::Cancelled) {
        return;
    }
    if let Some(previous) = inner.jobs.get(id) {
        if *previous.command.borrow() == Command::Run && transfer.state != State::Failed {
            return;
        }
        previous.cancel.cancel();
        previous.command.send_replace(Command::Pause);
    }
    let gate = inner
        .jobs
        .get(id)
        .map(|job| job.gate.clone())
        .unwrap_or_default();
    let job = Arc::new(Job {
        command: watch::channel(Command::Run).0,
        cancel: CancelFlag::new(),
        gate,
    });
    inner.jobs.insert(id.to_owned(), job.clone());
    core.state::<Transfers>().update(core, id, |transfer| {
        if matches!(
            transfer.state,
            State::Paused | State::Interrupted | State::Failed
        ) {
            transfer.state = State::Queued;
            transfer.fail = None;
        }
    });
    drop(inner);
    let c = core.clone();
    let id = id.to_owned();
    core.spawn(async move {
        // A rapid resume waits until the previous sender has reset its stream.
        let _gate = job.gate.lock().await;
        run_upload(&c, &id, &job).await;
        let uploads = c.state::<Uploads>();
        let mut inner = lock(&uploads.0);
        if *job.command.borrow() == Command::Cancel {
            inner.prepared.remove(&id);
            inner.offsets.remove(&id);
            // Preparation may have finished concurrently with cancel().
            inner.forget_fingerprint(&c, &id);
        }
        if inner
            .jobs
            .get(&id)
            .is_some_and(|current| Arc::ptr_eq(current, &job))
        {
            inner.jobs.remove(&id);
        }
    });
}

async fn prepared(
    core: &Arc<Core>,
    id: &str,
    transfer: &FileTransfer,
) -> Result<Arc<PreparedFile>, SendError> {
    let path = Path::new(transfer.local_path.as_deref().ok_or(SendError::NotAFile)?);
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(SendError::NotAFile);
    }
    let modified = metadata.modified().ok();
    let uploads = core.state::<Uploads>();
    let previous = {
        let mut inner = lock(&uploads.0);
        if let Some(cached) = inner.prepared.get(id) {
            if cached.file.size == metadata.len() && cached.modified == modified {
                return Ok(cached.file.clone());
            }
        }
        inner.load_fingerprints(core);
        let mtime_us = modified
            .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
            .and_then(|duration| u64::try_from(duration.as_micros()).ok())
            .unwrap_or(0);
        inner
            .fingerprints
            .get(id)
            .filter(|entry| {
                transfer.done != 0
                    && !inner.prepared.contains_key(id)
                    && entry.size == metadata.len()
                    && entry.mtime_us == mtime_us
            })
            .cloned()
    };
    // prepare uses asynchronous chunked reads; no blocking hash task is needed.
    let file = Arc::new(prepare(path, Some(id.to_owned())).await?);
    let after = tokio::fs::metadata(path).await?;
    if after.len() != metadata.len() || after.modified().ok() != modified {
        return Err(SendError::SourceChanged);
    }
    let sha256_hex = data_encoding::HEXLOWER.encode(&file.sha256);
    let unchanged = previous.is_some_and(|entry| {
        entry.size == file.size && entry.mtime_us == file.mtime_us && entry.sha256_hex == sha256_hex
    });
    let mut inner = lock(&uploads.0);
    let changed = inner.prepared.contains_key(id)
        || transfer.size != file.size
        || (transfer.done != 0 && !unchanged);
    if changed {
        inner.offsets.insert(id.to_owned(), 0);
    }
    inner.prepared.insert(
        id.to_owned(),
        Cached {
            file: file.clone(),
            modified,
        },
    );
    inner.fingerprints.insert(
        id.to_owned(),
        Fingerprint {
            size: file.size,
            mtime_us: file.mtime_us,
            sha256_hex,
            saved_at_us: core.now_us(),
        },
    );
    inner.save_fingerprints(core);
    drop(inner);
    core.state::<Transfers>().update(core, id, |transfer| {
        transfer.size = file.size;
        if changed {
            transfer.done = 0;
        }
    });
    Ok(file)
}

struct UploadSink {
    stream: UniSend,
    command: watch::Receiver<Command>,
    cancel: CancelFlag,
    peer_error: Option<pb::FileError>,
}

impl UploadSink {
    fn stopped(&mut self) -> bool {
        if *self.command.borrow_and_update() != Command::Run {
            self.cancel.cancel();
            true
        } else {
            false
        }
    }

    fn remember(&mut self, result: &Result<(), ConnError>) {
        // ConnError currently erases quinn::WriteError::Stopped's typed code.
        // Preserve known policy rejections rather than retrying them as outages.
        if let Err(ConnError::Quic(message)) = result {
            self.peer_error = message
                .strip_prefix("sending stopped by peer: error ")
                .and_then(|code| code.parse::<u32>().ok())
                .and_then(|code| match code {
                    STREAM_DISABLED => Some(pb::FileError::Disabled),
                    STREAM_NOT_ALLOWED => Some(pb::FileError::NotAllowed),
                    STREAM_BUSY => Some(pb::FileError::Busy),
                    // Host path-policy errors all use the generic rejected
                    // stream code. The detailed FileStatus may race the
                    // reset, so keep the transfer terminal if that status is
                    // not observed.
                    STREAM_REJECTED => Some(pb::FileError::PermissionDenied),
                    _ => None,
                });
        }
    }

    fn reset(&mut self) {
        let code = match *self.command.borrow() {
            Command::Pause => STREAM_PAUSED,
            Command::Cancel => STREAM_CANCELLED,
            Command::Run => STREAM_REJECTED,
        };
        let _ = self.stream.reset(code);
    }
}

impl FileSink for UploadSink {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), ConnError> {
        if self.stopped() {
            return Err(ConnError::Closed);
        }
        let result = tokio::select! {
            biased;
            _ = self.command.changed() => { self.cancel.cancel(); Err(ConnError::Closed) }
            result = self.stream.write_raw(bytes) => result,
        };
        self.remember(&result);
        result
    }

    async fn finish(&mut self) -> Result<(), ConnError> {
        if self.stopped() {
            return Err(ConnError::Closed);
        }
        let result = tokio::select! {
            biased;
            _ = self.command.changed() => { self.cancel.cancel(); Err(ConnError::Closed) }
            result = self.stream.finish() => result,
        };
        self.remember(&result);
        result
    }
}

enum AttemptError {
    Offline,
    Stopped,
    Unsupported,
    Host(pb::FileError, u64),
    Source(SendError),
}

enum Queried {
    Offset(u64),
    Done(pb::FileStatus),
}

fn status_error(status: &pb::FileStatus) -> pb::FileError {
    pb::FileError::try_from(status.error).unwrap_or(pb::FileError::Protocol)
}

async fn query(conn: &FilesConn, id: &str) -> Result<Queried, AttemptError> {
    for attempt in 0..3 {
        let request_id = conn.replies.next_id();
        let reply = conn.replies.wait(request_id);
        conn.handle.send_file_ctl(FileCtl::Query(pb::FileQuery {
            request_id,
            transfer_id: id.to_owned(),
        }));
        let status = match timeout(QUERY_TIMEOUT, reply).await {
            Ok(Ok(FileReply::Status(status))) if status.transfer_id == id => status,
            _ => {
                conn.handle.close();
                conn.disconnect();
                return Err(AttemptError::Offline);
            }
        };
        let error = status_error(&status);
        if error != pb::FileError::None {
            return Err(AttemptError::Host(error, status.received));
        }
        match pb::file_status::State::try_from(status.state) {
            Ok(pb::file_status::State::Partial) => return Ok(Queried::Offset(status.received)),
            Ok(pb::file_status::State::Unknown) => return Ok(Queried::Offset(0)),
            Ok(pb::file_status::State::Receiving) => {
                if attempt < 2 {
                    sleep(Duration::from_secs(2)).await;
                }
            }
            Ok(pb::file_status::State::Completed) => return Ok(Queried::Done(status)),
            _ => return Err(AttemptError::Host(pb::FileError::Protocol, status.received)),
        }
    }
    Err(AttemptError::Offline)
}

fn completed_status(status: pb::FileStatus, id: &str) -> Result<pb::FileStatus, AttemptError> {
    if status.transfer_id != id {
        return Err(AttemptError::Host(pb::FileError::Protocol, 0));
    }
    let error = status_error(&status);
    if error != pb::FileError::None {
        return Err(AttemptError::Host(error, status.received));
    }
    if status.state == pb::file_status::State::Completed as i32 {
        Ok(status)
    } else {
        Err(AttemptError::Host(pb::FileError::Protocol, status.received))
    }
}

async fn attempt_upload(
    core: &Arc<Core>,
    id: &str,
    job: &Job,
    transfer: &FileTransfer,
    file: &PreparedFile,
) -> Result<pb::FileStatus, AttemptError> {
    let mut command = job.command.subscribe();
    if *command.borrow_and_update() != Command::Run {
        return Err(AttemptError::Stopped);
    }
    let FileSourceRef::Computer { host_id } = &transfer.source else {
        return Err(AttemptError::Unsupported);
    };
    let pool = core.state::<FilesConns>();
    let conn = tokio::select! {
        biased;
        _ = command.changed() => return Err(AttemptError::Stopped),
        result = pool.get(core, host_id) => result.map_err(|error| match error {
            CoreError::Unsupported { message } if message.starts_with("files.disabled") => AttemptError::Host(pb::FileError::Disabled, 0),
            CoreError::Unsupported { .. } => AttemptError::Unsupported,
            CoreError::Http { status: 403, .. } => AttemptError::Host(pb::FileError::NotAllowed, 0),
            _ => AttemptError::Offline,
        })?,
    };
    let _activity = conn.activity(core);
    core.state::<Transfers>()
        .update(core, id, |t| t.net_path = conn.net_path);
    // Register before querying. A host can reject the destination while
    // answering this query; request_id=0 FileStatus notifications must not be
    // lost before the stream is opened.
    let mut status = conn.wait_status(id);
    let queried = tokio::select! {
        biased;
        _ = command.changed() => return Err(AttemptError::Stopped),
        result = query(&conn, id) => result?,
    };
    let uploads = core.state::<Uploads>();
    let explicit = lock(&uploads.0).offsets.get(id).copied();
    let offset = match (explicit, queried) {
        (Some(offset), _) => offset,
        (None, Queried::Offset(received)) => received,
        (None, Queried::Done(status))
            if status.received == file.size && !status.saved_name.is_empty() =>
        {
            tracing::debug!(transfer_id = id, "host already has this upload");
            return Ok(status);
        }
        (None, Queried::Done(status)) => {
            tracing::warn!(
                transfer_id = id,
                received = status.received,
                size = file.size,
                "host reports a different completed upload; sending again"
            );
            0
        }
    };
    let stream = tokio::select! {
        biased;
        _ = command.changed() => return Err(AttemptError::Stopped),
        result = conn.handle.open_file_stream() => result.map_err(|_| AttemptError::Offline)?,
    };
    lock(&uploads.0).offsets.remove(id);
    tracing::debug!(%host_id, transfer_id = id, size = file.size, offset, "upload stream opened");
    let mut sink = UploadSink {
        stream,
        command: job.command.subscribe(),
        cancel: job.cancel.clone(),
        peer_error: None,
    };
    let mut progress = |p: rsp_core::file_progress::TransferProgress| {
        if *job.command.borrow() == Command::Run {
            core.state::<Transfers>().progress(core, id, p.done);
        }
    };
    let now_us = || core.now_us();
    let result = tokio::select! {
        result = send_with(file, offset, transfer.remote_path.as_deref().unwrap_or(""), &mut sink, &mut progress, &job.cancel, &now_us) => result,
        reply = &mut status => {
            sink.reset();
            if *command.borrow() != Command::Run { return Err(AttemptError::Stopped); }
            let status = completed_status(reply.map_err(|_| AttemptError::Offline)?, id)?;
            update_state(core, id, State::Verifying, None);
            return Ok(status);
        }
    };
    if *command.borrow() != Command::Run {
        sink.reset();
        if *command.borrow() == Command::Cancel {
            send_cancel(&conn, id);
        }
        return Err(AttemptError::Stopped);
    }
    match result {
        Ok(SendOutcome::Completed { .. }) => {}
        Ok(SendOutcome::Interrupted { .. }) => {
            sink.reset();
            if let Some(error) = sink.peer_error {
                return Err(AttemptError::Host(error, 0));
            }
            if let Ok(reply) = status.try_recv() {
                return completed_status(reply, id);
            }
            return Err(AttemptError::Offline);
        }
        Ok(SendOutcome::Cancelled) => {
            sink.reset();
            return Err(AttemptError::Stopped);
        }
        Err(error) => {
            sink.reset();
            return Err(AttemptError::Source(error));
        }
    }
    update_state(core, id, State::Verifying, None);
    let wait = Duration::from_secs(30)
        + Duration::from_micros(file.size.saturating_mul(1_000_000) / (50 * 1024 * 1024));
    tokio::select! {
        biased;
        _ = command.changed() => { sink.reset(); Err(AttemptError::Stopped) }
        result = timeout(wait, status) => completed_status(result.map_err(|_| AttemptError::Offline)?.map_err(|_| AttemptError::Offline)?, id),
    }
}

fn update_state(core: &Arc<Core>, id: &str, state: State, fail: Option<FileTransferFail>) {
    if state == State::Failed {
        lock(&core.state::<Uploads>().0).forget_fingerprint(core, id);
    }
    core.state::<Transfers>().update(core, id, |transfer| {
        if !matches!(
            transfer.state,
            State::Paused | State::Cancelled | State::Completed
        ) {
            transfer.state = state;
            transfer.fail = fail;
        }
    });
}

fn restart(core: &Arc<Core>, id: &str, offset: u64, clear: bool) {
    let uploads = core.state::<Uploads>();
    let mut inner = lock(&uploads.0);
    if clear {
        inner.prepared.remove(id);
    }
    inner.offsets.insert(id.to_owned(), offset);
}

async fn run_upload(core: &Arc<Core>, id: &str, job: &Job) {
    let mut retries = 0;
    let mut sha_retried = false;
    let mut resume_retried = false;
    let mut source_retried = false;
    loop {
        let mut command = job.command.subscribe();
        if *command.borrow_and_update() != Command::Run {
            return;
        }
        let Some(transfer) = core.state::<Transfers>().get(core, id) else {
            return;
        };
        if matches!(
            transfer.state,
            State::Cancelled | State::Completed | State::Paused
        ) {
            return;
        }
        update_state(core, id, State::Running, None);
        let file = tokio::select! {
            biased;
            _ = command.changed() => return,
            result = prepared(core, id, &transfer) => result,
        };
        let result = match file {
            Ok(file) => attempt_upload(core, id, job, &transfer, &file).await,
            Err(error) => Err(AttemptError::Source(error)),
        };
        if *command.borrow() != Command::Run {
            return;
        }
        let fail = match result {
            Ok(status) => {
                // Remove the durable preparation snapshot before publishing
                // the terminal transfer state, so a caller observing
                // Completed cannot race a stale uploads.json entry.
                let uploads = core.state::<Uploads>();
                let mut inner = lock(&uploads.0);
                inner.prepared.remove(id);
                inner.offsets.remove(id);
                inner.forget_fingerprint(core, id);
                drop(inner);
                core.state::<Transfers>().update(core, id, |transfer| {
                    if !matches!(transfer.state, State::Cancelled | State::Paused) {
                        transfer.state = State::Completed;
                        transfer.done = transfer.size;
                        transfer.fail = None;
                        transfer.saved_name = Some(status.saved_name);
                    }
                });
                return;
            }
            Err(AttemptError::Stopped) => return,
            Err(AttemptError::Host(pb::FileError::Sha256Mismatch, _)) if !sha_retried => {
                sha_retried = true;
                restart(core, id, 0, true);
                continue;
            }
            Err(AttemptError::Host(pb::FileError::ResumeMismatch, received)) if !resume_retried => {
                resume_retried = true;
                restart(core, id, received, false);
                continue;
            }
            Err(AttemptError::Source(SendError::SourceChanged)) if !source_retried => {
                source_retried = true;
                restart(core, id, 0, true);
                continue;
            }
            Err(AttemptError::Host(error, _)) if error != pb::FileError::Busy => {
                Some(transfer_fail(error))
            }
            Err(AttemptError::Unsupported) => Some(FileTransferFail::Unsupported),
            Err(AttemptError::Source(error)) => Some(match error {
                SendError::SourceChanged => FileTransferFail::SourceChanged,
                SendError::Io(_) | SendError::NotAFile => FileTransferFail::Io,
                SendError::Codec(FileError::FileTooLarge { .. }) => FileTransferFail::TooLarge,
                _ => FileTransferFail::Protocol,
            }),
            _ => None,
        };
        if let Some(fail) = fail {
            update_state(core, id, State::Failed, Some(fail));
            return;
        }
        update_state(core, id, State::Interrupted, None);
        let Some(seconds) = RETRY_SECONDS.get(retries) else {
            return;
        };
        retries += 1;
        tokio::select! {
            biased;
            _ = command.changed() => return,
            _ = sleep(Duration::from_secs(*seconds)) => {}
        }
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn files_upload(
    host_id: String,
    local_path: String,
    remote_dir: Option<String>,
) -> Result<String, CoreError> {
    let c = core()?;
    api_files::register_driver(&c);
    c.clone()
        .run(async move { upload(&c, &host_id, &local_path, remote_dir).await })
        .await
}
