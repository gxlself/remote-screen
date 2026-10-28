//! Registration of the bundled login agent. Only the UI owns this lifecycle;
//! quitting the UI leaves the launchd-managed agent running.

#[cfg(any(target_os = "macos", test))]
use std::path::Path;

// Only the macOS registration (and tests) construct the non-`Unsupported` variants.
#[cfg_attr(not(any(target_os = "macos", test)), allow(dead_code))]
#[derive(Clone, Copy, Debug, Eq, PartialEq)]
pub enum AgentStatus {
    Enabled,
    RequiresApproval,
    NotRegistered,
    NotFound,
    Unsupported,
}

impl AgentStatus {
    #[cfg(any(target_os = "macos", test))]
    fn from_raw(value: isize) -> Self {
        match value {
            0 => Self::NotRegistered,
            1 => Self::Enabled,
            2 => Self::RequiresApproval,
            3 => Self::NotFound,
            _ => Self::Unsupported,
        }
    }

    fn as_str(self) -> &'static str {
        match self {
            Self::Enabled => "enabled",
            Self::RequiresApproval => "requires_approval",
            Self::NotRegistered => "not_registered",
            Self::NotFound => "not_found",
            Self::Unsupported => "unsupported",
        }
    }
}

#[cfg(any(target_os = "macos", test))]
fn running_from_bundle(exe: &Path) -> bool {
    exe.to_string_lossy().contains(".app/Contents/MacOS/")
}

#[cfg(target_os = "macos")]
mod platform {
    use objc2::{
        msg_send,
        rc::{autoreleasepool, Retained},
        runtime::{AnyClass, AnyObject},
    };
    use objc2_foundation::{ns_string, NSError};

    use super::AgentStatus;

    #[link(name = "ServiceManagement", kind = "framework")]
    extern "C" {}

    fn service_class() -> Result<&'static AnyClass, String> {
        AnyClass::get(c"SMAppService")
            .ok_or_else(|| "SMAppService is unavailable (macOS 13 or newer is required)".into())
    }

    fn service() -> Result<Retained<AnyObject>, String> {
        let class = service_class()?;
        // SAFETY: SMAppService's class method accepts an NSString plist name and
        // returns an autoreleased SMAppService. Retained keeps that object alive.
        let service: Option<Retained<AnyObject>> = unsafe {
            msg_send![class, agentServiceWithPlistName: ns_string!("ai.gxlself.remotehost.agent.plist")]
        };
        service.ok_or_else(|| "SMAppService could not create the bundled agent service".into())
    }

    pub(super) fn status() -> Result<AgentStatus, String> {
        autoreleasepool(|_| {
            let service = service()?;
            // SAFETY: This is an SMAppService; status returns SMAppServiceStatus
            // (an NSInteger), represented by isize on both supported macOS ABIs.
            let raw: isize = unsafe { msg_send![&*service, status] };
            Ok(AgentStatus::from_raw(raw))
        })
    }

    pub(super) fn register() -> Result<AgentStatus, String> {
        autoreleasepool(|_| {
            let service = service()?;
            // SAFETY: The selector returns BOOL and writes an autoreleased
            // NSError on failure. msg_send! retains the error for this Result.
            let result: Result<(), Retained<NSError>> =
                unsafe { msg_send![&*service, registerAndReturnError: _] };
            result.map_err(|error| format!("register agent: {error}"))?;
            status()
        })
    }

    pub(super) fn unregister() -> Result<(), String> {
        autoreleasepool(|_| {
            if status()? == AgentStatus::NotRegistered {
                return Ok(());
            }
            let service = service()?;
            // SAFETY: The selector has the same BOOL/NSError** convention as
            // registerAndReturnError:. The service remains retained for the call.
            let result: Result<(), Retained<NSError>> =
                unsafe { msg_send![&*service, unregisterAndReturnError: _] };
            result.map_err(|error| format!("unregister agent: {error}"))
        })
    }

    pub(super) fn open_settings() -> Result<(), String> {
        autoreleasepool(|_| {
            let class = service_class()?;
            // SAFETY: This is the documented no-argument SMAppService class
            // method; it returns void and opens System Settings' Login Items.
            let _: () = unsafe { msg_send![class, openSystemSettingsLoginItems] };
            Ok(())
        })
    }
}

