//! Clipboard conversions and bounded ICCCM INCR transfers, independent of X11.

use host_core::clip_convert::make_item;
use host_core::platform::ClipboardItem;
use rsp_core::pb::clipboard_header::Kind;

pub use host_core::clip_convert::{MAX_PNG_BYTES, MAX_TEXT_BYTES};

const PNG_SIGNATURE: &[u8] = b"\x89PNG\r\n\x1a\n";

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum ClipTarget {
    Utf8String,
    TextPlainUtf8,
    String,
    Png,
}

impl ClipTarget {
    pub fn atom_name(self) -> &'static str {
        match self {
            Self::Utf8String => "UTF8_STRING",
            Self::TextPlainUtf8 => "text/plain;charset=utf-8",
            Self::String => "STRING",
            Self::Png => "image/png",
        }
    }
}

pub fn pick_target(available: &[&str]) -> Option<ClipTarget> {
    [
        ClipTarget::Utf8String,
        ClipTarget::TextPlainUtf8,
        ClipTarget::String,
        ClipTarget::Png,
    ]
    .into_iter()
    .find(|target| available.contains(&target.atom_name()))
}

pub fn item_from(target: ClipTarget, data: Vec<u8>) -> Option<ClipboardItem> {
    if target == ClipTarget::Png {
        return (data.len() <= MAX_PNG_BYTES && data.starts_with(PNG_SIGNATURE))
            .then(|| make_item(Kind::Png, data));
    }
    let text = if target == ClipTarget::String {
        data.into_iter().map(char::from).collect::<String>()
    } else {
        String::from_utf8(data).ok()?
    };
    let text = text
        .trim_end_matches('\0')
        .replace("\r\n", "\n")
        .replace('\r', "\n");
    (text.len() <= MAX_TEXT_BYTES).then(|| make_item(Kind::Text, text.into_bytes()))
}

pub fn offered_targets(item: &ClipboardItem) -> Vec<ClipTarget> {
    match item.kind {
        Kind::Text => vec![
            ClipTarget::Utf8String,
            ClipTarget::TextPlainUtf8,
            ClipTarget::String,
        ],
        Kind::Png => vec![ClipTarget::Png],
        Kind::Unspecified => Vec::new(),
    }
}

pub fn data_for(item: &ClipboardItem, target: ClipTarget) -> Option<Vec<u8>> {
    match (item.kind, target) {
        (Kind::Text, ClipTarget::String) => Some(
            std::str::from_utf8(&item.data)
                .ok()?
                .chars()
                .map(|ch| u8::try_from(u32::from(ch)).unwrap_or(b'?'))
                .collect(),
        ),
        (Kind::Text, ClipTarget::Utf8String | ClipTarget::TextPlainUtf8)
        | (Kind::Png, ClipTarget::Png) => Some(item.data.clone()),
        _ => None,
    }
}

pub fn property_chunk(max_request_len_words: u32) -> usize {
    (u64::from(max_request_len_words) * 4)
        .saturating_sub(64)
        .min(256 * 1024) as usize
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum IncrState {
    More,
    Done(Vec<u8>),
    TooLarge,
}

pub struct IncrReader {
    buf: Vec<u8>,
    limit: usize,
    too_large: bool,
}

impl IncrReader {
    pub fn new(limit: usize) -> Self {
        Self {
            buf: Vec::new(),
            limit,
            too_large: false,
        }
    }

    pub fn push(&mut self, chunk: &[u8]) -> IncrState {
        if self.too_large || chunk.len() > self.limit.saturating_sub(self.buf.len()) {
            self.too_large = true;
            self.buf.clear();
            return IncrState::TooLarge;
        }
        if chunk.is_empty() {
            return IncrState::Done(std::mem::take(&mut self.buf));
        }
        self.buf.extend_from_slice(chunk);
        IncrState::More
    }
}

pub struct IncrWriter {
    data: Vec<u8>,
    chunk: usize,
    pos: usize,
    finished: bool,
}

impl IncrWriter {
    pub fn new(data: Vec<u8>, chunk: usize) -> Self {
        Self {
            data,
            chunk: chunk.max(1),
            pos: 0,
            finished: false,
        }
    }

    pub fn next_chunk(&mut self) -> Option<&[u8]> {
        if self.finished {
            return None;
        }
        let start = self.pos;
        if start == self.data.len() {
            self.finished = true;
        } else {
            self.pos = self.data.len().min(start.saturating_add(self.chunk));
        }
        Some(&self.data[start..self.pos])
    }

    pub fn total_len(&self) -> usize {
        self.data.len()
    }
}
