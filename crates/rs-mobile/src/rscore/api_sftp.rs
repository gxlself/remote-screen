//! Server file browsing and resumable transfers over a pooled SSH connection.

use std::collections::HashMap;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use ring::rand::{SecureRandom, SystemRandom};
use rs_ssh::sftp::{EntryKind, Sftp, SftpError};
use rs_ssh::sftp_transfer::{self, Cancel, Progress, TransferError, TransferOpts, TransferOutcome};
use rs_ssh::{SshError, SshSession};
use rsp_core::file_name::{candidate, sanitize, MAX_CANDIDATES};
use tokio::sync::Mutex as AsyncMutex;
use tokio::task::JoinHandle;
use tokio::time::Instant;

use super::api_ssh::{connect_once, resolve_server, CorePrompter, PromptRelay};
use super::dto_files::{
    FileEntry, FileEntryKind, FileListing, FileSourceRef, FileTransfer, FileTransferDirection,
    FileTransferFail, FileTransferState,
};
use super::error::CoreError;
use super::runtime::{core, Core};
use super::xfer::{SourceKind, TransferDriver, Transfers};

const IDLE: Duration = Duration::from_secs(120);
const RETRY_SECONDS: [u64; 3] = [2, 5, 10];

fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|error| error.into_inner())
}

#[async_trait]
pub trait SftpConnector: Send + Sync {
    async fn connect(&self, core: &Arc<Core>, server_id: &str) -> Result<SshSession, CoreError>;
}

pub struct RealConnector;

#[async_trait]
impl SftpConnector for RealConnector {
    async fn connect(&self, core: &Arc<Core>, server_id: &str) -> Result<SshSession, CoreError> {
        let resolved = resolve_server(core, server_id, None)
            .await
            .map_err(|_| http(404, "sftp.serverNotFound"))?;
        let prompter = Arc::new(CorePrompter::new(
            core.clone(),
            None,
            server_id.to_owned(),
            resolved.target.user.clone(),
            None,
            Arc::new(PromptRelay::default()),
        ));
        connect_once(core, &resolved, prompter)
            .await
            .map_err(|error| map_ssh_error(&error))
    }
}

fn http(status: u16, code: &str) -> CoreError {
    CoreError::Http {
        status,
        hub_code: None,
        message: code.to_owned(),
    }
}

/// Stable file-page errors, independent of SSH's diagnostic text.
pub fn map_ssh_error(error: &SshError) -> CoreError {
    match error {
        SshError::HostKeyRejected => http(401, "sftp.hostKeyUnknown"),
        SshError::HostKeyChanged { .. } => http(401, "sftp.hostKeyChanged"),
        SshError::AuthFailed { .. } => http(401, "sftp.authRequired"),
        SshError::Timeout | SshError::Transport(_) | SshError::Closed => {
            CoreError::network(format!("sftp.offline: {error}"))
        }
        _ => CoreError::internal(format!("sftp.internal: {error}")),
    }
}

fn map_sftp_error(error: &SftpError) -> CoreError {
    match error {
        SftpError::Unavailable => http(501, "sftp.unavailable"),
        SftpError::NotFound => http(404, "sftp.notFound"),
        SftpError::PermissionDenied => http(403, "sftp.permissionDenied"),
        SftpError::Ssh(error) => map_ssh_error(error),
        _ => CoreError::internal(format!("sftp.internal: {error}")),
    }
}

fn disconnected(error: &SftpError) -> bool {
    matches!(
        error,
        SftpError::Ssh(SshError::Closed | SshError::Transport(_) | SshError::Timeout)
    )
}

struct Connection {
    session: SshSession,
    sftp: Arc<Sftp>,
    active: AtomicUsize,
    last_used: Mutex<Instant>,
}

type ConnectionSlot = Arc<AsyncMutex<Option<Arc<Connection>>>>;

#[derive(Default)]
pub struct SftpConns {
    connector: Mutex<Option<Arc<dyn SftpConnector>>>,
    servers: Mutex<HashMap<String, ConnectionSlot>>,
    registered: Mutex<bool>,
}

/// A lease protects both directory requests and transfers from idle eviction.
struct Lease {
    connection: Arc<Connection>,
    slot: ConnectionSlot,
}

impl Drop for Lease {
    fn drop(&mut self) {
        *lock(&self.connection.last_used) = Instant::now();
        self.connection.active.fetch_sub(1, Ordering::AcqRel);
    }
}

