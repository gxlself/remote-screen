//! Shared validation, error mapping, and pagination for file control messages.
//!
//! This module is pure: it neither performs I/O nor depends on a clock. See
//! docs/dev/03-protocol.md §8.2 for the wire contract.

use std::io;

use prost::Message;
use thiserror::Error;

use crate::file::validate_transfer_id;
use crate::file_progress::FailReason;
use crate::pb;

/// The peer rejected the stream because validation failed or the channel is
/// unavailable.
pub const STREAM_REJECTED: u32 = 1;
/// The user paused the transfer; the receiver keeps the partial file.
pub const STREAM_PAUSED: u32 = 2;
/// The user cancelled the transfer.
pub const STREAM_CANCELLED: u32 = 3;
/// The configured concurrent-transfer limit was reached.
pub const STREAM_BUSY: u32 = 4;
/// File transfer is disabled by configuration.
pub const STREAM_DISABLED: u32 = 5;
/// The peer's role or the session intent does not allow this transfer.
pub const STREAM_NOT_ALLOWED: u32 = 6;

/// Maximum number of entries in one [`pb::FileListing`].
pub const MAX_LISTING_ENTRIES: usize = 1000;
/// Maximum encoded size of one [`pb::FileListing`].
pub const MAX_LISTING_BYTES: usize = 256 * 1024;
/// Maximum UTF-8 length of a path in a file control message.
pub const MAX_PATH_BYTES: usize = 4096;
/// Maximum UTF-8 length of one file entry name.
pub const MAX_ENTRY_NAME_BYTES: usize = 1024;
/// Maximum number of concurrent outbound file streams per connection.
pub const MAX_CONCURRENT_SENDS: usize = 2;
/// Maximum number of concurrent inbound file streams per connection.
pub const MAX_CONCURRENT_RECEIVES: usize = 4;

const SHA256_BYTES: usize = 32;
const ENTRY_FRAME_RESERVE: usize = 3;
const LISTING_FRAME_RESERVE: usize = 64;
const LISTING_ENTRY_BUDGET: usize =
    MAX_LISTING_BYTES - 2 * MAX_PATH_BYTES - 2 * ENTRY_FRAME_RESERVE - LISTING_FRAME_RESERVE;

/// A malformed inbound file control message.
#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum CtlError {
    #[error("file path is empty")]
    EmptyPath,
    #[error("file path is {0} bytes; the limit is {MAX_PATH_BYTES} bytes")]
    PathTooLong(usize),
    #[error("file path contains NUL")]
    NulInPath,
    #[error("file transfer id is invalid")]
    BadTransferId,
    #[error("file SHA-256 is {0} bytes; expected empty or {SHA256_BYTES} bytes")]
    BadSha256(usize),
    #[error("file offset is set without a transfer id")]
    OffsetWithoutId,
    #[error("file offset {offset} exceeds expected size {size}")]
    OffsetBeyondSize { offset: u64, size: u64 },
}

impl CtlError {
    /// Returns the protocol error sent on the wire for this local error.
    pub fn to_pb(&self) -> pb::FileError {
        pb::FileError::Protocol
    }
}

/// Validates an inbound directory-listing request.
pub fn validate_list(m: &pb::FileList) -> Result<(), CtlError> {
    validate_path(&m.path, true)
}

/// Validates an inbound file-download request.
pub fn validate_request(m: &pb::FileRequest) -> Result<(), CtlError> {
    validate_path(&m.path, false)?;

    if !m.transfer_id.is_empty() {
        validate_transfer_id(&m.transfer_id).map_err(|_| CtlError::BadTransferId)?;
    }

    if m.offset > 0 {
        if m.transfer_id.is_empty() {
            return Err(CtlError::OffsetWithoutId);
        }
        if m.offset > m.expect_size {
            return Err(CtlError::OffsetBeyondSize {
                offset: m.offset,
                size: m.expect_size,
            });
        }
    }

    if !m.expect_sha256.is_empty() && m.expect_sha256.len() != SHA256_BYTES {
        return Err(CtlError::BadSha256(m.expect_sha256.len()));
    }

    Ok(())
}

/// Validates an inbound upload-status query.
pub fn validate_query(m: &pb::FileQuery) -> Result<(), CtlError> {
    validate_required_transfer_id(&m.transfer_id)
}

/// Validates an inbound upload-status notification or response.
pub fn validate_status(m: &pb::FileStatus) -> Result<(), CtlError> {
    validate_required_transfer_id(&m.transfer_id)
}

