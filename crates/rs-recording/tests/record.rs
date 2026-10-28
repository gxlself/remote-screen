use proptest::prelude::*;
use rs_recording::record::{
    decode, iter, AudioRec, Body, ConfigRec, CursorShapeRec, GapReason, Record, RecordError,
    RecordingEnd, RecordingMeta, VideoRec, HEADER_LEN, MAX_BODY,
};

fn meta() -> RecordingMeta {
    RecordingMeta {
        host_id: "host-1".into(),
        host_name: "Example".into(),
        role: "viewer".into(),
        display_id: Some(2),
        started_at_us: 123,
        app_version: "1.2.3".into(),
        audio: true,
    }
}

fn end() -> RecordingEnd {
    RecordingEnd {
        duration_us: 5,
        frames: 6,
        keyframes: 1,
        audio_packets: 2,
        gaps: 3,
        dropped: 4,
        plain_bytes: 7,
        ended_at_us: 8,
        reason: "user".into(),
        width: 1920,
        height: 1080,
        codec: 2,
    }
}

fn all_bodies(local: Option<i64>) -> Vec<Record> {
    vec![
        Record {
            t_us: 0,
            body: Body::Meta(meta()),
        },
        Record {
            t_us: 1,
            body: Body::Config(ConfigRec {
                epoch: 2,
                display_id: 3,
                codec: 1,
                width: 1920,
                height: 1080,
                fps: 60,
                bitrate_kbps: 5000,
                cursor_x: 1.25,
                cursor_y: -2.5,
                cursor_visible: true,
            }),
        },
        Record {
            t_us: 2,
            body: Body::Video(VideoRec {
                frame_id: 4,
                epoch: 5,
                display_id: 6,
                flags: 7,
                keyframe: true,
                capture_ts_us: 8,
                capture_local_us: local,
                data: vec![9, 10, 11],
            }),
        },
        Record {
            t_us: 3,
            body: Body::CursorPos {
                x: 3.5,
                y: 4.5,
                visible: false,
            },
        },
        Record {
            t_us: 4,
            body: Body::CursorShape(CursorShapeRec {
                id: 5,
                width: 6,
                height: 7,
                hot_x: 8,
                hot_y: 9,
                scale: 1.5,
                png: vec![1, 2, 3],
            }),
        },
        Record {
            t_us: 5,
            body: Body::CursorUse(6),
        },
        Record {
            t_us: 6,
            body: Body::CursorHidden(true),
        },
        Record {
            t_us: 7,
            body: Body::Audio(AudioRec {
                seq: 8,
                capture_ts_us: 9,
                payload: vec![10, 11],
            }),
        },
        Record {
            t_us: 8,
            body: Body::Gap {
                reason: GapReason::Paused,
                real_us: 12,
            },
        },
        Record {
            t_us: 9,
            body: Body::End(end()),
        },
        Record {
            t_us: 10,
            body: Body::Unknown {
                kind: 200,
                body: vec![1, 2, 3],
            },
        },
    ]
}

fn encode(records: &[Record]) -> Vec<u8> {
    let mut bytes = Vec::new();
    for record in records {
        let before = bytes.len();
        record.encode_into(&mut bytes);
        assert_eq!(record.encoded_len(), bytes.len() - before);
    }
    bytes
}

#[test]
fn every_body_round_trips_with_both_local_timestamp_forms() {
    for local in [Some(-5), None] {
        let records = all_bodies(local);
        let bytes = encode(&records);
        assert_eq!(decode(&bytes).unwrap(), records);
    }
}

#[test]
fn cursor_use_fixed_vector() {
    let record = Record {
        t_us: 1,
        body: Body::CursorUse(0x0102_0304_0506_0708),
    };
    let mut bytes = Vec::new();
    record.encode_into(&mut bytes);
    assert_eq!(
        bytes,
        vec![6, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 8, 1, 2, 3, 4, 5, 6, 7, 8]
    );
}

#[test]
fn truncation_is_reported_at_record_start() {
    let full = [
        6, 0, 0, 0, 0, 0, 0, 0, 1, 0, 0, 0, 8, 1, 2, 3, 4, 5, 6, 7, 8,
    ];
    assert_eq!(
        decode(&full[..full.len() - 1]),
        Err(RecordError::Truncated { offset: 0 })
    );
    assert_eq!(
        decode(&full[..5]),
        Err(RecordError::Truncated { offset: 0 })
    );
}

#[test]
fn oversized_body_is_rejected_before_reading_body() {
    let mut bytes = vec![0u8; HEADER_LEN];
    bytes[9..13].copy_from_slice(&(MAX_BODY + 1).to_be_bytes());
    assert_eq!(
        decode(&bytes),
        Err(RecordError::TooLarge { len: MAX_BODY + 1 })
    );
}

fn raw_record(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut bytes = vec![kind];
    bytes.extend_from_slice(&1u64.to_be_bytes());
    bytes.extend_from_slice(&(body.len() as u32).to_be_bytes());
    bytes.extend_from_slice(body);
    bytes
}

