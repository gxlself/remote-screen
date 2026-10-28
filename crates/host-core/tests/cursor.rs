use host_core::platform::CursorUpdate;
use host_core::session::cursor::{
    ConnCursor, CursorChange, CursorHub, CursorOut, ECHO_WINDOW_US, POINTER_INTERVAL_US,
};
use rsp_core::cursor::{
    shape_id, CursorEvent, CursorState, CursorStreamDecoder, SHAPE_CACHE_CAPACITY,
};
use rsp_core::features::{CURSOR_SHAPE, POINTER_DGRAM};
use rsp_core::pb::cursor_msg::Msg;
use rsp_core::pb::{CursorMsg, CursorShape};
use rsp_core::pointer::{PointerDatagram, PointerFilter};

const ALL: u64 = CURSOR_SHAPE | POINTER_DGRAM;
const HOST_DISPLAY: u32 = 69_733_248;
const OTHER_DISPLAY: u32 = 69_733_249;
const WIRE_DISPLAY: u8 = 1;

fn shape(tag: u8) -> CursorShape {
    let png = vec![tag, tag.wrapping_add(1), 0x50, 0x4e, 0x47];
    CursorShape {
        id: shape_id(&png),
        width: 16,
        height: 18,
        hot_x: 2,
        hot_y: 3,
        scale: 2.0,
        png,
    }
}

fn position(display_id: u32, x: f32, y: f32, visible: bool) -> CursorUpdate {
    CursorUpdate::Position {
        display_id,
        seq: 99,
        x,
        y,
        visible,
    }
}

fn decode_streams(outputs: &[CursorOut]) -> Vec<CursorMsg> {
    let mut decoder = CursorStreamDecoder::new();
    let mut messages = Vec::new();
    for output in outputs {
        if let CursorOut::Stream(record) = output {
            decoder.push(record);
            while let Some(message) = decoder.next_msg().expect("cursor stream decodes") {
                messages.push(message);
            }
        }
    }
    assert!(decoder
        .next_msg()
        .expect("cursor stream ends cleanly")
        .is_none());
    messages
}

fn only_datagram(outputs: &[CursorOut]) -> PointerDatagram {
    let [CursorOut::Datagram(datagram)] = outputs else {
        panic!("expected exactly one pointer datagram, got {outputs:?}");
    };
    PointerDatagram::decode(datagram).expect("pointer datagram decodes")
}

fn apply_change(
    conn: &mut ConnCursor,
    hub: &mut CursorHub,
    update: CursorUpdate,
    now_us: u64,
) -> Vec<CursorOut> {
    let change = hub.apply(update).expect("update changes Hub state");
    conn.on_change(&change, hub, now_us)
}

fn assert_shape(message: &CursorMsg, expected: &CursorShape) {
    assert_eq!(
        message,
        &CursorMsg {
            msg: Some(Msg::Shape(expected.clone())),
        }
    );
}

fn assert_use(message: &CursorMsg, expected_id: u64) {
    assert_eq!(
        message,
        &CursorMsg {
            msg: Some(Msg::Use(expected_id)),
        }
    );
}

fn assert_hidden(message: &CursorMsg, expected: bool) {
    assert_eq!(
        message,
        &CursorMsg {
            msg: Some(Msg::Hidden(expected)),
        }
    );
}

#[test]
fn hub_deduplicates_shapes_and_rejects_unknown_uses() {
    let s1 = shape(1);
    let s2 = shape(2);
    let id1 = s1.id;
    let id2 = s2.id;
    let mut hub = CursorHub::new();

    assert_eq!(
        hub.apply(CursorUpdate::Shape(s1)),
        Some(CursorChange::Shape(id1))
    );
    assert_eq!(hub.current_shape(), Some(id1));
    assert_eq!(hub.apply(CursorUpdate::Use(id1)), None);
    assert_eq!(hub.apply(CursorUpdate::Use(u64::MAX)), None);
    assert_eq!(hub.current_shape(), Some(id1));
    assert_eq!(
        hub.apply(CursorUpdate::Shape(s2)),
        Some(CursorChange::Shape(id2))
    );
    assert_eq!(
        hub.apply(CursorUpdate::Use(id1)),
        Some(CursorChange::Shape(id1))
    );
    assert_eq!(hub.current_shape(), Some(id1));
}

