//! Input events on the client → host input stream.
//!
//! Pointer coordinates are normalized to the streamed display (0..1, origin
//! top-left), so the client does not need to know the host's point size.
//! Button events carry their own position: the host moves the cursor there
//! before pressing, which keeps clicks correct even if a move was dropped.

use crate::wire::{frame, DecodeError, Reader, Writer};

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u8)]
pub enum MouseButton {
    Left = 0,
    Right = 1,
    Middle = 2,
}

impl MouseButton {
    pub fn from_u8(v: u8) -> Result<Self, DecodeError> {
        match v {
            0 => Ok(MouseButton::Left),
            1 => Ok(MouseButton::Right),
            2 => Ok(MouseButton::Middle),
            _ => Err(DecodeError::InvalidValue("mouse button")),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq)]
pub enum InputEvent {
    MouseMove {
        x: f32,
        y: f32,
    },
    /// `clicks` is the click count (1 = single, 2 = double …), used by the host
    /// to set the OS click state.
    MouseButton {
        button: MouseButton,
        down: bool,
        x: f32,
        y: f32,
        clicks: u8,
    },
    /// Finger movement in pixels; content follows the finger like a trackpad
    /// with natural scrolling (positive `dy` = finger moved down).
    Scroll {
        dx: f32,
        dy: f32,
    },
}

const T_MOVE: u8 = 1;
const T_BUTTON: u8 = 2;
const T_SCROLL: u8 = 3;

fn checked_coord(v: f32) -> Result<f32, DecodeError> {
    if v.is_finite() {
        Ok(v.clamp(0.0, 1.0))
    } else {
        Err(DecodeError::InvalidValue("coordinate"))
    }
}

impl InputEvent {
    pub fn encode(&self) -> Vec<u8> {
        let mut w = Writer::new();
        match *self {
            InputEvent::MouseMove { x, y } => {
                w.u8(T_MOVE).f32(x).f32(y);
            }
            InputEvent::MouseButton {
                button,
                down,
                x,
                y,
                clicks,
            } => {
                w.u8(T_BUTTON)
                    .u8(button as u8)
                    .u8(down as u8)
                    .f32(x)
                    .f32(y)
                    .u8(clicks);
            }
            InputEvent::Scroll { dx, dy } => {
                w.u8(T_SCROLL).f32(dx).f32(dy);
            }
        }
        w.into_vec()
    }

    pub fn to_frame(&self) -> Vec<u8> {
        frame(&self.encode())
    }

    /// Decodes and sanitizes: coordinates are clamped to 0..1, NaN/inf rejected.
    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let ev = match r.u8()? {
            T_MOVE => InputEvent::MouseMove {
                x: checked_coord(r.f32()?)?,
                y: checked_coord(r.f32()?)?,
            },
            T_BUTTON => {
                let button = MouseButton::from_u8(r.u8()?)?;
                let down = r.u8()? != 0;
                let x = checked_coord(r.f32()?)?;
                let y = checked_coord(r.f32()?)?;
                let clicks = r.u8()?.max(1);
                InputEvent::MouseButton {
                    button,
                    down,
                    x,
                    y,
                    clicks,
                }
            }
            T_SCROLL => {
                let dx = r.f32()?;
                let dy = r.f32()?;
                if !dx.is_finite() || !dy.is_finite() {
                    return Err(DecodeError::InvalidValue("scroll delta"));
                }
                InputEvent::Scroll { dx, dy }
            }
            t => return Err(DecodeError::UnknownType(t)),
        };
        r.finish()?;
        Ok(ev)
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn events_round_trip() {
        let evs = [
            InputEvent::MouseMove { x: 0.1, y: 0.9 },
            InputEvent::MouseButton {
                button: MouseButton::Right,
                down: true,
                x: 0.5,
                y: 0.5,
                clicks: 1,
            },
            InputEvent::MouseButton {
                button: MouseButton::Left,
                down: false,
                x: 0.0,
                y: 1.0,
                clicks: 2,
            },
            InputEvent::Scroll { dx: -3.5, dy: 12.0 },
        ];
        for e in evs {
            assert_eq!(InputEvent::decode(&e.encode()).unwrap(), e);
        }
    }

    #[test]
    fn coordinates_are_clamped_and_nan_rejected() {
        let e = InputEvent::MouseMove { x: 1.7, y: -0.2 };
        assert_eq!(
            InputEvent::decode(&e.encode()).unwrap(),
            InputEvent::MouseMove { x: 1.0, y: 0.0 }
        );
        let bad = InputEvent::MouseMove {
            x: f32::NAN,
            y: 0.0,
        };
        assert_eq!(
            InputEvent::decode(&bad.encode()),
            Err(DecodeError::InvalidValue("coordinate"))
        );
    }

    #[test]
    fn zero_click_count_becomes_one() {
        let e = InputEvent::MouseButton {
            button: MouseButton::Left,
            down: true,
            x: 0.2,
            y: 0.2,
            clicks: 0,
        };
        match InputEvent::decode(&e.encode()).unwrap() {
            InputEvent::MouseButton { clicks, .. } => assert_eq!(clicks, 1),
            other => panic!("unexpected {other:?}"),
        }
    }
}
