//! Small, self-contained helpers for the SPA PODs used by PipeWire.
//!
//! The real SPA headers are deliberately not included here.  SPA PODs have a
//! simple little-endian representation, and keeping the representation in
//! this module makes the PipeWire FFI layer independent of libspa headers.

pub mod ty {
    pub const NONE: u32 = 1;
    pub const BOOL: u32 = 2;
    pub const ID: u32 = 3;
    pub const INT: u32 = 4;
    pub const LONG: u32 = 5;
    pub const FLOAT: u32 = 6;
    pub const DOUBLE: u32 = 7;
    pub const STRING: u32 = 8;
    pub const BYTES: u32 = 9;
    pub const RECTANGLE: u32 = 10;
    pub const FRACTION: u32 = 11;
    pub const BITMAP: u32 = 12;
    pub const ARRAY: u32 = 13;
    pub const STRUCT: u32 = 14;
    pub const OBJECT: u32 = 15;
    pub const SEQUENCE: u32 = 16;
    pub const POINTER: u32 = 17;
    pub const FD: u32 = 18;
    pub const CHOICE: u32 = 19;
    pub const POD: u32 = 20;
}

pub mod choice {
    pub const NONE: u32 = 0;
    pub const RANGE: u32 = 1;
    pub const STEP: u32 = 2;
    pub const ENUM: u32 = 3;
    pub const FLAGS: u32 = 4;
}

pub mod object_type {
    pub const FORMAT: u32 = 0x40003;
    pub const PARAM_BUFFERS: u32 = 0x40004;
    pub const PARAM_META: u32 = 0x40005;
}

pub mod param {
    pub const ENUM_FORMAT: u32 = 3;
    pub const FORMAT: u32 = 4;
    pub const BUFFERS: u32 = 5;
    pub const META: u32 = 6;
}

pub mod format_key {
    pub const MEDIA_TYPE: u32 = 1;
    pub const MEDIA_SUBTYPE: u32 = 2;
    pub const AUDIO_FORMAT: u32 = 0x10001;
    pub const AUDIO_RATE: u32 = 0x10003;
    pub const AUDIO_CHANNELS: u32 = 0x10004;
    pub const AUDIO_POSITION: u32 = 0x10005;
    pub const VIDEO_FORMAT: u32 = 0x20001;
    pub const VIDEO_MODIFIER: u32 = 0x20002;
    pub const VIDEO_SIZE: u32 = 0x20003;
    pub const VIDEO_FRAMERATE: u32 = 0x20004;
    pub const VIDEO_MAX_FRAMERATE: u32 = 0x20005;
}

pub mod media {
    pub const TYPE_AUDIO: u32 = 1;
    pub const TYPE_VIDEO: u32 = 2;
    pub const SUBTYPE_RAW: u32 = 1;
}

pub mod video_format {
    pub const RGBX: u32 = 7;
    pub const BGRX: u32 = 8;
    pub const XRGB: u32 = 9;
    pub const XBGR: u32 = 10;
    pub const RGBA: u32 = 11;
    pub const BGRA: u32 = 12;
    pub const ARGB: u32 = 13;
    pub const ABGR: u32 = 14;
}

pub mod audio_format {
    pub const S16_LE: u32 = 0x103;
    pub const F32_LE: u32 = 0x11b;
}

pub mod audio_channel {
    pub const MONO: u32 = 2;
    pub const FL: u32 = 3;
    pub const FR: u32 = 4;
}

pub mod buffers_key {
    pub const BUFFERS: u32 = 1;
    pub const BLOCKS: u32 = 2;
    pub const SIZE: u32 = 3;
    pub const STRIDE: u32 = 4;
    pub const ALIGN: u32 = 5;
    pub const DATA_TYPE: u32 = 6;
    pub const META_TYPE: u32 = 7;
}

pub mod meta_key {
    pub const TYPE: u32 = 1;
    pub const SIZE: u32 = 2;
}

pub mod meta_type {
    pub const HEADER: u32 = 1;
    pub const VIDEO_CROP: u32 = 2;
    pub const VIDEO_DAMAGE: u32 = 3;
    pub const BITMAP: u32 = 4;
    pub const CURSOR: u32 = 5;
}

pub mod data_type {
    pub const MEM_PTR: u32 = 1;
    pub const MEM_FD: u32 = 2;
    pub const DMA_BUF: u32 = 3;
}

