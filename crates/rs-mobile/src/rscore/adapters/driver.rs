//! Protocol driver interface . VncDriver (vnc.rs) and RdpDriver
//! (rdp.rs) implement it; session.rs drives it. Cards must not change this file.

use std::sync::Arc;

use async_trait::async_trait;
use rsp_core::hid::InputMsg;
use rsp_transport::net::NetStream;
use tokio::sync::{mpsc, watch};
use zeroize::Zeroizing;

use super::dto::{AdapterProtocol, AdapterScreen, CredField};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcPixels {
    Bgra8,
    Rgba8,
}

/// Frame output (RawFrame in rawframe.rs). Coordinates are framebuffer pixels;
/// implementations clip anything outside the current size. One writer (the
/// driver task); rs_raw_copy reads from other threads under the
/// implementation's lock.
pub trait FrameSink: Send + Sync {
    /// New size; contents become black (0, 0, 0, 255) and the next
    /// rs_raw_copy is a full copy.
    fn resize(&self, width: u32, height: u32);
    #[allow(clippy::too_many_arguments)]
    fn put(&self, x: u32, y: u32, w: u32, h: u32, src: &[u8], stride: usize, pix: SrcPixels);
    fn fill(&self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]);
    /// Copy inside the framebuffer; source and destination may overlap.
    fn copy_within(&self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32);
    /// End of a batch: seq + 1 and notify the native consumer. No-op without writes.
    fn commit(&self);
    /// RGBA8 non-premultiplied, w*4 bytes per row; w == 0 || h == 0 hides the cursor.
    fn cursor_shape(&self, w: u32, h: u32, hot_x: u32, hot_y: u32, rgba: &[u8]);
    /// Server-reported pointer position in pixels.
    fn cursor_pos(&self, x: u32, y: u32);
}

pub struct Credentials {
    pub user: Option<String>,
    pub password: Zeroizing<String>,
    pub domain: Option<String>,
}

/// PCM playout for an adapter session . Samples are interleaved signed 16-bit.
pub trait PcmSink: Send + Sync {
    fn format(&self, sample_rate: u32, channels: u16);
    fn samples(&self, pcm: &[i16]);
    /// Remote audio stopped; drop anything buffered.
    fn stop(&self);
}

/// What a driver asks of the session layer (session.rs implements it; driver
/// tests use fakes).
#[async_trait]
pub trait DriverHost: Send + Sync {
    /// Latest visibility, including changes made during connection/authentication.
    fn background(&self) -> watch::Receiver<bool> {
        watch::channel(false).1
    }
    fn session_id(&self) -> u64 {
        0
    }
    /// First call with use_saved and saved credentials → returns them without
    /// asking. Otherwise emits adapter.state needsCredentials (credError =
    /// failed_before) and waits for adapters_submit_credentials. None =
    /// cancelled by the user or the session closed.
    async fn credentials(&self, fields: &[CredField], failed_before: bool) -> Option<Credentials>;
    /// Authentication succeeded: persist the last submitted credentials if the
    /// user asked to remember them.
    fn auth_succeeded(&self);
    fn authenticating(&self);
    fn streaming(&self, width: u32, height: u32, server_name: &str);
    /// Remote clipboard text (UTF-8).
    fn clipboard(&self, text: &str);
    /// The remote clipboard now holds something other than text → adapter.clipboardOther.
    fn clipboard_other(&self) {}
    /// RDP certificate: SHA-256 of the leaf certificate DER as 64 lowercase hex
    /// chars. true = continue.
    async fn confirm_cert(&self, fp_hex: &str) -> bool;
    /// Remote monitors in framebuffer pixels, primary first → adapter.screens
    /// (session.rs, ).
    fn screens(&self, screens: &[AdapterScreen]) {
        let _ = screens;
    }
    /// Audio output for this session; None = do not open remote audio (
    /// returns Some).
    fn audio(&self) -> Option<Arc<dyn PcmSink>> {
        None
    }
}

/// Opens one TCP connection (session.rs: Core::net().dial_tcp_host with a
/// 15 s timeout). Drivers dial again for each authentication retry.
#[async_trait]
pub trait Dial: Send + Sync {
    async fn dial(&self) -> Result<NetStream, AdapterError>;
}

pub struct DriverParams {
    pub dial: Arc<dyn Dial>,
    /// TLS SNI / logs: the target addr (IPv6 without brackets).
    pub server_name: String,
    pub view_width: u32,
    pub view_height: u32,
    pub scale: f32,
    /// RDP monitors (AdapterOpenOpts.monitors clamped to 1..=4).
    pub monitors: u8,
}

