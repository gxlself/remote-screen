//! 文件通道 0x04 的接收引擎（自 `host_core::file_recv` 迁入，）。
//!
//! The receiver accepts a stream of `u32 length + FileHeader`, file bytes and a
//! FIN.  Bytes are written to a resumable `.rs-<id>.part` file; the final name
//! is only allocated after the whole file has passed SHA-256 verification.

use std::ffi::OsString;
use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use ring::digest;
use rsp_core::file::{
    self, DataSpan, FileError, FileStreamReceiver, PartialState, ResumePlan, StreamEnd,
    MAX_FILE_BYTES,
};
use rsp_core::file_name::{
    self, candidate, is_safe_component, partial_meta_name, partial_name, sanitize, MAX_CANDIDATES,
};
use rsp_core::file_progress::{CancelFlag, Direction, FailReason, Transfer, TransferProgress};
use rsp_core::pb::FileHeader;
use rsp_transport::{ConnError, UniRecv};
use serde_json::{json, Value};
use tokio::fs::{self, File, OpenOptions};
use tokio::io::{AsyncReadExt, AsyncSeekExt, AsyncWriteExt};
use tracing::{info, warn};

const DEFAULT_BUFFER_BYTES: usize = 64 * 1024;

/// A stream of raw file-channel bytes. `Ok(None)` means FIN.
///
/// This is deliberately not an `async fn` in a trait so callers can require a
/// `Send` future without triggering Clippy's `async_fn_in_trait` lint.
pub trait FileSource: Send {
    fn read(
        &mut self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<Option<usize>, ConnError>> + Send;
}

impl FileSource for UniRecv {
    fn read(
        &mut self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<Option<usize>, ConnError>> + Send {
        self.read_raw(buf)
    }
}

/// Where and how one incoming file is received.
#[derive(Debug, Clone)]
pub struct ReceiveConfig {
    pub dest_dir: PathBuf,
    pub max_file_bytes: u64,
    pub buf_size: usize,
}

/// Decides whether to receive a file and, optionally, where to publish it.
pub type AcceptFn<'a> = dyn FnMut(&FileHeader) -> Result<Option<PathBuf>, RecvError> + Send + 'a;

impl Default for ReceiveConfig {
    fn default() -> Self {
        Self {
            dest_dir: PathBuf::new(),
            max_file_bytes: MAX_FILE_BYTES,
            buf_size: DEFAULT_BUFFER_BYTES,
        }
    }
}

/// Result of receiving one file stream.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ReceiveOutcome {
    Completed {
        transfer_id: String,
        path: PathBuf,
        size: u64,
    },
    /// The peer stopped before the declared size; the partial files remain and
    /// the transfer can resume from `received`.
    Interrupted {
        transfer_id: String,
        received: u64,
    },
    Cancelled {
        transfer_id: String,
    },
}

/// Errors from the file receive engine.
#[derive(Debug, thiserror::Error)]
pub enum RecvError {
    #[error(transparent)]
    Codec(#[from] FileError),
    #[error("file rejected: {0:?}")]
    Rejected(rsp_core::pb::FileError),
    #[error(transparent)]
    Transport(#[from] ConnError),
    #[error("destination directory does not exist or is not a directory")]
    DestMissing,
    #[error("file is {size} bytes; the receive limit is {limit} bytes")]
    FileTooLarge { size: u64, limit: u64 },
    #[error("resume state does not match: received {have} bytes")]
    ResumeMismatch { have: u64 },
    #[error("file SHA-256 does not match the header")]
    Sha256Mismatch,
    #[error("no free destination name")]
    NoFreeName,
    #[error("file name is not safe to use as a destination component")]
    UnsafeName,
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Receive one complete file stream.
///
/// The destination directory must already exist. No temporary or final file is
/// created until a valid header has arrived and the declared size has passed
/// [`ReceiveConfig::max_file_bytes`].
pub async fn receive<S: FileSource>(
    source: &mut S,
    cfg: &ReceiveConfig,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    cancel: &CancelFlag,
    now_us: &(dyn Fn() -> u64 + Sync),
) -> Result<ReceiveOutcome, RecvError> {
    receive_with(source, cfg, &mut |_| Ok(None), progress, cancel, now_us).await
}

/// Receive one complete file stream, asking `accept` after the header arrives.
///
/// The partial files stay in [`ReceiveConfig::dest_dir`]. If `accept` returns a
/// directory, the verified file is published there instead. No temporary or
/// final file is created before `accept` succeeds.
#[allow(clippy::too_many_arguments)]
pub async fn receive_with<S: FileSource>(
    source: &mut S,
    cfg: &ReceiveConfig,
    accept: &mut AcceptFn<'_>,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    cancel: &CancelFlag,
    now_us: &(dyn Fn() -> u64 + Sync),
) -> Result<ReceiveOutcome, RecvError> {
    let mut buf = vec![0_u8; cfg.buf_size.max(1)];
    let mut receiver = Some(FileStreamReceiver::new());
    let mut active: Option<Active> = None;

    loop {
        if active.is_some() && cancel.is_cancelled() {
            let active = active.take().ok_or_else(|| {
                io_error(
                    io::ErrorKind::InvalidData,
                    "file transfer state disappeared",
                )
            })?;
            return cancel_active(active, progress).await;
        }

        let read = source.read(&mut buf).await;
        let count = match read {
            Ok(Some(count)) => count,
            Ok(None) => {
                let receiver = receiver.take().ok_or_else(|| {
                    io_error(
                        io::ErrorKind::InvalidData,
                        "file stream was already finished",
                    )
                })?;
                let end = receiver.finish()?;
                let Some(active) = active.take() else {
                    return Err(RecvError::Codec(FileError::EmptyStream));
                };
                return match end {
                    StreamEnd::Complete { .. } => finish_complete(active, cfg, progress).await,
                    StreamEnd::Truncated { .. } => interrupt_active(active, progress, now_us).await,
                };
            }
            Err(error) => {
                if let Some(active) = active.take() {
                    return interrupt_active(active, progress, now_us).await;
                }
                return Err(RecvError::Transport(error));
            }
        };

        if count > buf.len() {
            return Err(io_error(
                io::ErrorKind::InvalidData,
                "file source returned more bytes than the provided buffer holds",
            ));
        }
        if count == 0 {
            continue;
        }

        let receiver = receiver.as_mut().ok_or_else(|| {
            io_error(
                io::ErrorKind::InvalidData,
                "file stream was already finished",
            )
        })?;
        let fed = match receiver.feed(&buf[..count]) {
            Ok(fed) => fed,
            Err(error) => {
                if let Some(active) = active.take() {
                    return codec_after_header(active, error, progress).await;
                }
                return Err(RecvError::Codec(error));
            }
        };

        if let Some(header) = fed.header {
            if active.is_some() {
                return Err(io_error(
                    io::ErrorKind::InvalidData,
                    "file stream delivered its header twice",
                ));
            }
            active = Some(prepare(header, cfg, accept, progress, now_us).await?);
        }

        if let Some(DataSpan { offset, bytes }) = fed.data {
            let active = active.as_mut().ok_or_else(|| {
                io_error(
                    io::ErrorKind::InvalidData,
                    "file data arrived before its header",
                )
            })?;
            write_span(active, offset, bytes, progress, now_us).await?;
        }
    }
}

/// Number of bytes already present for a resumable transfer.
///
/// Returns `None` when both files are not a matching pair, the sidecar is
/// malformed, or either file is absent.
pub fn partial_received(dest_dir: &Path, transfer_id: &str) -> Result<Option<u64>, RecvError> {
    let part_name = partial_name(transfer_id).map_err(|_| RecvError::UnsafeName)?;
    let meta_name = partial_meta_name(transfer_id).map_err(|_| RecvError::UnsafeName)?;
    let meta_path = dest_dir.join(meta_name);
    let part_path = dest_dir.join(part_name);

    let Some(meta) = read_meta_sync(&meta_path)? else {
        return Ok(None);
    };
    if meta.transfer_id != transfer_id || !is_safe_component(&meta.name) {
        return Ok(None);
    }

    match std::fs::metadata(&part_path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= meta.size => {
            Ok(Some(metadata.len()))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RecvError::Io(error)),
    }
}

/// Metadata of a resumable partial file, for continuing a download.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialInfo {
    pub received: u64,
    pub size: u64,
    pub sha256: [u8; 32],
    pub mtime_us: u64,
    pub name: String,
}

/// Read the metadata and current length of a resumable partial file.
///
/// Returns `None` when the sidecar is missing or malformed, or when the data
/// file is absent or does not form a matching pair with the sidecar.
pub fn partial_info(dest_dir: &Path, transfer_id: &str) -> Result<Option<PartialInfo>, RecvError> {
    let part_name = partial_name(transfer_id).map_err(|_| RecvError::UnsafeName)?;
    let meta_name = partial_meta_name(transfer_id).map_err(|_| RecvError::UnsafeName)?;
    let meta_path = dest_dir.join(meta_name);
    let part_path = dest_dir.join(part_name);

    let Some(meta) = read_meta_sync(&meta_path)? else {
        return Ok(None);
    };
    if meta.transfer_id != transfer_id || !is_safe_component(&meta.name) {
        return Ok(None);
    }

    match std::fs::metadata(&part_path) {
        Ok(metadata) if metadata.is_file() && metadata.len() <= meta.size => {
            Ok(Some(PartialInfo {
                received: metadata.len(),
                size: meta.size,
                sha256: meta.sha256,
                mtime_us: meta.mtime_us,
                name: meta.name,
            }))
        }
        Ok(_) => Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RecvError::Io(error)),
    }
}

/// Delete the partial data and metadata for `transfer_id`.
///
/// Returns whether at least one of the two files was removed.
pub fn discard_partial(dest_dir: &Path, transfer_id: &str) -> Result<bool, RecvError> {
    file::validate_transfer_id(transfer_id)?;
    let part_path = dest_dir.join(partial_name(transfer_id).map_err(|_| RecvError::UnsafeName)?);
    let meta_path =
        dest_dir.join(partial_meta_name(transfer_id).map_err(|_| RecvError::UnsafeName)?);

    let mut removed = false;
    for path in [&part_path, &meta_path] {
        match std::fs::remove_file(path) {
            Ok(()) => removed = true,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(RecvError::Io(error)),
        }
    }
    Ok(removed)
}

/// Delete resumable partial files older than `max_age`.
///
/// Only regular files whose names are `.rs-<32 lowercase hex>.part` or
/// `.rs-<32 lowercase hex>.json` are considered. Other directory entries are
/// left untouched.
pub fn cleanup_stale(
    dest_dir: &Path,
    max_age: Duration,
    now: SystemTime,
) -> Result<usize, RecvError> {
    let cutoff = now.checked_sub(max_age).unwrap_or(UNIX_EPOCH);
    let mut removed = 0_usize;

    for entry in std::fs::read_dir(dest_dir)? {
        let entry = entry?;
        let file_name = entry.file_name();
        let Some(file_name) = file_name.to_str() else {
            continue;
        };
        if file_name::parse_partial_name(file_name).is_none() {
            continue;
        }

        let path = entry.path();
        let metadata = match std::fs::symlink_metadata(&path) {
            Ok(metadata) => metadata,
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(RecvError::Io(error)),
        };
        if !metadata.is_file() || metadata.modified()? >= cutoff {
            continue;
        }

        match std::fs::remove_file(&path) {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(RecvError::Io(error)),
        }
    }

    Ok(removed)
}

struct Active {
    header: FileHeader,
    final_dir: PathBuf,
    part_path: PathBuf,
    meta_path: PathBuf,
    file: File,
    transfer: Transfer,
    received: u64,
}

async fn prepare(
    header: FileHeader,
    cfg: &ReceiveConfig,
    accept: &mut AcceptFn<'_>,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    now_us: &(dyn Fn() -> u64 + Sync),
) -> Result<Active, RecvError> {
    match fs::metadata(&cfg.dest_dir).await {
        Ok(metadata) if metadata.is_dir() => {}
        Ok(_) => return Err(RecvError::DestMissing),
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            return Err(RecvError::DestMissing);
        }
        Err(error) => return Err(RecvError::Io(error)),
    }

