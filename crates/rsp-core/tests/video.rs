use rsp_core::fec::{MAX_DATA_SHARDS, MAX_PARITY_SHARDS};
use rsp_core::video::{
    packetize, packetize_tcp, FrameMeta, KeyframeReason, Reassembler, ReassemblyEvent, VideoHeader,
    FLAG_KEYFRAME, HEADER_LEN, MAX_REASSEMBLY_BYTES, MAX_SLOTS,
};
use rsp_core::wire::DecodeError;
use std::path::Path;

fn meta(frame_id: u32, flags: u8) -> FrameMeta {
    FrameMeta {
        flags,
        display_id: 3,
        epoch: 7,
        frame_id,
        capture_ts_us: 42,
    }
}

fn wire(header: VideoHeader, payload_len: usize) -> Vec<u8> {
    let mut out = Vec::with_capacity(HEADER_LEN + payload_len);
    header.write(&mut out);
    out.resize(HEADER_LEN + payload_len, 0);
    out
}

fn valid_header() -> VideoHeader {
    VideoHeader {
        flags: 0,
        display_id: 1,
        epoch: 2,
        frame_id: 3,
        shard_idx: 0,
        data_shards: 2,
        parity_shards: 1,
        shard_len: 16,
        frame_len: 24,
        capture_ts_us: 99,
    }
}

fn frame_packets(frame_id: u32, epoch: u8, flags: u8, len: usize, r: f32) -> Vec<bytes::Bytes> {
    packetize(
        &vec![frame_id as u8; len],
        FrameMeta {
            flags,
            display_id: 1,
            epoch,
            frame_id,
            capture_ts_us: u64::from(frame_id),
        },
        1200,
        r,
    )
    .unwrap()
}

fn events_frames(events: &[ReassemblyEvent]) -> Vec<u32> {
    events
        .iter()
        .filter_map(|event| match event {
            ReassemblyEvent::Frame(frame) => Some(frame.frame_id),
            _ => None,
        })
        .collect()
}

fn has_loss(events: &[ReassemblyEvent]) -> bool {
    events
        .iter()
        .any(|event| matches!(event, ReassemblyEvent::Lost { .. }))
}

#[test]
fn header_rejects_wrong_type() {
    let mut data = wire(valid_header(), 16);
    data[0] = 0x11;
    assert_eq!(
        VideoHeader::parse(&data).unwrap_err(),
        DecodeError::UnknownType(0x11)
    );
}

#[test]
fn header_rejects_reserved_flags() {
    let mut header = valid_header();
    header.flags = 0x04;
    assert!(VideoHeader::parse(&wire(header, 16)).is_err());
}

#[test]
fn header_rejects_zero_data_shards() {
    let mut header = valid_header();
    header.data_shards = 0;
    assert!(VideoHeader::parse(&wire(header, 16)).is_err());
}

#[test]
fn header_rejects_data_shards_above_4096() {
    let mut header = valid_header();
    header.data_shards = (MAX_DATA_SHARDS + 1) as u16;
    assert!(VideoHeader::parse(&wire(header, 16)).is_err());
}

#[test]
fn header_rejects_parity_shards_above_64() {
    let mut header = valid_header();
    header.parity_shards = (MAX_PARITY_SHARDS + 1) as u16;
    assert!(VideoHeader::parse(&wire(header, 16)).is_err());
}

#[test]
fn header_rejects_shard_index_out_of_range() {
    let mut header = valid_header();
    header.shard_idx = header.data_shards + header.parity_shards;
    assert!(VideoHeader::parse(&wire(header, 16)).is_err());
}

#[test]
fn header_rejects_quic_odd_shard_length() {
    let mut header = valid_header();
    header.shard_len = 15;
    header.frame_len = 15;
    assert!(VideoHeader::parse(&wire(header, 15)).is_err());
}

#[test]
fn header_rejects_quic_zero_shard_length() {
    let mut header = valid_header();
    header.shard_len = 0;
    assert!(VideoHeader::parse(&wire(header, 0)).is_err());
}

#[test]
fn header_rejects_frame_longer_than_data_capacity() {
    let mut header = valid_header();
    header.frame_len = 33;
    assert!(VideoHeader::parse(&wire(header, 16)).is_err());
}

#[test]
fn header_rejects_payload_length_mismatch() {
    let header = valid_header();
    assert!(VideoHeader::parse(&wire(header, 15)).is_err());
}

#[test]
fn header_rejects_tcp_data_shards_other_than_one() {
    let mut header = valid_header();
    header.data_shards = 2;
    header.parity_shards = 0;
    header.shard_len = 0;
    header.frame_len = 0;
    assert!(VideoHeader::parse(&wire(header, 0)).is_err());
}

#[test]
fn header_rejects_tcp_parity_shards_other_than_zero() {
    let mut header = valid_header();
    header.data_shards = 1;
    header.parity_shards = 1;
    header.shard_len = 0;
    header.frame_len = 0;
    assert!(VideoHeader::parse(&wire(header, 0)).is_err());
}

#[test]
fn header_rejects_tcp_payload_length_mismatch() {
    let mut header = valid_header();
    header.data_shards = 1;
    header.parity_shards = 0;
    header.shard_len = 0;
    header.frame_len = 4;
    assert!(VideoHeader::parse(&wire(header, 3)).is_err());
}

