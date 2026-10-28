//! Minimized inputs for crashes discovered by the `envelope` and
//! `trust_statement` fuzz targets. Each case must return an error (or simply
//! complete without panicking) for the exact byte sequence below.

use rsp_core::control::{ControlError, EnvelopeDecoder, MAX_CONTROL_FRAME};

#[test]
fn envelope_rejects_oversized_length_before_payload() {
    const OVERSIZED_FRAME: [u8; 4] = [0x00, 0x10, 0x00, 0x01];
    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&OVERSIZED_FRAME);
    assert_eq!(
        decoder.next(),
        Err(ControlError::FrameTooLarge(MAX_CONTROL_FRAME + 1))
    );
}

#[test]
fn envelope_error_is_sticky() {
    const OVERSIZED_FRAME: [u8; 4] = [0xff, 0xff, 0xff, 0xff];
    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&OVERSIZED_FRAME);
    assert_eq!(
        decoder.next(),
        Err(ControlError::FrameTooLarge(u32::MAX as usize))
    );
    decoder.push(b"ignored");
    assert_eq!(
        decoder.next(),
        Err(ControlError::FrameTooLarge(u32::MAX as usize))
    );
}
