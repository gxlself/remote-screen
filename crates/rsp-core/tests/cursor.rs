use std::fs;
use std::path::PathBuf;

use data_encoding::HEXLOWER;
use rsp_core::cursor::{
    encode, shape_id, CursorError, CursorEvent, CursorState, CursorStreamDecoder, ShapeCache,
    MAX_CURSOR_MESSAGE_LEN, SHAPE_CACHE_CAPACITY,
};
use rsp_core::pb::cursor_msg::Msg;
use rsp_core::pb::{CursorMsg, CursorShape};
use rsp_core::wire::DecodeError;
use serde_json::Value;

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/cursor")
}

fn read_vector(name: &str) -> (Vec<u8>, Value) {
    let dir = vector_dir();
    let bin = fs::read(dir.join(format!("{name}.bin"))).expect("vector binary");
    let json: Value =
        serde_json::from_slice(&fs::read(dir.join(format!("{name}.json"))).expect("vector json"))
            .expect("valid vector json");
    (bin, json)
}

fn decode_one(bin: &[u8]) -> CursorMsg {
    let mut decoder = CursorStreamDecoder::new();
    decoder.push(bin);
    decoder
        .next_msg()
        .expect("vector decodes")
        .expect("one message")
}

fn shape_from_json(json: &Value) -> CursorShape {
    CursorShape {
        id: json["id"].as_u64().expect("id"),
        width: json["width"].as_u64().expect("width") as u32,
        height: json["height"].as_u64().expect("height") as u32,
        hot_x: json["hot_x"].as_u64().expect("hot_x") as u32,
        hot_y: json["hot_y"].as_u64().expect("hot_y") as u32,
        scale: json["scale"].as_f64().expect("scale") as f32,
        png: HEXLOWER
            .decode(json["png_hex"].as_str().expect("png_hex").as_bytes())
            .expect("valid png hex"),
    }
}

fn msg_from_json(json: &Value) -> CursorMsg {
    let msg = match json["kind"].as_str().expect("kind") {
        "shape" => Msg::Shape(shape_from_json(&json["shape"])),
        "use" => Msg::Use(json["use"].as_u64().expect("use")),
        "hidden" => Msg::Hidden(json["hidden"].as_bool().expect("hidden")),
        other => panic!("unknown cursor vector kind: {other}"),
    };
    CursorMsg { msg: Some(msg) }
}

#[test]
fn cursor_vectors_decode_match_json_and_reencode_exactly() {
    for name in ["shape", "use", "hidden"] {
        let (bin, json) = read_vector(name);
        let expected = msg_from_json(&json);
        let decoded = decode_one(&bin);
        assert_eq!(decoded, expected, "decoded value differs for {name}");
        assert_eq!(encode(&decoded), bin, "re-encoded bytes differ for {name}");
    }
}

#[test]
fn hidden_false_round_trips() {
    let msg = CursorMsg {
        msg: Some(Msg::Hidden(false)),
    };
    assert_eq!(decode_one(&encode(&msg)), msg);
}

#[test]
fn shape_id_is_xxh3_64_of_png_bytes() {
    assert_eq!(shape_id(b""), 0x2d06_8005_38d3_94c2);
    let (_, json) = read_vector("shape");
    let shape = shape_from_json(&json["shape"]);
    assert_eq!(shape_id(&shape.png), shape.id);
    assert_ne!(shape_id(&shape.png), shape_id(b"different pixels"));
}

#[test]
fn cursor_stream_decoder_handles_split_and_coalesced_records() {
    let (shape_bin, shape_json) = read_vector("shape");
    let (use_bin, use_json) = read_vector("use");
    let (hidden_bin, hidden_json) = read_vector("hidden");
    let mut all = Vec::new();
    all.extend_from_slice(&shape_bin);
    all.extend_from_slice(&use_bin);
    all.extend_from_slice(&hidden_bin);

    let mut decoder = CursorStreamDecoder::new();
    let mut messages = Vec::new();
    for byte in all {
        decoder.push(&[byte]);
        while let Some(msg) = decoder.next_msg().unwrap() {
            messages.push(msg);
        }
    }
    assert_eq!(
        messages,
        [
            msg_from_json(&shape_json),
            msg_from_json(&use_json),
            msg_from_json(&hidden_json),
        ]
    );
    assert!(decoder.next_msg().unwrap().is_none());
}

