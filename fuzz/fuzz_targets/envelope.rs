#![no_main]

use libfuzzer_sys::fuzz_target;
use prost::Message;
use rsp_core::control::{self, ControlError, EnvelopeDecoder, MAX_CONTROL_FRAME};
use rsp_core::pb;

const CHUNK_MODULO: usize = 64;
const TRAILING: &[u8] = b"bytes pushed after the decoder became sticky";

fn round_trip(envelope: pb::Envelope) {
    let encoded = control::encode(&envelope);
    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&encoded);
    let decoded = decoder
        .next()
        .expect("re-encoded envelope must decode")
        .expect("re-encoded envelope must be complete");
    assert_eq!(
        control::encode(&decoded),
        encoded,
        "re-encoded envelope bytes differ"
    );
    assert!(
        decoder
            .next()
            .expect("a complete frame must not leave an error")
            .is_none(),
        "replay produced more than one envelope"
    );
}

fn expect_frame_too_large(bytes: &[u8]) {
    if bytes.len() < 4 {
        return;
    }
    let declared = u32::from_be_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]) as usize;
    if declared <= MAX_CONTROL_FRAME {
        return;
    }

    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&bytes[..4]);
    assert_eq!(
        decoder.next(),
        Err(ControlError::FrameTooLarge(declared)),
        "oversized length must fail before its payload arrives"
    );

    decoder.push(TRAILING);
    assert_eq!(
        decoder.next(),
        Err(ControlError::FrameTooLarge(declared)),
        "oversized-frame errors must be sticky"
    );
}

fuzz_target!(|data: &[u8]| {
    // Channel 0x00 carries a bare Envelope in TCP mode, without the u32 length.
    let _ = pb::Envelope::decode(data);

    // The first four bytes of the original input are a length declaration too.
    expect_frame_too_large(data);

    let Some((&chunk_selector, stream)) = data.split_first() else {
        return;
    };
    let chunk_size = 1 + usize::from(chunk_selector) % CHUNK_MODULO;

    let mut decoder = EnvelopeDecoder::new();
    let mut sticky: Option<ControlError> = None;
    for chunk in stream.chunks(chunk_size) {
        decoder.push(chunk);
        loop {
            match decoder.next() {
                Ok(None) => break,
                Ok(Some(envelope)) => {
                    assert!(
                        sticky.is_none(),
                        "decoder emitted an envelope after becoming sticky"
                    );
                    round_trip(envelope);
                }
                Err(error) => {
                    if let Some(previous) = &sticky {
                        assert_eq!(&error, previous, "decoder error is not sticky");
                    } else {
                        sticky = Some(error);
                    }
                    break;
                }
            }
        }
        if sticky.is_some() {
            break;
        }
    }

    if let Some(error) = sticky {
        for _ in 0..3 {
            decoder.push(TRAILING);
            assert_eq!(decoder.next(), Err(error.clone()));
        }
    }

    expect_frame_too_large(stream);
});
