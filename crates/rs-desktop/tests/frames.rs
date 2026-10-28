use std::{
    collections::BTreeMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        mpsc, Arc, Mutex, Weak,
    },
    thread,
    time::Duration,
};

use rs_desktop::{
    frames::{
        has_parameter_sets, to_desktop_frame, DecodeStats, FrameAdapter, FrameGate, GateAction,
        GateConfig, KeyframeReason, VideoControl, VideoEvents,
    },
    platform::{
        BitstreamFormat, DecodeResult, DesktopFrame, FrameDone, FrameSink, StreamInfo, VideoCodec,
    },
};
use rs_mobile::rscore::tables::{VideoConsumer, VideoTap};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::{pb, video::Frame};

use GateAction::{Drop, RequestKeyframe, Submit};
use KeyframeReason::{Backlog, DecoderReset};

#[test]
fn delta_before_keyframe_after_reset_dropped() {
    let mut gate = FrameGate::new(GateConfig::default());
    gate.reset(true, 0);
    assert_eq!(gate.on_frame(1, false, false, 0), [Drop(1)]);
    assert_eq!(gate.on_frame(2, true, false, 1), [Drop(2)]);
    assert_eq!(gate.on_frame(3, false, true, 2), [Drop(3)]);
    assert_eq!(gate.on_frame(4, true, true, 3), [Submit(4)]);
    assert!(!gate.waiting_for_keyframe());
    assert!(gate.on_tick(1_000_000).is_empty());
}

#[test]
fn reset_wait_requests_backlog_after_quiet_period() {
    let mut gate = FrameGate::new(GateConfig::default());
    gate.reset(true, 0);
    assert!(gate.on_tick(400_000).is_empty());
    assert_eq!(gate.on_tick(500_000), [RequestKeyframe(Backlog)]);
    assert!(gate.on_tick(999_999).is_empty());
    assert_eq!(gate.on_tick(1_000_000), [RequestKeyframe(Backlog)]);
}

#[test]
fn backlog_drops_and_requests() {
    let mut gate = FrameGate::new(GateConfig::default());
    for id in 1..=3 {
        assert_eq!(gate.on_frame(id, false, false, 0), [Submit(id)]);
    }
    assert_eq!(
        gate.on_frame(4, false, false, 10),
        [Drop(4), RequestKeyframe(Backlog)]
    );
    assert_eq!(gate.in_flight(), 3);
    assert_eq!(gate.on_frame(5, false, false, 20), [Drop(5)]);
    assert_eq!(gate.on_frame(6, true, false, 30), [Drop(6)]);
    assert_eq!(gate.on_frame(7, true, true, 40), [Submit(7)]);
    assert!(!gate.waiting_for_keyframe());
    // Previously submitted frames still own receipts, including during recovery.
    assert_eq!(gate.in_flight(), 4);
    for id in [3, 1, 7, 2] {
        assert!(gate.on_done(id, true, 50).is_empty());
    }
    assert_eq!(gate.in_flight(), 0);
    assert!(gate.on_tick(1_000_000).is_empty());
}

#[test]
fn decode_failure_requests_decoder_reset() {
    let mut gate = FrameGate::new(GateConfig::default());
    assert_eq!(gate.on_frame(1, false, false, 0), [Submit(1)]);
    assert_eq!(gate.on_frame(2, false, false, 0), [Submit(2)]);
    assert_eq!(gate.on_done(1, false, 10), [RequestKeyframe(DecoderReset)]);
    assert_eq!(gate.in_flight(), 1);
    assert_eq!(gate.on_frame(3, false, false, 20), [Drop(3)]);
    // An outstanding failure while waiting only removes that submission.
    assert!(gate.on_done(2, false, 400_000).is_empty());
    assert_eq!(gate.in_flight(), 0);
    assert_eq!(gate.on_tick(500_010), [RequestKeyframe(DecoderReset)]);
}