#[test]
fn attach_sends_current_shape_and_hidden_state() {
    let s1 = shape(3);
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);

    assert!(conn.attach(&mut hub).is_empty());

    hub.apply(CursorUpdate::Shape(s1.clone()));
    let outputs = conn.attach(&mut hub);
    let messages = decode_streams(&outputs);
    assert_eq!(messages.len(), 1);
    assert_shape(&messages[0], &s1);

    hub.apply(CursorUpdate::Hidden(true));
    let messages = decode_streams(&conn.attach(&mut hub));
    assert_eq!(messages.len(), 2);
    assert_shape(&messages[0], &s1);
    assert_hidden(&messages[1], true);
}

#[test]
fn shape_switch_uses_full_shape_then_cache_reference() {
    let s1 = shape(4);
    let s2 = shape(5);
    let id1 = s1.id;
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);

    hub.apply(CursorUpdate::Shape(s1.clone()));
    assert_eq!(decode_streams(&conn.attach(&mut hub)).len(), 1);

    let messages = decode_streams(&apply_change(
        &mut conn,
        &mut hub,
        CursorUpdate::Shape(s2.clone()),
        0,
    ));
    assert_eq!(messages.len(), 1);
    assert_shape(&messages[0], &s2);

    let messages = decode_streams(&apply_change(
        &mut conn,
        &mut hub,
        CursorUpdate::Use(id1),
        0,
    ));
    assert_eq!(messages.len(), 1);
    assert_use(&messages[0], id1);
}

#[test]
fn client_simulation_never_misses_and_resends_after_eviction() {
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);
    let mut client = CursorState::new();

    let first = shape(0);
    hub.apply(CursorUpdate::Shape(first.clone()));
    for message in decode_streams(&conn.attach(&mut hub)) {
        assert!(!matches!(client.apply(message), CursorEvent::Missing(_)));
    }

    let mut shapes = vec![first];
    for tag in 1..=65 {
        let next = shape(tag);
        shapes.push(next.clone());
        let outputs = apply_change(&mut conn, &mut hub, CursorUpdate::Shape(next), 0);
        for message in decode_streams(&outputs) {
            assert!(!matches!(client.apply(message), CursorEvent::Missing(_)));
        }
    }

    let outputs = apply_change(&mut conn, &mut hub, CursorUpdate::Use(shapes[0].id), 0);
    let messages = decode_streams(&outputs);
    assert_eq!(messages.len(), 1);
    assert_shape(&messages[0], &shapes[0]);
    for message in messages {
        assert!(!matches!(client.apply(message), CursorEvent::Missing(_)));
    }
    assert_eq!(client.cache().len(), SHAPE_CACHE_CAPACITY);

    for tag in 66..70 {
        let next = shape(tag);
        shapes.push(next.clone());
        let outputs = apply_change(&mut conn, &mut hub, CursorUpdate::Shape(next), 0);
        for message in decode_streams(&outputs) {
            assert!(!matches!(client.apply(message), CursorEvent::Missing(_)));
        }
    }
    for index in (1..shapes.len() - 1).rev() {
        let outputs = apply_change(&mut conn, &mut hub, CursorUpdate::Use(shapes[index].id), 0);
        for message in decode_streams(&outputs) {
            assert!(!matches!(client.apply(message), CursorEvent::Missing(_)));
        }
    }
}

