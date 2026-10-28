use std::collections::VecDeque;
use std::future::Future;

use prost::Message;
use rsp_client::clipboard::{
    receive, send, text_item, ClipboardClientError, ClipboardInbox, ClipboardPolicy, RawSink,
    RawSource,
};
use rsp_core::clipboard::{
    encode_stream, ClipboardError, ClipboardItem, ClipboardKind, MAX_PNG_BYTES,
};
use rsp_core::features::{CLIPBOARD_IMAGE, CLIPBOARD_TEXT};
use rsp_core::pb;
use rsp_transport::ConnError;

const WRITE_CHUNK_BYTES: usize = 64 << 10;

#[derive(Default)]
struct FakeSink {
    writes: Vec<Vec<u8>>,
    finishes: usize,
}

impl RawSink for FakeSink {
    fn write_raw(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.writes.push(bytes.to_vec());
        async { Ok(()) }
    }

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send {
        self.finishes += 1;
        async { Ok(()) }
    }
}

struct FakeSource {
    chunks: VecDeque<Vec<u8>>,
    fail_after_reads: Option<usize>,
    reads: usize,
}

impl FakeSource {
    fn new(chunks: Vec<Vec<u8>>) -> Self {
        Self {
            chunks: chunks.into(),
            fail_after_reads: None,
            reads: 0,
        }
    }

    fn chunked(data: &[u8], sizes: &[usize]) -> Self {
        let mut chunks = Vec::new();
        let mut offset = 0;
        let mut index = 0;
        while offset < data.len() {
            let size = sizes[index % sizes.len()];
            let end = (offset + size).min(data.len());
            chunks.push(data[offset..end].to_vec());
            offset = end;
            index += 1;
        }
        Self::new(chunks)
    }

    fn failing_after(data: &[u8], chunk_size: usize, successful_reads: usize) -> Self {
        let chunks = data
            .chunks(chunk_size)
            .map(|chunk| chunk.to_vec())
            .collect();
        Self {
            chunks,
            fail_after_reads: Some(successful_reads),
            reads: 0,
        }
    }
}

impl RawSource for FakeSource {
    fn read_raw(
        &mut self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<Option<usize>, ConnError>> + Send {
        self.reads += 1;
        let result = match self.fail_after_reads {
            Some(limit) if self.reads > limit => Err(ConnError::Closed),
            _ => match self.chunks.pop_front() {
                Some(chunk) => {
                    assert!(chunk.len() <= buf.len(), "fake chunk exceeds read buffer");
                    buf[..chunk.len()].copy_from_slice(&chunk);
                    Ok(Some(chunk.len()))
                }
                None => Ok(None),
            },
        };
        async move { result }
    }
}

fn controller_policy() -> ClipboardPolicy {
    ClipboardPolicy {
        peer_features: CLIPBOARD_TEXT | CLIPBOARD_IMAGE,
        role: pb::Role::Controller,
        to_phone: true,
        from_phone: true,
    }
}

fn png_item(size: usize) -> ClipboardItem {
    ClipboardItem::new(ClipboardKind::Png, vec![0x5a; size]).expect("PNG item within limit")
}

fn stream_with_raw_header(kind: ClipboardKind, sha256: [u8; 32], data: &[u8]) -> Vec<u8> {
    let header = pb::ClipboardHeader {
        kind: pb::clipboard_header::Kind::from(kind) as i32,
        size: data.len() as u64,
        sha256: sha256.to_vec(),
    }
    .encode_to_vec();
    let mut stream = Vec::with_capacity(4 + header.len() + data.len());
    stream.extend_from_slice(&(header.len() as u32).to_be_bytes());
    stream.extend_from_slice(&header);
    stream.extend_from_slice(data);
    stream
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, data);
    digest.as_ref().try_into().expect("SHA-256 is 32 bytes")
}

#[tokio::test]
async fn sends_text_as_exact_stream_and_finishes_once() {
    let item = text_item("你好 Remote Screen 👋").expect("valid text item");
    let expected = encode_stream(&item).expect("encodable item");
    let mut sink = FakeSink::default();

    send(&mut sink, &item, &controller_policy())
        .await
        .expect("text send");

    assert_eq!(sink.writes.concat(), expected);
    assert_eq!(sink.finishes, 1);
}

