//! 收到的文件名清洗与同名改名。
//!
//! The helpers in this module are intentionally I/O-free. They turn a peer
//! supplied [`FileHeader.name`](crate::pb::FileHeader) into one component that
//! can safely be joined to a download directory, and provide names for collision
//! candidates and resumable partial downloads.
//!
//! Unicode normalization is deliberately not performed: the protocol does not
//! require a particular normalization form, and keeping code points unchanged
//! avoids surprising the sender.

use thiserror::Error;

/// Maximum size of one file name component, in UTF-8 bytes.
pub const MAX_NAME_BYTES: usize = 255;

/// Number of collision candidates tried before the caller should report an
/// error. Candidate numbers therefore range from `0` through `999`.
pub const MAX_CANDIDATES: u32 = 1000;

const MAX_EXTENSION_BYTES: usize = 16;

const WINDOWS_RESERVED_NAMES: [&str; 22] = [
    "CON", "PRN", "AUX", "NUL", "COM1", "COM2", "COM3", "COM4", "COM5", "COM6", "COM7", "COM8",
    "COM9", "LPT1", "LPT2", "LPT3", "LPT4", "LPT5", "LPT6", "LPT7", "LPT8", "LPT9",
];

/// Error returned when a transfer id cannot be used in a partial file name.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Error)]
pub enum NameError {
    /// Transfer ids must be exactly 32 lowercase hexadecimal characters.
    #[error("transfer id must be 32 lowercase hexadecimal characters")]
    InvalidTransferId,
}

/// The two files used to make a resumable download.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PartialKind {
    /// The partially received file data (`.rs-<id>.part`).
    Data,
    /// The sidecar metadata file (`.rs-<id>.json`).
    Meta,
}

/// Turn a peer-supplied name into one safe file name component.
///
/// Slashes (both kinds) are treated as separators and only the last non-empty
/// component is retained. Invisible control and bidirectional-override
/// characters are removed, characters reserved by Windows are replaced with
/// `_`, and the result is prevented from being hidden or a reserved device
/// name. Overlong names are truncated at a UTF-8 character boundary while
/// retaining a short extension when possible.
pub fn sanitize(name: &str) -> String {
    let component = name
        .rsplit(['/', '\\'])
        .find(|part| !part.is_empty())
        .unwrap_or_default();

    let mut cleaned = String::with_capacity(component.len());
    for character in component.chars() {
        if is_invisible(character) {
            continue;
        }
        cleaned.push(match character {
            '<' | '>' | ':' | '"' | '|' | '?' | '*' => '_',
            other => other,
        });
    }

    let cleaned = trim_name(&cleaned);
    let mut cleaned = if cleaned.starts_with('.') {
        format!("_{cleaned}")
    } else {
        cleaned.to_owned()
    };

    if is_windows_reserved(primary_name(&cleaned)) {
        cleaned.insert(0, '_');
    }

    if cleaned.len() > MAX_NAME_BYTES {
        cleaned = truncate_sanitized(&cleaned);
    }

    if cleaned.is_empty() {
        "file".to_owned()
    } else {
        cleaned
    }
}

/// Build collision candidate `n` from an already sanitized name.
///
/// `n == 0` returns the name unchanged. For larger values ` (n)` is inserted
/// before the last dot, or appended when there is no such dot. The result is
/// truncated at a UTF-8 boundary when the insertion makes it too long.
pub fn candidate(sanitized: &str, n: u32) -> String {
    if n == 0 {
        return sanitized.to_owned();
    }

    let marker = format!(" ({n})");
    let result = match sanitized.rfind('.') {
        Some(dot) if dot > 0 => {
            let stem = &sanitized[..dot];
            let extension = &sanitized[dot..];
            if sanitized.len() + marker.len() <= MAX_NAME_BYTES {
                format!("{stem}{marker}{extension}")
            } else {
                let stem_budget = MAX_NAME_BYTES.saturating_sub(marker.len() + extension.len());
                let stem = truncate_utf8(stem, stem_budget);
                format!("{stem}{marker}{extension}")
            }
        }
        _ => {
            if sanitized.len() + marker.len() <= MAX_NAME_BYTES {
                format!("{sanitized}{marker}")
            } else {
                let prefix = truncate_utf8(sanitized, MAX_NAME_BYTES.saturating_sub(marker.len()));
                format!("{prefix}{marker}")
            }
        }
    };

    if result.len() <= MAX_NAME_BYTES {
        result
    } else {
        truncate_utf8(&result, MAX_NAME_BYTES).to_owned()
    }
}

