use std::fs;
use std::path::PathBuf;

use proptest::prelude::*;
use rsp_core::file::{
    build_header, encode_stream_prefix, new_transfer_id, resume_check, validate_header,
    validate_transfer_id, DataSpan, Fed, FileError, FileStreamReceiver, PartialState, ResumePlan,
    StreamEnd, MAX_FILE_BYTES, MAX_HEADER_BYTES, MAX_NAME_BYTES,
};
use rsp_core::pb::FileHeader;
use serde::Deserialize;
use sha2::{Digest, Sha256};

const TRANSFER_ID: &str = "00112233445566778899aabbccddeeff";
const MTIME_US: u64 = 1_735_689_600_000_000;
const DATA: &[u8] = b"hello world";

fn sha_hello() -> [u8; 32] {
    Sha256::digest(DATA).into()
}

fn hello_header(offset: u64) -> FileHeader {
    build_header(
        TRANSFER_ID,
        "hello.txt",
        DATA.len() as u64,
        offset,
        sha_hello(),
        MTIME_US,
    )
    .expect("test header is valid")
}

fn complete_stream(header: &FileHeader, data: &[u8]) -> Vec<u8> {
    let mut stream = encode_stream_prefix(header).expect("test header encodes");
    stream.extend_from_slice(data);
    stream
}

struct Received {
    receiver: FileStreamReceiver,
    header: Option<FileHeader>,
    header_count: usize,
    data: Vec<u8>,
    spans: Vec<(u64, usize)>,
}

impl Received {
    fn new() -> Self {
        Self {
            receiver: FileStreamReceiver::new(),
            header: None,
            header_count: 0,
            data: Vec::new(),
            spans: Vec::new(),
        }
    }

    fn feed(&mut self, chunk: &[u8]) {
        let Fed { header, data } = self.receiver.feed(chunk).expect("stream chunk is valid");
        if let Some(header) = header {
            self.header_count += 1;
            self.header = Some(header);
        }
        if let Some(DataSpan { offset, bytes }) = data {
            self.spans.push((offset, bytes.len()));
            self.data.extend_from_slice(bytes);
        }
    }

    fn feed_in_chunks(&mut self, stream: &[u8], chunk_size: usize) {
        for chunk in stream.chunks(chunk_size) {
            self.feed(chunk);
        }
    }

    fn feed_in_sizes(&mut self, stream: &[u8], sizes: &[usize]) {
        let mut position = 0;
        for size in sizes {
            if position == stream.len() {
                break;
            }
            let end = position.saturating_add(*size).min(stream.len());
            self.feed(&stream[position..end]);
            position = end;
        }
        if position < stream.len() {
            self.feed(&stream[position..]);
        }
    }

    fn assert_header_once(&self) {
        assert_eq!(
            self.header_count, 1,
            "header must be delivered exactly once"
        );
    }

    fn assert_contiguous_spans(&self) {
        let mut expected = self.header.as_ref().map_or(0, |header| header.offset);
        for (offset, len) in &self.spans {
            assert_eq!(*offset, expected, "span does not start at previous end");
            expected += *len as u64;
        }
        assert_eq!(expected, self.receiver.position());
    }
}

fn encoded_prefix_len(header: &FileHeader) -> usize {
    let prefix = encode_stream_prefix(header).expect("test header encodes");
    u32::from_be_bytes(prefix[..4].try_into().expect("four prefix bytes")) as usize
}

fn vector_dir() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("tests/vectors/file")
}

#[test]
fn complete_stream_round_trips_at_every_chunk_size() {
    let header = hello_header(0);
    let stream = complete_stream(&header, DATA);

    for chunk_size in [1, 2, 3, 7, 64, stream.len()] {
        let mut received = Received::new();
        received.feed_in_chunks(&stream, chunk_size);
        received.assert_header_once();
        received.assert_contiguous_spans();
        assert_eq!(received.header.as_ref(), Some(&header));
        assert_eq!(received.data, DATA);
        assert_eq!(received.spans.first().map(|(offset, _)| *offset), Some(0));
        assert_eq!(
            received
                .receiver
                .finish()
                .expect("complete stream finishes"),
            StreamEnd::Complete {
                header: header.clone()
            }
        );
    }
}

#[test]
fn header_and_data_can_arrive_in_one_chunk() {
    let header = hello_header(0);
    let stream = complete_stream(&header, DATA);
    let mut receiver = FileStreamReceiver::new();
    let fed = receiver.feed(&stream).expect("complete stream is valid");

    assert_eq!(fed.header, Some(header.clone()));
    let span = fed.data.expect("data follows the header in the same chunk");
    assert_eq!(span.offset, 0);
    assert_eq!(span.bytes, DATA);
    assert_eq!(receiver.position(), DATA.len() as u64);
    assert_eq!(receiver.finish().unwrap(), StreamEnd::Complete { header });
}

