#![cfg(windows)]
//! SCM entry point for one own-process service. A body panic is deliberately
//! not caught: crossing extern "system" aborts the process, allowing the MSI's
//! crash-only recovery actions to restart it. The agent supplies the panic hook.

use std::ffi::c_void;
use std::sync::{Arc, Mutex, OnceLock};
use std::time::Duration;

use tokio::sync::mpsc::{self, UnboundedReceiver, UnboundedSender};
use windows::core::{PCWSTR, PWSTR};
use windows::Win32::Foundation::ERROR_FAILED_SERVICE_CONTROLLER_CONNECT;
use windows::Win32::System::RemoteDesktop::WTSSESSION_NOTIFICATION;
use windows::Win32::System::Services::{
    RegisterServiceCtrlHandlerExW, SetServiceStatus, StartServiceCtrlDispatcherW, SERVICE_STATUS,
    SERVICE_STATUS_CURRENT_STATE, SERVICE_STATUS_HANDLE, SERVICE_TABLE_ENTRYW,
    SERVICE_WIN32_OWN_PROCESS,
};

use crate::svc_os_model::{
    controls_accepted, exit_codes, map_control, state_code, ScmEvent, ScmState, StatusSink,
    ERROR_CALL_NOT_IMPLEMENTED, START_WAIT_HINT, STOP_WAIT_HINT,
};
use crate::winproc_model::nul_terminated_utf16;

#[derive(Debug, thiserror::Error)]
pub enum ScmError {
    #[error("not started by the service control manager")]
    NotUnderScm,
    #[error("{0}")]
    Api(String),
}

pub type ServiceBody =
    Box<dyn FnOnce(Arc<dyn StatusSink>, UnboundedReceiver<ScmEvent>) -> u32 + Send + 'static>;

struct ServiceStart {
    name: Vec<u16>,
    body: ServiceBody,
}

static SERVICE: OnceLock<Mutex<Option<ServiceStart>>> = OnceLock::new();

/// Blocks until the dispatcher returns. The process reserves its one dispatcher
/// attempt even if registration fails; the body is never run a second time.
pub fn run_service(name: &str, body: ServiceBody) -> Result<(), ScmError> {
    let mut name = nul_terminated_utf16(name)
        .filter(|name| name.len() > 1)
        .ok_or_else(|| ScmError::Api("invalid service name".into()))?;
    SERVICE
        .set(Mutex::new(Some(ServiceStart {
            name: name.clone(),
            body,
        })))
        .map_err(|_| ScmError::Api("service already started".into()))?;
    let table = [
        SERVICE_TABLE_ENTRYW {
            lpServiceName: PWSTR(name.as_mut_ptr()),
            lpServiceProc: Some(service_main),
        },
        SERVICE_TABLE_ENTRYW::default(),
    ];
    // SAFETY: The table has a null terminator and its name storage stays alive
    // until this blocking dispatcher call returns. The callback has the SCM ABI.
    let result = unsafe { StartServiceCtrlDispatcherW(table.as_ptr()) };
    if result.is_err() {
        // Release an unused body on dispatcher failure, but keep the once guard.
        if let Some(slot) = SERVICE.get() {
            slot.lock()
                .unwrap_or_else(|error| error.into_inner())
                .take();
        }
    }
    result.map_err(|error| {
        if error.code()
            == windows::core::HRESULT::from_win32(ERROR_FAILED_SERVICE_CONTROLLER_CONNECT.0)
        {
            ScmError::NotUnderScm
        } else {
            ScmError::Api(error.to_string())
        }
    })
}

struct HandlerCtx {
    tx: UnboundedSender<ScmEvent>,
    status: OnceLock<Arc<ScmStatus>>,
}

