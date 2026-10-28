//! RFB message codecs.

use tokio::io::{AsyncRead, AsyncReadExt};

use crate::types::{
    security, PixelFormat, Rect, VncError, MAX_CUT_TEXT, MAX_FB_SIDE, MAX_NAME_LEN,
};

const MAX_ENCODINGS: usize = 64;

/// Supported RFB handshake versions, in negotiation order.
#[derive(Debug, Clone, Copy, PartialEq, Eq, PartialOrd, Ord)]
pub enum Version {
    /// RFB 3.3, including servers advertising 3.4 through 3.6.
    V3_3,
    /// RFB 3.7.
    V3_7,
    /// RFB 3.8, including Apple's 3.889 banner.
    V3_8,
}

/// Parse a 12-byte server banner and select the highest compatible version.
pub fn parse_server_version(b: &[u8; 12]) -> Result<Version, VncError> {
    if &b[..4] != b"RFB "
        || b[7] != b'.'
        || b[11] != b'\n'
        || !b[4..7].iter().all(u8::is_ascii_digit)
        || !b[8..11].iter().all(u8::is_ascii_digit)
    {
        return Err(VncError::Protocol(format!(
            "bad version {:?}",
            String::from_utf8_lossy(b)
        )));
    }

    let number = |digits: &[u8]| {
        digits
            .iter()
            .fold(0u16, |n, digit| n * 10 + u16::from(*digit - b'0'))
    };
    match (number(&b[4..7]), number(&b[8..11])) {
        (3, 3..=6) => Ok(Version::V3_3),
        (3, 7) => Ok(Version::V3_7),
        (3, 8..) | (4.., _) => Ok(Version::V3_8),
        _ => Err(VncError::UnsupportedVersion(
            String::from_utf8_lossy(&b[..11]).into_owned(),
        )),
    }
}

/// Encode the client's canonical 12-byte version banner.
pub fn client_version_bytes(v: Version) -> [u8; 12] {
    match v {
        Version::V3_3 => *b"RFB 003.003\n",
        Version::V3_7 => *b"RFB 003.007\n",
        Version::V3_8 => *b"RFB 003.008\n",
    }
}

/// Read exactly one server version banner.
pub async fn read_version<R: AsyncRead + Unpin>(r: &mut R) -> Result<Version, VncError> {
    parse_server_version(&read_array(r).await?)
}

/// Read the offered security types or the server's bounded refusal reason.
pub async fn read_security_types<R: AsyncRead + Unpin>(
    r: &mut R,
    v: Version,
) -> Result<Vec<u8>, VncError> {
    if v == Version::V3_3 {
        let t = r.read_u32().await?;
        if t == 0 {
            return Err(VncError::Refused(read_reason(r).await?));
        }
        let t = u8::try_from(t)
            .map_err(|_| VncError::Protocol(format!("invalid security type {t}")))?;
        return Ok(vec![t]);
    }

    let count = r.read_u8().await?;
    if count == 0 {
        return Err(VncError::Refused(read_reason(r).await?));
    }
    let mut types = vec![0; usize::from(count)];
    r.read_exact(&mut types).await?;
    Ok(types)
}

/// Choose the first offered type in the contract's NONE, ARD, VNC_AUTH order.
pub fn choose_security(offered: &[u8]) -> Option<u8> {
    security::PREFERRED
        .iter()
        .copied()
        .find(|t| offered.contains(t))
}

/// Report whether this version and security type include a SecurityResult.
pub fn needs_security_result(v: Version, sec: u8) -> bool {
    v == Version::V3_8 || sec != security::NONE
}

/// Read SecurityResult, including a failure reason only for RFB 3.8.
pub async fn read_security_result<R: AsyncRead + Unpin>(
    r: &mut R,
    v: Version,
) -> Result<(), VncError> {
    if r.read_u32().await? == 0 {
        return Ok(());
    }
    let reason = if v == Version::V3_8 {
        read_reason(r).await?
    } else {
        String::new()
    };
    Err(VncError::AuthFailed(reason))
}

