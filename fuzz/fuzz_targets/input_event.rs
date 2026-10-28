#![no_main]

use libfuzzer_sys::fuzz_target;
use rsp_core::hid::InputMsg;

fuzz_target!(|input: &[u8]| {
    let Ok(message) = InputMsg::decode(input) else {
        return;
    };
    let encoded = match message.encode() {
        Ok(encoded) => encoded,
        Err(error) => panic!("decoded {message:?} failed to re-encode: {error}"),
    };
    match InputMsg::decode(&encoded) {
        Ok(decoded) => assert_eq!(decoded, message),
        Err(error) => panic!("re-decoding an encoded {message:?} failed: {error}"),
    }
});