#[test]
fn resumed_stream_starts_at_header_offset() {
    let header = hello_header(5);
    let tail = &DATA[5..];
    let stream = complete_stream(&header, tail);
    let mut received = Received::new();
    received.feed_in_chunks(&stream, 2);
    received.assert_header_once();
    received.assert_contiguous_spans();
    assert_eq!(received.spans.first().map(|(offset, _)| *offset), Some(5));
    assert_eq!(received.data, tail);
    assert_eq!(
        received.receiver.finish().expect("resumed stream finishes"),
        StreamEnd::Complete { header }
    );
}

#[test]
fn early_fin_reports_truncated_position() {
    let header = hello_header(0);
    let stream = complete_stream(&header, &DATA[..4]);
    let mut received = Received::new();
    received.feed_in_chunks(&stream, 3);
    assert_eq!(
        received
            .receiver
            .finish()
            .expect("short stream is not an error"),
        StreamEnd::Truncated {
            header,
            position: 4,
        }
    );
}

#[test]
fn header_length_limit_is_checked_as_soon_as_prefix_arrives() {
    let mut receiver = FileStreamReceiver::new();
    assert!(matches!(
        receiver.feed(&(MAX_HEADER_BYTES as u32 + 1).to_be_bytes()),
        Err(FileError::HeaderLengthTooLarge(4097))
    ));
    assert!(matches!(
        receiver.feed(&[0]),
        Err(FileError::StreamFinished)
    ));
    assert!(matches!(receiver.finish(), Err(FileError::StreamFinished)));
}

#[test]
fn malformed_header_protobuf_is_rejected() {
    let mut framed = 1u32.to_be_bytes().to_vec();
    framed.push(0x08);
    let mut receiver = FileStreamReceiver::new();
    assert!(matches!(
        receiver.feed(&framed),
        Err(FileError::InvalidHeader(_))
    ));
    assert!(matches!(receiver.finish(), Err(FileError::StreamFinished)));
}

#[test]
fn header_limits_reject_oversized_and_unsafe_values() {
    let mut header = hello_header(0);

    header.size = MAX_FILE_BYTES + 1;
    assert!(matches!(
        validate_header(&header),
        Err(FileError::FileTooLarge {
            size,
            limit: MAX_FILE_BYTES,
        }) if size == MAX_FILE_BYTES + 1
    ));

    header = hello_header(0);
    header.offset = header.size + 1;
    assert!(matches!(
        validate_header(&header),
        Err(FileError::OffsetBeyondSize { offset, size }) if offset == 12 && size == 11
    ));

    header = hello_header(0);
    header.sha256.pop();
    assert!(matches!(
        validate_header(&header),
        Err(FileError::InvalidSha256Length(31))
    ));
}

#[test]
fn names_are_bounded_without_path_cleaning() {
    let too_long = "a".repeat(MAX_NAME_BYTES + 1);
    for name in ["", "a\0b", too_long.as_str()] {
        let header = FileHeader {
            transfer_id: TRANSFER_ID.to_owned(),
            name: name.to_string(),
            size: 0,
            offset: 0,
            sha256: sha_hello().to_vec(),
            mtime_us: MTIME_US,
            dest_dir: String::new(),
        };
        assert!(matches!(
            validate_header(&header),
            Err(FileError::InvalidName(_))
        ));
    }

    validate_header(&FileHeader {
        transfer_id: TRANSFER_ID.to_owned(),
        name: "a".repeat(MAX_NAME_BYTES),
        size: 0,
        offset: 0,
        sha256: sha_hello().to_vec(),
        mtime_us: MTIME_US,
        dest_dir: String::new(),
    })
    .expect("the name limit is inclusive");

    for name in ["a\\b.txt", "../x"] {
        build_header(TRANSFER_ID, name, 0, 0, sha_hello(), MTIME_US)
            .expect("path cleaning belongs to the receiver");
    }
}

#[test]
fn transfer_ids_must_be_lowercase_hex_of_exact_length() {
    for invalid in [
        "00112233445566778899AABBCCDDEEFF",
        "00112233445566778899aabbccddeef",
        "00112233445566778899aabbccddeeg",
        "",
    ] {
        assert!(matches!(
            validate_transfer_id(invalid),
            Err(FileError::InvalidTransferId)
        ));
    }
    validate_transfer_id(TRANSFER_ID).expect("fixed id is valid");

    let first = new_transfer_id().expect("random id is available");
    let second = new_transfer_id().expect("random id is available");
    validate_transfer_id(&first).expect("first random id is valid");
    validate_transfer_id(&second).expect("second random id is valid");
    assert_ne!(first, second);
}

