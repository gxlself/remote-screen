use rs_recording::chunk::{
    ChunkBuilder, ChunkOut, CHUNK_MAX_BYTES, CHUNK_MAX_SPAN_US, CHUNK_TARGET_BYTES,
};
use rs_recording::record::{
    decode, AudioRec, Body, ConfigRec, CursorShapeRec, GapReason, Record, RecordingEnd,
    RecordingMeta, VideoRec, HEADER_LEN,
};

fn meta() -> RecordingMeta {
    RecordingMeta {
        host_id: "h".into(),
        host_name: "Host".into(),
        role: "viewer".into(),
        display_id: None,
        started_at_us: 100,
        app_version: "1".into(),
        audio: false,
    }
}

fn config() -> ConfigRec {
    ConfigRec {
        epoch: 1,
        display_id: 2,
        codec: 1,
        width: 1920,
        height: 1080,
        fps: 60,
        bitrate_kbps: 4000,
        cursor_x: 10.0,
        cursor_y: 20.0,
        cursor_visible: true,
    }
}

fn rec(t_us: u64, body: Body) -> Record {
    Record { t_us, body }
}

fn video(t_us: u64, id: u32, keyframe: bool, size: usize) -> Record {
    rec(
        t_us,
        Body::Video(VideoRec {
            frame_id: id,
            epoch: 1,
            display_id: 2,
            flags: u8::from(keyframe),
            keyframe,
            capture_ts_us: t_us,
            capture_local_us: None,
            data: vec![id as u8; size],
        }),
    )
}

fn audio(t_us: u64) -> Record {
    rec(
        t_us,
        Body::Audio(AudioRec {
            seq: 1,
            capture_ts_us: t_us,
            payload: vec![1, 2],
        }),
    )
}

fn shape(id: u64) -> Body {
    Body::CursorShape(CursorShapeRec {
        id,
        width: 2,
        height: 3,
        hot_x: 0,
        hot_y: 1,
        scale: 1.0,
        png: vec![1, 2],
    })
}

fn cursor_pos() -> Body {
    Body::CursorPos {
        x: 4.0,
        y: 5.0,
        visible: true,
    }
}

fn end() -> RecordingEnd {
    RecordingEnd {
        reason: "sessionEnded".into(),
        ended_at_us: 77,
        ..RecordingEnd::default()
    }
}

fn records(chunk: &ChunkOut) -> Vec<Record> {
    assert!(chunk.plaintext.len() <= CHUNK_MAX_BYTES);
    let records = decode(&chunk.plaintext).expect("builder emits complete records");
    assert!(!records.is_empty());
    assert!(records.windows(2).all(|pair| pair[0].t_us <= pair[1].t_us));
    records
}

// Validate every emitted chunk, including those whose contents a test need not inspect.
fn push(builder: &mut ChunkBuilder, record: Record) -> Vec<ChunkOut> {
    let chunks = builder.push(record);
    assert!(chunks.len() <= 2);
    for chunk in &chunks {
        records(chunk);
    }
    chunks
}

fn finish(builder: ChunkBuilder, end: RecordingEnd) -> (Option<ChunkOut>, ChunkOut) {
    let (pending, final_chunk) = builder.finish(end);
    if let Some(chunk) = &pending {
        records(chunk);
    }
    let final_records = records(&final_chunk);
    assert_eq!(final_records.len(), 1);
    assert!(matches!(final_records[0].body, Body::End(_)));
    assert!(!final_chunk.keyframe_start);
    (pending, final_chunk)
}

fn final_end(chunk: &ChunkOut) -> RecordingEnd {
    match records(chunk).pop().unwrap().body {
        Body::End(end) => end,
        other => panic!("unexpected final body: {other:?}"),
    }
}