#[test]
fn request_throttle_300ms() {
    let mut gate = FrameGate::new(GateConfig::default());
    assert_eq!(gate.on_frame(1, true, true, 0), [Submit(1)]);
    assert_eq!(gate.on_done(1, false, 0), [RequestKeyframe(DecoderReset)]);
    assert_eq!(gate.on_frame(2, true, true, 50_000), [Submit(2)]);
    assert!(gate.on_done(2, false, 100_000).is_empty());
    assert!(gate.on_tick(499_999).is_empty());
    assert_eq!(gate.on_tick(500_000), [RequestKeyframe(DecoderReset)]);
}

#[test]
fn throttle_survives_reset() {
    let mut gate = FrameGate::new(GateConfig::default());
    gate.on_frame(1, true, true, 0);
    assert_eq!(gate.on_done(1, false, 0), [RequestKeyframe(DecoderReset)]);
    gate.reset(true, 50_000);
    assert_eq!(gate.on_frame(2, true, true, 60_000), [Submit(2)]);
    assert_eq!(gate.on_frame(3, false, false, 70_000), [Submit(3)]);
    assert_eq!(gate.on_frame(4, false, false, 80_000), [Submit(4)]);
    assert_eq!(gate.on_frame(5, false, false, 200_000), [Drop(5)]);
    assert_eq!(gate.on_tick(500_000), [RequestKeyframe(Backlog)]);
}

#[test]
fn stale_done_ignored() {
    let mut gate = FrameGate::new(GateConfig::default());
    assert_eq!(gate.on_frame(1, false, false, 0), [Submit(1)]);
    gate.reset(true, 10);
    assert!(gate.on_done(1, false, 20).is_empty());
    assert!(gate.on_done(1, true, 30).is_empty());
    assert_eq!(gate.in_flight(), 0);
    assert_eq!(gate.on_tick(500_010), [RequestKeyframe(Backlog)]);
    gate.reset(false, 600_000);
    assert!(gate.on_tick(2_000_000).is_empty());
    assert_eq!(gate.on_frame(2, false, false, 2_000_000), [Submit(2)]);
}

#[test]
fn in_flight_count() {
    let mut gate = FrameGate::new(GateConfig::default());
    assert_eq!(gate.on_frame(1, false, false, 0), [Submit(1)]);
    assert_eq!(gate.on_frame(2, false, false, 0), [Submit(2)]);
    assert!(gate.on_done(1, true, 10).is_empty());
    assert_eq!(gate.in_flight(), 1);
    assert!(gate.on_done(1, false, 20).is_empty());
    assert_eq!(gate.in_flight(), 1);
    assert!(!gate.waiting_for_keyframe());
}

#[test]
fn short_retry_interval_still_respects_common_throttle() {
    let mut gate = FrameGate::new(GateConfig {
        max_in_flight: 1,
        rerequest_after_us: 100_000,
        ..GateConfig::default()
    });
    gate.on_frame(1, true, true, 0);
    assert_eq!(gate.on_done(1, false, 0), [RequestKeyframe(DecoderReset)]);
    assert!(gate.on_tick(100_000).is_empty());
    assert!(gate.on_tick(299_999).is_empty());
    assert_eq!(gate.on_tick(300_000), [RequestKeyframe(DecoderReset)]);
    gate.reset(true, 310_000);
    assert!(gate.on_tick(410_000).is_empty());
    assert!(gate.on_tick(599_999).is_empty());
    assert_eq!(gate.on_tick(600_000), [RequestKeyframe(Backlog)]);
    assert_eq!(gate.on_frame(2, true, true, 610_000), [Submit(2)]);
    assert_eq!(gate.on_frame(3, false, false, 620_000), [Drop(3)]);
    assert_eq!(gate.on_tick(900_000), [RequestKeyframe(Backlog)]);
}

#[derive(Debug, Clone, PartialEq)]
enum Event {
    Configure(StreamInfo),
    Stream(StreamInfo),
    Pointer(f32, f32, bool),
    Cursor(pb::CursorMsg),
    SinkClosed,
    Closed(String),
}