pub mod header_flag {
    pub const DISCONT: u32 = 1 << 0;
    pub const CORRUPTED: u32 = 1 << 1;
}

pub mod chunk_flag {
    pub const CORRUPTED: i32 = 1 << 0;
    pub const EMPTY: i32 = 1 << 1;
}

/// `sizeof(spa_meta_header)` and the other metadata structures used here.
pub const META_HEADER_SIZE: usize = 32;
pub const META_REGION_SIZE: usize = 16;
pub const META_CURSOR_SIZE: usize = 28;
pub const META_BITMAP_SIZE: usize = 20;

/// Size of a cursor metadata block (cursor + bitmap header + ARGB pixels).
pub const fn cursor_meta_size(width: u32, height: u32) -> i32 {
    (48u64 + (width as u64) * (height as u64) * 4) as i32
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Value {
    Id(u32),
    Int(i32),
    Long(i64),
    Bool(bool),
    Rect { width: u32, height: u32 },
    Frac { num: u32, denom: u32 },
    IdArray(Vec<u32>),
    Choice { kind: u32, values: Vec<Value> },
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum PodError {
    #[error("pod truncated")]
    Truncated,
    #[error("unexpected pod type {found}, want {want}")]
    Type { found: u32, want: u32 },
    #[error("missing property {0:#x}")]
    Missing(u32),
}

/// A parsed object.  The property bytes remain borrowed from the original POD
/// so parsing does not allocate or copy the object itself.
pub struct PodObject<'a> {
    pub object_type: u32,
    pub id: u32,
    props: &'a [u8],
}

impl std::fmt::Debug for PodObject<'_> {
    fn fmt(&self, f: &mut std::fmt::Formatter<'_>) -> std::fmt::Result {
        f.debug_struct("PodObject")
            .field("object_type", &self.object_type)
            .field("id", &self.id)
            .field("keys", &self.keys())
            .finish()
    }
}

fn align8(value: usize) -> Option<usize> {
    value.checked_add(7).map(|v| v & !7)
}

fn put_u32(dst: &mut Vec<u8>, value: u32) {
    dst.extend_from_slice(&value.to_le_bytes());
}

fn put_i32(dst: &mut Vec<u8>, value: i32) {
    dst.extend_from_slice(&value.to_le_bytes());
}

fn put_i64(dst: &mut Vec<u8>, value: i64) {
    dst.extend_from_slice(&value.to_le_bytes());
}

fn pod(pod_type: u32, payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(8 + align8(payload.len()).unwrap_or(payload.len()));
    put_u32(&mut out, payload.len() as u32);
    put_u32(&mut out, pod_type);
    out.extend_from_slice(payload);
    while out.len() % 8 != 0 {
        out.push(0);
    }
    out
}

fn scalar_type(value: &Value) -> Option<(u32, usize)> {
    match value {
        Value::Id(_) => Some((ty::ID, 4)),
        Value::Int(_) => Some((ty::INT, 4)),
        Value::Rect { .. } => Some((ty::RECTANGLE, 8)),
        Value::Frac { .. } => Some((ty::FRACTION, 8)),
        Value::Long(_) | Value::Bool(_) | Value::IdArray(_) | Value::Choice { .. } => None,
    }
}

fn put_scalar(dst: &mut Vec<u8>, value: &Value) -> bool {
    match value {
        Value::Id(value) => {
            put_u32(dst, *value);
            true
        }
        Value::Int(value) => {
            put_i32(dst, *value);
            true
        }
        Value::Long(value) => {
            put_i64(dst, *value);
            true
        }
        Value::Bool(value) => {
            put_u32(dst, u32::from(*value));
            true
        }
        Value::Rect { width, height } => {
            put_u32(dst, *width);
            put_u32(dst, *height);
            true
        }
        Value::Frac { num, denom } => {
            put_u32(dst, *num);
            put_u32(dst, *denom);
            true
        }
        Value::IdArray(_) | Value::Choice { .. } => false,
    }
}