#[test]
fn before_first_keyframe_only_video_drops_are_counted_and_state_is_cached() {
    let mut builder = ChunkBuilder::new(meta());
    for i in 0..3 {
        assert!(push(&mut builder, video(i, i as u32, false, 1)).is_empty());
    }
    assert!(push(&mut builder, audio(3)).is_empty());
    assert!(push(&mut builder, rec(4, Body::Config(config()))).is_empty());
    assert_eq!(builder.stats().dropped, 3);
    assert_eq!(builder.stats().frames, 0);
    assert_eq!(builder.stats().audio_packets, 0);
    assert_eq!(builder.stats().width, 1920);
    assert_eq!(builder.stats().height, 1080);
    assert_eq!(builder.stats().codec, 1);
    assert!(push(
        &mut builder,
        rec(
            5,
            Body::Gap {
                reason: GapReason::Paused,
                real_us: 50
            }
        )
    )
    .is_empty());
    assert_eq!(builder.stats().gaps, 0);
    assert_eq!(builder.stats().last_t_us, 0);

    assert!(push(&mut builder, video(1000, 1, true, 2)).is_empty());
    assert_eq!(builder.stats().plain_bytes, 0);
    let (pending, final_chunk) = finish(builder, end());
    let pending = pending.unwrap();
    assert_eq!(
        records(&pending),
        [
            rec(0, Body::Meta(meta())),
            rec(1000, Body::Config(config())),
            video(1000, 1, true, 2),
        ]
    );
    assert!(pending.keyframe_start);
    assert_eq!(pending.t_start_us, 1000);
    let ended = final_end(&final_chunk);
    assert_eq!(ended.frames, 1);
    assert_eq!(ended.keyframes, 1);
    assert_eq!(ended.dropped, 3);
    assert_eq!(ended.plain_bytes, pending.plaintext.len() as u64);
    assert_eq!(ended.reason, "sessionEnded");
    assert_eq!(ended.ended_at_us, 77);
}

#[test]
fn every_keyframe_begins_with_the_latest_state_in_order() {
    for (state, prefix) in [
        (Body::CursorHidden(true), vec![Body::CursorHidden(true)]),
        (Body::CursorHidden(false), vec![Body::CursorHidden(false)]),
        (Body::CursorUse(7), vec![shape(7), Body::CursorUse(7)]),
        (Body::CursorUse(99), vec![Body::CursorUse(99)]),
    ] {
        let mut builder = ChunkBuilder::new(meta());
        push(&mut builder, rec(1, Body::Config(config())));
        push(&mut builder, rec(2, shape(7)));
        push(&mut builder, rec(3, cursor_pos()));
        push(&mut builder, video(4, 1, true, 1));
        push(&mut builder, rec(5, state));
        let old = push(&mut builder, video(6, 2, true, 1));
        assert_eq!(old.len(), 1);
        assert!(old[0].keyframe_start);
        assert_eq!(old[0].t_start_us, 4);
        let first = records(&old[0]);
        assert_eq!(first[3].body, shape(7));
        assert_eq!(first[4].body, Body::CursorUse(7));
        let (pending, _) = finish(builder, end());
        let pending = pending.unwrap();
        assert_eq!(pending.t_start_us, 6);
        assert!(pending.keyframe_start);
        let mut expected = vec![rec(6, Body::Config(config())), rec(6, cursor_pos())];
        expected.extend(prefix.into_iter().map(|body| rec(6, body)));
        expected.push(video(6, 2, true, 1));
        assert_eq!(records(&pending), expected);
    }
}

