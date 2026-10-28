//! File channel (0x04) hooks: receive controller uploads and send host files.

use super::{files_done, ConnCx};
use crate::{
    auth,
    file_browse::{self, BrowsePolicy, ListOptions},
    file_recv::{self, FileSource, ReceiveConfig, ReceiveOutcome, RecvError},
    file_send::{self, FileSink, PreparedFile, SendError, SendOutcome},
    platform::Platform,
    session::manager::{ConnInfo, ConnKey, EndReason, HostServices, SessionManager},
    user_dirs::{EnvDirs, UserDirs, UserDirsSource},
};
use rsp_core::{
    features,
    file::{FileStreamReceiver, MAX_HEADER_BYTES},
    file_ctl,
    file_progress::{CancelFlag, Direction, TransferProgress},
    pb,
};
use rsp_transport::{ConnError, StreamKind, UniRecv, UniSend};
use std::{
    collections::{HashMap, VecDeque},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicU64, AtomicUsize, Ordering},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};
use tokio::sync::{oneshot, watch, Notify};
use tracing::{debug, info, warn};

const FILES_IDLE_US: u64 = 10 * 60 * 1_000_000;
const REJECT_HEADER_WAIT: Duration = Duration::from_secs(1);
const MAX_PENDING_REJECTS: usize = 4;
type ActiveTransfers = Arc<Mutex<HashMap<String, Active>>>;

struct Active {
    conn: ConnKey,
    device_id: String,
    direction: Direction,
    cancel: CancelFlag,
    stop: Arc<Notify>,
    received: Arc<AtomicU64>,
    done: watch::Receiver<bool>,
}

fn lock_active(active: &ActiveTransfers) -> MutexGuard<'_, HashMap<String, Active>> {
    lock(active)
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub struct Host {
    downloads_dir: PathBuf,
    completed_path: Option<PathBuf>,
    completed_persist_lock: Arc<Mutex<()>>,
    user_dirs: OnceLock<Arc<dyn UserDirsSource>>,
    active: ActiveTransfers,
    completed: Mutex<files_done::CompletedUploads>,
    prepared: Mutex<PreparedCache>,
    send_counts: Mutex<HashMap<ConnKey, Arc<AtomicUsize>>>,
}

impl Host {
    pub fn new(services: &HostServices) -> Self {
        let completed_path = services
            .state_dir
            .as_ref()
            .map(|dir| dir.join("completed-uploads.json"));
        let completed = completed_path
            .as_deref()
            .map_or_else(files_done::CompletedUploads::default, |path| {
                files_done::CompletedUploads::load(path, services.clock.now_micros())
            });
        Self {
            downloads_dir: services.downloads_dir.clone(),
            completed_path,
            completed_persist_lock: Arc::new(Mutex::new(())),
            user_dirs: OnceLock::new(),
            active: Arc::new(Mutex::new(HashMap::new())),
            completed: Mutex::new(completed),
            prepared: Mutex::new(PreparedCache::default()),
            send_counts: Mutex::new(HashMap::new()),
        }
    }

    fn persist_completed_sync(&self) {
        let Some(path) = self.completed_path.clone() else {
            return;
        };
        let snapshot = lock(&self.completed).clone();
        let _lock = lock(&self.completed_persist_lock);
        if let Err(error) = snapshot.save(&path) {
            warn!(%error, "could not persist completed upload registry");
        }
    }

    async fn persist_completed(&self) {
        let Some(path) = self.completed_path.clone() else {
            return;
        };
        let snapshot = lock(&self.completed).clone();
        let persist_lock = self.completed_persist_lock.clone();
        match tokio::task::spawn_blocking(move || {
            let _lock = lock(&persist_lock);
            snapshot.save(&path)
        })
        .await
        {
            Ok(Ok(())) => {}
            Ok(Err(error)) => warn!(%error, "could not persist completed upload registry"),
            Err(error) => warn!(%error, "completed upload registry worker failed"),
        }
    }

    /// Install the directory resolver once. A second installation is ignored.
    pub fn set_user_dirs(&self, source: Arc<dyn UserDirsSource>) -> bool {
        self.user_dirs.set(source).is_ok()
    }

    /// Return one directory snapshot for the current operation.
    pub fn dirs(&self) -> UserDirs {
        self.user_dirs.get().map_or_else(
            || EnvDirs::new(self.downloads_dir.clone()).current(),
            |source| source.current(),
        )
    }

    pub fn user_can_write(&self, dir: &Path) -> bool {
        self.user_dirs
            .get()
            .is_none_or(|source| source.user_can_write(dir))
    }
}

#[derive(Default)]
pub struct Conn {
    active_receives: Arc<AtomicUsize>,
    active_sends: Arc<AtomicUsize>,
    active_lists: Arc<AtomicUsize>,
    pending_rejects: Arc<AtomicUsize>,
    list_serial: Arc<tokio::sync::Mutex<()>>,
    last_activity_us: u64,
}

pub enum Event {
    Send(Box<pb::Envelope>),
    TransferEnded,
    OpenSend(oneshot::Sender<Option<UniSend>>),
    OpenSendResult(oneshot::Sender<Result<UniSend, rsp_transport::ConnError>>),
}

impl std::fmt::Debug for Event {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        match self {
            Self::Send(_) => f.debug_tuple("Send").finish_non_exhaustive(),
            Self::TransferEnded => f.write_str("TransferEnded"),
            Self::OpenSend(_) => f.debug_tuple("OpenSend").finish_non_exhaustive(),
            Self::OpenSendResult(_) => f.debug_tuple("OpenSendResult").finish_non_exhaustive(),
        }
    }
}