/// Validates an inbound transfer cancellation.
pub fn validate_cancel(m: &pb::FileCancel) -> Result<(), CtlError> {
    validate_required_transfer_id(&m.transfer_id)
}

/// Maps a local transfer failure reason to its wire representation.
pub fn fail_to_pb(r: FailReason) -> pb::FileError {
    match r {
        FailReason::Io => pb::FileError::Io,
        FailReason::Sha256Mismatch => pb::FileError::Sha256Mismatch,
        FailReason::ResumeMismatch => pb::FileError::ResumeMismatch,
        FailReason::SourceChanged => pb::FileError::SourceChanged,
        FailReason::Protocol => pb::FileError::Protocol,
        FailReason::TooLarge => pb::FileError::TooLarge,
    }
}

/// Maps a wire file error back to a local transfer failure reason.
///
/// Errors that are not stable transfer failure reasons, such as
/// [`pb::FileError::NotFound`], return `None`.
pub fn pb_to_fail(e: pb::FileError) -> Option<FailReason> {
    match e {
        pb::FileError::Io => Some(FailReason::Io),
        pb::FileError::Sha256Mismatch => Some(FailReason::Sha256Mismatch),
        pb::FileError::ResumeMismatch => Some(FailReason::ResumeMismatch),
        pb::FileError::SourceChanged => Some(FailReason::SourceChanged),
        pb::FileError::Protocol => Some(FailReason::Protocol),
        pb::FileError::TooLarge => Some(FailReason::TooLarge),
        _ => None,
    }
}

/// Maps an I/O error kind to the corresponding wire file error.
pub fn io_to_pb(e: &io::Error) -> pb::FileError {
    match e.kind() {
        io::ErrorKind::NotFound => pb::FileError::NotFound,
        io::ErrorKind::PermissionDenied => pb::FileError::PermissionDenied,
        io::ErrorKind::StorageFull => pb::FileError::NoSpace,
        io::ErrorKind::NotADirectory => pb::FileError::NotADir,
        io::ErrorKind::IsADirectory => pb::FileError::NotAFile,
        io::ErrorKind::FileTooLarge => pb::FileError::TooLarge,
        _ => pb::FileError::Io,
    }
}

/// Returns the 0x04 stream stop/reset code associated with a wire error.
pub fn stream_code(e: pb::FileError) -> u32 {
    match e {
        pb::FileError::Disabled => STREAM_DISABLED,
        pb::FileError::NotAllowed => STREAM_NOT_ALLOWED,
        pb::FileError::Busy => STREAM_BUSY,
        _ => STREAM_REJECTED,
    }
}

/// Selects one page of entries and returns it with the next page offset.
///
/// At most [`MAX_LISTING_ENTRIES`] entries are returned. Entries are added in
/// order while their encoded size plus framing fits the page budget. A single
/// entry is always returned when `offset` points into the list, even if the
/// caller supplied an entry larger than the contract allows. A zero
/// `next_offset` means the list is exhausted.
pub fn page(entries: &[pb::FileEntry], offset: u32) -> (Vec<pb::FileEntry>, u32) {
    let start = offset as usize;
    if start >= entries.len() {
        return (Vec::new(), 0);
    }

    let mut end = start;
    let mut encoded_bytes = 0usize;
    while end < entries.len() && end - start < MAX_LISTING_ENTRIES {
        let entry_bytes = entries[end]
            .encoded_len()
            .saturating_add(ENTRY_FRAME_RESERVE);
        if end > start && encoded_bytes.saturating_add(entry_bytes) > LISTING_ENTRY_BUDGET {
            break;
        }
        encoded_bytes = encoded_bytes.saturating_add(entry_bytes);
        end += 1;
    }

    let page = entries[start..end].to_vec();
    let next_offset = if end < entries.len() {
        offset.saturating_add((end - start) as u32)
    } else {
        0
    };
    (page, next_offset)
}

fn validate_path(path: &str, allow_empty: bool) -> Result<(), CtlError> {
    if path.is_empty() {
        return if allow_empty {
            Ok(())
        } else {
            Err(CtlError::EmptyPath)
        };
    }
    if path.len() > MAX_PATH_BYTES {
        return Err(CtlError::PathTooLong(path.len()));
    }
    if path.contains('\0') {
        return Err(CtlError::NulInPath);
    }
    Ok(())
}

fn validate_required_transfer_id(transfer_id: &str) -> Result<(), CtlError> {
    if transfer_id.is_empty() {
        return Err(CtlError::BadTransferId);
    }
    validate_transfer_id(transfer_id).map_err(|_| CtlError::BadTransferId)
}
