//! 文件通道 0x04：文件头编解码与接收状态机。
//!
//! QUIC 单向流的内容是 `u32 长度 + FileHeader`，随后是从头部 `offset` 开始的
//! 文件字节，FIN 表示发送结束。本模块不读写磁盘，也不计算整文件 SHA-256；它只
//! 负责校验头部、把任意切分的流数据解码成按文件绝对偏移定位的片段，并判断流是
//! 完整结束还是可以在 `offset` 处续传。见 docs/dev/03-protocol.md §8.2。

use std::fmt;

use prost::Message;
use thiserror::Error;

use crate::pb::FileHeader;

/// 文档未规定，本模块默认：QUIC 流头长度前缀允许声明的最大 protobuf 字节数。
pub const MAX_HEADER_BYTES: usize = 4096;
/// 文档未规定，本模块默认：文件名的最大 UTF-8 字节数。
pub const MAX_NAME_BYTES: usize = 1024;
/// 文档未规定，本模块默认：单个文件的最大字节数（1 TiB）。
pub const MAX_FILE_BYTES: u64 = 1 << 40;
/// 文档未规定，本模块默认：`transfer_id` 的字符数。
pub const TRANSFER_ID_LEN: usize = 32;

const LENGTH_PREFIX_BYTES: usize = 4;
const RANDOM_ID_BYTES: usize = 16;
const SHA256_BYTES: usize = 32;
const HEX_DIGITS: &[u8; 16] = b"0123456789abcdef";

#[derive(Debug, Error)]
pub enum FileError {
    #[error("file stream ended with no header")]
    EmptyStream,
    #[error("file header length prefix {0} exceeds {MAX_HEADER_BYTES} bytes")]
    HeaderLengthTooLarge(usize),
    #[error("file header truncated: declared {declared} bytes, received {received}")]
    HeaderTruncated { declared: usize, received: usize },
    #[error("file header is {0} bytes; the limit is {MAX_HEADER_BYTES} bytes")]
    HeaderTooLarge(usize),
    #[error("file header protobuf is invalid: {0}")]
    InvalidHeader(#[from] prost::DecodeError),
    #[error("file transfer id is invalid")]
    InvalidTransferId,
    #[error("file name is invalid: {0}")]
    InvalidName(&'static str),
    #[error("file destination directory is invalid: {0}")]
    InvalidDestDir(&'static str),
    #[error("file SHA-256 is {0} bytes, expected {SHA256_BYTES}")]
    InvalidSha256Length(usize),
    #[error("file is {size} bytes; the limit is {limit} bytes")]
    FileTooLarge { size: u64, limit: u64 },
    #[error("file offset {offset} exceeds size {size}")]
    OffsetBeyondSize { offset: u64, size: u64 },
    #[error("file data exceeds declared size {size}: received {received} bytes")]
    DataExceedsSize { size: u64, received: u64 },
    #[error("file stream received data after it was poisoned by an earlier error")]
    StreamFinished,
    #[error("file resume state does not match: received {have} bytes")]
    ResumeMismatch { have: u64 },
    #[error("failed to generate a file transfer id: {0}")]
    Random(getrandom::Error),
}

/// 生成随机 128 位 `transfer_id`。
///
/// 文档只说随机 128 位，未规定文本编码；本模块默认使用 32 位小写十六进制。
pub fn new_transfer_id() -> Result<String, FileError> {
    let mut random = [0u8; RANDOM_ID_BYTES];
    getrandom::fill(&mut random).map_err(FileError::Random)?;

    let mut id = String::with_capacity(TRANSFER_ID_LEN);
    for byte in random {
        id.push(HEX_DIGITS[(byte >> 4) as usize] as char);
        id.push(HEX_DIGITS[(byte & 0x0f) as usize] as char);
    }
    Ok(id)
}

/// 校验 `transfer_id` 恰好是 32 位小写十六进制字符。
pub fn validate_transfer_id(id: &str) -> Result<(), FileError> {
    if id.len() != TRANSFER_ID_LEN
        || !id
            .bytes()
            .all(|byte| byte.is_ascii_digit() || (b'a'..=b'f').contains(&byte))
    {
        return Err(FileError::InvalidTransferId);
    }
    Ok(())
}

/// 校验文件头。路径清洗不在这里进行：合法文件名可以包含 `\`、`/` 与 `..`，
/// 接收方在落盘前使用 [`crate::file_name`] 处理。
pub fn validate_header(header: &FileHeader) -> Result<(), FileError> {
    validate_transfer_id(&header.transfer_id)?;

    if header.name.is_empty() {
        return Err(FileError::InvalidName("name is empty"));
    }
    if header.name.len() > MAX_NAME_BYTES {
        return Err(FileError::InvalidName("name exceeds 1024 bytes"));
    }
    if header.name.contains('\0') {
        return Err(FileError::InvalidName("name contains NUL"));
    }
    if header.dest_dir.len() > 2048 {
        return Err(FileError::InvalidDestDir("dest_dir exceeds 2048 bytes"));
    }
    if header.dest_dir.contains('\0') {
        return Err(FileError::InvalidDestDir("dest_dir contains NUL"));
    }
    if header.size > MAX_FILE_BYTES {
        return Err(FileError::FileTooLarge {
            size: header.size,
            limit: MAX_FILE_BYTES,
        });
    }
    if header.offset > header.size {
        return Err(FileError::OffsetBeyondSize {
            offset: header.offset,
            size: header.size,
        });
    }
    if header.sha256.len() != SHA256_BYTES {
        return Err(FileError::InvalidSha256Length(header.sha256.len()));
    }
    Ok(())
}

/// 构造并校验文件头。
pub fn build_header(
    transfer_id: impl Into<String>,
    name: impl Into<String>,
    size: u64,
    offset: u64,
    sha256: [u8; SHA256_BYTES],
    mtime_us: u64,
) -> Result<FileHeader, FileError> {
    let header = FileHeader {
        transfer_id: transfer_id.into(),
        name: name.into(),
        size,
        offset,
        sha256: sha256.to_vec(),
        mtime_us,
        dest_dir: String::new(),
    };
    validate_header(&header)?;
    Ok(header)
}

/// 编码单向流前缀：`u32 大端长度 + protobuf FileHeader`。调用方随后从
/// `header.offset` 开始写文件字节并在末尾 FIN。
pub fn encode_stream_prefix(header: &FileHeader) -> Result<Vec<u8>, FileError> {
    validate_header(header)?;
    let encoded = header.encode_to_vec();
    if encoded.len() > MAX_HEADER_BYTES {
        return Err(FileError::HeaderTooLarge(encoded.len()));
    }

    let mut out = Vec::with_capacity(LENGTH_PREFIX_BYTES + encoded.len());
    out.extend_from_slice(&(encoded.len() as u32).to_be_bytes());
    out.extend_from_slice(&encoded);
    Ok(out)
}

/// 一段属于文件的数据。`offset` 是文件内的绝对位置；`bytes` 借用本次 `feed`
/// 的输入块，不会复制文件内容。
#[derive(Clone, Copy, PartialEq, Eq)]
pub struct DataSpan<'a> {
    pub offset: u64,
    pub bytes: &'a [u8],
}

impl fmt::Debug for DataSpan<'_> {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("DataSpan")
            .field("offset", &self.offset)
            .field("len", &self.bytes.len())
            .finish()
    }
}

