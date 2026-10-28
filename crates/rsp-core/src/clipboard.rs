//! 剪贴板流消息。见 docs/dev/03-protocol.md §8.1。
//!
//! QUIC 单向流的内容是 `u32 长度 + ClipboardHeader`，随后是原始数据。接收方
//! 在 FIN 后校验长度、SHA-256 与文本 UTF-8，全部通过才交出 [`ClipboardItem`]。
//! TCP 回退把同一前缀和数据切成带 `item_id` / `flags` 的记录，再由
//! [`TcpReassembler`] 按项重组。

use std::collections::HashMap;
use std::fmt;

use bytes::Bytes;
use prost::Message;
use sha2::{Digest, Sha256};
use thiserror::Error;

use crate::pb::clipboard_header::Kind;
use crate::pb::ClipboardHeader;

/// 文本剪贴板的最大字节数。
pub const MAX_TEXT_BYTES: usize = 1 << 20;
/// PNG 剪贴板的最大字节数。
pub const MAX_PNG_BYTES: usize = 10 << 20;
/// QUIC 流头长度前缀的最大值。
pub const MAX_HEADER_BYTES: usize = 1024;
/// TCP 回退上同时组装的剪贴板项目上限。
pub const MAX_TCP_ITEMS: usize = 4;

const LENGTH_PREFIX_BYTES: usize = 4;
const TCP_FLAG_LAST: u8 = 0x01;
const TCP_FLAG_MASK: u8 = TCP_FLAG_LAST;
const TCP_RECORD_HEADER_BYTES: usize = 5;
const SHA256_BYTES: usize = 32;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipboardKind {
    Text,
    Png,
}

impl ClipboardKind {
    pub const fn max_bytes(self) -> usize {
        match self {
            Self::Text => MAX_TEXT_BYTES,
            Self::Png => MAX_PNG_BYTES,
        }
    }
}

impl fmt::Display for ClipboardKind {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        match self {
            Self::Text => f.write_str("text"),
            Self::Png => f.write_str("PNG"),
        }
    }
}

