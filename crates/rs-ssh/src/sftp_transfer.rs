//! Resumable SFTP transfers with progress reporting and cancellation.
//!
//! Partial files live beside their target so a later call can resume after a
//! network failure. The suffix is a fingerprint of the source path, size and
//! mtime. A changed source therefore gets a different partial file and can
//! never be confused with an older transfer.

use std::{
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
    time::{Duration, UNIX_EPOCH},
};

use thiserror::Error;
use tokio::{
    fs::{self, File},
    io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _},
};

use crate::sftp::{DirEntry, EntryKind, Sftp, SftpError};

const DEFAULT_CHUNK: usize = 32 * 1024;
/// Uploads keep at most this many writes in flight. A disconnect can leave
/// the remote file with a hole, so resume rewinds by this window and rewrites
/// the overlapping chunks with identical bytes.
const UPLOAD_WINDOW: usize = 16;
const PARTIAL_INFIX: &str = ".rspart-";

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct TransferOpts {
    /// Bytes requested per SFTP read or write. The default is 32 KiB.
    pub chunk: usize,
    /// Replace an existing destination. The default is false.
    pub overwrite: bool,
}

impl Default for TransferOpts {
    fn default() -> Self {
        Self {
            chunk: DEFAULT_CHUNK,
            overwrite: false,
        }
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub struct Progress {
    pub done: u64,
    pub total: u64,
}

#[derive(Clone, Default, Debug)]
pub struct Cancel(Arc<AtomicBool>);

impl Cancel {
    pub fn cancel(&self) {
        self.0.store(true, Ordering::SeqCst);
    }

    pub fn is_cancelled(&self) -> bool {
        self.0.load(Ordering::SeqCst)
    }
}

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum TransferOutcome {
    Completed { bytes: u64, resumed_from: u64 },
    Cancelled,
}

#[derive(Debug, Error)]
pub enum TransferError {
    #[error(transparent)]
    Sftp(#[from] SftpError),
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("destination already exists")]
    AlreadyExists,
    #[error("source is not a regular file")]
    NotAFile,
    #[error("remote file changed during the transfer")]
    RemoteChanged,
    #[error("local file changed during the transfer")]
    LocalChanged,
    #[error("size mismatch: expected {expected} bytes, got {got}")]
    SizeMismatch { expected: u64, got: u64 },
}

/// Downloads a regular remote file to `local`.
///
/// The local partial file is kept on transport errors, removed on cancellation,
/// and atomically renamed only after the size and remote identity checks pass.
pub async fn download(
    sftp: &Sftp,
    remote: &str,
    local: &Path,
    opts: &TransferOpts,
    progress: &mut (dyn FnMut(Progress) + Send),
    cancel: &Cancel,
) -> Result<TransferOutcome, TransferError> {
    let chunk = checked_chunk(opts)?;
    if !is_file(sftp.stat(remote).await?)? {
        return Err(TransferError::NotAFile);
    }
    if local.exists() && !opts.overwrite {
        return Err(TransferError::AlreadyExists);
    }

    let metadata = fs::metadata(local).await;
    if let Ok(metadata) = metadata {
        if !metadata.is_file() {
            return Err(TransferError::NotAFile);
        }
    }

    let remote_stat = sftp.stat(remote).await?;
    let size = source_size(&remote_stat)?;
    let mtime = source_mtime(&remote_stat)?;
    let tag = fingerprint(remote, size, mtime);
    let partial = partial_path(local, &tag)?;
    let siblings = partial_siblings(local).await?;
    remove_partials_except(&siblings, &partial).await?;

    let mut resumed_from = local_len_if_present(&partial).await?;
    if resumed_from > size {
        fs::remove_file(&partial).await?;
        resumed_from = 0;
    }
    if local.exists() && !fs::metadata(local).await?.is_file() {
        return Err(TransferError::NotAFile);
    }

    let mut file = match open_local_partial(&partial, resumed_from).await {
        Ok(file) => file,
        Err(error) => {
            keep_or_remove_partial(&partial, &error).await;
            return Err(TransferError::Io(error));
        }
    };

    // Keep every exit after opening the local file behind the same completion
    // barrier. Tokio file writes can finish accepting a buffer before their
    // blocking write has actually completed.
    let transfer: Result<Option<u64>, TransferError> = async {
        progress(Progress {
            done: resumed_from,
            total: size,
        });
        if cancel.is_cancelled() {
            return Ok(None);
        }

        let mut remote_file = sftp.open_read(remote, chunk).await?;
        let received = async {
            if resumed_from > 0 {
                remote_file.seek(resumed_from).await?;
            }
            let mut done = resumed_from;
            while done < size {
                if cancel.is_cancelled() {
                    return Ok(None);
                }
                let want = chunk.min((size - done) as usize);
                let data = remote_file.read_exact(want).await?;
                if data.is_empty() {
                    break;
                }
                file.write_all(&data).await?;
                done += data.len() as u64;
                progress(Progress { done, total: size });
            }
            Ok::<_, TransferError>((!cancel.is_cancelled()).then_some(done))
        }
        .await;

        // Closing consumes the read handle and discards its pending read
        // receivers. No reply can then enter the local write loop. On a
        // failed connection, dropping the handle also discards those receivers.
        let closed = remote_file.close().await;
        match received {
            Ok(Some(done)) => {
                closed?;
                Ok(Some(done))
            }
            result => result,
        }
    }
    .await;

    // Always await both operations, including on SFTP failure or cancellation.
    // `flush` waits for the last blocking write and surfaces its I/O error;
    // `sync_data` makes a retained partial durable before exposing its length.
    let flushed = file.flush().await;
    let synced = file.sync_data().await;
    if let Err(error) = flushed.and(synced) {
        drop(file);
        if cancel.is_cancelled() {
            remove_local_partial(&partial).await;
        } else {
            keep_or_remove_partial(&partial, &error).await;
        }
        return Err(TransferError::Io(error));
    }
    let done = match transfer {
        Ok(Some(done)) => done,
        Ok(None) => {
            drop(file);
            remove_local_partial(&partial).await;
            return Ok(TransferOutcome::Cancelled);
        }
        Err(error) => {
            drop(file);
            keep_or_remove_partial(&partial, &error).await;
            return Err(error);
        }
    };

    if done != size {
        remove_local_partial(&partial).await;
        return Err(TransferError::SizeMismatch {
            expected: size,
            got: done,
        });
    }
    let partial_size = fs::metadata(&partial).await?.len();
    if partial_size != size {
        remove_local_partial(&partial).await;
        return Err(TransferError::SizeMismatch {
            expected: size,
            got: partial_size,
        });
    }

    let after = match sftp.stat(remote).await {
        Ok(stat) => stat,
        Err(error) => {
            keep_or_remove_partial(&partial, &error).await;
            return Err(TransferError::Sftp(error));
        }
    };
    if source_size(&after)? != size || source_mtime(&after)? != mtime {
        remove_local_partial(&partial).await;
        return Err(TransferError::RemoteChanged);
    }

    if let Err(error) = file.sync_all().await {
        keep_or_remove_partial(&partial, &error).await;
        return Err(TransferError::Io(error));
    }
    if let Err(error) = file.set_modified(unix_time(mtime)?).await {
        keep_or_remove_partial(&partial, &error).await;
        return Err(TransferError::Io(error));
    }
    drop(file);

    if let Err(error) = fs::rename(&partial, local).await {
        if opts.overwrite {
            // Unix rename replaces atomically. Filesystems that reject replacing
            // an existing target get one portable remove-then-retry fallback.
            if fs::remove_file(local).await.is_ok() {
                if let Err(retry) = fs::rename(&partial, local).await {
                    keep_or_remove_partial(&partial, &retry).await;
                    return Err(TransferError::Io(retry));
                }
            } else {
                keep_or_remove_partial(&partial, &error).await;
                return Err(TransferError::Io(error));
            }
        } else {
            keep_or_remove_partial(&partial, &error).await;
            return Err(TransferError::Io(error));
        }
    }

    Ok(TransferOutcome::Completed {
        bytes: size,
        resumed_from,
    })
}

/// Uploads `local` to `remote` using a remote partial file and rename.
pub async fn upload(
    sftp: &Sftp,
    local: &Path,
    remote: &str,
    opts: &TransferOpts,
    progress: &mut (dyn FnMut(Progress) + Send),
    cancel: &Cancel,
) -> Result<TransferOutcome, TransferError> {
    let chunk = checked_chunk(opts)?;
    let local = fs::canonicalize(local).await?;
    let metadata = fs::metadata(&local).await?;
    if !metadata.is_file() {
        return Err(TransferError::NotAFile);
    }
    let size = metadata.len();
    let mtime = metadata_mtime(&metadata)?;
    let local_path = local.to_string_lossy();

    let remote_exists = match sftp.lstat(remote).await {
        Ok(_) => true,
        Err(SftpError::NotFound) => false,
        Err(error) => return Err(TransferError::Sftp(error)),
    };
    if remote_exists && !opts.overwrite {
        return Err(TransferError::AlreadyExists);
    }

    let tag = fingerprint(&local_path, size, mtime);
    let partial = format!("{remote}{PARTIAL_INFIX}{tag}");
    let siblings = remote_partial_siblings(sftp, remote).await?;
    let exists = match sftp.lstat(&partial).await {
        Ok(entry) => Some(entry),
        Err(SftpError::NotFound) => None,
        Err(error) => return Err(TransferError::Sftp(error)),
    };
    remove_remote_partials(sftp, &siblings, &partial).await?;

    let had_partial = exists.is_some();
    let partial_len = exists.as_ref().and_then(|entry| entry.size).unwrap_or(0);
    let resumed_from = if partial_len > size {
        0
    } else {
        resume_offset(partial_len, chunk)
    };
    if resumed_from == 0 && had_partial && partial_len > 0 {
        // The previous partial may extend past the data we are about to
        // rewrite or use a different alignment. Start from an empty file so
        // no stale tail survives and every restart begins at byte zero.
        remove_remote_partial(sftp, &partial).await?;
    }

    let mut file = File::open(&local).await?;
    file.seek(io::SeekFrom::Start(resumed_from)).await?;
    let mut remote_file = match sftp.open_write_resume(&partial, chunk, UPLOAD_WINDOW).await {
        Ok(file) => file,
        Err(error) => return Err(TransferError::Sftp(error)),
    };
    progress(Progress {
        done: resumed_from,
        total: size,
    });

    let transfer: Result<bool, TransferError> = async {
        // `sent` tracks bytes handed to the pipelined writer; `done` advances
        // only after a flush drains all outstanding acknowledgements.
        let mut done = resumed_from;
        let mut sent = resumed_from;
        let window_bytes = (UPLOAD_WINDOW * chunk) as u64;
        let mut next_flush = (resumed_from / chunk as u64 + UPLOAD_WINDOW as u64) * chunk as u64;
        let mut buffer = vec![0; chunk];
        while done < size {
            if cancel.is_cancelled() {
                return Ok(false);
            }
            if local_changed(&local, size, mtime).await? {
                return Err(TransferError::LocalChanged);
            }
            let want = chunk.min((size - sent) as usize);
            let count = file.read(&mut buffer[..want]).await?;
            if count == 0 {
                return Err(TransferError::SizeMismatch {
                    expected: size,
                    got: sent,
                });
            }
            remote_file.write_chunk(sent, &buffer[..count]).await?;
            sent += count as u64;
            if sent >= next_flush || sent == size {
                remote_file.flush().await?;
                done = sent;
                progress(Progress { done, total: size });
                next_flush = done.saturating_add(window_bytes);
            }
        }
        Ok(!cancel.is_cancelled())
    }
    .await;
    // The writer only enqueues writes while polled above; close drains pending
    // acknowledgements and consumes it before any outcome is returned. Errors
    // abandon remaining acknowledgements when the handle is dropped, without
    // spawning a task that could enqueue more writes after return.
    let closed = remote_file.close().await;
    if !transfer? {
        remove_remote_partial(sftp, &partial).await?;
        return Ok(TransferOutcome::Cancelled);
    }
    closed?;

    let partial_stat = sftp.lstat(&partial).await?;
    let uploaded = source_size(&partial_stat)?;
    if uploaded != size {
        return Err(TransferError::SizeMismatch {
            expected: size,
            got: uploaded,
        });
    }
    if let Err(error) = sftp.set_mtime(&partial, mtime).await {
        tracing::debug!(%error, "remote server rejected setting upload mtime");
    }

    let renamed = match sftp.posix_rename(&partial, remote).await {
        Ok(renamed) => renamed,
        Err(error) => {
            tracing::debug!(%error, "posix-rename probe failed; using portable fallback");
            false
        }
    };
    if !renamed {
        // russh-sftp 3.0.1 does not expose posix-rename@openssh.com itself, so
        // use a raw extension request and fall back when the server does not
        // implement it. The fallback removes an existing target first, which is
        // non-atomic; without overwrite, rename remains the no-clobber action.
        if opts.overwrite && remote_exists {
            remove_remote_file_if_present(sftp, remote).await?;
        }
        if let Err(error) = sftp.rename(&partial, remote).await {
            return Err(TransferError::Sftp(error));
        }
    }

    Ok(TransferOutcome::Completed {
        bytes: size,
        resumed_from,
    })
}

/// Remove every partial download that belongs to `local` (any fingerprint).
pub async fn discard_download_partials(local: &Path) -> Result<usize, TransferError> {
    match fs::symlink_metadata(local).await {
        Ok(metadata) if metadata.is_dir() => return Ok(0),
        Ok(_) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => return Err(TransferError::Io(error)),
    }
    let siblings = match partial_siblings(local).await {
        Ok(siblings) => siblings,
        Err(TransferError::Io(error)) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(error),
    };
    let prefix = format!(
        "{}{PARTIAL_INFIX}",
        local
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?
            .to_string_lossy()
    );

    let mut removed = 0;
    for path in siblings {
        let Some(name) = path.file_name() else {
            continue;
        };
        if !is_partial_name(&name.to_string_lossy(), &prefix) {
            continue;
        }
        match fs::symlink_metadata(&path).await {
            Ok(metadata) if !metadata.is_file() => continue,
            Ok(_) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => continue,
            Err(error) => return Err(TransferError::Io(error)),
        }
        match fs::remove_file(&path).await {
            Ok(()) => removed += 1,
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => return Err(TransferError::Io(error)),
        }
    }
    Ok(removed)
}

/// Remove every partial upload that belongs to `remote` on the server.
pub async fn discard_upload_partials(sftp: &Sftp, remote: &str) -> Result<usize, TransferError> {
    let siblings = match remote_partial_siblings(sftp, remote).await {
        Ok(siblings) => siblings,
        Err(TransferError::Sftp(SftpError::NotFound)) => return Ok(0),
        Err(error) => return Err(error),
    };

    let mut removed = 0;
    for path in siblings {
        match sftp.remove_file(&path).await {
            Ok(()) => removed += 1,
            Err(SftpError::NotFound) => {}
            Err(error) => return Err(TransferError::Sftp(error)),
        }
    }
    Ok(removed)
}

fn fingerprint(path: &str, size: u64, mtime: u64) -> String {
    let mut hash = 0xcbf2_9ce4_8422_2325_u64;
    for byte in path
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

fn checked_chunk(opts: &TransferOpts) -> Result<usize, io::Error> {
    if opts.chunk == 0 {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "transfer chunk must be positive",
        ));
    }
    Ok(opts.chunk)
}

fn resume_offset(partial_len: u64, chunk: usize) -> u64 {
    let window = UPLOAD_WINDOW as u64 * chunk as u64;
    partial_len.saturating_sub(window) / chunk as u64 * chunk as u64
}

fn is_file(entry: DirEntry) -> Result<bool, TransferError> {
    match entry.kind {
        EntryKind::File => Ok(true),
        EntryKind::Dir | EntryKind::Symlink | EntryKind::Other => Ok(false),
    }
}

fn source_size(entry: &DirEntry) -> Result<u64, TransferError> {
    entry.size.ok_or_else(|| {
        TransferError::Sftp(SftpError::Protocol(
            "server did not return a file size".to_owned(),
        ))
    })
}

fn source_mtime(entry: &DirEntry) -> Result<u64, TransferError> {
    entry.mtime.ok_or_else(|| {
        TransferError::Sftp(SftpError::Protocol(
            "server did not return an mtime".to_owned(),
        ))
    })
}

fn metadata_mtime(metadata: &std::fs::Metadata) -> Result<u64, TransferError> {
    Ok(metadata
        .modified()?
        .duration_since(UNIX_EPOCH)
        .unwrap_or(Duration::ZERO)
        .as_secs())
}

fn unix_time(seconds: u64) -> Result<std::time::SystemTime, TransferError> {
    UNIX_EPOCH
        .checked_add(Duration::from_secs(seconds))
        .ok_or_else(|| {
            TransferError::Sftp(SftpError::Protocol(
                "remote mtime is outside the supported range".to_owned(),
            ))
        })
}

fn partial_path(local: &Path, tag: &str) -> Result<PathBuf, TransferError> {
    let name = local.file_name().ok_or_else(|| {
        TransferError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "local destination has no file name",
        ))
    })?;
    let mut partial_name = name.to_os_string();
    partial_name.push(format!("{PARTIAL_INFIX}{tag}"));
    Ok(local.with_file_name(partial_name))
}