type History = Arc<Mutex<Vec<Event>>>;

#[derive(Default)]
struct SinkState {
    frames: Vec<DesktopFrame>,
    pending: BTreeMap<u32, FrameDone>,
}

struct TestSink {
    format: BitstreamFormat,
    synchronous: Option<DecodeResult>,
    history: History,
    state: Mutex<SinkState>,
}

impl TestSink {
    fn frames(&self) -> Vec<DesktopFrame> {
        self.state.lock().unwrap().frames.clone()
    }

    fn take_done(&self, id: u32) -> FrameDone {
        self.state.lock().unwrap().pending.remove(&id).unwrap()
    }

    fn finish(&self, id: u32, result: DecodeResult) {
        self.take_done(id).finish(result);
    }
}

impl FrameSink for TestSink {
    fn format(&self) -> BitstreamFormat {
        self.format
    }

    fn configure(&self, info: &StreamInfo) {
        self.history
            .lock()
            .unwrap()
            .push(Event::Configure(info.clone()));
    }

    fn submit(&self, frame: DesktopFrame, done: FrameDone) {
        let frame_id = frame.frame_id;
        self.state.lock().unwrap().frames.push(frame);
        if let Some(result) = self.synchronous {
            done.finish(result);
        } else {
            assert!(self
                .state
                .lock()
                .unwrap()
                .pending
                .insert(frame_id, done)
                .is_none());
        }
    }

    fn close(&self) {
        self.history.lock().unwrap().push(Event::SinkClosed);
    }
}

#[derive(Default)]
struct TestControl {
    time: AtomicU64,
    requests: Mutex<Vec<KeyframeReason>>,
    reports: Mutex<Vec<(u64, u64, u32)>>,
    // The synchronous-receipt test also reenters stats from every control
    // method, verifying that now_us/request/report never hold the adapter lock.
    observer: Mutex<Weak<FrameAdapter>>,
}

impl TestControl {
    fn set_time(&self, time: u64) {
        self.time.store(time, Ordering::SeqCst);
    }

    fn observe(&self) {
        let observer = self.observer.lock().unwrap().clone();
        if let Some(adapter) = observer.upgrade() {
            let _ = adapter.stats();
        }
    }

    fn requests(&self) -> Vec<KeyframeReason> {
        self.requests.lock().unwrap().clone()
    }

    fn reports(&self) -> Vec<(u64, u64, u32)> {
        self.reports.lock().unwrap().clone()
    }
}

impl VideoControl for TestControl {
    fn request_keyframe(&self, reason: KeyframeReason) {
        self.observe();
        self.requests.lock().unwrap().push(reason);
    }

    fn report_decoded(&self, decode_us: u64, latency_us: u64, queue: u32) {
        self.observe();
        self.reports
            .lock()
            .unwrap()
            .push((decode_us, latency_us, queue));
    }

    fn now_us(&self) -> u64 {
        self.observe();
        self.time.load(Ordering::SeqCst)
    }
}

struct TestEvents(History);

impl VideoEvents for TestEvents {
    fn stream(&self, info: &StreamInfo) {
        self.0.lock().unwrap().push(Event::Stream(info.clone()));
    }

    fn cursor_msg(&self, m: &pb::CursorMsg) {
        self.0.lock().unwrap().push(Event::Cursor(m.clone()));
    }

    fn host_pointer(&self, x: f32, y: f32, visible: bool) {
        self.0.lock().unwrap().push(Event::Pointer(x, y, visible));
    }

    fn closed(&self, reason: &str) {
        self.0
            .lock()
            .unwrap()
            .push(Event::Closed(reason.to_owned()));
    }
}

struct Harness {
    adapter: Arc<FrameAdapter>,
    sink: Arc<TestSink>,
    control: Arc<TestControl>,
    history: History,
}

impl Harness {
    fn new(format: BitstreamFormat) -> Self {
        Self::with_synchronous(format, None)
    }

