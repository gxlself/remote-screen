//! Encrypted `.rsrec` container format and crash recovery.

use std::fs::{self, File, OpenOptions};
use std::io::{self, Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};

use ring::digest::{digest, SHA256};

use crate::crypto::{open_chunk, seal_chunk, AuthFailed, FileKey, SecretWrapper, TAG_LEN};

pub const MAGIC: [u8; 8] = *b"RSREC\r\n\x1a";
pub const VERSION: u8 = 1;
pub const HEADER_FIXED_LEN: usize = 40;
pub const CHUNK_HEADER_LEN: usize = 21;
pub const TRAILER_LEN: usize = 16;
pub const TRAILER_MAGIC: [u8; 8] = *b"RSRECEND";
pub const FLAG_FINAL: u8 = 1;
pub const FLAG_KEYFRAME_START: u8 = 2;
pub const MAX_CHUNK_CT: u32 = (16 << 20) + 16;
pub const PART_SUFFIX: &str = ".part";

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FileHeader {
    pub file_id: [u8; 16],
    pub wrap_kind: u8,
    pub wrapped_key: Vec<u8>,
    pub created_at_us: u64,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ChunkInfo {
    pub index: u64,
    pub offset: u64,
    pub ct_len: u32,
    pub flags: u8,
    pub t_start_us: u64,
}

impl ChunkInfo {
    pub fn is_final(&self) -> bool {
        self.flags & FLAG_FINAL != 0
    }

    pub fn keyframe_start(&self) -> bool {
        self.flags & FLAG_KEYFRAME_START != 0
    }
}

#[derive(Debug, thiserror::Error)]
pub enum FileError {
    #[error("io: {0}")]
    Io(#[from] std::io::Error),
    #[error("not a recording file")]
    BadMagic,
    #[error("unsupported recording version {0}")]
    UnsupportedVersion(u8),
    #[error("corrupt recording: {0}")]
    Corrupt(&'static str),
    #[error("{0}")]
    Wrap(crate::crypto::WrapError),
    #[error("chunk {chunk} failed authentication")]
    Auth { chunk: u64 },
    #[error("chunk too large")]
    TooLarge,
    #[error("random source failed")]
    Random,
}

#[derive(Debug)]
struct HeaderBytes {
    header: FileHeader,
    bytes: Vec<u8>,
}

fn parent_dir(path: &Path) -> &Path {
    match path.parent() {
        Some(parent) if !parent.as_os_str().is_empty() => parent,
        _ => Path::new("."),
    }
}

fn sync_parent(path: &Path) -> Result<(), FileError> {
    File::open(parent_dir(path))?.sync_all()?;
    Ok(())
}

fn wrap_error(error: crate::crypto::WrapError) -> FileError {
    FileError::Wrap(error)
}

fn invalid_header_eof(error: io::Error) -> FileError {
    if error.kind() == io::ErrorKind::UnexpectedEof {
        FileError::Corrupt("truncated file header")
    } else {
        FileError::Io(error)
    }
}

fn parse_header_bytes(bytes: Vec<u8>) -> Result<HeaderBytes, FileError> {
    if bytes.len() < 32 {
        return Err(FileError::Corrupt("truncated file header"));
    }
    if bytes[..8] != MAGIC {
        return Err(FileError::BadMagic);
    }
    if bytes[8] != VERSION {
        return Err(FileError::UnsupportedVersion(bytes[8]));
    }
    if bytes[9] != 0 {
        return Err(FileError::Corrupt("invalid header flags"));
    }
    let header_len = u16::from_be_bytes([bytes[10], bytes[11]]) as usize;
    let wrapped_len = u16::from_be_bytes([bytes[30], bytes[31]]) as usize;
    if !(1..=255).contains(&wrapped_len) {
        return Err(FileError::Corrupt("invalid wrapped key length"));
    }
    let expected_len = HEADER_FIXED_LEN
        .checked_add(wrapped_len)
        .ok_or(FileError::Corrupt("header length overflow"))?;
    if header_len != expected_len || bytes.len() != expected_len {
        return Err(FileError::Corrupt("invalid header length"));
    }
    let wrapped_start = 32;
    let wrapped_end = wrapped_start + wrapped_len;
    let created_at_start = wrapped_end;
    let mut file_id = [0u8; 16];
    file_id.copy_from_slice(&bytes[12..28]);
    if bytes[29] != 0 {
        return Err(FileError::Corrupt("invalid header reserved byte"));
    }
    let mut wrapped_key = Vec::with_capacity(wrapped_len);
    wrapped_key.extend_from_slice(&bytes[wrapped_start..wrapped_end]);
    let created_at_us = u64::from_be_bytes(
        bytes[created_at_start..created_at_start + 8]
            .try_into()
            .map_err(|_| FileError::Corrupt("invalid creation time"))?,
    );
    Ok(HeaderBytes {
        header: FileHeader {
            file_id,
            wrap_kind: bytes[28],
            wrapped_key,
            created_at_us,
        },
        bytes,
    })
}

fn read_header_bytes(file: &mut File) -> Result<HeaderBytes, FileError> {
    file.seek(SeekFrom::Start(0))?;
    let mut prefix = [0u8; 32];
    file.read_exact(&mut prefix).map_err(invalid_header_eof)?;
    if prefix[..8] != MAGIC {
        return Err(FileError::BadMagic);
    }
    if prefix[8] != VERSION {
        return Err(FileError::UnsupportedVersion(prefix[8]));
    }
    let wrapped_len = u16::from_be_bytes([prefix[30], prefix[31]]) as usize;
    if !(1..=255).contains(&wrapped_len) {
        return Err(FileError::Corrupt("invalid wrapped key length"));
    }
    let total_len = HEADER_FIXED_LEN
        .checked_add(wrapped_len)
        .ok_or(FileError::Corrupt("header length overflow"))?;
    let mut bytes = vec![0u8; total_len];
    bytes[..32].copy_from_slice(&prefix);
    file.read_exact(&mut bytes[32..])
        .map_err(invalid_header_eof)?;
    parse_header_bytes(bytes)
}

pub fn read_header(path: &Path) -> Result<FileHeader, FileError> {
    let mut file = File::open(path)?;
    Ok(read_header_bytes(&mut file)?.header)
}

fn encode_header(
    file_id: [u8; 16],
    wrap_kind: u8,
    wrapped_key: &[u8],
    created_at_us: u64,
) -> Result<Vec<u8>, FileError> {
    if !(1..=255).contains(&wrapped_key.len()) {
        return Err(FileError::Wrap(crate::crypto::WrapError::Rejected));
    }
    let header_len = HEADER_FIXED_LEN
        .checked_add(wrapped_key.len())
        .ok_or(FileError::Corrupt("header length overflow"))?;
    let header_len =
        u16::try_from(header_len).map_err(|_| FileError::Corrupt("header length overflow"))?;
    let wrapped_len = u16::try_from(wrapped_key.len())
        .map_err(|_| FileError::Corrupt("wrapped key length overflow"))?;
    let mut bytes = Vec::with_capacity(header_len as usize);
    bytes.extend_from_slice(&MAGIC);
    bytes.push(VERSION);
    bytes.push(0);
    bytes.extend_from_slice(&header_len.to_be_bytes());
    bytes.extend_from_slice(&file_id);
    bytes.push(wrap_kind);
    bytes.push(0);
    bytes.extend_from_slice(&wrapped_len.to_be_bytes());
    bytes.extend_from_slice(wrapped_key);
    bytes.extend_from_slice(&created_at_us.to_be_bytes());
    Ok(bytes)
}

fn chunk_header_bytes(info: &ChunkInfo) -> [u8; CHUNK_HEADER_LEN] {
    let mut bytes = [0u8; CHUNK_HEADER_LEN];
    bytes[..4].copy_from_slice(&info.ct_len.to_be_bytes());
    bytes[4..12].copy_from_slice(&info.index.to_be_bytes());
    bytes[12] = info.flags;
    bytes[13..21].copy_from_slice(&info.t_start_us.to_be_bytes());
    bytes
}

fn parse_chunk_header(offset: u64, bytes: &[u8; CHUNK_HEADER_LEN]) -> ChunkInfo {
    let mut ct_len = [0u8; 4];
    let mut index = [0u8; 8];
    let mut t_start_us = [0u8; 8];
    ct_len.copy_from_slice(&bytes[..4]);
    index.copy_from_slice(&bytes[4..12]);
    t_start_us.copy_from_slice(&bytes[13..21]);
    ChunkInfo {
        ct_len: u32::from_be_bytes(ct_len),
        index: u64::from_be_bytes(index),
        offset,
        flags: bytes[12],
        t_start_us: u64::from_be_bytes(t_start_us),
    }
}

fn checked_chunk_end(offset: u64, ct_len: u32) -> Result<u64, FileError> {
    offset
        .checked_add(CHUNK_HEADER_LEN as u64)
        .and_then(|value| value.checked_add(ct_len as u64))
        .ok_or(FileError::Corrupt("chunk offset overflow"))
}

fn chunk_aad(header_bytes: &[u8], chunk_header: &[u8; CHUNK_HEADER_LEN]) -> Vec<u8> {
    let header_digest = digest(&SHA256, header_bytes);
    let mut aad = Vec::with_capacity(header_digest.as_ref().len() + CHUNK_HEADER_LEN);
    aad.extend_from_slice(header_digest.as_ref());
    aad.extend_from_slice(chunk_header);
    aad
}

fn ciphertext_len(plaintext: &[u8]) -> Result<u32, FileError> {
    if plaintext.len() > MAX_CHUNK_CT as usize - TAG_LEN {
        return Err(FileError::TooLarge);
    }
    Ok((plaintext.len() + TAG_LEN) as u32)
}

fn check_ct_len(ct_len: u32) -> Result<(), FileError> {
    if ct_len > MAX_CHUNK_CT {
        return Err(FileError::TooLarge);
    }
    if ct_len < TAG_LEN as u32 {
        return Err(FileError::Corrupt("ciphertext is shorter than the GCM tag"));
    }
    Ok(())
}

fn write_chunk(
    file: &mut File,
    key: &FileKey,
    header_bytes: &[u8],
    info: &ChunkInfo,
    plaintext: &[u8],
) -> Result<u64, FileError> {
    let ct_len = ciphertext_len(plaintext)?;
    debug_assert_eq!(info.ct_len, ct_len);
    let end = checked_chunk_end(info.offset, ct_len)?;
    let header = chunk_header_bytes(info);
    let aad = chunk_aad(header_bytes, &header);
    let sealed = seal_chunk(key, info.index, &aad, plaintext);
    debug_assert_eq!(sealed.len(), ct_len as usize);
    file.seek(SeekFrom::Start(info.offset))?;
    file.write_all(&header)?;
    file.write_all(&sealed)?;
    Ok(end)
}

fn write_trailer(file: &mut File, final_offset: u64) -> Result<(), FileError> {
    let mut trailer = [0u8; TRAILER_LEN];
    trailer[..8].copy_from_slice(&final_offset.to_be_bytes());
    trailer[8..].copy_from_slice(&TRAILER_MAGIC);
    file.write_all(&trailer)?;
    Ok(())
}

fn read_block_header(file: &mut File, offset: u64) -> Result<[u8; CHUNK_HEADER_LEN], FileError> {
    file.seek(SeekFrom::Start(offset))?;
    let mut bytes = [0u8; CHUNK_HEADER_LEN];
    file.read_exact(&mut bytes)?;
    Ok(bytes)
}

fn read_sealed(file: &mut File, info: &ChunkInfo) -> Result<Vec<u8>, FileError> {
    check_ct_len(info.ct_len)?;
    file.seek(SeekFrom::Start(info.offset + CHUNK_HEADER_LEN as u64))?;
    let mut sealed = vec![0u8; info.ct_len as usize];
    file.read_exact(&mut sealed)?;
    Ok(sealed)
}

pub struct RecordingWriter {
    path: PathBuf,
    file: File,
    key: FileKey,
    header_bytes: Vec<u8>,
    next_index: u64,
    bytes_written: u64,
}

impl RecordingWriter {
    pub fn create(
        path: &Path,
        wrapper: &dyn SecretWrapper,
        file_id: [u8; 16],
        created_at_us: u64,
    ) -> Result<Self, FileError> {
        let mut file = OpenOptions::new()
            .write(true)
            .read(true)
            .create_new(true)
            .open(path)?;
        let mut initialize = || -> Result<_, FileError> {
            let key = FileKey::random().map_err(|_| FileError::Random)?;
            let wrapped = wrapper.wrap(key.as_bytes()).map_err(wrap_error)?;
            let header_bytes = encode_header(file_id, wrapper.kind(), &wrapped, created_at_us)?;
            file.write_all(&header_bytes)?;
            file.sync_all()?;
            sync_parent(path)?;
            Ok((key, header_bytes))
        };
        let (key, header_bytes) = match initialize() {
            Ok(initialized) => initialized,
            Err(error) => {
                drop(file);
                let _ = fs::remove_file(path);
                return Err(error);
            }
        };
        Ok(Self {
            path: path.to_path_buf(),
            file,
            key,
            next_index: 0,
            bytes_written: header_bytes.len() as u64,
            header_bytes,
        })
    }

    fn append_flags(
        &mut self,
        t_start_us: u64,
        flags: u8,
        plaintext: &[u8],
    ) -> Result<u64, FileError> {
        let offset = self.bytes_written;
        let next_index = self
            .next_index
            .checked_add(1)
            .ok_or(FileError::Corrupt("chunk index overflow"))?;
        let info = ChunkInfo {
            index: self.next_index,
            offset,
            ct_len: ciphertext_len(plaintext)?,
            flags,
            t_start_us,
        };
        let end = write_chunk(
            &mut self.file,
            &self.key,
            &self.header_bytes,
            &info,
            plaintext,
        )?;
        self.next_index = next_index;
        self.bytes_written = end;
        Ok(offset)
    }

    pub fn append(
        &mut self,
        t_start_us: u64,
        keyframe_start: bool,
        plaintext: &[u8],
    ) -> Result<u64, FileError> {
        let flags = if keyframe_start {
            FLAG_KEYFRAME_START
        } else {
            0
        };
        self.append_flags(t_start_us, flags, plaintext)
    }

    pub fn sync(&mut self) -> Result<(), FileError> {
        self.file.sync_data()?;
        Ok(())
    }

    pub fn finish(mut self, t_start_us: u64, final_plaintext: &[u8]) -> Result<u64, FileError> {
        let final_offset = self.append_flags(t_start_us, FLAG_FINAL, final_plaintext)?;
        write_trailer(&mut self.file, final_offset)?;
        self.bytes_written = self
            .bytes_written
            .checked_add(TRAILER_LEN as u64)
            .ok_or(FileError::Corrupt("file length overflow"))?;
        self.file.sync_all()?;
        Ok(self.bytes_written)
    }

    pub fn bytes_written(&self) -> u64 {
        self.bytes_written
    }

    pub fn chunks_written(&self) -> u64 {
        self.next_index
    }

    pub fn path(&self) -> &Path {
        &self.path
    }
}

pub fn finalize_path(part: &Path) -> Result<PathBuf, FileError> {
    let destination = finalized_path(part)?;
    fs::rename(part, &destination)?;
    sync_parent(&destination)?;
    Ok(destination)
}

fn finalized_path(part: &Path) -> Result<PathBuf, FileError> {
    if part.extension() != Some(std::ffi::OsStr::new("part")) {
        return Err(FileError::Corrupt("path is not a .part file"));
    }
    Ok(part.with_extension(""))
}

pub struct RecordingReader {
    file: File,
    key: FileKey,
    header: FileHeader,
    header_bytes: Vec<u8>,
    chunks: Vec<ChunkInfo>,
    final_chunk: ChunkInfo,
}

impl RecordingReader {
    pub fn open(path: &Path, wrapper: &dyn SecretWrapper) -> Result<Self, FileError> {
        let mut file = File::open(path)?;
        let header_bytes = read_header_bytes(&mut file)?;
        if header_bytes.header.wrap_kind != wrapper.kind() {
            return Err(FileError::Wrap(crate::crypto::WrapError::Rejected));
        }
        let key = wrapper
            .unwrap(&header_bytes.header.wrapped_key)
            .map_err(wrap_error)
            .map(FileKey::from_bytes)?;
        let file_len = file.metadata()?.len();
        let header_len = header_bytes.bytes.len() as u64;
        if file_len < header_len + TRAILER_LEN as u64 {
            return Err(FileError::Corrupt("missing file trailer"));
        }
        let trailer_offset = file_len - TRAILER_LEN as u64;
        file.seek(SeekFrom::Start(trailer_offset))?;
        let mut trailer = [0u8; TRAILER_LEN];
        file.read_exact(&mut trailer)?;
        if trailer[8..] != TRAILER_MAGIC {
            return Err(FileError::Corrupt("invalid file trailer"));
        }
        let mut final_offset = [0u8; 8];
        final_offset.copy_from_slice(&trailer[..8]);
        let final_offset = u64::from_be_bytes(final_offset);
        if final_offset < header_len || final_offset >= trailer_offset {
            return Err(FileError::Corrupt("invalid final chunk offset"));
        }

        let mut chunks = Vec::new();
        let mut offset = header_len;
        let mut expected_index = 0u64;
        let mut final_chunk = None;
        while offset < trailer_offset {
            let remaining = trailer_offset - offset;
            if remaining < CHUNK_HEADER_LEN as u64 {
                return Err(FileError::Corrupt("truncated chunk header"));
            }
            let raw = read_block_header(&mut file, offset)?;
            let info = parse_chunk_header(offset, &raw);
            check_ct_len(info.ct_len)?;
            let end = checked_chunk_end(offset, info.ct_len)?;
            if end > trailer_offset {
                return Err(FileError::Corrupt("chunk crosses file trailer"));
            }
            if info.index != expected_index {
                return Err(FileError::Corrupt("chunk indexes are not contiguous"));
            }
            if info.is_final() {
                if end != trailer_offset {
                    return Err(FileError::Corrupt("final chunk is not last"));
                }
                final_chunk = Some(info);
            } else {
                chunks.push(info);
            }
            expected_index = expected_index
                .checked_add(1)
                .ok_or(FileError::Corrupt("chunk index overflow"))?;
            offset = end;
            if final_chunk.is_some() {
                break;
            }
        }
        if offset != trailer_offset {
            return Err(FileError::Corrupt("unexpected data after final chunk"));
        }
        let final_chunk = final_chunk.ok_or(FileError::Corrupt("missing final chunk"))?;
        if final_chunk.offset != final_offset {
            return Err(FileError::Corrupt("final chunk offset mismatch"));
        }
        Ok(Self {
            file,
            key,
            header: header_bytes.header,
            header_bytes: header_bytes.bytes,
            chunks,
            final_chunk,
        })
    }

    pub fn header(&self) -> &FileHeader {
        &self.header
    }

    pub fn chunks(&self) -> &[ChunkInfo] {
        &self.chunks
    }

    pub fn final_chunk(&self) -> ChunkInfo {
        self.final_chunk
    }

    fn read_info(&mut self, info: ChunkInfo) -> Result<Vec<u8>, FileError> {
        let raw = read_block_header(&mut self.file, info.offset)?;
        let aad = chunk_aad(&self.header_bytes, &raw);
        let sealed = read_sealed(&mut self.file, &info)?;
        open_chunk(&self.key, info.index, &aad, &sealed)
            .map_err(|AuthFailed| FileError::Auth { chunk: info.index })
    }

    pub fn read_chunk(&mut self, i: usize) -> Result<Vec<u8>, FileError> {
        let info = self
            .chunks
            .get(i)
            .copied()
            .ok_or(FileError::Corrupt("chunk index out of range"))?;
        self.read_info(info)
    }

    pub fn read_final(&mut self) -> Result<Vec<u8>, FileError> {
        self.read_info(self.final_chunk)
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RecoverKind {
    Complete,
    Recovered,
    Damaged,
}

#[derive(Debug)]
pub struct RecoverOutcome {
    pub kind: RecoverKind,
    pub valid_chunks: u64,
    pub path: PathBuf,
}

struct Damage {
    offset: u64,
    expected_index: u64,
    header_complete: bool,
    ct_len: u32,
}

fn candidate_is_valid(
    file: &mut File,
    header_bytes: &[u8],
    key: &FileKey,
    damage: &Damage,
    file_len: u64,
) -> bool {
    if !damage.header_complete || check_ct_len(damage.ct_len).is_err() {
        return false;
    }
    let candidate_offset = match checked_chunk_end(damage.offset, damage.ct_len) {
        Ok(offset) => offset,
        Err(_) => return false,
    };
    let candidate_header_end = match candidate_offset.checked_add(CHUNK_HEADER_LEN as u64) {
        Some(end) => end,
        None => return false,
    };
    if candidate_header_end > file_len {
        return false;
    }
    let raw = match read_block_header(file, candidate_offset) {
        Ok(raw) => raw,
        Err(_) => return false,
    };
    let info = parse_chunk_header(candidate_offset, &raw);
    if Some(info.index) != damage.expected_index.checked_add(1)
        || check_ct_len(info.ct_len).is_err()
    {
        return false;
    }
    let end = match checked_chunk_end(candidate_offset, info.ct_len) {
        Ok(end) => end,
        Err(_) => return false,
    };
    if end > file_len {
        return false;
    }
    let mut sealed = vec![0u8; info.ct_len as usize];
    if file
        .seek(SeekFrom::Start(candidate_offset + CHUNK_HEADER_LEN as u64))
        .and_then(|_| file.read_exact(&mut sealed))
        .is_err()
    {
        return false;
    }
    let aad = chunk_aad(header_bytes, &raw);
    open_chunk(key, info.index, &aad, &sealed).is_ok()
}

fn finalize_recovery(
    mut file: File,
    path: &Path,
    key: &FileKey,
    header_bytes: &[u8],
    damage: Damage,
    kind: RecoverKind,
    build_end: &mut dyn FnMut(RecoverKind) -> (u64, Vec<u8>),
) -> Result<PathBuf, FileError> {
    let (t_start_us, plaintext) = build_end(kind);
    let info = ChunkInfo {
        offset: damage.offset,
        index: damage.expected_index,
        flags: FLAG_FINAL,
        t_start_us,
        ct_len: ciphertext_len(&plaintext)?,
    };
    file.set_len(damage.offset)?;
    let end = write_chunk(&mut file, key, header_bytes, &info, &plaintext)?;
    file.seek(SeekFrom::Start(end))?;
    write_trailer(&mut file, damage.offset)?;
    file.sync_all()?;
    drop(file);
    finalize_path(path)
}

pub fn recover_part(
    part: &Path,
    wrapper: &dyn SecretWrapper,
    on_chunk: &mut dyn FnMut(&ChunkInfo, &[u8]),
    build_end: &mut dyn FnMut(RecoverKind) -> (u64, Vec<u8>),
) -> Result<RecoverOutcome, FileError> {
    finalized_path(part)?;
    let mut file = OpenOptions::new().read(true).write(true).open(part)?;
    let header_bytes = read_header_bytes(&mut file)?;
    if header_bytes.header.wrap_kind != wrapper.kind() {
        return Err(FileError::Wrap(crate::crypto::WrapError::Rejected));
    }
    let key = wrapper
        .unwrap(&header_bytes.header.wrapped_key)
        .map_err(wrap_error)
        .map(FileKey::from_bytes)?;
    let file_len = file.metadata()?.len();
    let header_len = header_bytes.bytes.len() as u64;
    let mut offset = header_len;
    let mut expected_index = 0u64;
    let mut valid_chunks = 0u64;
    let damage = loop {
        if offset >= file_len {
            break Damage {
                offset,
                expected_index,
                header_complete: false,
                ct_len: 0,
            };
        }
        if file_len - offset < CHUNK_HEADER_LEN as u64 {
            break Damage {
                offset,
                expected_index,
                header_complete: false,
                ct_len: 0,
            };
        }
        let raw = read_block_header(&mut file, offset)?;
        let info = parse_chunk_header(offset, &raw);
        if check_ct_len(info.ct_len).is_err() {
            break Damage {
                offset,
                expected_index,
                header_complete: true,
                ct_len: info.ct_len,
            };
        }
        let end = checked_chunk_end(offset, info.ct_len)?;
        if end > file_len {
            break Damage {
                offset,
                expected_index,
                header_complete: true,
                ct_len: info.ct_len,
            };
        }
        let mut sealed = vec![0u8; info.ct_len as usize];
        file.seek(SeekFrom::Start(offset + CHUNK_HEADER_LEN as u64))?;
        file.read_exact(&mut sealed)?;
        let aad = chunk_aad(&header_bytes.bytes, &raw);
        let plaintext = match open_chunk(&key, info.index, &aad, &sealed) {
            Ok(plaintext) => plaintext,
            Err(_) => {
                break Damage {
                    offset,
                    expected_index,
                    header_complete: true,
                    ct_len: info.ct_len,
                };
            }
        };
        if info.index != expected_index {
            break Damage {
                offset,
                expected_index,
                header_complete: true,
                ct_len: info.ct_len,
            };
        }
        if info.is_final() {
            file.set_len(end + TRAILER_LEN as u64)?;
            file.seek(SeekFrom::Start(end))?;
            write_trailer(&mut file, info.offset)?;
            file.sync_all()?;
            drop(file);
            let path = finalize_path(part)?;
            return Ok(RecoverOutcome {
                kind: RecoverKind::Complete,
                valid_chunks,
                path,
            });
        }
        on_chunk(&info, &plaintext);
        valid_chunks = valid_chunks
            .checked_add(1)
            .ok_or(FileError::Corrupt("chunk count overflow"))?;
        expected_index = expected_index
            .checked_add(1)
            .ok_or(FileError::Corrupt("chunk index overflow"))?;
        offset = end;
    };

    let kind = if candidate_is_valid(&mut file, &header_bytes.bytes, &key, &damage, file_len) {
        RecoverKind::Damaged
    } else {
        RecoverKind::Recovered
    };
    let path = finalize_recovery(
        file,
        part,
        &key,
        &header_bytes.bytes,
        damage,
        kind,
        build_end,
    )?;
    Ok(RecoverOutcome {
        kind,
        valid_chunks,
        path,
    })
}
