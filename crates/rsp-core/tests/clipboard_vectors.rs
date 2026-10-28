use std::fs;
use std::path::{Path, PathBuf};
use std::sync::Once;

use rsp_core::clipboard::{encode_stream, ClipboardItem, ClipboardKind, StreamReceiver};
use serde_json::{json, Value};
use sha2::{Digest, Sha256};

const PNG_SMALL: [u8; 68] = [
    0x89, 0x50, 0x4e, 0x47, 0x0d, 0x0a, 0x1a, 0x0a, 0x00, 0x00, 0x00, 0x0d, 0x49, 0x48, 0x44, 0x52,
    0x00, 0x00, 0x00, 0x01, 0x00, 0x00, 0x00, 0x01, 0x08, 0x04, 0x00, 0x00, 0x00, 0xb5, 0x1c, 0x0c,
    0x02, 0x00, 0x00, 0x00, 0x0b, 0x49, 0x44, 0x41, 0x54, 0x78, 0xda, 0x63, 0x64, 0xf8, 0x0f, 0x00,
    0x01, 0x05, 0x01, 0x01, 0x27, 0x18, 0xe3, 0x66, 0x00, 0x00, 0x00, 0x00, 0x49, 0x45, 0x4e, 0x44,
    0xae, 0x42, 0x60, 0x82,
];

static REGEN_ONCE: Once = Once::new();

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/clipboard")
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn unhex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0, "hex test vector has an odd length");
    (0..value.len())
        .step_by(2)
        .map(|index| u8::from_str_radix(&value[index..index + 2], 16).expect("hex test vector"))
        .collect()
}

fn is_regen() -> bool {
    std::env::var_os("RSP_REGEN_VECTORS").is_some_and(|value| value == "1")
}

fn png_item() -> ClipboardItem {
    ClipboardItem::new(ClipboardKind::Png, PNG_SMALL.to_vec())
        .expect("fixed PNG is a valid clipboard item")
}

fn png_json() -> Value {
    let sha256 = hex(&Sha256::digest(PNG_SMALL));
    json!({
        "name": "clipboard_png_small",
        "feature": "CLIPBOARD_IMAGE",
        "encoding": "quic-stream",
        "kind": "png",
        "size": PNG_SMALL.len(),
        "sha256": sha256,
        "png_hex": hex(&PNG_SMALL),
        "binary": "png_small.bin",
        "description": "Complete QUIC clipboard stream carrying a real 68-byte 1x1 grayscale-with-alpha PNG."
    })
}

fn ensure_vector_files(dir: &Path) -> Vec<u8> {
    let stream = encode_stream(&png_item()).expect("encode fixed PNG clipboard stream");
    let description = png_json();
    let mut pretty = serde_json::to_vec_pretty(&description).expect("serialize vector json");
    pretty.push(b'\n');

    if is_regen() {
        REGEN_ONCE.call_once(|| {
            fs::create_dir_all(dir).expect("create clipboard vector directory");
            fs::write(dir.join("png_small.bin"), &stream).expect("write PNG vector binary");
            fs::write(dir.join("png_small.json"), &pretty).expect("write PNG vector json");
        });
    } else {
        assert_eq!(
            fs::read(dir.join("png_small.bin")).expect("read png_small.bin"),
            stream,
            "png_small.bin drifted from encode_stream output"
        );
        assert_eq!(
            fs::read(dir.join("png_small.json")).expect("read png_small.json"),
            pretty,
            "png_small.json drifted from its expected fields"
        );
    }
    stream
}

fn receive_all(stream: &[u8]) -> ClipboardItem {
    let mut receiver = StreamReceiver::new();
    receiver.feed(stream).expect("one-shot clipboard feed");
    receiver.finish().expect("one-shot clipboard finish")
}

fn receive_bytewise(stream: &[u8]) -> ClipboardItem {
    let mut receiver = StreamReceiver::new();
    for byte in stream {
        receiver.feed(&[*byte]).expect("byte-wise clipboard feed");
    }
    receiver.finish().expect("byte-wise clipboard finish")
}

#[test]
fn png_clipboard_vector_round_trips() {
    let stream = ensure_vector_files(&vector_dir());
    let item = png_item();

    for received in [receive_all(&stream), receive_bytewise(&stream)] {
        assert_eq!(received.kind, ClipboardKind::Png);
        assert_eq!(received.data.as_ref(), PNG_SMALL.as_slice());
        assert_eq!(received, item);
    }
    assert_eq!(
        encode_stream(&item).expect("re-encode fixed PNG"),
        stream,
        "re-encoded clipboard stream differs from png_small.bin"
    );
}

#[test]
fn png_clipboard_json_matches_binary() {
    let _stream = ensure_vector_files(&vector_dir());
    let raw: Value = serde_json::from_slice(
        &fs::read(vector_dir().join("png_small.json")).expect("read png_small.json"),
    )
    .expect("parse png_small.json");

    assert_eq!(raw["name"], "clipboard_png_small");
    assert_eq!(raw["feature"], "CLIPBOARD_IMAGE");
    assert_eq!(raw["encoding"], "quic-stream");
    assert_eq!(raw["kind"], "png");
    assert_eq!(raw["size"], PNG_SMALL.len());
    assert_eq!(raw["binary"], "png_small.bin");
    let png = unhex(raw["png_hex"].as_str().expect("png_hex is a string"));
    assert_eq!(png, PNG_SMALL);
    assert_eq!(
        raw["sha256"].as_str().expect("sha256 is a string"),
        hex(&Sha256::digest(&png))
    );
    assert_eq!(raw, png_json(), "png_small.json fields drifted");
}

#[test]
fn png_small_is_a_real_png() {
    assert_eq!(&PNG_SMALL[..8], b"\x89PNG\r\n\x1a\n");
    assert_eq!(&PNG_SMALL[12..16], b"IHDR");
    assert_eq!(u32::from_be_bytes(PNG_SMALL[16..20].try_into().unwrap()), 1);
    assert_eq!(u32::from_be_bytes(PNG_SMALL[20..24].try_into().unwrap()), 1);
}