impl From<ClipboardKind> for Kind {
    fn from(kind: ClipboardKind) -> Self {
        match kind {
            ClipboardKind::Text => Self::Text,
            ClipboardKind::Png => Self::Png,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ClipboardItem {
    pub kind: ClipboardKind,
    pub data: Bytes,
}

impl ClipboardItem {
    pub fn new(kind: ClipboardKind, data: impl Into<Bytes>) -> Result<Self, ClipboardError> {
        let item = Self {
            kind,
            data: data.into(),
        };
        item.validate()?;
        Ok(item)
    }

    fn validate(&self) -> Result<(), ClipboardError> {
        validate_data(self.kind, &self.data)
    }
}

#[derive(Debug, Error)]
pub enum ClipboardError {
    #[error("clipboard stream ended with no header")]
    EmptyStream,
    #[error("clipboard header length prefix {0} exceeds {MAX_HEADER_BYTES} bytes")]
    HeaderLengthTooLarge(usize),
    #[error("clipboard header truncated: declared {declared} bytes, received {received}")]
    HeaderTruncated { declared: usize, received: usize },
    #[error("clipboard header protobuf is invalid: {0}")]
    InvalidHeader(#[from] prost::DecodeError),
    #[error("clipboard header kind {0} is unsupported")]
    UnknownKind(i32),
    #[error("clipboard header SHA-256 is {0} bytes, expected {SHA256_BYTES}")]
    InvalidSha256Length(usize),
    #[error("clipboard {kind} is {size} bytes; the limit is {limit} bytes")]
    DataTooLarge {
        kind: ClipboardKind,
        size: u64,
        limit: usize,
    },
    #[error("clipboard data exceeds declared size {declared}: received {received} bytes")]
    DataExceedsDeclaredSize { declared: u64, received: u64 },
    #[error("clipboard size mismatch at FIN: declared {declared}, received {received}")]
    SizeMismatch { declared: u64, received: u64 },
    #[error("clipboard SHA-256 does not match the header")]
    Sha256Mismatch,
    #[error("clipboard text is not valid UTF-8")]
    InvalidUtf8,
    #[error("clipboard stream received data after it was poisoned by an earlier error")]
    StreamFinished,
    #[error("TCP clipboard record is {actual} bytes; expected at least {minimum}")]
    TcpRecordTooShort { actual: usize, minimum: usize },
    #[error("TCP clipboard record has unsupported flags 0x{flags:02x}")]
    TcpInvalidFlags { flags: u8 },
    #[error("more than {MAX_TCP_ITEMS} clipboard items are in flight over TCP")]
    TooManyTcpItems,
}

fn validate_data(kind: ClipboardKind, data: &[u8]) -> Result<(), ClipboardError> {
    let limit = kind.max_bytes();
    if data.len() > limit {
        return Err(ClipboardError::DataTooLarge {
            kind,
            size: data.len() as u64,
            limit,
        });
    }
    if kind == ClipboardKind::Text && std::str::from_utf8(data).is_err() {
        return Err(ClipboardError::InvalidUtf8);
    }
    Ok(())
}

fn checked_size(kind: ClipboardKind, size: u64) -> Result<usize, ClipboardError> {
    let limit = kind.max_bytes();
    if size > limit as u64 {
        return Err(ClipboardError::DataTooLarge { kind, size, limit });
    }
    usize::try_from(size).map_err(|_| ClipboardError::DataTooLarge { kind, size, limit })
}

fn encode_header(kind: ClipboardKind, data: &[u8]) -> Vec<u8> {
    ClipboardHeader {
        kind: Kind::from(kind) as i32,
        size: data.len() as u64,
        sha256: Sha256::digest(data).to_vec(),
    }
    .encode_to_vec()
}

/// 编码 QUIC 流的前缀：`u32 长度 + ClipboardHeader`。调用方随后写原始数据并
/// 在结束时 FIN。
pub fn encode_stream_prefix(item: &ClipboardItem) -> Result<Vec<u8>, ClipboardError> {
    item.validate()?;
    let header = encode_header(item.kind, &item.data);
    if header.len() > MAX_HEADER_BYTES {
        return Err(ClipboardError::HeaderLengthTooLarge(header.len()));
    }

    let mut out = Vec::with_capacity(LENGTH_PREFIX_BYTES + header.len());
    out.extend_from_slice(&(header.len() as u32).to_be_bytes());
    out.extend_from_slice(&header);
    Ok(out)
}

/// 编码一个完整的 QUIC 剪贴板流，主要用于测试和离线工具。
pub fn encode_stream(item: &ClipboardItem) -> Result<Vec<u8>, ClipboardError> {
    let mut out = encode_stream_prefix(item)?;
    out.extend_from_slice(&item.data);
    Ok(out)
}

#[derive(Debug)]
struct HeaderInfo {
    kind: ClipboardKind,
    size: usize,
    sha256: [u8; SHA256_BYTES],
}

fn parse_header(buf: &[u8]) -> Result<HeaderInfo, ClipboardError> {
    let header = ClipboardHeader::decode(buf)?;
    let kind = match Kind::try_from(header.kind) {
        Ok(Kind::Text) => ClipboardKind::Text,
        Ok(Kind::Png) => ClipboardKind::Png,
        Ok(Kind::Unspecified) | Err(_) => {
            return Err(ClipboardError::UnknownKind(header.kind));
        }
    };
    let size = checked_size(kind, header.size)?;
    let sha256 = <[u8; SHA256_BYTES]>::try_from(header.sha256.as_slice())
        .map_err(|_| ClipboardError::InvalidSha256Length(header.sha256.len()))?;
    Ok(HeaderInfo { kind, size, sha256 })
}

enum ReceiveState {
    Prefix {
        buf: [u8; LENGTH_PREFIX_BYTES],
        filled: usize,
    },
    Header {
        len: usize,
        buf: Vec<u8>,
    },
    Data {
        kind: ClipboardKind,
        size: usize,
        sha256: [u8; SHA256_BYTES],
        buf: Vec<u8>,
    },
    Poisoned,
}

impl ReceiveState {
    fn feed(mut self, mut input: &[u8]) -> Result<Self, ClipboardError> {
        loop {
            self = match self {
                Self::Prefix {
                    mut buf,
                    mut filled,
                } => {
                    let take = (LENGTH_PREFIX_BYTES - filled).min(input.len());
                    buf[filled..filled + take].copy_from_slice(&input[..take]);
                    filled += take;
                    input = &input[take..];
                    if filled < LENGTH_PREFIX_BYTES {
                        Self::Prefix { buf, filled }
                    } else {
                        let len = u32::from_be_bytes(buf) as usize;
                        if len > MAX_HEADER_BYTES {
                            return Err(ClipboardError::HeaderLengthTooLarge(len));
                        }
                        Self::Header {
                            len,
                            buf: Vec::with_capacity(len),
                        }
                    }
                }
                Self::Header { len, mut buf } => {
                    let take = (len - buf.len()).min(input.len());
                    buf.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if buf.len() < len {
                        Self::Header { len, buf }
                    } else {
                        let header = parse_header(&buf)?;
                        Self::Data {
                            kind: header.kind,
                            size: header.size,
                            sha256: header.sha256,
                            buf: Vec::with_capacity(header.size),
                        }
                    }
                }
                Self::Data {
                    kind,
                    size,
                    sha256,
                    mut buf,
                } => {
                    let remaining = size - buf.len();
                    if input.len() > remaining {
                        return Err(ClipboardError::DataExceedsDeclaredSize {
                            declared: size as u64,
                            received: (buf.len() as u64).saturating_add(input.len() as u64),
                        });
                    }
                    buf.extend_from_slice(input);
                    return Ok(Self::Data {
                        kind,
                        size,
                        sha256,
                        buf,
                    });
                }
                Self::Poisoned => return Err(ClipboardError::StreamFinished),
            };

            if input.is_empty() {
                return Ok(self);
            }
        }
    }
}

/// 接收一条 QUIC 剪贴板流。`feed` 可以接受任意切分的数据块。
pub struct StreamReceiver {
    state: ReceiveState,
}

impl Default for StreamReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl StreamReceiver {
    pub fn new() -> Self {
        Self {
            state: ReceiveState::Prefix {
                buf: [0; LENGTH_PREFIX_BYTES],
                filled: 0,
            },
        }
    }

    pub fn feed(&mut self, chunk: &[u8]) -> Result<(), ClipboardError> {
        if chunk.is_empty() {
            return Ok(());
        }

        let state = std::mem::replace(&mut self.state, ReceiveState::Poisoned);
        match state.feed(chunk) {
            Ok(state) => {
                self.state = state;
                Ok(())
            }
            Err(error) => {
                self.state = ReceiveState::Poisoned;
                Err(error)
            }
        }
    }

    pub fn finish(self) -> Result<ClipboardItem, ClipboardError> {
        match self.state {
            ReceiveState::Prefix { filled: 0, .. } => Err(ClipboardError::EmptyStream),
            ReceiveState::Prefix { filled, .. } => Err(ClipboardError::HeaderTruncated {
                declared: LENGTH_PREFIX_BYTES,
                received: filled,
            }),
            ReceiveState::Header { len, buf } => Err(ClipboardError::HeaderTruncated {
                declared: len,
                received: buf.len(),
            }),
            ReceiveState::Data {
                kind,
                size,
                sha256,
                buf,
            } => {
                if buf.len() != size {
                    return Err(ClipboardError::SizeMismatch {
                        declared: size as u64,
                        received: buf.len() as u64,
                    });
                }
                if Sha256::digest(&buf).as_slice() != sha256 {
                    return Err(ClipboardError::Sha256Mismatch);
                }
                if kind == ClipboardKind::Text && std::str::from_utf8(&buf).is_err() {
                    return Err(ClipboardError::InvalidUtf8);
                }
                Ok(ClipboardItem {
                    kind,
                    data: Bytes::from(buf),
                })
            }
            ReceiveState::Poisoned => Err(ClipboardError::StreamFinished),
        }
    }
}

fn tcp_record(item_id: u32, last: bool, data: &[u8]) -> Vec<u8> {
    let mut record = Vec::with_capacity(TCP_RECORD_HEADER_BYTES + data.len());
    record.extend_from_slice(&item_id.to_be_bytes());
    record.push(if last { TCP_FLAG_LAST } else { 0 });
    record.extend_from_slice(data);
    record
}

/// 把 `prefix_and_data` 切成 TCP clipboard 记录。`max_chunk` 限制每条记录的数据
/// 块大小，不含 5 字节记录头；最后一条记录置 flags bit0。
pub fn encode_tcp_chunks(item_id: u32, prefix_and_data: &[u8], max_chunk: usize) -> Vec<Vec<u8>> {
    if prefix_and_data.is_empty() {
        return vec![tcp_record(item_id, true, &[])];
    }
    let chunk_len = max_chunk.max(1);
    let chunk_count = prefix_and_data.len().div_ceil(chunk_len);
    (0..chunk_count)
        .map(|index| {
            let start = index * chunk_len;
            let end = (start + chunk_len).min(prefix_and_data.len());
            tcp_record(
                item_id,
                index + 1 == chunk_count,
                &prefix_and_data[start..end],
            )
        })
        .collect()
}

/// 按 `item_id` 并行组装 TCP clipboard 记录。记录 payload 的格式为
/// `item_id u32 | flags u8 | 数据块`。
#[derive(Default)]
pub struct TcpReassembler {
    items: HashMap<u32, StreamReceiver>,
}

impl TcpReassembler {
    pub fn new() -> Self {
        Self::default()
    }

    /// 接收一条完整的 TCP clipboard 记录；最后一块通过校验时返回该项。
    pub fn feed(&mut self, record: &[u8]) -> Result<Option<ClipboardItem>, ClipboardError> {
        if record.len() < TCP_RECORD_HEADER_BYTES {
            return Err(ClipboardError::TcpRecordTooShort {
                actual: record.len(),
                minimum: TCP_RECORD_HEADER_BYTES,
            });
        }
        let item_id = u32::from_be_bytes([record[0], record[1], record[2], record[3]]);
        let flags = record[4];
        if flags & !TCP_FLAG_MASK != 0 {
            return Err(ClipboardError::TcpInvalidFlags { flags });
        }
        let last = flags & TCP_FLAG_LAST != 0;
        let chunk = &record[TCP_RECORD_HEADER_BYTES..];

        let mut receiver = match self.items.remove(&item_id) {
            Some(receiver) => receiver,
            None => {
                if self.items.len() >= MAX_TCP_ITEMS {
                    return Err(ClipboardError::TooManyTcpItems);
                }
                StreamReceiver::new()
            }
        };
        receiver.feed(chunk)?;
        if last {
            receiver.finish().map(Some)
        } else {
            self.items.insert(item_id, receiver);
            Ok(None)
        }
    }
}
