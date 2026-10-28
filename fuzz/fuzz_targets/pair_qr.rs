#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::qr::PairQr;

fuzz_target!(|input: &[u8]| {
    let Ok(value) = std::str::from_utf8(input) else {
        return;
    };
    let Ok(qr) = PairQr::parse(value) else {
        return;
    };
    let encoded = qr.to_url();
    match PairQr::parse(&encoded) {
        Ok(decoded) => assert_eq!(decoded, qr),
        Err(error) => panic!("a generated pairing URL did not parse: {error}: {encoded}"),
    }
});