#[tokio::test]
async fn sends_large_png_in_bounded_chunks() {
    let item = png_item(3 << 20);
    let expected = encode_stream(&item).expect("encodable item");
    let mut sink = FakeSink::default();

    send(&mut sink, &item, &controller_policy())
        .await
        .expect("PNG send");

    assert!(sink.writes.len() > 1);
    assert!(sink
        .writes
        .iter()
        .all(|write| write.len() <= WRITE_CHUNK_BYTES));
    assert_eq!(sink.writes.concat(), expected);
    assert_eq!(sink.finishes, 1);
}

#[tokio::test]
async fn rejects_send_before_writing_any_bytes() {
    let item = png_item(1024);
    let mut viewer_policy = controller_policy();
    viewer_policy.role = pb::Role::Viewer;
    let mut viewer_sink = FakeSink::default();
    let error = send(&mut viewer_sink, &item, &viewer_policy)
        .await
        .expect_err("viewer cannot send");
    assert!(matches!(error, ClipboardClientError::NotController));
    assert!(viewer_sink.writes.is_empty());
    assert_eq!(viewer_sink.finishes, 0);

    let mut disabled_policy = controller_policy();
    disabled_policy.from_phone = false;
    let mut disabled_sink = FakeSink::default();
    let error = send(&mut disabled_sink, &item, &disabled_policy)
        .await
        .expect_err("from_phone disabled");
    assert!(matches!(error, ClipboardClientError::Disabled));
    assert!(disabled_sink.writes.is_empty());
    assert_eq!(disabled_sink.finishes, 0);

    let mut text_only_policy = controller_policy();
    text_only_policy.peer_features = CLIPBOARD_TEXT;
    let mut unsupported_sink = FakeSink::default();
    let error = send(&mut unsupported_sink, &item, &text_only_policy)
        .await
        .expect_err("PNG unsupported");
    assert!(matches!(
        error,
        ClipboardClientError::FeatureUnsupported(ClipboardKind::Png)
    ));
    assert!(unsupported_sink.writes.is_empty());
    assert_eq!(unsupported_sink.finishes, 0);
}

#[tokio::test]
async fn receives_item_split_across_arbitrary_chunks() {
    let text = "clipboard stream split test ".repeat(400);
    let item = text_item(&text).expect("valid text item");
    let stream = encode_stream(&item).expect("encodable item");
    let mut source = FakeSource::chunked(&stream, &[1, 7, 4096]);

    let received = receive(&mut source, &controller_policy())
        .await
        .expect("receive text");

    assert_eq!(received, item);
    assert!(source.reads > 3);
}

#[tokio::test]
async fn rejects_oversized_header_without_reading_body() {
    let declared_size = 11 << 20;
    let header = pb::ClipboardHeader {
        kind: pb::clipboard_header::Kind::Png as i32,
        size: declared_size,
        sha256: vec![0; 32],
    }
    .encode_to_vec();
    let mut stream = Vec::with_capacity(4 + header.len());
    stream.extend_from_slice(&(header.len() as u32).to_be_bytes());
    stream.extend_from_slice(&header);
    let mut source = FakeSource::chunked(&stream, &[4096]);

    let error = receive(&mut source, &controller_policy())
        .await
        .expect_err("declared size exceeds limit");

    match error {
        ClipboardClientError::Codec(ClipboardError::DataTooLarge {
            kind: ClipboardKind::Png,
            size,
            limit,
        }) => {
            assert_eq!(size, declared_size);
            assert_eq!(limit, MAX_PNG_BYTES);
        }
        other => panic!("unexpected error: {other:?}"),
    }
    assert!(
        source.reads < 10,
        "receiver kept reading after header error"
    );
}

#[tokio::test]
async fn reports_sha_mismatch_and_midstream_transport_error() {
    let data = vec![0x31; 9000];
    let mut tampered_sha = sha256(&data);
    tampered_sha[0] ^= 0x80;
    let stream = stream_with_raw_header(ClipboardKind::Png, tampered_sha, &data);
    let mut source = FakeSource::chunked(&stream, &[4096]);
    let error = receive(&mut source, &controller_policy())
        .await
        .expect_err("tampered SHA-256");
    assert!(matches!(
        error,
        ClipboardClientError::Codec(ClipboardError::Sha256Mismatch)
    ));

    let item = png_item(512);
    let stream = encode_stream(&item).expect("encodable item");
    let mut source = FakeSource::failing_after(&stream, 8, 2);
    let error = receive(&mut source, &controller_policy())
        .await
        .expect_err("transport closes midstream");
    assert!(matches!(
        error,
        ClipboardClientError::Transport(ConnError::Closed)
    ));
}

