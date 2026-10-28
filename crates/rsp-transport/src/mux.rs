//! Channel identifiers and wire helpers shared by the QUIC and TCP transports.

use bytes::Bytes;

pub const CONTROL_CHANNEL: u8 = 0x00;
pub const INPUT_CHANNEL: u8 = 0x01;
pub const CURSOR_CHANNEL: u8 = 0x02;
pub const CLIPBOARD_CHANNEL: u8 = 0x03;
pub const FILE_CHANNEL: u8 = 0x04;
pub const FILE_TCP_CHUNK: usize = 16 << 10;
pub const FILE_FLAG_FIN: u8 = 1;
pub const FILE_FLAG_RESET: u8 = 2;
pub const FILE_FLAG_STOP: u8 = 4;
pub const VIDEO_CHANNEL: u8 = 0x10;
pub const AUDIO_CHANNEL: u8 = 0x11;
pub const POINTER_CHANNEL: u8 = 0x12;
pub const PROBE_CHANNEL: u8 = 0x13;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum StreamKind {
    Input = INPUT_CHANNEL,
    Cursor = CURSOR_CHANNEL,
    Clipboard = CLIPBOARD_CHANNEL,
    File = FILE_CHANNEL,
}

impl TryFrom<u8> for StreamKind {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            INPUT_CHANNEL => Ok(Self::Input),
            CURSOR_CHANNEL => Ok(Self::Cursor),
            CLIPBOARD_CHANNEL => Ok(Self::Clipboard),
            FILE_CHANNEL => Ok(Self::File),
            _ => Err(()),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
#[repr(u8)]
pub enum DatagramKind {
    Video = VIDEO_CHANNEL,
    Audio = AUDIO_CHANNEL,
    Pointer = POINTER_CHANNEL,
    Probe = PROBE_CHANNEL,
}

impl TryFrom<u8> for DatagramKind {
    type Error = ();

    fn try_from(value: u8) -> Result<Self, Self::Error> {
        match value {
            VIDEO_CHANNEL => Ok(Self::Video),
            AUDIO_CHANNEL => Ok(Self::Audio),
            POINTER_CHANNEL => Ok(Self::Pointer),
            PROBE_CHANNEL => Ok(Self::Probe),
            _ => Err(()),
        }
    }
}

/// Clipboard chunks carry raw stream bytes, including any inner record prefix.
pub(crate) fn clipboard_chunk(item_id: u32, last: bool, payload: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(5 + payload.len());
    out.extend_from_slice(&item_id.to_be_bytes());
    out.push(u8::from(last));
    out.extend_from_slice(payload);
    Bytes::from(out)
}

/// File chunks carry raw stream bytes; RESET and STOP carry a u32 BE code.
pub fn file_chunk(slot: u32, flags: u8, data: &[u8]) -> Bytes {
    let mut out = Vec::with_capacity(5 + data.len());
    out.extend_from_slice(&slot.to_be_bytes());
    out.push(flags);
    out.extend_from_slice(data);
    Bytes::from(out)
}
