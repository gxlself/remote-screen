//! SFTP browsing over an authenticated SSH transport.

use std::{io, sync::Arc};

use russh_sftp::{
    client::{fs::File as SftpHandle, Config as SftpConfig, RawSftpSession, SftpSession},
    protocol::{FileAttributes, FileType, OpenFlags, Packet, StatusCode},
};
use thiserror::Error;
use tokio::io::{AsyncReadExt as _, AsyncSeekExt as _, AsyncWriteExt as _};

use crate::{Connection, SshError, SshSession};

#[derive(Clone, Copy, Debug, PartialEq, Eq)]
pub enum EntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct DirEntry {
    /// A non-UTF-8 server path component is represented lossily.
    pub name: String,
    pub kind: EntryKind,
    pub size: Option<u64>,
    pub mtime: Option<u64>,
    pub permissions: Option<u32>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SftpError {
    #[error("SFTP subsystem is unavailable")]
    Unavailable,
    #[error("no such file or directory")]
    NotFound,
    #[error("permission denied")]
    PermissionDenied,
    #[error("file or directory already exists")]
    AlreadyExists,
    #[error("directory is not empty")]
    NotEmpty,
    #[error("SFTP failure: {0}")]
    Failure(String),
    #[error("SFTP protocol error: {0}")]
    Protocol(String),
    #[error(transparent)]
    Ssh(#[from] SshError),
}

/// An open SFTP subsystem. The transport stays alive for as long as this value.
pub struct Sftp {
    connection: Arc<Connection>,
    session: SftpSession,
}

impl SshSession {
    pub async fn sftp(&self) -> Result<Sftp, SftpError> {
        let connection = self.connection.clone();
        connection.shared.check()?;
        let session = tokio::time::timeout(
            connection.options.request_timeout,
            open_subsystem(&connection),
        )
        .await
        .map_err(|_| SshError::Timeout)??;
        connection.shared.check()?;
        tracing::debug!("SFTP subsystem opened");
        Ok(Sftp {
            connection,
            session,
        })
    }
}

async fn open_subsystem(connection: &Connection) -> Result<SftpSession, SftpError> {
    open_subsystem_with_config(connection, SftpConfig::default()).await
}

async fn open_subsystem_with_config(
    connection: &Connection,
    config: SftpConfig,
) -> Result<SftpSession, SftpError> {
    let stream = open_subsystem_stream(connection).await?;
    // Keep russh-sftp's pipelined defaults. Transfers recover conservatively
    // from the bytes that actually reached disk rather than limiting the
    // throughput of every SFTP session to one round trip per chunk.
    SftpSession::new_with_config(stream, config)
        .await
        .map_err(|error| map_client_error(connection, error))
}

async fn open_subsystem_stream(
    connection: &Connection,
) -> Result<russh::ChannelStream<russh::client::Msg>, SftpError> {
    let mut channel = connection
        .handle
        .channel_open_session()
        .await
        .map_err(SshError::from)?;
    channel
        .request_subsystem(true, "sftp")
        .await
        .map_err(SshError::from)?;
    let mut pending = std::collections::VecDeque::new();
    crate::pty::request_success(&mut channel, &mut pending)
        .await
        .map_err(|error| match error {
            SshError::RequestRejected => SftpError::Unavailable,
            error => SftpError::Ssh(error),
        })?;
    Ok(channel.into_stream())
}

impl Sftp {
    pub async fn home(&self) -> Result<String, SftpError> {
        let path = self.run("home", self.session.canonicalize(".")).await?;
        tracing::debug!(path, "SFTP home resolved");
        Ok(path)
    }

    pub async fn canonicalize(&self, path: &str) -> Result<String, SftpError> {
        let resolved = self
            .run("canonicalize", self.session.canonicalize(path))
            .await?;
        tracing::debug!(path, resolved, "SFTP canonicalize succeeded");
        Ok(resolved)
    }

    pub async fn list(&self, path: &str) -> Result<Vec<DirEntry>, SftpError> {
        let entries = self.run("list", self.session.read_dir(path)).await?;
        let mut entries: Vec<DirEntry> = entries
            .map(|entry| {
                let metadata = entry.metadata();
                attributes_to_entry(entry.file_name(), &metadata)
            })
            .collect();
        entries.sort_by(|left, right| {
            entry_rank(left.kind)
                .cmp(&entry_rank(right.kind))
                .then_with(|| left.name.to_lowercase().cmp(&right.name.to_lowercase()))
                .then_with(|| left.name.cmp(&right.name))
        });
        Ok(entries)
    }

    pub async fn stat(&self, path: &str) -> Result<DirEntry, SftpError> {
        let metadata = self.run("stat", self.session.metadata(path)).await?;
        tracing::debug!(path, "SFTP stat succeeded");
        Ok(attributes_to_entry(last_component(path), &metadata))
    }

    pub async fn lstat(&self, path: &str) -> Result<DirEntry, SftpError> {
        let metadata = self
            .run("lstat", self.session.symlink_metadata(path))
            .await?;
        tracing::debug!(path, "SFTP lstat succeeded");
        Ok(attributes_to_entry(last_component(path), &metadata))
    }

    pub async fn mkdir(&self, path: &str) -> Result<(), SftpError> {
        match self.run("mkdir", self.session.create_dir(path)).await {
            Ok(()) => Ok(()),
            Err(SftpError::Failure(message)) => {
                // SFTP v3 servers commonly report EEXIST as FAILURE.
                match self.lstat(path).await {
                    Ok(_) => Err(SftpError::AlreadyExists),
                    Err(SftpError::NotFound) => Err(SftpError::Failure(message)),
                    Err(error) => Err(error),
                }
            }
            Err(error) => Err(error),
        }
    }

    pub async fn rename(&self, from: &str, to: &str) -> Result<(), SftpError> {
        self.run("rename", self.session.rename(from, to)).await
    }

    pub async fn remove_file(&self, path: &str) -> Result<(), SftpError> {
        self.run("remove_file", self.session.remove_file(path))
            .await
    }

    pub async fn remove_dir(&self, path: &str) -> Result<(), SftpError> {
        match self.run("remove_dir", self.session.remove_dir(path)).await {
            Ok(()) => Ok(()),
            Err(SftpError::Failure(message)) => match self.list(path).await {
                Ok(entries) if entries.is_empty() => Err(SftpError::Failure(message)),
                Ok(_) => Err(SftpError::NotEmpty),
                Err(error) => Err(error),
            },
            Err(error) => Err(error),
        }
    }

    pub async fn close(self) {
        let _ = self.session.close().await;
        tracing::debug!("SFTP subsystem closed");
    }

    pub(crate) async fn open_read(
        &self,
        path: &str,
        chunk: usize,
    ) -> Result<SftpFile<'_>, SftpError> {
        let session = self
            .open_transfer_session(transfer_read_config(chunk))
            .await?;
        self.open_handle_in(&session, path, OpenFlags::READ).await
    }

    /// Opens a remote file for offset writes without truncating it, so an
    /// interrupted upload can resume from the handle's existing length.
    pub(crate) async fn open_write_resume(
        &self,
        path: &str,
        chunk: usize,
        max_in_flight: usize,
    ) -> Result<SftpFile<'_>, SftpError> {
        let session = self
            .open_transfer_session(transfer_write_config(chunk, max_in_flight))
            .await?;
        self.open_handle_in(&session, path, OpenFlags::WRITE | OpenFlags::CREATE)
            .await
    }