fn encode_value(value: &Value) -> Vec<u8> {
    match value {
        Value::Id(value) => {
            let mut payload = Vec::with_capacity(4);
            put_u32(&mut payload, *value);
            pod(ty::ID, &payload)
        }
        Value::Int(value) => {
            let mut payload = Vec::with_capacity(4);
            put_i32(&mut payload, *value);
            pod(ty::INT, &payload)
        }
        Value::Long(value) => {
            let mut payload = Vec::with_capacity(8);
            put_i64(&mut payload, *value);
            pod(ty::LONG, &payload)
        }
        Value::Bool(value) => {
            let mut payload = Vec::with_capacity(4);
            put_u32(&mut payload, u32::from(*value));
            pod(ty::BOOL, &payload)
        }
        Value::Rect { width, height } => {
            let mut payload = Vec::with_capacity(8);
            put_u32(&mut payload, *width);
            put_u32(&mut payload, *height);
            pod(ty::RECTANGLE, &payload)
        }
        Value::Frac { num, denom } => {
            let mut payload = Vec::with_capacity(8);
            put_u32(&mut payload, *num);
            put_u32(&mut payload, *denom);
            pod(ty::FRACTION, &payload)
        }
        Value::IdArray(values) => {
            let mut payload = Vec::with_capacity(8 + values.len() * 4);
            put_u32(&mut payload, 4);
            put_u32(&mut payload, ty::ID);
            for value in values {
                put_u32(&mut payload, *value);
            }
            pod(ty::ARRAY, &payload)
        }
        Value::Choice { kind, values } => {
            let valid = values
                .first()
                .and_then(scalar_type)
                .is_some_and(|first| values.iter().all(|value| scalar_type(value) == Some(first)));
            debug_assert!(valid, "a choice must contain one non-empty scalar type");
            if !valid {
                return pod(ty::CHOICE, &[0, 0, 0, 0, 0, 0, 0, 0]);
            }

            let (value_type, value_size) = scalar_type(&values[0]).unwrap_or((ty::NONE, 0));
            let mut payload = Vec::with_capacity(16 + values.len() * value_size);
            put_u32(&mut payload, *kind);
            put_u32(&mut payload, 0);
            put_u32(&mut payload, value_size as u32);
            put_u32(&mut payload, value_type);
            for value in values {
                // `valid` above guarantees that every value is a scalar of
                // the same type, so this cannot fail.
                let _ = put_scalar(&mut payload, value);
            }
            pod(ty::CHOICE, &payload)
        }
    }
}

/// Construct an object POD and append its properties in the supplied order.
pub fn object(object_type: u32, id: u32, props: &[(u32, Value)]) -> Vec<u8> {
    let mut payload = Vec::new();
    put_u32(&mut payload, object_type);
    put_u32(&mut payload, id);
    for (key, value) in props {
        put_u32(&mut payload, *key);
        put_u32(&mut payload, 0);
        payload.extend_from_slice(&encode_value(value));
    }
    pod(ty::OBJECT, &payload)
}

/// Construct the video format enum sent while negotiating a PipeWire stream.
pub fn video_enum_format(
    formats: &[u32],
    default_size: (u32, u32),
    max_size: (u32, u32),
    fps: u32,
) -> Vec<u8> {
    let mut format_values = Vec::with_capacity(formats.len() + 1);
    if let Some(default) = formats.first() {
        format_values.push(Value::Id(*default));
        format_values.extend(formats.iter().copied().map(Value::Id));
    }
    let format = if format_values.is_empty() {
        Value::Choice {
            kind: choice::NONE,
            values: Vec::new(),
        }
    } else {
        Value::Choice {
            kind: choice::ENUM,
            values: format_values,
        }
    };
    object(
        object_type::FORMAT,
        param::ENUM_FORMAT,
        &[
            (format_key::MEDIA_TYPE, Value::Id(media::TYPE_VIDEO)),
            (format_key::MEDIA_SUBTYPE, Value::Id(media::SUBTYPE_RAW)),
            (format_key::VIDEO_FORMAT, format),
            (
                format_key::VIDEO_SIZE,
                Value::Choice {
                    kind: choice::RANGE,
                    values: vec![
                        Value::Rect {
                            width: default_size.0,
                            height: default_size.1,
                        },
                        Value::Rect {
                            width: 1,
                            height: 1,
                        },
                        Value::Rect {
                            width: max_size.0,
                            height: max_size.1,
                        },
                    ],
                },
            ),
            (
                format_key::VIDEO_FRAMERATE,
                Value::Choice {
                    kind: choice::RANGE,
                    values: vec![
                        Value::Frac { num: fps, denom: 1 },
                        Value::Frac { num: 0, denom: 1 },
                        Value::Frac { num: 360, denom: 1 },
                    ],
                },
            ),
        ],
    )
}

