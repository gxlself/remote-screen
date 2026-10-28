//! Audio datagrams. See `docs/dev/03-protocol.md` §8.3.
//!
//! Each datagram carries one Opus packet behind an 11-byte header:
//! `type u8 = 0x11 | seq u16 | capture_ts_us u64 | opus_frame`.
//! All integers are big-endian. The Opus payload is opaque to this crate.

use std::fmt;

use bytes::Bytes;

use crate::wire::{DecodeError, Reader, Writer};

/// Datagram type used on the mux audio channel.
pub const AUDIO_TYPE: u8 = 0x11;
/// Fixed header length: type, sequence number, and capture timestamp.
pub const AUDIO_HEADER_LEN: usize = 11;
/// Largest payload allowed by RFC 6716 for a single Opus frame.
pub const MAX_OPUS_PACKET: usize = 1275;
/// Largest complete audio datagram, including the type byte.
pub const MAX_AUDIO_DATAGRAM: usize = AUDIO_HEADER_LEN + MAX_OPUS_PACKET;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AudioHeader {
    pub seq: u16,
    pub capture_ts_us: u64,
}

/// Error returned when audio data cannot be encoded.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AudioEncodeError {
    EmptyPayload,
    PayloadTooLarge(usize),
}

impl fmt::Display for AudioEncodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::EmptyPayload => write!(f, "audio payload is empty"),
            Self::PayloadTooLarge(len) => write!(
                f,
                "audio payload is {len} bytes; the limit is {MAX_OPUS_PACKET} bytes"
            ),
        }
    }
}

impl std::error::Error for AudioEncodeError {}

/// Encodes a complete audio datagram, including its type byte.
pub fn encode(h: AudioHeader, opus: &[u8]) -> Result<Bytes, AudioEncodeError> {
    if opus.is_empty() {
        return Err(AudioEncodeError::EmptyPayload);
    }
    if opus.len() > MAX_OPUS_PACKET {
        return Err(AudioEncodeError::PayloadTooLarge(opus.len()));
    }

    let mut w = Writer::new();
    w.u8(AUDIO_TYPE).u16(h.seq).u64(h.capture_ts_us);
    let mut datagram = w.into_vec();
    datagram.reserve_exact(opus.len());
    datagram.extend_from_slice(opus);
    Ok(Bytes::from(datagram))
}

/// Decodes a complete audio datagram, including its type byte.
pub fn decode(buf: &[u8]) -> Result<(AudioHeader, &[u8]), DecodeError> {
    let Some(&ty) = buf.first() else {
        return Err(DecodeError::Truncated);
    };
    if ty != AUDIO_TYPE {
        return Err(DecodeError::UnknownType(ty));
    }
    if buf.len() < AUDIO_HEADER_LEN + 1 {
        return Err(DecodeError::Truncated);
    }
    if buf.len() > MAX_AUDIO_DATAGRAM {
        return Err(DecodeError::InvalidValue("audio payload length"));
    }

    let mut r = Reader::new(&buf[..AUDIO_HEADER_LEN]);
    r.u8()?;
    let seq = r.u16()?;
    let capture_ts_us = r.u64()?;
    r.finish()?;
    Ok((AudioHeader { seq, capture_ts_us }, &buf[AUDIO_HEADER_LEN..]))
}

/// Signed distance from `b` to `a` using wrapping `u16` arithmetic.
///
/// A positive result means `a` is newer than `b`. The half-range boundary is
/// represented by [`i16::MIN`], so `seq_newer` is false for that case.
pub fn seq_delta(a: u16, b: u16) -> i16 {
    a.wrapping_sub(b) as i16
}

/// Returns whether `a` is newer than `b` according to [`seq_delta`].
pub fn seq_newer(a: u16, b: u16) -> bool {
    seq_delta(a, b) > 0
}