    pub(crate) async fn set_mtime(&self, path: &str, mtime: u64) -> Result<(), SftpError> {
        let Ok(mtime) = u32::try_from(mtime) else {
            return Err(SftpError::Failure(
                "mtime is outside the SFTP v3 range".into(),
            ));
        };
        let attributes = FileAttributes {
            mtime: Some(mtime),
            ..FileAttributes::empty()
        };
        self.run("setstat", self.session.set_metadata(path, attributes))
            .await
    }

    /// Returns whether the server accepted `posix-rename@openssh.com` for this
    /// pair of paths. `false` means the extension was not advertised or the
    /// server reported it unsupported; the caller must use the portable path.
    pub(crate) async fn posix_rename(&self, from: &str, to: &str) -> Result<bool, SftpError> {
        self.check()?;
        let stream = open_subsystem_stream(&self.connection).await?;
        let raw = RawSftpSession::new(stream);
        let version = raw
            .init()
            .await
            .map_err(|error| map_client_error(&self.connection, error))?;
        if !version.extensions.contains_key("posix-rename@openssh.com") {
            let _ = raw.close_session();
            return Ok(false);
        }

        let mut payload = Vec::new();
        for path in [from, to] {
            let len = u32::try_from(path.len()).map_err(|_| {
                SftpError::Protocol("path is too long for the SFTP rename extension".to_owned())
            })?;
            payload.extend_from_slice(&len.to_be_bytes());
            payload.extend_from_slice(path.as_bytes());
        }
        let result = raw.extended("posix-rename@openssh.com", payload).await;
        let _ = raw.close_session();
        let result = result.map_err(|error| map_client_error(&self.connection, error))?;
        match result {
            Packet::Status(status) if status.status_code == StatusCode::Ok => Ok(true),
            Packet::Status(status) if status.status_code == StatusCode::OpUnsupported => Ok(false),
            Packet::Status(status) => Err(SftpError::Failure(status.error_message)),
            _ => Err(SftpError::Protocol(
                "unexpected posix-rename response".to_owned(),
            )),
        }
    }