// SAFETY: SCM invokes this callback with its documented ABI; arguments are
// intentionally unused. All raw pointer operations below have local checks.
unsafe extern "system" fn service_main(_argc: u32, _argv: *mut PWSTR) {
    let Some(slot) = SERVICE.get() else {
        return;
    };
    let Some(start) = slot
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
    else {
        return;
    };
    let (tx, rx) = mpsc::unbounded_channel();
    // SCM may retain the context until process exit, including after body exit.
    let ctx: &'static HandlerCtx = Box::leak(Box::new(HandlerCtx {
        tx,
        status: OnceLock::new(),
    }));
    // SAFETY: name is NUL terminated and live during registration, and ctx is
    // immutable, synchronized and leaked for the entire callback lifetime.
    let handle = match unsafe {
        RegisterServiceCtrlHandlerExW(
            PCWSTR(start.name.as_ptr()),
            Some(handler),
            Some((ctx as *const HandlerCtx).cast()),
        )
    } {
        Ok(handle) => handle,
        Err(error) => {
            tracing::error!(%error, "RegisterServiceCtrlHandlerExW failed");
            return;
        }
    };
    let status = Arc::new(ScmStatus {
        handle,
        state: Mutex::new((ScmState::StartPending, 0)),
    });
    let _ = ctx.status.set(Arc::clone(&status));
    status.report(ScmState::StartPending, START_WAIT_HINT, 0);
    let code = (start.body)(status.clone(), rx);
    status.report(ScmState::Stopped, Duration::ZERO, code);
}

// SAFETY: This ABI and context layout match RegisterServiceCtrlHandlerExW.
unsafe extern "system" fn handler(
    control: u32,
    event_type: u32,
    event_data: *mut c_void,
    context: *mut c_void,
) -> u32 {
    if context.is_null() {
        return ERROR_CALL_NOT_IMPLEMENTED;
    }
    // SAFETY: Registration supplies a leaked HandlerCtx, never freed or mutated
    // without synchronization, for every invocation of this handler.
    let ctx = unsafe { &*context.cast::<HandlerCtx>() };
    let session_id = if control == 14 && !event_data.is_null() {
        // SAFETY: SCM guarantees a valid WTSSESSION_NOTIFICATION for SESSIONCHANGE;
        // other controls and null event pointers are excluded above.
        Some(unsafe { (*event_data.cast::<WTSSESSION_NOTIFICATION>()).dwSessionId })
    } else {
        None
    };
    let (event, result) = map_control(control, event_type, session_id);
    if let Some(event) = event {
        if matches!(event, ScmEvent::Stop | ScmEvent::Shutdown) {
            if let Some(status) = ctx.status.get() {
                status.stopping(STOP_WAIT_HINT);
            }
        }
        let _ = ctx.tx.send(event);
    }
    result
}

struct ScmStatus {
    handle: SERVICE_STATUS_HANDLE,
    state: Mutex<(ScmState, u32)>,
}

// SAFETY: The SCM status handle lives for the process lifetime and
// SetServiceStatus may be called from any thread. The mutex serializes reports.
unsafe impl Send for ScmStatus {}
// SAFETY: The handle has the same process-wide lifetime on shared references;
// SetServiceStatus is thread-safe, and mutable state is protected by the mutex.
unsafe impl Sync for ScmStatus {}

impl ScmStatus {
    fn report(&self, next: ScmState, wait_hint: Duration, code: u32) {
        let mut state = self.state.lock().unwrap_or_else(|error| error.into_inner());
        // Stopped is terminal (SCM closes its RPC context after that report).
        // A late startup notification must not undo a concurrent stop request.
        if state.0 == ScmState::Stopped
            || (state.0 == ScmState::StopPending
                && matches!(next, ScmState::StartPending | ScmState::Running))
        {
            return;
        }
        let pending = matches!(next, ScmState::StartPending | ScmState::StopPending);
        let checkpoint = if pending {
            state.1.saturating_add(1)
        } else {
            0
        };
        let (win32, specific) = exit_codes(code);
        let status = SERVICE_STATUS {
            dwServiceType: SERVICE_WIN32_OWN_PROCESS,
            dwCurrentState: SERVICE_STATUS_CURRENT_STATE(state_code(next)),
            dwControlsAccepted: controls_accepted(next),
            dwWin32ExitCode: win32,
            dwServiceSpecificExitCode: specific,
            dwCheckPoint: checkpoint,
            dwWaitHint: if pending {
                wait_hint.as_millis().min(u128::from(u32::MAX)) as u32
            } else {
                0
            },
        };
        *state = (next, checkpoint);
        // SAFETY: The registered SCM handle is live, status is readable for the
        // call, and the mutex prevents overlapping or out-of-order reports.
        if let Err(error) = unsafe { SetServiceStatus(self.handle, &status) } {
            tracing::error!(%error, ?next, "SetServiceStatus failed");
        }
    }
}

impl StatusSink for ScmStatus {
    fn running(&self) {
        self.report(ScmState::Running, Duration::ZERO, 0);
    }
    fn stopping(&self, wait_hint: Duration) {
        self.report(ScmState::StopPending, wait_hint, 0);
    }
}
