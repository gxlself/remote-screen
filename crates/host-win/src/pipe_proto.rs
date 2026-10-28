//! Wire protocol used between the Windows service and `rs-host-session.exe`.
//!
//! Frames are `u32` little-endian length prefixes (including the kind byte),
//! followed by a kind byte and the kind-specific body. Control messages use the
//! same tagged JSON style as the local host IPC.

use std::io;

use data_encoding::BASE64;
use host_core::platform::{CursorUpdate, DisplayInfo, PixelRect, PlatformError, RoiRegion};
use rsp_core::hid::InputMsg;
use rsp_core::pb;
use serde::{Deserialize, Serialize};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};

pub const PIPE_PROTO_VERSION: u32 = 1;
/// Maximum value of the length field, including the one-byte frame kind.
pub const MAX_FRAME: usize = 16 * 1024 * 1024;

pub const KIND_CTL: u8 = 0x01;
pub const KIND_VIDEO: u8 = 0x02;
pub const KIND_AUDIO: u8 = 0x03;
pub const KIND_BLOB: u8 = 0x04;

pub const UI_PIPE_NAME: &str = r"\\.\pipe\RemoteHost.ui";

pub fn session_pipe_name(session_id: u32) -> String {
    format!(r"\\.\pipe\RemoteHost.session.{session_id}")
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "t", rename_all = "snake_case")]
pub enum Ctl {
    Hello {
        version: u32,
        session_id: u32,
        pid: u32,
    },
    Welcome {
        version: u32,
    },
    Ping {
        n: u64,
    },
    Pong {
        n: u64,
    },
    Inject {
        display_id: u32,
        input: String,
    },
    ReleaseAll,
    Action {
        req: u32,
        kind: i32,
    },
    ClipboardGet {
        req: u32,
    },
    Watch {
        what: WatchKind,
    },
    Unwatch {
        what: WatchKind,
    },
    CaptureStart {
        req: u32,
        cap: u32,
        display: u32,
        fps: u32,
        max_width: u32,
        max_height: u32,
        #[serde(default)]
        region: Option<RegionWire>,
    },
    CaptureStop {
        cap: u32,
    },
    EncoderOpen {
        req: u32,
        enc: u32,
        codec: i32,
        width: u32,
        height: u32,
        fps: u32,
        bitrate_kbps: u32,
    },
    Encode {
        enc: u32,
        cap: u32,
        seq: u64,
        force_keyframe: bool,
        quality_boost: bool,
    },
    EncoderRate {
        enc: u32,
        kbps: u32,
        fps: u32,
    },
    EncoderKeyframe {
        enc: u32,
    },
    EncoderBoost {
        enc: u32,
        on: bool,
    },
    EncoderRoi {
        enc: u32,
        regions: Vec<RoiWire>,
    },
    EncoderInfo {
        enc: u32,
        roi: bool,
    },
    EncoderClose {
        enc: u32,
    },
    AudioStart {
        req: u32,
        sample_rate: u32,
        channels: u8,
        exclude_self: bool,
    },
    AudioStop,
    KeepAwake {
        on: bool,
    },
    Done {
        req: u32,
        error: Option<WireError>,
    },
    Displays {
        list: Vec<DisplayWire>,
    },
    Frame {
        cap: u32,
        seq: u64,
        width: u32,
        height: u32,
        capture_ts_us: u64,
    },
    CaptureEnded {
        cap: u32,
        error: Option<WireError>,
    },
    EncoderFailed {
        enc: u32,
        error: WireError,
    },
    CursorShape {
        id: u64,
        width: u32,
        height: u32,
        hot_x: u32,
        hot_y: u32,
        scale: f32,
        png: String,
    },
    CursorUse {
        id: u64,
    },
    CursorHidden {
        hidden: bool,
    },
    CursorPos {
        display_id: u32,
        seq: u32,
        x: f32,
        y: f32,
        visible: bool,
    },
    Activity {
        active: bool,
    },
    ClipboardChanged,
    Desktop {
        name: String,
    },
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash, Serialize, Deserialize)]
#[serde(rename_all = "snake_case")]
pub enum WatchKind {
    Cursor,
    Activity,
    Clipboard,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "code", rename_all = "snake_case")]
