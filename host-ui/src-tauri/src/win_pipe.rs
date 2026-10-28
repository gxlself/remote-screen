#![cfg(windows)]
//! Authenticate the pipe object before giving any transport to Tokio.
//!
//! A standard user usually cannot OpenProcess/OpenProcessToken on a SYSTEM
//! service, so checking its process token is not usable here. An executable-path
//! check is insufficient: a user can run the installed rs-host.exe and squat on
//! the pipe name, fooling the tray even without access to the service's keys.
//!
//! Creating an object with a chosen owner requires a token SID marked
//! SE_GROUP_OWNER, or SeRestorePrivilege. Standard users and UAC-filtered
//! administrators cannot choose SYSTEM (SY) or Administrators (BA) as owner;
//! BA in a filtered administrator token is deny-only. With the service online,
//! UI_PIPE_SDDL in crates/host-win/src/winproc_model.rs:8 grants console users
//! 0x12018b, excluding FILE_CREATE_PIPE_INSTANCE (0x4), so they cannot add an
//! instance to its pipe. With the service offline, a squatter's pipe has the
//! squatter's own owner SID and fails the whitelist.
//!
//! Session 0 is an additional check: services run there, whereas even a suitably
//! owned pipe created in an elevated administrator's user session is rejected.
//! Elevated administrators are outside the threat model. Identification-level
//! SQOS prevents a squatter from impersonating the tray user, and no bytes are
//! read or written before both identity checks pass.

use std::io;
use std::os::windows::io::{AsRawHandle, FromRawHandle, IntoRawHandle, OwnedHandle};

use ::windows::core::{PCWSTR, PWSTR};
use ::windows::Win32::Foundation::{
    LocalFree, ERROR_GEN_FAILURE, ERROR_INVALID_PARAMETER, ERROR_INVALID_SID, HANDLE, HLOCAL,
    WIN32_ERROR,
};
use ::windows::Win32::Security::Authorization::{
    ConvertSidToStringSidW, GetSecurityInfo, SE_FILE_OBJECT, SE_KERNEL_OBJECT, SE_OBJECT_TYPE,
};
use ::windows::Win32::Security::{OWNER_SECURITY_INFORMATION, PSECURITY_DESCRIPTOR, PSID};
use ::windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_FLAG_OVERLAPPED, FILE_SHARE_MODE, OPEN_EXISTING, SECURITY_IDENTIFICATION,
    SECURITY_SQOS_PRESENT,
};
use ::windows::Win32::System::Pipes::GetNamedPipeServerSessionId;
use tokio::net::windows::named_pipe::NamedPipeClient;

use crate::pipe_trust::{check_server, valid_pipe_name, Reject, CLIENT_ACCESS};

#[derive(Debug)]
pub(crate) enum ConnectError {
    Io(io::Error),
    Untrusted(Reject),
}

struct LocalAllocation(HLOCAL);

impl Drop for LocalAllocation {
    fn drop(&mut self) {
        // SAFETY: This guard exclusively owns a LocalAlloc buffer returned by
        // GetSecurityInfo or ConvertSidToStringSidW, or null (a permitted no-op).
        unsafe { LocalFree(Some(self.0)) };
    }
}

// The bindings wrap GetLastError as HRESULT; preserve the underlying Win32
// code instead of passing the HRESULT to io::Error::from_raw_os_error.
fn win32_code(error: &::windows::core::Error) -> u32 {
    WIN32_ERROR::from_error(error).map_or(ERROR_GEN_FAILURE.0, |code| code.0)
}

