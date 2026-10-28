//! Big-endian primitive encoding and length-prefixed framing for reliable streams.

use std::fmt;

/// Largest message accepted on a reliable stream.
pub const MAX_FRAME_LEN: usize = 1 << 20;

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum DecodeError {
    Truncated,
    TrailingBytes(usize),
    UnknownType(u8),
    BadUtf8,
    BadVersion(u8),
    FrameTooLarge(usize),
    InvalidValue(&'static str),
}

impl fmt::Display for DecodeError {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            DecodeError::Truncated => write!(f, "message truncated"),
            DecodeError::TrailingBytes(n) => write!(f, "{n} unexpected trailing bytes"),
            DecodeError::UnknownType(t) => write!(f, "unknown message type {t}"),
            DecodeError::BadUtf8 => write!(f, "string is not valid UTF-8"),
            DecodeError::BadVersion(v) => write!(f, "unsupported version {v}"),
            DecodeError::FrameTooLarge(n) => write!(f, "frame of {n} bytes exceeds limit"),
            DecodeError::InvalidValue(what) => write!(f, "invalid value for {what}"),
        }
    }
}

impl std::error::Error for DecodeError {}

#[derive(Default)]
pub struct Writer {
    buf: Vec<u8>,
}

impl Writer {
    pub fn new() -> Self {
        Self::default()
    }
    pub fn u8(&mut self, v: u8) -> &mut Self {
        self.buf.push(v);
        self
    }
    pub fn u16(&mut self, v: u16) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u32(&mut self, v: u32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn u64(&mut self, v: u64) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    pub fn f32(&mut self, v: f32) -> &mut Self {
        self.buf.extend_from_slice(&v.to_be_bytes());
        self
    }
    /// u16 length followed by UTF-8 bytes; longer strings are truncated on a char boundary.
    pub fn str(&mut self, s: &str) -> &mut Self {
        let mut end = s.len().min(u16::MAX as usize);
        while !s.is_char_boundary(end) {
            end -= 1;
        }
        self.u16(end as u16);
        self.buf.extend_from_slice(&s.as_bytes()[..end]);
        self
    }
    pub fn into_vec(self) -> Vec<u8> {
        self.buf
    }
}

pub struct Reader<'a> {
    buf: &'a [u8],
    pos: usize,
}

impl<'a> Reader<'a> {
    pub fn new(buf: &'a [u8]) -> Self {
        Self { buf, pos: 0 }
    }
    fn take(&mut self, n: usize) -> Result<&'a [u8], DecodeError> {
        if self.buf.len() - self.pos < n {
            return Err(DecodeError::Truncated);
        }
        let s = &self.buf[self.pos..self.pos + n];
        self.pos += n;
        Ok(s)
    }
    pub fn u8(&mut self) -> Result<u8, DecodeError> {
        Ok(self.take(1)?[0])
    }
    pub fn u16(&mut self) -> Result<u16, DecodeError> {
        Ok(u16::from_be_bytes(self.take(2)?.try_into().unwrap()))
    }
    pub fn u32(&mut self) -> Result<u32, DecodeError> {
        Ok(u32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn u64(&mut self) -> Result<u64, DecodeError> {
        Ok(u64::from_be_bytes(self.take(8)?.try_into().unwrap()))
    }
    pub fn f32(&mut self) -> Result<f32, DecodeError> {
        Ok(f32::from_be_bytes(self.take(4)?.try_into().unwrap()))
    }
    pub fn str(&mut self) -> Result<String, DecodeError> {
        let n = self.u16()? as usize;
        let bytes = self.take(n)?;
        String::from_utf8(bytes.to_vec()).map_err(|_| DecodeError::BadUtf8)
    }
    pub fn remaining(&self) -> &'a [u8] {
        &self.buf[self.pos..]
    }
    /// Errors if anything is left unread.
    pub fn finish(self) -> Result<(), DecodeError> {
        match self.buf.len() - self.pos {
            0 => Ok(()),
            n => Err(DecodeError::TrailingBytes(n)),
        }
    }
}

/// Prefixes `payload` with its u32 big-endian length.
pub fn frame(payload: &[u8]) -> Vec<u8> {
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out
}

/// Splits a byte stream back into length-prefixed frames.
#[derive(Default)]
pub struct FrameDecoder {
    buf: Vec<u8>,
}

impl FrameDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn push(&mut self, data: &[u8]) {
        self.buf.extend_from_slice(data);
    }

    /// Returns the next complete frame, `Ok(None)` if more bytes are needed.
    pub fn next_frame(&mut self) -> Result<Option<Vec<u8>>, DecodeError> {
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes(self.buf[..4].try_into().unwrap()) as usize;
        if len > MAX_FRAME_LEN {
            return Err(DecodeError::FrameTooLarge(len));
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let payload = self.buf[4..4 + len].to_vec();
        self.buf.drain(..4 + len);
        Ok(Some(payload))
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn primitives_round_trip() {
        let mut w = Writer::new();
        w.u8(7)
            .u16(0xBEEF)
            .u32(123_456)
            .u64(u64::MAX - 1)
            .f32(0.25)
            .str("屏幕 ok");
        let buf = w.into_vec();
        let mut r = Reader::new(&buf);
        assert_eq!(r.u8().unwrap(), 7);
        assert_eq!(r.u16().unwrap(), 0xBEEF);
        assert_eq!(r.u32().unwrap(), 123_456);
        assert_eq!(r.u64().unwrap(), u64::MAX - 1);
        assert_eq!(r.f32().unwrap(), 0.25);
        assert_eq!(r.str().unwrap(), "屏幕 ok");
        r.finish().unwrap();
    }

    #[test]
    fn reader_reports_truncation_and_trailing() {
        let mut r = Reader::new(&[0, 1]);
        assert_eq!(r.u32(), Err(DecodeError::Truncated));
        let mut r = Reader::new(&[1, 2, 3]);
        r.u8().unwrap();
        assert_eq!(r.finish(), Err(DecodeError::TrailingBytes(2)));
    }

    #[test]
    fn frame_decoder_handles_split_and_coalesced_input() {
        let a = frame(b"hello");
        let b = frame(b"");
        let c = frame(&[9u8; 300]);
        let mut all = Vec::new();
        all.extend_from_slice(&a);
        all.extend_from_slice(&b);
        all.extend_from_slice(&c);

        let mut d = FrameDecoder::new();
        let mut got = Vec::new();
        for chunk in all.chunks(3) {
            d.push(chunk);
            while let Some(f) = d.next_frame().unwrap() {
                got.push(f);
            }
        }
        assert_eq!(got, vec![b"hello".to_vec(), vec![], vec![9u8; 300]]);
    }

    #[test]
    fn frame_decoder_rejects_oversized_length() {
        let mut d = FrameDecoder::new();
        d.push(&((MAX_FRAME_LEN as u32) + 1).to_be_bytes());
        assert!(matches!(d.next_frame(), Err(DecodeError::FrameTooLarge(_))));
    }
}