    fn with_synchronous(format: BitstreamFormat, synchronous: Option<DecodeResult>) -> Self {
        let history = Arc::new(Mutex::new(Vec::new()));
        let sink = Arc::new(TestSink {
            format,
            synchronous,
            history: history.clone(),
            state: Mutex::new(SinkState::default()),
        });
        let control = Arc::new(TestControl::default());
        control.set_time(1_000_000);
        let adapter = FrameAdapter::new(
            sink.clone(),
            control.clone(),
            Arc::new(TestEvents(history.clone())),
            GateConfig::default(),
        );
        Self {
            adapter,
            sink,
            control,
            history,
        }
    }

    fn configure(&self, codec: pb::Codec, epoch: u32) {
        self.adapter.stream_config(&config(codec, epoch));
    }

    fn events(&self) -> Vec<Event> {
        self.history.lock().unwrap().clone()
    }
}

fn config(codec: pb::Codec, epoch: u32) -> pb::StreamConfig {
    pb::StreamConfig {
        epoch,
        codec: codec as i32,
        width: 1920,
        height: 1080,
        display_id: 1,
        cursor_x: 0.25,
        cursor_y: 0.5,
        cursor_visible: true,
        ..pb::StreamConfig::default()
    }
}

const VPS: &[u8] = &[0x40, 1, 0xa1];
const SPS: &[u8] = &[0x42, 1, 0xa2];
const PPS: &[u8] = &[0x44, 1, 0xa3];
const IDR: &[u8] = &[0x26, 1, 0xa4, 0xa5];
const DELTA: &[u8] = &[0x02, 1, 0xa6];
const H264_SPS: &[u8] = &[0x67, 0xb1];
const H264_PPS: &[u8] = &[0x68, 0xb2];
const H264_IDR: &[u8] = &[0x65, 0xb3, 0xb4];

fn annexb(nals: &[&[u8]]) -> Vec<u8> {
    let mut data = Vec::new();
    for (index, nal) in nals.iter().enumerate() {
        if index % 2 == 0 {
            data.extend_from_slice(&[0, 0, 0, 1]);
        } else {
            data.extend_from_slice(&[0, 0, 1]);
        }
        data.extend_from_slice(nal);
    }
    data
}

fn frame(id: u32, epoch: u8, keyframe: bool, data: Vec<u8>) -> ReceivedFrame {
    ReceivedFrame {
        frame: Frame {
            frame_id: id,
            epoch,
            display_id: 1,
            flags: u8::from(keyframe),
            keyframe,
            capture_ts_us: 800_000,
            data,
        },
        received_us: 1_000_000,
        capture_local_us: Some(900_000),
    }
}

fn keyframe(id: u32, epoch: u8) -> ReceivedFrame {
    frame(id, epoch, true, annexb(&[VPS, SPS, PPS, IDR]))
}

fn delta(id: u32, epoch: u8) -> ReceivedFrame {
    frame(id, epoch, false, annexb(&[DELTA]))
}

fn avcc(nal: &[u8]) -> Vec<u8> {
    let mut data = (nal.len() as u32).to_be_bytes().to_vec();
    data.extend_from_slice(nal);
    data
}

#[test]
fn first_config_configures_sink() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    let info = StreamInfo {
        codec: VideoCodec::Hevc,
        width: 1920,
        height: 1080,
        display_id: 1,
    };
    assert_eq!(
        h.events(),
        [
            Event::Configure(info.clone()),
            Event::Stream(info),
            Event::Pointer(0.25, 0.5, true),
        ]
    );
}

#[test]
fn replayed_config_no_reset() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&delta(2, 3));
    h.sink.finish(1, DecodeResult::Shown { decode_us: 5 });
    h.sink.finish(2, DecodeResult::Shown { decode_us: 7 });
    assert_eq!(h.sink.frames().len(), 2);
    assert_eq!(h.control.reports(), [(5, 0, 1), (7, 0, 0)]);
    assert_eq!(
        h.events()
            .iter()
            .filter(|event| matches!(event, Event::Configure(_)))
            .count(),
        1
    );
    assert_eq!(h.events().last(), Some(&Event::Pointer(0.25, 0.5, true)));
    assert_eq!(h.events().len(), 4);
}