async fn partial_siblings(local: &Path) -> Result<Vec<PathBuf>, TransferError> {
    let parent = local.parent().unwrap_or_else(|| Path::new("."));
    let prefix = format!(
        "{}{PARTIAL_INFIX}",
        local
            .file_name()
            .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "missing file name"))?
            .to_string_lossy()
    );
    let mut siblings = Vec::new();
    let mut entries = fs::read_dir(parent).await?;
    while let Some(entry) = entries.next_entry().await? {
        let name = entry.file_name();
        if name.to_string_lossy().starts_with(&prefix) {
            siblings.push(entry.path());
        }
    }
    Ok(siblings)
}

async fn remote_partial_siblings(sftp: &Sftp, remote: &str) -> Result<Vec<String>, TransferError> {
    let (parent, prefix) = split_remote(remote)?;
    let entries = sftp.list(parent).await?;
    Ok(entries
        .into_iter()
        .filter(|entry| is_partial_name(&entry.name, &prefix))
        .map(|entry| join_remote(parent, &entry.name))
        .collect())
}

fn is_partial_name(name: &str, prefix: &str) -> bool {
    let Some(tag) = name.strip_prefix(prefix) else {
        return false;
    };
    tag.len() == 8 && tag.bytes().all(|byte| byte.is_ascii_hexdigit())
}

