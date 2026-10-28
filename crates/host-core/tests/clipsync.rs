use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_core::platform::{self, PlatformError};
use host_core::session::clipboard::{
    apply_inbound, from_platform, pump_local_changes, quic_stream_bytes, read_stream, sha256,
    tcp_records, to_platform, write_stream, ClipDirections, ClipboardPort, ClipboardSync,
    InboundError, OutgoingClip, RejectReason, SkipReason, CLIPBOARD_STREAM_TYPE,
};
use prost::Message;
use rsp_core::clipboard::{
    encode_stream_prefix, ClipboardError, ClipboardItem, ClipboardKind, TcpReassembler,
    MAX_PNG_BYTES, MAX_TEXT_BYTES,
};
use rsp_core::features::{CLIPBOARD_IMAGE, CLIPBOARD_TEXT};
use rsp_core::pb::clipboard_header::Kind;
use rsp_core::pb::ClipboardHeader;
use tokio::io::{AsyncReadExt, AsyncWriteExt};
use tokio::sync::{mpsc, watch};

const T: u64 = CLIPBOARD_TEXT;
const I: u64 = CLIPBOARD_IMAGE;

fn directions() -> ClipDirections {
    ClipDirections {
        to_phone: true,
        from_phone: true,
    }
}

#[test]
fn directions_convert_from_config_and_can_be_updated() {
    let config = host_core::config::ClipboardConfig {
        to_phone: false,
        from_phone: true,
    };
    assert_eq!(
        ClipDirections::from(&config),
        ClipDirections {
            to_phone: false,
            from_phone: true,
        }
    );

    let mut sync = ClipboardSync::new(directions());
    sync.set_directions(ClipDirections::from(&config));
    assert_eq!(
        sync.outbound(Some(&text_platform("disabled")), Some(T)),
        Err(SkipReason::Disabled)
    );
}

fn platform_item(kind: Kind, data: impl Into<Vec<u8>>) -> platform::ClipboardItem {
    let data = data.into();
    platform::ClipboardItem {
        kind,
        sha256: sha256(&data),
        data,
    }
}

fn text_platform(text: &str) -> platform::ClipboardItem {
    platform_item(Kind::Text, text.as_bytes())
}

fn text_item(text: &str) -> ClipboardItem {
    ClipboardItem::new(ClipboardKind::Text, text.as_bytes().to_vec()).unwrap()
}

fn png_item(data: Vec<u8>) -> ClipboardItem {
    ClipboardItem::new(ClipboardKind::Png, data).unwrap()
}

fn png_platform(data: Vec<u8>) -> platform::ClipboardItem {
    platform_item(Kind::Png, data)
}

fn as_clipboard_error(error: host_core::session::clipboard::ClipStreamError) -> ClipboardError {
    match error {
        host_core::session::clipboard::ClipStreamError::Clipboard(error) => error,
        other => panic!("expected clipboard error, got {other:?}"),
    }
}

#[test]
fn outbound_text_recomputes_sha_and_keeps_kind() {
    let mut sync = ClipboardSync::new(directions());
    let item = platform_item(Kind::Text, "hello 世界".as_bytes());
    let outgoing = sync.outbound(Some(&item), Some(T)).unwrap();
    assert_eq!(outgoing.item.kind, ClipboardKind::Text);
    assert_eq!(outgoing.item.data.as_ref(), "hello 世界".as_bytes());
    assert_eq!(outgoing.sha256, sha256("hello 世界".as_bytes()));
    assert_eq!(outgoing.sha256, sha256(&item.data));
}

#[test]
fn outbound_checks_disable_controller_empty_and_kind_in_order() {
    let mut disabled = ClipboardSync::new(ClipDirections {
        to_phone: false,
        from_phone: true,
    });
    assert_eq!(disabled.outbound(None, None), Err(SkipReason::Disabled));

    let mut sync = ClipboardSync::new(directions());
    assert_eq!(sync.outbound(None, None), Err(SkipReason::NoController));
    assert_eq!(sync.outbound(None, Some(T)), Err(SkipReason::Empty));

    let unspecified = platform_item(Kind::Unspecified, b"x");
    assert_eq!(
        sync.outbound(Some(&unspecified), Some(T)),
        Err(SkipReason::InvalidKind)
    );
}

