//! Unicode text chunks for CGEventKeyboardSetUnicodeString.

use unicode_segmentation::UnicodeSegmentation;

/// Split text into UTF-16 chunks, normally at most `max_units` code units.
///
/// Pass 20 for macOS text injection. Extended grapheme clusters (including
/// combining marks and emoji ZWJ sequences) are never split, which also keeps
/// surrogate pairs intact. A cluster larger than the limit occupies a chunk
/// by itself and is allowed to exceed the limit. Zero is treated as a limit
/// of one. Empty input produces no chunks.
pub fn utf16_chunks(s: &str, max_units: usize) -> Vec<Vec<u16>> {
    let limit = max_units.max(1);
    let mut chunks = Vec::new();
    let mut chunk = Vec::new();
    for grapheme in s.graphemes(true) {
        let units = grapheme.encode_utf16().count();
        if !chunk.is_empty() && units > limit.saturating_sub(chunk.len()) {
            chunks.push(std::mem::take(&mut chunk));
        }
        chunk.extend(grapheme.encode_utf16());
    }
    if !chunk.is_empty() {
        chunks.push(chunk);
    }
    chunks
}