#[test]
fn avcc_keyframe_has_params() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    let frames = h.sink.frames();
    let f = &frames[0];
    assert_eq!(f.vps.as_deref(), Some(VPS));
    assert_eq!(f.sps.as_deref(), Some(SPS));
    assert_eq!(f.pps.as_deref(), Some(PPS));
    assert_eq!(f.data, avcc(IDR));
    assert_eq!(f.epoch, 3);
    assert_eq!(f.display_id, 1);
    assert_eq!(f.codec, VideoCodec::Hevc);
    assert_eq!(f.capture_ts_us, 800_000);
    assert_eq!(f.capture_local_us, Some(900_000));
    assert_eq!(f.received_us, 1_000_000);
    assert!(f.keyframe);
}

#[test]
fn avcc_delta_no_params() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    // Even unexpected in-band parameter sets are hidden on a delta, as in iOS.
    h.adapter
        .frame(&frame(2, 3, false, annexb(&[VPS, SPS, PPS, DELTA])));
    let frames = h.sink.frames();
    let f = &frames[1];
    assert_eq!((&f.vps, &f.sps, &f.pps), (&None, &None, &None));
    assert_eq!(f.data, avcc(DELTA));
    assert!(!f.keyframe);
}

#[test]
fn h264_avcc() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::H264, 3);
    h.adapter
        .frame(&frame(1, 3, true, annexb(&[H264_SPS, H264_PPS, H264_IDR])));
    let frames = h.sink.frames();
    let f = &frames[0];
    assert_eq!(f.vps, None);
    assert_eq!(f.sps.as_deref(), Some(H264_SPS));
    assert_eq!(f.pps.as_deref(), Some(H264_PPS));
    assert_eq!(f.data, avcc(H264_IDR));
    assert_eq!(f.codec, VideoCodec::H264);
}

#[test]
fn annexb_passthrough() {
    for (codec, data) in [
        (pb::Codec::Hevc, annexb(&[VPS, SPS, PPS, IDR])),
        (pb::Codec::H264, annexb(&[H264_SPS, H264_PPS, H264_IDR])),
    ] {
        let h = Harness::new(BitstreamFormat::AnnexB);
        h.configure(codec, 3);
        h.adapter.frame(&frame(1, 3, true, data.clone()));
        let frames = h.sink.frames();
        assert_eq!(frames[0].data, data);
        assert_eq!(
            (&frames[0].vps, &frames[0].sps, &frames[0].pps),
            (&None, &None, &None)
        );
    }
}

#[test]
fn epoch_mismatch_dropped() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 259);
    h.adapter.frame(&keyframe(1, 3));
    h.adapter.frame(&keyframe(2, 4));
    let frames = h.sink.frames();
    assert_eq!(frames.len(), 1);
    assert_eq!(frames[0].epoch, 259);
    assert_eq!(h.adapter.stats().dropped, 1);
}

#[test]
fn report_decoded_latency() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    h.control.set_time(1_012_000);
    h.sink.finish(1, DecodeResult::Shown { decode_us: 4_000 });
    assert_eq!(h.control.reports(), [(4_000, 12_000, 0)]);
    assert_eq!(
        h.adapter.stats(),
        DecodeStats {
            submitted: 1,
            shown: 1,
            last_decode_us: 4_000,
            ..DecodeStats::default()
        }
    );
}

#[test]
fn failed_done_requests_reset() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    h.sink.finish(1, DecodeResult::Failed);
    assert_eq!(h.control.requests(), [DecoderReset]);
    assert!(h.control.reports().is_empty());
    h.adapter.frame(&delta(2, 3));
    assert_eq!(h.sink.frames().len(), 1);
    assert_eq!(h.adapter.stats().failed, 1);
    assert_eq!(h.adapter.stats().dropped, 1);
    assert_eq!(h.adapter.stats().keyframe_requests, 1);
    h.control.set_time(1_500_000);
    h.adapter.tick();
    assert_eq!(h.control.requests(), [DecoderReset, DecoderReset]);
}

