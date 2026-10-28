//! Plaintext recording records (M3 V5).

use serde::{Deserialize, Serialize};

pub const MAX_BODY: u32 = 16 << 20;
/// `kind u8 | t_us u64 | body_len u32`, all integers big-endian.
pub const HEADER_LEN: usize = 13;

const KIND_META: u8 = 1;
const KIND_CONFIG: u8 = 2;
const KIND_VIDEO: u8 = 3;
const KIND_CURSOR_POS: u8 = 4;
const KIND_CURSOR_SHAPE: u8 = 5;
const KIND_CURSOR_USE: u8 = 6;
const KIND_CURSOR_HIDDEN: u8 = 7;
const KIND_AUDIO: u8 = 8;
const KIND_GAP: u8 = 9;
const KIND_END: u8 = 10;

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingMeta {
    pub host_id: String,
    pub host_name: String,
    pub role: String,
    pub display_id: Option<u32>,
    pub started_at_us: u64,
    pub app_version: String,
    pub audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Default, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingEnd {
    pub duration_us: u64,
    pub frames: u64,
    pub keyframes: u64,
    pub audio_packets: u64,
    pub gaps: u64,
    pub dropped: u64,
    pub plain_bytes: u64,
    pub ended_at_us: u64,
    pub reason: String,
    pub width: u32,
    pub height: u32,
    pub codec: u8,
}

#[derive(Debug, Clone, PartialEq)]
pub struct ConfigRec {
    pub epoch: u32,
    pub display_id: u32,
    pub codec: u8,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub cursor_x: f32,
    pub cursor_y: f32,
    pub cursor_visible: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoRec {
    pub frame_id: u32,
    pub epoch: u8,
    pub display_id: u8,
    pub flags: u8,
    pub keyframe: bool,
    pub capture_ts_us: u64,
    /// `i64::MIN` is reserved on the wire to represent `None`.
    pub capture_local_us: Option<i64>,
    pub data: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq)]
pub struct CursorShapeRec {
    pub id: u64,
    pub width: u32,
    pub height: u32,
    pub hot_x: u32,
    pub hot_y: u32,
    pub scale: f32,
    pub png: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioRec {
    pub seq: u16,
    pub capture_ts_us: u64,
    pub payload: Vec<u8>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GapReason {
    Backpressure = 1,
    Paused = 2,
    Reconnect = 3,
    Clock = 4,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Body {
    Meta(RecordingMeta),
    Config(ConfigRec),
    Video(VideoRec),
    CursorPos { x: f32, y: f32, visible: bool },
    CursorShape(CursorShapeRec),
    CursorUse(u64),
    CursorHidden(bool),
    Audio(AudioRec),
    Gap { reason: GapReason, real_us: u64 },
    End(RecordingEnd),
    Unknown { kind: u8, body: Vec<u8> },
}

#[derive(Debug, Clone, PartialEq)]
pub struct Record {
    pub t_us: u64,
    pub body: Body,
}

impl Record {
    pub fn kind(&self) -> u8 {
        match self.body {
            Body::Meta(_) => KIND_META,
            Body::Config(_) => KIND_CONFIG,
            Body::Video(_) => KIND_VIDEO,
            Body::CursorPos { .. } => KIND_CURSOR_POS,
            Body::CursorShape(_) => KIND_CURSOR_SHAPE,
            Body::CursorUse(_) => KIND_CURSOR_USE,
            Body::CursorHidden(_) => KIND_CURSOR_HIDDEN,
            Body::Audio(_) => KIND_AUDIO,
            Body::Gap { .. } => KIND_GAP,
            Body::End(_) => KIND_END,
            Body::Unknown { kind, .. } => kind,
        }
    }

    pub fn encoded_len(&self) -> usize {
        HEADER_LEN + body_len(&self.body)
    }

    pub fn encode_into(&self, out: &mut Vec<u8>) {
        let header_start = out.len();
        out.push(self.kind());
        out.extend_from_slice(&self.t_us.to_be_bytes());
        out.extend_from_slice(&[0; 4]);
        encode_body(&self.body, out);
        let body_len = out.len() - header_start - HEADER_LEN;
        assert!(
            body_len <= u32::MAX as usize,
            "record body exceeds u32 length"
        );
        out[header_start + 9..header_start + HEADER_LEN]
            .copy_from_slice(&(body_len as u32).to_be_bytes());
    }
}

#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum RecordError {
    #[error("truncated record at {offset}")]
    Truncated { offset: usize },
    #[error("record body too large: {len}")]
    TooLarge { len: u32 },
    #[error("bad body for kind {kind}: {reason}")]
    BadBody { kind: u8, reason: &'static str },
}

/// Decode an entire plaintext chunk, failing on its first malformed record.
pub fn decode(buf: &[u8]) -> Result<Vec<Record>, RecordError> {
    iter(buf).collect()
}

/// Iterate over framed records. A bad body can be skipped; an invalid frame
/// length or a truncated record ends iteration after reporting the error once.
pub struct Records<'a> {
    buf: &'a [u8],
    offset: usize,
    done: bool,
}

pub fn iter(buf: &[u8]) -> Records<'_> {
    Records {
        buf,
        offset: 0,
        done: false,
    }
}

impl Iterator for Records<'_> {
    type Item = Result<Record, RecordError>;

    fn next(&mut self) -> Option<Self::Item> {
        if self.done || self.offset == self.buf.len() {
            return None;
        }
        let start = self.offset;
        let Some((kind, t_us, len)) = parse_header(&self.buf[start..]) else {
            self.done = true;
            return Some(Err(RecordError::Truncated { offset: start }));
        };
        if len > MAX_BODY {
            self.done = true;
            return Some(Err(RecordError::TooLarge { len }));
        }
        let body_len = len as usize;
        let body_start = start + HEADER_LEN;
        let body_end = match body_start.checked_add(body_len) {
            Some(end) => end,
            None => {
                self.done = true;
                return Some(Err(RecordError::Truncated { offset: start }));
            }
        };
        if body_end > self.buf.len() {
            self.done = true;
            return Some(Err(RecordError::Truncated { offset: start }));
        }
        self.offset = body_end;
        let body = &self.buf[body_start..body_end];
        Some(decode_body(kind, t_us, body))
    }
}

impl std::iter::FusedIterator for Records<'_> {}

fn parse_header(buf: &[u8]) -> Option<(u8, u64, u32)> {
    let (kind, rest) = buf.split_first()?;
    let (t_us, rest) = rest.split_first_chunk::<8>()?;
    let (len, _) = rest.split_first_chunk::<4>()?;
    Some((*kind, u64::from_be_bytes(*t_us), u32::from_be_bytes(*len)))
}

fn body_len(body: &Body) -> usize {
    match body {
        Body::Meta(meta) => serde_json::to_vec(meta)
            .expect("recording metadata serialization cannot fail")
            .len(),
        Body::Config(_) => 34,
        Body::Video(video) => 24 + video.data.len(),
        Body::CursorPos { .. } => 9,
        Body::CursorShape(shape) => 28 + shape.png.len(),
        Body::CursorUse(_) => 8,
        Body::CursorHidden(_) => 1,
        Body::Audio(audio) => 10 + audio.payload.len(),
        Body::Gap { .. } => 9,
        Body::End(end) => serde_json::to_vec(end)
            .expect("recording end serialization cannot fail")
            .len(),
        Body::Unknown { body, .. } => body.len(),
    }
}

fn encode_body(body: &Body, out: &mut Vec<u8>) {
    match body {
        Body::Meta(meta) => {
            serde_json::to_writer(out, meta)
                .expect("metadata contains only JSON primitives and Vec writes cannot fail");
        }
        Body::Config(config) => {
            put_u32(out, config.epoch);
            put_u32(out, config.display_id);
            out.push(config.codec);
            put_u32(out, config.width);
            put_u32(out, config.height);
            put_u32(out, config.fps);
            put_u32(out, config.bitrate_kbps);
            put_f32(out, config.cursor_x);
            put_f32(out, config.cursor_y);
            put_bool(out, config.cursor_visible);
        }
        Body::Video(video) => {
            put_u32(out, video.frame_id);
            out.push(video.epoch);
            out.push(video.display_id);
            out.push(video.flags);
            put_bool(out, video.keyframe);
            put_u64(out, video.capture_ts_us);
            put_i64(out, video.capture_local_us.unwrap_or(i64::MIN));
            out.extend_from_slice(&video.data);
        }
        Body::CursorPos { x, y, visible } => {
            put_f32(out, *x);
            put_f32(out, *y);
            put_bool(out, *visible);
        }
        Body::CursorShape(shape) => {
            put_u64(out, shape.id);
            put_u32(out, shape.width);
            put_u32(out, shape.height);
            put_u32(out, shape.hot_x);
            put_u32(out, shape.hot_y);
            put_f32(out, shape.scale);
            out.extend_from_slice(&shape.png);
        }
        Body::CursorUse(id) => put_u64(out, *id),
        Body::CursorHidden(hidden) => put_bool(out, *hidden),
        Body::Audio(audio) => {
            put_u16(out, audio.seq);
            put_u64(out, audio.capture_ts_us);
            out.extend_from_slice(&audio.payload);
        }
        Body::Gap { reason, real_us } => {
            out.push(*reason as u8);
            put_u64(out, *real_us);
        }
        Body::End(end) => {
            serde_json::to_writer(out, end)
                .expect("end contains only JSON primitives and Vec writes cannot fail");
        }
        Body::Unknown { body, .. } => out.extend_from_slice(body),
    }
}

fn decode_body(kind: u8, t_us: u64, body: &[u8]) -> Result<Record, RecordError> {
    let required = match kind {
        KIND_CONFIG => Some((34, true)),
        KIND_VIDEO => Some((24, false)),
        KIND_CURSOR_POS => Some((9, true)),
        KIND_CURSOR_SHAPE => Some((28, false)),
        KIND_CURSOR_USE => Some((8, true)),
        KIND_CURSOR_HIDDEN => Some((1, true)),
        KIND_AUDIO => Some((10, false)),
        KIND_GAP => Some((9, true)),
        _ => None,
    };
    if let Some((len, exact)) = required {
        if body.len() < len || (exact && body.len() != len) {
            return Err(bad_len(kind));
        }
    }

    let mut r = BodyReader { buf: body, kind };
    let body = match kind {
        KIND_META => Body::Meta(parse_json(kind, body)?),
        KIND_CONFIG => Body::Config(ConfigRec {
            epoch: r.u32()?,
            display_id: r.u32()?,
            codec: r.u8()?,
            width: r.u32()?,
            height: r.u32()?,
            fps: r.u32()?,
            bitrate_kbps: r.u32()?,
            cursor_x: r.f32()?,
            cursor_y: r.f32()?,
            cursor_visible: r.boolean()?,
        }),
        KIND_VIDEO => {
            let frame_id = r.u32()?;
            let epoch = r.u8()?;
            let display_id = r.u8()?;
            let flags = r.u8()?;
            let keyframe = r.boolean()?;
            let capture_ts_us = r.u64()?;
            let capture_local = i64::from_be_bytes(r.take()?);
            Body::Video(VideoRec {
                frame_id,
                epoch,
                display_id,
                flags,
                keyframe,
                capture_ts_us,
                capture_local_us: (capture_local != i64::MIN).then_some(capture_local),
                data: r.buf.to_vec(),
            })
        }
        KIND_CURSOR_POS => Body::CursorPos {
            x: r.f32()?,
            y: r.f32()?,
            visible: r.boolean()?,
        },
        KIND_CURSOR_SHAPE => Body::CursorShape(CursorShapeRec {
            id: r.u64()?,
            width: r.u32()?,
            height: r.u32()?,
            hot_x: r.u32()?,
            hot_y: r.u32()?,
            scale: r.f32()?,
            png: r.buf.to_vec(),
        }),
        KIND_CURSOR_USE => Body::CursorUse(r.u64()?),
        KIND_CURSOR_HIDDEN => Body::CursorHidden(r.boolean()?),
        KIND_AUDIO => Body::Audio(AudioRec {
            seq: u16::from_be_bytes(r.take()?),
            capture_ts_us: r.u64()?,
            payload: r.buf.to_vec(),
        }),
        KIND_GAP => Body::Gap {
            reason: match r.u8()? {
                1 => GapReason::Backpressure,
                2 => GapReason::Paused,
                3 => GapReason::Reconnect,
                _ => GapReason::Clock,
            },
            real_us: r.u64()?,
        },
        KIND_END => Body::End(parse_json(kind, body)?),
        _ => Body::Unknown {
            kind,
            body: body.to_vec(),
        },
    };
    Ok(Record { t_us, body })
}

fn parse_json<T: for<'de> Deserialize<'de>>(kind: u8, body: &[u8]) -> Result<T, RecordError> {
    serde_json::from_slice(body).map_err(|_| RecordError::BadBody {
        kind,
        reason: "invalid JSON",
    })
}

struct BodyReader<'a> {
    buf: &'a [u8],
    kind: u8,
}