#[test]
fn reattach_always_sends_a_complete_shape() {
    let s1 = shape(6);
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);
    hub.apply(CursorUpdate::Shape(s1.clone()));

    assert_shape(&decode_streams(&conn.attach(&mut hub))[0], &s1);
    assert_shape(&decode_streams(&conn.attach(&mut hub))[0], &s1);
}

#[test]
fn feature_bits_gate_streams_and_datagrams_independently() {
    let s1 = shape(7);
    let s2 = shape(8);
    let mut hub = CursorHub::new();

    let mut pointer_only = ConnCursor::new(POINTER_DGRAM, false);
    pointer_only.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    hub.apply(CursorUpdate::Shape(s1));
    assert!(pointer_only.attach(&mut hub).is_empty());
    assert!(
        apply_change(&mut pointer_only, &mut hub, CursorUpdate::Shape(s2), 0)
            .iter()
            .all(|out| !matches!(out, CursorOut::Stream(_)))
    );
    let outputs = apply_change(
        &mut pointer_only,
        &mut hub,
        position(HOST_DISPLAY, 0.2, 0.7, true),
        1_000,
    );
    assert_eq!(only_datagram(&outputs).display, WIRE_DISPLAY);

    let mut shape_only = ConnCursor::new(CURSOR_SHAPE, false);
    shape_only.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    assert_eq!(decode_streams(&shape_only.attach(&mut hub)).len(), 1);
    let outputs = apply_change(
        &mut shape_only,
        &mut hub,
        position(HOST_DISPLAY, 0.3, 0.6, true),
        2_000,
    );
    assert!(outputs
        .iter()
        .all(|out| !matches!(out, CursorOut::Datagram(_))));
}

#[test]
fn pointer_datagram_uses_wire_display_and_per_connection_sequence() {
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);
    conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);

    let outputs = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.25, 0.75, true),
        1_000,
    );
    let datagram = only_datagram(&outputs);
    assert_eq!(
        datagram,
        PointerDatagram {
            seq: 1,
            display: WIRE_DISPLAY,
            visible: true,
            x: 0.25,
            y: 0.75,
        }
    );
}

#[test]
fn pointer_rate_limit_keeps_only_the_newest_pending_position() {
    let t0 = 1_000_000;
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);
    conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);

    let first = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.1, 0.1, true),
        t0,
    );
    assert_eq!(only_datagram(&first).seq, 1);

    let second = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.2, 0.2, true),
        t0 + 5_000,
    );
    assert!(second.is_empty());
    assert_eq!(conn.next_deadline(), Some(t0 + POINTER_INTERVAL_US));

    let third = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.3, 0.3, true),
        t0 + 10_000,
    );
    assert!(third.is_empty());
    assert_eq!(conn.next_deadline(), Some(t0 + POINTER_INTERVAL_US));

    let due = conn.poll(t0 + POINTER_INTERVAL_US);
    let datagram = only_datagram(&due);
    assert_eq!(datagram.seq, 2);
    assert!((datagram.x - 0.3).abs() < 1e-6);
    assert!((datagram.y - 0.3).abs() < 1e-6);
    assert_eq!(conn.next_deadline(), None);
    assert!(conn.poll(t0 + POINTER_INTERVAL_US).is_empty());
}

#[test]
fn switching_display_discards_pending_pointer_for_the_old_screen() {
    let t0 = 5_000_000;
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);
    conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);

    let first = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.1, 0.1, true),
        t0,
    );
    assert_eq!(only_datagram(&first).seq, 1);
    let pending = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.2, 0.2, true),
        t0 + 5_000,
    );
    assert!(pending.is_empty());
    assert_eq!(conn.next_deadline(), Some(t0 + POINTER_INTERVAL_US));

    conn.set_stream_display(OTHER_DISPLAY, 2);
    assert_eq!(conn.next_deadline(), None);
    assert!(conn.poll(t0 + POINTER_INTERVAL_US).is_empty());

    let next = apply_change(
        &mut conn,
        &mut hub,
        position(OTHER_DISPLAY, 0.6, 0.5, true),
        t0 + 20_000,
    );
    let datagram = only_datagram(&next);
    assert_eq!(datagram.display, 2);
    assert_eq!(datagram.seq, 2);
}