#[test]
fn extra_data_fails_without_delivering_a_span_and_poisons_stream() {
    let header = hello_header(0);
    let mut stream = encode_stream_prefix(&header).expect("test header encodes");
    stream.extend_from_slice(DATA);
    stream.push(b'!');

    let mut receiver = FileStreamReceiver::new();
    assert!(matches!(
        receiver.feed(&stream),
        Err(FileError::DataExceedsSize {
            size: 11,
            received: 12,
        })
    ));
    assert!(receiver.header().is_none());
    assert!(matches!(
        receiver.feed(&[0]),
        Err(FileError::StreamFinished)
    ));
    assert!(matches!(receiver.finish(), Err(FileError::StreamFinished)));
}

#[test]
fn extra_data_in_later_chunk_is_rejected_atomically() {
    let header = hello_header(0);
    let prefix_len = 4 + encoded_prefix_len(&header);
    let prefix = encode_stream_prefix(&header).expect("test header encodes");
    let mut receiver = FileStreamReceiver::new();
    assert_eq!(prefix.len(), prefix_len);
    let fed = receiver.feed(&prefix).expect("prefix is valid");
    assert_eq!(fed.header, Some(header.clone()));
    assert!(fed.data.is_none());

    let mut payload_and_extra = DATA.to_vec();
    payload_and_extra.push(b'!');

    assert!(matches!(
        receiver.feed(&payload_and_extra),
        Err(FileError::DataExceedsSize {
            size: 11,
            received: 12,
        })
    ));
    assert!(receiver.header().is_none());
    assert_eq!(receiver.position(), 0);
    assert!(matches!(
        receiver.feed(&[0]),
        Err(FileError::StreamFinished)
    ));
}

#[test]
fn empty_and_header_truncated_streams_have_distinct_errors() {
    assert!(matches!(
        FileStreamReceiver::new().finish(),
        Err(FileError::EmptyStream)
    ));

    let mut prefix = FileStreamReceiver::new();
    prefix.feed(&[0, 0]).expect("two prefix bytes are valid");
    assert!(matches!(
        prefix.finish(),
        Err(FileError::HeaderTruncated {
            declared: 4,
            received: 2,
        })
    ));

    let header = hello_header(0);
    let declared = encoded_prefix_len(&header);
    let prefix = encode_stream_prefix(&header).expect("test header encodes");
    let half = 4 + declared / 2;
    let mut receiver = FileStreamReceiver::new();
    receiver
        .feed(&prefix[..half])
        .expect("partial header is valid");
    assert!(matches!(
        receiver.finish(),
        Err(FileError::HeaderTruncated {
            declared: actual_declared,
            received,
        }) if actual_declared == declared && received == declared / 2
    ));
}

#[test]
fn empty_chunk_is_a_noop_before_and_after_header() {
    let header = hello_header(0);
    let stream = complete_stream(&header, DATA);
    let mut receiver = FileStreamReceiver::new();
    let fed = receiver.feed(&[]).expect("empty chunk is valid");
    assert!(fed.header.is_none() && fed.data.is_none());

    let prefix_len = encoded_prefix_len(&header) + 4;
    receiver
        .feed(&stream[..prefix_len])
        .expect("header arrives");
    let fed = receiver.feed(&[]).expect("empty chunk is valid");
    assert!(fed.header.is_none() && fed.data.is_none());
    assert_eq!(receiver.position(), 0);
}

#[test]
fn resume_check_table() {
    let partial = |received| PartialState {
        transfer_id: TRANSFER_ID.to_owned(),
        size: 11,
        sha256: sha_hello(),
        mtime_us: MTIME_US,
        received,
    };

    let fresh = hello_header(0);
    assert_eq!(resume_check(&fresh, None).unwrap(), ResumePlan::Fresh);
    assert_eq!(
        resume_check(&fresh, Some(&partial(11))).unwrap(),
        ResumePlan::Fresh
    );

    let resumed = hello_header(5);
    assert!(matches!(
        resume_check(&resumed, None),
        Err(FileError::ResumeMismatch { have: 0 })
    ));

    let mut wrong_sha = partial(5);
    wrong_sha.sha256[0] ^= 0xff;
    assert!(matches!(
        resume_check(&resumed, Some(&wrong_sha)),
        Err(FileError::ResumeMismatch { have: 0 })
    ));

    assert!(matches!(
        resume_check(&resumed, Some(&partial(3))),
        Err(FileError::ResumeMismatch { have: 3 })
    ));
    assert_eq!(
        resume_check(&resumed, Some(&partial(5))).unwrap(),
        ResumePlan::Continue { truncate_to: 5 }
    );
    assert_eq!(
        resume_check(&resumed, Some(&partial(8))).unwrap(),
        ResumePlan::Continue { truncate_to: 5 }
    );
}