pub const FEATURES: u64 = features::FILE_TRANSFER | features::FILE_BROWSE;

pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let Ok(runtime) = tokio::runtime::Handle::try_current() else {
        return;
    };
    let files = mgr.services().config.get().files.clone();
    let dirs = mgr.features().files.dirs();
    let dest_dir = destination(&dirs, &files.dest_dir);
    runtime.spawn_blocking(move || {
        let age = Duration::from_secs(u64::from(files.partial_max_age_days) * 86_400);
        if let Ok(removed) = file_recv::cleanup_stale(&dest_dir, age, SystemTime::now()) {
            debug!(removed, "stale file partials cleaned");
        }
    });
}

pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    cx.st.files.last_activity_us = cx.now_us();
    lock(&cx.mgr.features().files.send_counts).insert(cx.key, cx.st.files.active_sends.clone());
}

pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}

pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {
        Event::Send(env) => cx.send(*env),
        Event::TransferEnded => cx.st.files.last_activity_us = cx.now_us(),
        Event::OpenSendResult(tx) => {
            let conn = cx.conn.clone();
            cx.spawn(async move {
                let _ = tx.send(conn.open_uni(StreamKind::File).await);
            });
        }
        Event::OpenSend(tx) => {
            let conn = cx.conn.clone();
            cx.spawn(async move {
                let stream = match conn.open_uni(StreamKind::File).await {
                    Ok(stream) => Some(stream),
                    Err(error) => {
                        debug!(%error, "file send stream open failed");
                        None
                    }
                };
                let _ = tx.send(stream);
            });
        }
    }
}

pub fn deadline<P: Platform>(cx: &ConnCx<'_, P>) -> Option<u64> {
    if cx.info.intent != pb::Intent::Files
        || cx.st.files.active_receives.load(Ordering::Acquire) > 0
        || cx.st.files.active_sends.load(Ordering::Acquire) > 0
        || lock_active(&cx.mgr.features().files.active)
            .values()
            .any(|a| a.conn == cx.key)
    {
        return None;
    }
    Some(cx.st.files.last_activity_us.saturating_add(FILES_IDLE_US))
}

pub fn on_tick<P: Platform>(cx: &mut ConnCx<'_, P>, _now_us: u64) {
    if deadline(cx).is_some_and(|at| cx.now_us() >= at) {
        cx.end(EndReason::Idle);
    }
}

pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    lock(&cx.mgr.features().files.send_counts).remove(&cx.key);
    for active in lock_active(&cx.mgr.features().files.active).values() {
        if active.conn == cx.key {
            active.stop.notify_one();
        }
    }
}

