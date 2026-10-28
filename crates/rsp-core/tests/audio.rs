use std::fs;
use std::path::PathBuf;

use proptest::prelude::*;
use rsp_core::audio::{
    decode, encode, seq_delta, seq_newer, AudioEncodeError, AudioHeader, AUDIO_HEADER_LEN,
    AUDIO_TYPE, MAX_AUDIO_DATAGRAM, MAX_OPUS_PACKET,
};
use rsp_core::wire::DecodeError;
use serde_json::Value;

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/audio")
}

fn header(seq: u16, capture_ts_us: u64) -> AudioHeader {
    AudioHeader { seq, capture_ts_us }
}

#[test]
fn basic_vector_decodes_matches_json_and_reencodes_exactly() {
    let dir = vector_dir();
    let bin = fs::read(dir.join("basic.bin")).expect("basic vector binary");
    let json: Value =
        serde_json::from_slice(&fs::read(dir.join("basic.json")).expect("basic vector json"))
            .expect("valid basic vector json");

    assert_eq!(
        bin,
        [0x11, 0x01, 0x02, 0x00, 0x06, 0x0a, 0x24, 0x18, 0x1e, 0x40, 0x00, 0xf4, 0xff, 0xfe,]
    );
    assert_eq!(json["seq"].as_u64(), Some(258));
    assert_eq!(json["capture_ts_us"].as_u64(), Some(1_700_000_000_000_000));
    assert_eq!(json["payload_hex"].as_str(), Some("f4fffe"));

    let (decoded_header, payload) = decode(&bin).expect("basic vector decodes");
    assert_eq!(decoded_header, header(258, 1_700_000_000_000_000));
    assert_eq!(payload, [0xf4, 0xff, 0xfe]);
    assert_eq!(
        encode(decoded_header, payload)
            .expect("basic vector re-encodes")
            .as_ref(),
        bin
    );
}

#[test]
fn maximum_vector_decodes_and_reencodes_exactly() {
    let dir = vector_dir();
    let bin = fs::read(dir.join("max.bin")).expect("max vector binary");
    let json: Value =
        serde_json::from_slice(&fs::read(dir.join("max.json")).expect("max vector json"))
            .expect("valid max vector json");

    assert_eq!(bin.len(), MAX_AUDIO_DATAGRAM);
    assert_eq!(json["seq"].as_u64(), Some(u16::MAX as u64));
    assert_eq!(json["capture_ts_us"].as_u64(), Some(0));
    assert_eq!(json["payload_len"].as_u64(), Some(MAX_OPUS_PACKET as u64));

    let (decoded_header, payload) = decode(&bin).expect("max vector decodes");
    assert_eq!(decoded_header, header(u16::MAX, 0));
    assert_eq!(payload.len(), MAX_OPUS_PACKET);
    assert!(payload.iter().all(|&byte| byte == 0xab));
    assert_eq!(
        encode(decoded_header, payload)
            .expect("max vector re-encodes")
            .as_ref(),
        bin
    );
}

#[test]
fn decode_validates_type_and_length_in_order() {
    assert_eq!(decode(&[]), Err(DecodeError::Truncated));
    assert_eq!(
        decode(&[0x12; AUDIO_HEADER_LEN + 1]),
        Err(DecodeError::UnknownType(0x12))
    );
    assert_eq!(
        decode(&[AUDIO_TYPE; AUDIO_HEADER_LEN]),
        Err(DecodeError::Truncated)
    );

    let one_byte = encode(header(7, 9), &[0x42]).expect("single-byte payload encodes");
    assert_eq!(one_byte.len(), AUDIO_HEADER_LEN + 1);
    let (decoded_header, payload) = decode(&one_byte).expect("single-byte payload decodes");
    assert_eq!(decoded_header, header(7, 9));
    assert_eq!(payload, [0x42]);

    let mut oversized = vec![0; MAX_AUDIO_DATAGRAM + 1];
    oversized[0] = AUDIO_TYPE;
    assert_eq!(
        decode(&oversized),
        Err(DecodeError::InvalidValue("audio payload length"))
    );
}

#[test]
fn encode_rejects_empty_and_oversized_payloads() {
    assert_eq!(
        encode(header(1, 2), &[]),
        Err(AudioEncodeError::EmptyPayload)
    );
    assert_eq!(
        encode(header(1, 2), &[0; MAX_OPUS_PACKET + 1]),
        Err(AudioEncodeError::PayloadTooLarge(MAX_OPUS_PACKET + 1))
    );
}

#[test]
fn sequence_helpers_use_wrapping_u16_distance() {
    assert_eq!(seq_delta(5, 65530), 11);
    assert_eq!(seq_delta(65530, 5), -11);
    assert_eq!(seq_delta(7, 7), 0);
    assert!(seq_newer(0, 65535));
    assert_eq!(seq_delta(0, 32768), i16::MIN);
    assert!(!seq_newer(0, 32768));
}

proptest! {
    #[test]
    fn arbitrary_valid_audio_round_trips(
        seq in any::<u16>(),
        capture_ts_us in any::<u64>(),
        payload in prop::collection::vec(any::<u8>(), 1..=MAX_OPUS_PACKET),
    ) {
        let expected = header(seq, capture_ts_us);
        let encoded = encode(expected, &payload).expect("valid payload encodes");
        let (decoded_header, decoded_payload) = decode(&encoded).expect("encoded audio decodes");
        prop_assert_eq!(decoded_header, expected);
        prop_assert_eq!(decoded_payload, payload.as_slice());
    }
}