/// Construct the buffers parameter with a data type flags choice.
pub fn video_buffers(data_type_mask: i32) -> Vec<u8> {
    object(
        object_type::PARAM_BUFFERS,
        param::BUFFERS,
        &[(
            buffers_key::DATA_TYPE,
            Value::Choice {
                kind: choice::FLAGS,
                values: vec![Value::Int(data_type_mask)],
            },
        )],
    )
}

pub enum MetaSize {
    Fixed(i32),
    Range { default: i32, min: i32, max: i32 },
}

/// Construct a metadata parameter with a fixed or ranged metadata size.
pub fn meta_param(meta_type: u32, size: MetaSize) -> Vec<u8> {
    let size_value = match size {
        MetaSize::Fixed(size) => Value::Int(size),
        MetaSize::Range { default, min, max } => Value::Choice {
            kind: choice::RANGE,
            values: vec![Value::Int(default), Value::Int(min), Value::Int(max)],
        },
    };
    object(
        object_type::PARAM_META,
        param::META,
        &[
            (meta_key::TYPE, Value::Id(meta_type)),
            (meta_key::SIZE, size_value),
        ],
    )
}

/// Construct the audio raw format enum.
pub fn audio_enum_format(format: u32, rate: u32, channels: u32) -> Vec<u8> {
    let position = if channels == 1 {
        vec![audio_channel::MONO]
    } else {
        vec![audio_channel::FL, audio_channel::FR]
    };
    object(
        object_type::FORMAT,
        param::ENUM_FORMAT,
        &[
            (format_key::MEDIA_TYPE, Value::Id(media::TYPE_AUDIO)),
            (format_key::MEDIA_SUBTYPE, Value::Id(media::SUBTYPE_RAW)),
            (format_key::AUDIO_FORMAT, Value::Id(format)),
            (format_key::AUDIO_RATE, Value::Int(rate as i32)),
            (format_key::AUDIO_CHANNELS, Value::Int(channels as i32)),
            (format_key::AUDIO_POSITION, Value::IdArray(position)),
        ],
    )
}

fn read_u32(bytes: &[u8]) -> Option<u32> {
    let bytes: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(u32::from_le_bytes(bytes))
}

fn read_i32(bytes: &[u8]) -> Option<i32> {
    let bytes: [u8; 4] = bytes.get(..4)?.try_into().ok()?;
    Some(i32::from_le_bytes(bytes))
}

fn read_i64(bytes: &[u8]) -> Option<i64> {
    let bytes: [u8; 8] = bytes.get(..8)?.try_into().ok()?;
    Some(i64::from_le_bytes(bytes))
}

fn read_u64(bytes: &[u8]) -> Option<u64> {
    let bytes: [u8; 8] = bytes.get(..8)?.try_into().ok()?;
    Some(u64::from_le_bytes(bytes))
}

fn value_total(size: u32) -> Option<usize> {
    align8(size as usize)?.checked_add(8)
}

fn validate_properties(props: &[u8]) -> Result<(), PodError> {
    let mut offset = 0usize;
    while offset < props.len() {
        let remaining = props.len() - offset;
        if remaining < 8 {
            return Err(PodError::Truncated);
        }
        let value_start = offset.checked_add(8).ok_or(PodError::Truncated)?;
        let value_header = props
            .get(value_start..value_start.checked_add(8).ok_or(PodError::Truncated)?)
            .ok_or(PodError::Truncated)?;
        let size = read_u32(value_header).ok_or(PodError::Truncated)?;
        let total = value_total(size).ok_or(PodError::Truncated)?;
        if total > props.len() - value_start {
            return Err(PodError::Truncated);
        }
        offset = value_start.checked_add(total).ok_or(PodError::Truncated)?;
    }
    Ok(())
}

/// Parse an object POD and validate all property boundaries.
pub fn parse_object(pod: &[u8]) -> Result<PodObject<'_>, PodError> {
    if pod.len() < 8 {
        return Err(PodError::Truncated);
    }
    let size = read_u32(&pod[0..4]).ok_or(PodError::Truncated)?;
    let pod_type = read_u32(&pod[4..8]).ok_or(PodError::Truncated)?;
    if pod_type != ty::OBJECT {
        return Err(PodError::Type {
            found: pod_type,
            want: ty::OBJECT,
        });
    }
    let body_end = 8usize
        .checked_add(size as usize)
        .ok_or(PodError::Truncated)?;
    let pod_end = align8(body_end).ok_or(PodError::Truncated)?;
    if pod_end > pod.len() {
        return Err(PodError::Truncated);
    }
    let body = pod.get(8..body_end).ok_or(PodError::Truncated)?;
    if body.len() < 8 {
        return Err(PodError::Truncated);
    }
    let object_type = read_u32(&body[0..4]).ok_or(PodError::Truncated)?;
    let id = read_u32(&body[4..8]).ok_or(PodError::Truncated)?;
    let props = &body[8..];
    validate_properties(props)?;
    Ok(PodObject {
        object_type,
        id,
        props,
    })
}