#[test]
fn fixed_lengths_and_all_boolean_fields_are_validated() {
    for (kind, len) in [(2, 34), (4, 9), (6, 8), (7, 1), (9, 9)] {
        for wrong_len in [0, len - 1, len + 1] {
            assert!(matches!(
                decode(&raw_record(kind, &vec![0; wrong_len])),
                Err(RecordError::BadBody { kind: actual, .. }) if actual == kind
            ));
        }
    }
    for (kind, len, bool_offset) in [(2, 34, 33), (3, 24, 7), (4, 9, 8), (7, 1, 0)] {
        for invalid in [2, 255] {
            let mut body = vec![0; len];
            body[bool_offset] = invalid;
            assert!(matches!(
                decode(&raw_record(kind, &body)),
                Err(RecordError::BadBody { kind: actual, .. }) if actual == kind
            ));
        }
    }
    for (kind, min_len) in [(3, 24), (5, 28), (8, 10)] {
        for len in 0..min_len {
            assert!(matches!(
                decode(&raw_record(kind, &vec![0; len])),
                Err(RecordError::BadBody { kind: actual, .. }) if actual == kind
            ));
        }
    }
}

#[test]
fn meta_and_end_require_valid_utf8_json_of_the_right_type() {
    for kind in [1, 10] {
        for invalid in [b"".as_slice(), b"{", b"{}", b"null", b"[]", &[255]] {
            assert!(matches!(
                decode(&raw_record(kind, invalid)),
                Err(RecordError::BadBody { kind: actual, .. }) if actual == kind
            ));
        }
    }
    let meta_value = serde_json::to_value(meta()).unwrap();
    assert_eq!(meta_value["hostId"], "host-1");
    assert_eq!(meta_value["startedAtUs"], 123);
    assert!(meta_value.get("host_id").is_none());
    let end_value = serde_json::to_value(end()).unwrap();
    assert_eq!(end_value["audioPackets"], 2);
    assert_eq!(end_value["plainBytes"], 7);
}

#[test]
fn unknown_kind_preserves_body_and_framing() {
    let unknown = Record {
        t_us: 1,
        body: Body::Unknown {
            kind: 200,
            body: vec![9, 8],
        },
    };
    let known = Record {
        t_us: 2,
        body: Body::CursorUse(7),
    };
    let records = [unknown, known];
    assert_eq!(decode(&encode(&records)).unwrap(), records);
}

#[test]
fn unknown_gap_reason_becomes_clock() {
    let mut gap = vec![9];
    gap.extend_from_slice(&44u64.to_be_bytes());
    assert_eq!(
        decode(&raw_record(9, &gap)).unwrap(),
        [Record {
            t_us: 1,
            body: Body::Gap {
                reason: GapReason::Clock,
                real_us: 44,
            },
        }]
    );
}

#[test]
fn float_bits_and_local_timestamp_sentinel_are_preserved() {
    for bits in [0, 0x8000_0000, 0x7f80_0000, 0x7fc0_1234, 0xff80_0000] {
        let value = f32::from_bits(bits);
        let record = Record {
            t_us: 0,
            body: Body::CursorPos {
                x: value,
                y: value,
                visible: true,
            },
        };
        let decoded = decode(&encode(&[record])).unwrap();
        match decoded[0].body {
            Body::CursorPos { x, y, visible } => {
                assert_eq!(x.to_bits(), bits);
                assert_eq!(y.to_bits(), bits);
                assert!(visible);
            }
            _ => panic!("expected cursor position"),
        }
    }
    for local in [Some(i64::MIN + 1), Some(i64::MAX), None] {
        let records = all_bodies(local);
        assert_eq!(decode(&encode(&records)).unwrap(), records);
    }
}

#[test]
fn iterator_is_lazy_and_reports_fatal_framing_errors_once() {
    let records = all_bodies(Some(-5));
    let bytes = encode(&records);
    assert_eq!(
        iter(&bytes).collect::<Result<Vec<_>, _>>().unwrap(),
        records
    );

    let good = Record {
        t_us: 5,
        body: Body::CursorUse(6),
    };
    let first = encode(std::slice::from_ref(&good));
    for tail in [&first[..5], &first[..first.len() - 1]] {
        let mut bytes = first.clone();
        bytes.extend_from_slice(tail);
        let mut reader = iter(&bytes);
        assert_eq!(reader.next(), Some(Ok(good.clone())));
        assert_eq!(
            reader.next(),
            Some(Err(RecordError::Truncated {
                offset: first.len()
            }))
        );
        assert_eq!(reader.next(), None);
        assert_eq!(reader.next(), None);
    }

    let mut bytes = first.clone();
    let mut header = vec![0; HEADER_LEN];
    header[9..13].copy_from_slice(&(MAX_BODY + 1).to_be_bytes());
    bytes.extend_from_slice(&header);
    let mut reader = iter(&bytes);
    assert_eq!(reader.next(), Some(Ok(good.clone())));
    assert_eq!(
        reader.next(),
        Some(Err(RecordError::TooLarge { len: MAX_BODY + 1 }))
    );
    assert_eq!(reader.next(), None);

    let mut bytes = raw_record(7, &[2]);
    bytes.extend_from_slice(&first);
    let mut reader = iter(&bytes);
    assert!(matches!(
        reader.next(),
        Some(Err(RecordError::BadBody { kind: 7, .. }))
    ));
    assert_eq!(reader.next(), Some(Ok(good)));
    assert_eq!(reader.next(), None);
    assert!(decode(&[]).unwrap().is_empty());
}

