//! Pure logic for the xdg-desktop-portal capture flow.
//!
//! D-Bus transport deliberately lives outside this module.  The transport
//! layer turns [`Call`] values into method calls and turns `Request::Response`
//! values into [`RVal`] values before feeding them back to [`PortalFlow`].

use std::fs::{self, File, OpenOptions};
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};

use serde::Deserialize;
use thiserror::Error;
use tracing::warn;

pub const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
pub const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
pub const IFACE_RD: &str = "org.freedesktop.portal.RemoteDesktop";
pub const IFACE_SC: &str = "org.freedesktop.portal.ScreenCast";
pub const IFACE_REQUEST: &str = "org.freedesktop.portal.Request";
pub const IFACE_SESSION: &str = "org.freedesktop.portal.Session";

pub mod device {
    pub const KEYBOARD: u32 = 1;
    pub const POINTER: u32 = 2;
    pub const TOUCHSCREEN: u32 = 4;
}

pub mod source {
    pub const MONITOR: u32 = 1;
    pub const WINDOW: u32 = 2;
    pub const VIRTUAL: u32 = 4;
}

pub mod cursor {
    pub const HIDDEN: u32 = 1;
    pub const EMBEDDED: u32 = 2;
    pub const METADATA: u32 = 4;
}

pub mod persist {
    pub const NONE: u32 = 0;
    pub const TRANSIENT: u32 = 1;
    pub const PERSISTENT: u32 = 2;
}

pub mod response {
    pub const SUCCESS: u32 = 0;
    pub const CANCELLED: u32 = 1;
    pub const OTHER: u32 = 2;
}