#[test]
fn outbound_requires_matching_app_feature() {
    let mut sync = ClipboardSync::new(directions());
    let png = png_platform(b"png".to_vec());
    assert_eq!(
        sync.outbound(Some(&png), Some(T)),
        Err(SkipReason::NotSupportedByApp)
    );
    assert!(sync.outbound(Some(&png), Some(T | I)).is_ok());
}

#[test]
fn outbound_enforces_text_and_png_limits_and_utf8() {
    let mut sync = ClipboardSync::new(directions());
    let text_at_limit = platform_item(Kind::Text, vec![b'a'; MAX_TEXT_BYTES]);
    assert!(sync.outbound(Some(&text_at_limit), Some(T)).is_ok());
    let text_over_limit = platform_item(Kind::Text, vec![b'a'; MAX_TEXT_BYTES + 1]);
    assert_eq!(
        sync.outbound(Some(&text_over_limit), Some(T)),
        Err(SkipReason::TooLarge)
    );

    let mut png_sync = ClipboardSync::new(directions());
    let png_at_limit = png_platform(vec![0x42; MAX_PNG_BYTES]);
    assert!(png_sync.outbound(Some(&png_at_limit), Some(I)).is_ok());
    let png_over_limit = png_platform(vec![0x42; MAX_PNG_BYTES + 1]);
    assert_eq!(
        png_sync.outbound(Some(&png_over_limit), Some(I)),
        Err(SkipReason::TooLarge)
    );

    let mut text_sync = ClipboardSync::new(directions());
    let invalid = platform_item(Kind::Text, vec![0xff, 0xfe]);
    assert_eq!(
        text_sync.outbound(Some(&invalid), Some(T)),
        Err(SkipReason::InvalidText)
    );
}

#[test]
fn outbound_deduplicates_until_controller_changes() {
    let mut sync = ClipboardSync::new(directions());
    let item = text_platform("same");
    assert!(sync.outbound(Some(&item), Some(T)).is_ok());
    assert_eq!(
        sync.outbound(Some(&item), Some(T)),
        Err(SkipReason::Duplicate)
    );
    sync.on_controller_changed();
    assert!(sync.outbound(Some(&item), Some(T)).is_ok());
}

#[test]
fn inbound_sets_echo_guard_and_echo_is_suppressed_once() {
    let mut sync = ClipboardSync::new(directions());
    let x = text_item("from phone X");
    let expected_sha = sha256(x.data.as_ref());

    let platform_x = sync.inbound(true, x.clone()).unwrap();
    assert_eq!(platform_x.kind, Kind::Text);
    assert_eq!(platform_x.data, x.data.as_ref());
    assert_eq!(platform_x.sha256, expected_sha);

    let platform_x = platform_item(Kind::Text, x.data.to_vec());
    assert_eq!(
        sync.outbound(Some(&platform_x), Some(T)),
        Err(SkipReason::Echo)
    );
    assert_eq!(
        sync.outbound(Some(&platform_x), Some(T)),
        Err(SkipReason::Duplicate)
    );

    let y = text_platform("local Y");
    assert!(sync.outbound(Some(&y), Some(T)).is_ok());
    assert!(sync.outbound(Some(&platform_x), Some(T)).is_ok());
}

#[test]
fn a_different_local_copy_clears_the_echo_guard() {
    let mut sync = ClipboardSync::new(directions());
    let x = text_item("phone X");
    let platform_x = platform_item(Kind::Text, x.data.to_vec());
    sync.inbound(true, x).unwrap();

    let y = text_platform("local Y");
    assert!(sync.outbound(Some(&y), Some(T)).is_ok());
    assert!(sync.outbound(Some(&platform_x), Some(T)).is_ok());
}

#[test]
fn inbound_rejects_viewers_and_disabled_direction() {
    let mut sync = ClipboardSync::new(directions());
    assert!(matches!(
        sync.inbound(false, text_item("viewer")),
        Err(RejectReason::Viewer)
    ));

    let mut disabled = ClipboardSync::new(ClipDirections {
        to_phone: true,
        from_phone: false,
    });
    assert!(matches!(
        disabled.inbound(true, text_item("disabled")),
        Err(RejectReason::Disabled)
    ));
}