#[test]
fn shape_cache_keeps_the_last_eight_ids_and_replaces_duplicates() {
    let mut builder = ChunkBuilder::new(meta());
    for id in 0..8 {
        push(&mut builder, rec(1, shape(id)));
    }
    let replacement = Body::CursorShape(CursorShapeRec {
        png: vec![8, 9, 10],
        ..match shape(0) {
            Body::CursorShape(shape) => shape,
            _ => unreachable!(),
        }
    });
    push(&mut builder, rec(1, replacement.clone()));
    push(&mut builder, rec(1, shape(8)));
    push(&mut builder, rec(1, Body::CursorUse(1)));
    push(&mut builder, video(2, 1, true, 1));
    let emitted = push(&mut builder, video(3, 2, true, 1));
    assert_eq!(emitted.len(), 1);
    let first = records(&emitted[0]);
    assert_eq!(first.len(), 3);
    assert_eq!(first[1].body, Body::CursorUse(1));
    push(&mut builder, rec(4, Body::CursorUse(0)));
    push(&mut builder, video(5, 3, true, 1));
    let (pending, _) = finish(builder, end());
    assert_eq!(
        records(&pending.unwrap()),
        [
            rec(5, replacement),
            rec(5, Body::CursorUse(0)),
            video(5, 3, true, 1),
        ]
    );
}

#[test]
fn size_boundary_emits_full_chunks_then_continues_without_keyframe_flag() {
    let mut builder = ChunkBuilder::new(meta());
    assert!(push(&mut builder, video(0, 1, true, 1)).is_empty());
    let mut emitted = Vec::new();
    for i in 0..300u32 {
        emitted.extend(push(
            &mut builder,
            video((u64::from(i) + 1) * 1_000, i + 2, false, 4096),
        ));
    }
    assert!(!emitted.is_empty());
    assert_eq!(
        builder.stats().plain_bytes,
        emitted
            .iter()
            .map(|chunk| chunk.plaintext.len() as u64)
            .sum::<u64>()
    );
    assert!(emitted
        .iter()
        .all(|chunk| chunk.plaintext.len() >= CHUNK_TARGET_BYTES));
    let (pending, final_chunk) = finish(builder, end());
    let pending = pending.unwrap();
    assert!(!pending.keyframe_start);
    assert_eq!(pending.t_start_us, records(&pending)[0].t_us);
    emitted.push(pending);
    assert_eq!(
        emitted.iter().filter(|chunk| chunk.keyframe_start).count(),
        1
    );
    assert_eq!(final_end(&final_chunk).frames, 301);
    assert_eq!(
        final_end(&final_chunk).plain_bytes,
        emitted
            .iter()
            .map(|chunk| chunk.plaintext.len() as u64)
            .sum::<u64>()
    );
}

#[test]
fn time_boundary_is_inclusive_and_next_record_starts_a_non_keyframe_chunk() {
    let mut builder = ChunkBuilder::new(meta());
    push(&mut builder, video(100, 1, true, 1));
    assert!(push(&mut builder, audio(100 + CHUNK_MAX_SPAN_US - 1)).is_empty());
    let chunks = push(&mut builder, audio(100 + CHUNK_MAX_SPAN_US));
    assert_eq!(chunks.len(), 1);
    assert_eq!(chunks[0].t_start_us, 100);
    assert!(chunks[0].keyframe_start);
    let next = 101 + CHUNK_MAX_SPAN_US;
    assert!(push(&mut builder, audio(next)).is_empty());
    let (pending, final_chunk) = finish(builder, end());
    let pending = pending.unwrap();
    assert!(!pending.keyframe_start);
    assert_eq!(pending.t_start_us, next);
    assert_eq!(records(&pending), [audio(next)]);
    assert_eq!(final_end(&final_chunk).audio_packets, 3);
}

