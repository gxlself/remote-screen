//! On-demand FILES sessions, directory browsing and resumable computer downloads.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex, MutexGuard, OnceLock, Weak};
use std::time::Duration;

use async_trait::async_trait;
use rsp_client::files::{FileCtl, FileReplies, FileReply, FILES_FEATURES};
use rsp_client::{Session, SessionError, SessionEvent, SessionHandle};
use rsp_core::errors::ErrorCode;
use rsp_core::file::new_transfer_id;
use rsp_core::file_ctl::{self, pb_to_fail, STREAM_CANCELLED, STREAM_NOT_ALLOWED, STREAM_PAUSED};
use rsp_core::file_progress::{CancelFlag, FailReason};
use rsp_core::pb;
use rsp_files::recv::{
    discard_partial, partial_info, receive_with, FileSource, ReceiveConfig, ReceiveOutcome,
    RecvError,
};
use rsp_transport::{ConnError, UniRecv};
use tokio::sync::{oneshot, watch, Mutex as AsyncMutex};
use tokio::time::{sleep, timeout};

use super::api_sessions::host_params;
use super::dto_core::{net_path_from_pb, NetPath};
use super::dto_files::{
    FileEntry, FileEntryKind, FileListing, FileSourceRef, FileTransfer, FileTransferDirection,
    FileTransferFail, FileTransferState,
};
use super::error::CoreError;
use super::runtime::{core, Core};
use super::xfer::{SourceKind, TransferDriver, Transfers};

const IDLE_US: u64 = 60_000_000;
const PAGE_TIMEOUT: Duration = Duration::from_secs(15);
const STREAM_TIMEOUT: Duration = Duration::from_secs(30);
const RETRY_SECONDS: [u64; 3] = [2, 5, 10];

type Slot = Arc<HostSlot>;

#[derive(Default)]
pub(crate) struct HostSlot {
    pub(crate) entry: AsyncMutex<Option<Arc<FilesConn>>>,
    generation: AtomicU64,
    failure: Mutex<Option<CoreError>>,
}

// Dropping a caller during authentication must also stop the background session.
struct Opening(Option<SessionHandle>);
impl Drop for Opening {
    fn drop(&mut self) {
        if let Some(handle) = self.0.take() {
            handle.close();
        }
    }
}
type ReceiveResult = Result<ReceiveOutcome, RecvError>;

fn lock<T>(m: &Mutex<T>) -> MutexGuard<'_, T> {
    m.lock().unwrap_or_else(|e| e.into_inner())
}

pub fn files_dir(core: &Arc<Core>) -> Result<PathBuf, CoreError> {
    let path = core
        .config()
        .files_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| core.data_dir().join("files"));
    std::fs::create_dir_all(&path).map_err(|_| CoreError::internal("files.io"))?;
    Ok(path)
}

#[derive(Default)]
pub struct FilesConns {
    pub(crate) slots: Mutex<HashMap<String, Slot>>,
    register: OnceLock<()>,
    jobs: Mutex<HashMap<String, Arc<DownloadJob>>>,
}

struct Activity {
    alive: bool,
    count: usize,
    last_us: u64,
}

pub struct FilesConn {
    pub host_id: String,
    pub handle: SessionHandle,
    pub replies: FileReplies,
    pub net_path: Option<NetPath>,
    activity: Mutex<Activity>,
    closed: watch::Sender<bool>,
    downloads: Mutex<HashMap<String, Arc<Attempt>>>,
    statuses: Mutex<HashMap<String, oneshot::Sender<pb::FileStatus>>>,
}

/// Hold this guard for the duration of an upload or other operation using a connection.
pub struct FilesActivity {
    conn: Arc<FilesConn>,
    core: Weak<Core>,
}

impl Drop for FilesActivity {
    fn drop(&mut self) {
        let mut activity = lock(&self.conn.activity);
        activity.count = activity.count.saturating_sub(1);
        if let Some(core) = self.core.upgrade() {
            activity.last_us = core.now_us();
        }
    }
}

impl FilesConn {
    pub fn activity(self: &Arc<Self>, core: &Arc<Core>) -> FilesActivity {
        let mut activity = lock(&self.activity);
        activity.count += 1;
        activity.last_us = core.now_us();
        FilesActivity {
            conn: self.clone(),
            core: Arc::downgrade(core),
        }
    }

