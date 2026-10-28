//! Service control, tray and deployment rules independent of Win32.
//! Deployment markers contain export metadata only, never pairing secrets.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::Duration;

use rsp_core::trust::Permission;
use serde::{Deserialize, Serialize};
use sha2::{Digest, Sha256};

use crate::supervisor::SessionChangeKind;

pub const SERVICE_NAME: &str = "RemoteHostService";
pub const TRAY_EXE: &str = "RemoteHost.exe";
pub const DEPLOY_REG_KEY: &str = r"SOFTWARE\Remote Host\Deploy";
pub const DEPLOY_MARKER: &str = "deploy_pairfile.json";
pub const START_WAIT_HINT: Duration = Duration::from_secs(30);
pub const STOP_WAIT_HINT: Duration = Duration::from_secs(15);
pub const ACCEPT_RUNNING: u32 = 0x85;
pub const ERROR_CALL_NOT_IMPLEMENTED: u32 = 120;
pub const ERROR_SERVICE_SPECIFIC_ERROR: u32 = 1066;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ScmEvent {
    Stop,
    Shutdown,
    SessionChange {
        kind: SessionChangeKind,
        session_id: u32,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ScmState {
    StartPending,
    Running,
    StopPending,
    Stopped,
}

pub fn state_code(state: ScmState) -> u32 {
    match state {
        ScmState::StartPending => 2,
        ScmState::Running => 4,
        ScmState::StopPending => 3,
        ScmState::Stopped => 1,
    }
}

pub fn controls_accepted(state: ScmState) -> u32 {
    if state == ScmState::Running {
        ACCEPT_RUNNING
    } else {
        0
    }
}

/// Returns the event to deliver and the SCM handler's Win32 result code.
pub fn map_control(
    control: u32,
    event_type: u32,
    session_id: Option<u32>,
) -> (Option<ScmEvent>, u32) {
    match control {
        1 => (Some(ScmEvent::Stop), 0),
        5 => (Some(ScmEvent::Shutdown), 0),
        14 => (
            session_id.map(|session_id| ScmEvent::SessionChange {
                kind: SessionChangeKind::from_wparam(event_type),
                session_id,
            }),
            0,
        ),
        4 => (None, 0),
        _ => (None, ERROR_CALL_NOT_IMPLEMENTED),
    }
}

/// Returns (dwWin32ExitCode, dwServiceSpecificExitCode).
pub fn exit_codes(code: u32) -> (u32, u32) {
    if code == 0 {
        (0, 0)
    } else {
        (ERROR_SERVICE_SPECIFIC_ERROR, code)
    }
}

pub trait StatusSink: Send + Sync {
    fn running(&self);
    fn stopping(&self, wait_hint: Duration);
}

pub struct NoopStatus;

impl StatusSink for NoopStatus {
    fn running(&self) {}
    fn stopping(&self, _wait_hint: Duration) {}
}

pub fn is_image(process_name: &str, image: &str) -> bool {
    fn basename(name: &str) -> &str {
        name.rsplit(['\\', '/']).next().unwrap_or("")
    }
    basename(process_name).eq_ignore_ascii_case(basename(image))
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TrayDecision {
    Launch,
    AlreadyRunning,
    NoUser,
    NoSession,
    Missing,
}

pub fn decide_tray(
    console: Option<u32>,
    has_user: bool,
    running: bool,
    exe_exists: bool,
) -> TrayDecision {
    if !exe_exists {
        TrayDecision::Missing
    } else if console.is_none() {
        TrayDecision::NoSession
    } else if !has_user {
        TrayDecision::NoUser
    } else if running {
        TrayDecision::AlreadyRunning
    } else {
        TrayDecision::Launch
    }
}

fn is_absolute_windows_path(value: &str) -> bool {
    let bytes = value.as_bytes();
    if bytes.len() >= 3
        && bytes[0].is_ascii_alphabetic()
        && bytes[1] == b':'
        && matches!(bytes[2], b'\\' | b'/')
    {
        return true;
    }
    value.strip_prefix(r"\\").is_some_and(|unc| {
        let mut parts = unc.split(['\\', '/']);
        parts.next().is_some_and(|server| !server.is_empty())
            && parts.next().is_some_and(|share| !share.is_empty())
    })
}

/// Uses Windows string concatenation even when evaluated on Linux.
pub fn downloads_dir(public: Option<&str>) -> PathBuf {
    let public = public
        .filter(|value| is_absolute_windows_path(value))
        .unwrap_or(r"C:\Users\Public");
    let public = public.trim_end_matches('\\');
    PathBuf::from(format!(r"{public}\Downloads"))
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployValues {
    pub pair_file: String,
    pub permission: Option<String>,
    pub label: Option<String>,
    /// Registry FILETIME, deliberately kept in its original units for hashing.
    pub stamp: u64,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct DeployPairFile {
    pub dir: PathBuf,
    pub file_name: Option<String>,
    pub permission: Permission,
    pub label: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum DeployError {
    #[error("pair file path must be Windows absolute")]
    NotAbsolute,
    #[error("invalid pair file permission: {0}")]
    BadPermission(String),
    #[error("pair file label exceeds 64 characters")]
    LabelTooLong,
    #[error("invalid pair file name")]
    BadFileName,
}

pub fn parse_deploy(values: &DeployValues) -> Result<DeployPairFile, DeployError> {
    let path = values.pair_file.trim().trim_end_matches(['\\', '/']);
    if !is_absolute_windows_path(path) {
        return Err(DeployError::NotAbsolute);
    }
    let (dir, file_name) = if path.to_ascii_lowercase().ends_with(".rspair") {
        let split = path.rfind(['\\', '/']).ok_or(DeployError::BadFileName)?;
        let name = &path[split + 1..];
        if name.is_empty()
            || name
                .chars()
                .any(|c| c.is_control() || ":*?\"<>|".contains(c))
        {
            return Err(DeployError::BadFileName);
        }
        // Preserve the separator for a drive root (C:\file.rspair).
        let parent_end = if split == 2 && path.as_bytes()[1] == b':' {
            3
        } else {
            split
        };
        (PathBuf::from(&path[..parent_end]), Some(name.to_owned()))
    } else {
        (PathBuf::from(path), None)
    };
    let permission = match values.permission.as_deref().unwrap_or("") {
        "" | "control" => Permission::Control,
        "view_only" => Permission::ViewOnly,
        "unattended" => Permission::Unattended,
        other => return Err(DeployError::BadPermission(other.to_owned())),
    };
    let label = values
        .label
        .as_ref()
        .filter(|label| !label.is_empty())
        .cloned();
    if label
        .as_ref()
        .is_some_and(|label| label.chars().count() > 64)
    {
        return Err(DeployError::LabelTooLong);
    }
    Ok(DeployPairFile {
        dir,
        file_name,
        permission,
        label,
    })
}

pub fn deploy_fingerprint(values: &DeployValues) -> String {
    let mut digest = Sha256::new();
    for field in [
        &values.pair_file,
        values.permission.as_deref().unwrap_or(""),
        values.label.as_deref().unwrap_or(""),
    ] {
        digest.update(field.as_bytes());
        digest.update([0]);
    }
    digest.update(values.stamp.to_le_bytes());
    data_encoding::HEXLOWER.encode(&digest.finalize())
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
pub struct DeployMarker {
    pub fingerprint: String,
    pub file_id: String,
    pub path: String,
    pub exported_at_us: u64,
}

pub fn load_marker(path: &Path) -> Option<DeployMarker> {
    load_json(path)
}

pub fn save_marker(path: &Path, marker: &DeployMarker) -> std::io::Result<()> {
    save_json(path, marker)
}

pub fn needs_export(values: Option<&DeployValues>, marker: Option<&DeployMarker>) -> bool {
    values.is_some_and(|values| {
        marker.is_none_or(|marker| marker.fingerprint != deploy_fingerprint(values))
    })
}

pub(crate) fn load_json<T: serde::de::DeserializeOwned>(path: &Path) -> Option<T> {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => return None,
        Err(error) => {
            tracing::warn!(%error, "cannot read service metadata");
            return None;
        }
    };
    match serde_json::from_slice(&bytes) {
        Ok(value) => Some(value),
        Err(_) => {
            // Do not log parser errors: they can quote the file's contents.
            tracing::warn!("invalid service metadata JSON");
            None
        }
    }
}

/// The service owns these files and serializes writes; use a sibling temporary
/// file so replacement stays on the same filesystem on both Windows and Unix.
pub(crate) fn save_json<T: Serialize>(path: &Path, value: &T) -> std::io::Result<()> {
    let bytes = serde_json::to_vec(value).map_err(std::io::Error::other)?;
    let mut temporary = path.as_os_str().to_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);
    let result = fs::write(&temporary, bytes).and_then(|()| fs::rename(&temporary, path));
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result
}