fn decode_scalar(pod_type: u32, payload: &[u8]) -> Option<Value> {
    match pod_type {
        ty::ID if payload.len() == 4 => Some(Value::Id(read_u32(payload)?)),
        ty::INT if payload.len() == 4 => Some(Value::Int(read_i32(payload)?)),
        ty::LONG if payload.len() == 8 => Some(Value::Long(read_i64(payload)?)),
        ty::BOOL if payload.len() == 4 => Some(Value::Bool(read_u32(payload)? != 0)),
        ty::RECTANGLE if payload.len() == 8 => Some(Value::Rect {
            width: read_u32(&payload[0..4])?,
            height: read_u32(&payload[4..8])?,
        }),
        ty::FRACTION if payload.len() == 8 => Some(Value::Frac {
            num: read_u32(&payload[0..4])?,
            denom: read_u32(&payload[4..8])?,
        }),
        _ => None,
    }
}

fn decode_choice(payload: &[u8]) -> Option<Value> {
    if payload.len() < 8 {
        return None;
    }
    let kind = read_u32(&payload[0..4])?;
    // The second word is choice flags.  Current constructors always use zero,
    // but readers intentionally tolerate flags from a newer PipeWire.
    let _flags = read_u32(&payload[4..8])?;
    if payload.len() == 8 {
        return Some(Value::Choice {
            kind,
            values: Vec::new(),
        });
    }
    if payload.len() < 16 {
        return None;
    }
    let value_size = read_u32(&payload[8..12])? as usize;
    let value_type = read_u32(&payload[12..16])?;
    if value_size == 0 {
        return None;
    }
    let values_bytes = &payload[16..];
    if !values_bytes.len().is_multiple_of(value_size) {
        return None;
    }
    let mut values = Vec::with_capacity(values_bytes.len() / value_size);
    for value_bytes in values_bytes.chunks_exact(value_size) {
        values.push(decode_scalar(value_type, value_bytes)?);
    }
    Some(Value::Choice { kind, values })
}

fn decode_array(payload: &[u8]) -> Option<Value> {
    if payload.len() < 8 {
        return None;
    }
    let element_size = read_u32(&payload[0..4])? as usize;
    let element_type = read_u32(&payload[4..8])?;
    if element_size != 4 || element_type != ty::ID {
        return None;
    }
    let values_bytes = &payload[8..];
    if !values_bytes.len().is_multiple_of(element_size) {
        return None;
    }
    let mut values = Vec::with_capacity(values_bytes.len() / element_size);
    for value_bytes in values_bytes.chunks_exact(element_size) {
        values.push(read_u32(value_bytes)?);
    }
    Some(Value::IdArray(values))
}

fn decode_pod(bytes: &[u8]) -> Option<Value> {
    if bytes.len() < 8 {
        return None;
    }
    let size = read_u32(&bytes[0..4])? as usize;
    let pod_type = read_u32(&bytes[4..8])?;
    let end = 8usize.checked_add(size)?;
    let payload = bytes.get(8..end)?;
    match pod_type {
        ty::ARRAY => decode_array(payload),
        ty::CHOICE => decode_choice(payload),
        _ => decode_scalar(pod_type, payload),
    }
}