/// 一次 `feed` 产生的结果。头部只会在它恰好到齐的那次调用里出现一次；数据
/// 每次调用最多返回一段。
#[derive(Debug)]
pub struct Fed<'a> {
    pub header: Option<FileHeader>,
    pub data: Option<DataSpan<'a>>,
}

/// 文件流结束时的状态。`Truncated` 不是协议错误：发送方中途取消或 FIN 提前
/// 到达时，接收方可以从 `position` 处请求续传。
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum StreamEnd {
    Complete { header: FileHeader },
    Truncated { header: FileHeader, position: u64 },
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
        header: FileHeader,
        position: u64,
    },
    Poisoned,
}

impl ReceiveState {
    fn feed<'a>(mut self, mut input: &'a [u8]) -> Result<(Self, Fed<'a>), FileError> {
        let mut fed_header = None;

        loop {
            match self {
                Self::Prefix {
                    mut buf,
                    mut filled,
                } => {
                    let take = (LENGTH_PREFIX_BYTES - filled).min(input.len());
                    buf[filled..filled + take].copy_from_slice(&input[..take]);
                    filled += take;
                    input = &input[take..];
                    if filled < LENGTH_PREFIX_BYTES {
                        return Ok((
                            Self::Prefix { buf, filled },
                            Fed {
                                header: fed_header,
                                data: None,
                            },
                        ));
                    }

                    let len = u32::from_be_bytes(buf) as usize;
                    if len > MAX_HEADER_BYTES {
                        return Err(FileError::HeaderLengthTooLarge(len));
                    }
                    self = Self::Header {
                        len,
                        buf: Vec::with_capacity(len),
                    };
                }
                Self::Header { len, mut buf } => {
                    let take = (len - buf.len()).min(input.len());
                    buf.extend_from_slice(&input[..take]);
                    input = &input[take..];
                    if buf.len() < len {
                        return Ok((
                            Self::Header { len, buf },
                            Fed {
                                header: fed_header,
                                data: None,
                            },
                        ));
                    }

                    let header = FileHeader::decode(buf.as_slice())?;
                    validate_header(&header)?;
                    fed_header = Some(header.clone());
                    self = Self::Data {
                        position: header.offset,
                        header,
                    };
                }
                Self::Data {
                    header,
                    mut position,
                } => {
                    if input.is_empty() {
                        return Ok((
                            Self::Data { header, position },
                            Fed {
                                header: fed_header,
                                data: None,
                            },
                        ));
                    }

                    let remaining = header.size.saturating_sub(position);
                    let input_len = input.len() as u64;
                    if input_len > remaining {
                        return Err(FileError::DataExceedsSize {
                            size: header.size,
                            received: position.saturating_add(input_len),
                        });
                    }

                    let data = DataSpan {
                        offset: position,
                        bytes: input,
                    };
                    position += input_len;
                    return Ok((
                        Self::Data { header, position },
                        Fed {
                            header: fed_header,
                            data: Some(data),
                        },
                    ));
                }
                Self::Poisoned => return Err(FileError::StreamFinished),
            }
        }
    }
}