#[derive(Debug, Deserialize)]
struct FileVector {
    feature: String,
    encoding: String,
    transfer_id: String,
    name: String,
    size: u64,
    offset: u64,
    sha256: String,
    mtime_us: u64,
    payload_hex: String,
    binary: String,
    description: String,
}

fn decode_hex(value: &str) -> Vec<u8> {
    assert_eq!(value.len() % 2, 0);
    value
        .as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            let digit = |byte: u8| match byte {
                b'0'..=b'9' => byte - b'0',
                b'a'..=b'f' => byte - b'a' + 10,
                b'A'..=b'F' => byte - b'A' + 10,
                _ => panic!("invalid hex digit"),
            };
            (digit(pair[0]) << 4) | digit(pair[1])
        })
        .collect()
}

#[test]
fn fixed_vectors_match_json_and_binary() {
    for vector_file in ["stream_hello.json", "header_resume.json"] {
        let json = fs::read(vector_dir().join(vector_file)).expect("vector json");
        let vector: FileVector = serde_json::from_slice(&json).expect("valid vector json");
        assert_eq!(vector.feature, "FILE_TRANSFER");
        assert_eq!(vector.encoding, "quic-stream");
        assert!(!vector.description.is_empty());

        let sha: [u8; 32] = decode_hex(&vector.sha256)
            .try_into()
            .expect("vector SHA is 32 bytes");
        let header = build_header(
            vector.transfer_id.as_str(),
            vector.name.as_str(),
            vector.size,
            vector.offset,
            sha,
            vector.mtime_us,
        )
        .expect("vector header is valid");
        let payload = decode_hex(&vector.payload_hex);
        let expected = complete_stream(&header, &payload);
        let binary = fs::read(vector_dir().join(&vector.binary)).expect("vector binary");
        assert_eq!(expected, binary, "re-encoded vector is byte-identical");

        let mut received = Received::new();
        received.feed_in_chunks(&binary, 3);
        received.assert_header_once();
        received.assert_contiguous_spans();
        let parsed_header = received.header.expect("vector contains a header");
        assert_eq!(parsed_header.transfer_id, vector.transfer_id);
        assert_eq!(parsed_header.name, vector.name);
        assert_eq!(parsed_header.size, vector.size);
        assert_eq!(parsed_header.offset, vector.offset);
        assert_eq!(parsed_header.sha256, sha);
        assert_eq!(parsed_header.mtime_us, vector.mtime_us);
        assert_eq!(received.data, payload);
        assert_eq!(
            received.receiver.position(),
            vector.offset + payload.len() as u64
        );
        if vector.offset + payload.len() as u64 == vector.size {
            assert!(matches!(
                received.receiver.finish(),
                Ok(StreamEnd::Complete { .. })
            ));
        } else {
            assert!(matches!(
                received.receiver.finish(),
                Ok(StreamEnd::Truncated { .. })
            ));
        }
    }
}

proptest! {
    #[test]
    fn arbitrary_data_and_splits_round_trip(
        data in prop::collection::vec(any::<u8>(), 0..4096),
        sizes in prop::collection::vec(1usize..=8192, 0..64),
    ) {
        let sha: [u8; 32] = Sha256::digest(&data).into();
        let header = build_header(
            TRANSFER_ID,
            "random.bin",
            data.len() as u64,
            0,
            sha,
            MTIME_US,
        ).expect("generated header is valid");
        let stream = complete_stream(&header, &data);

        let mut received = Received::new();
        received.feed_in_sizes(&stream, &sizes);
        received.assert_header_once();
        received.assert_contiguous_spans();
        prop_assert_eq!(received.header, Some(header.clone()));
        prop_assert_eq!(received.data, data);
        prop_assert_eq!(
            received.receiver.finish().expect("complete stream finishes"),
            StreamEnd::Complete { header }
        );
    }
}

#[test]
fn destination_directory_is_bounded_without_host_path_policy() {
    let mut header = hello_header(0);
    for destination in ["", "relative", "/Users/example/Downloads"] {
        header.dest_dir = destination.into();
        validate_header(&header).unwrap();
        let prefix = encode_stream_prefix(&header).unwrap();
        let mut receiver = FileStreamReceiver::new();
        assert_eq!(receiver.feed(&prefix).unwrap().header, Some(header.clone()));
    }
    header.dest_dir = "界".repeat(682) + "ab";
    validate_header(&header).unwrap(); // 2048 UTF-8 bytes, inclusive.
    header.dest_dir.push('x');
    assert!(matches!(
        validate_header(&header),
        Err(FileError::InvalidDestDir(_))
    ));
    header.dest_dir = "a\0b".into();
    assert!(matches!(
        validate_header(&header),
        Err(FileError::InvalidDestDir(_))
    ));
}