#[test]
fn leaving_the_streamed_display_sends_one_invisible_datagram() {
    let t0 = 2_000_000;
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);
    conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);

    let before = apply_change(
        &mut conn,
        &mut hub,
        position(OTHER_DISPLAY, 0.8, 0.8, true),
        t0,
    );
    assert!(before.is_empty());

    let on_screen = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.4, 0.6, true),
        t0 + 1,
    );
    assert!(only_datagram(&on_screen).visible);

    let leaving = apply_change(
        &mut conn,
        &mut hub,
        position(OTHER_DISPLAY, 0.9, 0.9, true),
        t0 + 1 + POINTER_INTERVAL_US,
    );
    let datagram = only_datagram(&leaving);
    assert_eq!(datagram.display, WIRE_DISPLAY);
    assert!(!datagram.visible);
    assert_eq!((datagram.x, datagram.y), (0.4, 0.6));

    let still_away = apply_change(
        &mut conn,
        &mut hub,
        position(OTHER_DISPLAY, 0.7, 0.7, true),
        t0 + 1 + 2 * POINTER_INTERVAL_US,
    );
    assert!(still_away.is_empty());

    let returned = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.5, 0.5, true),
        t0 + 1 + 3 * POINTER_INTERVAL_US,
    );
    let datagram = only_datagram(&returned);
    assert!(datagram.visible);
    assert_eq!((datagram.x, datagram.y), (0.5, 0.5));
}

#[test]
fn injected_pointer_echo_is_suppressed_only_for_the_controller() {
    let t = 3_000_000;
    let mut echo_hub = CursorHub::new();
    let mut echo_conn = ConnCursor::new(ALL, true);
    echo_conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    echo_conn.note_injected(0.5, 0.5, t);
    let near_echo = echo_conn.on_change(
        &CursorChange::Position {
            display_id: HOST_DISPLAY,
            x: 0.505,
            y: 0.5,
            visible: true,
        },
        &mut echo_hub,
        t + 50_000,
    );
    assert!(near_echo.is_empty());

    let mut late_hub = CursorHub::new();
    let mut late_conn = ConnCursor::new(ALL, true);
    late_conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    late_conn.note_injected(0.5, 0.5, t);
    let late_echo = late_conn.on_change(
        &CursorChange::Position {
            display_id: HOST_DISPLAY,
            x: 0.505,
            y: 0.5,
            visible: true,
        },
        &mut late_hub,
        t + ECHO_WINDOW_US,
    );
    assert_eq!(only_datagram(&late_echo).seq, 1);

    let mut moved_hub = CursorHub::new();
    let mut moved_conn = ConnCursor::new(ALL, true);
    moved_conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    moved_conn.note_injected(0.5, 0.5, t);
    let moved = moved_conn.on_change(
        &CursorChange::Position {
            display_id: HOST_DISPLAY,
            x: 0.8,
            y: 0.5,
            visible: true,
        },
        &mut moved_hub,
        t + 50_000,
    );
    assert_eq!(only_datagram(&moved).seq, 1);

    let mut viewer_hub = CursorHub::new();
    let mut viewer = ConnCursor::new(ALL, false);
    viewer.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    viewer.note_injected(0.5, 0.5, t);
    let mut seqs = Vec::new();
    for (at, x) in [
        (t + 50_000, 0.505),
        (t + 200_000, 0.505),
        (t + 250_000, 0.8),
    ] {
        let outputs = viewer.on_change(
            &CursorChange::Position {
                display_id: HOST_DISPLAY,
                x,
                y: 0.5,
                visible: true,
            },
            &mut viewer_hub,
            at,
        );
        seqs.push(only_datagram(&outputs).seq);
    }
    assert_eq!(seqs, [1, 2, 3]);
}

