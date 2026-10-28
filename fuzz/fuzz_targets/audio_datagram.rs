#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::audio::{decode, encode};

fuzz_target!(|input: &[u8]| {
    if let Ok((header, payload)) = decode(input) {
        let encoded = encode(header, payload).expect("decoded audio datagram re-encodes");
        assert_eq!(
            encoded.as_ref(),
            input,
            "accepted audio datagram changed when re-encoded"
        );
    }
});
