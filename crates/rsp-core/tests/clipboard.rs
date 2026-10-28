use std::fs;
use std::path::PathBuf;

use rsp_core::clipboard::{
    encode_stream, encode_stream_prefix, encode_tcp_chunks, ClipboardError, ClipboardItem,
    ClipboardKind, StreamReceiver, TcpReassembler, MAX_PNG_BYTES, MAX_TEXT_BYTES,
};

fn item(kind: ClipboardKind, data: impl Into<bytes::Bytes>) -> ClipboardItem {
    ClipboardItem::new(kind, data).expect("test item is valid")
}

fn receive(stream: &[u8], chunk_size: usize) -> Result<ClipboardItem, ClipboardError> {
    let mut receiver = StreamReceiver::new();
    for chunk in stream.chunks(chunk_size.max(1)) {
        receiver.feed(chunk)?;
    }
    receiver.finish()
}

fn header_prefix(header: &rsp_core::pb::ClipboardHeader) -> Vec<u8> {
    use prost::Message;

    let header = header.encode_to_vec();
    let mut prefix = Vec::with_capacity(4 + header.len());
    prefix.extend_from_slice(&(header.len() as u32).to_be_bytes());
    prefix.extend_from_slice(&header);
    prefix
}

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/clipboard")
}

#[test]
fn text_round_trips_at_every_chunking() {
    let text = "你好, Remote Screen 👋🏽\nhello\t世界";
    let item = item(ClipboardKind::Text, text.as_bytes());
    let stream = encode_stream(&item).unwrap();

    for chunk_size in [1, 2, 3, 7, 64, stream.len()] {
        assert_eq!(receive(&stream, chunk_size).unwrap(), item);
    }
}

#[test]
fn png_round_trips_with_arbitrary_bytes() {
    let data: Vec<u8> = (0..4096).map(|i| (i * 37 + 11) as u8).collect();
    let item = item(ClipboardKind::Png, data);
    assert_eq!(receive(&encode_stream(&item).unwrap(), 13).unwrap(), item);
}

#[test]
fn empty_text_round_trips() {
    let item = item(ClipboardKind::Text, Vec::new());
    assert_eq!(receive(&encode_stream(&item).unwrap(), 1).unwrap(), item);
}

#[test]
fn text_limit_is_inclusive() {
    let at_limit = item(ClipboardKind::Text, vec![b'a'; MAX_TEXT_BYTES]);
    assert_eq!(
        receive(&encode_stream(&at_limit).unwrap(), 8191).unwrap(),
        at_limit
    );

    let too_large = vec![b'a'; MAX_TEXT_BYTES + 1];
    assert!(matches!(
        ClipboardItem::new(ClipboardKind::Text, too_large.clone()),
        Err(ClipboardError::DataTooLarge { .. })
    ));
    assert!(matches!(
        encode_stream_prefix(&ClipboardItem {
            kind: ClipboardKind::Text,
            data: too_large.into(),
        }),
        Err(ClipboardError::DataTooLarge { .. })
    ));
}

#[test]
fn png_limit_is_inclusive() {
    let item = item(ClipboardKind::Png, vec![0x5a; MAX_PNG_BYTES]);
    assert_eq!(
        receive(&encode_stream(&item).unwrap(), 65536).unwrap(),
        item
    );

    let mut receiver = StreamReceiver::new();
    let header = rsp_core::pb::ClipboardHeader {
        kind: rsp_core::pb::clipboard_header::Kind::Png as i32,
        size: (MAX_PNG_BYTES + 1) as u64,
        sha256: vec![0; 32],
    };
    let prefix = header_prefix(&header);
    assert!(matches!(
        receiver.feed(&prefix),
        Err(ClipboardError::DataTooLarge {
            kind: ClipboardKind::Png,
            ..
        })
    ));
}

#[test]
fn header_size_limit_fails_before_data_arrives() {
    let mut receiver = StreamReceiver::new();
    let header = rsp_core::pb::ClipboardHeader {
        kind: rsp_core::pb::clipboard_header::Kind::Text as i32,
        size: (MAX_TEXT_BYTES + 1) as u64,
        sha256: vec![0; 32],
    };
    assert!(matches!(
        receiver.feed(&header_prefix(&header)),
        Err(ClipboardError::DataTooLarge { .. })
    ));
}