    if header.size > cfg.max_file_bytes {
        return Err(RecvError::FileTooLarge {
            size: header.size,
            limit: cfg.max_file_bytes,
        });
    }

    let final_dir = accept(&header)?.unwrap_or_else(|| cfg.dest_dir.clone());
    if final_dir != cfg.dest_dir {
        match fs::metadata(&final_dir).await {
            Ok(metadata) if metadata.is_dir() => {}
            Ok(_) => return Err(RecvError::DestMissing),
            Err(error) if error.kind() == io::ErrorKind::NotFound => {
                return Err(RecvError::DestMissing);
            }
            Err(error) => return Err(RecvError::Io(error)),
        }
    }

    let part_file_name = partial_name(&header.transfer_id).map_err(|_| RecvError::UnsafeName)?;
    let meta_file_name =
        partial_meta_name(&header.transfer_id).map_err(|_| RecvError::UnsafeName)?;
    let part_path = cfg.dest_dir.join(part_file_name);
    let meta_path = cfg.dest_dir.join(meta_file_name);

    let partial = read_partial_state(&part_path, &meta_path, &header.transfer_id).await?;
    let plan = match file::resume_check(&header, partial.as_ref()) {
        Ok(plan) => plan,
        Err(FileError::ResumeMismatch { have }) => return Err(RecvError::ResumeMismatch { have }),
        Err(error) => return Err(RecvError::Codec(error)),
    };

