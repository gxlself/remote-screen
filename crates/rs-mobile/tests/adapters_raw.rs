mod support;

use std::{
    ffi::{c_void, CStr},
    io::Cursor,
    ptr, slice,
    sync::{
        atomic::{AtomicBool, AtomicU8, AtomicUsize, Ordering},
        Arc, Barrier, Mutex, MutexGuard,
    },
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

use rs_mobile::rscore::{
    adapters::{
        cursor::{encode_png, CursorState},
        driver::{FrameSink, SrcPixels},
        ffi_raw::*,
        rawframe::{RawConsumer, RawFrame, RawRegistry},
        ADAPTER_ID_BASE,
    },
    ffi_session::{
        RsEvent, RS_EV_CLOSED, RS_EV_CURSOR_HIDDEN, RS_EV_CURSOR_POS, RS_EV_CURSOR_SHAPE,
        RS_EV_CURSOR_USE, RS_EV_STREAM_CONFIG,
    },
    runtime::{install_core, take_core, Core},
};

#[derive(Debug, PartialEq)]
enum Seen {
    Ready(u64, u32, u32),
    Config {
        size: (u32, u32),
        epoch: u32,
        codec: u8,
        fps: u32,
        display_id: u32,
        visible: u8,
        cursor: (f32, f32),
        src: [f32; 4],
        display: (u32, u32),
    },
    Shape {
        id: u64,
        size: (u32, u32),
        hot: (u32, u32),
        scale: f32,
        png: Vec<u8>,
    },
    Use(u64),
    Hidden(u8),
    Pos(f32, f32, u8),
    Closed(String),
}

#[derive(Default)]
struct Rec {
    log: Mutex<Vec<Seen>>,
}

impl Rec {
    fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

impl RawConsumer for Rec {
    fn ready(&self, seq: u64, width: u32, height: u32) {
        self.log
            .lock()
            .unwrap()
            .push(Seen::Ready(seq, width, height));
    }

    fn event(&self, e: &RsEvent) {
        let event = match e.kind {
            RS_EV_STREAM_CONFIG => Seen::Config {
                size: (e.width, e.height),
                epoch: e.epoch,
                codec: e.codec,
                fps: e.fps,
                display_id: e.display_id,
                visible: e.cursor_visible,
                cursor: (e.cursor_x, e.cursor_y),
                src: [e.src_x, e.src_y, e.src_w, e.src_h],
                display: (e.display_w, e.display_h),
            },
            RS_EV_CURSOR_SHAPE => Seen::Shape {
                id: e.shape_id,
                size: (e.width, e.height),
                hot: (e.hot_x, e.hot_y),
                scale: e.scale,
                // SAFETY: RawConsumer borrows valid event data for this callback;
                // copy it before returning and before the cursor cache changes.
                png: unsafe { slice::from_raw_parts(e.data, e.data_len).to_vec() },
            },
            RS_EV_CURSOR_USE => Seen::Use(e.shape_id),
            RS_EV_CURSOR_HIDDEN => Seen::Hidden(e.cursor_visible),
            RS_EV_CURSOR_POS => Seen::Pos(e.cursor_x, e.cursor_y, e.cursor_visible),
            RS_EV_CLOSED => Seen::Closed(
                // SAFETY: The close reason is a live CString for this callback.
                unsafe { CStr::from_ptr(e.message) }
                    .to_str()
                    .unwrap()
                    .to_owned(),
            ),
            _ => panic!("unexpected raw event {}", e.kind),
        };
        self.log.lock().unwrap().push(event);
    }
}

fn rect(x: u32, y: u32, w: u32, h: u32) -> RsRect {
    RsRect { x, y, w, h }
}

fn config(epoch: u32) -> Seen {
    Seen::Config {
        size: (4, 3),
        epoch,
        codec: 0,
        fps: 0,
        display_id: 0,
        visible: 1,
        cursor: (0.5, 0.5),
        src: [0.0, 0.0, 1.0, 1.0],
        display: (0, 0),
    }
}

fn committed_frame() -> Arc<RawFrame> {
    let frame = RawFrame::new();
    frame.resize(4, 3);
    frame.commit();
    frame
}

fn pixels(frame: &RawFrame) -> Vec<u8> {
    let (w, h) = frame.size();
    let mut pixels = vec![0xEE; w as usize * h as usize * 4];
    assert_eq!(
        frame.copy_into(0, &mut pixels, w as usize * 4, w, h, RS_PIX_BGRA8),
        (frame.seq(), rect(0, 0, w, h))
    );
    pixels
}

#[test]
fn resize_emits_config() {
    let frame = RawFrame::new();
    assert_eq!((frame.seq(), frame.size(), frame.epoch()), (0, (0, 0), 0));
    frame.resize(4, 3);
    let rec = Arc::new(Rec::default());
    assert!(frame.attach(rec.clone()));
    assert!(frame.is_attached());
    assert!(!frame.is_closed());
    assert_eq!(rec.take(), [config(1)]);
    assert_eq!((frame.seq(), frame.size(), frame.epoch()), (0, (4, 3), 1));
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(0, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (0, RsRect::default())
    );
    assert_eq!(dst, [0xEE; 48]);
}

#[test]
fn commit_without_writes_is_noop() {
    let frame = RawFrame::new();
    let rec = Arc::new(Rec::default());
    frame.attach(rec.clone());
    frame.commit();
    assert!(rec.take().is_empty());
    frame.resize(4, 3);
    assert_eq!(rec.take(), [config(1)]);
    frame.commit();
    assert_eq!(frame.seq(), 1);
    assert_eq!(rec.take(), [Seen::Ready(1, 4, 3)]);
    assert_eq!(pixels(&frame), [0, 0, 0, 255].repeat(12));
    frame.commit();
    assert_eq!(frame.seq(), 1);
    assert!(rec.take().is_empty());
}

#[test]
fn put_fill_commit_ready() {
    let frame = committed_frame();
    let rec = Arc::new(Rec::default());
    frame.attach(rec.clone());
    rec.take();
    frame.fill(1, 1, 2, 1, [1, 2, 3, 255]);
    frame.commit();
    assert_eq!(rec.take(), [Seen::Ready(2, 4, 3)]);
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(1, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (2, rect(1, 1, 2, 1))
    );
    let mut expected = [0xEE; 48];
    expected[20..28].copy_from_slice(&[1, 2, 3, 255].repeat(2));
    assert_eq!(dst, expected);
}

#[test]
fn rgba_source_and_rgba_copy() {
    let frame = committed_frame();
    frame.put(0, 0, 1, 1, &[10, 20, 30, 127], 4, SrcPixels::Rgba8);
    frame.commit();
    assert_eq!(&pixels(&frame)[..4], &[30, 20, 10, 127]);
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(1, &mut dst, 16, 4, 3, RS_PIX_RGBA8),
        (2, rect(0, 0, 1, 1))
    );
    assert_eq!(&dst[..4], &[10, 20, 30, 127]);
    assert_eq!(&dst[4..], &[0xEE; 44]);
}

#[test]
fn full_copy_cases() {
    let frame = committed_frame();
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(0, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (1, rect(0, 0, 4, 3))
    );
    for n in 0..17 {
        frame.fill(1, 1, 1, 1, [n, 2, 3, 255]);
        frame.commit();
    }
    assert_eq!(
        frame.copy_into(frame.seq() - 17, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (18, rect(0, 0, 4, 3))
    );
    assert_eq!(
        frame.copy_into(frame.seq() - 16, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (18, rect(1, 1, 1, 1))
    );
    frame.resize(4, 3);
    frame.commit();
    assert_eq!(
        frame.copy_into(18, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (19, rect(0, 0, 4, 3))
    );
    assert_eq!(frame.epoch(), 2);
    assert_eq!(dst.to_vec(), [0, 0, 0, 255].repeat(12));
}

#[test]
fn dirty_history_union_and_resize_boundary() {
    let frame = committed_frame();
    frame.fill(1, 0, 1, 1, [1; 4]);
    frame.commit();
    frame.fill(2, 2, 1, 1, [2; 4]);
    frame.commit();
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(1, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (3, rect(1, 0, 2, 3))
    );
    for y in 0..3 {
        assert_eq!(&dst[y * 16..y * 16 + 4], &[0xEE; 4]);
        assert_eq!(&dst[y * 16 + 12..y * 16 + 16], &[0xEE; 4]);
    }
    assert_eq!(&dst[4..8], &[1; 4]);
    assert_eq!(&dst[20..28], &[0, 0, 0, 255].repeat(2));
    assert_eq!(&dst[40..44], &[2; 4]);
    frame.resize(4, 3);
    frame.commit();
    frame.fill(2, 1, 1, 1, [3; 4]);
    frame.commit();
    assert_eq!(
        frame.copy_into(3, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (5, rect(0, 0, 4, 3))
    );
    assert_eq!(
        frame.copy_into(4, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (5, rect(2, 1, 1, 1))
    );
}

#[test]
fn nothing_changed() {
    let frame = committed_frame();
    for since in [frame.seq(), u64::MAX] {
        let mut dst = [0xEE; 48];
        assert_eq!(
            frame.copy_into(since, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
            (frame.seq(), RsRect::default())
        );
        assert_eq!(dst, [0xEE; 48]);
    }
}

#[test]
fn bad_dst() {
    let frame = committed_frame();
    for (stride, w, h, pix, len) in [
        (16, 3, 3, RS_PIX_BGRA8, 48),
        (16, 4, 2, RS_PIX_BGRA8, 48),
        (15, 4, 3, RS_PIX_BGRA8, 48),
        (16, 4, 3, 3, 48),
        (16, 4, 3, RS_PIX_BGRA8, 47),
        (usize::MAX, 4, 3, RS_PIX_BGRA8, 48),
        (16, 0, 0, RS_PIX_BGRA8, 48),
    ] {
        let mut dst = vec![0xEE; len];
        assert_eq!(
            frame.copy_into(0, &mut dst, stride, w, h, pix),
            (0, RsRect::default())
        );
        assert_eq!(dst, vec![0xEE; len]);
    }
}

#[test]
fn clip_and_overlap() {
    let frame = committed_frame();
    frame.fill(3, 2, 5, 5, [1, 2, 3, 255]);
    frame.commit();
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(1, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (2, rect(3, 2, 1, 1))
    );
    assert_eq!(&dst[44..], &[1, 2, 3, 255]);
    assert_eq!(&dst[..44], &[0xEE; 44]);
    frame.fill(10, 10, 1, 1, [9; 4]);
    frame.fill(u32::MAX, 0, u32::MAX, 1, [9; 4]);
    frame.copy_within(10, 10, 0, 0, 1, 1);
    frame.commit();
    assert_eq!(frame.seq(), 2);
    let row: Vec<_> = (1..=4).flat_map(|n| [n, n, n, 255]).collect();
    frame.put(0, 0, 4, 1, &row, 16, SrcPixels::Bgra8);
    frame.copy_within(0, 0, 1, 0, 3, 1);
    frame.commit();
    let copied = pixels(&frame);
    assert_eq!(&copied[..4], &row[..4]);
    assert_eq!(&copied[4..16], &row[..12]);
}

const RGBA: [u8; 16] = [
    10, 20, 30, 255, 40, 50, 60, 127, 70, 80, 90, 0, 1, 2, 3, 255,
];

fn assert_shape(event: &Seen, id: u64) {
    let Seen::Shape {
        id: actual_id,
        size,
        hot,
        scale,
        png,
    } = event
    else {
        panic!("expected shape, got {event:?}");
    };
    assert_eq!((*actual_id, *size, *hot, *scale), (id, (2, 2), (1, 1), 1.0));
    assert!(!png.is_empty());
    let mut reader = png::Decoder::new(Cursor::new(png)).read_info().unwrap();
    let mut rgba = vec![0; reader.output_buffer_size().unwrap()];
    let info = reader.next_frame(&mut rgba).unwrap();
    assert_eq!((info.width, info.height), (2, 2));
    assert_eq!(info.color_type, png::ColorType::Rgba);
    assert_eq!(info.bit_depth, png::BitDepth::Eight);
    assert_eq!(&rgba[..info.buffer_size()], &RGBA);
}

#[test]
fn cursor_events() {
    let frame = committed_frame();
    let rec = Arc::new(Rec::default());
    frame.attach(rec.clone());
    rec.take();
    frame.cursor_shape(2, 2, 1, 1, &RGBA);
    let events = rec.take();
    assert_eq!(events.len(), 2);
    assert_shape(&events[0], 1);
    assert_eq!(events[1], Seen::Use(1));
    frame.cursor_shape(0, 0, 0, 0, &[]);
    assert_eq!(rec.take(), [Seen::Hidden(0)]);
    frame.cursor_pos(3, 2);
    assert_eq!(rec.take(), [Seen::Pos(1.0, 1.0, 0)]);
    frame.cursor_shape(2, 2, 1, 1, &RGBA);
    let events = rec.take();
    assert_eq!(events.len(), 3);
    assert_shape(&events[0], 2);
    assert_eq!(events[1..], [Seen::Use(2), Seen::Hidden(1)]);
    frame.cursor_pos(3, 2);
    assert_eq!(rec.take(), [Seen::Pos(1.0, 1.0, 1)]);
}

#[test]
fn attach_replays() {
    let frame = committed_frame();
    frame.cursor_shape(2, 2, 1, 1, &RGBA);
    frame.cursor_pos(3, 2);
    let rec = Arc::new(Rec::default());
    assert!(frame.attach(rec.clone()));
    let events = rec.take();
    assert_eq!(events.len(), 5);
    assert_eq!(events[0], config(1));
    assert_shape(&events[1], 1);
    assert_eq!(
        events[2..],
        [Seen::Use(1), Seen::Pos(1.0, 1.0, 1), Seen::Ready(1, 4, 3)]
    );
    frame.detach();
    assert!(!frame.is_attached());
    frame.cursor_shape(0, 1, 0, 0, &[]);
    frame.attach(rec.clone());
    let events = rec.take();
    assert_eq!(
        events[2..],
        [
            Seen::Hidden(0),
            Seen::Pos(1.0, 1.0, 0),
            Seen::Ready(1, 4, 3)
        ]
    );
}

#[test]
fn replace_consumer() {
    let frame = committed_frame();
    let a = Arc::new(Rec::default());
    let b = Arc::new(Rec::default());
    assert!(frame.attach(a.clone()));
    a.take();
    assert!(frame.attach(b.clone()));
    b.take();
    frame.fill(0, 0, 1, 1, [1; 4]);
    frame.commit();
    assert!(a.take().is_empty());
    assert_eq!(b.take(), [Seen::Ready(2, 4, 3)]);
}

struct Slow {
    entered: Barrier,
    ended: Mutex<Option<Instant>>,
}

impl RawConsumer for Slow {
    fn ready(&self, _: u64, _: u32, _: u32) {
        self.entered.wait();
        thread::sleep(Duration::from_millis(200));
        *self.ended.lock().unwrap() = Some(Instant::now());
    }

    fn event(&self, _: &RsEvent) {}
}

#[test]
fn detach_waits_for_callback() {
    let frame = RawFrame::new();
    frame.resize(4, 3);
    let slow = Arc::new(Slow {
        entered: Barrier::new(2),
        ended: Mutex::new(None),
    });
    frame.attach(slow.clone());
    let writer = {
        let frame = frame.clone();
        thread::spawn(move || frame.commit())
    };
    slow.entered.wait();
    frame.detach();
    let detached = Instant::now();
    assert!(slow.ended.lock().unwrap().unwrap() <= detached);
    assert!(!frame.is_attached());
    writer.join().unwrap();
}

struct OnReady {
    rec: Arc<Rec>,
    run: Box<dyn Fn() + Send + Sync>,
}

impl RawConsumer for OnReady {
    fn ready(&self, seq: u64, w: u32, h: u32) {
        self.rec.ready(seq, w, h);
        (self.run)();
    }

    fn event(&self, event: &RsEvent) {
        self.rec.event(event);
    }
}

#[test]
fn reentrant_detach_in_callback() {
    let frame = RawFrame::new();
    let rec = Arc::new(Rec::default());
    let weak = Arc::downgrade(&frame);
    frame.attach(Arc::new(OnReady {
        rec: rec.clone(),
        run: Box::new(move || {
            let frame = weak.upgrade().unwrap();
            assert_eq!((frame.seq(), frame.size()), (1, (4, 3)));
            frame.detach();
        }),
    }));
    frame.resize(4, 3);
    rec.take();
    frame.commit();
    assert_eq!(rec.take(), [Seen::Ready(1, 4, 3)]);
    assert!(!frame.is_attached());
    frame.fill(0, 0, 1, 1, [1; 4]);
    frame.commit();
    assert!(rec.take().is_empty());
}

#[test]
fn close_once() {
    let frame = committed_frame();
    let rec = Arc::new(Rec::default());
    frame.attach(rec.clone());
    rec.take();
    frame.close("bye");
    frame.close("again");
    assert_eq!(rec.take(), [Seen::Closed("bye".into())]);
    assert!(frame.is_closed());
    assert!(!frame.is_attached());
    assert!(!frame.attach(rec.clone()));
    frame.resize(2, 2);
    frame.fill(0, 0, 1, 1, [9; 4]);
    frame.commit();
    frame.cursor_shape(2, 2, 1, 1, &RGBA);
    assert!(rec.take().is_empty());
    let frame = RawFrame::new();
    frame.attach(rec.clone());
    frame.close("a\0b");
    assert_eq!(rec.take(), [Seen::Closed("a?b".into())]);
}

#[test]
fn uncommitted_writes_are_not_visible() {
    let frame = committed_frame();
    frame.fill(0, 0, 1, 1, [1; 4]);
    assert_eq!(pixels(&frame), [0, 0, 0, 255].repeat(12));
    frame.fill(3, 2, 1, 1, [2; 4]);
    frame.commit();
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(1, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (2, rect(0, 0, 4, 3))
    );
    assert_eq!(&dst[..4], &[1; 4]);
    assert_eq!(&dst[44..], &[2; 4]);
    assert_eq!(&dst[4..44], &[0, 0, 0, 255].repeat(10));
    frame.resize(2, 2);
    let mut resized = [0xEE; 16];
    assert_eq!(
        frame.copy_into(0, &mut resized, 8, 2, 2, RS_PIX_BGRA8),
        (0, RsRect::default())
    );
    assert_eq!(resized, [0xEE; 16]);
    frame.commit();
    assert_eq!(pixels(&frame), [0, 0, 0, 255].repeat(4));
}

#[test]
fn source_validation_clipping_and_padding() {
    let frame = committed_frame();
    for (w, h, len, stride) in [(2, 2, 15, 8), (2, 2, 16, 7), (2, 3, 16, usize::MAX)] {
        frame.put(0, 0, w, h, &vec![9; len], stride, SrcPixels::Bgra8);
    }
    frame.put(
        0,
        0,
        u32::MAX,
        u32::MAX,
        &[9; 16],
        usize::MAX,
        SrcPixels::Bgra8,
    );
    frame.put(u32::MAX, 0, 1, 1, &[], 0, SrcPixels::Rgba8);
    for (w, h) in [(0, 3), (4, 0), (8193, 3), (4, 8193)] {
        frame.resize(w, h);
    }
    frame.commit();
    assert_eq!((frame.seq(), frame.epoch(), frame.size()), (1, 1, (4, 3)));
    // Two source rows, each padded to 12 bytes, cropped at the right edge.
    let src = [
        1, 2, 3, 255, 4, 5, 6, 255, 0xDD, 0xDD, 0xDD, 0xDD, 7, 8, 9, 255, 10, 11, 12, 255,
    ];
    frame.put(3, 1, 2, 2, &src, 12, SrcPixels::Bgra8);
    frame.commit();
    let mut dst = [0xEE; 56]; // No padding required after the last row.
    assert_eq!(
        frame.copy_into(1, &mut dst, 20, 4, 3, RS_PIX_BGRA8),
        (2, rect(3, 1, 1, 2))
    );
    let mut expected = [0xEE; 56];
    expected[32..36].copy_from_slice(&src[..4]);
    expected[52..56].copy_from_slice(&src[12..16]);
    assert_eq!(dst, expected);
}

#[test]
fn copy_within_vertical_and_clipped_source() {
    let original: Vec<_> = (0..12).flat_map(|n| [n, 0, 0, 255]).collect();
    for (sy, dy) in [(0, 1), (1, 0)] {
        let frame = committed_frame();
        frame.put(0, 0, 4, 3, &original, 16, SrcPixels::Bgra8);
        frame.copy_within(0, sy, 0, dy, 4, 2);
        frame.commit();
        let mut expected = original.clone();
        expected[dy as usize * 16..dy as usize * 16 + 32]
            .copy_from_slice(&original[sy as usize * 16..sy as usize * 16 + 32]);
        assert_eq!(pixels(&frame), expected);
    }
    let frame = committed_frame();
    frame.put(0, 0, 4, 3, &original, 16, SrcPixels::Bgra8);
    frame.commit();
    frame.copy_within(3, 2, 1, 0, u32::MAX, u32::MAX);
    frame.commit();
    let mut dst = [0xEE; 48];
    assert_eq!(
        frame.copy_into(2, &mut dst, 16, 4, 3, RS_PIX_BGRA8),
        (3, rect(1, 0, 1, 1))
    );
    assert_eq!(&dst[4..8], &original[44..48]);
}

#[test]
fn cursor_validation_and_position_edges() {
    assert!(encode_png(0, 1, &[]).is_err());
    assert!(encode_png(1, 0, &[]).is_err());
    assert!(encode_png(2, 2, &RGBA[..15]).is_err());
    assert!(encode_png(1, 1, &RGBA).is_err());
    assert!(encode_png(u32::MAX, u32::MAX, &[]).is_err());
    let mut cursor = CursorState::default();
    assert!(cursor.pos(0, 0, 0, 0).is_none());
    let event = cursor.pos(0, 0, 1, 1).unwrap();
    assert_eq!(
        (event.cursor_x, event.cursor_y, event.cursor_visible),
        (0.0, 0.0, 1)
    );
    assert!(cursor.shape(2, 2, 1, 1, &RGBA[..15]).is_empty());
    assert_eq!(cursor.shape(2, 2, 1, 1, &RGBA)[0].shape_id, 1);
    cursor.shape(0, 2, 0, 0, &[]);
    assert!(cursor.shape(2, 2, 1, 1, &RGBA[..15]).is_empty());
    let replay = cursor.replay();
    assert_eq!(replay[1].kind, RS_EV_CURSOR_HIDDEN);
    assert_eq!(replay[1].cursor_visible, 0);
    assert_eq!(replay[2].cursor_visible, 0);
}

#[test]
fn reentrant_replacement_last_operation_wins() {
    for detach_last in [false, true] {
        let frame = RawFrame::new();
        frame.resize(4, 3);
        let a = Arc::new(Rec::default());
        let b = Arc::new(Rec::default());
        let c = Arc::new(Rec::default());
        let weak = Arc::downgrade(&frame);
        let (b2, c2) = (b.clone(), c.clone());
        frame.attach(Arc::new(OnReady {
            rec: a.clone(),
            run: Box::new(move || {
                let frame = weak.upgrade().unwrap();
                assert!(frame.attach(b2.clone()));
                frame.detach();
                assert!(frame.attach(c2.clone()));
                // Replacement callbacks must wait for this callback to return.
                assert!(b2.take().is_empty());
                assert!(c2.take().is_empty());
                if detach_last {
                    frame.detach();
                }
            }),
        }));
        frame.commit();
        assert_eq!(a.take(), [config(1), Seen::Ready(1, 4, 3)]);
        assert!(b.take().is_empty());
        if detach_last {
            assert!(c.take().is_empty());
            assert!(!frame.is_attached());
        } else {
            assert_eq!(c.take(), [config(1), Seen::Ready(1, 4, 3)]);
            frame.fill(0, 0, 1, 1, [1; 4]);
            frame.commit();
            assert_eq!(c.take(), [Seen::Ready(2, 4, 3)]);
        }
    }
}

#[test]
fn close_from_callback_cancels_replacement() {
    let frame = RawFrame::new();
    let a = Arc::new(Rec::default());
    let b = Arc::new(Rec::default());
    let weak = Arc::downgrade(&frame);
    let b2 = b.clone();
    frame.attach(Arc::new(OnReady {
        rec: a.clone(),
        run: Box::new(move || {
            let frame = weak.upgrade().unwrap();
            frame.attach(b2.clone());
            frame.close("inside");
            frame.detach();
            assert!(!frame.attach(b2.clone()));
        }),
    }));
    frame.resize(4, 3);
    frame.commit();
    assert_eq!(
        a.take(),
        [
            config(1),
            Seen::Ready(1, 4, 3),
            Seen::Closed("inside".into())
        ]
    );
    assert!(b.take().is_empty());
    assert!(frame.is_closed());
    assert!(!frame.is_attached());
}

#[test]
fn nested_callbacks_track_each_tap() {
    let a = RawFrame::new();
    a.resize(4, 3);
    let b = committed_frame();
    let a_rec = Arc::new(Rec::default());
    let b_rec = Arc::new(Rec::default());
    let weak_a = Arc::downgrade(&a);
    let b_consumer = Arc::new(OnReady {
        rec: b_rec.clone(),
        run: Box::new(move || weak_a.upgrade().unwrap().detach()),
    });
    a.attach(Arc::new(OnReady {
        rec: a_rec.clone(),
        run: Box::new(move || {
            assert!(b.attach(b_consumer.clone()));
            assert!(b.is_attached());
        }),
    }));
    a.commit();
    assert_eq!(a_rec.take(), [config(1), Seen::Ready(1, 4, 3)]);
    assert_eq!(b_rec.take(), [config(1), Seen::Ready(1, 4, 3)]);
    assert!(!a.is_attached());
    a.fill(0, 0, 1, 1, [1; 4]);
    a.commit();
    assert!(a_rec.take().is_empty());
}

#[derive(Default)]
struct SerialRec {
    active: AtomicBool,
    overlap: AtomicBool,
    calls: AtomicUsize,
}

impl SerialRec {
    fn call(&self) {
        if self.active.swap(true, Ordering::SeqCst) {
            self.overlap.store(true, Ordering::SeqCst);
        }
        thread::sleep(Duration::from_millis(2));
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.active.store(false, Ordering::SeqCst);
    }
}

impl RawConsumer for SerialRec {
    fn ready(&self, _: u64, _: u32, _: u32) {
        self.call();
    }

    fn event(&self, _: &RsEvent) {
        self.call();
    }
}

#[test]
fn concurrent_ready_and_events_are_serialized() {
    let frame = committed_frame();
    let rec = Arc::new(SerialRec::default());
    frame.attach(rec.clone());
    let start = Arc::new(Barrier::new(2));
    let writer = {
        let frame = frame.clone();
        let start = start.clone();
        thread::spawn(move || {
            start.wait();
            for n in 0..20 {
                frame.fill(0, 0, 1, 1, [n; 4]);
                frame.commit();
            }
        })
    };
    start.wait();
    for _ in 0..20 {
        frame.cursor_pos(3, 2);
    }
    writer.join().unwrap();
    frame.detach();
    assert_eq!(rec.calls.load(Ordering::SeqCst), 42);
    assert!(!rec.overlap.load(Ordering::SeqCst));
    assert!(!rec.active.load(Ordering::SeqCst));
}

const FFI_ID: u64 = ADAPTER_ID_BASE + 7;
static FFI_CORE: Mutex<()> = Mutex::new(());
static FFI_REC: Rec = Rec {
    log: Mutex::new(Vec::new()),
};
static FFI_THREADS: Mutex<Vec<ThreadId>> = Mutex::new(Vec::new());
static REENTER: AtomicU8 = AtomicU8::new(0);
static REATTACHED: AtomicBool = AtomicBool::new(false);

fn reenter_ffi(trigger: u8) {
    if REENTER
        .compare_exchange(trigger, 0, Ordering::SeqCst, Ordering::SeqCst)
        .is_ok()
    {
        rs_raw_detach(FFI_ID);
        // SAFETY: These process-lifetime callbacks use no borrowed context.
        let attached =
            unsafe { rs_raw_attach(FFI_ID, ptr::null_mut(), Some(ffi_ready), Some(ffi_event)) };
        REATTACHED.store(attached, Ordering::SeqCst);
    }
}

extern "C" fn ffi_ready(_: *mut c_void, seq: u64, w: u32, h: u32) {
    FFI_REC.ready(seq, w, h);
    FFI_THREADS.lock().unwrap().push(thread::current().id());
    reenter_ffi(1);
}

extern "C" fn ffi_event(_: *mut c_void, event: *const RsEvent) {
    // SAFETY: FfiRaw supplies a valid event for the duration of this callback.
    FFI_REC.event(unsafe { &*event });
    FFI_THREADS.lock().unwrap().push(thread::current().id());
    reenter_ffi(2);
}

struct Installed {
    core: Arc<Core>,
    _serial: MutexGuard<'static, ()>,
}

impl Installed {
    fn new() -> Self {
        let serial = FFI_CORE.lock().unwrap();
        FFI_REC.take();
        FFI_THREADS.lock().unwrap().clear();
        REENTER.store(0, Ordering::SeqCst);
        REATTACHED.store(false, Ordering::SeqCst);
        let core = support::test_core();
        install_core(core.clone());
        Self {
            core,
            _serial: serial,
        }
    }
}

impl Drop for Installed {
    fn drop(&mut self) {
        rs_raw_detach(FFI_ID);
        take_core();
    }
}

#[test]
fn ffi_attach_and_copy() {
    let installed = Installed::new();
    let frame = RawFrame::new();
    let registry = installed.core.state::<RawRegistry>();
    registry.insert(FFI_ID, frame.clone());
    // SAFETY: Callbacks and their static storage outlive detach.
    assert!(unsafe { rs_raw_attach(FFI_ID, ptr::null_mut(), Some(ffi_ready), Some(ffi_event)) });
    frame.resize(4, 3);
    frame.fill(1, 1, 2, 1, [1, 2, 3, 255]);
    frame.commit();
    assert_eq!(FFI_REC.take(), [config(1), Seen::Ready(1, 4, 3)]);
    let mut buf = [0xEE; 48];
    let mut copied = RsRect::default();
    // SAFETY: buf is exclusive and sufficiently sized; copied is writable.
    let seq = unsafe {
        rs_raw_copy(
            FFI_ID,
            0,
            buf.as_mut_ptr(),
            16,
            4,
            3,
            RS_PIX_BGRA8,
            &mut copied,
        )
    };
    assert_eq!(seq, frame.seq());
    assert_eq!(copied, rect(0, 0, 4, 3));
    assert_eq!(buf.to_vec(), pixels(&frame));
    rs_raw_detach(FFI_ID);
    assert!(!frame.is_attached());
    FFI_THREADS.lock().unwrap().clear();
    // SAFETY: The callbacks have process lifetime. This attachment replays.
    assert!(unsafe { rs_raw_attach(FFI_ID, ptr::null_mut(), Some(ffi_ready), Some(ffi_event)) });
    assert_eq!(FFI_REC.take(), [config(1), Seen::Ready(1, 4, 3)]);
    assert!(FFI_THREADS
        .lock()
        .unwrap()
        .iter()
        .all(|id| *id != thread::current().id()));
    rs_raw_detach(FFI_ID);
    assert!(Arc::ptr_eq(&registry.remove(FFI_ID).unwrap(), &frame));
    assert!(registry.get(FFI_ID).is_none());
    assert!(registry.remove(FFI_ID).is_none());
}

#[test]
fn ffi_rejects() {
    let installed = Installed::new();
    let frame = committed_frame();
    installed
        .core
        .state::<RawRegistry>()
        .insert(FFI_ID, frame.clone());
    // SAFETY: Callbacks are static. Valid copy buffers have the required size;
    // nulls and overflowing lengths must be rejected before dereferencing.
    unsafe {
        assert!(!rs_raw_attach(
            5,
            ptr::null_mut(),
            Some(ffi_ready),
            Some(ffi_event)
        ));
        assert!(!rs_raw_attach(
            FFI_ID + 1,
            ptr::null_mut(),
            Some(ffi_ready),
            Some(ffi_event)
        ));
        assert!(!rs_raw_attach(
            FFI_ID,
            ptr::null_mut(),
            None,
            Some(ffi_event)
        ));
        assert!(!rs_raw_attach(
            FFI_ID,
            ptr::null_mut(),
            Some(ffi_ready),
            None
        ));
        assert_eq!(
            rs_raw_copy(
                FFI_ID,
                0,
                ptr::null_mut(),
                16,
                4,
                3,
                RS_PIX_BGRA8,
                ptr::null_mut()
            ),
            0
        );
        let mut buf = [0xEE; 48];
        let mut copied = rect(9, 9, 9, 9);
        for (id, stride, w, h, pix) in [
            (FFI_ID + 1, 16, 4, 3, RS_PIX_BGRA8),
            (5, 16, 4, 3, RS_PIX_BGRA8),
            (FFI_ID, 16, 0, 3, RS_PIX_BGRA8),
            (FFI_ID, 16, 4, 0, RS_PIX_BGRA8),
            (FFI_ID, 15, 4, 3, RS_PIX_BGRA8),
            (FFI_ID, usize::MAX, 4, 3, RS_PIX_BGRA8),
            (FFI_ID, usize::MAX, 4, 2, RS_PIX_BGRA8),
            (FFI_ID, isize::MAX as usize, 4, 2, RS_PIX_BGRA8),
        ] {
            assert_eq!(
                rs_raw_copy(id, 0, buf.as_mut_ptr(), stride, w, h, pix, &mut copied),
                0
            );
            assert_eq!(buf, [0xEE; 48]);
        }
        assert_eq!(
            rs_raw_copy(FFI_ID, 0, buf.as_mut_ptr(), 16, 4, 3, 3, &mut copied),
            0
        );
        assert_eq!(copied, RsRect::default());
        assert_eq!(buf, [0xEE; 48]);
        assert_eq!(
            rs_raw_copy(
                FFI_ID,
                0,
                buf.as_mut_ptr(),
                16,
                4,
                3,
                RS_PIX_BGRA8,
                ptr::null_mut()
            ),
            1
        );
        buf.fill(0xEE);
        assert_eq!(
            rs_raw_copy(
                FFI_ID,
                1,
                buf.as_mut_ptr(),
                16,
                4,
                3,
                RS_PIX_BGRA8,
                &mut copied
            ),
            1
        );
        assert_eq!(copied, RsRect::default());
        assert_eq!(buf, [0xEE; 48]);
        frame.close("done");
        assert!(!rs_raw_attach(
            FFI_ID,
            ptr::null_mut(),
            Some(ffi_ready),
            Some(ffi_event)
        ));
        take_core();
        assert!(!rs_raw_attach(
            FFI_ID,
            ptr::null_mut(),
            Some(ffi_ready),
            Some(ffi_event)
        ));
        assert_eq!(
            rs_raw_copy(
                FFI_ID,
                0,
                buf.as_mut_ptr(),
                16,
                4,
                3,
                RS_PIX_BGRA8,
                ptr::null_mut()
            ),
            0
        );
    }
    rs_raw_detach(FFI_ID);
}

#[test]
fn ffi_reentrant_attach_replays_on_callback_thread() {
    let installed = Installed::new();
    let frame = committed_frame();
    installed.core.state::<RawRegistry>().insert(FFI_ID, frame);
    for trigger in [1, 2] {
        REENTER.store(trigger, Ordering::SeqCst);
        REATTACHED.store(false, Ordering::SeqCst);
        // SAFETY: Callbacks and their records are process-lifetime statics.
        assert!(unsafe {
            rs_raw_attach(FFI_ID, ptr::null_mut(), Some(ffi_ready), Some(ffi_event))
        });
        assert!(REATTACHED.load(Ordering::SeqCst));
        let events = FFI_REC.take();
        if trigger == 1 {
            assert_eq!(
                events,
                [
                    config(1),
                    Seen::Ready(1, 4, 3),
                    config(1),
                    Seen::Ready(1, 4, 3)
                ]
            );
        } else {
            assert_eq!(events, [config(1), config(1), Seen::Ready(1, 4, 3)]);
        }
        let threads = std::mem::take(&mut *FFI_THREADS.lock().unwrap());
        assert!(threads.iter().all(|id| *id == threads[0]));
        assert_ne!(threads[0], thread::current().id());
        rs_raw_detach(FFI_ID);
    }
}
