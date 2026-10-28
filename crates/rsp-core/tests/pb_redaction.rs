use rsp_core::control::{encode, EnvelopeDecoder};
use rsp_core::pb;

const ACCOUNT: &str = "alice";
const PASSWORD: &[u8] = b"hunter2-xyz";

fn pair_password() -> pb::PairPassword {
    pb::PairPassword {
        device_name: "New iPad".into(),
        platform: pb::Platform::Ipados as i32,
        account: ACCOUNT.into(),
        password: PASSWORD.to_vec(),
    }
}

fn assert_redacted(debug: &str) {
    assert!(debug.contains("<redacted>"));
    for secret in [ACCOUNT, "hunter2-xyz", "104, 117", "104,117", "68756e74"] {
        assert!(
            !debug.contains(secret),
            "Debug output leaked PairPassword credentials"
        );
    }
}

#[test]
fn pair_password_debug_redacts_credentials() {
    assert_redacted(&format!("{:?}", pair_password()));
}

#[test]
fn enveloped_pair_password_debug_redacts_credentials() {
    let envelope = pb::Envelope::from(pair_password());
    assert_redacted(&format!("{envelope:?}"));
}

#[test]
fn pair_password_wire_encoding_preserves_credentials() {
    let expected = pair_password();
    let framed = encode(&pb::Envelope::from(expected.clone()));
    let mut decoder = EnvelopeDecoder::new();
    decoder.push(&framed);
    let envelope = decoder
        .next()
        .expect("pair password frame decodes")
        .expect("pair password envelope is complete");
    let decoded = match envelope.msg {
        Some(pb::envelope::Msg::PairPassword(message)) => message,
        _ => panic!("expected a PairPassword envelope"),
    };

    assert_eq!(decoded, expected);
    assert_eq!(decoded.account, ACCOUNT);
    assert_eq!(decoded.password, PASSWORD);
    assert!(decoder.next().expect("no trailing frame").is_none());
}
