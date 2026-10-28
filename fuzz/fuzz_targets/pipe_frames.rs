#![no_main]

use arbitrary::Unstructured;
use host_win::pipe_proto::{encode_frame, FrameReader, PipeError, PipeFrame, MAX_FRAME};
use libfuzzer_sys::fuzz_target;

const MAX_PUSHES: usize = 256;
const MAX_CHUNK: usize = 64;

fn oversized_length_is_rejected_before_payload() {
    let oversized = MAX_FRAME + 1;
    let mut reader = FrameReader::new();
    reader.push(&(oversized as u32).to_le_bytes());
    assert!(
        matches!(reader.next(), Err(PipeError::TooLarge(actual)) if actual == oversized),
        "an oversized frame length must be rejected with its declared size"
    );
}

fn check_round_trip(frame: &PipeFrame) {
    let encoded = encode_frame(frame).expect("an accepted pipe frame must re-encode");
    assert!(
        encoded.len() <= MAX_FRAME + 4,
        "an accepted pipe frame exceeds the length-prefixed limit"
    );

    let mut reader = FrameReader::new();
    reader.push(&encoded);
    let decoded = reader
        .next()
        .expect("a re-encoded pipe frame must decode")
        .expect("a re-encoded pipe frame must be complete");
    assert_eq!(
        encode_frame(&decoded).expect("a decoded pipe frame must re-encode"),
        encoded,
        "a decoded pipe frame differs from the accepted frame"
    );
    assert!(
        matches!(reader.next(), Ok(None)),
        "a complete pipe frame left a framing error"
    );
}

fn check_error(error: PipeError) {
    if let PipeError::TooLarge(declared) = error {
        assert!(
            declared > MAX_FRAME,
            "a frame was rejected as too large without exceeding MAX_FRAME"
        );
    }
}

fuzz_target!(|input: &[u8]| {
    oversized_length_is_rejected_before_payload();

    let Some((&selector, stream)) = input.split_first() else {
        return;
    };
    if stream.is_empty() {
        return;
    }

    let mut chunk_source = Unstructured::new(stream);
    let mut reader = FrameReader::new();
    let chunk_mask = usize::from(selector);
    let mut position = 0usize;
    let mut pushes = 0usize;

    while position < stream.len() && pushes < MAX_PUSHES {
        let remaining = stream.len() - position;
        let requested = usize::from(chunk_source.arbitrary::<u8>().unwrap_or(1));
        let chunk_len = ((requested ^ chunk_mask) % remaining.min(MAX_CHUNK)) + 1;
        let end = position + chunk_len;
        reader.push(&stream[position..end]);
        position = end;
        pushes += 1;

        loop {
            match reader.next() {
                Ok(Some(frame)) => check_round_trip(&frame),
                Ok(None) => break,
                Err(error) => {
                    check_error(error);
                    // FrameReader does not remove an invalid prefix, so an
                    // error is effectively sticky. Stop rather than calling
                    // next() again after a failure.
                    return;
                }
            }
        }
    }
});