pub fn on_stream<P: Platform>(cx: &mut ConnCx<'_, P>, kind: u8, mut recv: UniRecv) {
    cx.st.files.last_activity_us = cx.now_us();
    let files = cx.mgr.services().config.get().files.clone();
    if !files.enabled || !files.from_phone {
        reject_stream(cx, recv, file_ctl::STREAM_DISABLED, pb::FileError::Disabled);
        return;
    }
    if !may_transfer(cx.info) {
        debug!(kind, "file stream permission denied");
        reject_stream(
            cx,
            recv,
            file_ctl::STREAM_NOT_ALLOWED,
            pb::FileError::NotAllowed,
        );
        return;
    }
    let Some(slot) = CountSlot::acquire(
        &cx.st.files.active_receives,
        file_ctl::MAX_CONCURRENT_RECEIVES,
    ) else {
        reject_stream(cx, recv, file_ctl::STREAM_BUSY, pb::FileError::Busy);
        return;
    };

    let dirs = cx.mgr.features().files.dirs();
    let dest_dir = destination(&dirs, &files.dest_dir);
    let info = cx.info.clone();
    let started_us = cx.now_us();
    let mgr = cx.mgr.clone();
    let key = cx.key;
    let active = mgr.features().files.active.clone();
    let received = Arc::new(AtomicU64::new(0));
    let (done_tx, done_rx) = watch::channel(false);
    // Construct before spawning so aborting an unpolled task also releases it.
    let mut guard = TransferGuard {
        mgr: mgr.clone(),
        key,
        active: active.clone(),
        id: None,
        received: received.clone(),
        done: done_tx,
        _slot: slot,
    };
    cx.spawn(async move {
        if tokio::fs::create_dir_all(&dest_dir).await.is_err() {
            warn!("file receive directory unavailable");
            let _ = recv.stop(file_ctl::STREAM_REJECTED);
            return;
        }
        let mut cfg = ReceiveConfig {
            dest_dir,
            ..ReceiveConfig::default()
        };
        if files.max_file_mb > 0 {
            cfg.max_file_bytes = files.max_file_mb.saturating_mul(1 << 20);
        }
        let cancel = CancelFlag::new();
        let stop = Arc::new(Notify::new());
        let mut source = Stoppable {
            inner: recv,
            stop: stop.clone(),
            cancel: cancel.clone(),
            header: Some(FileStreamReceiver::new()),
            transfer_id: String::new(),
            received: received.clone(),
            next_received: None,
        };
        let mut accept = |header: &pb::FileHeader| {
            let destination = if header.dest_dir.is_empty() {
                None
            } else {
                if !files.browse {
                    return Err(RecvError::Rejected(pb::FileError::Disabled));
                }
                let policy =
                    browse_policy(mgr.services(), &files, &dirs).map_err(RecvError::Rejected)?;
                let resolved = file_browse::resolve_dest_dir(&policy, &header.dest_dir)
                    .map_err(|error| RecvError::Rejected(error.to_pb()))?;
                if !mgr.features().files.user_can_write(&resolved) {
                    debug!("file destination rejected by user directory resolver");
                    return Err(RecvError::Rejected(pb::FileError::PermissionDenied));
                }
                Some(resolved)
            };
            let mut transfers = lock_active(&active);
            if let Some(previous) = transfers.get(&header.transfer_id) {
                if previous.conn != key || previous.direction == Direction::Send {
                    previous.stop.notify_one();
                    return Err(RecvError::Rejected(pb::FileError::Busy));
                }
            }
            received.store(header.offset, Ordering::Release);
            transfers.insert(
                header.transfer_id.clone(),
                Active {
                    conn: key,
                    device_id: info.device_id.clone(),
                    direction: Direction::Receive,
                    cancel: cancel.clone(),
                    stop: stop.clone(),
                    received: received.clone(),
                    done: done_rx.clone(),
                },
            );
            lock(&mgr.features().files.completed).forget(&header.transfer_id, &info.device_id);
            mgr.features().files.persist_completed_sync();
            guard.id = Some(header.transfer_id.clone());
            Ok(destination)
        };
        let mut progress = |event: TransferProgress| {
            received.store(event.done, Ordering::Release);
        };
        let outcome = file_recv::receive_with(
            &mut source,
            &cfg,
            &mut accept,
            &mut progress,
            &cancel,
            &|| mgr.now_us(),
        )
        .await;
        let mut status = pb::FileStatus {
            transfer_id: source.transfer_id.clone(),
            ..Default::default()
        };
        match outcome {
            Ok(ReceiveOutcome::Completed {
                transfer_id,
                size,
                path,
            }) => {
                info!(%transfer_id, size, "file receive completed");
                status.transfer_id = transfer_id;
                status.state = pb::file_status::State::Completed as i32;
                status.received = size;
                status.saved_name = path
                    .file_name()
                    .map(|n| n.to_string_lossy().into_owned())
                    .unwrap_or_default();
                lock(&mgr.features().files.completed).record(files_done::CompletedUpload {
                    transfer_id: status.transfer_id.clone(),
                    device_id: info.device_id.clone(),
                    size,
                    saved_name: status.saved_name.clone(),
                    at_us: mgr.now_us(),
                });
                mgr.features().files.persist_completed().await;
                mgr.platform().on_file_received(&path, &info.device_name);
                audit_transfer(
                    mgr.services(),
                    &info,
                    "upload",
                    size,
                    started_us,
                    mgr.now_us(),
                );
            }
            Ok(ReceiveOutcome::Cancelled { transfer_id }) => {
                status.transfer_id = transfer_id;
                status.state = pb::file_status::State::Cancelled as i32;
                let _ = source.inner.stop(file_ctl::STREAM_CANCELLED);
            }
            Ok(ReceiveOutcome::Interrupted {
                transfer_id,
                received,
            }) => {
                info!(%transfer_id, received, "file receive interrupted");
                return;
            }
            Err(error) => {
                let code = recv_error(&error);
                warn!(transfer_id = %status.transfer_id, error = ?code, "file receive failed");
                status.state = pb::file_status::State::Failed as i32;
                status.error = code as i32;
                if let RecvError::ResumeMismatch { have } = error {
                    status.received = have;
                }
                let _ = source.inner.stop(file_ctl::stream_code(code));
            }
        }
        drop(guard);
        post_status(&mgr, key, status);
    });
}

fn reject_stream<P: Platform>(
    cx: &mut ConnCx<'_, P>,
    mut recv: UniRecv,
    code: u32,
    error: pb::FileError,
) {
    let Some(slot) = CountSlot::acquire(&cx.st.files.pending_rejects, MAX_PENDING_REJECTS) else {
        let _ = recv.stop(code);
        return;
    };
    let mgr = cx.mgr.clone();
    let key = cx.key;
    cx.spawn(async move {
        let _slot = slot;
        let header = tokio::time::timeout(REJECT_HEADER_WAIT, peek_header(&mut recv))
            .await
            .ok()
            .flatten();
        if let Some(header) = header {
            post_status(
                &mgr,
                key,
                pb::FileStatus {
                    transfer_id: header.transfer_id,
                    state: pb::file_status::State::Failed as i32,
                    error: error as i32,
                    ..Default::default()
                },
            );
        }
        let _ = recv.stop(code);
    });
}

async fn peek_header(recv: &mut UniRecv) -> Option<pb::FileHeader> {
    let mut parser = FileStreamReceiver::new();
    let mut buf = [0; 1024];
    let mut read = 0usize;
    loop {
        match recv.read(&mut buf).await {
            Ok(Some(0)) => {}
            Ok(Some(n)) => {
                read = read.saturating_add(n);
                if read > 4 + MAX_HEADER_BYTES {
                    return None;
                }
                let fed = parser.feed(&buf[..n]).ok()?;
                if let Some(header) = fed.header {
                    return Some(header);
                }
            }
            Ok(None) | Err(_) => return None,
        }
    }
}

fn may_transfer(info: &ConnInfo) -> bool {
    info.role == auth::Role::Controller || (info.intent == pb::Intent::Files && info.can_control)
}

