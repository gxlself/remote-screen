#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::video::{VideoHeader, HEADER_LEN};

fuzz_target!(|data: &[u8]| {
    let Ok((header, rest)) = VideoHeader::parse(data) else {
        return;
    };
    assert!(
        data.len() >= HEADER_LEN,
        "accepted a video datagram shorter than its fixed header"
    );
    assert_eq!(
        header.encode().as_slice(),
        &data[..HEADER_LEN],
        "accepted video header changed when re-encoded"
    );
    assert_eq!(
        rest.len(),
        data.len() - HEADER_LEN,
        "accepted video payload length does not match the datagram"
    );
    assert_eq!(
        rest,
        &data[HEADER_LEN..],
        "accepted video payload is not the datagram suffix"
    );
});