fn split_remote(remote: &str) -> Result<(&str, String), TransferError> {
    let (parent, name) = match remote.rsplit_once('/') {
        Some(("", name)) => ("/", name),
        Some((parent, name)) => (parent, name),
        None => (".", remote),
    };
    if name.is_empty() {
        return Err(TransferError::NotAFile);
    }
    Ok((parent, format!("{name}{PARTIAL_INFIX}")))
}

fn join_remote(parent: &str, name: &str) -> String {
    if parent == "/" {
        format!("/{name}")
    } else {
        format!("{parent}/{name}")
    }
}

async fn remove_partials_except(all: &[PathBuf], keep: &Path) -> Result<(), TransferError> {
    for path in all {
        if path != keep {
            if let Err(error) = fs::remove_file(path).await {
                if error.kind() != io::ErrorKind::NotFound {
                    return Err(TransferError::Io(error));
                }
            }
        }
    }
    Ok(())
}

async fn remove_remote_partials(
    sftp: &Sftp,
    all: &[String],
    keep: &str,
) -> Result<(), TransferError> {
    for path in all {
        if path != keep {
            remove_remote_file_if_present(sftp, path).await?;
        }
    }
    Ok(())
}

async fn remove_remote_file_if_present(sftp: &Sftp, path: &str) -> Result<(), TransferError> {
    match sftp.remove_file(path).await {
        Ok(()) | Err(SftpError::NotFound) => Ok(()),
        Err(error) => Err(TransferError::Sftp(error)),
    }
}