pub enum WireError {
    Unsupported,
    PermissionDenied { message: String },
    Platform { message: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
pub struct DisplayWire {
    pub id: u32,
    pub name: String,
    pub width_px: u32,
    pub height_px: u32,
    pub scale: f32,
    pub x: i32,
    pub y: i32,
    pub refresh_hz: u32,
    pub primary: bool,
    pub rotation: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RegionWire {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
pub struct RoiWire {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub qp: i8,
}

impl From<PixelRect> for RegionWire {
    fn from(rect: PixelRect) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }
}

impl From<RegionWire> for PixelRect {
    fn from(rect: RegionWire) -> Self {
        Self {
            x: rect.x,
            y: rect.y,
            w: rect.w,
            h: rect.h,
        }
    }
}

impl From<RoiRegion> for RoiWire {
    fn from(region: RoiRegion) -> Self {
        Self {
            x: region.rect.x,
            y: region.rect.y,
            w: region.rect.w,
            h: region.rect.h,
            qp: region.qp_delta,
        }
    }
}

impl From<RoiWire> for RoiRegion {
    fn from(region: RoiWire) -> Self {
        Self {
            rect: PixelRect {
                x: region.x,
                y: region.y,
                w: region.w,
                h: region.h,
            },
            qp_delta: region.qp,
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct VideoPacket {
    pub enc: u32,
    pub keyframe: bool,
    pub still_refine: bool,
    pub capture_ts_us: u64,
    pub encoded_ts_us: u64,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct AudioPacket {
    pub capture_ts_us: u64,
    pub channels: u8,
    pub frames: u32,
    pub samples: Vec<f32>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum BlobKind {
    Text = 1,
    Png = 2,
}

#[derive(Debug, Clone, PartialEq)]
pub struct BlobPacket {
    pub req: u32,
    pub kind: BlobKind,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum PipeFrame {
    Ctl(Ctl),
    Video(VideoPacket),
    Audio(AudioPacket),
    Blob(BlobPacket),
}

#[derive(Debug, thiserror::Error)]
pub enum PipeError {
    #[error(transparent)]
    Io(#[from] io::Error),
    #[error("frame length {0} exceeds the limit")]
    TooLarge(usize),
    #[error("invalid frame: {0}")]
    BadFrame(&'static str),
    #[error("unknown frame kind {0:#04x}")]
    UnknownKind(u8),
    #[error("invalid control JSON: {0}")]
    BadJson(String),
    #[error("unsupported pipe protocol version {got}")]
    Version { got: u32 },
}

const VIDEO_HEADER: usize = 21;
const AUDIO_HEADER: usize = 13;
const BLOB_HEADER: usize = 5;
const FLAG_KEYFRAME: u8 = 0x01;
const FLAG_STILL_REFINE: u8 = 0x02;

pub fn encode_frame(frame: &PipeFrame) -> Result<Vec<u8>, PipeError> {
    match frame {
        PipeFrame::Ctl(ctl) => {
            let body =
                serde_json::to_vec(ctl).map_err(|error| PipeError::BadJson(error.to_string()))?;
            encode_body(KIND_CTL, &body)
        }
        PipeFrame::Video(packet) => {
            let body_len = VIDEO_HEADER
                .checked_add(packet.data.len())
                .ok_or(PipeError::TooLarge(usize::MAX))?;
            check_length(body_len)?;
            let mut body = Vec::with_capacity(body_len);
            body.extend_from_slice(&packet.enc.to_le_bytes());
            let mut flags = 0;
            if packet.keyframe {
                flags |= FLAG_KEYFRAME;
            }
            if packet.still_refine {
                flags |= FLAG_STILL_REFINE;
            }
            body.push(flags);
            body.extend_from_slice(&packet.capture_ts_us.to_le_bytes());
            body.extend_from_slice(&packet.encoded_ts_us.to_le_bytes());
            body.extend_from_slice(&packet.data);
            encode_body(KIND_VIDEO, &body)
        }
        PipeFrame::Audio(packet) => {
            validate_audio_samples(packet.frames, packet.channels, packet.samples.len())?;
            let samples_len = packet
                .samples
                .len()
                .checked_mul(4)
                .ok_or(PipeError::TooLarge(usize::MAX))?;
            let body_len = AUDIO_HEADER
                .checked_add(samples_len)
                .ok_or(PipeError::TooLarge(usize::MAX))?;
            check_length(body_len)?;
            let mut body = Vec::with_capacity(body_len);
            body.extend_from_slice(&packet.capture_ts_us.to_le_bytes());
            body.push(packet.channels);
            body.extend_from_slice(&packet.frames.to_le_bytes());
            for sample in &packet.samples {
                body.extend_from_slice(&sample.to_le_bytes());
            }
            encode_body(KIND_AUDIO, &body)
        }
        PipeFrame::Blob(packet) => {
            let body_len = BLOB_HEADER
                .checked_add(packet.data.len())
                .ok_or(PipeError::TooLarge(usize::MAX))?;
            check_length(body_len)?;
            let mut body = Vec::with_capacity(body_len);
            body.extend_from_slice(&packet.req.to_le_bytes());
            body.push(packet.kind as u8);
            body.extend_from_slice(&packet.data);
            encode_body(KIND_BLOB, &body)
        }
    }
}

fn check_length(body_len: usize) -> Result<(), PipeError> {
    let frame_len = body_len
        .checked_add(1)
        .ok_or(PipeError::TooLarge(usize::MAX))?;
    if frame_len > MAX_FRAME {
        return Err(PipeError::TooLarge(frame_len));
    }
    Ok(())
}

fn encode_body(kind: u8, body: &[u8]) -> Result<Vec<u8>, PipeError> {
    check_length(body.len())?;
    let frame_len = body.len() + 1;
    let mut out = Vec::with_capacity(4 + frame_len);
    out.extend_from_slice(&(frame_len as u32).to_le_bytes());
    out.push(kind);
    out.extend_from_slice(body);
    Ok(out)
}

fn validate_audio_samples(frames: u32, channels: u8, samples: usize) -> Result<(), PipeError> {
    let expected = usize::try_from(frames)
        .ok()
        .and_then(|count| count.checked_mul(usize::from(channels)))
        .ok_or(PipeError::BadFrame("audio samples"))?;
    if samples != expected {
        return Err(PipeError::BadFrame("audio samples"));
    }
    Ok(())
}

#[derive(Debug, Default)]
pub struct FrameReader {
    buffer: Vec<u8>,
}

impl FrameReader {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, bytes: &[u8]) {
        self.buffer.extend_from_slice(bytes);
    }

    /// Reads the next complete frame, or `None` until enough bytes are buffered.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<PipeFrame>, PipeError> {
        let Some(length_bytes) = self.buffer.get(..4) else {
            return Ok(None);
        };
        let frame_len = u32::from_le_bytes([
            length_bytes[0],
            length_bytes[1],
            length_bytes[2],
            length_bytes[3],
        ]) as usize;
        check_declared_length(frame_len)?;

        let Some(&kind) = self.buffer.get(4) else {
            return Ok(None);
        };
        ensure_known_kind(kind)?;

        let total = 4 + frame_len;
        if self.buffer.len() < total {
            return Ok(None);
        }
        let frame = decode_body(kind, &self.buffer[5..total])?;
        self.buffer.drain(..total);
        Ok(Some(frame))
    }
}

pub async fn read_frame<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<Option<PipeFrame>, PipeError> {
    let mut length_bytes = [0_u8; 4];
    if reader.read(&mut length_bytes[..1]).await? == 0 {
        return Ok(None);
    }
    reader.read_exact(&mut length_bytes[1..]).await?;

    let frame_len = u32::from_le_bytes(length_bytes) as usize;
    check_declared_length(frame_len)?;

    let mut kind_byte = [0_u8; 1];
    reader.read_exact(&mut kind_byte).await?;
    let kind = kind_byte[0];
    ensure_known_kind(kind)?;

    let mut body = vec![0_u8; frame_len - 1];
    reader.read_exact(&mut body).await?;
    decode_body(kind, &body).map(Some)
}

pub async fn write_frame<W: AsyncWrite + Unpin>(
    writer: &mut W,
    frame: &PipeFrame,
) -> Result<(), PipeError> {
    let bytes = encode_frame(frame)?;
    writer.write_all(&bytes).await?;
    Ok(())
}

fn check_declared_length(frame_len: usize) -> Result<(), PipeError> {
    if frame_len == 0 {
        return Err(PipeError::BadFrame("frame length"));
    }
    if frame_len > MAX_FRAME {
        return Err(PipeError::TooLarge(frame_len));
    }
    Ok(())
}

fn ensure_known_kind(kind: u8) -> Result<(), PipeError> {
    match kind {
        KIND_CTL | KIND_VIDEO | KIND_AUDIO | KIND_BLOB => Ok(()),
        other => Err(PipeError::UnknownKind(other)),
    }
}

fn decode_body(kind: u8, body: &[u8]) -> Result<PipeFrame, PipeError> {
    match kind {
        KIND_CTL => {
            let ctl = serde_json::from_slice(body)
                .map_err(|error| PipeError::BadJson(error.to_string()))?;
            Ok(PipeFrame::Ctl(ctl))
        }
        KIND_VIDEO => decode_video(body).map(PipeFrame::Video),
        KIND_AUDIO => decode_audio(body).map(PipeFrame::Audio),
        KIND_BLOB => decode_blob(body).map(PipeFrame::Blob),
        other => Err(PipeError::UnknownKind(other)),
    }
}

fn decode_video(body: &[u8]) -> Result<VideoPacket, PipeError> {
    if body.len() < VIDEO_HEADER {
        return Err(PipeError::BadFrame("video header"));
    }
    let enc = read_u32(body, 0)?;
    let flags = *body.get(4).ok_or(PipeError::BadFrame("video header"))?;
    let capture_ts_us = read_u64(body, 5)?;
    let encoded_ts_us = read_u64(body, 13)?;
    let data = body
        .get(VIDEO_HEADER..)
        .ok_or(PipeError::BadFrame("video header"))?
        .to_vec();
    Ok(VideoPacket {
        enc,
        keyframe: flags & FLAG_KEYFRAME != 0,
        still_refine: flags & FLAG_STILL_REFINE != 0,
        capture_ts_us,
        encoded_ts_us,
        data,
    })
}

fn decode_audio(body: &[u8]) -> Result<AudioPacket, PipeError> {
    if body.len() < AUDIO_HEADER {
        return Err(PipeError::BadFrame("audio header"));
    }
    let capture_ts_us = read_u64(body, 0)?;
    let channels = *body.get(8).ok_or(PipeError::BadFrame("audio header"))?;
    let frames = read_u32(body, 9)?;
    let sample_bytes = body
        .get(AUDIO_HEADER..)
        .ok_or(PipeError::BadFrame("audio header"))?;
    let sample_count = sample_bytes.len() / 4;
    if sample_bytes.len() % 4 != 0 {
        return Err(PipeError::BadFrame("audio samples"));
    }
    validate_audio_samples(frames, channels, sample_count)?;

    let mut samples = Vec::with_capacity(sample_count);
    for bytes in sample_bytes.chunks_exact(4) {
        let sample: [u8; 4] = bytes
            .try_into()
            .map_err(|_| PipeError::BadFrame("audio samples"))?;
        samples.push(f32::from_le_bytes(sample));
    }
    Ok(AudioPacket {
        capture_ts_us,
        channels,
        frames,
        samples,
    })
}

fn decode_blob(body: &[u8]) -> Result<BlobPacket, PipeError> {
    if body.len() < BLOB_HEADER {
        return Err(PipeError::BadFrame("blob header"));
    }
    let req = read_u32(body, 0)?;
    let kind = match *body.get(4).ok_or(PipeError::BadFrame("blob header"))? {
        1 => BlobKind::Text,
        2 => BlobKind::Png,
        _ => return Err(PipeError::BadFrame("blob kind")),
    };
    let data = body
        .get(BLOB_HEADER..)
        .ok_or(PipeError::BadFrame("blob header"))?
        .to_vec();
    Ok(BlobPacket { req, kind, data })
}

fn read_u32(bytes: &[u8], offset: usize) -> Result<u32, PipeError> {
    let value: [u8; 4] = bytes
        .get(offset..offset + 4)
        .ok_or(PipeError::BadFrame("truncated header"))?
        .try_into()
        .map_err(|_| PipeError::BadFrame("truncated header"))?;
    Ok(u32::from_le_bytes(value))
}

fn read_u64(bytes: &[u8], offset: usize) -> Result<u64, PipeError> {
    let value: [u8; 8] = bytes
        .get(offset..offset + 8)
        .ok_or(PipeError::BadFrame("truncated header"))?
        .try_into()
        .map_err(|_| PipeError::BadFrame("truncated header"))?;
    Ok(u64::from_le_bytes(value))
}

pub fn check_hello(ctl: &Ctl) -> Result<(u32, u32), PipeError> {
    match ctl {
        Ctl::Hello {
            version,
            session_id,
            pid,
        } => {
            if *version != PIPE_PROTO_VERSION {
                return Err(PipeError::Version { got: *version });
            }
            Ok((*session_id, *pid))
        }
        _ => Err(PipeError::BadFrame("hello")),
    }
}

pub fn input_to_b64(msg: &InputMsg) -> Result<String, PipeError> {
    let encoded = msg.encode().map_err(|_| PipeError::BadFrame("input"))?;
    Ok(BASE64.encode(&encoded))
}

pub fn input_from_b64(input: &str) -> Result<InputMsg, PipeError> {
    let bytes = BASE64
        .decode(input.as_bytes())
        .map_err(|_| PipeError::BadFrame("input"))?;
    InputMsg::decode(&bytes).map_err(|_| PipeError::BadFrame("input"))
}

impl From<&DisplayInfo> for DisplayWire {
    fn from(display: &DisplayInfo) -> Self {
        Self {
            id: display.id,
            name: display.name.clone(),
            width_px: display.width_px,
            height_px: display.height_px,
            scale: display.scale,
            x: display.x,
            y: display.y,
            refresh_hz: display.refresh_hz,
            primary: display.primary,
            rotation: display.rotation,
        }
    }
}

impl From<DisplayWire> for DisplayInfo {
    fn from(display: DisplayWire) -> Self {
        Self {
            id: display.id,
            name: display.name,
            width_px: display.width_px,
            height_px: display.height_px,
            scale: display.scale,
            x: display.x,
            y: display.y,
            refresh_hz: display.refresh_hz,
            primary: display.primary,
            rotation: display.rotation,
        }
    }
}

pub fn cursor_to_ctl(update: &CursorUpdate) -> Ctl {
    match update {
        CursorUpdate::Shape(shape) => Ctl::CursorShape {
            id: shape.id,
            width: shape.width,
            height: shape.height,
            hot_x: shape.hot_x,
            hot_y: shape.hot_y,
            scale: shape.scale,
            png: BASE64.encode(&shape.png),
        },
        CursorUpdate::Use(id) => Ctl::CursorUse { id: *id },
        CursorUpdate::Hidden(hidden) => Ctl::CursorHidden { hidden: *hidden },
        CursorUpdate::Position {
            display_id,
            seq,
            x,
            y,
            visible,
        } => Ctl::CursorPos {
            display_id: *display_id,
            seq: *seq,
            x: *x,
            y: *y,
            visible: *visible,
        },
    }
}

pub fn ctl_to_cursor(ctl: &Ctl) -> Option<CursorUpdate> {
    match ctl {
        Ctl::CursorShape {
            id,
            width,
            height,
            hot_x,
            hot_y,
            scale,
            png,
        } => {
            let png = BASE64.decode(png.as_bytes()).ok()?;
            Some(CursorUpdate::Shape(pb::CursorShape {
                id: *id,
                width: *width,
                height: *height,
                hot_x: *hot_x,
                hot_y: *hot_y,
                scale: *scale,
                png,
            }))
        }
        Ctl::CursorUse { id } => Some(CursorUpdate::Use(*id)),
        Ctl::CursorHidden { hidden } => Some(CursorUpdate::Hidden(*hidden)),
        Ctl::CursorPos {
            display_id,
            seq,
            x,
            y,
            visible,
        } => Some(CursorUpdate::Position {
            display_id: *display_id,
            seq: *seq,
            x: *x,
            y: *y,
            visible: *visible,
        }),
        _ => None,
    }
}

impl From<&PlatformError> for WireError {
    fn from(error: &PlatformError) -> Self {
        match error {
            PlatformError::Unsupported => Self::Unsupported,
            PlatformError::PermissionDenied(message) => Self::PermissionDenied {
                message: message.clone(),
            },
            PlatformError::Platform(message) => Self::Platform {
                message: message.clone(),
            },
            PlatformError::Io(error) => Self::Platform {
                message: error.to_string(),
            },
        }
    }
}

impl From<WireError> for PlatformError {
    fn from(error: WireError) -> Self {
        match error {
            WireError::Unsupported => Self::Unsupported,
            WireError::PermissionDenied { message } => Self::PermissionDenied(message),
            WireError::Platform { message } => Self::Platform(message),
        }
    }
}