impl Lease {
    fn sftp(&self) -> &Sftp {
        &self.connection.sftp
    }

    async fn error(&self, error: &SftpError) -> CoreError {
        if disconnected(error) {
            let mut slot = self.slot.lock().await;
            if slot
                .as_ref()
                .is_some_and(|entry| Arc::ptr_eq(entry, &self.connection))
            {
                slot.take();
            }
        }
        map_sftp_error(error)
    }
}

impl SftpConns {
    async fn get(&self, core: &Arc<Core>, server_id: &str) -> Result<Lease, CoreError> {
        let slot = lock(&self.servers)
            .entry(server_id.to_owned())
            .or_default()
            .clone();
        // Hold only this server's lock through the handshake, so other servers
        // can connect concurrently and same-server callers share one session.
        let mut entry = slot.lock().await;
        if entry.is_none() {
            let connector = lock(&self.connector)
                .clone()
                .unwrap_or_else(|| Arc::new(RealConnector));
            let session = connector.connect(core, server_id).await?;
            let sftp = match session.sftp().await {
                Ok(sftp) => sftp,
                Err(error) => {
                    session.close().await;
                    return Err(map_sftp_error(&error));
                }
            };
            *entry = Some(Arc::new(Connection {
                session,
                sftp: Arc::new(sftp),
                active: AtomicUsize::new(0),
                last_used: Mutex::new(Instant::now()),
            }));
        }
        let connection = entry
            .as_ref()
            .expect("successful connection initialization fills the slot")
            .clone();
        connection.active.fetch_add(1, Ordering::AcqRel);
        drop(entry);
        Ok(Lease { connection, slot })
    }

    async fn reap(&self) {
        let slots: Vec<_> = lock(&self.servers).values().cloned().collect();
        for slot in slots {
            let expired = {
                let mut entry = slot.lock().await;
                if entry.as_ref().is_some_and(|connection| {
                    connection.active.load(Ordering::Acquire) == 0
                        && lock(&connection.last_used).elapsed() >= IDLE
                }) {
                    entry.take()
                } else {
                    None
                }
            };
            if let Some(connection) = expired {
                close_connection(connection).await;
            }
        }
    }
}

async fn close_connection(connection: Arc<Connection>) {
    if let Ok(connection) = Arc::try_unwrap(connection) {
        if let Ok(sftp) = Arc::try_unwrap(connection.sftp) {
            sftp.close().await;
        }
        connection.session.close().await;
    }
}

pub fn set_connector(core: &Arc<Core>, connector: Arc<dyn SftpConnector>) {
    *lock(&core.state::<SftpConns>().connector) = Some(connector);
}

struct Running {
    handle: JoinHandle<()>,
    cancel: Cancel,
    stopped: bool,
}

#[derive(Default)]
struct Job {
    gate: AsyncMutex<()>,
    running: Mutex<Option<Running>>,
    cancelled: AtomicBool,
}

#[derive(Default)]
struct Jobs {
    entries: Mutex<HashMap<String, Arc<Job>>>,
    // Reserve destinations before creating records. Pauses keep these paths.
    upload_paths: Mutex<HashMap<String, UploadDestination>>,
    allocation: AsyncMutex<()>,
}

struct UploadDestination {
    path: String,
    number: u32,
}

impl Jobs {
    fn job(&self, id: &str) -> Arc<Job> {
        lock(&self.entries)
            .entry(id.to_owned())
            .or_default()
            .clone()
    }
}

struct ServerDriver;

#[async_trait]
impl TransferDriver for ServerDriver {
    fn pause(&self, core: &Arc<Core>, id: &str) {
        let job = core.state::<Jobs>().job(id);
        if let Some(running) = lock(&job.running).as_mut() {
            running.stopped = true;
            running.handle.abort();
        };
    }

    fn cancel(&self, core: &Arc<Core>, id: &str) {
        let job = core.state::<Jobs>().job(id);
        job.cancelled.store(true, Ordering::Release);
        if let Some(running) = lock(&job.running).as_ref() {
            running.cancel.cancel();
        }
        let transfer = core.state::<Transfers>().get(core, id);
        let core_for_task = core.clone();
        core.spawn(async move {
            // An aborted task must finish dropping its handles before cleanup.
            let _gate = job.gate.lock().await;
            let previous = lock(&job.running).take();
            if let Some(running) = previous {
                let _ = running.handle.await;
            }
            // transfers.remove may delete the row before this task is polled.
            if let Some(transfer) = transfer {
                discard_partials(&core_for_task, &transfer).await;
            }
        });
    }