/// Read a u32-length reason, capped at MAX_NAME_LEN and decoded as lossy UTF-8.
pub async fn read_reason<R: AsyncRead + Unpin>(r: &mut R) -> Result<String, VncError> {
    read_string(r, "reason").await
}

/// Server framebuffer dimensions, native pixel format, and desktop name.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ServerInit {
    /// Framebuffer width in pixels.
    pub width: u16,
    /// Framebuffer height in pixels.
    pub height: u16,
    /// Server's native pixel format.
    pub pixel_format: PixelFormat,
    /// Desktop name, decoded as lossy UTF-8.
    pub name: String,
}

/// Encode whether the server should share the desktop with other clients.
pub fn client_init(shared: bool) -> [u8; 1] {
    [u8::from(shared)]
}

/// Read ServerInit, rejecting invalid dimensions and oversized names before allocation.
pub async fn read_server_init<R: AsyncRead + Unpin>(r: &mut R) -> Result<ServerInit, VncError> {
    let width = r.read_u16().await?;
    let height = r.read_u16().await?;
    if width == 0 || height == 0 || width > MAX_FB_SIDE || height > MAX_FB_SIDE {
        return Err(VncError::Protocol(format!(
            "invalid framebuffer size {width}x{height}"
        )));
    }
    let pixel_format = decode_pixel_format(&read_array(r).await?);
    let name = read_string(r, "desktop name").await?;
    Ok(ServerInit {
        width,
        height,
        pixel_format,
        name,
    })
}

/// Encode the 16-byte PIXEL_FORMAT with big-endian maxima and zero padding.
pub fn encode_pixel_format(pf: &PixelFormat) -> [u8; 16] {
    let mut b = [0; 16];
    b[..4].copy_from_slice(&[
        pf.bits_per_pixel,
        pf.depth,
        u8::from(pf.big_endian),
        u8::from(pf.true_colour),
    ]);
    b[4..6].copy_from_slice(&pf.red_max.to_be_bytes());
    b[6..8].copy_from_slice(&pf.green_max.to_be_bytes());
    b[8..10].copy_from_slice(&pf.blue_max.to_be_bytes());
    b[10..13].copy_from_slice(&[pf.red_shift, pf.green_shift, pf.blue_shift]);
    b
}

/// Decode PIXEL_FORMAT without validation, treating every nonzero flag as true.
pub fn decode_pixel_format(b: &[u8; 16]) -> PixelFormat {
    PixelFormat {
        bits_per_pixel: b[0],
        depth: b[1],
        big_endian: b[2] != 0,
        true_colour: b[3] != 0,
        red_max: u16::from_be_bytes([b[4], b[5]]),
        green_max: u16::from_be_bytes([b[6], b[7]]),
        blue_max: u16::from_be_bytes([b[8], b[9]]),
        red_shift: b[10],
        green_shift: b[11],
        blue_shift: b[12],
    }
}

/// Standard client-to-server messages from RFC 6143 section 7.5.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientMsg {
    /// Request the pixel format for subsequent framebuffer updates.
    SetPixelFormat(PixelFormat),
    /// Advertise at most 64 encodings in preference order.
    SetEncodings(Vec<i32>),
    /// Request framebuffer contents for a rectangle.
    FramebufferUpdateRequest {
        /// Request only changes when true.
        incremental: bool,
        /// Requested framebuffer area.
        rect: Rect,
    },
    /// Press or release an X11 keysym.
    KeyEvent {
        /// True for a key press, false for a release.
        down: bool,
        /// X11 keysym value.
        keysym: u32,
    },
    /// Move the pointer and update its pressed-button mask.
    PointerEvent {
        /// Buttons currently pressed.
        mask: u8,
        /// Horizontal pointer position.
        x: u16,
        /// Vertical pointer position.
        y: u16,
    },
    /// Send at most MAX_CUT_TEXT bytes of Latin-1 clipboard text.
    ClientCutText(Vec<u8>),
}