#[tokio::test]
async fn quic_stream_round_trips_utf8_and_png() {
    let mut png = vec![0_u8; 200 * 1024];
    getrandom::fill(&mut png).unwrap();
    for item in [text_item("中文 + emoji 😀🚀"), png_item(png)] {
        let (mut writer, mut reader) = tokio::io::duplex(16 * 1024);
        let expected = item.clone();
        let write = tokio::spawn(async move { write_stream(&mut writer, &item).await });

        let mut stream_type = [0_u8; 1];
        reader.read_exact(&mut stream_type).await.unwrap();
        assert_eq!(stream_type[0], CLIPBOARD_STREAM_TYPE);
        let received = read_stream(&mut reader).await.unwrap();
        assert_eq!(received, expected);
        write.await.unwrap().unwrap();
    }
}

#[tokio::test]
async fn read_stream_reports_corruption_truncation_and_oversize_promptly() {
    let item = text_item("tamper me");
    let mut bytes = quic_stream_bytes(&item).unwrap();
    let last = bytes.len() - 1;
    bytes[last] ^= 0x80;
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    let send = bytes[1..].to_vec();
    let write = tokio::spawn(async move {
        writer.write_all(&send).await.unwrap();
        writer.shutdown().await.unwrap();
    });
    let error = read_stream(&mut reader).await.unwrap_err();
    assert!(matches!(
        as_clipboard_error(error),
        ClipboardError::Sha256Mismatch
    ));
    write.await.unwrap();

    let (mut writer, mut reader) = tokio::io::duplex(1024);
    let complete = quic_stream_bytes(&item).unwrap();
    let truncated = complete[1..complete.len() - 2].to_vec();
    let write = tokio::spawn(async move {
        writer.write_all(&truncated).await.unwrap();
        writer.shutdown().await.unwrap();
    });
    let error = read_stream(&mut reader).await.unwrap_err();
    assert!(matches!(
        as_clipboard_error(error),
        ClipboardError::SizeMismatch { .. }
    ));
    write.await.unwrap();

    let header = ClipboardHeader {
        kind: Kind::Png as i32,
        size: 20 * 1024 * 1024,
        sha256: vec![0; 32],
    }
    .encode_to_vec();
    let mut prefix = (header.len() as u32).to_be_bytes().to_vec();
    prefix.extend_from_slice(&header);
    let (mut writer, mut reader) = tokio::io::duplex(1024);
    writer.write_all(&prefix).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(1), read_stream(&mut reader)).await;
    let error = result
        .expect("oversize header must fail before EOF")
        .unwrap_err();
    assert!(matches!(
        as_clipboard_error(error),
        ClipboardError::DataTooLarge { .. }
    ));
}

#[test]
fn tcp_records_reassemble_and_item_ids_start_at_one() {
    let item = png_item((0..4096).map(|i| (i % 239) as u8).collect());
    let records = tcp_records(7, &item, 1000).unwrap();
    assert!(records.len() > 1);
    let mut reassembler = TcpReassembler::new();
    let mut received = None;
    for record in records {
        if let Some(item) = reassembler.feed(&record).unwrap() {
            received = Some(item);
        }
    }
    assert_eq!(received, Some(item));

    let mut sync = ClipboardSync::new(directions());
    assert_eq!(sync.next_tcp_item_id(), 1);
    assert_eq!(sync.next_tcp_item_id(), 2);
    assert_eq!(sync.next_tcp_item_id(), 3);
}

#[derive(Default)]
struct FakePort {
    current: Mutex<Option<platform::ClipboardItem>>,
    gets: AtomicUsize,
    sets: AtomicUsize,
    fail_set: AtomicBool,
    set_items: Mutex<Vec<platform::ClipboardItem>>,
}

impl ClipboardPort for FakePort {
    fn get(&self) -> Option<platform::ClipboardItem> {
        self.gets.fetch_add(1, Ordering::Relaxed);
        self.current.lock().unwrap().clone()
    }