#[test]
fn header_vectors_round_trip() {
    for name in ["ordinary", "keyframe", "tcp"] {
        let path = Path::new(env!("CARGO_MANIFEST_DIR"))
            .join("tests/vectors/video")
            .join(format!("{name}.bin"));
        let data = std::fs::read(path).unwrap();
        let mut offset = 0;
        while offset < data.len() {
            let shard_len = u16::from_be_bytes([data[offset + 14], data[offset + 15]]) as usize;
            let frame_len = u32::from_be_bytes([
                data[offset + 16],
                data[offset + 17],
                data[offset + 18],
                data[offset + 19],
            ]) as usize;
            let payload_len = if shard_len == 0 { frame_len } else { shard_len };
            let record_len = HEADER_LEN + payload_len;
            let (header, _payload) =
                VideoHeader::parse(&data[offset..offset + record_len]).unwrap();
            assert_eq!(
                &data[offset..offset + HEADER_LEN],
                &header.encode(),
                "{name} header must round-trip byte-for-byte"
            );
            offset += record_len;
            assert_eq!(
                header.frame_id,
                if name == "keyframe" {
                    42
                } else if name == "tcp" {
                    5
                } else {
                    10
                }
            );
        }
    }
}

#[test]
fn packetize_s_k_m_table_mtu1200_1452_r0_01_05() {
    for (mtu, redundancy, expected_m) in [
        (1200, 0.0, 0),
        (1200, 0.1, 1),
        (1200, 0.5, 2),
        (1452, 0.0, 0),
        (1452, 0.1, 1),
        (1452, 0.5, 2),
    ] {
        packetize_s_k_m_case(mtu, redundancy, expected_m);
    }
}

fn packetize_s_k_m_case(mtu: usize, redundancy: f32, expected_m: usize) {
    let shard_len = ((mtu - HEADER_LEN) / 2) * 2;
    let frame = vec![0x5a; shard_len * 3 + 1];
    let packets = packetize(&frame, meta(11, FLAG_KEYFRAME), mtu, redundancy).unwrap();
    let (header, payload) = VideoHeader::parse(&packets[0]).unwrap();
    assert_eq!(header.shard_len as usize, shard_len);
    assert_eq!(header.data_shards, 4);
    assert_eq!(header.parity_shards as usize, expected_m);
    assert_eq!(payload.len(), shard_len);
    assert_eq!(packets.len(), 4 + expected_m);
}

#[test]
fn packetize_s_k_m_rejects_k4097() {
    let shard_len = 1172;
    let frame = vec![0u8; shard_len * 4097];
    assert!(matches!(
        packetize(&frame, meta(12, 0), 1200, 0.1),
        Err(rsp_core::video::PacketizeError::FrameTooLarge { k: 4097 })
    ));
}

const TEST_SHARD_LEN: usize = 1172;

fn packetize_k_m(flags: u8, k: usize, redundancy: f32) -> (VideoHeader, usize, usize) {
    let frame_len = (k - 1) * TEST_SHARD_LEN + 1;
    let frame: Vec<u8> = (0..frame_len)
        .map(|idx| (idx.wrapping_mul(31) % 251) as u8)
        .collect();
    let packets = packetize(&frame, meta(21, flags), 1200, redundancy).unwrap();
    let (first, _) = VideoHeader::parse(&packets[0]).unwrap();
    let mut data = 0;
    let mut parity = 0;
    for packet in &packets {
        let (header, payload) = VideoHeader::parse(packet).unwrap();
        assert_eq!(header.data_shards as usize, k);
        assert_eq!(header.parity_shards, first.parity_shards);
        assert_eq!(payload.len(), TEST_SHARD_LEN);
        if usize::from(header.shard_idx) < k {
            data += 1;
        } else {
            parity += 1;
        }
    }
    assert_eq!(data, k);
    assert_eq!(parity, usize::from(first.parity_shards));
    assert_eq!(packets.len(), k + parity);
    (first, data, parity)
}

#[test]
fn packetize_applies_keyframe_redundancy_floor() {
    for (flags, k, redundancy, expected_m) in [
        (FLAG_KEYFRAME, 44, 0.05, 11),
        (0, 44, 0.05, 3),
        (rsp_core::video::FLAG_STILL, 44, 0.05, 3),
        (FLAG_KEYFRAME, 44, 0.30, 14),
        (FLAG_KEYFRAME, 44, 0.0, 0),
        (FLAG_KEYFRAME, 300, 0.05, 64),
        (FLAG_KEYFRAME, 1, 0.05, 1),
    ] {
        let (header, data, parity) = packetize_k_m(flags, k, redundancy);
        assert_eq!(data, k, "flags={flags:#x} k={k} r={redundancy}");
        assert_eq!(
            (usize::from(header.parity_shards), parity),
            (expected_m, expected_m),
            "flags={flags:#x} k={k} r={redundancy}"
        );
    }
}