impl ClientMsg {
    /// Encode a message, checking encoding counts and clipboard lengths before allocation.
    pub fn encode(&self) -> Result<Vec<u8>, VncError> {
        let mut b = Vec::new();
        match self {
            Self::SetPixelFormat(pf) => {
                b.extend_from_slice(&[0, 0, 0, 0]);
                b.extend_from_slice(&encode_pixel_format(pf));
            }
            Self::SetEncodings(encodings) => {
                check_encodings_len(encodings.len())?;
                b.extend_from_slice(&[2, 0]);
                b.extend_from_slice(&(encodings.len() as u16).to_be_bytes());
                for encoding in encodings {
                    b.extend_from_slice(&encoding.to_be_bytes());
                }
            }
            Self::FramebufferUpdateRequest { incremental, rect } => {
                b.extend_from_slice(&[3, u8::from(*incremental)]);
                b.extend_from_slice(&encode_rect(*rect));
            }
            Self::KeyEvent { down, keysym } => {
                b.extend_from_slice(&[4, u8::from(*down), 0, 0]);
                b.extend_from_slice(&keysym.to_be_bytes());
            }
            Self::PointerEvent { mask, x, y } => {
                b.extend_from_slice(&[5, *mask]);
                b.extend_from_slice(&x.to_be_bytes());
                b.extend_from_slice(&y.to_be_bytes());
            }
            Self::ClientCutText(text) => {
                if text.len() > MAX_CUT_TEXT as usize {
                    return Err(VncError::Protocol(format!(
                        "client cut text length {} exceeds {MAX_CUT_TEXT}",
                        text.len()
                    )));
                }
                b.extend_from_slice(&[6, 0, 0, 0]);
                b.extend_from_slice(&(text.len() as u32).to_be_bytes());
                b.extend_from_slice(text);
            }
        }
        Ok(b)
    }
}

/// Read one client message, bounding encoding counts and clipboard allocations.
pub async fn read_client_msg<R: AsyncRead + Unpin>(r: &mut R) -> Result<ClientMsg, VncError> {
    match r.read_u8().await? {
        0 => {
            let _padding: [u8; 3] = read_array(r).await?;
            Ok(ClientMsg::SetPixelFormat(decode_pixel_format(
                &read_array(r).await?,
            )))
        }
        2 => {
            let _padding = r.read_u8().await?;
            let count = usize::from(r.read_u16().await?);
            check_encodings_len(count)?;
            let mut encodings = Vec::with_capacity(count);
            for _ in 0..count {
                encodings.push(r.read_i32().await?);
            }
            Ok(ClientMsg::SetEncodings(encodings))
        }
        3 => {
            let incremental = r.read_u8().await? != 0;
            let rect = decode_rect(read_array(r).await?);
            Ok(ClientMsg::FramebufferUpdateRequest { incremental, rect })
        }
        4 => {
            let down = r.read_u8().await? != 0;
            let _padding: [u8; 2] = read_array(r).await?;
            let keysym = r.read_u32().await?;
            Ok(ClientMsg::KeyEvent { down, keysym })
        }
        5 => {
            let mask = r.read_u8().await?;
            let x = r.read_u16().await?;
            let y = r.read_u16().await?;
            Ok(ClientMsg::PointerEvent { mask, x, y })
        }
        6 => {
            let _padding: [u8; 3] = read_array(r).await?;
            let len = r.read_u32().await?;
            let text = read_bounded_bytes(r, len, MAX_CUT_TEXT, "client cut text").await?;
            Ok(ClientMsg::ClientCutText(text))
        }
        t => Err(VncError::Protocol(format!(
            "unknown client message type {t}"
        ))),
    }
}