    async fn resume(&self, core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
        start_transfer(core, id).await
    }
}

/// Install exactly one driver, idle reaper, and shutdown hook per Core.
pub fn register_driver(core: &Arc<Core>) {
    let conns = core.state::<SftpConns>();
    let mut registered = lock(&conns.registered);
    if *registered {
        return;
    }
    *registered = true;
    core.state::<Transfers>()
        .register_driver(SourceKind::Server, Arc::new(ServerDriver));
    let weak = Arc::downgrade(&conns);
    let reaper = core.spawn(async move {
        loop {
            tokio::time::sleep(Duration::from_secs(1)).await;
            let Some(conns) = weak.upgrade() else {
                return;
            };
            conns.reap().await;
        }
    });
    let jobs = core.state::<Jobs>();
    let handle = core.handle();
    let weak = Arc::downgrade(&conns);
    core.add_shutdown_hook(Box::new(move || {
        reaper.abort();
        for job in lock(&jobs.entries).values() {
            if let Some(running) = lock(&job.running).as_ref() {
                running.handle.abort();
            }
        }
        if let Some(conns) = weak.upgrade() {
            let slots: Vec<_> = lock(&conns.servers).drain().map(|(_, slot)| slot).collect();
            handle.spawn(async move {
                for slot in slots {
                    let entry = slot.lock().await.take();
                    if let Some(connection) = entry {
                        // Shutdown also closes active connections.
                        if let Ok(connection) = Arc::try_unwrap(connection) {
                            connection.session.close().await;
                        }
                    }
                }
            });
        }
    }));
}

async fn directory(lease: &Lease, path: Option<&str>) -> Result<String, CoreError> {
    let result = match path {
        Some(path) => lease.sftp().canonicalize(path).await,
        None => lease.sftp().home().await,
    };
    match result {
        Ok(path) => Ok(path),
        Err(error) => Err(lease.error(&error).await),
    }
}

pub async fn list(
    core: &Arc<Core>,
    server_id: &str,
    path: Option<String>,
) -> Result<FileListing, CoreError> {
    register_driver(core);
    let lease = core.state::<SftpConns>().get(core, server_id).await?;
    let path = directory(&lease, path.as_deref()).await?;
    let entries = match lease.sftp().list(&path).await {
        Ok(entries) => entries,
        Err(error) => return Err(lease.error(&error).await),
    };
    let parent = parent_path(&path);
    Ok(FileListing {
        path,
        parent,
        entries: entries
            .into_iter()
            .map(|entry| FileEntry {
                name: entry.name,
                kind: match entry.kind {
                    EntryKind::File => FileEntryKind::File,
                    EntryKind::Dir => FileEntryKind::Dir,
                    EntryKind::Symlink => FileEntryKind::Symlink,
                    EntryKind::Other => FileEntryKind::Other,
                },
                size: if entry.kind == EntryKind::Dir {
                    None
                } else {
                    entry.size
                },
                mtime_us: entry.mtime.map(|time| time.saturating_mul(1_000_000)),
                child_count: None,
            })
            .collect(),
        truncated: false,
        net_path: None,
    })
}

fn parent_path(path: &str) -> Option<String> {
    (path != "/").then(|| {
        path.trim_end_matches('/')
            .rsplit_once('/')
            .map(|(parent, _)| if parent.is_empty() { "/" } else { parent })
            .unwrap_or("/")
            .to_owned()
    })
}

fn transfer_id() -> Result<String, CoreError> {
    let mut bytes = [0; 16];
    SystemRandom::new()
        .fill(&mut bytes)
        .map_err(|_| CoreError::internal("sftp.internal: random transfer id"))?;
    Ok(data_encoding::HEXLOWER.encode(&bytes))
}

fn new_transfer(
    core: &Arc<Core>,
    server_id: &str,
    name: String,
) -> Result<FileTransfer, CoreError> {
    Ok(FileTransfer {
        transfer_id: transfer_id()?,
        direction: FileTransferDirection::Download,
        source: FileSourceRef::Server {
            server_id: server_id.to_owned(),
        },
        name,
        remote_path: None,
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
    })
}