#[test]
fn keyframe_recovers_after_losing_any_eleven_shards() {
    let k = 44;
    let m = 11;
    let frame_len = (k - 1) * TEST_SHARD_LEN + 1;
    let original: Vec<u8> = (0..frame_len)
        .map(|idx| (idx.wrapping_mul(17) % 251) as u8)
        .collect();
    let packets = packetize(&original, meta(22, FLAG_KEYFRAME), 1200, 0.05).unwrap();
    let (header, _) = VideoHeader::parse(&packets[0]).unwrap();
    assert_eq!(
        (header.data_shards as usize, header.parity_shards as usize),
        (k, m)
    );
    assert_eq!(packets.len(), k + m);

    let total = k + m;
    let mut losses = Vec::new();
    for start in 0..total {
        losses.push((0..m).map(|offset| (start + offset) % total).collect());
    }
    let mut state = 0x9e37_79b9_u64;
    for _ in 0..32 {
        let mut indices: Vec<usize> = (0..total).collect();
        for selected in 0..m {
            state ^= state >> 12;
            state ^= state << 25;
            state ^= state >> 27;
            let remaining = total - selected;
            let offset = (state as usize) % remaining;
            indices.swap(selected, selected + offset);
        }
        losses.push(indices[..m].to_vec());
    }

    for (case, lost) in losses.iter().enumerate() {
        let lost: std::collections::BTreeSet<usize> = lost.iter().copied().collect();
        assert_eq!(lost.len(), m);
        let mut reassembler = Reassembler::with_fps(60);
        let mut events = Vec::new();
        let ordered: Vec<&bytes::Bytes> = if case % 3 == 0 {
            packets.iter().rev().collect()
        } else {
            packets.iter().collect()
        };
        for (order, packet) in ordered.into_iter().enumerate() {
            let (shard, _) = VideoHeader::parse(packet).unwrap();
            if !lost.contains(&usize::from(shard.shard_idx)) {
                events.extend(reassembler.push(packet, order as u64).unwrap());
            }
        }
        let recovered = events.into_iter().find_map(|event| match event {
            ReassemblyEvent::Frame(frame) => Some(frame),
            _ => None,
        });
        assert_eq!(
            recovered.as_ref().map(|frame| frame.data.as_slice()),
            Some(original.as_slice()),
            "case {case} lost {lost:?}"
        );
        assert_eq!(reassembler.stats.frames_lost, 0, "case {case}");
    }
}

#[test]
fn packetize_tcp_writes_one_unpadded_record() {
    let frame = b"tcp access unit";
    let packets = packetize_tcp(frame, meta(13, FLAG_KEYFRAME)).unwrap();
    assert_eq!(packets.len(), 1);
    let (header, payload) = VideoHeader::parse(&packets[0]).unwrap();
    assert_eq!(
        (header.data_shards, header.parity_shards, header.shard_len),
        (1, 0, 0)
    );
    assert_eq!(payload, frame);
}

#[test]
fn reassembler_delivers_out_of_order_shards() {
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    let mut reassembler = Reassembler::with_fps(60);
    let mut events = Vec::new();
    for packet in packets.iter().rev() {
        events.extend(reassembler.push(packet, 1_000).unwrap());
    }
    assert_eq!(events_frames(&events), vec![0]);
}

#[test]
fn reassembler_ignores_duplicate_shards() {
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 1000, 0.0);
    let mut reassembler = Reassembler::with_fps(60);
    assert_eq!(
        events_frames(&reassembler.push(&packets[0], 0).unwrap()),
        vec![0]
    );
    assert!(reassembler.push(&packets[0], 1).unwrap().is_empty());
    assert!(reassembler.stats.stale >= 1);
}

#[test]
fn reassembler_discards_inconsistent_slot_metadata() {
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    let mut inconsistent = packets[1].to_vec();
    inconsistent[1] = 0;
    let mut reassembler = Reassembler::with_fps(60);
    reassembler.push(&packets[0], 0).unwrap();
    assert!(reassembler.push(&inconsistent, 1).unwrap().is_empty());
    assert_eq!(reassembler.stats.inconsistent, 1);
    assert!(reassembler.stats.discarded >= 1);
}

#[test]
fn reassembler_discards_old_epoch() {
    let mut reassembler = Reassembler::with_fps(60);
    let first = frame_packets(0, 3, FLAG_KEYFRAME, 500, 0.0);
    assert_eq!(
        events_frames(&reassembler.push(&first[0], 0).unwrap()),
        vec![0]
    );
    let old = frame_packets(1, 2, FLAG_KEYFRAME, 500, 0.0);
    assert!(reassembler.push(&old[0], 1).unwrap().is_empty());
    assert!(reassembler.stats.discarded >= 1);
}

#[test]
fn reassembler_handles_epoch_wrap_255_to_0() {
    let mut reassembler = Reassembler::with_fps(60);
    let at_255 = frame_packets(0, 255, FLAG_KEYFRAME, 500, 0.0);
    let at_0 = frame_packets(0, 0, FLAG_KEYFRAME, 500, 0.0);
    assert_eq!(
        events_frames(&reassembler.push(&at_255[0], 0).unwrap()),
        vec![0]
    );
    assert_eq!(
        events_frames(&reassembler.push(&at_0[0], 1).unwrap()),
        vec![0]
    );
}

#[test]
fn reassembler_recovers_after_deleting_any_m_shards() {
    let frame_len = 7 * 1172;
    let mut seed = 0x4d59_5df4_u64;
    for case in 0..16 {
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let first = (seed % 9) as usize;
        seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
        let mut second = (seed % 9) as usize;
        if second == first {
            second = (second + 1) % 9;
        }
        let packets = frame_packets(case as u32, 1, FLAG_KEYFRAME, frame_len, 2.0 / 7.0);
        let (header, _) = VideoHeader::parse(&packets[0]).unwrap();
        assert_eq!((header.data_shards, header.parity_shards), (7, 2));
        let mut reassembler = Reassembler::with_fps(60);
        let mut events = Vec::new();
        for (idx, packet) in packets.iter().enumerate() {
            if idx != first && idx != second {
                events.extend(reassembler.push(packet, 0).unwrap());
            }
        }
        let recovered = events.into_iter().find_map(|event| match event {
            ReassemblyEvent::Frame(frame) => Some(frame),
            _ => None,
        });
        assert_eq!(
            recovered.as_ref().map(|frame| frame.frame_id),
            Some(case as u32)
        );
        assert_eq!(
            recovered.as_ref().map(|frame| frame.data.as_slice()),
            Some(vec![case as u8; frame_len].as_slice())
        );
    }
}

