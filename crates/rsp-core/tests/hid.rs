use std::fs;
use std::path::PathBuf;

use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit, TouchPhase, MAX_TEXT_BYTES};
use rsp_core::wire::DecodeError;
use serde_json::Value;

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/hid")
}

fn button(name: &str) -> Button {
    match name {
        "left" => Button::Left,
        "right" => Button::Right,
        "middle" => Button::Middle,
        "back" => Button::Back,
        "forward" => Button::Forward,
        other => panic!("unknown button in vector: {other}"),
    }
}

fn scroll_phase(name: &str) -> ScrollPhase {
    match name {
        "none" => ScrollPhase::None,
        "began" => ScrollPhase::Began,
        "changed" => ScrollPhase::Changed,
        "ended" => ScrollPhase::Ended,
        other => panic!("unknown scroll phase in vector: {other}"),
    }
}

fn scroll_unit(name: &str) -> ScrollUnit {
    match name {
        "pixel" => ScrollUnit::Pixel,
        "line" => ScrollUnit::Line,
        other => panic!("unknown scroll unit in vector: {other}"),
    }
}

fn touch_phase(name: &str) -> TouchPhase {
    match name {
        "began" => TouchPhase::Began,
        "moved" => TouchPhase::Moved,
        "ended" => TouchPhase::Ended,
        "cancelled" => TouchPhase::Cancelled,
        other => panic!("unknown touch phase in vector: {other}"),
    }
}

fn f32_field(json: &Value, name: &str) -> f32 {
    json[name].as_f64().expect("number field") as f32
}

fn u64_field(json: &Value, name: &str) -> u64 {
    json[name].as_u64().expect("unsigned field")
}

fn input_from_json(json: &Value) -> InputMsg {
    match json["type"].as_str().expect("type") {
        "MouseMove" => InputMsg::MouseMove {
            x: f32_field(json, "x"),
            y: f32_field(json, "y"),
        },
        "MouseButton" => InputMsg::MouseButton {
            button: button(json["button"].as_str().expect("button")),
            down: json["down"].as_bool().expect("down"),
            x: f32_field(json, "x"),
            y: f32_field(json, "y"),
            clicks: u64_field(json, "clicks") as u8,
        },
        "Scroll" => InputMsg::Scroll {
            dx: f32_field(json, "dx"),
            dy: f32_field(json, "dy"),
            phase: scroll_phase(json["phase"].as_str().expect("phase")),
            momentum: scroll_phase(json["momentum"].as_str().expect("momentum")),
            unit: scroll_unit(json["unit"].as_str().expect("unit")),
        },
        "Key" => InputMsg::Key {
            usage: u64_field(json, "usage") as u16,
            down: json["down"].as_bool().expect("down"),
            mods: Mods::from_bits(u64_field(json, "mods") as u8),
        },
        "Text" => InputMsg::Text(json["text"].as_str().expect("text").to_owned()),
        "ReleaseAll" => InputMsg::ReleaseAll,
        "Touch" => InputMsg::Touch {
            id: u64_field(json, "id") as u8,
            phase: touch_phase(json["phase"].as_str().expect("phase")),
            x: f32_field(json, "x"),
            y: f32_field(json, "y"),
            pressure: f32_field(json, "pressure"),
        },
        other => panic!("unknown input vector type: {other}"),
    }
}

#[test]
fn input_vectors_decode_match_json_and_reencode_exactly() {
    let names = [
        "mouse_move",
        "mouse_button",
        "scroll",
        "key",
        "text",
        "release_all",
        "touch",
    ];
    for name in names {
        let dir = vector_dir();
        let bin = fs::read(dir.join(format!("{name}.bin"))).expect("vector binary");
        let json: Value = serde_json::from_slice(
            &fs::read(dir.join(format!("{name}.json"))).expect("vector json"),
        )
        .expect("valid vector json");
        let expected = input_from_json(&json);
        let decoded = InputMsg::decode(&bin).expect("vector decodes");
        assert_eq!(decoded, expected, "decoded value differs for {name}");
        assert_eq!(
            decoded.encode().unwrap(),
            bin,
            "re-encoded bytes differ for {name}"
        );

        let framed = decoded.to_frame().unwrap();
        assert_eq!(&framed[..4], &(bin.len() as u32).to_be_bytes());
        assert_eq!(&framed[4..], bin);
    }
}

