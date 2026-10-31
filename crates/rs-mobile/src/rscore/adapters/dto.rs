//! Adapter JSON shapes shared with JS (07 RsCore `adapters`, ).
//! Session ids inside JSON are decimal strings.

use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum AdapterProtocol {
    Vnc,
    Rdp,
}

impl AdapterProtocol {
    pub fn default_port(self) -> u16 {
        match self {
            Self::Vnc => 5900,
            Self::Rdp => 3389,
        }
    }
    pub fn as_str(self) -> &'static str {
        match self {
            Self::Vnc => "vnc",
            Self::Rdp => "rdp",
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterTarget {
    pub protocol: AdapterProtocol,
    /// Directory computer, when opened from one (credential key, logs).
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub host_id: Option<String>,
    /// IP literal or host name, IPv6 without brackets.
    pub addr: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub port: Option<u16>,
}

impl AdapterTarget {
    pub fn port_or_default(&self) -> u16 {
        self.port.unwrap_or(self.protocol.default_port())
    }
    /// Key for saved credentials and certificate pins:
    /// `<protocol>:<hostId>`, or `<protocol>:<addr>:<port>` without a host id.
    pub fn store_key(&self) -> String {
        match &self.host_id {
            Some(id) => format!("{}:{}", self.protocol.as_str(), id),
            None => format!(
                "{}:{}:{}",
                self.protocol.as_str(),
                self.addr,
                self.port_or_default()
            ),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdapterContext {
    LoginWindow,
    NoHost,
    Fallback,
    Manual,
    /// Native-first connect (V10): the app chose the system protocol itself.
    Native,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterOpenOpts {
    /// View size in points; RDP derives its desktop size from it.
    #[serde(default)]
    pub view_width: u32,
    #[serde(default)]
    pub view_height: u32,
    #[serde(default = "default_scale")]
    pub scale: f32,
    /// RDP monitors side by side, 1..=4 ; VNC ignores it.
    #[serde(default = "default_monitors")]
    pub monitors: u8,
    /// Try saved credentials first.
    #[serde(default = "default_true")]
    pub use_saved: bool,
    /// Where the user came from; logged only.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub context: Option<AdapterContext>,
}

fn default_scale() -> f32 {
    1.0
}

fn default_monitors() -> u8 {
    1
}

fn default_true() -> bool {
    true
}

impl Default for AdapterOpenOpts {
    fn default() -> Self {
        Self {
            view_width: 0,
            view_height: 0,
            scale: 1.0,
            monitors: 1,
            use_saved: true,
            context: None,
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum AdapterPhase {
    Connecting,
    NeedsCredentials,
    Authenticating,
    Streaming,
    Ended,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CredField {
    User,
    Password,
    Domain,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterErrorInfo {
    /// AdapterError::code(), e.g. "authFailed".
    pub code: String,
    /// RS-E number (C11.3); 0 for cancelled.
    pub rs_code: u16,
    /// English diagnostic text for logs; never shown verbatim.
    pub message: String,
}

/// How a VNC monitor was guessed from the picture.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum ScreenGuess {
    /// Split along the black gaps between monitors of different sizes.
    BlackGap,
    /// No gaps: a very wide or tall desktop split in halves.
    Aspect,
}

impl ScreenGuess {
    /// Log value.
    pub fn as_str(self) -> &'static str {
        match self {
            Self::BlackGap => "black-gap",
            Self::Aspect => "aspect",
        }
    }
}

/// One remote monitor in framebuffer pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterScreen {
    pub x: u32,
    pub y: u32,
    pub width: u32,
    pub height: u32,
    /// Set when the app guessed this monitor; None = reported by the server.
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub guess: Option<ScreenGuess>,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "type")]
pub enum AdapterEvent {
    #[serde(rename = "adapter.state", rename_all = "camelCase")]
    State {
        session_id: String,
        protocol: AdapterProtocol,
        phase: AdapterPhase,
        #[serde(default, skip_serializing_if = "Vec::is_empty")]
        cred_fields: Vec<CredField>,
        /// needsCredentials after a failed attempt.
        cred_error: bool,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        width: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        height: Option<u32>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        server_name: Option<String>,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        error: Option<AdapterErrorInfo>,
    },
    #[serde(rename = "adapter.certPrompt", rename_all = "camelCase")]
    CertPrompt {
        session_id: String,
        /// New SHA-256 fingerprint, lowercase hex pairs joined by ':'.
        fp: String,
        #[serde(default, skip_serializing_if = "Option::is_none")]
        old_fp: Option<String>,
    },
    #[serde(rename = "adapter.clipboard", rename_all = "camelCase")]
    Clipboard { session_id: String, text: String },
    /// The computer copied something other than text (a screenshot, files).
    #[serde(rename = "adapter.clipboardOther", rename_all = "camelCase")]
    ClipboardOther { session_id: String },
    /// Monitor layout, primary (or, when guessed, top-left) first; sent once
    /// known and after every change.
    #[serde(rename = "adapter.screens", rename_all = "camelCase")]
    Screens {
        session_id: String,
        screens: Vec<AdapterScreen>,
    },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct AdapterSummary {
    pub session_id: String,
    pub target: AdapterTarget,
    pub phase: AdapterPhase,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub width: Option<u32>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub height: Option<u32>,
}

#[derive(Debug, Clone, Default, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct SavedCredentialInfo {
    pub saved: bool,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub user: Option<String>,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub domain: Option<String>,
}