fn destination(dirs: &UserDirs, configured: &str) -> PathBuf {
    // A transfer may outlive a console-user change. Query/cancel recomputes
    // this path, so resume can start over and cancellation can miss a `.part`
    // in the old directory; on-host-start cleanup removes such remnants from
    // the directory resolved at that next startup. We deliberately do not
    // record a directory per transfer.
    if configured.is_empty() {
        dirs.downloads.clone()
    } else {
        dirs.home.as_deref().map_or_else(
            || PathBuf::from(configured),
            |home| file_browse::expand_home(configured, home),
        )
    }
}

fn browse_policy(
    services: &HostServices,
    files: &crate::config::FilesConfig,
    dirs: &UserDirs,
) -> Result<BrowsePolicy, pb::FileError> {
    let home = dirs.home.as_deref().ok_or_else(|| {
        warn!("file browsing unavailable: HOME is missing");
        pb::FileError::PermissionDenied
    })?;
    let deny: Vec<_> = services.state_dir.iter().cloned().collect();
    Ok(BrowsePolicy::new(
        &files.browse_roots,
        Path::new(&home),
        &deny,
    ))
}

fn recv_error(error: &RecvError) -> pb::FileError {
    match error {
        RecvError::Rejected(error) => *error,
        RecvError::Sha256Mismatch => pb::FileError::Sha256Mismatch,
        RecvError::ResumeMismatch { .. } => pb::FileError::ResumeMismatch,
        RecvError::FileTooLarge { .. } => pb::FileError::TooLarge,
        RecvError::Io(error) => file_ctl::io_to_pb(error),
        _ => pb::FileError::Protocol,
    }
}

fn post_status<P: Platform>(mgr: &SessionManager<P>, key: ConnKey, status: pb::FileStatus) {
    let _ = mgr.post(
        key,
        super::FeatureEvent::Files(Event::Send(Box::new(status.into()))),
    );
}

struct TransferGuard<P: Platform> {
    mgr: Arc<SessionManager<P>>,
    key: ConnKey,
    active: ActiveTransfers,
    id: Option<String>,
    received: Arc<AtomicU64>,
    done: watch::Sender<bool>,
    _slot: CountSlot,
}

impl<P: Platform> Drop for TransferGuard<P> {
    fn drop(&mut self) {
        let mut active = lock_active(&self.active);
        if let Some(id) = &self.id {
            if active
                .get(id)
                .is_some_and(|a| Arc::ptr_eq(&a.received, &self.received))
            {
                active.remove(id);
            }
        }
        self.done.send_replace(true);
        drop(active);
        let _ = self
            .mgr
            .post(self.key, super::FeatureEvent::Files(Event::TransferEnded));
    }
}

struct Stoppable<S> {
    inner: S,
    stop: Arc<Notify>,
    cancel: CancelFlag,
    // Observe the header even when the engine rejects size before accept().
    header: Option<FileStreamReceiver>,
    transfer_id: String,
    received: Arc<AtomicU64>,
    next_received: Option<u64>,
}

impl<S: FileSource> FileSource for Stoppable<S> {
    async fn read(&mut self, buf: &mut [u8]) -> Result<Option<usize>, ConnError> {
        // A subsequent read means the engine finished writing the prior span.
        // Progress events are throttled, so also publish this committed offset.
        if let Some(received) = self.next_received {
            self.received.store(received, Ordering::Release);
        }
        let read = tokio::select! {
            biased;
            _ = self.stop.notified() => {
                // Wake the engine's cancellation check without treating cancel as interruption.
                if self.cancel.is_cancelled() { return Ok(Some(0)); }
                return Err(ConnError::Closed);
            }
            read = self.inner.read(buf) => read,
        };
        if let Ok(Some(count)) = read {
            if let Some(parser) = &mut self.header {
                match parser.feed(&buf[..count]) {
                    Ok(fed) => {
                        if let Some(header) = fed.header {
                            self.transfer_id = header.transfer_id;
                            self.next_received = Some(header.offset);
                            self.header = None;
                        }
                        if let Some(span) = fed.data {
                            self.next_received = Some(span.offset + span.bytes.len() as u64);
                        }
                    }
                    Err(_) => self.header = None,
                }
            } else if let Some(received) = &mut self.next_received {
                *received = received.saturating_add(count as u64);
            }
        }
        read
    }
}

impl FileSink for Stoppable<UniSend> {
    async fn write(&mut self, bytes: &[u8]) -> Result<(), ConnError> {
        tokio::select! {
            biased;
            _ = self.stop.notified() => {
                // A successful wake lets send_with observe the cancellation flag.
                if self.cancel.is_cancelled() { Ok(()) } else { Err(ConnError::Closed) }
            }
            result = self.inner.write_raw(bytes) => result,
        }
    }

    async fn finish(&mut self) -> Result<(), ConnError> {
        tokio::select! {
            biased;
            _ = self.stop.notified() => Err(ConnError::Closed),
            result = self.inner.finish() => result,
        }
    }
}

/// FIFO snapshots keyed by canonical path, size, and Unix-microsecond mtime.
/// Looking up an entry deliberately does not change insertion order.
#[derive(Default)]
struct PreparedCache(VecDeque<PreparedFile>);