fn query_owner(handle: HANDLE, object_type: SE_OBJECT_TYPE) -> Result<String, Reject> {
    let mut owner = PSID::default();
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: handle is borrowed from the live OwnedHandle in connect. The
    // output pointers are writable; owner will refer into descriptor's buffer.
    let status = unsafe {
        GetSecurityInfo(
            handle,
            object_type,
            OWNER_SECURITY_INFORMATION,
            Some(&mut owner),
            None,
            None,
            None,
            Some(&mut descriptor),
        )
    };
    let _descriptor = LocalAllocation(HLOCAL(descriptor.0));
    if status.is_err() {
        return Err(Reject::Query {
            step: "GetSecurityInfo",
            code: status.0,
        });
    }
    if owner.0.is_null() {
        return Err(Reject::Query {
            step: "GetSecurityInfo",
            code: ERROR_INVALID_SID.0,
        });
    }

    let mut sid = PWSTR::null();
    // SAFETY: owner is the SID returned by GetSecurityInfo; _descriptor remains
    // alive throughout conversion. sid is a writable output pointer.
    let converted = unsafe { ConvertSidToStringSidW(owner, &mut sid) };
    let _sid = LocalAllocation(HLOCAL(sid.0.cast()));
    converted.map_err(|error| Reject::Query {
        step: "ConvertSidToStringSidW",
        code: win32_code(&error),
    })?;
    // SAFETY: Successful ConvertSidToStringSidW returns a NUL-terminated UTF-16
    // SID string, and _sid keeps its allocation alive until copying finishes.
    unsafe { sid.to_string() }.map_err(|_| Reject::Query {
        step: "ConvertSidToStringSidW",
        code: ERROR_INVALID_SID.0,
    })
}

/// Must be polled inside a Tokio runtime with I/O enabled.
pub(crate) async fn connect(name: &str) -> Result<NamedPipeClient, ConnectError> {
    // Also defend direct BridgeConfig callers against NUL truncation or opening
    // a file/remote pipe; the environment override is validated separately.
    if !valid_pipe_name(name) {
        return Err(ConnectError::Io(io::Error::new(
            io::ErrorKind::InvalidInput,
            "invalid local pipe name",
        )));
    }
    let name: Vec<u16> = name.encode_utf16().chain(std::iter::once(0)).collect();
    // SAFETY: name is NUL-terminated and lives through this synchronous call.
    // No security attributes or template handle are supplied. The explicit
    // access mask excludes create-instance rights; SQOS forbids impersonation.
    let handle = unsafe {
        CreateFileW(
            PCWSTR(name.as_ptr()),
            CLIENT_ACCESS,
            FILE_SHARE_MODE(0),
            None,
            OPEN_EXISTING,
            FILE_FLAG_OVERLAPPED | SECURITY_SQOS_PRESENT | SECURITY_IDENTIFICATION,
            None,
        )
    }
    .map_err(|error| ConnectError::Io(io::Error::from_raw_os_error(win32_code(&error) as i32)))?;
    // SAFETY: CreateFileW succeeded and returned a newly owned valid handle.
    // Adopt it immediately so every rejection and query failure closes it.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let raw = HANDLE(handle.as_raw_handle());

    let mut session_id = 0;
    // SAFETY: raw borrows the live pipe handle; session_id is writable output.
    let session = unsafe { GetNamedPipeServerSessionId(raw, &mut session_id) }
        .map(|()| session_id)
        .map_err(|error| Reject::Query {
            step: "GetNamedPipeServerSessionId",
            code: win32_code(&error),
        });
    let owner = match query_owner(raw, SE_KERNEL_OBJECT) {
        Err(Reject::Query {
            step: "GetSecurityInfo",
            code,
        }) if code == ERROR_INVALID_PARAMETER.0 => query_owner(raw, SE_FILE_OBJECT),
        result => result,
    };
    check_server(session, owner).map_err(ConnectError::Untrusted)?;

    // SAFETY: Both trust checks passed. Transfer sole ownership of the connected,
    // overlapped pipe to Tokio, which closes it even if registration fails. The
    // caller polls this function inside the bridge's I/O-enabled Tokio runtime.
    unsafe { NamedPipeClient::from_raw_handle(handle.into_raw_handle()) }.map_err(ConnectError::Io)
}