#[test]
fn reassembler_declares_loss_after_deleting_m_plus_one_shards() {
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 7 * 1172, 2.0 / 7.0);
    let (header, _) = VideoHeader::parse(&packets[0]).unwrap();
    let mut reassembler = Reassembler::with_fps(60);
    for packet in packets.iter().take((header.data_shards - 1) as usize) {
        reassembler.push(packet, 0).unwrap();
    }
    assert!(has_loss(&reassembler.tick(50_000)));
}

#[test]
fn reassembler_timeout_uses_50ms_floor_at_60fps() {
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    let mut reassembler = Reassembler::new(2);
    reassembler.set_fps(60);
    reassembler.push(&packets[0], 0).unwrap();
    assert!(!has_loss(&reassembler.tick(49_999)));
    assert!(has_loss(&reassembler.tick(50_000)));
}

#[test]
fn reassembler_default_fps_is_60() {
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    let mut reassembler = Reassembler::new(2);
    reassembler.push(&packets[0], 0).unwrap();
    assert!(has_loss(&reassembler.tick(50_000)));
}

#[test]
fn reassembler_timeout_uses_two_frame_intervals_at_10fps() {
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    let mut reassembler = Reassembler::new(2);
    reassembler.set_fps(10);
    reassembler.push(&packets[0], 0).unwrap();
    assert!(!has_loss(&reassembler.tick(199_999)));
    assert!(has_loss(&reassembler.tick(200_000)));
}

#[test]
fn reassembler_declares_gap_when_two_newer_frames_are_ready() {
    let mut reassembler = Reassembler::with_fps(60);
    let first = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    reassembler.push(&first[0], 0).unwrap();
    let second = frame_packets(2, 1, 0, 500, 0.0);
    let third = frame_packets(3, 1, 0, 500, 0.0);
    reassembler.push(&second[0], 1).unwrap();
    let events = reassembler.push(&third[0], 2).unwrap();
    assert!(has_loss(&events));
    assert!(events.iter().any(|event| matches!(
        event,
        ReassemblyEvent::NeedKeyframe {
            reason: KeyframeReason::Loss,
            ..
        }
    )));
}

#[test]
fn reassembler_drops_p_frames_while_waiting_for_keyframe() {
    let mut reassembler = Reassembler::with_fps(60);
    let first = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    reassembler.push(&first[0], 0).unwrap();
    let missing = frame_packets(2, 1, 0, 500, 0.0);
    let newer = frame_packets(3, 1, 0, 500, 0.0);
    reassembler.push(&missing[0], 1).unwrap();
    let loss = reassembler.push(&newer[0], 2).unwrap();
    assert!(has_loss(&loss));
    let p = frame_packets(4, 1, 0, 500, 0.0);
    assert!(events_frames(&reassembler.push(&p[0], 3).unwrap()).is_empty());
    let key = frame_packets(5, 1, FLAG_KEYFRAME, 500, 0.0);
    assert_eq!(
        events_frames(&reassembler.push(&key[0], 4).unwrap()),
        vec![5]
    );
}

#[test]
fn reassembler_retries_need_keyframe_after_500ms() {
    let mut reassembler = Reassembler::with_fps(60);
    let first = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    reassembler.push(&first[0], 0).unwrap();
    assert!(has_loss(&reassembler.tick(50_000)));
    assert!(!reassembler
        .tick(549_999)
        .iter()
        .any(|event| matches!(event, ReassemblyEvent::NeedKeyframe { .. })));
    assert!(reassembler
        .tick(550_000)
        .iter()
        .any(|event| matches!(event, ReassemblyEvent::NeedKeyframe { .. })));
}

#[test]
fn reassembler_flush_backlog_requests_backlog_keyframe() {
    let mut reassembler = Reassembler::with_fps(60);
    let key = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    reassembler.push(&key[0], 0).unwrap();
    let events = reassembler.flush_backlog(1_000);
    assert!(events.iter().any(|event| matches!(
        event,
        ReassemblyEvent::NeedKeyframe {
            reason: KeyframeReason::Backlog,
            ..
        }
    )));
}

#[test]
fn reassembler_delivers_frames_monotonically() {
    let mut reassembler = Reassembler::with_fps(60);
    let first = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    reassembler.push(&first[0], 0).unwrap();
    let later = frame_packets(2, 1, 0, 500, 0.0);
    reassembler.push(&later[0], 1).unwrap();
    let middle = frame_packets(1, 1, 0, 500, 0.0);
    assert_eq!(
        events_frames(&reassembler.push(&middle[0], 2).unwrap()),
        vec![1, 2]
    );
    assert!(reassembler.push(&middle[0], 3).unwrap().is_empty());
}