impl PreparedCache {
    fn get(&self, path: &Path, size: u64, mtime_us: u64, id: &str) -> Option<PreparedFile> {
        self.0
            .iter()
            .find(|file| file.path == path && file.size == size && file.mtime_us == mtime_us)
            .map(|file| {
                let mut file = file.clone();
                file.transfer_id = id.to_owned();
                file
            })
    }

    fn insert(&mut self, file: PreparedFile) {
        self.remove(&file.path);
        if self.0.len() == 16 {
            self.0.pop_front();
        }
        self.0.push_back(file);
    }

    fn remove(&mut self, path: &Path) {
        self.0.retain(|file| file.path != path);
    }
}

fn mtime_us(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|time| time.duration_since(UNIX_EPOCH).ok())
        .and_then(|time| u64::try_from(time.as_micros()).ok())
        .unwrap_or(0)
}

async fn prepare_cached<P: Platform>(
    mgr: &SessionManager<P>,
    path: &Path,
    id: String,
) -> Result<PreparedFile, SendError> {
    let metadata = tokio::fs::metadata(path).await?;
    let cached =
        lock(&mgr.features().files.prepared).get(path, metadata.len(), mtime_us(&metadata), &id);
    if let Some(file) = cached {
        return Ok(file);
    }
    let prepared = file_send::prepare(path, Some(id)).await?;
    lock(&mgr.features().files.prepared).insert(prepared.clone());
    Ok(prepared)
}

struct SendTransfer<P: Platform> {
    _guard: TransferGuard<P>,
    cancel: CancelFlag,
    stop: Arc<Notify>,
}

async fn register_send<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    info: &ConnInfo,
    id: &str,
    slot: CountSlot,
) -> Result<SendTransfer<P>, pb::FileError> {
    let cancel = CancelFlag::new();
    let stop = Arc::new(Notify::new());
    let received = Arc::new(AtomicU64::new(0));
    let (done_tx, done_rx) = watch::channel(false);
    let mut guard = TransferGuard {
        mgr: mgr.clone(),
        key: info.key,
        active: mgr.features().files.active.clone(),
        id: None,
        received: received.clone(),
        done: done_tx,
        _slot: slot,
    };
    // Recheck under the registry lock after waiting: another request may have
    // claimed the id in the meantime. The total replacement wait is bounded.
    tokio::time::timeout(Duration::from_secs(2), async {
        loop {
            let previous = {
                let mut active = lock_active(&guard.active);
                if let Some(previous) = active.get(id) {
                    if previous.conn != info.key {
                        previous.stop.notify_one();
                    }
                    Some(previous.done.clone())
                } else {
                    active.insert(
                        id.to_owned(),
                        Active {
                            conn: info.key,
                            device_id: info.device_id.clone(),
                            direction: Direction::Send,
                            cancel: cancel.clone(),
                            stop: stop.clone(),
                            received: received.clone(),
                            done: done_rx.clone(),
                        },
                    );
                    guard.id = Some(id.to_owned());
                    None
                }
            };
            let Some(mut done) = previous else {
                return;
            };
            let _ = done.wait_for(|done| *done).await;
        }
    })
    .await
    .map_err(|_| pb::FileError::Busy)?;
    Ok(SendTransfer {
        _guard: guard,
        cancel,
        stop,
    })
}

async fn transmit<P: Platform>(
    mgr: &SessionManager<P>,
    key: ConnKey,
    prepared: &PreparedFile,
    offset: u64,
    transfer: &SendTransfer<P>,
) -> Result<SendOutcome, SendError> {
    let (tx, rx) = oneshot::channel();
    if !mgr.post(key, super::FeatureEvent::Files(Event::OpenSendResult(tx))) {
        return Err(not_connected());
    }
    let stream = match rx.await {
        Ok(Ok(stream)) => stream,
        Ok(Err(rsp_transport::ConnError::Unsupported(message))) => {
            return Err(SendError::Io(io::Error::new(
                io::ErrorKind::Unsupported,
                message,
            )));
        }
        _ => return Err(not_connected()),
    };
    let mut sink = Stoppable {
        inner: stream,
        stop: transfer.stop.clone(),
        cancel: transfer.cancel.clone(),
        header: None,
        transfer_id: prepared.transfer_id.clone(),
        received: transfer._guard.received.clone(),
        next_received: None,
    };
    // Do not capture the sink itself in the progress callback.
    let received = sink.received.clone();
    let mut progress = |event: TransferProgress| {
        received.store(event.done, Ordering::Release);
    };
    let outcome = file_send::send_with(
        prepared,
        offset,
        "",
        &mut sink,
        &mut progress,
        &transfer.cancel,
        &|| mgr.now_us(),
    )
    .await;
    let outcome = if transfer.cancel.is_cancelled() {
        let _ = sink.inner.reset(file_ctl::STREAM_CANCELLED);
        Ok(SendOutcome::Cancelled)
    } else {
        outcome
    };
    if outcome.is_err() {
        let _ = sink.inner.reset(file_ctl::STREAM_REJECTED);
    }
    if matches!(outcome, Err(SendError::SourceChanged)) {
        lock(&mgr.features().files.prepared).remove(&prepared.path);
    }
    outcome
}