#[tokio::test]
async fn rejects_receive_before_reading_for_viewer_or_disabled() {
    let item = text_item("not read").expect("valid text item");
    let stream = encode_stream(&item).expect("encodable item");

    let mut viewer_policy = controller_policy();
    viewer_policy.role = pb::Role::Viewer;
    let mut viewer_source = FakeSource::chunked(&stream, &[1]);
    let error = receive(&mut viewer_source, &viewer_policy)
        .await
        .expect_err("viewer cannot receive");
    assert!(matches!(error, ClipboardClientError::NotController));
    assert_eq!(viewer_source.reads, 0);

    let mut disabled_policy = controller_policy();
    disabled_policy.to_phone = false;
    let mut disabled_source = FakeSource::chunked(&stream, &[1]);
    let error = receive(&mut disabled_source, &disabled_policy)
        .await
        .expect_err("to_phone disabled");
    assert!(matches!(error, ClipboardClientError::Disabled));
    assert_eq!(disabled_source.reads, 0);
}

#[tokio::test]
async fn inbox_keeps_only_latest_and_suppresses_duplicates_and_echoes() {
    let text_a = format!("first\nline\t{}", "a".repeat(120));
    let item_a = text_item(&text_a).expect("valid text item");
    let item_b = text_item("B").expect("valid text item");
    let item_c = text_item("C").expect("valid text item");
    let mut inbox = ClipboardInbox::new();

    let offer_a = inbox
        .on_received(item_a.clone())
        .expect("first item produces an offer");
    let expected_preview: String = text_a
        .chars()
        .take(100)
        .map(|character| match character {
            '\n' | '\r' | '\t' => ' ',
            other => other,
        })
        .collect();
    assert_eq!(offer_a.kind, ClipboardKind::Text);
    assert_eq!(offer_a.size, text_a.len());
    assert_eq!(offer_a.sha256, sha256(text_a.as_bytes()));
    assert_eq!(offer_a.preview.as_deref(), Some(expected_preview.as_str()));
    assert_eq!(
        offer_a.preview.as_ref().map(|text| text.chars().count()),
        Some(100)
    );
    assert!(!offer_a
        .preview
        .as_deref()
        .unwrap_or("")
        .contains(['\n', '\t']));
    assert_eq!(inbox.latest(), Some(&offer_a));

    assert!(
        inbox.on_received(item_a).is_none(),
        "duplicate must be suppressed"
    );

    let offer_b = inbox
        .on_received(item_b.clone())
        .expect("new item replaces the latest");
    assert_eq!(inbox.latest(), Some(&offer_b));
    inbox.on_sent(&item_c);
    assert!(
        inbox.on_received(item_c).is_none(),
        "sent echo must be suppressed"
    );

    assert_eq!(inbox.take(), Some(item_b));
    assert!(inbox.latest().is_none());

    let png_offer = inbox
        .on_received(png_item(32))
        .expect("PNG produces an offer");
    assert_eq!(png_offer.kind, ClipboardKind::Png);
    assert!(png_offer.preview.is_none());
}

#[tokio::test]
async fn errors_never_render_clipboard_content() {
    const MARKER: &str = "SECRET-MARKER";

    let item = text_item(MARKER).expect("valid text item");
    let mut sink = FakeSink::default();
    let mut policy = controller_policy();
    policy.peer_features = 0;
    let send_error = send(&mut sink, &item, &policy)
        .await
        .expect_err("feature unsupported");

    let mut invalid_text = MARKER.as_bytes().to_vec();
    invalid_text.push(0xff);
    let stream = stream_with_raw_header(ClipboardKind::Text, sha256(&invalid_text), &invalid_text);
    let mut source = FakeSource::chunked(&stream, &[3]);
    let receive_error = receive(&mut source, &controller_policy())
        .await
        .expect_err("invalid UTF-8");

    for error in [send_error, receive_error] {
        let display = format!("{error}");
        let debug = format!("{error:?}");
        assert!(
            !display.contains(MARKER),
            "Display leaked clipboard content"
        );
        assert!(!debug.contains(MARKER), "Debug leaked clipboard content");
    }

    let offer = ClipboardInbox::new()
        .on_received(item)
        .expect("offer for UI");
    assert!(!format!("{offer:?}").contains(MARKER));
}