async fn remove_remote_partial(sftp: &Sftp, path: &str) -> Result<(), TransferError> {
    remove_remote_file_if_present(sftp, path).await
}

async fn local_len_if_present(path: &Path) -> Result<u64, TransferError> {
    match fs::metadata(path).await {
        Ok(metadata) if metadata.is_file() => Ok(metadata.len()),
        Ok(_) => Err(TransferError::NotAFile),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(0),
        Err(error) => Err(TransferError::Io(error)),
    }
}

async fn open_local_partial(path: &Path, offset: u64) -> io::Result<File> {
    let mut options = fs::OpenOptions::new();
    options.create(true).write(true);
    if offset == 0 {
        options.truncate(true);
    } else {
        options.append(true);
    }
    options.open(path).await
}

async fn local_changed(path: &Path, size: u64, mtime: u64) -> Result<bool, TransferError> {
    let metadata = match fs::metadata(path).await {
        Ok(metadata) => metadata,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(true),
        Err(error) => return Err(TransferError::Io(error)),
    };
    Ok(!metadata.is_file() || metadata.len() != size || metadata_mtime(&metadata)? != mtime)
}

async fn keep_or_remove_partial<E>(path: &Path, error: &E) {
    let _ = error;
    tracing::debug!(path = %path.display(), "keeping partial transfer");
}

async fn remove_local_partial(path: &Path) {
    match fs::remove_file(path).await {
        Ok(()) => {}
        Err(error) if error.kind() == io::ErrorKind::NotFound => {}
        Err(error) => tracing::debug!(path = %path.display(), %error, "failed to remove partial"),
    }
}

#[cfg(test)]
mod tests {
    use super::fingerprint;

    #[test]
    fn fingerprint_is_stable_and_changes_with_each_field() {
        assert_eq!(
            fingerprint("/home/test/blob", 22, 1_700_000_000),
            "27644a00"
        );
        assert_eq!(fingerprint("/a", 123, 456), "6067fcbd");
        assert_ne!(fingerprint("/a", 123, 456), fingerprint("/b", 123, 456));
        assert_ne!(fingerprint("/a", 123, 456), fingerprint("/a", 124, 456));
        assert_ne!(fingerprint("/a", 123, 456), fingerprint("/a", 123, 457));
    }
}