    fn set(&self, item: platform::ClipboardItem) -> Result<(), PlatformError> {
        self.sets.fetch_add(1, Ordering::Relaxed);
        if self.fail_set.load(Ordering::Relaxed) {
            return Err(PlatformError::Platform("set failed".into()));
        }
        self.set_items.lock().unwrap().push(item);
        Ok(())
    }
}

#[tokio::test]
async fn pump_coalesces_notifications_respects_controller_and_exits_on_close() {
    let port = Arc::new(FakePort {
        current: Mutex::new(Some(text_platform("pumped"))),
        ..FakePort::default()
    });
    let (change_tx, change_rx) = mpsc::channel(8);
    let (controller_tx, controller_rx) = watch::channel(Some(T));
    let sync = Arc::new(Mutex::new(ClipboardSync::new(directions())));
    let (out_tx, mut out_rx) = mpsc::channel(4);

    let task = tokio::spawn(pump_local_changes(
        port.clone(),
        change_rx,
        sync.clone(),
        controller_rx,
        out_tx,
    ));

    for _ in 0..3 {
        change_tx.send(()).await.unwrap();
    }
    let first = tokio::time::timeout(Duration::from_secs(1), out_rx.recv())
        .await
        .unwrap()
        .unwrap();
    assert_eq!(first.item.data.as_ref(), b"pumped");
    assert_eq!(port.gets.load(Ordering::Relaxed), 1);
    assert!(out_rx.try_recv().is_err());

    controller_tx.send(None).unwrap();
    change_tx.send(()).await.unwrap();
    assert!(
        tokio::time::timeout(Duration::from_millis(100), out_rx.recv())
            .await
            .is_err()
    );

    drop(change_tx);
    tokio::time::timeout(Duration::from_secs(1), task)
        .await
        .unwrap()
        .unwrap();
}

#[test]
fn apply_inbound_writes_controller_items_and_rejects_viewers() {
    let port = FakePort::default();
    let sync = Mutex::new(ClipboardSync::new(directions()));
    let item = text_item("from controller");
    apply_inbound(&port, &sync, true, item.clone()).unwrap();

    assert_eq!(port.sets.load(Ordering::Relaxed), 1);
    let set_items = port.set_items.lock().unwrap();
    assert_eq!(set_items.len(), 1);
    assert_eq!(set_items[0].kind, Kind::Text);
    assert_eq!(set_items[0].data, item.data.as_ref());
    assert_eq!(set_items[0].sha256, sha256(item.data.as_ref()));
    drop(set_items);

    let error = apply_inbound(&port, &sync, false, text_item("viewer")).unwrap_err();
    assert!(matches!(
        error,
        InboundError::Rejected(RejectReason::Viewer)
    ));
    assert_eq!(port.sets.load(Ordering::Relaxed), 1);

    port.fail_set.store(true, Ordering::Relaxed);
    let error = apply_inbound(&port, &sync, true, text_item("platform failure")).unwrap_err();
    assert!(matches!(error, InboundError::Platform(..)));
    assert_eq!(port.sets.load(Ordering::Relaxed), 2);
}

#[test]
fn platform_conversion_preserves_kind_and_data() {
    for item in [
        text_item("round trip 文本"),
        png_item(vec![0, 1, 2, 3, 0xff]),
    ] {
        let platform_item = to_platform(&item);
        assert_eq!(platform_item.sha256, sha256(item.data.as_ref()));
        let back = from_platform(&platform_item).unwrap();
        assert_eq!(back, item);
    }

    let unspecified = platform_item(Kind::Unspecified, b"x");
    assert_eq!(from_platform(&unspecified), Err(SkipReason::InvalidKind));
}

#[test]
fn encoded_bytes_include_type_and_prefix() {
    let item = text_item("prefix");
    let encoded = quic_stream_bytes(&item).unwrap();
    assert_eq!(encoded[0], CLIPBOARD_STREAM_TYPE);
    let mut expected = encode_stream_prefix(&item).unwrap();
    expected.extend_from_slice(&item.data);
    assert_eq!(&encoded[1..], expected.as_slice());
}

#[test]
fn outgoing_clip_is_comparable_and_constructible() {
    let item = text_item("x");
    let outgoing = OutgoingClip {
        sha256: sha256(item.data.as_ref()),
        item,
    };
    assert_eq!(outgoing, outgoing.clone());
}
