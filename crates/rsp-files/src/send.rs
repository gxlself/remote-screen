//! 文件通道 0x04 的发送引擎（自 `host_core::file_send` 迁入，）。
//!
//! The sender writes `u32 length + FileHeader`, followed by file bytes starting
//! at the header offset and a FIN. The whole-file SHA-256 is computed with
//! asynchronous 64 KiB reads in [`prepare`], so large files do not need to be
//! buffered in memory.

use std::future::Future;
use std::io;
use std::path::{Path, PathBuf};
use std::time::UNIX_EPOCH;

use ring::digest;
use rsp_core::file::{self, FileError};
use rsp_core::file_name::sanitize;
use rsp_core::file_progress::{
    CancelFlag, Direction, FailReason, Transfer, TransferError, TransferProgress,
};
use rsp_transport::{ConnError, UniSend};
use tokio::fs::File;
use tokio::io::{AsyncReadExt, AsyncSeekExt};
use tracing::{info, warn};

const CHUNK_BYTES: usize = 64 * 1024;

/// A stream of raw file-channel bytes.
///
/// This deliberately returns a `Send` future instead of using `async fn` in a
/// trait, matching [`crate::recv::FileSource`].
pub trait FileSink: Send {
    fn write(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send;

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send;
}

impl FileSink for UniSend {
    fn write(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.write_raw(bytes)
    }

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send {
        UniSend::finish(self)
    }
}

/// A file snapshot ready to be sent, including the whole-file hash.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PreparedFile {
    pub path: PathBuf,
    pub transfer_id: String,
    pub name: String,
    pub size: u64,
    pub sha256: [u8; 32],
    pub mtime_us: u64,
}