#[test]
fn dropped_frame_done_counts_failed() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    drop(h.sink.take_done(1));
    assert_eq!(h.control.requests(), [DecoderReset]);
    assert_eq!(h.adapter.stats().failed, 1);
}

#[test]
fn sync_done_inside_submit_no_deadlock() {
    let h = Harness::with_synchronous(
        BitstreamFormat::Avcc,
        Some(DecodeResult::Shown { decode_us: 4_000 }),
    );
    *h.control.observer.lock().unwrap() = Arc::downgrade(&h.adapter);
    let adapter = h.adapter.clone();
    let control = h.control.clone();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        adapter.stream_config(&config(pb::Codec::Hevc, 3));
        control.set_time(1_500_000);
        adapter.tick(); // Reentrant request_keyframe as well as now_us/report.
        adapter.frame(&keyframe(1, 3));
        tx.send(()).unwrap();
    });
    rx.recv_timeout(Duration::from_secs(2))
        .expect("synchronous receipt or reentrant control deadlocked");
    worker.join().unwrap();
    assert_eq!(h.control.requests(), [Backlog]);
    assert_eq!(h.control.reports(), [(4_000, 500_000, 0)]);
}

#[test]
fn epoch_change_ignores_old_done() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    let old_done = h.sink.take_done(1);
    h.configure(pb::Codec::Hevc, 4);
    h.adapter.frame(&keyframe(1, 4));
    old_done.finish(DecodeResult::Failed);
    assert!(h.control.requests().is_empty());
    assert_eq!(h.adapter.stats().failed, 0);
    h.sink.finish(1, DecodeResult::Shown { decode_us: 1 });
    assert_eq!(h.control.reports(), [(1, 0, 0)]);
}

#[test]
fn closed_stops() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    h.adapter.closed("bye");
    let events = h.events();
    assert_eq!(
        &events[events.len() - 2..],
        &[Event::SinkClosed, Event::Closed("bye".to_owned())]
    );
    h.adapter.frame(&keyframe(2, 3));
    h.configure(pb::Codec::Hevc, 4);
    h.adapter.cursor(&CursorEvent {
        x: 0.1,
        y: 0.2,
        visible: false,
    });
    h.adapter.cursor_msg(&pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Hidden(true)),
    });
    h.control.set_time(2_000_000);
    h.adapter.tick();
    h.sink.finish(1, DecodeResult::Failed);
    h.adapter.closed("again");
    h.adapter.close();
    assert_eq!(h.events(), events);
    assert_eq!(h.sink.frames().len(), 1);
    assert!(h.control.requests().is_empty());
    assert!(h.control.reports().is_empty());
    assert_eq!(h.adapter.stats().failed, 0);
    assert_eq!(h.adapter.stats().dropped, 1);
}

#[test]
fn cursor_forwarding() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.adapter.cursor(&CursorEvent {
        x: 0.25,
        y: 0.5,
        visible: false,
    });
    let m = pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Shape(pb::CursorShape {
            id: 7,
            width: 16,
            height: 16,
            hot_x: 2,
            hot_y: 3,
            scale: 2.0,
            png: vec![1, 2, 3],
        })),
    };
    h.adapter.cursor_msg(&m);
    h.adapter.clock(12_000, -100);
    assert_eq!(
        h.events(),
        [Event::Pointer(0.25, 0.5, false), Event::Cursor(m)]
    );
}

#[test]
fn unknown_codec_ignored() {
    let h = Harness::new(BitstreamFormat::Avcc);
    let mut invalid = config(pb::Codec::Hevc, 3);
    invalid.codec = 99;
    h.adapter.stream_config(&invalid);
    h.adapter.frame(&keyframe(1, 3));
    assert!(h.events().is_empty());
    assert!(h.sink.frames().is_empty());
    assert_eq!(h.adapter.stats().dropped, 1);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(2, 3));
    invalid.epoch = 4;
    h.adapter.stream_config(&invalid);
    h.adapter.frame(&delta(3, 3));
    assert_eq!(h.sink.frames().len(), 2);
}