    pub fn wait_status(&self, transfer_id: &str) -> oneshot::Receiver<pb::FileStatus> {
        let (tx, rx) = oneshot::channel();
        let activity = lock(&self.activity);
        if activity.alive {
            lock(&self.statuses).insert(transfer_id.to_owned(), tx);
        }
        rx
    }

    pub(crate) fn disconnect(&self) {
        lock(&self.activity).alive = false;
        self.closed.send_replace(true);
        self.replies.fail_all();
        lock(&self.statuses).clear();
    }
}

impl FilesConns {
    pub async fn get(&self, core: &Arc<Core>, host_id: &str) -> Result<Arc<FilesConn>, CoreError> {
        let slot = lock(&self.slots)
            .entry(host_id.to_owned())
            .or_default()
            .clone();
        // Only this host's callers wait for its dial and authentication.
        let generation = slot.generation.load(Ordering::Acquire);
        let mut entry = slot.entry.lock().await;
        if let Some(conn) = entry.as_ref() {
            let mut activity = lock(&conn.activity);
            if activity.alive {
                activity.last_us = core.now_us();
                return Ok(conn.clone());
            }
        }
        if generation != slot.generation.load(Ordering::Acquire) {
            if let Some(error) = lock(&slot.failure).clone() {
                return Err(error);
            }
        }
        let (handle, mut events, net_path) = match open_connection(core, host_id).await {
            Ok(opened) => opened,
            Err(error) => {
                *lock(&slot.failure) = Some(error.clone());
                slot.generation.fetch_add(1, Ordering::Release);
                return Err(error);
            }
        };
        let conn = Arc::new(FilesConn {
            host_id: host_id.to_owned(),
            handle,
            replies: FileReplies::default(),
            net_path,
            activity: Mutex::new(Activity {
                alive: true,
                count: 0,
                last_us: core.now_us(),
            }),
            closed: watch::channel(false).0,
            downloads: Mutex::new(HashMap::new()),
            statuses: Mutex::new(HashMap::new()),
        });
        *entry = Some(conn.clone());
        let weak_core = Arc::downgrade(core);
        let pump_conn = conn.clone();
        let weak_slot = Arc::downgrade(&slot);
        core.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs(1));
            loop {
                tokio::select! {
                    event = events.recv() => {
                        let Some(event) = event else { break };
                        pump_conn.replies.route(&event);
                        match event {
                            SessionEvent::FileStream(stream) => {
                                if let Some(core) = weak_core.upgrade() {
                                    let conn = pump_conn.clone();
                                    core.clone().spawn(async move { receive_stream(core, conn, stream.0).await; });
                                } else { break; }
                            }
                            SessionEvent::FileStatus(status) => {
                                if status.request_id == 0 {
                                    if let Some(tx) = lock(&pump_conn.statuses).remove(&status.transfer_id) {
                                        let _ = tx.send(status.clone());
                                    }
                                }
                                if status.state == pb::file_status::State::Failed as i32 {
                                    if let Some(attempt) = lock(&pump_conn.downloads).get(&status.transfer_id) {
                                        if let Some(tx) = lock(&attempt.status).take() { let _ = tx.send(status); }
                                    }
                                }
                            }
                            SessionEvent::Ended { .. } => break,
                            _ => {}
                        }
                    }
                    _ = tick.tick() => {
                        let Some(core) = weak_core.upgrade() else { break };
                        let Some(slot) = weak_slot.upgrade() else { break };
                        let mut entry = slot.entry.lock().await;
                        let mut activity = lock(&pump_conn.activity);
                        let mut statuses = lock(&pump_conn.statuses);
                        statuses.retain(|_, tx| !tx.is_closed());
                        if activity.count == 0 && statuses.is_empty()
                            && core.now_us().saturating_sub(activity.last_us) >= IDLE_US {
                            activity.alive = false;
                            if entry.as_ref().is_some_and(|c| Arc::ptr_eq(c, &pump_conn)) { *entry = None; }
                            break;
                        }
                    }
                }
            }
            pump_conn.handle.close();
            pump_conn.disconnect();
        });
        let shutdown = Arc::downgrade(&conn);
        core.add_shutdown_hook(Box::new(move || {
            if let Some(conn) = shutdown.upgrade() {
                conn.handle.close();
                conn.disconnect();
            }
        }));
        Ok(conn)
    }
}