    let (file, received) = match plan {
        ResumePlan::Fresh => {
            let mut options = OpenOptions::new();
            options.read(true).write(true).create(true).truncate(true);
            #[cfg(unix)]
            options.mode(0o600);
            let file = options.open(&part_path).await?;
            set_mode(&file, 0o600).await?;
            write_metadata(&cfg.dest_dir, &meta_path, &header).await?;
            (file, 0)
        }
        ResumePlan::Continue { truncate_to } => {
            let mut file = OpenOptions::new()
                .read(true)
                .write(true)
                .open(&part_path)
                .await?;
            file.set_len(truncate_to).await?;
            file.seek(std::io::SeekFrom::Start(truncate_to)).await?;
            (file, truncate_to)
        }
    };

    let mut transfer = Transfer::new(header.transfer_id.clone(), Direction::Receive, header.size);
    let started = transfer
        .start(header.offset, now_us())
        .map_err(transfer_io_error)?;
    progress(started);

    Ok(Active {
        header,
        final_dir,
        part_path,
        meta_path,
        file,
        transfer,
        received,
    })
}

async fn write_span(
    active: &mut Active,
    offset: u64,
    bytes: &[u8],
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    now_us: &(dyn Fn() -> u64 + Sync),
) -> Result<(), RecvError> {
    if offset != active.received {
        return Err(io_error(
            io::ErrorKind::InvalidData,
            format!(
                "file data offset {offset} does not match current length {}",
                active.received
            ),
        ));
    }

    if let Err(error) = active.file.write_all(bytes).await {
        let _ = active.file.sync_all().await;
        if let Ok(event) = active.transfer.fail(FailReason::Io) {
            progress(event);
        }
        return Err(RecvError::Io(error));
    }
    active.received += bytes.len() as u64;

    match active.transfer.advance(bytes.len() as u64, now_us()) {
        Ok(Some(event)) => progress(event),
        Ok(None) => {}
        Err(error) => {
            if let Ok(event) = active.transfer.fail(FailReason::Io) {
                progress(event);
            }
            return Err(transfer_io_error(error));
        }
    }
    Ok(())
}

async fn interrupt_active(
    mut active: Active,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    now_us: &(dyn Fn() -> u64 + Sync),
) -> Result<ReceiveOutcome, RecvError> {
    active.file.sync_all().await?;
    let event = active
        .transfer
        .interrupt(now_us())
        .map_err(transfer_io_error)?;
    progress(event);
    info!(
        transfer_id = %active.header.transfer_id,
        size = active.header.size,
        received = active.received,
        result = "interrupted",
        "file receive stopped"
    );
    Ok(ReceiveOutcome::Interrupted {
        transfer_id: active.header.transfer_id,
        received: active.received,
    })
}

async fn cancel_active(
    mut active: Active,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
) -> Result<ReceiveOutcome, RecvError> {
    let event = active.transfer.cancel().map_err(transfer_io_error)?;
    progress(event);
    drop(active.file);
    remove_if_present(&active.part_path).await?;
    remove_if_present(&active.meta_path).await?;
    info!(
        transfer_id = %active.header.transfer_id,
        size = active.header.size,
        result = "cancelled",
        "file receive cancelled"
    );
    Ok(ReceiveOutcome::Cancelled {
        transfer_id: active.header.transfer_id,
    })
}

async fn codec_after_header(
    mut active: Active,
    error: FileError,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
) -> Result<ReceiveOutcome, RecvError> {
    let _ = active.file.sync_all().await;
    if let Ok(event) = active.transfer.fail(FailReason::Protocol) {
        progress(event);
    }
    warn!(
        transfer_id = %active.header.transfer_id,
        size = active.header.size,
        result = "protocol error",
        "file receive failed"
    );
    Err(RecvError::Codec(error))
}

async fn finish_complete(
    mut active: Active,
    cfg: &ReceiveConfig,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
) -> Result<ReceiveOutcome, RecvError> {
    active.file.sync_all().await?;
    let event = active.transfer.verifying().map_err(transfer_io_error)?;
    progress(event);

    let mut verify_file = File::open(&active.part_path).await?;
    let mut hasher = digest::Context::new(&digest::SHA256);
    let mut buf = vec![0_u8; DEFAULT_BUFFER_BYTES];
    let mut hashed = 0_u64;
    loop {
        let count = verify_file.read(&mut buf).await?;
        if count == 0 {
            break;
        }
        hashed += count as u64;
        hasher.update(&buf[..count]);
    }
    let actual = hasher.finish();

    if hashed != active.header.size || actual.as_ref() != active.header.sha256.as_slice() {
        drop(verify_file);
        drop(active.file);
        remove_if_present(&active.part_path).await?;
        remove_if_present(&active.meta_path).await?;
        let event = active
            .transfer
            .fail(FailReason::Sha256Mismatch)
            .map_err(transfer_io_error)?;
        progress(event);
        warn!(
            transfer_id = %active.header.transfer_id,
            size = active.header.size,
            result = "sha256 mismatch",
            "file receive failed"
        );
        return Err(RecvError::Sha256Mismatch);
    }

    if active.header.mtime_us > 0 {
        let modified = UNIX_EPOCH
            .checked_add(Duration::from_micros(active.header.mtime_us))
            .ok_or_else(|| {
                io_error(
                    io::ErrorKind::InvalidInput,
                    "file modification time is out of range",
                )
            })?;
        verify_file.set_modified(modified).await?;
    }
    set_mode(&verify_file, 0o644).await?;
    drop(verify_file);
    drop(active.file);

    let safe_name = sanitize(&active.header.name);
    let (placeholder_path, final_name) = reserve_name(&active.final_dir, &safe_name).await?;
    let final_path = active.final_dir.join(&final_name);
    if !is_safe_component(&final_name)
        || final_path.parent() != Some(active.final_dir.as_path())
        || placeholder_path != final_path
    {
        let _ = fs::remove_file(&placeholder_path).await;
        return Err(RecvError::UnsafeName);
    }

    if let Err(error) = publish_partial(&active.part_path, &final_path, false).await {
        let _ = fs::remove_file(&placeholder_path).await;
        return Err(error);
    }
    if let Err(error) = remove_if_present(&active.meta_path).await {
        warn!(
            transfer_id = %active.header.transfer_id,
            size = active.header.size,
            result = "metadata cleanup failed",
            "file receive completed with cleanup error"
        );
        return Err(error);
    }
    sync_dir(&active.final_dir).await?;
    if active.final_dir != cfg.dest_dir {
        sync_dir(&cfg.dest_dir).await?;
    }

    let event = active.transfer.complete().map_err(transfer_io_error)?;
    progress(event);
    info!(
        transfer_id = %active.header.transfer_id,
        size = active.header.size,
        result = "completed",
        "file receive completed"
    );
    Ok(ReceiveOutcome::Completed {
        transfer_id: active.header.transfer_id,
        path: final_path,
        size: active.header.size,
    })
}

/// Move a verified partial file to its final name, copying on cross-device
/// renames. `force_copy` is exposed for the copy-path test.
pub(crate) async fn publish_partial(
    part_path: &Path,
    final_path: &Path,
    force_copy: bool,
) -> Result<(), RecvError> {
    if !force_copy {
        match fs::rename(part_path, final_path).await {
            Ok(()) => return Ok(()),
            Err(error) if error.kind() == io::ErrorKind::CrossesDevices => {}
            Err(error) => return Err(RecvError::Io(error)),
        }
    }

    let result = async {
        let mut source = File::open(part_path).await?;
        let modified = source.metadata().await?.modified().ok();
        let mut destination = OpenOptions::new()
            .write(true)
            .create(true)
            .truncate(true)
            .open(final_path)
            .await?;
        let mut buf = vec![0_u8; DEFAULT_BUFFER_BYTES];
        loop {
            let count = source.read(&mut buf).await?;
            if count == 0 {
                break;
            }
            destination.write_all(&buf[..count]).await?;
        }
        set_mode(&destination, 0o644).await?;
        if let Some(modified) = modified {
            destination.set_modified(modified).await?;
        }
        destination.sync_all().await?;
        Ok::<(), RecvError>(())
    }
    .await;

    if let Err(error) = result {
        let _ = fs::remove_file(final_path).await;
        return Err(error);
    }
    remove_if_present(part_path).await
}

async fn reserve_name(dest_dir: &Path, safe_name: &str) -> Result<(PathBuf, String), RecvError> {
    for number in 0..MAX_CANDIDATES {
        let name = candidate(safe_name, number);
        if !is_safe_component(&name) {
            return Err(RecvError::UnsafeName);
        }
        let path = dest_dir.join(&name);
        if path.parent() != Some(dest_dir) {
            return Err(RecvError::UnsafeName);
        }

        let mut options = OpenOptions::new();
        options.write(true).create_new(true);
        #[cfg(unix)]
        options.mode(0o644);
        match options.open(&path).await {
            Ok(placeholder) => {
                drop(placeholder);
                return Ok((path, name));
            }
            Err(error) if error.kind() == io::ErrorKind::AlreadyExists => continue,
            Err(error) => return Err(RecvError::Io(error)),
        }
    }
    Err(RecvError::NoFreeName)
}

async fn read_partial_state(
    part_path: &Path,
    meta_path: &Path,
    transfer_id: &str,
) -> Result<Option<PartialState>, RecvError> {
    let Some(meta) = read_meta(meta_path).await? else {
        return Ok(None);
    };
    if meta.transfer_id != transfer_id || !is_safe_component(&meta.name) {
        return Ok(None);
    }

    let length = match fs::metadata(part_path).await {
        Ok(metadata) if metadata.is_file() => metadata.len(),
        Ok(_) => return Ok(None),
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(None),
        Err(error) => return Err(RecvError::Io(error)),
    };

    Ok(Some(PartialState {
        transfer_id: meta.transfer_id,
        size: meta.size,
        sha256: meta.sha256,
        mtime_us: meta.mtime_us,
        received: length,
    }))
}

struct PartialMeta {
    transfer_id: String,
    name: String,
    size: u64,
    sha256: [u8; 32],
    mtime_us: u64,
}

async fn read_meta(path: &Path) -> Result<Option<PartialMeta>, RecvError> {
    match fs::read(path).await {
        Ok(bytes) => Ok(parse_meta(&bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RecvError::Io(error)),
    }
}

fn read_meta_sync(path: &Path) -> Result<Option<PartialMeta>, RecvError> {
    match std::fs::read(path) {
        Ok(bytes) => Ok(parse_meta(&bytes)),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(None),
        Err(error) => Err(RecvError::Io(error)),
    }
}

fn parse_meta(bytes: &[u8]) -> Option<PartialMeta> {
    let value: Value = serde_json::from_slice(bytes).ok()?;
    let transfer_id = value.get("transfer_id")?.as_str()?.to_owned();
    let name = value.get("name")?.as_str()?.to_owned();
    let size = value.get("size")?.as_u64()?;
    let sha256_hex = value.get("sha256_hex")?.as_str()?;
    let mtime_us = value.get("mtime_us")?.as_u64()?;
    Some(PartialMeta {
        transfer_id,
        name,
        size,
        sha256: decode_hex_32(sha256_hex)?,
        mtime_us,
    })
}

async fn write_metadata(
    dest_dir: &Path,
    meta_path: &Path,
    header: &FileHeader,
) -> Result<(), RecvError> {
    let value = json!({
        "transfer_id": header.transfer_id,
        "name": sanitize(&header.name),
        "size": header.size,
        "sha256_hex": encode_hex(&header.sha256),
        "mtime_us": header.mtime_us,
    });
    let bytes = serde_json::to_vec(&value).map_err(json_io_error)?;
    let mut temporary_name = OsString::from(
        meta_path
            .file_name()
            .ok_or_else(|| io_error(io::ErrorKind::InvalidInput, "metadata path has no name"))?,
    );
    temporary_name.push(".tmp");
    let temporary = dest_dir.join(temporary_name);

    let result = async {
        let mut options = OpenOptions::new();
        options.write(true).create(true).truncate(true);
        #[cfg(unix)]
        options.mode(0o600);
        let mut file = options.open(&temporary).await?;
        file.write_all(&bytes).await?;
        file.sync_all().await?;
        drop(file);
        fs::rename(&temporary, meta_path).await?;
        Ok::<(), io::Error>(())
    }
    .await;

    if result.is_err() {
        let _ = fs::remove_file(&temporary).await;
    }
    result?;
    sync_dir(dest_dir).await?;
    Ok(())
}

async fn remove_if_present(path: &Path) -> Result<(), RecvError> {
    match fs::remove_file(path).await {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(RecvError::Io(error)),
    }
}

async fn sync_dir(path: &Path) -> Result<(), RecvError> {
    #[cfg(unix)]
    {
        File::open(path).await?.sync_all().await?;
    }
    #[cfg(not(unix))]
    {
        let _ = path;
    }
    Ok(())
}

async fn set_mode(file: &File, mode: u32) -> Result<(), RecvError> {
    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        file.set_permissions(std::fs::Permissions::from_mode(mode))
            .await?;
    }
    #[cfg(not(unix))]
    {
        let _ = (file, mode);
    }
    Ok(())
}

fn transfer_io_error(error: rsp_core::file_progress::TransferError) -> RecvError {
    RecvError::Io(io::Error::other(error))
}

fn json_io_error(error: serde_json::Error) -> RecvError {
    RecvError::Io(io::Error::other(error))
}

fn io_error(kind: io::ErrorKind, message: impl Into<String>) -> RecvError {
    RecvError::Io(io::Error::new(kind, message.into()))
}

fn encode_hex(bytes: &[u8]) -> String {
    const HEX: &[u8; 16] = b"0123456789abcdef";
    let mut text = String::with_capacity(bytes.len() * 2);
    for byte in bytes {
        text.push(HEX[(byte >> 4) as usize] as char);
        text.push(HEX[(byte & 0x0f) as usize] as char);
    }
    text
}

fn decode_hex_32(text: &str) -> Option<[u8; 32]> {
    if text.len() != 64 {
        return None;
    }
    let mut bytes = [0_u8; 32];
    for (target, pair) in bytes.iter_mut().zip(text.as_bytes().chunks_exact(2)) {
        let high = hex_digit(pair[0])?;
        let low = hex_digit(pair[1])?;
        *target = (high << 4) | low;
    }
    Some(bytes)
}

fn hex_digit(byte: u8) -> Option<u8> {
    match byte {
        b'0'..=b'9' => Some(byte - b'0'),
        b'a'..=b'f' => Some(byte - b'a' + 10),
        b'A'..=b'F' => Some(byte - b'A' + 10),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use std::sync::atomic::{AtomicU64, Ordering};
    use std::time::{SystemTime, UNIX_EPOCH};

    use super::publish_partial;

    static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

    #[tokio::test]
    async fn forced_copy_publishes_content_and_removes_the_partial() {
        let serial = NEXT_DIR.fetch_add(1, Ordering::Relaxed);
        let nanos = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .expect("test clock is after the epoch")
            .as_nanos();
        let dir = std::env::temp_dir().join(format!(
            "rs-publish-copy-{}-{nanos}-{serial}",
            std::process::id()
        ));
        std::fs::create_dir_all(&dir).expect("test directory is created");
        let data: Vec<u8> = (0_u32..160 * 1024)
            .map(|index| (index.wrapping_mul(13) % 241) as u8)
            .collect();
        let part = dir.join("source.part");
        let final_path = dir.join("published.bin");
        std::fs::write(&part, &data).expect("partial file is written");

        let result = publish_partial(&part, &final_path, true).await;
        let bytes = std::fs::read(&final_path);
        let part_exists = part.exists();

        result.expect("forced copy succeeds");
        assert_eq!(bytes.expect("published file is readable"), data);
        assert!(!part_exists);
        let _ = std::fs::remove_dir_all(&dir);
    }
}