/// 增量接收一条 QUIC 文件单向流。`feed` 接受任意切分的数据块。
pub struct FileStreamReceiver {
    state: ReceiveState,
}

impl Default for FileStreamReceiver {
    fn default() -> Self {
        Self::new()
    }
}

impl FileStreamReceiver {
    pub fn new() -> Self {
        Self {
            state: ReceiveState::Prefix {
                buf: [0; LENGTH_PREFIX_BYTES],
                filled: 0,
            },
        }
    }

    /// 接收一块流数据。空块不改变状态；出错后接收器会永久进入 Poisoned。
    pub fn feed<'a>(&mut self, chunk: &'a [u8]) -> Result<Fed<'a>, FileError> {
        if chunk.is_empty() {
            return Ok(Fed {
                header: None,
                data: None,
            });
        }

        let state = std::mem::replace(&mut self.state, ReceiveState::Poisoned);
        match state.feed(chunk) {
            Ok((state, fed)) => {
                self.state = state;
                Ok(fed)
            }
            Err(error) => Err(error),
        }
    }

    /// 头部到齐后的文件头。
    pub fn header(&self) -> Option<&FileHeader> {
        match &self.state {
            ReceiveState::Data { header, .. } => Some(header),
            ReceiveState::Prefix { .. } | ReceiveState::Header { .. } | ReceiveState::Poisoned => {
                None
            }
        }
    }

    /// 下一个字节的文件内绝对位置。头部到齐前为 0，到齐后从 `header.offset`
    /// 开始。
    pub fn position(&self) -> u64 {
        match &self.state {
            ReceiveState::Data { position, .. } => *position,
            ReceiveState::Prefix { .. } | ReceiveState::Header { .. } | ReceiveState::Poisoned => 0,
        }
    }

    /// 处理流结束（FIN）。数据只到文件一半时返回 `Truncated`，调用方可以据此
    /// 续传；它本身不是错误。
    pub fn finish(self) -> Result<StreamEnd, FileError> {
        match self.state {
            ReceiveState::Prefix { filled: 0, .. } => Err(FileError::EmptyStream),
            ReceiveState::Prefix { filled, .. } => Err(FileError::HeaderTruncated {
                declared: LENGTH_PREFIX_BYTES,
                received: filled,
            }),
            ReceiveState::Header { len, buf } => Err(FileError::HeaderTruncated {
                declared: len,
                received: buf.len(),
            }),
            ReceiveState::Data { header, position } => {
                if position == header.size {
                    Ok(StreamEnd::Complete { header })
                } else {
                    Ok(StreamEnd::Truncated { header, position })
                }
            }
            ReceiveState::Poisoned => Err(FileError::StreamFinished),
        }
    }
}

/// 调用方保存的已落盘半截文件状态，用于判断一个续传头能否接着现有内容写。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PartialState {
    pub transfer_id: String,
    pub size: u64,
    pub sha256: [u8; SHA256_BYTES],
    pub mtime_us: u64,
    pub received: u64,
}

/// 续传头对现有半截文件的处理方式。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ResumePlan {
    Fresh,
    Continue { truncate_to: u64 },
}

/// 检查文件头是否可以接到 `partial` 后面。
///
/// `offset == 0` 总是新传输，调用方应丢弃已有半截文件。续传必须匹配
/// transfer id、总大小、整文件 SHA-256 和 mtime；本地已有字节数不足头部声明的
/// offset 时不能续传，多出的字节则截断到 offset。
pub fn resume_check(
    header: &FileHeader,
    partial: Option<&PartialState>,
) -> Result<ResumePlan, FileError> {
    validate_header(header)?;

    if header.offset == 0 {
        return Ok(ResumePlan::Fresh);
    }

    let Some(partial) = partial else {
        return Err(FileError::ResumeMismatch { have: 0 });
    };
    if partial.transfer_id != header.transfer_id
        || partial.size != header.size
        || partial.sha256.as_slice() != header.sha256.as_slice()
        || partial.mtime_us != header.mtime_us
    {
        return Err(FileError::ResumeMismatch { have: 0 });
    }
    if partial.received < header.offset {
        return Err(FileError::ResumeMismatch {
            have: partial.received,
        });
    }
    Ok(ResumePlan::Continue {
        truncate_to: header.offset,
    })
}