#[test]
fn every_input_event_round_trips() {
    let events = [
        InputMsg::MouseMove { x: 0.0, y: 1.0 },
        InputMsg::MouseButton {
            button: Button::Forward,
            down: true,
            x: 0.25,
            y: 0.75,
            clicks: 3,
        },
        InputMsg::Scroll {
            dx: -4.5,
            dy: 18.0,
            phase: ScrollPhase::Began,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Line,
        },
        InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::LCTRL | Mods::RSHIFT | Mods::RMETA,
        },
        InputMsg::Text("hello 世界".to_owned()),
        InputMsg::Text(String::new()),
        InputMsg::ReleaseAll,
        InputMsg::Touch {
            id: 9,
            phase: TouchPhase::Cancelled,
            x: 0.5,
            y: 0.4,
            pressure: 0.25,
        },
    ];
    for event in events {
        assert_eq!(InputMsg::decode(&event.encode().unwrap()).unwrap(), event);
    }
}

#[test]
fn mods_are_hid_ordered_bits() {
    assert_eq!(Mods::LCTRL.bits(), 0x01);
    assert_eq!(Mods::LSHIFT.bits(), 0x02);
    assert_eq!(Mods::LALT.bits(), 0x04);
    assert_eq!(Mods::LMETA.bits(), 0x08);
    assert_eq!(Mods::RCTRL.bits(), 0x10);
    assert_eq!(Mods::RSHIFT.bits(), 0x20);
    assert_eq!(Mods::RALT.bits(), 0x40);
    assert_eq!(Mods::RMETA.bits(), 0x80);
    let all = Mods::LCTRL
        | Mods::LSHIFT
        | Mods::LALT
        | Mods::LMETA
        | Mods::RCTRL
        | Mods::RSHIFT
        | Mods::RALT
        | Mods::RMETA;
    assert_eq!(all.bits(), u8::MAX);
    assert!(all.contains(Mods::LMETA));
}

#[test]
fn coordinates_are_finite_and_clamped() {
    for msg in [
        InputMsg::MouseMove { x: 1.3, y: -0.2 },
        InputMsg::MouseButton {
            button: Button::Left,
            down: false,
            x: 3.0,
            y: -3.0,
            clicks: 0,
        },
        InputMsg::Touch {
            id: 1,
            phase: TouchPhase::Moved,
            x: 1.4,
            y: -0.1,
            pressure: 2.0,
        },
    ] {
        let decoded = InputMsg::decode(&msg.encode().unwrap()).unwrap();
        match decoded {
            InputMsg::MouseMove { x, y } => {
                assert_eq!((x, y), (1.0, 0.0));
            }
            InputMsg::MouseButton { x, y, clicks, .. } => {
                assert_eq!((x, y, clicks), (1.0, 0.0, 1));
            }
            InputMsg::Touch { x, y, pressure, .. } => {
                assert_eq!((x, y, pressure), (1.0, 0.0, 1.0));
            }
            other => panic!("unexpected decoded event: {other:?}"),
        }
    }
}