fn arb_float() -> impl Strategy<Value = f32> {
    any::<i16>().prop_map(|value| f32::from(value) / 10.0)
}

fn arb_body() -> impl Strategy<Value = Body> {
    prop_oneof![
        (
            ".{0,24}",
            ".{0,24}",
            any::<bool>(),
            prop::option::of(any::<u32>()),
            any::<u64>(),
            ".{0,16}",
            any::<bool>()
        )
            .prop_map(
                |(
                    host_id,
                    host_name,
                    controller,
                    display_id,
                    started_at_us,
                    app_version,
                    audio,
                )| Body::Meta(RecordingMeta {
                    host_id,
                    host_name,
                    role: if controller { "controller" } else { "viewer" }.into(),
                    display_id,
                    started_at_us,
                    app_version,
                    audio,
                })
            ),
        (
            any::<u32>(),
            any::<u32>(),
            any::<u8>(),
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
            arb_float(),
            arb_float(),
            any::<bool>()
        )
            .prop_map(
                |(
                    epoch,
                    display_id,
                    codec,
                    width,
                    height,
                    fps,
                    bitrate_kbps,
                    cursor_x,
                    cursor_y,
                    cursor_visible,
                )| Body::Config(ConfigRec {
                    epoch,
                    display_id,
                    codec,
                    width,
                    height,
                    fps,
                    bitrate_kbps,
                    cursor_x,
                    cursor_y,
                    cursor_visible,
                })
            ),
        (
            any::<u32>(),
            any::<u8>(),
            any::<u8>(),
            any::<u8>(),
            any::<bool>(),
            any::<u64>(),
            prop::option::of((i64::MIN + 1)..=i64::MAX),
            prop::collection::vec(any::<u8>(), 0..=4096)
        )
            .prop_map(
                |(
                    frame_id,
                    epoch,
                    display_id,
                    flags,
                    keyframe,
                    capture_ts_us,
                    capture_local_us,
                    data,
                )| Body::Video(VideoRec {
                    frame_id,
                    epoch,
                    display_id,
                    flags,
                    keyframe,
                    capture_ts_us,
                    capture_local_us,
                    data,
                })
            ),
        (arb_float(), arb_float(), any::<bool>()).prop_map(|(x, y, visible)| Body::CursorPos {
            x,
            y,
            visible
        }),
        (
            any::<u64>(),
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
            any::<u32>(),
            arb_float(),
            prop::collection::vec(any::<u8>(), 0..128)
        )
            .prop_map(
                |(id, width, height, hot_x, hot_y, scale, png)| Body::CursorShape(CursorShapeRec {
                    id,
                    width,
                    height,
                    hot_x,
                    hot_y,
                    scale,
                    png
                })
            ),
        any::<u64>().prop_map(Body::CursorUse),
        any::<bool>().prop_map(Body::CursorHidden),
        (
            any::<u16>(),
            any::<u64>(),
            prop::collection::vec(any::<u8>(), 0..128)
        )
            .prop_map(|(seq, capture_ts_us, payload)| Body::Audio(AudioRec {
                seq,
                capture_ts_us,
                payload
            })),
        (
            prop::sample::select(vec![
                GapReason::Backpressure,
                GapReason::Paused,
                GapReason::Reconnect,
                GapReason::Clock
            ]),
            any::<u64>()
        )
            .prop_map(|(reason, real_us)| Body::Gap { reason, real_us }),
        (
            any::<[u64; 8]>(),
            any::<u32>(),
            any::<u32>(),
            any::<u8>(),
            ".{0,24}"
        )
            .prop_map(
                |(counts, width, height, codec, reason)| Body::End(RecordingEnd {
                    duration_us: counts[0],
                    frames: counts[1],
                    keyframes: counts[2],
                    audio_packets: counts[3],
                    gaps: counts[4],
                    dropped: counts[5],
                    plain_bytes: counts[6],
                    ended_at_us: counts[7],
                    reason,
                    width,
                    height,
                    codec,
                })
            ),
        (11u8..=255, prop::collection::vec(any::<u8>(), 0..128))
            .prop_map(|(kind, body)| Body::Unknown { kind, body }),
    ]
}

proptest! {
    #[test]
    fn random_mixed_records_round_trip(records in prop::collection::vec((any::<u64>(), arb_body()).prop_map(|(t_us, body)| Record { t_us, body }), 0..32)) {
        let bytes = encode(&records);
        prop_assert_eq!(decode(&bytes).unwrap(), records);
    }
}