/// Send one file on a new host-to-app 0x04 unidirectional stream.
pub async fn send_file<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    key: ConnKey,
    path: &Path,
) -> Result<SendOutcome, SendError> {
    let Some(info) = mgr.info(key) else {
        return Err(not_connected());
    };
    let started_us = mgr.now_us();
    if info.client_features & features::FILE_TRANSFER == 0 {
        return Err(not_connected());
    }
    let count = lock(&mgr.features().files.send_counts)
        .get(&key)
        .cloned()
        .ok_or_else(not_connected)?;
    let slot = CountSlot::acquire(&count, file_ctl::MAX_CONCURRENT_SENDS).ok_or_else(|| {
        SendError::Io(io::Error::new(io::ErrorKind::WouldBlock, "file sends busy"))
    })?;
    let prepared = file_send::prepare(path, None).await?;
    let transfer = register_send(mgr, &info, &prepared.transfer_id, slot)
        .await
        .map_err(|_| SendError::Io(io::Error::new(io::ErrorKind::WouldBlock, "file sends busy")))?;
    let outcome = transmit(mgr, key, &prepared, 0, &transfer).await;
    if let Ok(SendOutcome::Completed { .. }) = &outcome {
        audit_transfer(
            mgr.services(),
            &info,
            "download",
            prepared.size,
            started_us,
            mgr.now_us(),
        );
    }
    if matches!(outcome, Ok(SendOutcome::Cancelled)) {
        post_status(
            mgr,
            key,
            pb::FileStatus {
                transfer_id: prepared.transfer_id.clone(),
                state: pb::file_status::State::Cancelled as i32,
                ..Default::default()
            },
        );
    }
    outcome
}

fn audit_transfer(
    services: &HostServices,
    info: &ConnInfo,
    op: &'static str,
    bytes: u64,
    started_us: u64,
    now_us: u64,
) {
    let secs = i64::try_from(now_us.saturating_sub(started_us) / 1_000_000).unwrap_or(i64::MAX);
    super::roster::push_audit(
        services,
        &super::roster::AuditFields::from_info(info),
        "file",
        now_us,
        Some(secs),
        serde_json::json!({"op": op, "files": 1, "bytes": bytes}),
    );
}

struct CountSlot(Arc<AtomicUsize>);

impl CountSlot {
    fn acquire(active: &Arc<AtomicUsize>, limit: usize) -> Option<Self> {
        active
            .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
                (count < limit).then_some(count + 1)
            })
            .ok()
            .map(|_| Self(active.clone()))
    }
}

impl Drop for CountSlot {
    fn drop(&mut self) {
        self.0.fetch_sub(1, Ordering::AcqRel);
    }
}

fn not_connected() -> SendError {
    SendError::Io(io::Error::new(
        io::ErrorKind::NotConnected,
        "file transfer connection is unavailable",
    ))
}

pub fn on_file_list<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::FileList) {
    cx.st.files.last_activity_us = cx.now_us();
    let files = cx.mgr.services().config.get().files.clone();
    let mut listing = pb::FileListing {
        request_id: m.request_id,
        path: m.path.clone(),
        ..Default::default()
    };
    let error = if file_ctl::validate_list(&m).is_err() {
        Some(pb::FileError::Protocol)
    } else if !files.enabled || !files.browse {
        Some(pb::FileError::Disabled)
    } else if !may_transfer(cx.info) {
        Some(pb::FileError::NotAllowed)
    } else {
        None
    };
    if let Some(error) = error {
        listing.error = error as i32;
        cx.send(listing);
        return;
    }
    let Some(slot) = CountSlot::acquire(&cx.st.files.active_lists, 5) else {
        listing.error = pb::FileError::Busy as i32;
        cx.send(listing);
        return;
    };
    let serial = cx.st.files.list_serial.clone();
    let mgr = cx.mgr.clone();
    let key = cx.key;
    let dirs = mgr.features().files.dirs();
    cx.spawn(async move {
        let worker_mgr = mgr.clone();
        let result = tokio::time::timeout(Duration::from_secs(10), async move {
            let permit = serial.lock_owned().await;
            tokio::task::spawn_blocking(move || {
                // Keep both guards in the blocking task if timeout fires while
                // macOS is waiting for TCC: no second directory read can start.
                let (_permit, _slot) = (permit, slot);
                let policy = browse_policy(worker_mgr.services(), &files, &dirs)?;
                let dir = file_browse::resolve_dir(&policy, &m.path).map_err(|e| e.to_pb())?;
                let entries = file_browse::list_dir(&policy, &dir.path, &ListOptions::default())
                    .map_err(|e| e.to_pb())?;
                let (entries, next_offset) = file_ctl::page(&entries, m.offset);
                Ok::<_, pb::FileError>((dir, entries, next_offset))
            })
            .await
            .map_err(|_| pb::FileError::Io)?
        })
        .await;
        match result {
            Ok(Ok((dir, entries, next_offset))) => {
                listing.path = dir.path.to_string_lossy().into_owned();
                listing.parent = dir
                    .parent
                    .map(|path| path.to_string_lossy().into_owned())
                    .unwrap_or_default();
                listing.entries = entries;
                listing.next_offset = next_offset;
            }
            result => {
                listing.error = match result {
                    Ok(Err(error)) => error,
                    _ => pb::FileError::Io,
                } as i32
            }
        }
        debug!(
            request_id = listing.request_id,
            entries = listing.entries.len(),
            error = listing.error,
            "file listing result"
        );
        let _ = mgr.post(
            key,
            super::FeatureEvent::Files(Event::Send(Box::new(listing.into()))),
        );
    });
}