async fn open_connection(
    core: &Arc<Core>,
    host_id: &str,
) -> Result<
    (
        SessionHandle,
        rsp_client::session::EventStream,
        Option<NetPath>,
    ),
    CoreError,
> {
    let mut params = host_params(core, host_id, pb::Intent::Files)
        .map_err(|e| connection_error(ErrorCode::from_u16(e.code as u16)))?;
    params.features = FILES_FEATURES;
    params.required_features = rsp_core::features::FILE_BROWSE;
    let (handle, mut events) = Session::open(params).await.map_err(|e| match e {
        SessionError::Dial { code, .. } => connection_error(Some(code)),
        _ => connection_error(None),
    })?;
    let mut opening = Opening(Some(handle.clone()));
    let mut net_path = None;
    let ready = timeout(STREAM_TIMEOUT, async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::ServerHello(hello) => {
                    net_path = hello.peer.and_then(|peer| {
                        pb::PathKind::try_from(peer.path)
                            .ok()
                            .and_then(net_path_from_pb)
                    });
                }
                SessionEvent::Role(_) => return Ok(()),
                SessionEvent::Error { code, .. } => return Err(connection_error(Some(code))),
                SessionEvent::Ended { code, .. } => return Err(connection_error(code)),
                _ => {}
            }
        }
        Err(connection_error(None))
    })
    .await
    .unwrap_or_else(|_| Err(connection_error(None)));
    ready?;
    opening.0 = None;
    Ok((handle, events, net_path))
}

fn connection_error(code: Option<ErrorCode>) -> CoreError {
    match code {
        Some(ErrorCode::HostTooOld | ErrorCode::AppTooOld) => {
            CoreError::unsupported("files.unsupported: required features rejected")
        }
        Some(ErrorCode::RemoteControlDisabled) => {
            CoreError::unsupported("files.disabled: remote control disabled")
        }
        Some(
            ErrorCode::NotPaired
            | ErrorCode::DeviceRevoked
            | ErrorCode::PolicyDenied
            | ErrorCode::IntranetDenied
            | ErrorCode::ApprovalDenied,
        ) => http(403, "files.notAllowed"),
        _ => CoreError::network("files.offline: connection unavailable"),
    }
}

fn http(status: u16, message: &str) -> CoreError {
    CoreError::Http {
        status,
        hub_code: None,
        message: message.to_owned(),
    }
}

fn listing_error(error: pb::FileError) -> CoreError {
    match error {
        pb::FileError::NotFound => http(404, "files.notFound"),
        pb::FileError::PermissionDenied => http(403, "files.permissionDenied"),
        pb::FileError::OutsideRoot => http(403, "files.outsideRoot"),
        pb::FileError::NotAllowed => http(403, "files.notAllowed"),
        pb::FileError::Disabled => CoreError::unsupported("files.disabled"),
        pb::FileError::Busy => http(429, "files.busy"),
        _ => CoreError::internal("files.protocol"),
    }
}

