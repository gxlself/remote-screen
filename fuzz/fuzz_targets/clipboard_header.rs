#![no_main]

use libfuzzer_sys::fuzz_target;
use prost::Message;
use rsp_core::clipboard::{ClipboardError, StreamReceiver, MAX_HEADER_BYTES, MAX_TEXT_BYTES};
use rsp_core::pb::{clipboard_header::Kind, ClipboardHeader};

fuzz_target!(|input: &[u8]| {
    let mut oversized_prefix = StreamReceiver::new();
    let length = (MAX_HEADER_BYTES as u32) + 1;
    assert!(matches!(
        oversized_prefix.feed(&length.to_be_bytes()),
        Err(ClipboardError::HeaderLengthTooLarge(actual)) if actual == length as usize
    ));

    let header = ClipboardHeader {
        kind: Kind::Text as i32,
        size: (MAX_TEXT_BYTES as u64) + 1,
        sha256: vec![0; 32],
    };
    let encoded = header.encode_to_vec();
    let mut oversized_data = (encoded.len() as u32).to_be_bytes().to_vec();
    oversized_data.extend_from_slice(&encoded);
    let mut oversized_receiver = StreamReceiver::new();
    assert!(matches!(
        oversized_receiver.feed(&oversized_data),
        Err(ClipboardError::DataTooLarge { .. })
    ));

    let chunk_size = usize::from(input.first().copied().unwrap_or(0)) % 32 + 1;
    let mut receiver = StreamReceiver::new();
    for chunk in input.chunks(chunk_size) {
        if receiver.feed(chunk).is_err() {
            break;
        }
    }
    let _ = receiver.finish();
});