    async fn open_handle_in(
        &self,
        session: &SftpSession,
        path: &str,
        flags: OpenFlags,
    ) -> Result<SftpFile<'_>, SftpError> {
        let file = tokio::time::timeout(
            self.connection.options.request_timeout,
            session.open_with_flags(path, flags),
        )
        .await
        .map_err(|_| SftpError::Ssh(SshError::Timeout))?
        .map_err(|error| map_client_error(&self.connection, error))?;
        Ok(SftpFile { sftp: self, file })
    }

    async fn open_transfer_session(&self, config: SftpConfig) -> Result<SftpSession, SftpError> {
        self.check()?;
        tokio::time::timeout(
            self.connection.options.request_timeout,
            open_subsystem_with_config(&self.connection, config),
        )
        .await
        .map_err(|_| SftpError::Ssh(SshError::Timeout))?
    }

    fn map_file_error(&self, error: io::Error) -> SftpError {
        self.check().err().unwrap_or_else(|| match error.kind() {
            io::ErrorKind::NotFound => SftpError::NotFound,
            io::ErrorKind::PermissionDenied => SftpError::PermissionDenied,
            io::ErrorKind::AlreadyExists => SftpError::AlreadyExists,
            io::ErrorKind::TimedOut => SftpError::Ssh(SshError::Timeout),
            _ => SftpError::Protocol(error.to_string()),
        })
    }

    fn check(&self) -> Result<(), SftpError> {
        self.connection.shared.check()?;
        Ok(())
    }

    async fn run<T>(
        &self,
        operation: &'static str,
        future: impl std::future::Future<Output = Result<T, russh_sftp::client::error::Error>>,
    ) -> Result<T, SftpError> {
        self.check()?;
        let result = tokio::time::timeout(self.connection.options.request_timeout, future)
            .await
            .map_err(|_| SftpError::Ssh(SshError::Timeout))
            .and_then(|result| result.map_err(|error| map_client_error(&self.connection, error)));
        match result {
            Ok(value) => {
                tracing::debug!(operation, "SFTP operation succeeded");
                Ok(value)
            }
            Err(error) => {
                tracing::debug!(operation, "SFTP operation failed");
                Err(error)
            }
        }
    }
}

/// A remote file handle used by `sftp_transfer` for bounded, sequential chunks.
/// Offsets are explicit so an interrupted transfer never depends on the prior
/// SFTP session's handle state.
pub(crate) struct SftpFile<'a> {
    sftp: &'a Sftp,
    file: SftpHandle,
}

impl SftpFile<'_> {
    pub(crate) async fn seek(&mut self, offset: u64) -> Result<u64, SftpError> {
        self.sftp.check()?;
        let timeout = self.sftp.connection.options.request_timeout;
        tokio::time::timeout(timeout, self.file.seek(io::SeekFrom::Start(offset)))
            .await
            .map_err(|_| SftpError::Ssh(SshError::Timeout))?
            .map_err(|error| self.sftp.map_file_error(error))
    }

    pub(crate) async fn read_exact(&mut self, len: usize) -> Result<Vec<u8>, SftpError> {
        self.sftp.check()?;
        let file = &mut self.file;
        let timeout = self.sftp.connection.options.request_timeout;
        let mut data = vec![0; len];
        let result = tokio::time::timeout(timeout, file.read_exact(&mut data))
            .await
            .map_err(|_| SftpError::Ssh(SshError::Timeout))?;
        match result {
            Ok(_) => Ok(data),
            Err(error) if error.kind() == io::ErrorKind::UnexpectedEof => Ok(Vec::new()),
            Err(error) => Err(self.sftp.map_file_error(error)),
        }
    }

    pub(crate) async fn write_chunk(&mut self, offset: u64, data: &[u8]) -> Result<(), SftpError> {
        self.sftp.check()?;
        let file = &mut self.file;
        let timeout = self.sftp.connection.options.request_timeout;
        tokio::time::timeout(timeout, async move {
            file.seek(io::SeekFrom::Start(offset)).await?;
            file.write_all(data).await
        })
        .await
        .map_err(|_| SftpError::Ssh(SshError::Timeout))?
        .map_err(|error| self.sftp.map_file_error(error))
    }

    pub(crate) async fn flush(&mut self) -> Result<(), SftpError> {
        self.sftp.check()?;
        let file = &mut self.file;
        let timeout = self.sftp.connection.options.request_timeout;
        tokio::time::timeout(timeout, file.flush())
            .await
            .map_err(|_| SftpError::Ssh(SshError::Timeout))?
            .map_err(|error| self.sftp.map_file_error(error))
    }

    pub(crate) async fn close(self) -> Result<(), SftpError> {
        self.sftp.check()?;
        let timeout = self.sftp.connection.options.request_timeout;
        tokio::time::timeout(timeout, self.file.close())
            .await
            .map_err(|_| SftpError::Ssh(SshError::Timeout))?
            .map_err(|error| self.sftp.map_file_error(error))
    }
}