#[test]
fn header_length_prefix_over_one_kib_fails_immediately() {
    let mut receiver = StreamReceiver::new();
    assert!(matches!(
        receiver.feed(&1025u32.to_be_bytes()),
        Err(ClipboardError::HeaderLengthTooLarge(1025))
    ));
}

#[test]
fn sha256_tampering_is_rejected() {
    let item = item(ClipboardKind::Png, vec![1, 2, 3, 4, 5]);
    let mut stream = encode_stream(&item).unwrap();
    *stream.last_mut().unwrap() ^= 0x80;
    assert!(matches!(
        receive(&stream, 3),
        Err(ClipboardError::Sha256Mismatch)
    ));
}

#[test]
fn short_and_long_sizes_are_rejected() {
    let item = item(ClipboardKind::Png, vec![7; 64]);
    let mut short = encode_stream(&item).unwrap();
    short.pop();
    assert!(matches!(
        receive(&short, 5),
        Err(ClipboardError::SizeMismatch { .. })
    ));

    let mut long = encode_stream(&item).unwrap();
    long.push(8);
    assert!(matches!(
        receive(&long, 5),
        Err(ClipboardError::DataExceedsDeclaredSize { .. })
    ));
}

#[test]
fn invalid_utf8_text_is_rejected() {
    let data = [0xff, 0xfe, 0xfd, 0xfc];
    let sha = {
        use sha2::{Digest, Sha256};
        Sha256::digest(data).to_vec()
    };
    let header = rsp_core::pb::ClipboardHeader {
        kind: rsp_core::pb::clipboard_header::Kind::Text as i32,
        size: data.len() as u64,
        sha256: sha,
    };
    let mut framed = header_prefix(&header);
    framed.extend_from_slice(&data);
    assert!(matches!(
        receive(&framed, 2),
        Err(ClipboardError::InvalidUtf8)
    ));
}

#[test]
fn malformed_header_is_rejected() {
    let header = rsp_core::pb::ClipboardHeader {
        kind: rsp_core::pb::clipboard_header::Kind::Text as i32,
        size: 0,
        sha256: vec![0; 32],
    };
    let mut framed = header_prefix(&header);
    let declared = u32::from_be_bytes(framed[..4].try_into().unwrap()) as usize;
    framed.extend_from_slice(&[0x08]);
    framed[..4].copy_from_slice(&((declared + 1) as u32).to_be_bytes());
    let mut receiver = StreamReceiver::new();
    assert!(matches!(
        receiver.feed(&framed),
        Err(ClipboardError::InvalidHeader(_))
    ));
}

#[test]
fn invalid_header_sha_length_is_rejected() {
    let header = rsp_core::pb::ClipboardHeader {
        kind: rsp_core::pb::clipboard_header::Kind::Text as i32,
        size: 0,
        sha256: vec![0; 31],
    };
    let mut receiver = StreamReceiver::new();
    assert!(matches!(
        receiver.feed(&header_prefix(&header)),
        Err(ClipboardError::InvalidSha256Length(31))
    ));
}

#[test]
fn receiver_rejects_feed_after_error() {
    let mut receiver = StreamReceiver::new();
    assert!(matches!(
        receiver.feed(&1025u32.to_be_bytes()),
        Err(ClipboardError::HeaderLengthTooLarge(1025))
    ));
    assert!(matches!(receiver.feed(&[]), Ok(())));
    assert!(matches!(
        receiver.feed(&[0]),
        Err(ClipboardError::StreamFinished)
    ));
}

#[test]
fn empty_and_truncated_streams_have_distinct_errors() {
    assert!(matches!(
        StreamReceiver::new().finish(),
        Err(ClipboardError::EmptyStream)
    ));

    let mut receiver = StreamReceiver::new();
    receiver.feed(&[0, 0]).unwrap();
    assert!(matches!(
        receiver.finish(),
        Err(ClipboardError::HeaderTruncated { .. })
    ));
}

