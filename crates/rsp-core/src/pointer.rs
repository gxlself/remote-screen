//! Pointer datagrams. See `docs/dev/03-protocol.md` §6.2.
//!
//! The datagram has a fixed 15-byte layout:
//! `type u8 | seq u32 | display u8 | flags u8 | x f32 | y f32`.
//! All integers and floats are big-endian.

use crate::wire::{DecodeError, Reader, Writer};

pub const POINTER_DATAGRAM_LEN: usize = 15;
pub const POINTER_TYPE: u8 = 0x12;

const FLAG_VISIBLE: u8 = 0x01;
const FLAG_MASK: u8 = FLAG_VISIBLE;

#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PointerDatagram {
    pub seq: u32,
    pub display: u8,
    pub visible: bool,
    pub x: f32,
    pub y: f32,
}

impl PointerDatagram {
    pub fn encode(&self) -> [u8; POINTER_DATAGRAM_LEN] {
        let mut w = Writer::new();
        w.u8(POINTER_TYPE)
            .u32(self.seq)
            .u8(self.display)
            .u8(if self.visible { FLAG_VISIBLE } else { 0 })
            .f32(self.x)
            .f32(self.y);
        let vec = w.into_vec();
        let mut out = [0; POINTER_DATAGRAM_LEN];
        out.copy_from_slice(&vec);
        out
    }

    pub fn decode(buf: &[u8]) -> Result<Self, DecodeError> {
        let mut r = Reader::new(buf);
        let ty = r.u8()?;
        if ty != POINTER_TYPE {
            return Err(DecodeError::UnknownType(ty));
        }
        let seq = r.u32()?;
        let display = r.u8()?;
        let flags = r.u8()?;
        if flags & !FLAG_MASK != 0 {
            return Err(DecodeError::InvalidValue("pointer flags"));
        }
        let x = r.f32()?;
        let y = r.f32()?;
        if !x.is_finite() || !y.is_finite() {
            return Err(DecodeError::InvalidValue("pointer coordinate"));
        }
        r.finish()?;
        Ok(Self {
            seq,
            display,
            visible: flags & FLAG_VISIBLE != 0,
            x,
            y,
        })
    }
}

/// Drops stale pointer datagrams by sequence number.
///
/// Sequence numbers are compared with wrapping arithmetic so that
/// `u32::MAX -> 0 -> 1` remains increasing. The first datagram is always
/// accepted.
#[derive(Debug, Default, Clone, Copy)]
pub struct PointerFilter {
    last_seq: Option<u32>,
}

impl PointerFilter {
    pub const fn new() -> Self {
        Self { last_seq: None }
    }

    pub fn accept(&mut self, datagram: &PointerDatagram) -> bool {
        if let Some(last) = self.last_seq {
            if datagram.seq.wrapping_sub(last) as i32 <= 0 {
                return false;
            }
        }
        self.last_seq = Some(datagram.seq);
        true
    }

    pub const fn last_seq(&self) -> Option<u32> {
        self.last_seq
    }
}