pub async fn download(
    core: &Arc<Core>,
    server_id: &str,
    remote_path: &str,
) -> Result<String, CoreError> {
    register_driver(core);
    let lease = core.state::<SftpConns>().get(core, server_id).await?;
    let metadata = match lease.sftp().stat(remote_path).await {
        Ok(metadata) => metadata,
        Err(error) => return Err(lease.error(&error).await),
    };
    let name = sanitize(remote_path);
    let mut transfer = new_transfer(core, server_id, name.clone())?;
    transfer.remote_path = Some(remote_path.to_owned());
    transfer.size = metadata.size.unwrap_or(0);
    let directory = core
        .config()
        .files_dir
        .map(PathBuf::from)
        .unwrap_or_else(|| core.data_dir().join("files"));
    tokio::fs::create_dir_all(&directory).await?;
    let jobs = core.state::<Jobs>();
    let allocation = jobs.allocation.lock().await;
    let transfers = core.state::<Transfers>();
    let existing = transfers.list(core);
    let mut local_path = None;
    for number in 0..MAX_CANDIDATES {
        let path = directory.join(candidate(&name, number));
        let reserved = existing.iter().any(|entry| {
            entry.local_path.as_deref() == path.to_str()
                && !matches!(
                    entry.state,
                    FileTransferState::Cancelled | FileTransferState::Failed
                )
        });
        match std::fs::symlink_metadata(&path) {
            Err(error) if error.kind() == std::io::ErrorKind::NotFound && !reserved => {
                local_path = Some(path.to_string_lossy().into_owned());
                break;
            }
            Err(error) if error.kind() != std::io::ErrorKind::NotFound => return Err(error.into()),
            _ => {}
        }
    }
    transfer.local_path =
        Some(local_path.ok_or_else(|| CoreError::internal("sftp.internal: no free name"))?);
    let id = transfer.transfer_id.clone();
    transfers.create(core, transfer);
    drop(allocation);
    start_transfer(core, &id).await?;
    Ok(id)
}

fn join_remote(directory: &str, name: &str) -> String {
    format!("{}/{name}", directory.trim_end_matches('/'))
}

async fn reserve_upload(
    core: &Arc<Core>,
    lease: &Lease,
    transfer: &FileTransfer,
) -> Result<String, CoreError> {
    let jobs = core.state::<Jobs>();
    let _allocation = jobs.allocation.lock().await;
    let directory = transfer.remote_path.as_deref().unwrap_or("/");
    let first = lock(&jobs.upload_paths)
        .get(&transfer.transfer_id)
        .map(|destination| destination.number + 1)
        .or_else(|| {
            transfer.saved_name.as_ref().and_then(|saved| {
                (0..MAX_CANDIDATES)
                    .find(|number| candidate(&transfer.name, *number) == *saved)
                    .map(|number| number + 1)
            })
        })
        .unwrap_or(0);
    for number in first..MAX_CANDIDATES {
        let name = candidate(&transfer.name, number);
        let path = join_remote(directory, &name);
        if lock(&jobs.upload_paths).iter().any(|(id, reserved)| {
            id != &transfer.transfer_id
                && reserved.path == path
                && core
                    .state::<Transfers>()
                    .get(core, id)
                    .is_some_and(|entry| {
                        entry.source == transfer.source
                            && !matches!(
                                entry.state,
                                FileTransferState::Failed | FileTransferState::Cancelled
                            )
                    })
        }) {
            continue;
        }
        match lease.sftp().lstat(&path).await {
            Ok(_) => continue,
            Err(SftpError::NotFound) => {
                lock(&jobs.upload_paths).insert(
                    transfer.transfer_id.clone(),
                    UploadDestination {
                        path: path.clone(),
                        number,
                    },
                );
                return Ok(path);
            }
            Err(error) => return Err(lease.error(&error).await),
        }
    }
    Err(CoreError::internal("sftp.internal: no free name"))
}