/// Server message headers whose variable payloads remain in the stream.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ServerMsgHeader {
    /// A framebuffer update followed by the given number of rectangles.
    FramebufferUpdate {
        /// Number of rectangle headers and payloads that follow.
        rects: u16,
    },
    /// A colour-map update followed by count times six bytes of RGB values.
    SetColourMapEntries {
        /// First colour-map index being updated.
        first: u16,
        /// Number of RGB entries that follow.
        count: u16,
    },
    /// Ring the client bell; no payload follows.
    Bell,
    /// A clipboard update whose bytes must be read or skipped by the caller.
    ServerCutText {
        /// Length of the clipboard payload, without applying a size limit.
        len: u32,
    },
}

/// Read exactly one server header, leaving all payload bytes for the caller.
pub async fn read_server_msg_header<R: AsyncRead + Unpin>(
    r: &mut R,
) -> Result<ServerMsgHeader, VncError> {
    match r.read_u8().await? {
        0 => {
            let _padding = r.read_u8().await?;
            let rects = r.read_u16().await?;
            Ok(ServerMsgHeader::FramebufferUpdate { rects })
        }
        1 => {
            let _padding = r.read_u8().await?;
            let first = r.read_u16().await?;
            let count = r.read_u16().await?;
            Ok(ServerMsgHeader::SetColourMapEntries { first, count })
        }
        2 => Ok(ServerMsgHeader::Bell),
        3 => {
            let _padding: [u8; 3] = read_array(r).await?;
            let len = r.read_u32().await?;
            Ok(ServerMsgHeader::ServerCutText { len })
        }
        t => Err(VncError::Protocol(format!(
            "unknown server message type {t}"
        ))),
    }
}

/// Rectangle geometry and the signed encoding number preceding its payload.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectHeader {
    /// Framebuffer area described by the payload.
    pub rect: Rect,
    /// Encoding or pseudo-encoding number.
    pub encoding: i32,
}

/// Read a 12-byte rectangle header without consuming its encoding payload.
pub async fn read_rect_header<R: AsyncRead + Unpin>(r: &mut R) -> Result<RectHeader, VncError> {
    let rect = decode_rect(read_array(r).await?);
    let encoding = r.read_i32().await?;
    Ok(RectHeader { rect, encoding })
}

/// Discard exactly n bytes using an 8 KiB scratch buffer, propagating premature EOF.
pub async fn skip<R: AsyncRead + Unpin>(r: &mut R, mut n: u64) -> Result<(), VncError> {
    let mut buffer = [0; 8 * 1024];
    while n != 0 {
        let len = n.min(buffer.len() as u64) as usize;
        r.read_exact(&mut buffer[..len]).await?;
        n -= len as u64;
    }
    Ok(())
}

/// Encode a 3.7/3.8 security list, keeping at most the u8 wire limit of 255 types.
pub fn encode_security_types(types: &[u8]) -> Vec<u8> {
    let count = types.len().min(usize::from(u8::MAX));
    let mut b = Vec::with_capacity(1 + count);
    b.push(count as u8);
    b.extend_from_slice(&types[..count]);
    b
}

/// Encode the single u32 security type sent by an RFB 3.3 server.
pub fn encode_security_type_33(t: u32) -> [u8; 4] {
    t.to_be_bytes()
}

/// Encode a UTF-8 reason, capping at the u32 wire limit on a character boundary.
pub fn encode_reason(s: &str) -> Vec<u8> {
    let len = s.floor_char_boundary(s.len().min(u32::MAX as usize));
    let mut b = Vec::new();
    append_length_prefixed(&mut b, &s.as_bytes()[..len]);
    b
}

/// Encode SecurityResult, appending a reason only for failures with Some(reason).
pub fn encode_security_result(ok: bool, reason: Option<&str>) -> Vec<u8> {
    let mut b = u32::from(!ok).to_be_bytes().to_vec();
    if !ok {
        if let Some(reason) = reason {
            b.extend_from_slice(&encode_reason(reason));
        }
    }
    b
}