#[test]
fn oversized_keyframes_suppress_deltas_before_and_after_recording_starts() {
    let mut builder = ChunkBuilder::new(meta());
    let oversized = CHUNK_MAX_BYTES / 2 + 1;
    assert!(push(&mut builder, video(1, 1, true, oversized)).is_empty());
    assert!(push(&mut builder, video(2, 2, false, 1)).is_empty());
    assert!(push(&mut builder, video(3, 3, false, 1)).is_empty());
    assert_eq!(builder.stats().dropped, 3);
    assert!(push(&mut builder, video(4, 4, true, 1)).is_empty());
    assert!(push(&mut builder, video(5, 5, true, oversized)).is_empty());
    for i in 6..9 {
        assert!(push(&mut builder, video(i, i as u32, false, 1)).is_empty());
    }
    assert!(push(&mut builder, audio(9)).is_empty());
    assert_eq!(builder.stats().dropped, 7);
    assert_eq!(builder.stats().frames, 1);
    assert_eq!(push(&mut builder, video(10, 10, true, 1)).len(), 1);
    assert!(push(&mut builder, video(11, 11, false, oversized)).is_empty());
    assert!(push(&mut builder, video(12, 12, false, 1)).is_empty());
    let (pending, final_chunk) = finish(builder, end());
    assert_eq!(
        records(&pending.unwrap()),
        [video(10, 10, true, 1), video(12, 12, false, 1)]
    );
    let ended = final_end(&final_chunk);
    assert_eq!(ended.dropped, 8);
    assert_eq!(ended.frames, 3);
    assert_eq!(ended.keyframes, 2);
    assert_eq!(ended.audio_packets, 1);
}

#[test]
fn record_size_limit_counts_encoded_headers_and_full_keyframes_can_emit_two_chunks() {
    let mut builder = ChunkBuilder::new(meta());
    push(&mut builder, video(1, 1, true, 1));
    let max_payload = CHUNK_MAX_BYTES / 2 - HEADER_LEN - 24;
    let chunks = push(&mut builder, video(2, 2, true, max_payload));
    assert_eq!(chunks.len(), 2);
    assert_eq!(chunks[0].t_start_us, 1);
    assert_eq!(chunks[1].t_start_us, 2);
    assert!(chunks.iter().all(|chunk| chunk.keyframe_start));
    assert_eq!(chunks[1].plaintext.len(), CHUNK_MAX_BYTES / 2);
    assert!(push(&mut builder, video(3, 3, false, max_payload + 1)).is_empty());
    let (pending, final_chunk) = finish(builder, end());
    assert!(pending.is_none());
    assert_eq!(final_end(&final_chunk).dropped, 1);
    assert_eq!(final_end(&final_chunk).frames, 2);
}

#[test]
fn oversized_state_updates_the_cache_but_is_not_written_again_in_a_prelude() {
    let mut builder = ChunkBuilder::new(meta());
    push(
        &mut builder,
        rec(
            1,
            Body::CursorShape(CursorShapeRec {
                png: vec![1; CHUNK_MAX_BYTES / 2],
                ..match shape(7) {
                    Body::CursorShape(shape) => shape,
                    _ => unreachable!(),
                }
            }),
        ),
    );
    assert_eq!(builder.stats().dropped, 1);
    push(&mut builder, video(2, 1, true, 1));
    let (pending, final_chunk) = finish(builder, end());
    assert_eq!(
        records(&pending.unwrap()),
        [
            rec(0, Body::Meta(meta())),
            rec(2, Body::CursorUse(7)),
            video(2, 1, true, 1),
        ]
    );
    assert_eq!(final_end(&final_chunk).dropped, 1);
}

#[test]
fn keyframe_and_state_prelude_together_never_exceed_the_chunk_cap() {
    let mut builder = ChunkBuilder::new(meta());
    push(
        &mut builder,
        rec(
            0,
            Body::CursorShape(CursorShapeRec {
                png: vec![1; CHUNK_MAX_BYTES / 2 - HEADER_LEN - 28],
                ..match shape(7) {
                    Body::CursorShape(shape) => shape,
                    _ => unreachable!(),
                }
            }),
        ),
    );
    let frame_size = CHUNK_MAX_BYTES / 2 - HEADER_LEN - 24;
    assert!(push(&mut builder, video(1, 1, true, frame_size)).is_empty());
    assert_eq!(builder.stats().dropped, 1);
    assert!(push(&mut builder, video(2, 2, false, 1)).is_empty());
    assert_eq!(builder.stats().dropped, 2);
    push(&mut builder, rec(3, Body::CursorHidden(true)));
    assert_eq!(push(&mut builder, video(4, 4, true, frame_size)).len(), 1);
    let (_, final_chunk) = finish(builder, end());
    assert_eq!(final_end(&final_chunk).frames, 1);
}