pub async fn upload(
    core: &Arc<Core>,
    server_id: &str,
    local_path: &str,
    remote_dir: Option<String>,
) -> Result<String, CoreError> {
    register_driver(core);
    let metadata = tokio::fs::metadata(local_path).await.map_err(|error| {
        if error.kind() == std::io::ErrorKind::NotFound {
            http(404, "files.localNotFound")
        } else {
            CoreError::internal("sftp.internal: local file metadata")
        }
    })?;
    let name = Path::new(local_path)
        .file_name()
        .and_then(|name| name.to_str())
        .ok_or_else(|| CoreError::internal("sftp.internal: invalid local name"))?
        .to_owned();
    let lease = core.state::<SftpConns>().get(core, server_id).await?;
    let directory = directory(&lease, remote_dir.as_deref()).await?;
    let mut transfer = new_transfer(core, server_id, name)?;
    transfer.direction = FileTransferDirection::Upload;
    transfer.local_path = Some(local_path.to_owned());
    transfer.remote_path = Some(directory);
    transfer.size = metadata.len();
    let remote = reserve_upload(core, &lease, &transfer).await?;
    transfer.saved_name = remote.rsplit('/').next().map(str::to_owned);
    let id = transfer.transfer_id.clone();
    core.state::<Transfers>().create(core, transfer);
    start_transfer(core, &id).await?;
    Ok(id)
}

fn upload_path(core: &Arc<Core>, transfer: &FileTransfer) -> Option<String> {
    lock(&core.state::<Jobs>().upload_paths)
        .get(&transfer.transfer_id)
        .map(|destination| destination.path.clone())
        .or_else(|| {
            Some(join_remote(
                transfer.remote_path.as_deref()?,
                transfer.saved_name.as_deref()?,
            ))
        })
}

async fn start_transfer(core: &Arc<Core>, id: &str) -> Result<(), CoreError> {
    let job = core.state::<Jobs>().job(id);
    let _gate = job.gate.lock().await;
    let previous = {
        let mut running = lock(&job.running);
        if running
            .as_ref()
            .is_some_and(|running| !running.stopped && !running.handle.is_finished())
        {
            return Ok(());
        }
        running.take()
    };
    if let Some(running) = previous {
        let _ = running.handle.await;
    }
    let transfers = core.state::<Transfers>();
    let transfer = transfers
        .get(core, id)
        .ok_or_else(|| http(404, "transfers.notFound"))?;
    if matches!(
        transfer.state,
        FileTransferState::Cancelled | FileTransferState::Completed
    ) {
        return Ok(());
    }
    if job.cancelled.load(Ordering::Acquire) {
        return Ok(());
    }
    let cancel = Cancel::default();
    let task_cancel = cancel.clone();
    let task_job = job.clone();
    let task_core = core.clone();
    let id = id.to_owned();
    let mut running = lock(&job.running);
    transfers.update(core, &id, |entry| {
        entry.state = FileTransferState::Running;
        entry.fail = None;
    });
    let handle = core.spawn(async move {
        run_transfer(&task_core, &id, &task_job, &task_cancel).await;
    });
    *running = Some(Running {
        handle,
        cancel,
        stopped: false,
    });
    Ok(())
}

fn state(core: &Arc<Core>, id: &str, next: FileTransferState, fail: Option<FileTransferFail>) {
    core.state::<Transfers>().update(core, id, |entry| {
        if matches!(
            entry.state,
            FileTransferState::Running | FileTransferState::Interrupted
        ) {
            entry.state = next;
            entry.fail = fail;
        }
    });
}

fn core_failure(error: &CoreError) -> FileTransferFail {
    match error {
        CoreError::Http { status: 401, .. } => FileTransferFail::AuthRequired,
        CoreError::Http { status: 403, .. } => FileTransferFail::PermissionDenied,
        CoreError::Http { status: 404, .. } => FileTransferFail::NotFound,
        CoreError::Http { status: 501, .. } => FileTransferFail::Unsupported,
        _ => FileTransferFail::Io,
    }
}

fn transfer_failure(error: &TransferError) -> FileTransferFail {
    match error {
        TransferError::Sftp(SftpError::NotFound) => FileTransferFail::NotFound,
        TransferError::Sftp(SftpError::PermissionDenied) => FileTransferFail::PermissionDenied,
        TransferError::Sftp(SftpError::Ssh(
            SshError::AuthFailed { .. }
            | SshError::HostKeyRejected
            | SshError::HostKeyChanged { .. },
        )) => FileTransferFail::AuthRequired,
        TransferError::RemoteChanged | TransferError::LocalChanged => {
            FileTransferFail::SourceChanged
        }
        TransferError::SizeMismatch { .. }
        | TransferError::Sftp(SftpError::Protocol(_))
        | TransferError::NotAFile => FileTransferFail::Protocol,
        TransferError::Io(error) if error.kind() == std::io::ErrorKind::StorageFull => {
            FileTransferFail::NoSpace
        }
        _ => FileTransferFail::Io,
    }
}