#[test]
fn oversized_and_malformed_cursor_records_are_rejected() {
    let mut decoder = CursorStreamDecoder::new();
    decoder.push(&((MAX_CURSOR_MESSAGE_LEN as u32) + 1).to_be_bytes());
    assert!(matches!(
        decoder.next_msg(),
        Err(CursorError::Wire(DecodeError::FrameTooLarge(_)))
    ));

    let mut decoder = CursorStreamDecoder::new();
    decoder.push(&1u32.to_be_bytes());
    decoder.push(&[0xff]);
    assert!(matches!(decoder.next_msg(), Err(CursorError::Protobuf(_))));

    let mut decoder = CursorStreamDecoder::new();
    decoder.push(&0u32.to_be_bytes());
    assert!(matches!(decoder.next_msg(), Err(CursorError::EmptyMessage)));
}

fn shape(id: u64) -> CursorShape {
    CursorShape {
        id,
        width: 16,
        height: 16,
        hot_x: 1,
        hot_y: 2,
        scale: 2.0,
        png: vec![id as u8; 8],
    }
}

#[test]
fn lru_evicts_the_least_recently_used_shape() {
    let mut cache = ShapeCache::new();
    assert_eq!(cache.capacity(), SHAPE_CACHE_CAPACITY);
    for id in 0..SHAPE_CACHE_CAPACITY as u64 {
        cache.insert(shape(id));
    }
    assert_eq!(cache.len(), 64);

    assert!(cache.get(0).is_some());
    cache.insert(shape(64));
    assert_eq!(cache.len(), 64);
    assert!(cache.get(1).is_none(), "shape 1 was least recently used");
    assert!(cache.get(0).is_some(), "shape 0 was refreshed by get");
    assert!(cache.get(64).is_some());
}

#[test]
fn replacing_an_existing_shape_does_not_evict_another() {
    let mut cache = ShapeCache::new();
    for id in 0..64 {
        cache.insert(shape(id));
    }
    cache.insert(CursorShape {
        width: 99,
        ..shape(0)
    });
    assert_eq!(cache.len(), 64);
    assert_eq!(cache.get(0).unwrap().width, 99);
}

#[test]
fn cursor_state_tracks_show_hide_use_and_missing() {
    let mut state = CursorState::new();
    let initial = CursorShape {
        id: 0x1122_3344_5566_7788,
        width: 32,
        height: 32,
        hot_x: 2,
        hot_y: 3,
        scale: 2.0,
        png: vec![1, 2, 3, 4],
    };
    let shown = state.apply(CursorMsg {
        msg: Some(Msg::Shape(initial.clone())),
    });
    let CursorEvent::Show(shown_shape) = shown else {
        panic!("shape should show");
    };
    assert_eq!(shown_shape.as_ref(), &initial);

    assert_eq!(
        state.apply(CursorMsg {
            msg: Some(Msg::Hidden(true)),
        }),
        CursorEvent::Hide
    );
    assert!(state.is_hidden());
    let CursorEvent::Show(restored) = state.apply(CursorMsg {
        msg: Some(Msg::Hidden(false)),
    }) else {
        panic!("unhiding should restore the current shape");
    };
    assert_eq!(restored.as_ref(), &initial);
    assert!(!state.is_hidden());

    let CursorEvent::Show(used) = state.apply(CursorMsg {
        msg: Some(Msg::Use(initial.id)),
    }) else {
        panic!("cache hit should show");
    };
    assert_eq!(used.as_ref(), &initial);
    assert_eq!(state.current(), Some(&initial));

    assert_eq!(
        state.apply(CursorMsg {
            msg: Some(Msg::Use(0xdead_beef)),
        }),
        CursorEvent::Missing(0xdead_beef)
    );
}

#[test]
fn use_hit_refreshes_lru_order() {
    let mut state = CursorState::new();
    for id in 0..64 {
        state.apply(CursorMsg {
            msg: Some(Msg::Shape(shape(id))),
        });
    }
    assert!(matches!(
        state.apply(CursorMsg {
            msg: Some(Msg::Use(0)),
        }),
        CursorEvent::Show(_)
    ));
    state.apply(CursorMsg {
        msg: Some(Msg::Shape(shape(64))),
    });
    assert!(state.cache_mut().get(0).is_some());
    assert!(state.cache_mut().get(1).is_none());
}