/// Errors from preparing or sending a file.
#[derive(Debug, thiserror::Error)]
pub enum SendError {
    #[error("source is not a regular file")]
    NotAFile,
    #[error("file offset {offset} exceeds size {size}")]
    OffsetBeyondSize { offset: u64, size: u64 },
    #[error("source file changed since it was prepared")]
    SourceChanged,
    #[error(transparent)]
    Codec(#[from] FileError),
    #[error(transparent)]
    Io(#[from] io::Error),
}

/// Result of one file send attempt.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SendOutcome {
    Completed {
        sent: u64,
    },
    /// `sent` is the absolute file offset of the last successfully written
    /// data byte, excluding the stream prefix.
    Interrupted {
        sent: u64,
    },
    Cancelled,
}

/// Read the source once to compute its SHA-256 and capture stable metadata.
///
/// `transfer_id` must be supplied when resuming an existing transfer; otherwise
/// a fresh random identifier is generated.
pub async fn prepare(path: &Path, transfer_id: Option<String>) -> Result<PreparedFile, SendError> {
    let metadata = tokio::fs::metadata(path).await?;
    if !metadata.is_file() {
        return Err(SendError::NotAFile);
    }

    let mut source = File::open(path).await?;

    let size = metadata.len();
    let mtime_us = metadata_mtime_us(&metadata);
    let transfer_id = match transfer_id {
        Some(transfer_id) => transfer_id,
        None => file::new_transfer_id()?,
    };
    let name = sanitize(&path.file_name().unwrap_or_default().to_string_lossy());

    let mut hasher = digest::Context::new(&digest::SHA256);
    let mut buf = vec![0_u8; CHUNK_BYTES];
    let mut hashed = 0_u64;
    loop {
        let count = source.read(&mut buf).await?;
        if count == 0 {
            break;
        }
        hashed = hashed.saturating_add(count as u64);
        hasher.update(&buf[..count]);
    }
    if hashed != size {
        return Err(SendError::SourceChanged);
    }

    let sha256: [u8; 32] = hasher
        .finish()
        .as_ref()
        .try_into()
        .map_err(|_| io::Error::other("SHA-256 digest has an unexpected length"))?;

    Ok(PreparedFile {
        path: path.to_path_buf(),
        transfer_id,
        name,
        size,
        sha256,
        mtime_us,
    })
}

/// Send one file stream starting at the absolute `offset`.
pub async fn send<K: FileSink>(
    file: &PreparedFile,
    offset: u64,
    sink: &mut K,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    cancel: &CancelFlag,
    now_us: &(dyn Fn() -> u64 + Sync),
) -> Result<SendOutcome, SendError> {
    send_with(file, offset, "", sink, progress, cancel, now_us).await
}

/// Send one file stream with `dest_dir` in its header.
#[allow(clippy::too_many_arguments)]
pub async fn send_with<K: FileSink>(
    file: &PreparedFile,
    offset: u64,
    dest_dir: &str,
    sink: &mut K,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    cancel: &CancelFlag,
    now_us: &(dyn Fn() -> u64 + Sync),
) -> Result<SendOutcome, SendError> {
    if offset > file.size {
        return Err(SendError::OffsetBeyondSize {
            offset,
            size: file.size,
        });
    }

    let mut source = File::open(&file.path).await?;
    let metadata = source.metadata().await?;
    if !metadata.is_file() {
        return Err(SendError::NotAFile);
    }
    if metadata.len() != file.size || metadata_mtime_us(&metadata) != file.mtime_us {
        log_send_result(&file.transfer_id, file.size, "source changed");
        return Err(SendError::SourceChanged);
    }

    let mut header = file::build_header(
        file.transfer_id.clone(),
        sanitize(&file.name),
        file.size,
        offset,
        file.sha256,
        file.mtime_us,
    )?;
    header.dest_dir = dest_dir.to_owned();
    let prefix = file::encode_stream_prefix(&header)?;
    source.seek(io::SeekFrom::Start(offset)).await?;

    let mut transfer = Transfer::new(file.transfer_id.clone(), Direction::Send, file.size);
    let started = transfer
        .start(offset, now_us())
        .map_err(transfer_io_error)?;
    progress(started);

    if cancel.is_cancelled() {
        return Ok(cancel_send(
            &mut transfer,
            progress,
            &file.transfer_id,
            file.size,
        ));
    }

    if sink.write(&prefix).await.is_err() {
        return Ok(interrupt_send(
            &mut transfer,
            progress,
            now_us,
            &file.transfer_id,
            file.size,
            offset,
        ));
    }

    let mut sent = offset;
    let end = file.size;
    let mut buf = vec![0_u8; CHUNK_BYTES];
    while sent < end {
        if cancel.is_cancelled() {
            return Ok(cancel_send(
                &mut transfer,
                progress,
                &file.transfer_id,
                file.size,
            ));
        }

        let want = usize::try_from((end - sent).min(CHUNK_BYTES as u64))
            .map_err(|_| io::Error::other("file read size does not fit usize"))?;
        let count = match source.read(&mut buf[..want]).await {
            Ok(0) => {
                fail_transfer(&mut transfer, progress, FailReason::SourceChanged);
                log_send_result(&file.transfer_id, file.size, "source changed");
                return Err(SendError::SourceChanged);
            }
            Ok(count) => count,
            Err(error) => {
                fail_transfer(&mut transfer, progress, FailReason::Io);
                log_send_result(&file.transfer_id, file.size, "io error");
                return Err(SendError::Io(error));
            }
        };
        if count > want {
            fail_transfer(&mut transfer, progress, FailReason::Io);
            log_send_result(&file.transfer_id, file.size, "io error");
            return Err(SendError::Io(io::Error::new(
                io::ErrorKind::InvalidData,
                "source returned more bytes than requested",
            )));
        }

        if sink.write(&buf[..count]).await.is_err() {
            return Ok(interrupt_send(
                &mut transfer,
                progress,
                now_us,
                &file.transfer_id,
                file.size,
                sent,
            ));
        }
        sent = sent
            .checked_add(count as u64)
            .ok_or_else(|| io::Error::other("file send offset overflow"))?;

        match transfer.advance(count as u64, now_us()) {
            Ok(Some(event)) => progress(event),
            Ok(None) => {}
            Err(error) => {
                fail_transfer(&mut transfer, progress, FailReason::Io);
                log_send_result(&file.transfer_id, file.size, "io error");
                return Err(transfer_io_error(error));
            }
        }

        if cancel.is_cancelled() {
            return Ok(cancel_send(
                &mut transfer,
                progress,
                &file.transfer_id,
                file.size,
            ));
        }
    }

    if cancel.is_cancelled() {
        return Ok(cancel_send(
            &mut transfer,
            progress,
            &file.transfer_id,
            file.size,
        ));
    }

    match source.read(&mut buf[..1]).await {
        Ok(0) => {}
        Ok(_) => {
            fail_transfer(&mut transfer, progress, FailReason::SourceChanged);
            log_send_result(&file.transfer_id, file.size, "source changed");
            return Err(SendError::SourceChanged);
        }
        Err(error) => {
            fail_transfer(&mut transfer, progress, FailReason::Io);
            log_send_result(&file.transfer_id, file.size, "io error");
            return Err(SendError::Io(error));
        }
    }

    if cancel.is_cancelled() {
        return Ok(cancel_send(
            &mut transfer,
            progress,
            &file.transfer_id,
            file.size,
        ));
    }

    if sink.finish().await.is_err() {
        return Ok(interrupt_send(
            &mut transfer,
            progress,
            now_us,
            &file.transfer_id,
            file.size,
            sent,
        ));
    }

    let verifying = transfer.verifying().map_err(transfer_io_error)?;
    progress(verifying);
    let completed = transfer.complete().map_err(transfer_io_error)?;
    progress(completed);
    log_send_result(&file.transfer_id, file.size, "completed");
    Ok(SendOutcome::Completed { sent })
}

fn metadata_mtime_us(metadata: &std::fs::Metadata) -> u64 {
    metadata
        .modified()
        .ok()
        .and_then(|modified| modified.duration_since(UNIX_EPOCH).ok())
        .and_then(|duration| u64::try_from(duration.as_micros()).ok())
        .unwrap_or(0)
}

fn transfer_io_error(error: TransferError) -> SendError {
    SendError::Io(io::Error::other(error))
}

fn fail_transfer(
    transfer: &mut Transfer,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    reason: FailReason,
) {
    if let Ok(event) = transfer.fail(reason) {
        progress(event);
    }
}

fn cancel_send(
    transfer: &mut Transfer,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    transfer_id: &str,
    size: u64,
) -> SendOutcome {
    if let Ok(event) = transfer.cancel() {
        progress(event);
    }
    log_send_result(transfer_id, size, "cancelled");
    SendOutcome::Cancelled
}

fn interrupt_send(
    transfer: &mut Transfer,
    progress: &mut (dyn FnMut(TransferProgress) + Send),
    now_us: &(dyn Fn() -> u64 + Sync),
    transfer_id: &str,
    size: u64,
    sent: u64,
) -> SendOutcome {
    if let Ok(event) = transfer.interrupt(now_us()) {
        progress(event);
    }
    log_send_result(transfer_id, size, "interrupted");
    SendOutcome::Interrupted { sent }
}

fn log_send_result(transfer_id: &str, size: u64, result: &'static str) {
    if result == "source changed" {
        warn!(%transfer_id, size, result, "file send stopped");
    } else {
        info!(%transfer_id, size, result, "file send stopped");
    }
}
