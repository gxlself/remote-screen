#![no_main]
#![allow(clippy::chunks_exact_to_as_chunks)]

use libfuzzer_sys::fuzz_target;
use rsp_core::pointer::{PointerDatagram, PointerFilter};

fuzz_target!(|input: &[u8]| {
    if let Ok(datagram) = PointerDatagram::decode(input) {
        let mut filter = PointerFilter::new();
        assert!(filter.accept(&datagram));
        assert!(!filter.accept(&datagram), "duplicate sequence was accepted");
        assert_eq!(filter.last_seq(), Some(datagram.seq));
    }

    let mut filter = PointerFilter::new();
    let mut last_accepted = None;
    for chunk in input.chunks_exact(rsp_core::pointer::POINTER_DATAGRAM_LEN) {
        let Ok(datagram) = PointerDatagram::decode(chunk) else {
            continue;
        };
        let immediate_duplicate = last_accepted == Some(datagram.seq);
        let accepted = filter.accept(&datagram);
        assert!(
            !(immediate_duplicate && accepted),
            "filter accepted duplicate sequence {}",
            datagram.seq
        );
        if accepted {
            assert_eq!(filter.last_seq(), Some(datagram.seq));
            last_accepted = Some(datagram.seq);
        }
    }
});