impl PodObject<'_> {
    /// Decode one property.  Unknown SPA POD types are intentionally omitted.
    pub fn get(&self, key: u32) -> Option<Value> {
        let mut offset = 0usize;
        while offset < self.props.len() {
            let value_start = offset.checked_add(8)?;
            let value_header = self.props.get(value_start..value_start.checked_add(8)?)?;
            let property_key = read_u32(self.props.get(offset..offset.checked_add(4)?)?)?;
            let size = read_u32(value_header)?;
            let total = value_total(size)?;
            let value_end = value_start.checked_add(total)?;
            let value_bytes = self.props.get(value_start..value_end)?;
            if property_key == key {
                return decode_pod(value_bytes);
            }
            offset = value_end;
        }
        None
    }

    /// Return property keys in their encoded order.
    pub fn keys(&self) -> Vec<u32> {
        let mut keys = Vec::new();
        let mut offset = 0usize;
        while offset < self.props.len() {
            let Some(value_start) = offset.checked_add(8) else {
                break;
            };
            let Some(key_bytes) = self.props.get(offset..offset + 4) else {
                break;
            };
            let Some(value_header) = self.props.get(value_start..value_start + 8) else {
                break;
            };
            let Some(key) = read_u32(key_bytes) else {
                break;
            };
            let Some(size) = read_u32(value_header) else {
                break;
            };
            let Some(total) = value_total(size) else {
                break;
            };
            let Some(next) = value_start.checked_add(total) else {
                break;
            };
            if next > self.props.len() {
                break;
            }
            keys.push(key);
            offset = next;
        }
        keys
    }
}

/// Replace a choice by its default (first) value.
pub fn fixated(value: Value) -> Value {
    match value {
        Value::Choice { values, .. } => {
            values
                .into_iter()
                .next()
                .map(fixated)
                .unwrap_or(Value::Choice {
                    kind: choice::NONE,
                    values: Vec::new(),
                })
        }
        value => value,
    }
}

fn required_id(object: &PodObject<'_>, key: u32) -> Result<u32, PodError> {
    match object.get(key).map(fixated) {
        Some(Value::Id(value)) => Ok(value),
        _ => Err(PodError::Missing(key)),
    }
}

