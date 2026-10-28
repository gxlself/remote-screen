use std::fs;
use std::path::PathBuf;

use rsp_core::pointer::{PointerDatagram, PointerFilter, POINTER_DATAGRAM_LEN, POINTER_TYPE};
use rsp_core::wire::DecodeError;
use serde_json::Value;

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/pointer")
}

fn datagram(seq: u32, visible: bool, x: f32, y: f32) -> PointerDatagram {
    PointerDatagram {
        seq,
        display: 2,
        visible,
        x,
        y,
    }
}

#[test]
fn pointer_vector_decodes_matches_json_and_reencodes_exactly() {
    let dir = vector_dir();
    let bin = fs::read(dir.join("basic.bin")).expect("vector binary");
    let json: Value =
        serde_json::from_slice(&fs::read(dir.join("basic.json")).expect("vector json"))
            .expect("valid vector json");
    let expected = PointerDatagram {
        seq: json["seq"].as_u64().expect("seq") as u32,
        display: json["display"].as_u64().expect("display") as u8,
        visible: json["visible"].as_bool().expect("visible"),
        x: json["x"].as_f64().expect("x") as f32,
        y: json["y"].as_f64().expect("y") as f32,
    };
    assert_eq!(bin.len(), POINTER_DATAGRAM_LEN);
    let decoded = PointerDatagram::decode(&bin).expect("vector decodes");
    assert_eq!(decoded, expected);
    assert_eq!(decoded.encode().as_slice(), bin);
}

#[test]
fn datagrams_are_exactly_fifteen_bytes_and_round_trip() {
    for (visible, x, y) in [
        (true, 0.0, 0.0),
        (false, 1.0, 1.0),
        (true, 0.125, 0.875),
        (false, 123.5, -9.25),
    ] {
        let d = datagram(0x0102_0304, visible, x, y);
        let bytes = d.encode();
        assert_eq!(bytes.len(), POINTER_DATAGRAM_LEN);
        assert_eq!(bytes[0], POINTER_TYPE);
        assert_eq!(PointerDatagram::decode(&bytes).unwrap(), d);
    }
}

#[test]
fn length_type_and_flags_are_validated() {
    let bytes = datagram(1, true, 0.5, 0.5).encode();
    assert_eq!(
        PointerDatagram::decode(&bytes[..POINTER_DATAGRAM_LEN - 1]),
        Err(DecodeError::Truncated)
    );
    let mut long = bytes.to_vec();
    long.push(0);
    assert_eq!(
        PointerDatagram::decode(&long),
        Err(DecodeError::TrailingBytes(1))
    );
    let mut wrong_type = bytes;
    wrong_type[0] = 0x13;
    assert_eq!(
        PointerDatagram::decode(&wrong_type),
        Err(DecodeError::UnknownType(0x13))
    );
    for invalid in [0x02, 0x80, 0xff] {
        let mut flagged = bytes;
        flagged[6] = invalid;
        assert_eq!(
            PointerDatagram::decode(&flagged),
            Err(DecodeError::InvalidValue("pointer flags"))
        );
    }
}

#[test]
fn non_finite_coordinates_are_rejected() {
    let mut bytes = datagram(1, true, 0.5, 0.5).encode();
    bytes[7..11].copy_from_slice(&f32::NAN.to_be_bytes());
    assert_eq!(
        PointerDatagram::decode(&bytes),
        Err(DecodeError::InvalidValue("pointer coordinate"))
    );
}

#[test]
fn out_of_order_sequences_only_accept_newest() {
    let mut filter = PointerFilter::new();
    let mut accepted = Vec::new();
    for seq in [5, 3, 6, 4] {
        if filter.accept(&datagram(seq, true, 0.5, 0.5)) {
            accepted.push(seq);
        }
    }
    assert_eq!(accepted, [5, 6]);
    assert_eq!(filter.last_seq(), Some(6));
}

#[test]
fn first_datagram_is_always_accepted_and_duplicates_are_rejected() {
    let mut filter = PointerFilter::new();
    assert!(filter.accept(&datagram(0, false, 0.0, 0.0)));
    assert!(!filter.accept(&datagram(0, false, 0.0, 0.0)));
    assert!(!filter.accept(&datagram(u32::MAX, false, 0.0, 0.0)));
}

#[test]
fn sequence_wrap_is_accepted() {
    let mut filter = PointerFilter::new();
    assert!(filter.accept(&datagram(u32::MAX - 1, true, 0.0, 0.0)));
    assert!(filter.accept(&datagram(u32::MAX, true, 0.0, 0.0)));
    assert!(filter.accept(&datagram(0, true, 0.0, 0.0)));
    assert!(filter.accept(&datagram(1, true, 0.0, 0.0)));
    assert!(!filter.accept(&datagram(0, true, 0.0, 0.0)));
    assert!(!filter.accept(&datagram(1, true, 0.0, 0.0)));
}
