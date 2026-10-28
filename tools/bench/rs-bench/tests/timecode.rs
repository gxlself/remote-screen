use rs_bench::timecode::{
    bits_to_word, cell_center_norm, cell_rect, crc8, decode_word, encode, quiet_probes_norm,
    quiet_rect, render_calib, render_code, unit_x, unit_y, word_bits, Code, GrayImage, PxRect,
    DATA_BITS, PAYLOAD_MASK, WHITEN_MASK,
};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../vectors/timecode.json"))
        .expect("timecode vector JSON must be valid")
}

fn hex_u64(value: &Value, key: &str) -> u64 {
    u64::from_str_radix(
        value[key].as_str().expect("vector field must be a string"),
        16,
    )
    .expect("vector hex field must be valid")
}

fn rect_from_json(value: &Value) -> PxRect {
    let values = value.as_array().expect("rectangle must be an array");
    PxRect {
        x0: values[0].as_u64().expect("x0 must be an integer") as u32,
        y0: values[1].as_u64().expect("y0 must be an integer") as u32,
        x1: values[2].as_u64().expect("x1 must be an integer") as u32,
        y1: values[3].as_u64().expect("y1 must be an integer") as u32,
    }
}

fn code_from_json(code: &Value) -> Code {
    match code["type"].as_str().expect("code type must be a string") {
        "time" => Code::time_from_unix_ms(code["ms"].as_u64().expect("time ms must be an integer")),
        "calib" => Code::Calib {
            width: code["width"]
                .as_u64()
                .expect("calib width must be an integer") as u32,
            height: code["height"]
                .as_u64()
                .expect("calib height must be an integer") as u32,
        },
        "end" => Code::End,
        other => panic!("unknown vector code type {other}"),
    }
}

#[test]
fn crc8_check_value() {
    assert_eq!(crc8(b"123456789"), 0xf4);
}

#[test]
fn vectors_encode() {
    let root = vectors();
    for case in root["cases"].as_array().expect("cases must be an array") {
        let code = code_from_json(&case["code"]);
        assert_eq!(encode(code), hex_u64(case, "whitened"), "{}", case["name"]);
        assert_eq!(
            code.type_id(),
            case["type"].as_u64().unwrap() as u8,
            "{}",
            case["name"]
        );
        assert_eq!(
            code.payload(),
            case["payload"].as_u64().unwrap(),
            "{}",
            case["name"]
        );
        let bits: String = word_bits(encode(code))
            .into_iter()
            .map(|bit| if bit { '1' } else { '0' })
            .collect();
        assert_eq!(bits, case["bits"].as_str().unwrap(), "{}", case["name"]);
    }
}

#[test]
fn vectors_decode_roundtrip() {
    let root = vectors();
    for case in root["cases"].as_array().unwrap() {
        let code = code_from_json(&case["code"]);
        let whitened = hex_u64(case, "whitened");
        assert_eq!(decode_word(whitened), Some(code), "{}", case["name"]);
        assert_eq!(bits_to_word(&word_bits(whitened)), whitened);
    }
}

#[test]
fn single_bit_flip_rejected() {
    let root = vectors();
    let case = root["cases"]
        .as_array()
        .unwrap()
        .iter()
        .find(|case| case["name"] == "time_sample")
        .unwrap();
    let whitened = hex_u64(case, "whitened");
    for index in 0..DATA_BITS {
        assert_eq!(
            decode_word(whitened ^ (1u64 << (47 - index))),
            None,
            "bit {index}"
        );
    }
}

fn raw_word(type_id: u8, payload: u64) -> u64 {
    let v40 = ((type_id as u64) << 36) | (payload & PAYLOAD_MASK);
    let bytes = v40.to_be_bytes();
    let word = (v40 << 8) | crc8(&bytes[3..8]) as u64;
    word ^ WHITEN_MASK
}

#[test]
fn invalid_words() {
    assert_eq!(decode_word(raw_word(1, 0) | (1 << 48)), None);
    assert_eq!(decode_word(raw_word(0, 0)), None);
    assert_eq!(decode_word(raw_word(4, 0)), None);
    assert_eq!(
        decode_word(encode(Code::Calib {
            width: 0,
            height: 5
        })),
        None
    );
}

#[test]
fn time_payload_masked() {
    assert_eq!(
        Code::time_from_unix_ms(1_792_507_200_123),
        Code::Time(5_800_804_987)
    );
    assert_eq!(encode(Code::Time((1 << 36) | 7)), encode(Code::Time(7)));
}