impl BodyReader<'_> {
    fn take<const N: usize>(&mut self) -> Result<[u8; N], RecordError> {
        let (value, rest) = self
            .buf
            .split_first_chunk::<N>()
            .ok_or_else(|| bad_len(self.kind))?;
        self.buf = rest;
        Ok(*value)
    }

    fn u8(&mut self) -> Result<u8, RecordError> {
        Ok(self.take::<1>()?[0])
    }

    fn u32(&mut self) -> Result<u32, RecordError> {
        Ok(u32::from_be_bytes(self.take()?))
    }

    fn u64(&mut self) -> Result<u64, RecordError> {
        Ok(u64::from_be_bytes(self.take()?))
    }

    fn f32(&mut self) -> Result<f32, RecordError> {
        Ok(f32::from_bits(self.u32()?))
    }

    fn boolean(&mut self) -> Result<bool, RecordError> {
        match self.u8()? {
            0 => Ok(false),
            1 => Ok(true),
            _ => Err(RecordError::BadBody {
                kind: self.kind,
                reason: "invalid bool",
            }),
        }
    }
}

fn bad_len(kind: u8) -> RecordError {
    RecordError::BadBody {
        kind,
        reason: "invalid length",
    }
}

fn put_bool(out: &mut Vec<u8>, value: bool) {
    out.push(u8::from(value));
}

fn put_f32(out: &mut Vec<u8>, value: f32) {
    out.extend_from_slice(&value.to_bits().to_be_bytes());
}

fn put_u16(out: &mut Vec<u8>, value: u16) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u32(out: &mut Vec<u8>, value: u32) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_u64(out: &mut Vec<u8>, value: u64) {
    out.extend_from_slice(&value.to_be_bytes());
}

fn put_i64(out: &mut Vec<u8>, value: i64) {
    out.extend_from_slice(&value.to_be_bytes());
}
