//! RSP/1 input-stream events. See `docs/dev/03-protocol.md` §6.1.
//!
//! These events supersede the M0 [`crate::input::InputEvent`] subset. The old
//! type stays in place for the macOS host; new code should use [`InputMsg`].

use std::fmt;
use std::ops::{BitOr, BitOrAssign};

use crate::wire::{frame, DecodeError, Reader, Writer};

/// Maximum UTF-8 payload of a [`InputMsg::Text`] event.
pub const MAX_TEXT_BYTES: usize = 4096;

const T_MOUSE_MOVE: u8 = 0x01;
const T_MOUSE_BUTTON: u8 = 0x02;
const T_SCROLL: u8 = 0x03;
const T_KEY: u8 = 0x04;
const T_TEXT: u8 = 0x05;
const T_RELEASE_ALL: u8 = 0x06;
const T_TOUCH: u8 = 0x07;

/// Mouse buttons, in protocol order.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum Button {
    Left = 0,
    Right = 1,
    Middle = 2,
    Back = 3,
    Forward = 4,
}

impl Button {
    pub fn from_u8(value: u8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::Left),
            1 => Ok(Self::Right),
            2 => Ok(Self::Middle),
            3 => Ok(Self::Back),
            4 => Ok(Self::Forward),
            _ => Err(DecodeError::InvalidValue("mouse button")),
        }
    }
}

/// Scroll gesture phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ScrollPhase {
    None = 0,
    Began = 1,
    Changed = 2,
    Ended = 3,
}

impl ScrollPhase {
    pub fn from_u8(value: u8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::None),
            1 => Ok(Self::Began),
            2 => Ok(Self::Changed),
            3 => Ok(Self::Ended),
            _ => Err(DecodeError::InvalidValue("scroll phase")),
        }
    }
}

/// Scroll delta units.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum ScrollUnit {
    Pixel = 0,
    Line = 1,
}

impl ScrollUnit {
    pub fn from_u8(value: u8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::Pixel),
            1 => Ok(Self::Line),
            _ => Err(DecodeError::InvalidValue("scroll unit")),
        }
    }
}

/// Native touch phase.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum TouchPhase {
    Began = 0,
    Moved = 1,
    Ended = 2,
    Cancelled = 3,
}

impl TouchPhase {
    pub fn from_u8(value: u8) -> Result<Self, DecodeError> {
        match value {
            0 => Ok(Self::Began),
            1 => Ok(Self::Moved),
            2 => Ok(Self::Ended),
            3 => Ok(Self::Cancelled),
            _ => Err(DecodeError::InvalidValue("touch phase")),
        }
    }
}

/// USB HID modifier state. Bit 0..7 are left Ctrl, Shift, Alt, Meta, then the
/// corresponding right-side modifiers.
#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct Mods(u8);

impl Mods {
    pub const NONE: Self = Self(0);
    pub const LCTRL: Self = Self(1 << 0);
    pub const LSHIFT: Self = Self(1 << 1);
    pub const LALT: Self = Self(1 << 2);
    pub const LMETA: Self = Self(1 << 3);
    pub const RCTRL: Self = Self(1 << 4);
    pub const RSHIFT: Self = Self(1 << 5);
    pub const RALT: Self = Self(1 << 6);
    pub const RMETA: Self = Self(1 << 7);

    pub const fn from_bits(bits: u8) -> Self {
        Self(bits)
    }

    pub const fn bits(self) -> u8 {
        self.0
    }

    pub const fn contains(self, other: Self) -> bool {
        self.0 & other.0 == other.0
    }
}

impl BitOr for Mods {
    type Output = Self;

    fn bitor(self, rhs: Self) -> Self::Output {
        Self(self.0 | rhs.0)
    }
}

impl BitOrAssign for Mods {
    fn bitor_assign(&mut self, rhs: Self) {
        self.0 |= rhs.0;
    }
}

/// All input-stream event variants from RSP/1 §6.1.
#[derive(Debug, Clone, PartialEq)]
pub enum InputMsg {
    MouseMove {
        x: f32,
        y: f32,
    },
    MouseButton {
        button: Button,
        down: bool,
        x: f32,
        y: f32,
        clicks: u8,
    },
    Scroll {
        dx: f32,
        dy: f32,
        phase: ScrollPhase,
        momentum: ScrollPhase,
        unit: ScrollUnit,
    },
    Key {
        usage: u16,
        down: bool,
        mods: Mods,
    },
    Text(String),
    ReleaseAll,
    Touch {
        id: u8,
        phase: TouchPhase,
        x: f32,
        y: f32,
        pressure: f32,
    },
}

/// Error returned when an event cannot be represented by the wire format.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EncodeError {
    TextTooLong(usize),
}