fn attributes_to_entry(name: String, attributes: &FileAttributes) -> DirEntry {
    DirEntry {
        name,
        kind: entry_kind(attributes.file_type()),
        size: attributes.size,
        mtime: attributes.mtime.map(u64::from),
        permissions: attributes.permissions,
    }
}

fn entry_kind(kind: FileType) -> EntryKind {
    match kind {
        FileType::File => EntryKind::File,
        FileType::Dir => EntryKind::Dir,
        FileType::Symlink => EntryKind::Symlink,
        FileType::Other => EntryKind::Other,
    }
}

fn entry_rank(kind: EntryKind) -> u8 {
    match kind {
        EntryKind::Dir => 0,
        _ => 1,
    }
}

fn transfer_read_config(chunk: usize) -> SftpConfig {
    let default = SftpConfig::default();
    // A data packet consists of its 4-byte length, type, request id, data
    // length, and chunk bytes (13 bytes total of framing).
    let Some(packet_len) = u32::try_from(chunk)
        .ok()
        .and_then(|chunk| chunk.checked_add(13))
    else {
        return default;
    };
    SftpConfig {
        max_packet_len: packet_len,
        ..default
    }
}

fn transfer_write_config(chunk: usize, max_in_flight: usize) -> SftpConfig {
    let default = SftpConfig::default();
    // Leave room for the write framing and variable-length handle so one
    // application chunk maps to one write request rather than a split write
    // that would consume two slots in the client's acknowledgement window.
    let Some(packet_len) = u32::try_from(chunk)
        .ok()
        .and_then(|chunk| chunk.checked_add(1024))
    else {
        return default;
    };
    SftpConfig {
        max_packet_len: packet_len,
        max_write_packet_len: packet_len,
        max_concurrent_writes: max_in_flight.max(1),
        ..default
    }
}

fn last_component(path: &str) -> String {
    path.rsplit('/')
        .find(|component| !component.is_empty())
        .unwrap_or(path)
        .to_owned()
}

fn map_client_error(connection: &Connection, error: russh_sftp::client::error::Error) -> SftpError {
    use russh_sftp::client::error::Error as ClientError;

    match error {
        ClientError::Status(status) => match status.status_code {
            StatusCode::NoSuchFile => SftpError::NotFound,
            StatusCode::PermissionDenied => SftpError::PermissionDenied,
            StatusCode::Failure | StatusCode::OpUnsupported => {
                SftpError::Failure(status.error_message)
            }
            StatusCode::NoConnection | StatusCode::ConnectionLost => {
                match connection.shared.check() {
                    Err(error) => SftpError::Ssh(error),
                    Ok(()) => SftpError::Ssh(SshError::Closed),
                }
            }
            StatusCode::BadMessage => SftpError::Protocol(status.error_message),
            StatusCode::Ok | StatusCode::Eof => SftpError::Protocol(status.error_message),
        },
        ClientError::IO(message) => match connection.shared.check() {
            Err(error) => SftpError::Ssh(error),
            Ok(()) => SftpError::Ssh(SshError::Transport(message)),
        },
        ClientError::Timeout => SftpError::Ssh(SshError::Timeout),
        ClientError::Limited(message) => SftpError::Protocol(message),
        ClientError::UnexpectedPacket => SftpError::Protocol("unexpected SFTP packet".to_owned()),
        ClientError::UnexpectedBehavior(message) => match connection.shared.check() {
            Err(error) => SftpError::Ssh(error),
            Ok(()) => SftpError::Protocol(message),
        },
    }
}
