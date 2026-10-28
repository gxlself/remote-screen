//! NSPasteboard access for text and PNG clipboard items.

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::thread;
use std::time::Duration;

use host_core::platform::{ClipboardItem, PlatformError, Result};
use rsp_core::pb::clipboard_header::Kind;
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

const MAX_TEXT_BYTES: usize = 1 << 20;
const MAX_PNG_BYTES: usize = 10 << 20;
const MAX_CLIPBOARD_BYTES: usize = MAX_PNG_BYTES;

extern "C" {
    fn rs_clipboard_get(kind: *mut i32, data: *mut u8, capacity: usize, length: *mut usize) -> i32;
    fn rs_clipboard_set(kind: i32, data: *const u8, length: usize, change_count: *mut u64) -> i32;
    fn rs_clipboard_change_count() -> u64;
}

pub fn get() -> Option<ClipboardItem> {
    let mut kind = 0i32;
    let mut length = 0usize;
    // SAFETY: a null data pointer with zero capacity asks the shim for the
    // current item size; all output pointers are valid locals.
    let status = unsafe { rs_clipboard_get(&mut kind, std::ptr::null_mut(), 0, &mut length) };
    if status == 0 || status == -1 || length > MAX_CLIPBOARD_BYTES {
        return None;
    }
    let mut data = vec![0u8; length];
    // SAFETY: `data` has exactly the capacity requested by the shim.
    let status = unsafe { rs_clipboard_get(&mut kind, data.as_mut_ptr(), data.len(), &mut length) };
    if status <= 0 || length > data.len() {
        return None;
    }
    data.truncate(length);
    let kind = match Kind::try_from(kind).ok()? {
        Kind::Text if data.len() <= MAX_TEXT_BYTES => Kind::Text,
        Kind::Png if data.len() <= MAX_PNG_BYTES => Kind::Png,
        _ => return None,
    };
    let mut sha256 = [0u8; 32];
    sha256.copy_from_slice(Sha256::digest(&data).as_slice());
    Some(ClipboardItem { kind, data, sha256 })
}

pub fn set(item: &ClipboardItem) -> Result<u64> {
    let (kind, limit) = match item.kind {
        Kind::Text => (1, MAX_TEXT_BYTES),
        Kind::Png => (2, MAX_PNG_BYTES),
        Kind::Unspecified => return Err(PlatformError::Unsupported),
    };
    if item.data.len() > limit {
        return Err(PlatformError::Platform(format!(
            "clipboard item is {} bytes; limit is {limit}",
            item.data.len()
        )));
    }
    if item.kind == Kind::Text && std::str::from_utf8(&item.data).is_err() {
        return Err(PlatformError::Platform(
            "clipboard text is not valid UTF-8".into(),
        ));
    }
    let mut change_count = 0u64;
    // SAFETY: the shim copies this immutable byte slice before returning.
    let status =
        unsafe { rs_clipboard_set(kind, item.data.as_ptr(), item.data.len(), &mut change_count) };
    if status != 0 {
        return Err(PlatformError::Platform(format!(
            "NSPasteboard write failed ({status})"
        )));
    }
    Ok(change_count)
}

pub fn change_count() -> u64 {
    // SAFETY: the shim only reads NSPasteboard.generalPasteboard.changeCount.
    unsafe { rs_clipboard_change_count() }
}

pub fn watch(tx: mpsc::Sender<()>, own_change_count: Arc<AtomicU64>) {
    thread::spawn(move || {
        let mut previous = change_count();
        loop {
            if tx.is_closed() {
                return;
            }
            thread::sleep(Duration::from_millis(300));
            let current = change_count();
            if current == previous {
                continue;
            }
            previous = current;
            if current == own_change_count.load(Ordering::Acquire) {
                continue;
            }
            match tx.try_send(()) {
                Ok(()) | Err(mpsc::error::TrySendError::Full(_)) => {}
                Err(mpsc::error::TrySendError::Closed(_)) => return,
            }
        }
    });
}