/// Name of the partial data file for a resumable transfer.
pub fn partial_name(transfer_id: &str) -> Result<String, NameError> {
    if !valid_transfer_id(transfer_id) {
        return Err(NameError::InvalidTransferId);
    }
    Ok(format!(".rs-{transfer_id}.part"))
}

/// Name of the metadata sidecar for a resumable transfer.
pub fn partial_meta_name(transfer_id: &str) -> Result<String, NameError> {
    if !valid_transfer_id(transfer_id) {
        return Err(NameError::InvalidTransferId);
    }
    Ok(format!(".rs-{transfer_id}.json"))
}

/// Recognise the two partial-file names generated by this module.
pub fn parse_partial_name(file_name: &str) -> Option<(String, PartialKind)> {
    let (transfer_id, kind) = if let Some(transfer_id) = file_name
        .strip_prefix(".rs-")
        .and_then(|rest| rest.strip_suffix(".part"))
    {
        (transfer_id, PartialKind::Data)
    } else if let Some(transfer_id) = file_name
        .strip_prefix(".rs-")
        .and_then(|rest| rest.strip_suffix(".json"))
    {
        (transfer_id, PartialKind::Meta)
    } else {
        return None;
    };

    valid_transfer_id(transfer_id).then(|| (transfer_id.to_owned(), kind))
}

/// Final assertion for a name that is about to be joined to a directory.
///
/// Partial download names start with a dot and are intentionally handled by
/// [`parse_partial_name`] instead.
pub fn is_safe_component(name: &str) -> bool {
    !name.is_empty()
        && name.len() <= MAX_NAME_BYTES
        && !name.contains(['/', '\\', '\0'])
        && name != "."
        && name != ".."
        && !name.starts_with('.')
}

fn is_invisible(character: char) -> bool {
    matches!(
        character,
        '\u{0000}'..='\u{001F}'
            | '\u{007F}'
            | '\u{0080}'..='\u{009F}'
            | '\u{200B}'..='\u{200F}'
            | '\u{202A}'..='\u{202E}'
            | '\u{2066}'..='\u{2069}'
            | '\u{FEFF}'
    )
}

fn trim_name(name: &str) -> &str {
    name.trim()
        .trim_end_matches(|character: char| character == '.' || character.is_whitespace())
}

fn primary_name(name: &str) -> &str {
    name.split_once('.').map_or(name, |(primary, _)| primary)
}

fn is_windows_reserved(primary: &str) -> bool {
    WINDOWS_RESERVED_NAMES
        .iter()
        .any(|reserved| primary.eq_ignore_ascii_case(reserved))
}

fn truncate_sanitized(name: &str) -> String {
    let truncated = match name.rfind('.') {
        Some(dot) if dot > 0 => {
            let stem = &name[..dot];
            let extension = &name[dot + 1..];
            if extension.len() <= MAX_EXTENSION_BYTES && !stem.is_empty() {
                let stem_budget = MAX_NAME_BYTES.saturating_sub(extension.len() + 1);
                let stem = truncate_utf8(stem, stem_budget);
                format!("{stem}.{extension}")
            } else {
                truncate_utf8(name, MAX_NAME_BYTES).to_owned()
            }
        }
        _ => truncate_utf8(name, MAX_NAME_BYTES).to_owned(),
    };

    trim_name(&truncated).to_owned()
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }

    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

// Validate transfer ids before constructing local file paths.
fn valid_transfer_id(transfer_id: &str) -> bool {
    transfer_id.len() == 32
        && transfer_id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
}
