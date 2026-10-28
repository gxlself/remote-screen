//! Local byte-mode named pipes for the service, session process and host UI.

use std::io;
use std::os::windows::io::AsRawHandle;
use std::time::Duration;

use host_core::ipc_listener::IpcListener;
use tokio::net::windows::named_pipe::{
    ClientOptions, NamedPipeClient, NamedPipeServer, PipeMode, ServerOptions,
};
use windows::core::PCWSTR;
use windows::Win32::Foundation::{
    LocalFree, ERROR_FILE_NOT_FOUND, ERROR_PIPE_BUSY, HANDLE, HLOCAL,
};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SDDL_REVISION_1,
};
use windows::Win32::Security::{PSECURITY_DESCRIPTOR, SECURITY_ATTRIBUTES};
use windows::Win32::System::Pipes::{GetNamedPipeClientProcessId, GetNamedPipeClientSessionId};

use crate::pipe_proto::UI_PIPE_NAME;
use crate::winproc_model::{nul_terminated_utf16, verify_console_client, UI_PIPE_SDDL};

struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW allocated
        // this descriptor with LocalAlloc; this is its sole owner.
        unsafe { LocalFree(Some(HLOCAL(self.0 .0))) };
    }
}

pub struct PipeServer {
    name: String,
    sddl: String,
    pending: NamedPipeServer,
}

impl PipeServer {
    /// Creates the first instance immediately, failing if the name is already owned.
    /// Must be called within a Tokio runtime with I/O enabled.
    pub fn new(name: &str, sddl: &str) -> io::Result<Self> {
        let pending = create_instance(name, sddl, true)?;
        Ok(Self {
            name: name.to_owned(),
            sddl: sddl.to_owned(),
            pending,
        })
    }

    /// Keeps a replacement instance available before returning the connected one.
    /// Dropping a pending accept future leaves the listener's instance intact.
    pub async fn accept(&mut self) -> io::Result<(NamedPipeServer, u32)> {
        self.pending.connect().await?;
        let next = match create_instance(&self.name, &self.sddl, false) {
            Ok(next) => next,
            Err(error) => {
                // Preserve the first instance and let a later accept retry.
                let _ = self.pending.disconnect();
                return Err(error);
            }
        };
        let connected = std::mem::replace(&mut self.pending, next);
        let mut pid = 0;
        // SAFETY: The connected pipe owns a live handle; pid is writable output.
        unsafe { GetNamedPipeClientProcessId(HANDLE(connected.as_raw_handle()), &mut pid) }
            .map_err(io::Error::from)?;
        Ok((connected, pid))
    }
}

fn create_instance(name: &str, sddl: &str, first: bool) -> io::Result<NamedPipeServer> {
    if name.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pipe name contains NUL",
        ));
    }
    let wide = nul_terminated_utf16(sddl)
        .ok_or_else(|| io::Error::new(io::ErrorKind::InvalidInput, "SDDL contains NUL"))?;
    let mut descriptor = PSECURITY_DESCRIPTOR::default();
    // SAFETY: wide is NUL terminated; descriptor is writable output and is
    // adopted immediately below after a successful allocation.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(wide.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor,
            None,
        )
    }
    .map_err(io::Error::from)?;
    let descriptor = SecurityDescriptor(descriptor);
    let attributes = SECURITY_ATTRIBUTES {
        nLength: size_of::<SECURITY_ATTRIBUTES>() as u32,
        lpSecurityDescriptor: descriptor.0 .0,
        bInheritHandle: false.into(),
    };
    // SAFETY: attributes and its owned descriptor both outlive the synchronous
    // CreateNamedPipe call. Windows copies the descriptor, and handles are not inherited.
    unsafe {
        ServerOptions::new()
            .first_pipe_instance(first)
            .reject_remote_clients(true)
            .pipe_mode(PipeMode::Byte)
            .create_with_security_attributes_raw(
                name,
                (&attributes as *const SECURITY_ATTRIBUTES)
                    .cast_mut()
                    .cast(),
            )
    }
}

/// The inner server is private so callers cannot bypass the UI ACL constructor.
pub struct UiPipeListener(PipeServer);

impl UiPipeListener {
    pub fn new() -> io::Result<Self> {
        PipeServer::new(UI_PIPE_NAME, UI_PIPE_SDDL).map(Self)
    }
}

impl IpcListener for UiPipeListener {
    type Stream = NamedPipeServer;

    async fn accept(&mut self) -> io::Result<Self::Stream> {
        loop {
            let (stream, pid) = self.0.accept().await?;
            let mut session = 0;
            // SAFETY: stream owns a connected pipe handle; session is writable output.
            let result = unsafe {
                GetNamedPipeClientSessionId(HANDLE(stream.as_raw_handle()), &mut session)
            };
            if let Err(error) = result {
                tracing::warn!(pid, %error, "cannot verify UI pipe client session");
                let _ = stream.disconnect();
                continue;
            }
            let console = crate::wts::active_console_session();
            if !verify_console_client(console, session) {
                tracing::warn!(pid, session, console = ?console, "rejecting UI pipe client outside active console");
                let _ = stream.disconnect();
                continue;
            }
            tracing::debug!(pid, session, "accepted UI pipe client");
            return Ok(stream);
        }
    }
}

/// Connects the SYSTEM session process, retrying only transient startup errors.
/// Ordinary UI clients must request the specific rights in UI_PIPE_SDDL rather
/// than GENERIC_WRITE (which also requests FILE_CREATE_PIPE_INSTANCE).
pub async fn connect_client(name: &str, timeout: Duration) -> io::Result<NamedPipeClient> {
    if name.contains('\0') {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "pipe name contains NUL",
        ));
    }
    tokio::time::timeout(timeout, async {
        loop {
            match ClientOptions::new().open(name) {
                Ok(client) => return Ok(client),
                Err(error) if matches!(error.raw_os_error(), Some(code) if code == ERROR_PIPE_BUSY.0 as i32 || code == ERROR_FILE_NOT_FOUND.0 as i32) => {
                    tokio::time::sleep(Duration::from_millis(50)).await;
                }
                Err(error) => return Err(error),
            }
        }
    }).await.map_err(|_| io::Error::new(io::ErrorKind::TimedOut, "timed out connecting to named pipe"))?
}