pub fn ensure_registered() {
    #[cfg(target_os = "macos")]
    {
        match std::env::current_exe() {
            Ok(exe) if running_from_bundle(&exe) => {}
            Ok(_) => {
                tracing::info!(
                    "Skipping agent registration outside an app bundle (development mode)"
                );
                return;
            }
            Err(error) => {
                tracing::error!(%error, "Could not locate the UI executable for agent registration");
                return;
            }
        }
        match platform::status() {
            Ok(AgentStatus::NotRegistered) => match platform::register() {
                Ok(status) => {
                    tracing::info!(status = status.as_str(), "Registered Remote Host agent")
                }
                Err(error) => tracing::error!(%error, "Could not register Remote Host agent"),
            },
            Ok(AgentStatus::RequiresApproval) => {
                tracing::info!(
                    "Remote Host agent requires approval in System Settings' Login Items"
                );
            }
            Ok(AgentStatus::NotFound) => {
                tracing::error!("Remote Host agent plist was not found in the app bundle");
            }
            Ok(AgentStatus::Enabled) => {}
            Ok(AgentStatus::Unsupported) => {
                tracing::error!("SMAppService returned an unsupported agent status");
            }
            Err(error) => tracing::error!(%error, "Could not query Remote Host agent registration"),
        }
    }
}

pub fn unregister() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        platform::unregister()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("unsupported".into())
    }
}

#[tauri::command]
pub fn agent_service_status() -> String {
    #[cfg(target_os = "macos")]
    let status = platform::status().unwrap_or_else(|error| {
        tracing::error!(%error, "Could not query Remote Host agent registration");
        AgentStatus::Unsupported
    });
    #[cfg(not(target_os = "macos"))]
    let status = AgentStatus::Unsupported;
    status.as_str().into()
}

#[tauri::command]
pub fn agent_service_register() -> Result<String, String> {
    #[cfg(target_os = "macos")]
    {
        if !running_from_bundle(&std::env::current_exe().map_err(|error| error.to_string())?) {
            return Err("Agent registration requires a bundled Remote Host.app".into());
        }
        platform::register().map(|status| status.as_str().into())
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("unsupported".into())
    }
}

#[tauri::command]
pub fn agent_service_open_settings() -> Result<(), String> {
    #[cfg(target_os = "macos")]
    {
        platform::open_settings()
    }
    #[cfg(not(target_os = "macos"))]
    {
        Err("unsupported".into())
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_sm_app_service_status() {
        for (raw, expected) in [
            (0, AgentStatus::NotRegistered),
            (1, AgentStatus::Enabled),
            (2, AgentStatus::RequiresApproval),
            (3, AgentStatus::NotFound),
            (-1, AgentStatus::Unsupported),
            (4, AgentStatus::Unsupported),
            (isize::MAX, AgentStatus::Unsupported),
        ] {
            assert_eq!(AgentStatus::from_raw(raw), expected);
        }
    }

    #[test]
    fn recognizes_a_bundled_executable() {
        assert!(running_from_bundle(Path::new(
            "/Applications/Remote Host.app/Contents/MacOS/Remote Host"
        )));
        assert!(running_from_bundle(Path::new(
            "/tmp/build output/Remote Host.app/Contents/MacOS/Remote Host"
        )));
        assert!(!running_from_bundle(Path::new(
            "target/debug/remote-host-ui"
        )));
        assert!(!running_from_bundle(Path::new(
            "/Applications/Remote Host.app.backup/Contents/MacOS/Remote Host"
        )));
    }
}