#[test]
fn tcp_chunks_have_exact_item_flags_and_boundaries() {
    let chunks = encode_tcp_chunks(0x0102_0304, &[1, 2, 3, 4, 5], 2);
    assert_eq!(chunks.len(), 3);
    assert_eq!(chunks[0], vec![1, 2, 3, 4, 0, 1, 2]);
    assert_eq!(chunks[1], vec![1, 2, 3, 4, 0, 3, 4]);
    assert_eq!(chunks[2], vec![1, 2, 3, 4, 1, 5]);
}

#[test]
fn tcp_two_items_interleaved() {
    let first = item(ClipboardKind::Text, "first item 你好".as_bytes());
    let second = item(
        ClipboardKind::Png,
        (0..1000).map(|i| i as u8).collect::<Vec<_>>(),
    );
    let first_chunks = encode_tcp_chunks(7, &encode_stream(&first).unwrap(), 9);
    let second_chunks = encode_tcp_chunks(42, &encode_stream(&second).unwrap(), 17);
    assert!(first_chunks.len() > 1 && second_chunks.len() > 1);

    let mut reassembler = TcpReassembler::new();
    let mut first_out = None;
    let mut second_out = None;
    let mut first_iter = first_chunks.into_iter();
    let mut second_iter = second_chunks.into_iter();
    loop {
        let mut progressed = false;
        if let Some(chunk) = first_iter.next() {
            if let Some(item) = reassembler.feed(&chunk).unwrap() {
                first_out = Some(item);
            }
            progressed = true;
        }
        if let Some(chunk) = second_iter.next() {
            if let Some(item) = reassembler.feed(&chunk).unwrap() {
                second_out = Some(item);
            }
            progressed = true;
        }
        if !progressed {
            break;
        }
    }
    assert_eq!(first_out.unwrap(), first);
    assert_eq!(second_out.unwrap(), second);
}

#[test]
fn tcp_rejects_more_than_four_items_in_flight() {
    let mut reassembler = TcpReassembler::new();
    for item_id in 1u32..=4 {
        let mut record = item_id.to_be_bytes().to_vec();
        record.push(0);
        record.extend_from_slice(&[0, 0]);
        assert_eq!(reassembler.feed(&record).unwrap(), None);
    }
    let mut fifth = 5u32.to_be_bytes().to_vec();
    fifth.push(0);
    assert!(matches!(
        reassembler.feed(&fifth),
        Err(ClipboardError::TooManyTcpItems)
    ));
}

#[test]
fn tcp_rejects_unknown_flag_bits() {
    let mut reassembler = TcpReassembler::new();
    let record = [0, 0, 0, 1, 0x02, 0];
    assert!(matches!(
        reassembler.feed(&record),
        Err(ClipboardError::TcpInvalidFlags { flags: 0x02 })
    ));
}

#[test]
fn tcp_rejects_short_records() {
    let mut reassembler = TcpReassembler::new();
    assert!(matches!(
        reassembler.feed(&[0, 0, 0, 1]),
        Err(ClipboardError::TcpRecordTooShort { .. })
    ));
}

#[test]
fn fixed_text_vector_matches() {
    let bin = fs::read(vector_dir().join("text_hello.bin")).unwrap();
    let json = String::from_utf8(fs::read(vector_dir().join("text_hello.json")).unwrap()).unwrap();
    let text = "hello, Remote Screen 你好";
    let expected = item(ClipboardKind::Text, text);
    assert_eq!(encode_stream(&expected).unwrap(), bin);
    assert_eq!(receive(&bin, 1).unwrap(), expected);
    assert!(json.contains("\"encoding\": \"quic-stream\""));
    assert!(json.contains("\"kind\": \"text\""));
    assert!(json.contains("\"size\": 27"));
    assert!(json.contains(
        "\"sha256\": \"a44d38b60214f2fbdd147b1bc66f260eb5f35e8a8affc7109f90277f644b33a3\""
    ));
}
