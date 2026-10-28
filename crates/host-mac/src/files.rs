//! File channel contract: files (03 §8.2, 07).

use std::ffi::CString;
use std::io;
use std::os::unix::ffi::OsStrExt;
use std::path::Path;
use std::time::{SystemTime, UNIX_EPOCH};

/// Build the macOS quarantine attribute value used for received files.
pub fn quarantine_value(agent_name: &str, now_unix_s: u64) -> String {
    format!("0081;{:x};{};", now_unix_s, agent_name)
}

/// Mark a received file as downloaded from an external source.
pub fn quarantine(path: &Path, agent_name: &str, now_unix_s: u64) -> io::Result<()> {
    let path = CString::new(path.as_os_str().as_bytes())
        .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "path contains NUL byte"))?;
    let value = quarantine_value(agent_name, now_unix_s);
    let name = b"com.apple.quarantine\0";

    // SAFETY: `path` and `name` are NUL-terminated C strings, and `value`
    // remains alive for the duration of this call with its exact byte length.
    let result = unsafe {
        libc::setxattr(
            path.as_ptr(),
            name.as_ptr().cast(),
            value.as_ptr().cast(),
            value.len(),
            0,
            0,
        )
    };
    if result == -1 {
        Err(io::Error::last_os_error())
    } else {
        Ok(())
    }
}

pub(crate) fn unix_seconds() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map_or(0, |duration| duration.as_secs())
}