impl fmt::Display for EncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::TextTooLong(len) => write!(
                f,
                "text is {len} bytes; the limit is {MAX_TEXT_BYTES} bytes"
            ),
        }
    }
}

impl std::error::Error for EncodeError {}

impl InputMsg {
    /// Encodes the event, returning [`EncodeError::TextTooLong`] rather than
    /// truncating or panicking on oversized text.
    pub fn encode(&self) -> Result<Vec<u8>, EncodeError> {
        let mut w = Writer::new();
        match self {
            Self::MouseMove { x, y } => {
                w.u8(T_MOUSE_MOVE).f32(*x).f32(*y);
            }
            Self::MouseButton {
                button,
                down,
                x,
                y,
                clicks,
            } => {
                w.u8(T_MOUSE_BUTTON)
                    .u8(*button as u8)
                    .u8(*down as u8)
                    .f32(*x)
                    .f32(*y)
                    .u8(*clicks);
            }
            Self::Scroll {
                dx,
                dy,
                phase,
                momentum,
                unit,
            } => {
                w.u8(T_SCROLL)
                    .f32(*dx)
                    .f32(*dy)
                    .u8(*phase as u8)
                    .u8(*momentum as u8)
                    .u8(*unit as u8);
            }
            Self::Key { usage, down, mods } => {
                w.u8(T_KEY).u16(*usage).u8(*down as u8).u8(mods.bits());
            }
            Self::Text(text) => {
                if text.len() > MAX_TEXT_BYTES {
                    return Err(EncodeError::TextTooLong(text.len()));
                }
                let mut out = Vec::with_capacity(3 + text.len());
                out.push(T_TEXT);
                out.extend_from_slice(&(text.len() as u16).to_be_bytes());
                out.extend_from_slice(text.as_bytes());
                return Ok(out);
            }
            Self::ReleaseAll => {
                w.u8(T_RELEASE_ALL);
            }
            Self::Touch {
                id,
                phase,
                x,
                y,
                pressure,
            } => {
                w.u8(T_TOUCH)
                    .u8(*id)
                    .u8(*phase as u8)
                    .f32(*x)
                    .f32(*y)
                    .f32(*pressure);
            }
        }
        Ok(w.into_vec())
    }

    pub fn to_frame(&self) -> Result<Vec<u8>, EncodeError> {
        self.encode().map(|encoded| frame(&encoded))
    }

    /// Decodes one event. Coordinates are clamped to 0..1; non-finite values,
    /// unknown enum values, and trailing bytes are rejected.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let msg = match r.u8()? {
            T_MOUSE_MOVE => Self::MouseMove {
                x: decode_coord(r.f32()?)?,
                y: decode_coord(r.f32()?)?,
            },
            T_MOUSE_BUTTON => Self::MouseButton {
                button: Button::from_u8(r.u8()?)?,
                down: r.u8()? != 0,
                x: decode_coord(r.f32()?)?,
                y: decode_coord(r.f32()?)?,
                clicks: r.u8()?.max(1),
            },
            T_SCROLL => {
                let dx = r.f32()?;
                let dy = r.f32()?;
                if !dx.is_finite() || !dy.is_finite() {
                    return Err(DecodeError::InvalidValue("scroll delta"));
                }
                Self::Scroll {
                    dx,
                    dy,
                    phase: ScrollPhase::from_u8(r.u8()?)?,
                    momentum: ScrollPhase::from_u8(r.u8()?)?,
                    unit: ScrollUnit::from_u8(r.u8()?)?,
                }
            }
            T_KEY => Self::Key {
                usage: r.u16()?,
                down: r.u8()? != 0,
                mods: Mods::from_bits(r.u8()?),
            },
            T_TEXT => {
                let text = r.str()?;
                if text.len() > MAX_TEXT_BYTES {
                    return Err(DecodeError::InvalidValue("text length"));
                }
                Self::Text(text)
            }
            T_RELEASE_ALL => Self::ReleaseAll,
            T_TOUCH => Self::Touch {
                id: r.u8()?,
                phase: TouchPhase::from_u8(r.u8()?)?,
                x: decode_coord(r.f32()?)?,
                y: decode_coord(r.f32()?)?,
                pressure: decode_pressure(r.f32()?)?,
            },
            ty => return Err(DecodeError::UnknownType(ty)),
        };
        r.finish()?;
        Ok(msg)
    }
}

fn decode_coord(value: f32) -> Result<f32, DecodeError> {
    if !value.is_finite() {
        return Err(DecodeError::InvalidValue("coordinate"));
    }
    Ok(value.clamp(0.0, 1.0))
}

fn decode_pressure(value: f32) -> Result<f32, DecodeError> {
    if !value.is_finite() {
        return Err(DecodeError::InvalidValue("pressure"));
    }
    Ok(value.clamp(0.0, 1.0))
}