#[test]
fn newer_incomplete_frame_survives_older_frame_completion() {
    let mut reassembler = Reassembler::with_fps(60);
    let older = frame_packets(1, 1, FLAG_KEYFRAME, 2500, 0.0);
    let newer = frame_packets(2, 1, 0, 2500, 0.0);
    let mut events = Vec::new();

    for idx in 0..older.len() {
        events.extend(reassembler.push(&older[idx], 10).unwrap());
        events.extend(reassembler.push(&newer[idx], 10).unwrap());
    }

    assert_eq!(events_frames(&events), vec![1, 2]);
    assert!(!has_loss(&events));
    assert!(!events
        .iter()
        .any(|event| matches!(event, ReassemblyEvent::NeedKeyframe { .. })));
    assert_eq!(reassembler.stats.frames_lost, 0);
    assert!(!reassembler.waiting_keyframe());
}

#[test]
fn frame_id_wraparound_keeps_newer_frames() {
    let ids = [u32::MAX - 1, u32::MAX, 0, 1];
    let packets: Vec<_> = ids
        .iter()
        .enumerate()
        .map(|(index, &id)| {
            let flags = if index == 0 { FLAG_KEYFRAME } else { 0 };
            frame_packets(id, 1, flags, 2500, 0.0)
        })
        .collect();
    let mut reassembler = Reassembler::with_fps(60);
    let mut events = Vec::new();

    for shard in 0..packets[0].len() {
        for frame in &packets {
            events.extend(reassembler.push(&frame[shard], 10).unwrap());
        }
    }

    assert_eq!(events_frames(&events), ids);
    assert!(!has_loss(&events));
    assert!(!events
        .iter()
        .any(|event| matches!(event, ReassemblyEvent::NeedKeyframe { .. })));
    assert_eq!(reassembler.stats.frames_lost, 0);
}

#[test]
fn keyframe_recovery_prefers_newest_across_wraparound() {
    let mut reassembler = Reassembler::with_fps(60);
    let first = frame_packets(u32::MAX - 3, 1, FLAG_KEYFRAME, 500, 0.0);
    assert_eq!(
        events_frames(&reassembler.push(&first[0], 0).unwrap()),
        vec![u32::MAX - 3]
    );

    let older = frame_packets(u32::MAX - 1, 1, FLAG_KEYFRAME, 500, 0.0);
    assert!(reassembler.push(&older[0], 1).unwrap().is_empty());
    let newer = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    let events = reassembler.push(&newer[0], 2).unwrap();

    assert_eq!(events_frames(&events), vec![0]);
    assert!(events.iter().any(|event| matches!(
        event,
        ReassemblyEvent::NeedKeyframe {
            reason: KeyframeReason::Loss,
            ..
        }
    )));
}

#[test]
fn truly_old_incomplete_frame_is_still_expired() {
    let mut reassembler = Reassembler::with_fps(60);
    let mut events = Vec::new();
    let first = frame_packets(4, 1, FLAG_KEYFRAME, 500, 0.0);
    events.extend(reassembler.push(&first[0], 0).unwrap());

    let old = frame_packets(5, 1, 0, 2500, 0.0);
    events.extend(reassembler.push(&old[0], 1).unwrap());
    let seventh = frame_packets(7, 1, 0, 500, 0.0);
    events.extend(reassembler.push(&seventh[0], 2).unwrap());
    assert!(events
        .iter()
        .any(|event| matches!(event, ReassemblyEvent::Lost { frame_id: 5 })));
    assert!(events.iter().any(|event| matches!(
        event,
        ReassemblyEvent::NeedKeyframe {
            reason: KeyframeReason::Loss,
            ..
        }
    )));
    assert_eq!(reassembler.stats.frames_lost, 1);

    let eighth = frame_packets(8, 1, FLAG_KEYFRAME, 500, 0.0);
    events.extend(reassembler.push(&eighth[0], 3).unwrap());
    assert_eq!(events_frames(&events), vec![4, 8]);
}

#[test]
fn shard_completing_at_timeout_boundary_is_delivered() {
    let mut reassembler = Reassembler::with_fps(60);
    let key = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    assert!(reassembler.push(&key[0], 0).unwrap().is_empty());
    assert!(reassembler.push(&key[1], 25_000).unwrap().is_empty());

    let events = reassembler.push(&key[2], 50_000).unwrap();
    assert_eq!(events_frames(&events), vec![0]);
    assert!(!has_loss(&events));
    assert_eq!(reassembler.stats.frames_lost, 0);
}

#[test]
fn queued_fec_shards_survive_processing_pause() {
    let mut reassembler = Reassembler::with_fps(30);
    let key = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    reassembler.push(&key[0], 0).unwrap();

    let packets = frame_packets(1, 1, 0, 7 * 1172, 2.0 / 7.0);
    let mut events = Vec::new();
    // Data shard 1 is lost. The remaining data and parity are queued in wire
    // order, but the receiver is descheduled partway through consuming them.
    // Several shards (not just the completing one) remain after the 2/fps
    // deadline, so expiring on every push would destroy a recoverable frame.
    for (idx, packet) in packets.iter().enumerate() {
        if idx == 1 {
            continue;
        }
        let processing_us = if idx < 3 { 1_000 } else { 150_000 };
        events.extend(reassembler.push(packet, processing_us).unwrap());
    }
    // The idle timer runs after the queued datagrams have been consumed.
    events.extend(reassembler.tick(150_000));
    assert!(!has_loss(&events), "{events:?}");
    assert_eq!(events_frames(&events), vec![1]);
    let data = events.iter().find_map(|event| match event {
        ReassemblyEvent::Frame(frame) => Some(frame.data.as_slice()),
        _ => None,
    });
    assert_eq!(data, Some(vec![1; 7 * 1172].as_slice()));
    assert_eq!(reassembler.stats.frames_recovered, 1);
    assert_eq!(reassembler.stats.frames_lost, 0);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn processing_pause_does_not_hide_unrecoverable_fec_frame() {
    let mut reassembler = Reassembler::with_fps(30);
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 7 * 1172, 2.0 / 7.0);
    // Losing m+1 shards leaves fewer than k, regardless of processing speed.
    for (idx, packet) in packets.iter().enumerate().skip(3) {
        let processing_us = if idx < 5 { 1_000 } else { 150_000 };
        reassembler.push(packet, processing_us).unwrap();
    }
    assert!(!has_loss(&reassembler.tick(150_000)));
    let events = reassembler.tick(216_666);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ReassemblyEvent::Lost { frame_id: 0 }))
            .count(),
        1
    );
    assert!(events_frames(&events).is_empty());
    assert_eq!(reassembler.stats.frames_recovered, 0);
    assert_eq!(reassembler.stats.frames_lost, 1);
    assert_eq!(reassembler.buffered_bytes(), 0);
    assert!(!has_loss(&reassembler.tick(250_000)));
}

