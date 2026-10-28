//! Shared rs-rdp types . Cards must not change this file.

use std::sync::Arc;

use async_trait::async_trait;
use zeroize::Zeroizing;

/// Desktop rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

/// Frame and pointer output. Pixels are RGBA8.
pub trait RdpSink: Send + Sync {
    fn resize(&self, w: u16, h: u16);
    /// Copy `rect` out of the full RGBA8 image (`stride` bytes per row).
    fn update(&self, image: &[u8], stride: usize, rect: Rect);
    /// End of one batch of graphics updates.
    fn commit(&self);
    /// RGBA8, non-premultiplied, w*4 bytes per row.
    fn pointer_bitmap(&self, w: u16, h: u16, hot_x: u16, hot_y: u16, rgba: &[u8]);
    fn pointer_hidden(&self);
    fn pointer_default(&self);
    fn pointer_pos(&self, x: u16, y: u16);
    /// Remote clipboard text (clip.rs, ).
    fn clipboard(&self, text: &str) {
        let _ = text;
    }
    /// Monitor rectangles in desktop pixels, primary first (display.rs, ).
    /// Called once the layout is known and after every change.
    fn monitors(&self, layout: &[Rect]) {
        let _ = layout;
    }
}

/// PCM output for rdpsnd (audio.rs, ). Samples are interleaved signed 16-bit.
pub trait RdpAudioSink: Send + Sync {
    /// Format agreed with the server; called before the first samples and whenever it changes.
    fn format(&self, sample_rate: u32, channels: u16);
    fn samples(&self, pcm: &[i16]);
    /// The server stopped or closed audio; drop anything buffered.
    fn stop(&self);
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MouseButton {
    Left,
    Right,
    Middle,
    X1,
    X2,
}

/// Input already mapped to RDP terms (input.rs / scancode.rs produce it, client.rs sends it).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdpInput {
    Key {
        scancode: u8,
        extended: bool,
        down: bool,
    },
    Unicode {
        code: u16,
        down: bool,
    },
    MouseMove {
        x: u16,
        y: u16,
    },
    MouseButton {
        button: MouseButton,
        down: bool,
        x: u16,
        y: u16,
    },
    /// Wheel rotation in RDP units (120 = one notch), positive = up / right, |units| <= 255.
    Wheel {
        horizontal: bool,
        units: i16,
    },
    /// Release every key and button the client believes is down.
    ReleaseAll,
}

/// What the driver sends into a running connection (client.rs routes it, ).
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum RdpCommand {
    Input(RdpInput),
    /// Phone → remote clipboard text (clip.rs, ).
    Clipboard(String),
    /// New desktop size per monitor from `desktop::desktop_size` (display.rs, ).
    Resize {
        width: u16,
        height: u16,
        scale_percent: u32,
    },
    /// Monitors side by side, 1..=4, each the current monitor size (display.rs, ).
    /// A server without display control ends the connection with `RdpError::Reconnect`.
    Monitors(u8),
}

pub struct RdpCredentials {
    pub user: String,
    pub password: Zeroizing<String>,
    pub domain: Option<String>,
}

/// Certificate decision: SHA-256 of the leaf certificate DER.
#[async_trait]
pub trait CertCheck: Send + Sync {
    async fn check(&self, sha256: [u8; 32]) -> bool;
}

pub struct RdpConfig {
    /// TLS server name / logging; an IP literal is fine (verification is TOFU, not WebPKI).
    pub server_name: String,
    /// Some → offer HYBRID (CredSSP/NTLM) and SSL, the server picks; with SSL the credentials
    /// go into Client Info with autologon. None → SSL only (Windows shows its own logon screen).
    pub credentials: Option<RdpCredentials>,
    /// Size of one monitor.
    pub width: u16,
    pub height: u16,
    /// Desktop scale percent (100 by default,  5).
    pub scale_percent: u32,
    pub client_name: String,
    pub cert: Arc<dyn CertCheck>,
    /// Graphics pipeline (EGFX) with H.264 when the server agrees; false = bitmap codecs only
    /// (gfx.rs, ).
    pub gfx: bool,
    /// Monitors side by side, each `width` × `height`, 1..=4 (display.rs, ;  9).
    pub monitors: u8,
    /// Some → open rdpsnd and play remote audio (audio.rs, ).
    pub audio: Option<Arc<dyn RdpAudioSink>>,
    /// Text clipboard over cliprdr (clip.rs, ).
    pub clipboard: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum RdpError {
    #[error("io: {0}")]
    Io(String),
    #[error("tls: {0}")]
    Tls(String),
    #[error("certificate rejected")]
    CertRejected,
    /// Server requires HYBRID (NLA) but no credentials were given.
    #[error("server requires NLA")]
    NlaRequired,
    /// Server offers only protocols we do not implement (e.g. RDSTLS only).
    #[error("unsupported security: {0}")]
    UnsupportedSecurity(String),
    #[error("authentication failed")]
    AuthFailed,
    #[error("protocol: {0}")]
    Protocol(String),
    /// Server ended the session (disconnect reason text when known).
    #[error("server closed: {0}")]
    Closed(String),
    #[error("cancelled")]
    Cancelled,
    /// `RdpCommand::Monitors` needs a new connection (no display control at runtime):
    /// connect again with `RdpConfig.monitors` set to this count.
    #[error("reconnect with {0} monitors")]
    Reconnect(u8),
}