fn require_object_type(object: &PodObject<'_>, expected: u32) -> Result<(), PodError> {
    if object.object_type == expected {
        Ok(())
    } else {
        Err(PodError::Type {
            found: object.object_type,
            want: expected,
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct VideoFormat {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub framerate: (u32, u32),
    pub modifier: Option<u64>,
}

/// Parse a fixed or enumerated raw video format object.
pub fn parse_video_format(pod: &[u8]) -> Result<VideoFormat, PodError> {
    let object = parse_object(pod)?;
    require_object_type(&object, object_type::FORMAT)?;
    let media_type = required_id(&object, format_key::MEDIA_TYPE)?;
    if media_type != media::TYPE_VIDEO {
        return Err(PodError::Type {
            found: media_type,
            want: media::TYPE_VIDEO,
        });
    }
    let subtype = required_id(&object, format_key::MEDIA_SUBTYPE)?;
    if subtype != media::SUBTYPE_RAW {
        return Err(PodError::Type {
            found: subtype,
            want: media::SUBTYPE_RAW,
        });
    }
    let format = required_id(&object, format_key::VIDEO_FORMAT)?;
    let (width, height) = match object.get(format_key::VIDEO_SIZE).map(fixated) {
        Some(Value::Rect { width, height }) => (width, height),
        _ => return Err(PodError::Missing(format_key::VIDEO_SIZE)),
    };
    let framerate = match object.get(format_key::VIDEO_FRAMERATE).map(fixated) {
        Some(Value::Frac { num, denom }) => (num, denom),
        _ => (0, 1),
    };
    let modifier = match object.get(format_key::VIDEO_MODIFIER).map(fixated) {
        Some(Value::Long(value)) => Some(value as u64),
        _ => None,
    };
    Ok(VideoFormat {
        format,
        width,
        height,
        framerate,
        modifier,
    })
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AudioFormat {
    pub format: u32,
    pub rate: u32,
    pub channels: u32,
}

/// Parse a fixed or enumerated raw audio format object.
pub fn parse_audio_format(pod: &[u8]) -> Result<AudioFormat, PodError> {
    let object = parse_object(pod)?;
    require_object_type(&object, object_type::FORMAT)?;
    let media_type = required_id(&object, format_key::MEDIA_TYPE)?;
    if media_type != media::TYPE_AUDIO {
        return Err(PodError::Type {
            found: media_type,
            want: media::TYPE_AUDIO,
        });
    }
    let subtype = required_id(&object, format_key::MEDIA_SUBTYPE)?;
    if subtype != media::SUBTYPE_RAW {
        return Err(PodError::Type {
            found: subtype,
            want: media::SUBTYPE_RAW,
        });
    }
    let format = required_id(&object, format_key::AUDIO_FORMAT)?;
    let rate = match object.get(format_key::AUDIO_RATE).map(fixated) {
        Some(Value::Int(value)) if value >= 0 => value as u32,
        _ => return Err(PodError::Missing(format_key::AUDIO_RATE)),
    };
    let channels = match object.get(format_key::AUDIO_CHANNELS).map(fixated) {
        Some(Value::Int(value)) if value >= 0 => value as u32,
        _ => return Err(PodError::Missing(format_key::AUDIO_CHANNELS)),
    };
    Ok(AudioFormat {
        format,
        rate,
        channels,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MetaHeader {
    pub flags: u32,
    pub offset: u32,
    pub pts: i64,
    pub dts_offset: i64,
    pub seq: u64,
}

/// Parse the fixed-size `spa_meta_header` bytes.
pub fn parse_meta_header(bytes: &[u8]) -> Option<MetaHeader> {
    if bytes.len() < META_HEADER_SIZE {
        return None;
    }
    Some(MetaHeader {
        flags: read_u32(&bytes[0..4])?,
        offset: read_u32(&bytes[4..8])?,
        pts: read_i64(&bytes[8..16])?,
        dts_offset: read_i64(&bytes[16..24])?,
        seq: read_u64(&bytes[24..32])?,
    })
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Region {
    pub x: i32,
    pub y: i32,
    pub width: u32,
    pub height: u32,
}

/// Parse consecutive damage regions, stopping at the first zero-sized one.
pub fn parse_damage(bytes: &[u8]) -> Vec<Region> {
    let mut regions = Vec::new();
    for chunk in bytes.chunks_exact(META_REGION_SIZE) {
        let Some(width) = read_u32(&chunk[8..12]) else {
            break;
        };
        let Some(height) = read_u32(&chunk[12..16]) else {
            break;
        };
        if width == 0 || height == 0 {
            break;
        }
        let Some(x) = read_i32(&chunk[0..4]) else {
            break;
        };
        let Some(y) = read_i32(&chunk[4..8]) else {
            break;
        };
        regions.push(Region {
            x,
            y,
            width,
            height,
        });
    }
    regions
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorBitmap {
    pub format: u32,
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    pub pixels: Vec<u8>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CursorMeta {
    pub id: u32,
    pub flags: u32,
    pub x: i32,
    pub y: i32,
    pub hot_x: i32,
    pub hot_y: i32,
    pub bitmap: Option<CursorBitmap>,
}

/// Parse cursor metadata and, when present and complete, its bitmap pixels.
pub fn parse_cursor(bytes: &[u8]) -> Option<CursorMeta> {
    if bytes.len() < META_CURSOR_SIZE {
        return None;
    }
    let id = read_u32(&bytes[0..4])?;
    if id == 0 {
        return None;
    }
    let flags = read_u32(&bytes[4..8])?;
    let x = read_i32(&bytes[8..12])?;
    let y = read_i32(&bytes[12..16])?;
    let hot_x = read_i32(&bytes[16..20])?;
    let hot_y = read_i32(&bytes[20..24])?;
    let bitmap_offset = read_u32(&bytes[24..28])? as usize;
    let bitmap = if bitmap_offset == 0 {
        None
    } else {
        parse_cursor_bitmap(bytes, bitmap_offset)
    };
    Some(CursorMeta {
        id,
        flags,
        x,
        y,
        hot_x,
        hot_y,
        bitmap,
    })
}

fn parse_cursor_bitmap(bytes: &[u8], bitmap_offset: usize) -> Option<CursorBitmap> {
    let bitmap_start = bitmap_offset;
    let header = bytes.get(bitmap_start..bitmap_start.checked_add(META_BITMAP_SIZE)?)?;
    let format = read_u32(&header[0..4])?;
    let width = read_u32(&header[4..8])?;
    let height = read_u32(&header[8..12])?;
    let stride = read_i32(&header[12..16])?;
    let pixels_offset = read_u32(&header[16..20])? as usize;
    if width == 0 || height == 0 || stride < 0 {
        return None;
    }
    let min_stride = (width as usize).checked_mul(4)?;
    let stride = stride as usize;
    if stride < min_stride {
        return None;
    }
    let pixel_len = stride.checked_mul(height as usize)?;
    let pixels_start = bitmap_start.checked_add(pixels_offset)?;
    let pixels_end = pixels_start.checked_add(pixel_len)?;
    let pixels = bytes.get(pixels_start..pixels_end)?.to_vec();
    Some(CursorBitmap {
        format,
        width,
        height,
        stride: stride as u32,
        pixels,
    })
}