#[test]
fn sequence_wraparound_is_accepted_by_pointer_filter() {
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false).with_initial_seq(u32::MAX);
    conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    let mut filter = PointerFilter::new();

    let first = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.1, 0.2, true),
        4_000_000,
    );
    let second = apply_change(
        &mut conn,
        &mut hub,
        position(HOST_DISPLAY, 0.2, 0.3, true),
        4_000_000 + POINTER_INTERVAL_US,
    );
    let first = only_datagram(&first);
    let second = only_datagram(&second);

    assert_eq!(first.seq, u32::MAX);
    assert_eq!(second.seq, 0);
    assert!(filter.accept(&first));
    assert!(filter.accept(&second));
}

#[test]
fn hidden_changes_are_sent_and_repeated_values_are_ignored() {
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);

    let change = hub
        .apply(CursorUpdate::Hidden(true))
        .expect("hidden changes");
    let outputs = conn.on_change(&change, &mut hub, 0);
    let messages = decode_streams(&outputs);
    assert_eq!(messages.len(), 1);
    assert_hidden(&messages[0], true);

    let repeated = hub.apply(CursorUpdate::Hidden(true));
    assert_eq!(repeated, None);
    let outputs = repeated
        .map(|change| conn.on_change(&change, &mut hub, 0))
        .unwrap_or_default();
    assert!(outputs.is_empty());
}

#[test]
fn oversized_shapes_are_dropped_without_changing_the_current_shape() {
    let small = shape(10);
    let id = small.id;
    let oversized_png = vec![0x42; (rsp_core::cursor::MAX_CURSOR_MESSAGE_LEN) + 1];
    let oversized = CursorShape {
        id: shape_id(&oversized_png),
        width: 512,
        height: 512,
        hot_x: 0,
        hot_y: 0,
        scale: 1.0,
        png: oversized_png,
    };
    let oversized_id = oversized.id;
    let mut hub = CursorHub::new();

    assert_eq!(
        hub.apply(CursorUpdate::Shape(small)),
        Some(CursorChange::Shape(id))
    );
    assert_eq!(hub.apply(CursorUpdate::Shape(oversized)), None);
    assert_eq!(hub.current_shape(), Some(id));
    assert!(hub.shape(oversized_id).is_none());
}

#[test]
fn host_shape_store_keeps_more_than_the_client_mirror() {
    let mut hub = CursorHub::new();
    let mut first = None;
    for tag in 0..=64 {
        let next = shape(tag);
        if tag == 0 {
            first = Some(next.clone());
        }
        hub.apply(CursorUpdate::Shape(next));
    }
    let first = first.expect("first shape");
    assert_eq!(hub.current_shape(), Some(shape(64).id));
    assert_eq!(hub.shape(first.id), Some(first.clone()));

    let mut conn = ConnCursor::new(CURSOR_SHAPE, false);
    conn.attach(&mut hub);
    let messages = decode_streams(&apply_change(
        &mut conn,
        &mut hub,
        CursorUpdate::Use(first.id),
        0,
    ));
    assert_eq!(messages.len(), 1);
    assert_shape(&messages[0], &shape(0));
}

#[test]
fn stream_config_reports_cursor_only_on_the_streamed_display() {
    let mut hub = CursorHub::new();
    let mut conn = ConnCursor::new(ALL, false);
    assert_eq!(conn.stream_config_cursor(&hub), (0.5, 0.5, false));

    conn.set_stream_display(HOST_DISPLAY, WIRE_DISPLAY);
    hub.apply(position(HOST_DISPLAY, 0.2, 0.8, true));
    assert_eq!(conn.stream_config_cursor(&hub), (0.2, 0.8, true));

    hub.apply(position(OTHER_DISPLAY, 0.3, 0.7, true));
    assert_eq!(conn.stream_config_cursor(&hub), (0.5, 0.5, false));
}
