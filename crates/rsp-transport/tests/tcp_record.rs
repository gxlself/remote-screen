use bytes::Bytes;
use rsp_transport::mux::StreamKind;
use rsp_transport::tcp::{
    encode_tcp_record, MuxEvent, TcpMuxState, TcpRecordDecoder, TcpRecordHeader, VirtualStreamId,
    MAX_CONTROL_RECORD, MAX_DATA_RECORD,
};

#[test]
fn incremental_records_and_empty_payloads() {
    let first = encode_tcp_record(0, b"control").unwrap();
    let empty = encode_tcp_record(2, b"").unwrap();
    let third = encode_tcp_record(0x10, b"video").unwrap();
    let wire = [first.as_ref(), empty.as_ref(), third.as_ref()].concat();
    let mut decoder = TcpRecordDecoder::default();
    let mut records = Vec::new();
    for byte in wire {
        records.extend(decoder.push(&[byte]).unwrap());
    }
    assert_eq!(
        records,
        vec![
            (0, Bytes::from_static(b"control")),
            (2, Bytes::new()),
            (0x10, Bytes::from_static(b"video"))
        ]
    );
    decoder.finish().unwrap();
}

#[test]
fn channel_boundaries_and_oversize_headers() {
    for (channel, limit) in [
        (0, MAX_CONTROL_RECORD),
        (3, MAX_DATA_RECORD),
        (0xff, MAX_DATA_RECORD),
    ] {
        let data = vec![0xaa; limit];
        let encoded = encode_tcp_record(channel, &data).unwrap();
        let mut decoder = TcpRecordDecoder::default();
        let records = decoder.push(&encoded).unwrap();
        assert_eq!(records, vec![(channel, Bytes::from(data))]);
        decoder.finish().unwrap();
        assert!(encode_tcp_record(channel, &vec![0; limit + 1]).is_err());
        let mut header = vec![channel];
        header.extend_from_slice(&((limit + 1) as u32).to_be_bytes());
        let mut decoder = TcpRecordDecoder::default();
        assert!(decoder.push(&header).is_err());
        assert!(decoder.push(&[]).is_err());
        assert!(TcpRecordHeader::decode(&header).is_err());
    }
}

#[test]
fn truncated_at_every_position() {
    let wire = encode_tcp_record(3, b"12345").unwrap();
    for end in 1..wire.len() {
        let mut decoder = TcpRecordDecoder::default();
        assert!(decoder.push(&wire[..end]).unwrap().is_empty());
        assert!(decoder.finish().is_err(), "end={end}");
    }
    assert!(TcpRecordHeader::decode(&[0; 4]).is_err());
    let header = TcpRecordHeader {
        channel: 3,
        length: 5,
    };
    assert_eq!(
        TcpRecordHeader::decode(&header.encode().unwrap()).unwrap(),
        header
    );
}

fn clipboard(id: u32, last: bool, data: &[u8]) -> Bytes {
    let mut out = id.to_be_bytes().to_vec();
    out.push(u8::from(last));
    out.extend_from_slice(data);
    Bytes::from(out)
}

#[test]
fn multiplexes_independent_clipboard_items_and_fin() {
    let mut mux = TcpMuxState::default();
    let a = VirtualStreamId {
        kind: StreamKind::Clipboard,
        item_id: 7,
    };
    let b = VirtualStreamId {
        kind: StreamKind::Clipboard,
        item_id: 8,
    };
    assert_eq!(
        mux.receive(3, clipboard(7, false, b"a")).unwrap(),
        vec![
            MuxEvent::Open(a),
            MuxEvent::Data(a, Bytes::from_static(b"a"))
        ]
    );
    assert_eq!(
        mux.receive(3, clipboard(8, true, b"b")).unwrap(),
        vec![
            MuxEvent::Open(b),
            MuxEvent::Data(b, Bytes::from_static(b"b")),
            MuxEvent::Fin(b)
        ]
    );
    assert_eq!(
        mux.receive(3, clipboard(7, true, b"z")).unwrap(),
        vec![
            MuxEvent::Data(a, Bytes::from_static(b"z")),
            MuxEvent::Fin(a)
        ]
    );
}

#[test]
fn cursor_is_persistent_and_other_channels_are_classified() {
    let mut mux = TcpMuxState::default();
    let cursor = VirtualStreamId {
        kind: StreamKind::Cursor,
        item_id: 0,
    };
    assert_eq!(
        mux.receive(2, Bytes::from_static(b"x")).unwrap(),
        vec![
            MuxEvent::Open(cursor),
            MuxEvent::Data(cursor, Bytes::from_static(b"\0\0\0\x01x"))
        ]
    );
    assert_eq!(
        mux.receive(2, Bytes::new()).unwrap(),
        vec![MuxEvent::Data(cursor, Bytes::from_static(b"\0\0\0\0"))]
    );
    assert_eq!(
        mux.receive(0, Bytes::new()).unwrap(),
        vec![MuxEvent::Control(Bytes::new())]
    );
    assert_eq!(
        mux.receive(0x80, Bytes::new()).unwrap(),
        vec![MuxEvent::Datagram(0x80, Bytes::new())]
    );
    assert_eq!(
        mux.receive(9, Bytes::new()).unwrap(),
        vec![MuxEvent::Ignore]
    );
    assert!(mux.receive(1, Bytes::new()).is_err());
    assert!(mux.receive(3, Bytes::from_static(b"bad")).is_err());
    assert!(mux.receive(3, Bytes::from_static(b"\0\0\0\0\x02")).is_err());
}

#[test]
fn bounds_unfinished_clipboard_items() {
    let mut mux = TcpMuxState::default();
    for id in 0..64 {
        mux.receive(3, clipboard(id, false, b"")).unwrap();
    }
    assert!(mux.receive(3, clipboard(64, false, b"")).is_err());
    mux.receive(3, clipboard(0, true, b"")).unwrap();
    mux.receive(3, clipboard(64, false, b"")).unwrap();
}
