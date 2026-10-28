#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_transport::mux::CONTROL_CHANNEL;
use rsp_transport::tcp::{TcpRecordDecoder, MAX_CONTROL_RECORD, MAX_DATA_RECORD};

fuzz_target!(|input: &[u8]| {
    let chunk_size = usize::from(input.first().copied().unwrap_or(0)) % 64 + 1;
    let mut decoder = TcpRecordDecoder::default();
    for chunk in input.chunks(chunk_size) {
        match decoder.push(chunk) {
            Ok(records) => {
                for (channel, payload) in records {
                    let limit = if channel == CONTROL_CHANNEL {
                        MAX_CONTROL_RECORD
                    } else {
                        MAX_DATA_RECORD
                    };
                    assert!(
                        payload.len() <= limit,
                        "channel {channel} produced a {}-byte payload",
                        payload.len()
                    );
                }
            }
            Err(_) => break,
        }
    }
    let _ = decoder.finish();

    let mut oversized_header = vec![CONTROL_CHANNEL];
    oversized_header.extend_from_slice(&((MAX_CONTROL_RECORD as u32) + 1).to_be_bytes());
    assert!(TcpRecordDecoder::default().push(&oversized_header).is_err());
});
