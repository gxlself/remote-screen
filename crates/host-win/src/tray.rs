#![cfg(windows)]
//! Starts the tray with the logged-in user's filtered token, never the service's
//! SYSTEM token or an administrator's linked elevated token.

use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;

use host_core::platform::PlatformError;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::HANDLE;
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityIdentification, TokenPrimary, TOKEN_ACCESS_MASK,
};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::RemoteDesktop::{
    WTSEnumerateProcessesW, WTSFreeMemory, WTSQueryUserToken, WTS_PROCESS_INFOW,
};
use windows::Win32::System::SystemServices::MAXIMUM_ALLOWED;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, CREATE_UNICODE_ENVIRONMENT, PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::svc_os_model::{decide_tray, is_image, TrayDecision};
use crate::winproc_model::{command_line, nul_terminated_utf16};

fn platform_error(api: &str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Platform(format!("{api}: {error}"))
}

pub(crate) fn user_token(session_id: u32) -> Result<OwnedHandle, PlatformError> {
    let mut token = HANDLE::default();
    // SAFETY: token is writable output; WTS checks SYSTEM/SeTcbPrivilege and
    // returns the logged-in user's token for the supplied session.
    unsafe { WTSQueryUserToken(session_id, &mut token) }
        .map_err(|error| platform_error("WTSQueryUserToken", error))?;
    // SAFETY: WTSQueryUserToken transferred a valid unique handle on success.
    Ok(unsafe { OwnedHandle::from_raw_handle(token.0) })
}

/// Only SYSTEM with SeTcbPrivilege can query the user token. OwnedHandle closes
/// it immediately, including when this function is used only as a predicate.
pub fn session_has_user(session_id: u32) -> bool {
    user_token(session_id).is_ok()
}

struct ProcessList(*mut WTS_PROCESS_INFOW);

impl Drop for ProcessList {
    fn drop(&mut self) {
        if !self.0.is_null() {
            // SAFETY: This guard solely owns the allocation returned by WTS,
            // including any non-null allocation on the enumeration error path.
            unsafe { WTSFreeMemory(self.0.cast()) };
        }
    }
}

pub fn running_in_session(session_id: u32, image: &str) -> Result<bool, PlatformError> {
    let mut processes = ProcessList(std::ptr::null_mut());
    let mut count = 0;
    // SAFETY: The current server is selected, version is 1, and both outputs
    // are writable. The guard frees the returned memory on all exit paths.
    unsafe { WTSEnumerateProcessesW(None, 0, 1, &mut processes.0, &mut count) }
        .map_err(|error| platform_error("WTSEnumerateProcessesW", error))?;
    if count == 0 {
        return Ok(false);
    }
    if processes.0.is_null() {
        return Err(platform_error(
            "WTSEnumerateProcessesW",
            "null process list",
        ));
    }
    // SAFETY: Successful enumeration returns count initialized records, kept
    // alive by the guard while this slice and their names are inspected.
    let entries = unsafe { std::slice::from_raw_parts(processes.0, count as usize) };
    for entry in entries {
        if entry.SessionId != session_id || entry.pProcessName.is_null() {
            continue;
        }
        // SAFETY: WTS owns this NUL-terminated UTF-16 name inside the live list.
        let name = unsafe { entry.pProcessName.to_string() }
            .map_err(|error| platform_error("WTSEnumerateProcessesW process name", error))?;
        if is_image(&name, image) {
            return Ok(true);
        }
    }
    Ok(false)
}

struct EnvironmentBlock(*mut c_void);

impl Drop for EnvironmentBlock {
    fn drop(&mut self) {
        // SAFETY: This guard solely owns a successful CreateEnvironmentBlock allocation.
        if let Err(error) = unsafe { DestroyEnvironmentBlock(self.0) } {
            tracing::warn!(%error, "DestroyEnvironmentBlock failed");
        }
    }
}

fn wide(value: &str) -> Result<Vec<u16>, PlatformError> {
    nul_terminated_utf16(value)
        .ok_or_else(|| platform_error("CreateProcessAsUserW", "input contains NUL"))
}