async fn retry(core: &Arc<Core>, id: &str, retries: &mut usize) -> bool {
    state(core, id, FileTransferState::Interrupted, None);
    let Some(seconds) = RETRY_SECONDS.get(*retries) else {
        return false;
    };
    *retries += 1;
    tokio::time::sleep(Duration::from_secs(*seconds)).await;
    state(core, id, FileTransferState::Running, None);
    true
}

async fn run_transfer(core: &Arc<Core>, id: &str, job: &Job, cancel: &Cancel) {
    let transfers = core.state::<Transfers>();
    let mut retries = 0;
    let mut source_changes = 0;
    loop {
        if job.cancelled.load(Ordering::Acquire) {
            return;
        }
        let Some(transfer) = transfers.get(core, id) else {
            return;
        };
        if !matches!(
            transfer.state,
            FileTransferState::Running | FileTransferState::Interrupted
        ) {
            return;
        }
        let FileSourceRef::Server { server_id } = &transfer.source else {
            return;
        };
        let lease = match core.state::<SftpConns>().get(core, server_id).await {
            Ok(lease) => lease,
            Err(error) => {
                if matches!(error, CoreError::Network { .. }) {
                    if retry(core, id, &mut retries).await {
                        continue;
                    }
                } else {
                    state(
                        core,
                        id,
                        FileTransferState::Failed,
                        Some(core_failure(&error)),
                    );
                }
                return;
            }
        };
        let Some(local) = transfer.local_path.as_deref() else {
            state(
                core,
                id,
                FileTransferState::Failed,
                Some(FileTransferFail::Io),
            );
            return;
        };
        let mut progress = |progress: Progress| {
            if !job.cancelled.load(Ordering::Acquire) {
                if progress.total != transfer.size {
                    transfers.update(core, id, |entry| entry.size = progress.total);
                }
                transfers.progress(core, id, progress.done);
            }
        };
        let opts = TransferOpts::default();
        let result = match transfer.direction {
            FileTransferDirection::Download => {
                let Some(remote) = transfer.remote_path.as_deref() else {
                    return;
                };
                sftp_transfer::download(
                    lease.sftp(),
                    remote,
                    Path::new(local),
                    &opts,
                    &mut progress,
                    cancel,
                )
                .await
            }
            FileTransferDirection::Upload => {
                let Some(remote) = upload_path(core, &transfer) else {
                    return;
                };
                sftp_transfer::upload(
                    lease.sftp(),
                    Path::new(local),
                    &remote,
                    &opts,
                    &mut progress,
                    cancel,
                )
                .await
            }
        };
        if job.cancelled.load(Ordering::Acquire) {
            return;
        }
        match result {
            Ok(TransferOutcome::Completed { bytes, .. }) => {
                transfers.progress(core, id, bytes);
                state(core, id, FileTransferState::Completed, None);
                return;
            }
            Ok(TransferOutcome::Cancelled) => return,
            Err(TransferError::AlreadyExists)
                if transfer.direction == FileTransferDirection::Upload =>
            {
                match reserve_upload(core, &lease, &transfer).await {
                    Ok(path) => {
                        transfers.update(core, id, |entry| {
                            entry.saved_name = path.rsplit('/').next().map(str::to_owned)
                        });
                        continue;
                    }
                    Err(error) => {
                        if matches!(error, CoreError::Network { .. }) {
                            drop(lease);
                            if retry(core, id, &mut retries).await {
                                continue;
                            }
                        } else {
                            state(
                                core,
                                id,
                                FileTransferState::Failed,
                                Some(core_failure(&error)),
                            );
                        }
                        return;
                    }
                }
            }
            Err(TransferError::RemoteChanged | TransferError::LocalChanged)
                if source_changes == 0 =>
            {
                source_changes += 1;
            }
            Err(TransferError::Sftp(error)) if disconnected(&error) => {
                lease.error(&error).await;
                drop(lease);
                if !retry(core, id, &mut retries).await {
                    return;
                }
            }
            Err(error) => {
                state(
                    core,
                    id,
                    FileTransferState::Failed,
                    Some(transfer_failure(&error)),
                );
                return;
            }
        }
    }
}