#[test]
fn parameter_sets_require_each_header_and_ignore_payload_lookalikes() {
    for codec in [VideoCodec::Hevc, VideoCodec::H264] {
        assert!(!has_parameter_sets(&[], codec));
        assert!(!has_parameter_sets(&[0, 0, 0, 1], codec));
        assert!(!has_parameter_sets(&[0, 0, 1, 0, 0, 1], codec));
        assert!(!has_parameter_sets(&[0x40, 0x42, 0x44, 0x67, 0x68], codec));
    }
    assert!(has_parameter_sets(
        &annexb(&[PPS, IDR, VPS, SPS]),
        VideoCodec::Hevc
    ));
    for nals in [[VPS, SPS, IDR], [VPS, PPS, IDR], [SPS, PPS, IDR]] {
        assert!(!has_parameter_sets(&annexb(&nals), VideoCodec::Hevc));
    }
    assert!(!has_parameter_sets(
        &annexb(&[&[0x26, 1, 0x40, 0x42, 0x44]]),
        VideoCodec::Hevc
    ));
    assert!(has_parameter_sets(
        &annexb(&[H264_PPS, H264_IDR, H264_SPS]),
        VideoCodec::H264
    ));
    assert!(!has_parameter_sets(
        &annexb(&[H264_SPS, H264_IDR]),
        VideoCodec::H264
    ));
    assert!(!has_parameter_sets(
        &annexb(&[H264_PPS, H264_IDR]),
        VideoCodec::H264
    ));
    assert!(!has_parameter_sets(
        &annexb(&[&[0x65, 0x67, 0x68]]),
        VideoCodec::H264
    ));
}

#[test]
fn h264_delta_clears_params_and_preserves_missing_capture_clock() {
    let mut f = frame(7, 3, false, annexb(&[H264_SPS, H264_PPS, H264_IDR]));
    f.capture_local_us = None;
    let converted = to_desktop_frame(&f, VideoCodec::H264, BitstreamFormat::Avcc, 259);
    assert_eq!(converted.data, avcc(H264_IDR));
    assert_eq!(converted.vps, None);
    assert_eq!(converted.sps, None);
    assert_eq!(converted.pps, None);
    assert_eq!(converted.capture_local_us, None);
    assert_eq!(converted.epoch, 259);
}

#[test]
fn queue_reports_remaining_submissions_and_latency_saturates() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    h.adapter.frame(&delta(2, 3));
    h.adapter.frame(&delta(3, 3));
    h.control.set_time(999_000);
    let sink = h.sink.clone();
    thread::spawn(move || sink.finish(2, DecodeResult::Shown { decode_us: 8 }))
        .join()
        .unwrap();
    h.sink.finish(1, DecodeResult::Shown { decode_us: 9 });
    h.sink.finish(3, DecodeResult::Shown { decode_us: 10 });
    assert_eq!(h.control.reports(), [(8, 0, 2), (9, 0, 1), (10, 0, 0)]);
    assert_eq!(h.adapter.stats().shown, 3);
    assert_eq!(h.adapter.stats().last_decode_us, 10);
}

#[test]
fn adapter_tick_requests_until_parameterized_keyframe() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&delta(1, 3));
    h.adapter
        .frame(&frame(2, 3, true, annexb(&[SPS, PPS, IDR])));
    assert!(h.sink.frames().is_empty());
    h.control.set_time(1_499_999);
    h.adapter.tick();
    assert!(h.control.requests().is_empty());
    h.control.set_time(1_500_000);
    h.adapter.tick();
    h.control.set_time(2_000_000);
    h.adapter.tick();
    assert_eq!(h.control.requests(), [Backlog, Backlog]);
    h.adapter.frame(&keyframe(3, 3));
    h.control.set_time(3_000_000);
    h.adapter.tick();
    assert_eq!(h.control.requests(), [Backlog, Backlog]);
    assert_eq!(h.adapter.stats().keyframe_requests, 2);
    assert_eq!(h.adapter.stats().dropped, 2);
}

