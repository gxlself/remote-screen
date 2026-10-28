#![cfg(windows)]
//! Reads MSI deployment metadata from the 64-bit registry view. This module
//! never opens or logs the contents of a pairing file.

use host_core::platform::PlatformError;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{ERROR_FILE_NOT_FOUND, ERROR_MORE_DATA, FILETIME, WIN32_ERROR};
use windows::Win32::System::Registry::{
    RegCloseKey, RegGetValueW, RegOpenKeyExW, RegQueryInfoKeyW, HKEY, HKEY_LOCAL_MACHINE, KEY_READ,
    KEY_WOW64_64KEY, RRF_RT_REG_SZ,
};

use crate::svc_os_model::{DeployValues, DEPLOY_REG_KEY};

struct Key(HKEY);

impl Drop for Key {
    fn drop(&mut self) {
        // SAFETY: This guard solely owns the opened registry key handle.
        let _ = unsafe { RegCloseKey(self.0) };
    }
}

fn check(api: &str, status: WIN32_ERROR) -> Result<(), PlatformError> {
    status
        .ok()
        .map_err(|error| PlatformError::Platform(format!("{api}: {error}")))
}

/// Returns None if the key or PairFile value is absent, or PairFile is empty.
pub fn read_deploy() -> Result<Option<DeployValues>, PlatformError> {
    let name: Vec<_> = DEPLOY_REG_KEY.encode_utf16().chain(Some(0)).collect();
    let mut raw = HKEY::default();
    // SAFETY: The machine hive is valid, name is NUL terminated, and raw is
    // writable output. Explicit WOW64 flags select the MSI's 64-bit view.
    let status = unsafe {
        RegOpenKeyExW(
            HKEY_LOCAL_MACHINE,
            PCWSTR(name.as_ptr()),
            Some(0),
            KEY_READ | KEY_WOW64_64KEY,
            &mut raw,
        )
    };
    if status == ERROR_FILE_NOT_FOUND {
        return Ok(None);
    }
    check("RegOpenKeyExW", status)?;
    let key = Key(raw);
    let mut written = FILETIME::default();
    // SAFETY: key is owned, all unused outputs are null, and written is live
    // writable storage for the key's last-write FILETIME.
    check("RegQueryInfoKeyW", unsafe {
        RegQueryInfoKeyW(
            key.0,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            None,
            Some(&mut written),
        )
    })?;
    let Some(pair_file) = read_string(&key, w!("PairFile"))?.filter(|value| !value.is_empty())
    else {
        return Ok(None);
    };
    Ok(Some(DeployValues {
        pair_file,
        permission: read_string(&key, w!("PairFilePermission"))?,
        label: read_string(&key, w!("PairFileLabel"))?,
        stamp: (u64::from(written.dwHighDateTime) << 32) | u64::from(written.dwLowDateTime),
    }))
}

fn read_string(key: &Key, name: PCWSTR) -> Result<Option<String>, PlatformError> {
    // A registry writer can resize a value between the two reads. Retry a
    // bounded number of times and report an error if it never settles.
    for _ in 0..4 {
        let mut size = 0;
        // SAFETY: key is owned, name comes from a static w! literal, and size
        // is writable. No data buffer is supplied for this length query.
        let status = unsafe {
            RegGetValueW(
                key.0,
                PCWSTR::null(),
                name,
                RRF_RT_REG_SZ,
                None,
                None,
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        check("RegGetValueW size", status)?;
        let mut buffer = vec![0_u16; (size as usize).div_ceil(2).max(1)];
        // SAFETY: buffer is aligned UTF-16 storage with at least size bytes;
        // size is both the supplied capacity and writable returned byte count.
        let status = unsafe {
            RegGetValueW(
                key.0,
                PCWSTR::null(),
                name,
                RRF_RT_REG_SZ,
                None,
                Some(buffer.as_mut_ptr().cast()),
                Some(&mut size),
            )
        };
        if status == ERROR_FILE_NOT_FOUND {
            return Ok(None);
        }
        if status == ERROR_MORE_DATA {
            continue;
        }
        check("RegGetValueW data", status)?;
        if size % 2 != 0 || size as usize / 2 > buffer.len() {
            return Err(PlatformError::Platform(
                "RegGetValueW: invalid UTF-16 byte count".into(),
            ));
        }
        buffer.truncate(size as usize / 2);
        let end = buffer
            .iter()
            .position(|value| *value == 0)
            .unwrap_or(buffer.len());
        return String::from_utf16(&buffer[..end])
            .map(Some)
            .map_err(|_| PlatformError::Platform("RegGetValueW: invalid UTF-16 string".into()));
    }
    Err(PlatformError::Platform(
        "RegGetValueW: value changed during read".into(),
    ))
}