pub async fn list(
    core: &Arc<Core>,
    host_id: &str,
    path: Option<String>,
) -> Result<FileListing, CoreError> {
    register_driver(core);
    let path = path.unwrap_or_default();
    file_ctl::validate_list(&pb::FileList {
        path: path.clone(),
        ..Default::default()
    })
    .map_err(|_| CoreError::internal("files.invalidPath"))?;
    let conn = core.state::<FilesConns>().get(core, host_id).await?;
    let _activity = conn.activity(core);
    let mut listing = FileListing {
        path: path.clone(),
        parent: None,
        entries: Vec::new(),
        truncated: false,
        net_path: conn.net_path,
    };
    let mut offset = 0;
    for _ in 0..10 {
        let request_id = conn.replies.next_id();
        let reply = conn.replies.wait(request_id);
        conn.handle.send_file_ctl(FileCtl::List(pb::FileList {
            request_id,
            path: path.clone(),
            offset,
        }));
        let page = match timeout(PAGE_TIMEOUT, reply).await {
            Ok(Ok(FileReply::Listing(page))) => page,
            _ => {
                // FileReplies has no per-request removal API. Closing also clears timed-out waiters.
                conn.handle.close();
                conn.disconnect();
                return Err(connection_error(None));
            }
        };
        let error = pb::FileError::try_from(page.error).unwrap_or(pb::FileError::Protocol);
        if error != pb::FileError::None {
            return Err(listing_error(error));
        }
        listing.path = page.path;
        listing.parent = (!page.parent.is_empty()).then_some(page.parent);
        listing
            .entries
            .extend(page.entries.into_iter().map(|entry| {
                let kind = match pb::file_entry::Kind::try_from(entry.kind) {
                    Ok(pb::file_entry::Kind::File) => FileEntryKind::File,
                    Ok(pb::file_entry::Kind::Dir) => FileEntryKind::Dir,
                    Ok(pb::file_entry::Kind::Symlink) => FileEntryKind::Symlink,
                    _ => FileEntryKind::Other,
                };
                FileEntry {
                    size: (kind != FileEntryKind::Dir).then_some(entry.size),
                    kind,
                    name: entry.name,
                    mtime_us: (entry.mtime_us != 0).then_some(entry.mtime_us),
                    child_count: u32::try_from(entry.child_count).ok(),
                }
            }));
        offset = page.next_offset;
        if offset == 0 {
            return Ok(listing);
        }
    }
    listing.truncated = true;
    Ok(listing)
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum Command {
    Run,
    Pause,
    Cancel,
}

struct DownloadJob {
    command: watch::Sender<Command>,
    gate: Arc<AsyncMutex<()>>,
    cancel: CancelFlag,
}

struct Attempt {
    job: Arc<DownloadJob>,
    stop: watch::Sender<bool>,
    // Protect claiming a stream and taking its completion sender together.
    complete: Mutex<Option<oneshot::Sender<ReceiveResult>>>,
    status: Mutex<Option<oneshot::Sender<pb::FileStatus>>>,
    receiving: watch::Sender<bool>,
}

struct DownloadSource {
    stream: UniRecv,
    attempt: Arc<Mutex<Option<Arc<Attempt>>>>,
    cancel: CancelFlag,
    closed: watch::Receiver<bool>,
}

impl FileSource for DownloadSource {
    async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, ConnError> {
        let attempt = lock(&self.attempt).clone();
        if let Some(attempt) = attempt {
            let mut command = attempt.job.command.subscribe();
            let mut stopped = attempt.stop.subscribe();
            loop {
                let action = *command.borrow_and_update();
                if action == Command::Cancel || attempt.job.cancel.is_cancelled() {
                    self.cancel.cancel();
                    let _ = self.stream.stop(STREAM_CANCELLED);
                    return Ok(Some(0));
                }
                if action == Command::Pause || *stopped.borrow_and_update() || *self.closed.borrow()
                {
                    let _ = self.stream.stop(STREAM_PAUSED);
                    return Err(ConnError::Closed);
                }
                tokio::select! {
                    result = self.stream.read_raw(buf) => return result,
                    _ = command.changed() => {},
                    _ = stopped.changed() => {},
                    _ = self.closed.changed() => {},
                }
            }
        } else {
            timeout(PAGE_TIMEOUT, self.stream.read_raw(buf))
                .await
                .unwrap_or(Err(ConnError::Closed))
        }
    }
}

async fn receive_stream(core: Arc<Core>, conn: Arc<FilesConn>, stream: UniRecv) {
    let selected = Arc::new(Mutex::new(None::<Arc<Attempt>>));
    let cancel = CancelFlag::new();
    let mut source = DownloadSource {
        stream,
        attempt: selected.clone(),
        cancel: cancel.clone(),
        closed: conn.closed.subscribe(),
    };
    let Ok(dest_dir) = files_dir(&core) else {
        return;
    };
    let mut accept = |header: &pb::FileHeader| {
        let pending = lock(&conn.downloads);
        let attempt = pending
            .get(&header.transfer_id)
            .filter(|a| *a.job.command.borrow() == Command::Run && !*a.stop.borrow())
            .ok_or(RecvError::Rejected(pb::FileError::NotAllowed))?;
        if attempt.receiving.send_replace(true) {
            return Err(RecvError::Rejected(pb::FileError::NotAllowed));
        }
        *lock(&selected) = Some(attempt.clone());
        core.state::<Transfers>()
            .update(&core, &header.transfer_id, |t| {
                t.size = header.size;
                t.name = header.name.clone();
            });
        Ok(None)
    };
    let mut progress = |p: rsp_core::file_progress::TransferProgress| {
        core.state::<Transfers>()
            .progress(&core, &p.transfer_id, p.done);
    };
    let result = receive_with(
        &mut source,
        &ReceiveConfig {
            dest_dir,
            ..Default::default()
        },
        &mut accept,
        &mut progress,
        &cancel,
        &|| core.now_us(),
    )
    .await;
    if matches!(result, Err(RecvError::Rejected(_))) {
        let _ = source.stream.stop(STREAM_NOT_ALLOWED);
    }
    let attempt = lock(&selected).take();
    if let Some(attempt) = attempt {
        if let Some(tx) = lock(&attempt.complete).take() {
            let _ = tx.send(result);
        }
    }
}

pub async fn download(
    core: &Arc<Core>,
    host_id: &str,
    remote_path: &str,
) -> Result<String, CoreError> {
    register_driver(core);
    let id = new_transfer_id().map_err(|_| CoreError::internal("files.random"))?;
    file_ctl::validate_request(&pb::FileRequest {
        path: remote_path.to_owned(),
        transfer_id: id.clone(),
        ..Default::default()
    })
    .map_err(|_| CoreError::internal("files.invalidPath"))?;
    files_dir(core)?;
    let name = Path::new(remote_path)
        .file_name()
        .and_then(|n| n.to_str())
        .filter(|n| !n.is_empty())
        .ok_or_else(|| CoreError::internal("files.invalidPath"))?
        .to_owned();
    core.state::<Transfers>().create(
        core,
        FileTransfer {
            transfer_id: id.clone(),
            direction: FileTransferDirection::Download,
            source: FileSourceRef::Computer {
                host_id: host_id.to_owned(),
            },
            name,
            remote_path: Some(remote_path.to_owned()),
            local_path: None,
            size: 0,
            done: 0,
            bytes_per_sec: 0,
            state: FileTransferState::Queued,
            fail: None,
            net_path: None,
            saved_name: None,
            started_at: core.now_us(),
            updated_at: core.now_us(),
        },
    );
    start_download(core, &id);
    Ok(id)
}

fn start_download(core: &Arc<Core>, id: &str) {
    let pool = core.state::<FilesConns>();
    let mut jobs = lock(&pool.jobs);
    let current = core.state::<Transfers>().get(core, id);
    if current.as_ref().is_none_or(|t| {
        matches!(
            t.state,
            FileTransferState::Completed | FileTransferState::Cancelled
        )
    }) {
        return;
    }
    if let Some(previous) = jobs.get(id) {
        if *previous.command.borrow() == Command::Run
            && current.as_ref().is_some_and(|t| {
                matches!(
                    t.state,
                    FileTransferState::Queued | FileTransferState::Running
                )
            })
        {
            return;
        }
        previous.command.send_replace(Command::Pause);
    }
    let gate = jobs.get(id).map(|j| j.gate.clone()).unwrap_or_default();
    let job = Arc::new(DownloadJob {
        command: watch::channel(Command::Run).0,
        gate,
        cancel: CancelFlag::new(),
    });
    jobs.insert(id.to_owned(), job.clone());
    if core.state::<Transfers>().get(core, id).is_some_and(|t| {
        matches!(
            t.state,
            FileTransferState::Paused | FileTransferState::Failed | FileTransferState::Interrupted
        )
    }) {
        core.state::<Transfers>().update(core, id, |t| {
            if matches!(
                t.state,
                FileTransferState::Paused
                    | FileTransferState::Failed
                    | FileTransferState::Interrupted
            ) {
                t.state = FileTransferState::Queued;
                t.fail = None;
            }
        });
    }
    let id = id.to_owned();
    let c = core.clone();
    core.spawn(async move {
        // A rapid pause/resume must finish flushing the old stream before reopening its partial.
        let _gate = job.gate.lock().await;
        run_download(&c, &id, &job).await;
        if job.cancel.is_cancelled() {
            if let Ok(dir) = files_dir(&c) {
                let _ = discard_partial(&dir, &id);
            }
        }
        let pool = c.state::<FilesConns>();
        let mut jobs = lock(&pool.jobs);
        if jobs.get(&id).is_some_and(|j| Arc::ptr_eq(j, &job)) {
            jobs.remove(&id);
        }
    });
}

fn update_state(
    core: &Arc<Core>,
    id: &str,
    state: FileTransferState,
    fail: Option<FileTransferFail>,
) {
    core.state::<Transfers>().update(core, id, |t| {
        if !matches!(
            t.state,
            FileTransferState::Paused | FileTransferState::Cancelled | FileTransferState::Completed
        ) {
            t.state = state;
            t.fail = fail;
        }
    });
}

fn fail(core: &Arc<Core>, id: &str, reason: FileTransferFail) {
    update_state(core, id, FileTransferState::Failed, Some(reason));
}

enum AttemptError {
    Offline,
    Host(pb::FileError),
    Receive(RecvError),
    Stopped,
    Unsupported,
}

async fn attempt_download(
    core: &Arc<Core>,
    id: &str,
    job: &Arc<DownloadJob>,
    dir: &Path,
    host_id: &str,
    path: &str,
) -> Result<(ReceiveOutcome, Arc<FilesConn>), AttemptError> {
    let mut command = job.command.subscribe();
    if *command.borrow_and_update() != Command::Run {
        return Err(AttemptError::Stopped);
    }
    let pool = core.state::<FilesConns>();
    let conn = tokio::select! {
        _ = command.changed() => return Err(AttemptError::Stopped),
        result = pool.get(core, host_id) => result.map_err(|e| match e {
            CoreError::Unsupported { message } if message.starts_with("files.disabled") => AttemptError::Host(pb::FileError::Disabled),
            CoreError::Unsupported { .. } => AttemptError::Unsupported,
            CoreError::Http { status: 403, .. } => AttemptError::Host(pb::FileError::NotAllowed),
            _ => AttemptError::Offline,
        })?,
    };
    let _activity = conn.activity(core);
    core.state::<Transfers>()
        .update(core, id, |t| t.net_path = conn.net_path);
    let partial = partial_info(dir, id).map_err(AttemptError::Receive)?;
    let mut request = pb::FileRequest {
        request_id: conn.replies.next_id(),
        path: path.to_owned(),
        transfer_id: id.to_owned(),
        ..Default::default()
    };
    if let Some(info) = partial {
        request.offset = info.received;
        request.expect_size = info.size;
        request.expect_mtime_us = info.mtime_us;
        request.expect_sha256 = info.sha256.to_vec();
    }
    let (tx, mut done) = oneshot::channel();
    let (status_tx, mut status) = oneshot::channel();
    let attempt = Arc::new(Attempt {
        job: job.clone(),
        stop: watch::channel(false).0,
        complete: Mutex::new(Some(tx)),
        status: Mutex::new(Some(status_tx)),
        receiving: watch::channel(false).0,
    });
    lock(&conn.downloads).insert(id.to_owned(), attempt.clone());
    let mut closed = conn.closed.subscribe();
    conn.handle.send_file_ctl(FileCtl::Request(request));
    let deadline = sleep(STREAM_TIMEOUT);
    tokio::pin!(deadline);
    let mut awaiting_stream = true;
    let mut received_result = false;
    let mut result = loop {
        if *command.borrow() != Command::Run {
            break Err(AttemptError::Stopped);
        }
        if *closed.borrow() {
            break Err(AttemptError::Offline);
        }
        tokio::select! {
            result = &mut done => { received_result = true; break result.map_err(|_| AttemptError::Offline).and_then(|r| r.map_err(AttemptError::Receive)); },
            result = &mut status => break Err(result.map(|s| AttemptError::Host(pb::FileError::try_from(s.error).unwrap_or(pb::FileError::Protocol))).unwrap_or(AttemptError::Offline)),
            _ = command.changed() => break Err(AttemptError::Stopped),
            _ = closed.changed() => break Err(AttemptError::Offline),
            _ = &mut deadline, if awaiting_stream => {
                if *attempt.receiving.borrow() { awaiting_stream = false; }
                else { break Err(AttemptError::Offline); }
            }
        }
    };
    lock(&conn.downloads).remove(id);
    if result.is_err() {
        attempt.stop.send_replace(true);
        if *attempt.receiving.borrow() && !received_result {
            // Do not drop a receiving future: interruption flushes its resumable metadata.
            if let Ok(Ok(outcome @ ReceiveOutcome::Completed { .. })) = (&mut done).await {
                result = Ok(outcome);
            }
        }
        conn.handle.send_file_ctl(FileCtl::Cancel(pb::FileCancel {
            transfer_id: id.to_owned(),
        }));
    }
    if job.cancel.is_cancelled() || *job.command.borrow() == Command::Cancel {
        let _ = discard_partial(dir, id);
    }
    result.map(|outcome| (outcome, conn))
}

async fn run_download(core: &Arc<Core>, id: &str, job: &Arc<DownloadJob>) {
    if *job.command.borrow() != Command::Run {
        return;
    }
    let Some(transfer) = core.state::<Transfers>().get(core, id) else {
        return;
    };
    if matches!(
        transfer.state,
        FileTransferState::Completed | FileTransferState::Cancelled
    ) {
        return;
    }
    let FileSourceRef::Computer { host_id } = transfer.source else {
        return;
    };
    let Some(path) = transfer.remote_path else {
        fail(core, id, FileTransferFail::Protocol);
        return;
    };
    let dir = match files_dir(core) {
        Ok(dir) => dir,
        Err(_) => {
            fail(core, id, FileTransferFail::Io);
            return;
        }
    };
    let mut retries = 0;
    let mut restarted = false;
    loop {
        if *job.command.borrow() != Command::Run {
            return;
        }
        // Preserve a pause/cancel already published by the transfer table.
        core.state::<Transfers>().update(core, id, |t| {
            if matches!(
                t.state,
                FileTransferState::Queued
                    | FileTransferState::Running
                    | FileTransferState::Interrupted
            ) {
                t.state = FileTransferState::Running;
                t.fail = None;
            }
        });
        let result = attempt_download(core, id, job, &dir, &host_id, &path).await;
        if *job.command.borrow() != Command::Run
            && !matches!(result, Ok((ReceiveOutcome::Completed { .. }, _)))
        {
            return;
        }
        match result {
            Ok((ReceiveOutcome::Completed { path, size, .. }, conn)) => {
                if *job.command.borrow() == Command::Cancel {
                    let _ = tokio::fs::remove_file(path).await;
                    return;
                }
                core.state::<Transfers>().update(core, id, |t| {
                    if t.state != FileTransferState::Cancelled {
                        t.local_path = Some(path.to_string_lossy().into_owned());
                        t.size = size;
                        t.done = size;
                        t.state = FileTransferState::Completed;
                        t.fail = None;
                    }
                });
                conn.handle.send_file_ctl(FileCtl::Status(pb::FileStatus {
                    transfer_id: id.to_owned(),
                    state: pb::file_status::State::Completed as i32,
                    received: size,
                    ..Default::default()
                }));
                return;
            }
            Ok((ReceiveOutcome::Cancelled { .. }, _)) | Err(AttemptError::Stopped) => return,
            Err(AttemptError::Host(pb::FileError::SourceChanged))
            | Err(AttemptError::Receive(RecvError::ResumeMismatch { .. }))
                if !restarted =>
            {
                restarted = true;
                if discard_partial(&dir, id).is_err() {
                    fail(core, id, FileTransferFail::Io);
                    return;
                }
                core.state::<Transfers>().progress(core, id, 0);
                continue;
            }
            Err(AttemptError::Host(error)) if error != pb::FileError::Busy => {
                fail(core, id, transfer_fail(error));
                return;
            }
            Err(AttemptError::Unsupported) => {
                fail(core, id, FileTransferFail::Unsupported);
                return;
            }
            Err(AttemptError::Receive(error)) if !matches!(error, RecvError::Transport(_)) => {
                fail(core, id, receive_fail(&error));
                return;
            }
            _ => {}
        }
        if *job.command.borrow() != Command::Run {
            return;
        }
        update_state(core, id, FileTransferState::Interrupted, None);
        let Some(seconds) = RETRY_SECONDS.get(retries) else {
            return;
        };
        retries += 1;
        let mut command = job.command.subscribe();
        if *command.borrow_and_update() != Command::Run {
            return;
        }
        tokio::select! { _ = sleep(Duration::from_secs(*seconds)) => {}, _ = command.changed() => return }
    }
}

pub(crate) fn transfer_fail(error: pb::FileError) -> FileTransferFail {
    match error {
        pb::FileError::NotFound => FileTransferFail::NotFound,
        pb::FileError::PermissionDenied | pb::FileError::OutsideRoot => {
            FileTransferFail::PermissionDenied
        }
        pb::FileError::NotAllowed => FileTransferFail::NotAllowed,
        pb::FileError::Disabled => FileTransferFail::Disabled,
        pb::FileError::NoSpace => FileTransferFail::NoSpace,
        _ => match pb_to_fail(error) {
            Some(FailReason::Io) => FileTransferFail::Io,
            Some(FailReason::Sha256Mismatch) => FileTransferFail::Sha256Mismatch,
            Some(FailReason::ResumeMismatch) => FileTransferFail::ResumeMismatch,
            Some(FailReason::SourceChanged) => FileTransferFail::SourceChanged,
            Some(FailReason::TooLarge) => FileTransferFail::TooLarge,
            _ => FileTransferFail::Protocol,
        },
    }
}

fn receive_fail(error: &RecvError) -> FileTransferFail {
    match error {
        RecvError::Sha256Mismatch => FileTransferFail::Sha256Mismatch,
        RecvError::ResumeMismatch { .. } => FileTransferFail::ResumeMismatch,
        RecvError::Io(e) if e.kind() == std::io::ErrorKind::StorageFull => {
            FileTransferFail::NoSpace
        }
        RecvError::Io(_) | RecvError::DestMissing | RecvError::NoFreeName => FileTransferFail::Io,
        RecvError::FileTooLarge { .. } => FileTransferFail::TooLarge,
        RecvError::Rejected(error) => transfer_fail(*error),
        _ => FileTransferFail::Protocol,
    }
}

struct ComputerDriver;

fn is_upload(core: &Arc<Core>, id: &str) -> bool {
    core.state::<Transfers>()
        .get(core, id)
        .is_some_and(|t| t.direction == FileTransferDirection::Upload)
}

#[async_trait]
impl TransferDriver for ComputerDriver {
    fn pause(&self, core: &Arc<Core>, id: &str) {
        if is_upload(core, id) {
            super::files_upload::pause(core, id);
            return;
        }
        if let Some(job) = lock(&core.state::<FilesConns>().jobs).get(id) {
            job.command.send_replace(Command::Pause);
        }
    }

    fn cancel(&self, core: &Arc<Core>, id: &str) {
        if is_upload(core, id) {
            super::files_upload::cancel(core, id);
            return;
        }
        let pool = core.state::<FilesConns>();
        let jobs = lock(&pool.jobs);
        if let Some(job) = jobs.get(id) {
            job.cancel.cancel();
            job.command.send_replace(Command::Cancel);
        }
        if let Some(FileTransfer {
            source: FileSourceRef::Computer { host_id },
            ..
        }) = core.state::<Transfers>().get(core, id)
        {
            if let Some(slot) = lock(&pool.slots).get(&host_id) {
                if let Ok(entry) = slot.entry.try_lock() {
                    if let Some(conn) = entry.as_ref() {
                        conn.handle.send_file_ctl(FileCtl::Cancel(pb::FileCancel {
                            transfer_id: id.to_owned(),
                        }));
                    }
                }
            }
        }
        // A running worker owns cleanup until the stream has flushed. Otherwise remove directly.
        if !jobs.contains_key(id) {
            if let Ok(dir) = files_dir(core) {
                let _ = discard_partial(&dir, id);
            }
        }
    }

    async fn resume(&self, core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
        if is_upload(core, id) {
            return super::files_upload::resume(core, id).await;
        }
        start_download(core, id);
        Ok(())
    }
}

pub fn register_driver(core: &Arc<Core>) {
    core.state::<FilesConns>().register.get_or_init(|| {
        core.state::<Transfers>()
            .register_driver(SourceKind::Computer, Arc::new(ComputerDriver));
    });
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn files_list(host_id: String, path: Option<String>) -> Result<String, CoreError> {
    let c = core()?;
    register_driver(&c);
    c.clone()
        .run(async move { Ok(serde_json::to_string(&list(&c, &host_id, path).await?)?) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn files_download(host_id: String, remote_path: String) -> Result<String, CoreError> {
    let c = core()?;
    register_driver(&c);
    c.clone()
        .run(async move { download(&c, &host_id, &remote_path).await })
        .await
}