#[test]
fn overdue_tick_after_receiver_pause_preserves_queued_fec_shards() {
    let mut reassembler = Reassembler::with_fps(30);
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 7 * 1172, 2.0 / 7.0);
    let mut events = Vec::new();
    for packet in &packets[..3] {
        events.extend(reassembler.push(packet, 1_000).unwrap());
    }
    // The receiver and upstream QUIC driver resume at different times. An
    // overdue idle tick can run before the next part of the flight is ready.
    reassembler.pause_deadlines(20_000, 100_000);
    events.extend(reassembler.tick(100_000));
    assert!(!has_loss(&events), "{events:?}");
    for packet in &packets[4..] {
        events.extend(reassembler.push(packet, 101_000).unwrap());
    }
    assert_eq!(events_frames(&events), vec![0]);
    assert_eq!(reassembler.stats.frames_recovered, 1);
    assert_eq!(reassembler.stats.frames_lost, 0);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn sparse_idle_ticks_still_expire_missing_shards_after_receiver_pause() {
    let mut reassembler = Reassembler::with_fps(30);
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    reassembler.push(&packets[0], 1_000).unwrap();
    reassembler.pause_deadlines(1_000, 100_000);
    assert!(!has_loss(&reassembler.tick(100_000)));
    assert!(!has_loss(&reassembler.tick(166_665)));
    let events = reassembler.tick(166_666);
    assert!(has_loss(&events), "{events:?}");
    assert_eq!(reassembler.stats.frames_lost, 1);
    assert_eq!(reassembler.buffered_bytes(), 0);
    assert!(!has_loss(&reassembler.tick(200_000)));
}

#[test]
fn receiver_pause_preserves_slot_age_without_pushing_fresh_slots_into_the_future() {
    let mut reassembler = Reassembler::with_fps(30);
    let old = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    let fresh = frame_packets(1, 1, FLAG_KEYFRAME, 2500, 0.0);
    reassembler.push(&old[0], 1_000).unwrap();
    reassembler.push(&fresh[0], 190_000).unwrap();
    reassembler.pause_deadlines(20_000, 200_000);
    assert!(!has_loss(&reassembler.tick(247_665)));
    let events = reassembler.tick(247_666);
    assert!(events.contains(&ReassemblyEvent::Lost { frame_id: 0 }));
    assert!(!events.contains(&ReassemblyEvent::Lost { frame_id: 1 }));
    assert!(!has_loss(&reassembler.tick(266_665)));
    assert!(reassembler
        .tick(266_666)
        .contains(&ReassemblyEvent::Lost { frame_id: 1 }));
    assert_eq!(reassembler.stats.frames_lost, 2);
}