/// Requires SYSTEM and an absolute Unicode executable path. No handle is
/// inherited, and dropping the process handles does not terminate the child.
pub fn spawn_as_user(session_id: u32, exe: &Path, args: &[&str]) -> Result<u32, PlatformError> {
    if !exe.is_absolute() {
        return Err(platform_error(
            "CreateProcessAsUserW",
            "executable path must be absolute",
        ));
    }
    let exe_text = exe
        .to_str()
        .ok_or_else(|| platform_error("CreateProcessAsUserW", "executable path is not Unicode"))?;
    let parent = exe.parent().and_then(Path::to_str).ok_or_else(|| {
        platform_error("CreateProcessAsUserW", "executable has no parent directory")
    })?;
    let application = wide(exe_text)?;
    let directory = wide(parent)?;
    let mut command = wide(&command_line(exe_text, args))?;
    let mut desktop = wide(r"winsta0\default")?;
    let token = user_token(session_id)?;
    let mut duplicate = HANDLE::default();
    // SAFETY: token remains live, no security attributes are supplied, and the
    // writable output receives a new primary token without changing elevation.
    unsafe {
        DuplicateTokenEx(
            HANDLE(token.as_raw_handle()),
            TOKEN_ACCESS_MASK(MAXIMUM_ALLOWED),
            None,
            SecurityIdentification,
            TokenPrimary,
            &mut duplicate,
        )
    }
    .map_err(|error| platform_error("DuplicateTokenEx", error))?;
    // SAFETY: DuplicateTokenEx transferred a unique primary token on success.
    let duplicate = unsafe { OwnedHandle::from_raw_handle(duplicate.0) };
    let mut environment = std::ptr::null_mut();
    // SAFETY: The token is owned and the output is writable; false prevents
    // inheriting the SYSTEM service's environment into the user's tray.
    unsafe {
        CreateEnvironmentBlock(
            &mut environment,
            Some(HANDLE(duplicate.as_raw_handle())),
            false,
        )
    }
    .map_err(|error| platform_error("CreateEnvironmentBlock", error))?;
    let environment = EnvironmentBlock(environment);
    let startup = STARTUPINFOW {
        cb: size_of::<STARTUPINFOW>() as u32,
        lpDesktop: PWSTR(desktop.as_mut_ptr()),
        ..Default::default()
    };
    let mut process = PROCESS_INFORMATION::default();
    // SAFETY: All strings are NUL terminated and live, command is writable,
    // token/environment are owned, output is writable, and handles are not
    // inherited. An explicit application path avoids ambiguous command parsing.
    unsafe {
        CreateProcessAsUserW(
            Some(HANDLE(duplicate.as_raw_handle())),
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT,
            Some(environment.0.cast_const()),
            PCWSTR(directory.as_ptr()),
            &startup,
            &mut process,
        )
    }
    .map_err(|error| platform_error("CreateProcessAsUserW", error))?;
    // SAFETY: Successful process creation transfers the unique process handle.
    let _process = unsafe { OwnedHandle::from_raw_handle(process.hProcess.0) };
    // SAFETY: Successful process creation also transfers the unique thread handle.
    let _thread = unsafe { OwnedHandle::from_raw_handle(process.hThread.0) };
    Ok(process.dwProcessId)
}

pub fn ensure_tray(exe: &Path) -> Result<(TrayDecision, Option<u32>), PlatformError> {
    let console = crate::wts::active_console_session();
    let exists = exe.is_file();
    let has_user = exists && console.is_some_and(session_has_user);
    let running = if let Some(session) = console.filter(|_| has_user) {
        let image = exe
            .file_name()
            .and_then(|name| name.to_str())
            .ok_or_else(|| platform_error("ensure_tray", "executable name is not Unicode"))?;
        running_in_session(session, image)?
    } else {
        false
    };
    let decision = decide_tray(console, has_user, running, exists);
    tracing::info!(?decision, "tray launch decision");
    let pid = match (decision, console) {
        (TrayDecision::Launch, Some(session)) => Some(spawn_as_user(session, exe, &[])?),
        _ => None,
    };
    Ok((decision, pid))
}
