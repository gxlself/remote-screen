use host_core::clip_convert::make_item;
use host_linux::clip_model::{
    data_for, item_from, offered_targets, pick_target, property_chunk, ClipTarget, IncrReader,
    IncrState, IncrWriter, MAX_PNG_BYTES, MAX_TEXT_BYTES,
};
use rsp_core::pb::clipboard_header::Kind;

#[test]
fn pick_order() {
    assert_eq!(
        pick_target(&["TARGETS", "image/png", "STRING", "UTF8_STRING"]),
        Some(ClipTarget::Utf8String)
    );
    assert_eq!(
        pick_target(&["image/png", "STRING"]),
        Some(ClipTarget::String)
    );
    assert_eq!(pick_target(&["image/png"]), Some(ClipTarget::Png));
    assert_eq!(pick_target(&["text/html"]), None);
    assert_eq!(
        pick_target(&["STRING", "text/plain;charset=utf-8"]),
        Some(ClipTarget::TextPlainUtf8)
    );
}

#[test]
fn text_decoding() {
    let item = item_from(ClipTarget::Utf8String, b"a\r\nb\rc\0".to_vec()).unwrap();
    assert_eq!(item.kind, Kind::Text);
    assert_eq!(item.data, b"a\nb\nc");
    assert_eq!(
        item.sha256,
        make_item(Kind::Text, b"a\nb\nc".to_vec()).sha256
    );
    assert_eq!(
        item_from(ClipTarget::String, vec![0xe9]).unwrap().data,
        "\u{e9}".as_bytes()
    );
    assert!(item_from(ClipTarget::Utf8String, vec![0xff]).is_none());
    assert!(item_from(ClipTarget::TextPlainUtf8, vec![0xff]).is_none());
    assert!(item_from(ClipTarget::Utf8String, vec![b'a'; MAX_TEXT_BYTES + 1]).is_none());
    assert!(item_from(ClipTarget::String, vec![0xe9; MAX_TEXT_BYTES / 2 + 1]).is_none());
    assert!(item_from(ClipTarget::Utf8String, vec![b'a'; MAX_TEXT_BYTES]).is_some());
    assert_eq!(
        item_from(ClipTarget::TextPlainUtf8, b"a\0b\0\0".to_vec())
            .unwrap()
            .data,
        b"a\0b"
    );
    assert!(item_from(ClipTarget::Utf8String, vec![])
        .unwrap()
        .data
        .is_empty());
}

#[test]
fn png_checks() {
    let data = b"\x89PNG\r\n\x1a\n".to_vec();
    let item = item_from(ClipTarget::Png, data.clone()).unwrap();
    assert_eq!(item.kind, Kind::Png);
    assert_eq!(item.data, data);
    assert_eq!(item.sha256, make_item(Kind::Png, data).sha256);
    assert!(item_from(ClipTarget::Png, b"not PNG".to_vec()).is_none());
    let mut data = vec![0; MAX_PNG_BYTES + 1];
    data[..8].copy_from_slice(b"\x89PNG\r\n\x1a\n");
    assert!(item_from(ClipTarget::Png, data).is_none());
}

#[test]
fn offered_and_data() {
    let text = make_item(Kind::Text, "\u{e9}\u{4e2d}".as_bytes().to_vec());
    assert_eq!(
        offered_targets(&text),
        vec![
            ClipTarget::Utf8String,
            ClipTarget::TextPlainUtf8,
            ClipTarget::String
        ]
    );
    assert_eq!(data_for(&text, ClipTarget::String), Some(vec![0xe9, b'?']));
    assert_eq!(
        data_for(&text, ClipTarget::Utf8String),
        Some(text.data.clone())
    );
    assert_eq!(
        data_for(&text, ClipTarget::TextPlainUtf8),
        Some(text.data.clone())
    );
    assert!(data_for(&text, ClipTarget::Png).is_none());
    let png = make_item(Kind::Png, b"\x89PNG\r\n\x1a\n".to_vec());
    assert_eq!(offered_targets(&png), vec![ClipTarget::Png]);
    assert!(data_for(&png, ClipTarget::Utf8String).is_none());
    assert_eq!(data_for(&png, ClipTarget::Png), Some(png.data));
    let unspecified = make_item(Kind::Unspecified, vec![]);
    assert!(offered_targets(&unspecified).is_empty());
    assert!(data_for(&unspecified, ClipTarget::String).is_none());
}

#[test]
fn chunk_size() {
    assert_eq!(property_chunk(65535), 262_076);
    assert_eq!(property_chunk(4_194_303), 262_144);
    assert_eq!(property_chunk(u32::MAX), 262_144);
    assert_eq!(property_chunk(0), 0);
    assert_eq!(property_chunk(16), 0);
    assert_eq!(property_chunk(17), 4);
}

#[test]
fn incr_reader() {
    let mut reader = IncrReader::new(4);
    assert_eq!(reader.push(b"ab"), IncrState::More);
    assert_eq!(reader.push(b"cd"), IncrState::More);
    assert_eq!(reader.push(b""), IncrState::Done(b"abcd".to_vec()));
    let mut reader = IncrReader::new(3);
    assert_eq!(reader.push(b"ab"), IncrState::More);
    assert_eq!(reader.push(b"cd"), IncrState::TooLarge);
    assert_eq!(reader.push(b""), IncrState::TooLarge);
    assert_eq!(reader.push(b"a"), IncrState::TooLarge);
    assert_eq!(IncrReader::new(0).push(b""), IncrState::Done(vec![]));
    assert_eq!(IncrReader::new(0).push(b"a"), IncrState::TooLarge);
}

#[test]
fn incr_writer() {
    let mut writer = IncrWriter::new((0..10).collect(), 4);
    assert_eq!(writer.total_len(), 10);
    assert_eq!(writer.next_chunk(), Some(&[0, 1, 2, 3][..]));
    assert_eq!(writer.next_chunk(), Some(&[4, 5, 6, 7][..]));
    assert_eq!(writer.next_chunk(), Some(&[8, 9][..]));
    assert_eq!(writer.next_chunk(), Some(&[][..]));
    assert_eq!(writer.next_chunk(), None);
    assert_eq!(writer.total_len(), 10);
    let mut empty = IncrWriter::new(vec![], 4);
    assert_eq!(empty.next_chunk(), Some(&[][..]));
    assert_eq!(empty.next_chunk(), None);
    let mut zero_chunk = IncrWriter::new(vec![1], 0);
    assert_eq!(zero_chunk.next_chunk(), Some(&[1][..]));
    assert_eq!(zero_chunk.next_chunk(), Some(&[][..]));
    assert_eq!(zero_chunk.next_chunk(), None);
}