/// Capabilities read from the two portal interfaces.
///
/// An absent interface is represented by a `None` version.  The bit masks are
/// left as zero when the corresponding property is unavailable.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortalCaps {
    pub rd_version: Option<u32>,
    pub rd_devices: u32,
    pub sc_version: Option<u32>,
    pub sc_sources: u32,
    pub sc_cursor_modes: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PortalKind {
    RemoteDesktop,
    ScreenCast,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalPlan {
    pub kind: PortalKind,
    pub devices: u32,
    pub cursor_mode: u32,
    pub persist: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum PlanError {
    #[error("no ScreenCast portal")]
    NoScreenCast,
    #[error("portal cannot share monitors")]
    NoMonitorSource,
}

/// Select the portal interface and options for the requested session.
pub fn plan(caps: &PortalCaps, want_input: bool) -> Result<PortalPlan, PlanError> {
    let sc_version = caps.sc_version.ok_or(PlanError::NoScreenCast)?;
    if caps.sc_sources & source::MONITOR == 0 {
        return Err(PlanError::NoMonitorSource);
    }

    let rd_devices = caps.rd_devices & (device::KEYBOARD | device::POINTER);
    let kind = if want_input && caps.rd_version.is_some() && rd_devices != 0 {
        PortalKind::RemoteDesktop
    } else {
        PortalKind::ScreenCast
    };

    let cursor_mode = if sc_version >= 2 {
        [cursor::METADATA, cursor::EMBEDDED, cursor::HIDDEN]
            .into_iter()
            .find(|mode| caps.sc_cursor_modes & mode != 0)
            .unwrap_or(0)
    } else {
        0
    };

    let persist = match kind {
        PortalKind::RemoteDesktop => caps.rd_version.is_some_and(|version| version >= 2),
        PortalKind::ScreenCast => sc_version >= 4,
    };

    Ok(PortalPlan {
        kind,
        devices: if kind == PortalKind::RemoteDesktop {
            rd_devices
        } else {
            0
        },
        cursor_mode,
        persist,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Method {
    RdCreateSession,
    ScCreateSession,
    SelectDevices,
    SelectSources,
    RdStart,
    ScStart,
}

impl Method {
    pub fn interface(self) -> &'static str {
        match self {
            Self::RdCreateSession | Self::SelectDevices | Self::RdStart => IFACE_RD,
            Self::ScCreateSession | Self::SelectSources | Self::ScStart => IFACE_SC,
        }
    }

    pub fn member(self) -> &'static str {
        match self {
            Self::RdCreateSession | Self::ScCreateSession => "CreateSession",
            Self::SelectDevices => "SelectDevices",
            Self::SelectSources => "SelectSources",
            Self::RdStart | Self::ScStart => "Start",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum OptValue {
    Str(String),
    U32(u32),
    Bool(bool),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Call {
    pub method: Method,
    /// All methods except CreateSession have a session object path.
    pub session: Option<String>,
    /// Options in insertion order.  The transport layer adds the `Start`
    /// parent-window argument separately (it is always the empty string).
    pub options: Vec<(&'static str, OptValue)>,
    /// Request object path to subscribe to before making the call.
    pub request_path: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum RVal {
    Str(String),
    U32(u32),
    Bool(bool),
    Streams(Vec<StreamInfo>),
    Other,
}

#[derive(Debug, Clone, PartialEq, Eq, Default)]
pub struct StreamInfo {
    pub node_id: u32,
    pub id: Option<String>,
    pub position: Option<(i32, i32)>,
    pub size: Option<(i32, i32)>,
    pub source_type: Option<u32>,
    pub mapping_id: Option<String>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StartResults {
    pub kind: PortalKind,
    pub session: String,
    pub devices: u32,
    pub streams: Vec<StreamInfo>,
    pub restore_token: Option<String>,
    pub clipboard_enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FlowState {
    Continue,
    Done(StartResults),
}

#[derive(Debug, Clone, PartialEq, Eq, Error)]
pub enum FlowError {
    #[error("user cancelled the portal dialog")]
    Cancelled,
    #[error("portal request failed with code {0}")]
    Failed(u32),
    #[error("portal did not return a session handle")]
    MissingSession,
    #[error("portal returned no streams")]
    NoStreams,
    #[error("unexpected response")]
    Unexpected,
}

const METHODS_RD: [Method; 4] = [
    Method::RdCreateSession,
    Method::SelectDevices,
    Method::SelectSources,
    Method::RdStart,
];
const METHODS_SC: [Method; 3] = [
    Method::ScCreateSession,
    Method::SelectSources,
    Method::ScStart,
];

/// State machine for the portal calls.  It contains no D-Bus types so it can
/// be exercised on platforms without a portal implementation.
pub struct PortalFlow {
    plan: PortalPlan,
    step: usize,
    session: Option<String>,
    token_prefix: String,
    sender: String,
    restore_token: Option<String>,
    pending: bool,
    terminal: bool,
}

impl PortalFlow {
    /// `sender` is the connection's unique name (for example `":1.42"`).
    /// `token_prefix` is expected to contain only ASCII letters, digits and
    /// underscores.  A restore token is considered only for persistent plans.
    pub fn new(
        plan: PortalPlan,
        sender: &str,
        token_prefix: &str,
        restore_token: Option<String>,
    ) -> Self {
        let use_restore_token = plan.persist;
        Self {
            plan,
            step: 0,
            session: None,
            token_prefix: token_prefix.to_owned(),
            sender: sender.to_owned(),
            restore_token: if use_restore_token {
                restore_token
            } else {
                None
            },
            pending: false,
            terminal: false,
        }
    }

    /// Return the next call, or wait until the previous request has received a
    /// response.
    pub fn next_call(&mut self) -> Option<Call> {
        if self.pending || self.terminal {
            return None;
        }

        let methods = match self.plan.kind {
            PortalKind::RemoteDesktop => &METHODS_RD[..],
            PortalKind::ScreenCast => &METHODS_SC[..],
        };
        let method = methods.get(self.step).copied()?;
        let handle_token = format!("{}{}", self.token_prefix, self.step);
        let mut options = vec![("handle_token", OptValue::Str(handle_token.clone()))];
        let session = if matches!(method, Method::RdCreateSession | Method::ScCreateSession) {
            options.push((
                "session_handle_token",
                OptValue::Str(format!("{}s", self.token_prefix)),
            ));
            None
        } else {
            self.session.clone()
        };

        match method {
            Method::SelectDevices => {
                options.push(("types", OptValue::U32(self.plan.devices)));
                if self.plan.persist {
                    options.push(("persist_mode", OptValue::U32(persist::PERSISTENT)));
                    if let Some(token) = self.restore_token.as_ref() {
                        options.push(("restore_token", OptValue::Str(token.clone())));
                    }
                }
            }
            Method::SelectSources => {
                options.push(("types", OptValue::U32(source::MONITOR)));
                options.push(("multiple", OptValue::Bool(true)));
                if self.plan.cursor_mode != 0 {
                    options.push(("cursor_mode", OptValue::U32(self.plan.cursor_mode)));
                }
                if self.plan.kind == PortalKind::ScreenCast && self.plan.persist {
                    options.push(("persist_mode", OptValue::U32(persist::PERSISTENT)));
                    if let Some(token) = self.restore_token.as_ref() {
                        options.push(("restore_token", OptValue::Str(token.clone())));
                    }
                }
            }
            Method::RdCreateSession
            | Method::ScCreateSession
            | Method::RdStart
            | Method::ScStart => {}
        }

        self.pending = true;
        Some(Call {
            method,
            session,
            options,
            request_path: request_path(&self.sender, &handle_token),
        })
    }

    /// Feed the response for the call returned by [`Self::next_call`].
    pub fn on_response(
        &mut self,
        code: u32,
        results: &[(String, RVal)],
    ) -> Result<FlowState, FlowError> {
        if !self.pending || self.terminal {
            return Err(FlowError::Unexpected);
        }
        self.pending = false;

        if code != response::SUCCESS {
            self.terminal = true;
            return Err(if code == response::CANCELLED {
                FlowError::Cancelled
            } else {
                FlowError::Failed(code)
            });
        }

        let methods = match self.plan.kind {
            PortalKind::RemoteDesktop => &METHODS_RD[..],
            PortalKind::ScreenCast => &METHODS_SC[..],
        };
        let Some(method) = methods.get(self.step).copied() else {
            self.terminal = true;
            return Err(FlowError::Unexpected);
        };

        match method {
            Method::RdCreateSession | Method::ScCreateSession => {
                let Some(RVal::Str(session)) = result_value(results, "session_handle") else {
                    self.terminal = true;
                    return Err(FlowError::MissingSession);
                };
                if session.is_empty() {
                    self.terminal = true;
                    return Err(FlowError::MissingSession);
                }
                self.session = Some(session.clone());
                self.step += 1;
                Ok(FlowState::Continue)
            }
            Method::SelectDevices | Method::SelectSources => {
                self.step += 1;
                Ok(FlowState::Continue)
            }
            Method::RdStart | Method::ScStart => {
                let Some(RVal::Streams(streams)) = result_value(results, "streams") else {
                    self.terminal = true;
                    return Err(FlowError::NoStreams);
                };
                if streams.is_empty() {
                    self.terminal = true;
                    return Err(FlowError::NoStreams);
                }
                let Some(session) = self.session.clone() else {
                    self.terminal = true;
                    return Err(FlowError::Unexpected);
                };
                self.terminal = true;
                Ok(FlowState::Done(StartResults {
                    kind: self.plan.kind,
                    session,
                    devices: if self.plan.kind == PortalKind::RemoteDesktop {
                        result_u32(results, "devices").unwrap_or(0)
                    } else {
                        0
                    },
                    streams: streams.clone(),
                    restore_token: result_string(results, "restore_token"),
                    clipboard_enabled: result_bool(results, "clipboard_enabled").unwrap_or(false),
                }))
            }
        }
    }
}

fn result_value<'a>(results: &'a [(String, RVal)], key: &str) -> Option<&'a RVal> {
    results
        .iter()
        .find_map(|(name, value)| (name == key).then_some(value))
}

fn result_u32(results: &[(String, RVal)], key: &str) -> Option<u32> {
    match result_value(results, key) {
        Some(RVal::U32(value)) => Some(*value),
        _ => None,
    }
}

fn result_string(results: &[(String, RVal)], key: &str) -> Option<String> {
    match result_value(results, key) {
        Some(RVal::Str(value)) => Some(value.clone()),
        _ => None,
    }
}

fn result_bool(results: &[(String, RVal)], key: &str) -> Option<bool> {
    match result_value(results, key) {
        Some(RVal::Bool(value)) => Some(*value),
        _ => None,
    }
}

pub fn request_path(sender: &str, token: &str) -> String {
    format!(
        "{PORTAL_PATH}/request/{}/{}",
        sender_component(sender),
        token
    )
}

pub fn session_path(sender: &str, token: &str) -> String {
    format!(
        "{PORTAL_PATH}/session/{}/{}",
        sender_component(sender),
        token
    )
}

fn sender_component(sender: &str) -> String {
    sender.strip_prefix(':').unwrap_or(sender).replace('.', "_")
}

pub fn valid_token(token: &str) -> bool {
    !token.is_empty()
        && token
            .bytes()
            .all(|byte| byte.is_ascii_alphanumeric() || byte == b'_')
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct StreamDisplay {
    pub id: u32,
    pub node_id: u32,
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
    pub primary: bool,
}

pub fn stream_displays(streams: &[StreamInfo]) -> Vec<StreamDisplay> {
    let mut displays = Vec::new();
    let mut sequential_x: i64 = 0;

    for (index, stream) in streams.iter().enumerate() {
        if stream
            .source_type
            .is_some_and(|source_type| source_type & source::MONITOR == 0)
        {
            continue;
        }

        let (width, height) = stream
            .size
            .map(|(width, height)| (width.max(0) as u32, height.max(0) as u32))
            .unwrap_or((0, 0));
        let (x, y) = stream
            .position
            .unwrap_or_else(|| (saturating_i64_to_i32(sequential_x), 0));
        sequential_x = sequential_x.saturating_add(i64::from(width));

        displays.push(StreamDisplay {
            id: index as u32 + 1,
            node_id: stream.node_id,
            x,
            y,
            width,
            height,
            primary: false,
        });
    }

    let primary_index = displays
        .iter()
        .position(|display| display.x == 0 && display.y == 0)
        .unwrap_or(0);
    if let Some(primary) = displays.get_mut(primary_index) {
        primary.primary = true;
    }
    displays
}

fn saturating_i64_to_i32(value: i64) -> i32 {
    value.clamp(i64::from(i32::MIN), i64::from(i32::MAX)) as i32
}

pub const RESTORE_FILE: &str = "portal-restore.json";

pub struct RestoreTokenStore {
    path: PathBuf,
}

impl RestoreTokenStore {
    pub fn new(state_dir: &Path) -> Self {
        Self {
            path: state_dir.join(RESTORE_FILE),
        }
    }

    pub fn load(&self, kind: PortalKind) -> Option<String> {
        let bytes = match fs::read(&self.path) {
            Ok(bytes) => bytes,
            Err(error) if error.kind() == io::ErrorKind::NotFound => return None,
            Err(error) => {
                warn!(error = %error, "failed to read portal restore token");
                return None;
            }
        };

        let record: RestoreRecord = match serde_json::from_slice(&bytes) {
            Ok(record) => record,
            Err(error) => {
                warn!(error = %error, "failed to parse portal restore token");
                return None;
            }
        };
        if record.version != 1 || record.kind != kind_name(kind) {
            return None;
        }
        Some(record.token)
    }

    pub fn save(&self, kind: PortalKind, token: &str) -> io::Result<()> {
        let Some(parent) = self.path.parent() else {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "restore token path has no parent",
            ));
        };

        let existed = parent.exists();
        fs::create_dir_all(parent)?;
        if !existed {
            set_mode(parent, 0o700)?;
        }

        let record = RestoreRecord {
            version: 1,
            kind: kind_name(kind).to_owned(),
            token: token.to_owned(),
        };
        let data = serde_json::to_vec(&record).map_err(|error| {
            io::Error::new(
                io::ErrorKind::InvalidData,
                format!("failed to serialize portal restore token: {error}"),
            )
        })?;

        let temp_path = temporary_path(&self.path);
        let write_result = (|| {
            let mut file = open_private_temp(&temp_path)?;
            file.write_all(&data)?;
            file.sync_all()?;
            drop(file);
            fs::rename(&temp_path, &self.path)
        })();
        if write_result.is_err() {
            let _ = fs::remove_file(&temp_path);
        }
        write_result
    }

    pub fn clear(&self) -> io::Result<()> {
        match fs::remove_file(&self.path) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(error),
        }
    }
}

#[derive(Debug, Deserialize, serde::Serialize)]
struct RestoreRecord {
    version: u32,
    kind: String,
    token: String,
}

fn kind_name(kind: PortalKind) -> &'static str {
    match kind {
        PortalKind::RemoteDesktop => "remote_desktop",
        PortalKind::ScreenCast => "screencast",
    }
}

static TEMP_COUNTER: AtomicU64 = AtomicU64::new(0);

fn temporary_path(path: &Path) -> PathBuf {
    let counter = TEMP_COUNTER.fetch_add(1, Ordering::Relaxed);
    let name = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or(RESTORE_FILE);
    path.with_file_name(format!(".{name}.tmp-{}-{counter}", std::process::id()))
}

fn open_private_temp(path: &Path) -> io::Result<File> {
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    options.open(path)
}

#[cfg(unix)]
fn set_mode(path: &Path, mode: u32) -> io::Result<()> {
    use std::os::unix::fs::PermissionsExt;
    fs::set_permissions(path, fs::Permissions::from_mode(mode))
}

#[cfg(not(unix))]
fn set_mode(_path: &Path, _mode: u32) -> io::Result<()> {
    Ok(())
}
