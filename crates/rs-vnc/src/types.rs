//! Shared rs-vnc types.

/// Largest framebuffer side accepted from ServerInit / DesktopSize.
pub const MAX_FB_SIDE: u16 = 8192;
/// Largest desktop name accepted from ServerInit (bytes).
pub const MAX_NAME_LEN: u32 = 4096;
/// Largest ServerCutText accepted (bytes); bigger ones are read and discarded.
pub const MAX_CUT_TEXT: u32 = 1 << 20;
/// Largest ZRLE rectangle payload accepted (bytes).
pub const MAX_ZRLE_LEN: u32 = 64 << 20;

/// RFC 6143 §7.4 PIXEL_FORMAT. Only true-colour formats are supported.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct PixelFormat {
    pub bits_per_pixel: u8,
    pub depth: u8,
    pub big_endian: bool,
    pub true_colour: bool,
    pub red_max: u16,
    pub green_max: u16,
    pub blue_max: u16,
    pub red_shift: u8,
    pub green_shift: u8,
    pub blue_shift: u8,
}

impl PixelFormat {
    /// The format this client requests: 32bpp, depth 24, little-endian,
    /// R<<16 G<<8 B<<0, i.e. memory bytes B, G, R, X.
    pub const BGRX32: PixelFormat = PixelFormat {
        bits_per_pixel: 32,
        depth: 24,
        big_endian: false,
        true_colour: true,
        red_max: 255,
        green_max: 255,
        blue_max: 255,
        red_shift: 16,
        green_shift: 8,
        blue_shift: 0,
    };

    pub fn bytes_per_pixel(&self) -> usize {
        (self.bits_per_pixel as usize).div_ceil(8)
    }
}

/// Framebuffer rectangle in pixels.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct Rect {
    pub x: u16,
    pub y: u16,
    pub w: u16,
    pub h: u16,
}

impl Rect {
    pub const fn new(x: u16, y: u16, w: u16, h: u16) -> Self {
        Rect { x, y, w, h }
    }
    pub fn is_empty(&self) -> bool {
        self.w == 0 || self.h == 0
    }
    /// True when the rectangle lies inside a `fb_w` x `fb_h` framebuffer.
    pub fn fits(&self, fb_w: u16, fb_h: u16) -> bool {
        u32::from(self.x) + u32::from(self.w) <= u32::from(fb_w)
            && u32::from(self.y) + u32::from(self.h) <= u32::from(fb_h)
    }
}

/// Encoding numbers (RFC 6143 §7.7 and the community registry).
pub mod encoding {
    pub const RAW: i32 = 0;
    pub const COPY_RECT: i32 = 1;
    pub const ZRLE: i32 = 16;
    pub const CURSOR: i32 = -239;
    pub const DESKTOP_SIZE: i32 = -223;
    pub const LAST_RECT: i32 = -224;
    pub const POINTER_POS: i32 = -232;
    pub const EXTENDED_DESKTOP_SIZE: i32 = -308;
    /// Order sent in SetEncodings.
    pub const PREFERRED: [i32; 8] = [
        ZRLE,
        COPY_RECT,
        RAW,
        CURSOR,
        DESKTOP_SIZE,
        EXTENDED_DESKTOP_SIZE,
        LAST_RECT,
        POINTER_POS,
    ];
}

/// Security type numbers.
pub mod security {
    pub const INVALID: u8 = 0;
    pub const NONE: u8 = 1;
    pub const VNC_AUTH: u8 = 2;
    pub const ARD: u8 = 30;
    /// Client preference, best first.
    pub const PREFERRED: [u8; 3] = [NONE, ARD, VNC_AUTH];
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum VncError {
    #[error("io: {0}")]
    Io(String),
    /// Peer closed the stream (EOF) or sent a close reason.
    #[error("server closed: {0}")]
    Closed(String),
    #[error("protocol: {0}")]
    Protocol(String),
    #[error("unsupported protocol version {0:?}")]
    UnsupportedVersion(String),
    /// None of the offered security types is supported (the offered list is kept).
    #[error("no supported security type (offered {0:?})")]
    UnsupportedSecurity(Vec<u8>),
    /// SecurityResult failed; the reason string from the server when present.
    #[error("authentication failed: {0}")]
    AuthFailed(String),
    /// Server refused before security negotiation (security-type count 0 + reason).
    #[error("refused by server: {0}")]
    Refused(String),
    #[error("cancelled")]
    Cancelled,
}

impl From<std::io::Error> for VncError {
    fn from(e: std::io::Error) -> Self {
        if e.kind() == std::io::ErrorKind::UnexpectedEof {
            VncError::Closed("eof".into())
        } else {
            VncError::Io(e.to_string())
        }
    }
}
