#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::wire::{DecodeError, FrameDecoder, MAX_FRAME_LEN};

fuzz_target!(|input: &[u8]| {
    let chunk_size = usize::from(input.first().copied().unwrap_or(0)) % 64 + 1;
    let mut decoder = FrameDecoder::new();
    for chunk in input.chunks(chunk_size) {
        decoder.push(chunk);
        loop {
            match decoder.next_frame() {
                Ok(Some(frame)) => assert!(frame.len() <= MAX_FRAME_LEN),
                Ok(None) => break,
                Err(_) => return,
            }
        }
    }

    let mut oversized = FrameDecoder::new();
    oversized.push(&((MAX_FRAME_LEN as u32) + 1).to_be_bytes());
    assert!(matches!(
        oversized.next_frame(),
        Err(DecodeError::FrameTooLarge(length)) if length == MAX_FRAME_LEN + 1
    ));
});