#[test]
fn backwards_timestamps_use_the_previous_input_time_even_when_it_was_dropped() {
    let mut builder = ChunkBuilder::new(meta());
    push(&mut builder, video(50, 0, false, 1));
    push(&mut builder, video(10, 1, true, 1));
    push(&mut builder, audio(5));
    push(&mut builder, video(40, 2, false, 1));
    assert_eq!(builder.stats().last_t_us, 50);
    let (pending, final_chunk) = finish(builder, end());
    let pending = pending.unwrap();
    assert_eq!(pending.t_start_us, 50);
    let decoded = records(&pending);
    assert_eq!(
        decoded.iter().map(|record| record.t_us).collect::<Vec<_>>(),
        [0, 50, 50, 50]
    );
    match &decoded[1].body {
        Body::Video(video) => assert_eq!(video.capture_ts_us, 10),
        _ => panic!("expected video"),
    }
    assert_eq!(final_end(&final_chunk).duration_us, 50);
    assert_eq!(final_chunk.t_start_us, 50);
    assert_eq!(records(&final_chunk)[0].t_us, 50);
}

#[test]
fn finish_overrides_every_statistic_and_preserves_caller_fields() {
    let mut builder = ChunkBuilder::new(meta());
    push(&mut builder, video(1, 0, false, 1));
    push(&mut builder, rec(2, Body::Config(config())));
    push(&mut builder, video(3, 1, true, 1));
    push(&mut builder, audio(4));
    push(
        &mut builder,
        rec(
            5,
            Body::Gap {
                reason: GapReason::Reconnect,
                real_us: 900,
            },
        ),
    );
    let mut latest_config = config();
    latest_config.width = 1280;
    latest_config.height = 720;
    latest_config.codec = 2;
    push(&mut builder, rec(6, Body::Config(latest_config)));
    let emitted = push(&mut builder, video(7, 2, true, 1));
    let expected = builder.stats().clone();
    assert_eq!(
        (
            expected.frames,
            expected.keyframes,
            expected.audio_packets,
            expected.gaps,
            expected.dropped
        ),
        (2, 2, 1, 1, 1)
    );
    assert_eq!(expected.plain_bytes, emitted[0].plaintext.len() as u64);
    let (pending, final_chunk) = finish(
        builder,
        RecordingEnd {
            duration_us: 999,
            frames: 999,
            keyframes: 999,
            audio_packets: 999,
            gaps: 999,
            dropped: 999,
            plain_bytes: 999,
            ended_at_us: 123,
            reason: "diskLow".into(),
            width: 999,
            height: 999,
            codec: 99,
        },
    );
    let ended = final_end(&final_chunk);
    assert_eq!(
        ended,
        RecordingEnd {
            duration_us: expected.last_t_us,
            frames: expected.frames,
            keyframes: expected.keyframes,
            audio_packets: expected.audio_packets,
            gaps: expected.gaps,
            dropped: expected.dropped,
            plain_bytes: expected.plain_bytes + pending.unwrap().plaintext.len() as u64,
            ended_at_us: 123,
            reason: "diskLow".into(),
            width: 1280,
            height: 720,
            codec: 2,
        }
    );
}

#[test]
fn finish_without_a_keyframe_returns_only_a_final_chunk() {
    let mut builder = ChunkBuilder::new(meta());
    push(&mut builder, rec(50, Body::Config(config())));
    push(&mut builder, video(60, 1, false, 1));
    let (pending, final_chunk) = finish(builder, end());
    assert!(pending.is_none());
    let ended = final_end(&final_chunk);
    assert_eq!(ended.frames, 0);
    assert_eq!(ended.duration_us, 0);
    assert_eq!(ended.dropped, 1);
    assert_eq!(ended.plain_bytes, 0);
    assert_eq!(ended.width, 1920);
}