async fn discard_partials(core: &Arc<Core>, transfer: &FileTransfer) {
    let id = transfer.transfer_id.as_str();
    let result = match transfer.direction {
        FileTransferDirection::Download => {
            let Some(local) = transfer.local_path.as_deref() else {
                return;
            };
            sftp_transfer::discard_download_partials(Path::new(local)).await
        }
        FileTransferDirection::Upload => {
            let FileSourceRef::Server { server_id } = &transfer.source else {
                return;
            };
            let Some(remote) = upload_path(core, transfer) else {
                return;
            };
            match core.state::<SftpConns>().get(core, server_id).await {
                Ok(lease) => {
                    let result =
                        sftp_transfer::discard_upload_partials(lease.sftp(), &remote).await;
                    if let Err(TransferError::Sftp(error)) = &result {
                        lease.error(error).await;
                    }
                    result
                }
                Err(_) => {
                    tracing::warn!(
                        server_id,
                        transfer_id = id,
                        "SFTP cancellation could not connect; remote partials remain"
                    );
                    return;
                }
            }
        }
    };
    if result.is_err() {
        tracing::warn!(
            transfer_id = id,
            "SFTP cancellation could not discard partials"
        );
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn sftp_list(server_id: String, path: Option<String>) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { Ok(serde_json::to_string(&list(&c, &server_id, path).await?)?) })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn sftp_download(server_id: String, remote_path: String) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { download(&c, &server_id, &remote_path).await })
        .await
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn sftp_upload(
    server_id: String,
    local_path: String,
    remote_dir: Option<String>,
) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { upload(&c, &server_id, &local_path, remote_dir).await })
        .await
}

#[cfg(test)]
#[allow(dead_code)]
#[path = "../../../rs-ssh/tests/support/sftp_server.rs"]
mod test_server;

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn root_has_no_parent() {
        assert_eq!(parent_path("/"), None);
        assert_eq!(parent_path("/home"), Some("/".into()));
        assert_eq!(parent_path("/home/test"), Some("/home".into()));
    }

    #[tokio::test]
    async fn idle_expiry_closes_only_after_last_active_lease() {
        let server = test_server::SftpTestServer::start_temporary("mobile-idle").await;
        let session = server.connect().await;
        let sftp = Arc::new(session.sftp().await.unwrap());
        let weak_sftp = Arc::downgrade(&sftp);
        let connection = Arc::new(Connection {
            session,
            sftp,
            active: AtomicUsize::new(1),
            last_used: Mutex::new(Instant::now()),
        });
        let slot = Arc::new(AsyncMutex::new(Some(connection.clone())));
        let lease = Lease {
            connection,
            slot: slot.clone(),
        };
        let conns = SftpConns::default();
        lock(&conns.servers).insert("server-a".into(), slot.clone());
        tokio::time::pause();
        tokio::time::advance(Duration::from_secs(121)).await;
        conns.reap().await;
        assert!(slot.lock().await.is_some(), "active transfer was evicted");
        drop(lease);
        tokio::time::advance(Duration::from_secs(119)).await;
        conns.reap().await;
        assert!(
            slot.lock().await.is_some(),
            "idle timer did not reset on release"
        );
        tokio::time::advance(Duration::from_secs(1)).await;
        tokio::time::resume();
        conns.reap().await;
        assert!(slot.lock().await.is_none());
        assert!(weak_sftp.upgrade().is_none());
    }

    #[test]
    fn transfer_failures_keep_stable_categories() {
        let cases = [
            (
                TransferError::Sftp(SftpError::NotFound),
                FileTransferFail::NotFound,
            ),
            (
                TransferError::Sftp(SftpError::PermissionDenied),
                FileTransferFail::PermissionDenied,
            ),
            (
                TransferError::Sftp(SftpError::Ssh(SshError::AuthFailed { tried: vec![] })),
                FileTransferFail::AuthRequired,
            ),
            (
                TransferError::RemoteChanged,
                FileTransferFail::SourceChanged,
            ),
            (TransferError::LocalChanged, FileTransferFail::SourceChanged),
            (
                TransferError::SizeMismatch {
                    expected: 3,
                    got: 2,
                },
                FileTransferFail::Protocol,
            ),
            (
                TransferError::Io(std::io::ErrorKind::StorageFull.into()),
                FileTransferFail::NoSpace,
            ),
            (
                TransferError::Io(std::io::ErrorKind::Other.into()),
                FileTransferFail::Io,
            ),
        ];
        for (error, expected) in cases {
            assert_eq!(transfer_failure(&error), expected);
        }
    }
}