/// No Debug: TypeSecret carries a password.
pub enum DriverInput {
    Hid(InputMsg),
    /// Phone → remote clipboard ("send clipboard").
    Clipboard(String),
    /// "Type password": one key press/release per char, no Enter.
    TypeSecret(Zeroizing<String>),
    /// RDP: Ctrl+Alt+End (secure attention on the remote side).
    /// VNC: Ctrl+Alt+Delete pressed and released.
    CtrlAltDel,
    /// View size changed (adapters.resize). RDP: displaycontrol ;
    /// VNC: ignored.
    Resize {
        view_width: u32,
        view_height: u32,
        scale: f32,
    },
    /// RDP: monitors side by side, 1..=4 (adapters.setMonitors), over
    /// displaycontrol or by reconnecting with the same credentials; VNC: ignored.
    Monitors(u8),
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum AdapterError {
    /// TCP connect refused: the computer is up but nothing listens on the
    /// port (Screen Sharing / Remote Desktop is off) → RS-E 205.
    #[error("connection refused")]
    Refused,
    /// Connect timed out, host / network unreachable, address not available
    /// or name not resolved → RS-E 200.
    #[error("connection timed out or unreachable")]
    Timeout,
    #[error("authentication failed")]
    AuthFailed,
    #[error("unsupported security: {0}")]
    UnsupportedSecurity(String),
    #[error("server requires network level authentication")]
    NlaRequired,
    #[error("server certificate rejected")]
    CertRejected,
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("server closed: {0}")]
    ServerClosed(String),
    #[error("io: {0}")]
    Io(String),
    #[error("cancelled")]
    Cancelled,
}

impl AdapterError {
    /// JS error code (i18n `adapters.errors.<code>`).
    pub fn code(&self) -> &'static str {
        match self {
            Self::Refused => "refused",
            Self::Timeout => "timeout",
            Self::AuthFailed => "authFailed",
            Self::UnsupportedSecurity(_) => "unsupportedSecurity",
            Self::NlaRequired => "nlaRequired",
            Self::CertRejected => "certRejected",
            Self::Protocol(_) => "protocol",
            Self::ServerClosed(_) => "serverClosed",
            Self::Io(_) => "io",
            Self::Cancelled => "cancelled",
        }
    }

    /// RS-E number (10-appendix, C11.3); 0 = no error page.
    pub fn rs_code(&self) -> u16 {
        match self {
            Self::Refused => rs_e::SERVICE_OFF,
            Self::Timeout => rs_e::UNREACHABLE,
            Self::AuthFailed => rs_e::AUTH_FAILED,
            Self::UnsupportedSecurity(_) | Self::NlaRequired => rs_e::UNSUPPORTED,
            Self::CertRejected => rs_e::CERT_REJECTED,
            Self::Protocol(_) | Self::ServerClosed(_) | Self::Io(_) => rs_e::BROKEN,
            Self::Cancelled => 0,
        }
    }
}

/// RS-E numbers for adapter errors (10-appendix, C11.3). The scheduler
/// allocates them ) and replaces 200..205 when landing
/// C3. Cards and tests use these names, never literal numbers.
pub mod rs_e {
    /// Host / network unreachable, name not resolved, or the 15 s connect timeout.
    pub const UNREACHABLE: u16 = 200;
    /// Authentication failed three times.
    pub const AUTH_FAILED: u16 = 201;
    /// Only unsupported security types / protocols offered (including NLA required).
    pub const UNSUPPORTED: u16 = 202;
    /// The changed RDP certificate was not trusted.
    pub const CERT_REJECTED: u16 = 203;
    /// Connection broken: server closed, protocol error, network error.
    pub const BROKEN: u16 = 204;
    /// Connect refused: the computer is up but Screen Sharing / Remote Desktop is off.
    pub const SERVICE_OFF: u16 = 205;
}

#[async_trait]
pub trait ProtocolDriver: Send + Sync {
    fn protocol(&self) -> AdapterProtocol;
    /// Whole session: connect, authenticate (with retries), stream, take input.
    /// Returns Ok(()) when `input` closes (the user closed the session).
    async fn run(
        &self,
        params: DriverParams,
        host: Arc<dyn DriverHost>,
        sink: Arc<dyn FrameSink>,
        input: mpsc::Receiver<DriverInput>,
    ) -> Result<(), AdapterError>;
}
