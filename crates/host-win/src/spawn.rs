//! Launches session workers using a duplicate of the SYSTEM service token.

use std::ffi::c_void;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::Path;
use std::sync::Arc;

use host_core::platform::PlatformError;
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::{HANDLE, WAIT_OBJECT_0};
use windows::Win32::Security::{
    DuplicateTokenEx, SecurityIdentification, SetTokenInformation, TokenPrimary, TokenSessionId,
    TOKEN_ACCESS_MASK, TOKEN_ADJUST_DEFAULT, TOKEN_ADJUST_SESSIONID, TOKEN_ASSIGN_PRIMARY,
    TOKEN_DUPLICATE, TOKEN_QUERY,
};
use windows::Win32::System::Environment::{CreateEnvironmentBlock, DestroyEnvironmentBlock};
use windows::Win32::System::SystemServices::MAXIMUM_ALLOWED;
use windows::Win32::System::Threading::{
    CreateProcessAsUserW, GetCurrentProcess, GetExitCodeProcess, OpenProcessToken,
    TerminateProcess, WaitForSingleObject, CREATE_NO_WINDOW, CREATE_UNICODE_ENVIRONMENT, INFINITE,
    PROCESS_INFORMATION, STARTUPINFOW,
};

use crate::winproc_model::{command_line, nul_terminated_utf16};

struct EnvironmentBlock(*mut c_void);

impl Drop for EnvironmentBlock {
    fn drop(&mut self) {
        // SAFETY: This is the sole owner of a successful CreateEnvironmentBlock allocation.
        if let Err(error) = unsafe { DestroyEnvironmentBlock(self.0) } {
            tracing::warn!(%error, "DestroyEnvironmentBlock failed");
        }
    }
}

pub struct ChildProcess {
    pub pid: u32,
    handle: Arc<OwnedHandle>,
}

fn platform_error(api: &str, error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Platform(format!("{api}: {error}"))
}

fn wide(value: &str) -> Result<Vec<u16>, PlatformError> {
    nul_terminated_utf16(value)
        .ok_or_else(|| platform_error("CreateProcessAsUserW", "input contains NUL"))
}

/// Requires a SYSTEM service with SeTcbPrivilege enabled to set TokenSessionId.
/// CreateProcessAsUserW additionally needs SeIncreaseQuotaPrivilege and may need
/// SeAssignPrimaryTokenPrivilege. No user token or user environment is used.
/// `exe` must be an absolute Unicode path; the child starts in its parent directory
/// on `winsta0\default`. Dropping the returned handle does not terminate the child.
pub fn spawn_in_session(
    session_id: u32,
    exe: &Path,
    args: &[&str],
) -> Result<ChildProcess, PlatformError> {
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
    let mut desktop = wide("winsta0\\default")?;
    let mut token = HANDLE::default();
    // SAFETY: The current process pseudo-handle is valid and token is writable output.
    unsafe {
        OpenProcessToken(
            GetCurrentProcess(),
            TOKEN_DUPLICATE
                | TOKEN_QUERY
                | TOKEN_ASSIGN_PRIMARY
                | TOKEN_ADJUST_DEFAULT
                | TOKEN_ADJUST_SESSIONID,
            &mut token,
        )
    }
    .map_err(|error| platform_error("OpenProcessToken", error))?;
    // SAFETY: OpenProcessToken returned a valid, uniquely owned handle.
    let token = unsafe { OwnedHandle::from_raw_handle(token.0) };
    let mut duplicate = HANDLE::default();
    // SAFETY: token stays alive, no security attributes are supplied, and duplicate is writable output.
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
    // SAFETY: DuplicateTokenEx returned a valid, uniquely owned primary token.
    let duplicate = unsafe { OwnedHandle::from_raw_handle(duplicate.0) };
    // SAFETY: The input points to a live DWORD of exactly the stated size, and the token is owned.
    unsafe {
        SetTokenInformation(
            HANDLE(duplicate.as_raw_handle()),
            TokenSessionId,
            (&session_id as *const u32).cast(),
            size_of::<u32>() as u32,
        )
    }
    .map_err(|error| platform_error("SetTokenInformation", error))?;
    let mut environment = std::ptr::null_mut();
    // SAFETY: The token is live and environment is writable output, adopted immediately on success.
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
    // SAFETY: All strings are NUL terminated and live through the call; command
    // is writable. The environment/token are owned, output is writable, and no
    // handles are inherited. lpApplicationName specifies the executable exactly.
    unsafe {
        CreateProcessAsUserW(
            Some(HANDLE(duplicate.as_raw_handle())),
            PCWSTR(application.as_ptr()),
            Some(PWSTR(command.as_mut_ptr())),
            None,
            None,
            false,
            CREATE_UNICODE_ENVIRONMENT | CREATE_NO_WINDOW,
            Some(environment.0.cast_const()),
            PCWSTR(directory.as_ptr()),
            &startup,
            &mut process,
        )
    }
    .map_err(|error| platform_error("CreateProcessAsUserW", error))?;
    // SAFETY: A successful CreateProcessAsUserW transfers both unique handles to us.
    let process_handle = unsafe { OwnedHandle::from_raw_handle(process.hProcess.0) };
    // SAFETY: The primary thread handle is valid and has not been adopted elsewhere.
    let thread_handle = unsafe { OwnedHandle::from_raw_handle(process.hThread.0) };
    drop(thread_handle);
    Ok(ChildProcess {
        pid: process.dwProcessId,
        handle: Arc::new(process_handle),
    })
}

impl ChildProcess {
    /// Waits off the async executor. An OS/join failure is logged and returns
    /// u32::MAX because this contract has no error return. Cancelling the future
    /// does not cancel the OS wait; its Arc keeps the process handle alive.
    pub async fn wait(&self) -> u32 {
        let handle = Arc::clone(&self.handle);
        let result = tokio::task::spawn_blocking(move || {
            let raw = HANDLE(handle.as_raw_handle());
            // SAFETY: The closure owns an Arc to the live process handle throughout the wait.
            if unsafe { WaitForSingleObject(raw, INFINITE) } != WAIT_OBJECT_0 {
                return Err(platform_error(
                    "WaitForSingleObject",
                    windows::core::Error::from_thread(),
                ));
            }
            let mut code = 0;
            // SAFETY: The process handle remains live and code is writable output.
            unsafe { GetExitCodeProcess(raw, &mut code) }
                .map_err(|error| platform_error("GetExitCodeProcess", error))?;
            Ok(code)
        })
        .await;
        match result {
            Ok(Ok(code)) => code,
            Ok(Err(error)) => {
                tracing::error!(pid = self.pid, %error, "process wait failed");
                u32::MAX
            }
            Err(error) => {
                tracing::error!(pid = self.pid, %error, "process wait task failed");
                u32::MAX
            }
        }
    }

    pub fn kill(&self) -> Result<(), PlatformError> {
        let handle = HANDLE(self.handle.as_raw_handle());
        // SAFETY: self owns a live process handle; a zero timeout only queries its state.
        if unsafe { WaitForSingleObject(handle, 0) } == WAIT_OBJECT_0 {
            return Ok(());
        }
        // SAFETY: The owned process handle has termination rights from process creation.
        if let Err(error) = unsafe { TerminateProcess(handle, 1) } {
            // SAFETY: Recheck the same live handle in case exit raced with TerminateProcess.
            if unsafe { WaitForSingleObject(handle, 0) } != WAIT_OBJECT_0 {
                return Err(platform_error("TerminateProcess", error));
            }
        }
        Ok(())
    }
}