#[test]
fn vectors_geometry() {
    let root = vectors();
    for geometry in root["geometry"].as_array().unwrap() {
        let width = geometry["width"].as_u64().unwrap() as u32;
        let height = geometry["height"].as_u64().unwrap() as u32;
        assert_eq!(
            quiet_rect(width, height),
            rect_from_json(&geometry["quiet"])
        );
        assert_eq!(
            cell_rect(0, 0, width, height),
            rect_from_json(&geometry["cell_0_0"])
        );
        assert_eq!(
            cell_rect(7, 5, width, height),
            rect_from_json(&geometry["cell_7_5"])
        );
    }
    assert_eq!(unit_x(3, 1366), 64);
    assert_eq!(unit_y(15, 768), 320);
}

#[test]
fn cell_centers() {
    assert_eq!(cell_center_norm(0, 0), (3.5 / 64.0, 15.5 / 36.0));
    assert_eq!(cell_center_norm(7, 5), (10.5 / 64.0, 20.5 / 36.0));
    assert_eq!(
        quiet_probes_norm(),
        [
            (2.5 / 64.0, 14.5 / 36.0),
            (11.5 / 64.0, 14.5 / 36.0),
            (2.5 / 64.0, 21.5 / 36.0),
            (11.5 / 64.0, 21.5 / 36.0),
        ]
    );
}

#[test]
fn render_code_pixels() {
    let code = Code::Time(5_800_804_987);
    let mut image = GrayImage::new(1920, 1080, 77);
    render_code(&mut image, code);
    let bits = word_bits(encode(code));
    assert_eq!(image.get(91, 451), if bits[0] { 255 } else { 0 });
    assert_eq!(image.get(301, 601), if bits[47] { 255 } else { 0 });
    assert_eq!(image.get(61, 421), 0);
    assert_eq!(image.get(59, 419), 77);
    assert_eq!(image.get(360, 660), 77);
}

#[test]
fn render_calib_both_phases() {
    let white = render_calib(1280, 720, true);
    let black = render_calib(1280, 720, false);
    assert_eq!(white.get(0, 0), 255);
    assert_eq!(white.get(41, 281), 0);
    assert_eq!(black.get(0, 0), 0);
    for row in 0..6 {
        for col in 0..8 {
            let rect = cell_rect(col, row, 1280, 720);
            if rect.width() > 0 && rect.height() > 0 {
                assert_eq!(
                    white.get(rect.x0, rect.y0),
                    black.get(rect.x0, rect.y0),
                    "cell ({col}, {row})"
                );
            }
        }
    }
}

#[test]
fn fill_rect_and_crop_clip() {
    let mut image = GrayImage::new(5, 4, 1);
    image.fill_rect(
        PxRect {
            x0: 3,
            y0: 2,
            x1: 9,
            y1: 9,
        },
        9,
    );
    for y in 0..4 {
        for x in 0..5 {
            assert_eq!(image.get(x, y), if x >= 3 && y >= 2 { 9 } else { 1 });
        }
    }
    let crop = image.crop(PxRect {
        x0: 2,
        y0: 1,
        x1: 6,
        y1: 3,
    });
    assert_eq!((crop.width, crop.height), (3, 2));
    assert_eq!(crop.data, vec![1, 1, 1, 1, 9, 9]);
}

#[test]
fn pgm_roundtrip() {
    let image = GrayImage::from_raw(3, 2, vec![0, 1, 2, 253, 254, 255]).unwrap();
    let pgm = image.to_pgm();
    assert!(pgm.starts_with(b"P5\n3 2\n255\n"));
    assert_eq!(GrayImage::from_pgm(&pgm).unwrap(), image);

    let commented = b"P5\n# x\n3 2\n255\n\0\x01\x02\xfd\xfe\xff";
    assert_eq!(GrayImage::from_pgm(commented).unwrap(), image);
}

#[test]
fn pgm_rejects() {
    assert!(GrayImage::from_pgm(b"P2\n1 1\n255\n\0").is_err());
    assert!(GrayImage::from_pgm(b"P5\n1 1\n65535\n\0").is_err());
    assert!(GrayImage::from_pgm(b"P5\n2 1\n255\n\0").is_err());
    assert!(GrayImage::from_raw(2, 2, vec![0; 3]).is_err());
}