pub fn on_file_request<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::FileRequest) {
    let started_us = cx.now_us();
    cx.st.files.last_activity_us = started_us;
    let files = cx.mgr.services().config.get().files.clone();
    let mut status = pb::FileStatus {
        request_id: m.request_id,
        transfer_id: m.transfer_id.clone(),
        state: pb::file_status::State::Failed as i32,
        ..Default::default()
    };
    let error = if file_ctl::validate_request(&m).is_err() {
        Some(pb::FileError::Protocol)
    } else if !files.enabled || !files.to_phone {
        Some(pb::FileError::Disabled)
    } else if !may_transfer(cx.info) {
        Some(pb::FileError::NotAllowed)
    } else {
        None
    };
    if let Some(error) = error {
        status.error = error as i32;
        cx.send(status);
        return;
    }
    let Some(slot) = CountSlot::acquire(&cx.st.files.active_sends, file_ctl::MAX_CONCURRENT_SENDS)
    else {
        status.error = pb::FileError::Busy as i32;
        cx.send(status);
        return;
    };
    let mgr = cx.mgr.clone();
    let info = cx.info.clone();
    let key = cx.key;
    let dirs = mgr.features().files.dirs();
    cx.spawn(async move {
        let worker_mgr = mgr.clone();
        let path = m.path.clone();
        let resolved = tokio::task::spawn_blocking(move || {
            let policy = browse_policy(worker_mgr.services(), &files, &dirs)?;
            file_browse::resolve_file(&policy, &path).map_err(|error| error.to_pb())
        })
        .await;
        let result: Result<bool, pb::FileError> = async {
            let path = resolved.map_err(|_| pb::FileError::Io)??;
            let id = if m.transfer_id.is_empty() {
                rsp_core::file::new_transfer_id().map_err(|_| pb::FileError::Protocol)?
            } else {
                m.transfer_id.clone()
            };
            let prepared = prepare_cached(&mgr, &path, id).await.map_err(|error| {
                if matches!(error, SendError::SourceChanged) {
                    lock(&mgr.features().files.prepared).remove(&path);
                }
                send_error(&error)
            })?;
            status.transfer_id = prepared.transfer_id.clone();
            let offset = if m.offset > 0
                && m.expect_size == prepared.size
                && m.expect_mtime_us == prepared.mtime_us
                && m.expect_sha256 == prepared.sha256
            {
                m.offset
            } else {
                0
            };
            let transfer = register_send(&mgr, &info, &prepared.transfer_id, slot).await?;
            let outcome = transmit(&mgr, key, &prepared, offset, &transfer)
                .await
                .map_err(|error| send_error(&error))?;
            match outcome {
                SendOutcome::Completed { .. } => {
                    info!(request_id = m.request_id, transfer_id = %prepared.transfer_id,
                        size = prepared.size, "file download completed");
                    audit_transfer(
                        mgr.services(),
                        &info,
                        "download",
                        prepared.size,
                        started_us,
                        mgr.now_us(),
                    );
                    Ok(false)
                }
                SendOutcome::Cancelled => {
                    status.state = pb::file_status::State::Cancelled as i32;
                    Ok(true)
                }
                SendOutcome::Interrupted { .. } => Ok(false),
            }
        }
        .await;
        match result {
            Ok(false) => return,
            Ok(true) => {}
            Err(error) => {
                status.error = error as i32;
                warn!(request_id = status.request_id, transfer_id = %status.transfer_id,
                    ?error, "file download failed");
            }
        }
        post_status(&mgr, key, status);
    });
}

fn send_error(error: &SendError) -> pb::FileError {
    match error {
        SendError::SourceChanged => pb::FileError::SourceChanged,
        SendError::Io(error) => file_ctl::io_to_pb(error),
        _ => pb::FileError::Protocol,
    }
}