/// Encode ServerInit with its dimensions, pixel format, and UTF-8 desktop name.
pub fn encode_server_init(si: &ServerInit) -> Vec<u8> {
    let mut b = Vec::new();
    b.extend_from_slice(&si.width.to_be_bytes());
    b.extend_from_slice(&si.height.to_be_bytes());
    b.extend_from_slice(&encode_pixel_format(&si.pixel_format));
    b.extend_from_slice(&encode_reason(&si.name));
    b
}

/// Encode the four-byte FramebufferUpdate header.
pub fn encode_fb_update_header(rects: u16) -> [u8; 4] {
    let [high, low] = rects.to_be_bytes();
    [0, 0, high, low]
}

/// Encode rectangle coordinates, dimensions, and signed encoding in big endian.
pub fn encode_rect_header(h: &RectHeader) -> [u8; 12] {
    let mut b = [0; 12];
    b[..8].copy_from_slice(&encode_rect(h.rect));
    b[8..].copy_from_slice(&h.encoding.to_be_bytes());
    b
}

/// Encode the payload-free Bell message.
pub fn encode_bell() -> [u8; 1] {
    [2]
}

/// Encode ServerCutText, keeping at most the u32 wire limit of payload bytes.
pub fn encode_server_cut_text(text: &[u8]) -> Vec<u8> {
    let mut b = vec![3, 0, 0, 0];
    append_length_prefixed(&mut b, text);
    b
}

/// Convert UTF-8 to Latin-1 after replacing CRLF pairs with LF; reject other characters.
pub fn to_latin1(s: &str) -> Option<Vec<u8>> {
    s.replace("\r\n", "\n")
        .chars()
        .map(|c| u8::try_from(u32::from(c)).ok())
        .collect()
}

/// Convert Latin-1 bytes to Unicode characters with the same numeric values.
pub fn from_latin1(b: &[u8]) -> String {
    b.iter().map(|byte| char::from(*byte)).collect()
}

async fn read_array<R: AsyncRead + Unpin, const N: usize>(r: &mut R) -> Result<[u8; N], VncError> {
    let mut b = [0; N];
    r.read_exact(&mut b).await?;
    Ok(b)
}

async fn read_bounded_bytes<R: AsyncRead + Unpin>(
    r: &mut R,
    len: u32,
    max: u32,
    field: &str,
) -> Result<Vec<u8>, VncError> {
    if len > max {
        return Err(VncError::Protocol(format!(
            "{field} length {len} exceeds {max}"
        )));
    }
    let mut b = vec![0; len as usize];
    r.read_exact(&mut b).await?;
    Ok(b)
}

async fn read_string<R: AsyncRead + Unpin>(r: &mut R, field: &str) -> Result<String, VncError> {
    let len = r.read_u32().await?;
    let b = read_bounded_bytes(r, len, MAX_NAME_LEN, field).await?;
    Ok(String::from_utf8_lossy(&b).into_owned())
}

fn check_encodings_len(len: usize) -> Result<(), VncError> {
    if len > MAX_ENCODINGS {
        return Err(VncError::Protocol(format!(
            "encoding count {len} exceeds {MAX_ENCODINGS}"
        )));
    }
    Ok(())
}

fn encode_rect(rect: Rect) -> [u8; 8] {
    let mut b = [0; 8];
    for (chunk, value) in b.chunks_exact_mut(2).zip([rect.x, rect.y, rect.w, rect.h]) {
        chunk.copy_from_slice(&value.to_be_bytes());
    }
    b
}

fn decode_rect(b: [u8; 8]) -> Rect {
    Rect::new(
        u16::from_be_bytes([b[0], b[1]]),
        u16::from_be_bytes([b[2], b[3]]),
        u16::from_be_bytes([b[4], b[5]]),
        u16::from_be_bytes([b[6], b[7]]),
    )
}

fn append_length_prefixed(b: &mut Vec<u8>, payload: &[u8]) {
    let len = payload.len().min(u32::MAX as usize);
    b.extend_from_slice(&(len as u32).to_be_bytes());
    b.extend_from_slice(&payload[..len]);
}