#[test]
fn fec_flight_making_progress_survives_regular_idle_polls() {
    let mut reassembler = Reassembler::with_fps(30);
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 7 * 1172, 2.0 / 7.0);
    let mut events = Vec::new();
    // Pacing may split one flight across many scheduler turns. Poll every
    // 20 ms and supply a distinct shard every 40 ms, losing data shard 1.
    // Useful progress renews the strict idle deadline even when the whole
    // flight spans several frame intervals.
    let surviving = packets
        .iter()
        .enumerate()
        .filter_map(|(idx, packet)| (idx != 1).then_some(packet));
    for (ordinal, packet) in surviving.enumerate() {
        let now_us = ordinal as u64 * 40_000;
        if ordinal > 0 {
            for poll_us in ((now_us - 20_000)..now_us).step_by(20_000) {
                events.extend(reassembler.tick(poll_us));
            }
        }
        events.extend(reassembler.push(packet, now_us).unwrap());
    }
    assert!(!has_loss(&events), "{events:?}");
    assert_eq!(events_frames(&events), vec![0]);
    assert_eq!(reassembler.stats.frames_recovered, 1);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn duplicate_shards_do_not_renew_idle_deadline() {
    let mut reassembler = Reassembler::with_fps(30);
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    reassembler.push(&packets[0], 0).unwrap();
    reassembler.push(&packets[1], 30_000).unwrap();
    assert!(!has_loss(&reassembler.tick(60_000)));
    assert!(reassembler.push(&packets[1], 80_000).unwrap().is_empty());
    assert!(!has_loss(&reassembler.tick(96_665)));
    assert!(has_loss(&reassembler.tick(96_666)));
    assert_eq!(reassembler.stats.frames_lost, 1);
    assert_eq!(reassembler.stats.frames_recovered, 0);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn timed_out_frame_cannot_be_recreated_by_late_fec_tail() {
    let mut reassembler = Reassembler::with_fps(60);
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 7 * 1172, 2.0 / 7.0);
    for packet in &packets[..3] {
        reassembler.push(packet, 0).unwrap();
    }
    assert!(has_loss(&reassembler.tick(50_000)));
    // The remaining six shards cannot resurrect the slot or produce another
    // loss. No newer keyframe has arrived to make frame 0 stale by frame id.
    for packet in &packets[3..] {
        assert!(reassembler.push(packet, 51_000).unwrap().is_empty());
    }
    assert!(!has_loss(&reassembler.tick(101_000)));
    assert_eq!(reassembler.stats.frames_lost, 1);
    assert_eq!(reassembler.stats.frames_recovered, 0);
    assert_eq!(reassembler.slot_count(), 0);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn idle_fallback_ignores_duplicate_and_inconsistent_traffic() {
    let mut reassembler = Reassembler::with_fps(30);
    let packets = frame_packets(0, 1, FLAG_KEYFRAME, 2500, 0.0);
    reassembler.push(&packets[0], 0).unwrap();
    let mut inconsistent = packets[1].to_vec();
    inconsistent[1] |= rsp_core::video::FLAG_STILL;
    for now_us in [10_000, 20_000, 30_000, 40_000, 50_000, 60_000] {
        assert!(reassembler.push(&packets[0], now_us).unwrap().is_empty());
        assert!(reassembler.push(&inconsistent, now_us).unwrap().is_empty());
        assert!(!has_loss(&reassembler.tick(now_us)));
    }
    assert!(!has_loss(&reassembler.tick(66_665)));
    assert!(has_loss(&reassembler.tick(66_666)));
    assert_eq!(reassembler.stats.frames_lost, 1);
    assert_eq!(reassembler.stats.frames_recovered, 0);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn newer_frames_still_expire_incomplete_frame_without_idle_tick() {
    let mut reassembler = Reassembler::with_fps(30);
    let key = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    reassembler.push(&key[0], 0).unwrap();
    let incomplete = frame_packets(1, 1, 0, 7 * 1172, 2.0 / 7.0);
    reassembler.push(&incomplete[0], 1_000).unwrap();

    let second = frame_packets(2, 1, 0, 500, 0.0);
    let events = reassembler.push(&second[0], 150_000).unwrap();
    assert!(!has_loss(&events), "{events:?}");
    assert!(events_frames(&events).is_empty());

    let third = frame_packets(3, 1, FLAG_KEYFRAME, 500, 0.0);
    let events = reassembler.push(&third[0], 150_000).unwrap();
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ReassemblyEvent::Lost { frame_id: 1 }))
            .count(),
        1
    );
    assert_eq!(events_frames(&events), vec![3]);
    assert_eq!(reassembler.stats.frames_lost, 1);
    assert_eq!(reassembler.buffered_bytes(), 0);
}

#[test]
fn completed_frame_after_newer_delivery_is_counted_lost_once() {
    let mut reassembler = Reassembler::with_fps(60);
    let mut events = Vec::new();
    let key = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    events.extend(reassembler.push(&key[0], 0).unwrap());
    let older = frame_packets(1, 1, 0, 2500, 0.0);
    events.extend(reassembler.push(&older[0], 1).unwrap());
    let second = frame_packets(2, 1, FLAG_KEYFRAME, 500, 0.0);
    events.extend(reassembler.push(&second[0], 2).unwrap());
    let third = frame_packets(3, 1, 0, 500, 0.0);
    events.extend(reassembler.push(&third[0], 3).unwrap());
    assert_eq!(events_frames(&events), vec![0, 2, 3]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ReassemblyEvent::Lost { frame_id: 1 }))
            .count(),
        1
    );
    assert_eq!(reassembler.stats.frames_lost, 1);

    // The still-late original shards of frame 1 cannot resurrect it after
    // frames 2 and 3 have been delivered.
    for shard in &older[1..] {
        events.extend(reassembler.push(shard, 4).unwrap());
    }
    assert_eq!(events_frames(&events), vec![0, 2, 3]);
    assert_eq!(reassembler.stats.frames_lost, 1);
}

#[test]
fn late_fec_shard_after_newer_delivery_cannot_revive_older_frame() {
    let mut reassembler = Reassembler::with_fps(60);
    let mut events = Vec::new();
    let key = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    events.extend(reassembler.push(&key[0], 0).unwrap());
    // Frame 1 needs three shards for FEC recovery. Deliver two now; the third
    // will arrive only after newer frames have been delivered.
    let older = frame_packets(1, 1, 0, 3 * 1172, 2.0 / 3.0);
    let (header, _) = VideoHeader::parse(&older[0]).unwrap();
    assert_eq!((header.data_shards, header.parity_shards), (3, 2));
    events.extend(reassembler.push(&older[0], 1).unwrap());
    events.extend(reassembler.push(&older[3], 2).unwrap());
    assert_eq!(events_frames(&events), vec![0]);

    let second = frame_packets(2, 1, FLAG_KEYFRAME, 500, 0.0);
    events.extend(reassembler.push(&second[0], 3).unwrap());
    let third = frame_packets(3, 1, 0, 500, 0.0);
    events.extend(reassembler.push(&third[0], 4).unwrap());
    assert_eq!(events_frames(&events), vec![0, 2, 3]);

    // This shard would let frame 1 recover, but it is older than every frame
    // already delivered. It must not be delivered again or out of order.
    events.extend(reassembler.push(&older[1], 5).unwrap());
    assert_eq!(events_frames(&events), vec![0, 2, 3]);
    assert_eq!(
        events
            .iter()
            .filter(|event| matches!(event, ReassemblyEvent::Lost { frame_id: 1 }))
            .count(),
        1
    );
    assert_eq!(reassembler.stats.frames_lost, 1);
}