pub fn on_file_query<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::FileQuery) {
    cx.st.files.last_activity_us = cx.now_us();
    let files = cx.mgr.services().config.get().files.clone();
    let mut status = pb::FileStatus {
        request_id: m.request_id,
        transfer_id: m.transfer_id,
        ..Default::default()
    };
    if !files.enabled || !files.from_phone {
        status.state = pb::file_status::State::Failed as i32;
        status.error = pb::FileError::Disabled as i32;
        cx.send(status);
        return;
    }
    if !may_transfer(cx.info) {
        status.state = pb::file_status::State::Failed as i32;
        status.error = pb::FileError::NotAllowed as i32;
        cx.send(status);
        return;
    }
    let mgr = cx.mgr.clone();
    let key = cx.key;
    let device_id = cx.info.device_id.clone();
    let dirs = mgr.features().files.dirs();
    let dest_dir = destination(&dirs, &files.dest_dir);
    cx.spawn(async move {
        if file_ctl::validate_query(&pb::FileQuery {
            request_id: status.request_id,
            transfer_id: status.transfer_id.clone(),
        })
        .is_err()
        {
            status.state = pb::file_status::State::Failed as i32;
            status.error = pb::FileError::Protocol as i32;
            post_status(&mgr, key, status);
            return;
        }
        let previous = {
            let active = lock_active(&mgr.features().files.active);
            active.get(&status.transfer_id).map(|a| {
                if a.conn == key && a.direction == Direction::Receive {
                    status.state = pb::file_status::State::Receiving as i32;
                    status.received = a.received.load(Ordering::Acquire);
                } else {
                    a.stop.notify_one();
                }
                a.done.clone()
            })
        };
        if status.state == pb::file_status::State::Receiving as i32 {
            post_status(&mgr, key, status);
            return;
        }
        if let Some(mut done) = previous {
            if tokio::time::timeout(Duration::from_secs(2), done.wait_for(|done| *done))
                .await
                .is_err()
            {
                status.state = pb::file_status::State::Failed as i32;
                status.error = pb::FileError::Busy as i32;
                post_status(&mgr, key, status);
                return;
            }
        }
        let done = lock(&mgr.features().files.completed).lookup(
            &status.transfer_id,
            &device_id,
            mgr.now_us(),
        );
        if let Some(done) = done {
            status.state = pb::file_status::State::Completed as i32;
            status.received = done.size;
            status.saved_name = done.saved_name;
            debug!(transfer_id = %status.transfer_id, "file query: already completed");
            post_status(&mgr, key, status);
            return;
        }
        let id = status.transfer_id.clone();
        match tokio::task::spawn_blocking(move || file_recv::partial_received(&dest_dir, &id)).await
        {
            Ok(Ok(Some(received))) => {
                status.state = pb::file_status::State::Partial as i32;
                status.received = received;
            }
            Ok(Ok(None)) => status.state = pb::file_status::State::Unknown as i32,
            result => {
                status.state = pb::file_status::State::Failed as i32;
                status.error = match result {
                    Ok(Err(error)) => recv_error(&error),
                    _ => pb::FileError::Io,
                } as i32;
            }
        }
        post_status(&mgr, key, status);
    });
}
pub fn on_file_status<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::FileStatus) {
    cx.st.files.last_activity_us = cx.now_us();
    if matches!(
        pb::file_status::State::try_from(m.state),
        Ok(pb::file_status::State::Completed | pb::file_status::State::Failed)
    ) {
        debug!(transfer_id = %m.transfer_id, state = m.state, error = m.error, "peer file result");
    }
}
pub fn on_file_cancel<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::FileCancel) {
    cx.st.files.last_activity_us = cx.now_us();
    if !may_transfer(cx.info) {
        cx.send(pb::FileStatus {
            transfer_id: m.transfer_id,
            state: pb::file_status::State::Failed as i32,
            error: pb::FileError::NotAllowed as i32,
            ..Default::default()
        });
        return;
    }
    if file_ctl::validate_cancel(&m).is_err() {
        debug!("invalid file cancellation ignored");
        return;
    }
    {
        let active = lock_active(&cx.mgr.features().files.active);
        if let Some(active) = active.get(&m.transfer_id) {
            if active.device_id == cx.info.device_id {
                active.cancel.cancel();
                active.stop.notify_one();
            }
            return;
        }
    }
    let mgr = cx.mgr.clone();
    let key = cx.key;
    let files = mgr.services().config.get().files.clone();
    let dirs = mgr.features().files.dirs();
    let dest_dir = destination(&dirs, &files.dest_dir);
    cx.spawn(async move {
        let id = m.transfer_id.clone();
        let result =
            tokio::task::spawn_blocking(move || file_recv::discard_partial(&dest_dir, &id)).await;
        let mut status = pb::FileStatus {
            transfer_id: m.transfer_id,
            state: pb::file_status::State::Cancelled as i32,
            ..Default::default()
        };
        match result {
            Ok(Ok(_)) => {}
            result => {
                status.state = pb::file_status::State::Failed as i32;
                status.error = match result {
                    Ok(Err(error)) => recv_error(&error),
                    _ => pb::FileError::Io,
                } as i32;
            }
        }
        post_status(&mgr, key, status);
    });
}

#[cfg(test)]
mod tests {
    use super::*;

    fn prepared(index: u64) -> PreparedFile {
        PreparedFile {
            path: PathBuf::from(format!("/example/{index}")),
            transfer_id: format!("{index:032x}"),
            name: index.to_string(),
            size: index,
            sha256: [index as u8; 32],
            mtime_us: index + 100,
        }
    }

    #[test]
    fn prepared_cache_checks_metadata_and_copies_transfer_id() {
        let mut cache = PreparedCache::default();
        let file = prepared(1);
        cache.insert(file.clone());
        let hit = cache
            .get(&file.path, file.size, file.mtime_us, "replacement")
            .unwrap();
        assert_eq!(hit.transfer_id, "replacement");
        assert_eq!(hit.sha256, file.sha256);
        assert_eq!(cache.0[0].transfer_id, file.transfer_id);
        assert!(cache
            .get(&file.path, file.size + 1, file.mtime_us, "x")
            .is_none());
        assert!(cache
            .get(&file.path, file.size, file.mtime_us + 1, "x")
            .is_none());
        cache.remove(&file.path);
        assert!(cache.0.is_empty());
    }

    #[test]
    fn prepared_cache_evicts_in_insertion_order_and_replaces_stale_paths() {
        let mut cache = PreparedCache::default();
        for index in 0..16 {
            cache.insert(prepared(index));
        }
        let oldest = prepared(0);
        assert!(cache
            .get(&oldest.path, oldest.size, oldest.mtime_us, "x")
            .is_some());
        cache.insert(prepared(16));
        assert_eq!(cache.0.len(), 16);
        assert!(cache
            .get(&oldest.path, oldest.size, oldest.mtime_us, "x")
            .is_none());
        let mut changed = prepared(1);
        changed.mtime_us += 1;
        cache.insert(changed.clone());
        assert_eq!(cache.0.len(), 16);
        assert_eq!(cache.0.back(), Some(&changed));
        assert!(cache
            .get(&changed.path, changed.size, changed.mtime_us - 1, "x")
            .is_none());
    }
}
