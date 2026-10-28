#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::file::{DataSpan, Fed, FileError, FileStreamReceiver, StreamEnd, MAX_HEADER_BYTES};

fn oversized_length_prefix_is_rejected() {
    let length = MAX_HEADER_BYTES + 1;
    let mut receiver = FileStreamReceiver::new();
    assert!(matches!(
        receiver.feed(&(length as u32).to_be_bytes()),
        Err(FileError::HeaderLengthTooLarge(actual)) if actual == length
    ));
}

fuzz_target!(|input: &[u8]| {
    oversized_length_prefix_is_rejected();

    let chunk_size = usize::from(input.first().copied().unwrap_or(0)) % 32 + 1;
    let mut receiver = FileStreamReceiver::new();
    let mut header = None;
    let mut header_count = 0usize;
    let mut expected_offset = None;
    let mut previous_position = receiver.position();

    for chunk in input.chunks(chunk_size) {
        let Fed {
            header: fed_header,
            data,
        } = match receiver.feed(chunk) {
            Ok(fed) => fed,
            Err(_) => break,
        };

        if let Some(new_header) = fed_header {
            assert_eq!(header_count, 0, "header was delivered more than once");
            assert!(
                new_header.offset <= new_header.size,
                "header offset must not exceed its declared size"
            );
            header_count += 1;
            header = Some(new_header);
        }

        if let Some(DataSpan { offset, bytes }) = data {
            let current_header = header
                .as_ref()
                .expect("data span was emitted before the header");
            let expected = expected_offset.unwrap_or(current_header.offset);
            assert_eq!(offset, expected, "data spans must be contiguous");

            let end = offset
                .checked_add(bytes.len() as u64)
                .expect("data span end overflowed");
            assert!(
                end <= current_header.size,
                "data span exceeded the declared file size"
            );
            assert_eq!(
                receiver.position(),
                end,
                "receiver position must match the last data span"
            );
            expected_offset = Some(end);
        }

        let position = receiver.position();
        assert!(
            position >= previous_position,
            "receiver position must be monotonic"
        );
        assert!(
            header.as_ref().is_none_or(|header| position <= header.size),
            "receiver position exceeded the declared file size"
        );
        previous_position = position;
    }

    let position = receiver.position();
    match receiver.finish() {
        Ok(StreamEnd::Complete { header }) => {
            assert_eq!(
                position, header.size,
                "a complete stream must end at the declared size"
            );
        }
        Ok(StreamEnd::Truncated { header, position }) => {
            assert!(
                position < header.size,
                "a truncated stream must end before the declared size"
            );
        }
        Err(_) => {}
    }
});