#[test]
fn non_finite_values_are_rejected() {
    let mut move_msg = vec![0x01];
    move_msg.extend_from_slice(&f32::NAN.to_be_bytes());
    move_msg.extend_from_slice(&0.5f32.to_be_bytes());
    assert_eq!(
        InputMsg::decode(&move_msg),
        Err(DecodeError::InvalidValue("coordinate"))
    );

    let mut scroll = vec![0x03];
    scroll.extend_from_slice(&f32::INFINITY.to_be_bytes());
    scroll.extend_from_slice(&0.0f32.to_be_bytes());
    scroll.extend_from_slice(&[1, 0, 0]);
    assert_eq!(
        InputMsg::decode(&scroll),
        Err(DecodeError::InvalidValue("scroll delta"))
    );

    let mut touch = vec![0x07, 1, 1];
    touch.extend_from_slice(&0.5f32.to_be_bytes());
    touch.extend_from_slice(&0.5f32.to_be_bytes());
    touch.extend_from_slice(&f32::NEG_INFINITY.to_be_bytes());
    assert_eq!(
        InputMsg::decode(&touch),
        Err(DecodeError::InvalidValue("pressure"))
    );
}

#[test]
fn unknown_types_enums_and_trailing_bytes_are_rejected() {
    assert_eq!(
        InputMsg::decode(&[0x08]),
        Err(DecodeError::UnknownType(0x08))
    );
    assert_eq!(InputMsg::decode(&[0x01, 0, 0]), Err(DecodeError::Truncated));

    let mut trailing = InputMsg::MouseMove { x: 0.1, y: 0.2 }.encode().unwrap();
    trailing.push(0xaa);
    assert_eq!(
        InputMsg::decode(&trailing),
        Err(DecodeError::TrailingBytes(1))
    );

    let mut bad_button = vec![0x02, 5, 1];
    bad_button.extend_from_slice(&0.5f32.to_be_bytes());
    bad_button.extend_from_slice(&0.5f32.to_be_bytes());
    bad_button.push(1);
    assert_eq!(
        InputMsg::decode(&bad_button),
        Err(DecodeError::InvalidValue("mouse button"))
    );

    let mut scroll = vec![0x03];
    scroll.extend_from_slice(&0.0f32.to_be_bytes());
    scroll.extend_from_slice(&0.0f32.to_be_bytes());
    scroll.extend_from_slice(&[4, 0, 0]);
    assert_eq!(
        InputMsg::decode(&scroll),
        Err(DecodeError::InvalidValue("scroll phase"))
    );

    scroll[9] = 4;
    assert_eq!(
        InputMsg::decode(&scroll),
        Err(DecodeError::InvalidValue("scroll phase"))
    );

    scroll[9] = 0;
    scroll[10] = 0;
    scroll[11] = 2;
    assert_eq!(
        InputMsg::decode(&scroll),
        Err(DecodeError::InvalidValue("scroll unit"))
    );

    let mut touch = vec![0x07, 1, 4];
    touch.extend_from_slice(&0.0f32.to_be_bytes());
    touch.extend_from_slice(&0.0f32.to_be_bytes());
    touch.extend_from_slice(&0.0f32.to_be_bytes());
    assert_eq!(
        InputMsg::decode(&touch),
        Err(DecodeError::InvalidValue("touch phase"))
    );
}

#[test]
fn text_length_utf8_and_boundaries_are_enforced() {
    let at_limit = InputMsg::Text("a".repeat(MAX_TEXT_BYTES));
    assert_eq!(
        InputMsg::decode(&at_limit.encode().unwrap()).unwrap(),
        at_limit
    );

    let too_long = InputMsg::Text("a".repeat(MAX_TEXT_BYTES + 1));
    assert!(too_long.encode().is_err());

    let mut record = vec![0x05];
    record.extend_from_slice(&((MAX_TEXT_BYTES + 1) as u16).to_be_bytes());
    record.extend(std::iter::repeat_n(b'a', MAX_TEXT_BYTES + 1));
    assert_eq!(
        InputMsg::decode(&record),
        Err(DecodeError::InvalidValue("text length"))
    );

    assert_eq!(
        InputMsg::decode(&[0x05, 0x00, 0x02, 0xff, 0xfe]),
        Err(DecodeError::BadUtf8)
    );
}