#[test]
fn adapter_backlog_keeps_outstanding_receipts_and_recovers() {
    let h = Harness::new(BitstreamFormat::AnnexB);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    for id in 2..=5 {
        h.adapter.frame(&delta(id, 3));
    }
    assert_eq!(h.sink.frames().len(), 3);
    assert_eq!(h.adapter.stats().dropped, 2);
    assert_eq!(h.control.requests(), [Backlog]);
    h.sink.finish(2, DecodeResult::Failed);
    assert_eq!(h.control.requests(), [Backlog]);
    h.adapter.frame(&keyframe(6, 3));
    h.sink.finish(6, DecodeResult::Shown { decode_us: 5 });
    assert_eq!(h.control.reports(), [(5, 0, 2)]);
    assert_eq!(h.adapter.stats().submitted, 4);
    assert_eq!(h.adapter.stats().failed, 1);
}

#[test]
fn changed_config_invalidates_old_success_even_with_same_wire_epoch() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    // Full epochs differ by 256, so only generation can reject the old done.
    h.configure(pb::Codec::Hevc, 259);
    h.sink.finish(1, DecodeResult::Shown { decode_us: 77 });
    assert!(h.control.reports().is_empty());
    assert_eq!(h.adapter.stats().shown, 0);
    h.adapter.frame(&delta(2, 3));
    assert_eq!(h.sink.frames().len(), 1);
    h.adapter.frame(&keyframe(3, 3));
    assert_eq!(h.sink.frames()[1].epoch, 259);
}

#[test]
fn video_tap_replay_and_synchronous_receipt_work_together() {
    let h = Harness::with_synchronous(
        BitstreamFormat::Avcc,
        Some(DecodeResult::Shown { decode_us: 3 }),
    );
    let tap = VideoTap::default();
    tap.deliver_config(&config(pb::Codec::Hevc, 3));
    let cursor = CursorEvent {
        x: 0.75,
        y: 0.25,
        visible: false,
    };
    tap.deliver_cursor(&cursor);
    let message = pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Use(7)),
    };
    tap.deliver_cursor_msg(&message);
    assert!(tap.attach(h.adapter.clone()));
    assert!(tap.deliver_frame(&keyframe(1, 3)));
    assert!(tap.attach(h.adapter.clone()));
    assert!(tap.deliver_frame(&delta(2, 3)));
    assert_eq!(h.sink.frames().len(), 2);
    assert_eq!(h.control.reports(), [(3, 0, 0), (3, 0, 0)]);
    assert_eq!(
        h.events()
            .iter()
            .filter(|event| matches!(event, Event::Configure(_)))
            .count(),
        1
    );
    assert!(h.events().contains(&Event::Pointer(0.75, 0.25, false)));
    assert!(h.events().contains(&Event::Cursor(message)));
}

#[test]
fn receipts_do_not_keep_adapter_alive() {
    let h = Harness::new(BitstreamFormat::Avcc);
    h.configure(pb::Codec::Hevc, 3);
    h.adapter.frame(&keyframe(1, 3));
    let weak = Arc::downgrade(&h.adapter);
    drop(h.adapter);
    assert!(weak.upgrade().is_none());
    drop(h.sink.take_done(1));
    assert!(h.control.requests().is_empty());
}

#[test]
fn keyframe_reasons_use_protocol_values() {
    assert_eq!(
        DecoderReset.to_pb(),
        pb::keyframe_request::Reason::DecoderReset
    );
    assert_eq!(Backlog.to_pb(), pb::keyframe_request::Reason::Backlog);
    assert_eq!(DecoderReset.to_pb() as i32, 2);
    assert_eq!(Backlog.to_pb() as i32, 4);
}