#[test]
fn fec_recovered_frame_ignores_late_original_shard() {
    let mut reassembler = Reassembler::with_fps(60);
    let mut events = Vec::new();
    let key = frame_packets(0, 1, FLAG_KEYFRAME, 500, 0.0);
    events.extend(reassembler.push(&key[0], 0).unwrap());

    let frame = frame_packets(1, 1, 0, 3 * 1172, 2.0 / 3.0);
    let (header, _) = VideoHeader::parse(&frame[0]).unwrap();
    assert_eq!((header.data_shards, header.parity_shards), (3, 2));
    // Data shards 0 and 1 plus parity shard 0 are enough to recover data 2.
    events.extend(reassembler.push(&frame[0], 1).unwrap());
    events.extend(reassembler.push(&frame[1], 2).unwrap());
    assert_eq!(events_frames(&events), vec![0]);
    events.extend(reassembler.push(&frame[3], 3).unwrap());
    assert_eq!(events_frames(&events), vec![0, 1]);
    assert_eq!(reassembler.stats.frames_recovered, 1);
    assert_eq!(reassembler.stats.frames_lost, 0);

    // The original data shard recovered above arrives late. It must not cause
    // a second delivery of the same frame.
    events.extend(reassembler.push(&frame[2], 4).unwrap());
    assert_eq!(events_frames(&events), vec![0, 1]);
    assert_eq!(reassembler.stats.frames_recovered, 1);
    assert_eq!(reassembler.stats.frames_lost, 0);
}

#[test]
fn reassembler_enforces_eight_slots_and_64mib_memory_cap() {
    let mut reassembler = Reassembler::with_fps(60);
    for id in 0..MAX_SLOTS as u32 {
        let packets = frame_packets(id, 1, FLAG_KEYFRAME, 2500, 0.0);
        reassembler.push(&packets[0], 0).unwrap();
    }
    assert_eq!(reassembler.slot_count(), MAX_SLOTS);
    assert!(reassembler.buffered_bytes() <= MAX_REASSEMBLY_BYTES);

    let ninth = frame_packets(MAX_SLOTS as u32, 1, FLAG_KEYFRAME, 2500, 0.0);
    assert!(has_loss(&reassembler.push(&ninth[0], 0).unwrap()));
    assert_eq!(reassembler.slot_count(), MAX_SLOTS);

    let huge_header = VideoHeader {
        flags: FLAG_KEYFRAME,
        display_id: 1,
        epoch: 1,
        frame_id: 99,
        shard_idx: 0,
        data_shards: MAX_DATA_SHARDS as u16,
        parity_shards: MAX_PARITY_SHARDS as u16,
        shard_len: u16::MAX - 1,
        frame_len: (MAX_DATA_SHARDS * (u16::MAX as usize - 1)) as u32,
        capture_ts_us: 0,
    };
    let huge = wire(huge_header, usize::from(u16::MAX - 1));
    assert!(has_loss(&reassembler.push(&huge, 1).unwrap()));
    assert!(reassembler.buffered_bytes() <= MAX_REASSEMBLY_BYTES);
}

fn random_loss_run(drop_percent: u32) -> (usize, usize) {
    let mut seed = 0x9e37_79b9_u64;
    let mut reassembler = Reassembler::with_fps(60);
    let mut delivered = 0;
    let total = 10_000;
    for frame_id in 0..total {
        let size = match frame_id % 4 {
            0 => 96,
            1 => 840,
            2 => 1_900,
            _ => 4_800,
        };
        let flags = if frame_id % 10 == 0 { FLAG_KEYFRAME } else { 0 };
        let packets = frame_packets(frame_id as u32, 1, flags, size, 0.1);
        let mut events = Vec::new();
        for packet in packets {
            seed = seed.wrapping_mul(6_364_136_223_846_793_005).wrapping_add(1);
            if (seed % 100) as u32 >= drop_percent {
                events.extend(reassembler.push(&packet, frame_id as u64 * 16_667).unwrap());
            }
        }
        events.extend(reassembler.tick(frame_id as u64 * 16_667 + 1));
        delivered += events
            .iter()
            .filter(|event| matches!(event, ReassemblyEvent::Frame(_)))
            .count();
    }
    (delivered, total)
}

#[test]
fn random_loss_1_percent_10000_frames_recovery_rate() {
    let (delivered, total) = random_loss_run(1);
    let rate = delivered as f64 / total as f64;
    println!("random_loss_1_percent: delivered={delivered}/{total} recovery_rate={rate:.4}");
    assert!(rate >= 0.90, "recovery rate {rate:.4} below 0.90");
}

#[test]
fn random_loss_5_percent_10000_frames_recovery_rate() {
    let (delivered, total) = random_loss_run(5);
    let rate = delivered as f64 / total as f64;
    println!("random_loss_5_percent: delivered={delivered}/{total} recovery_rate={rate:.4}");
    assert!(rate >= 0.80, "recovery rate {rate:.4} below 0.80");
}
