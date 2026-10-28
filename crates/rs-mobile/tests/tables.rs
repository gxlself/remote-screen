use std::{
    sync::{mpsc, Arc, Mutex},
    thread,
    time::Duration,
};

use rs_mobile::rscore::{
    dto_core::{OpenIntent, OpenOpts, Quality, SessionPhase, SshState},
    tables::{
        SessionTable, SshTable, TermInput, TermOutput, TermTap, VideoConsumer, VideoTap,
        TERM_BUFFER_CAPACITY,
    },
};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::{pb, video::Frame};
use tokio::time::timeout;

const DEADLINE: Duration = Duration::from_secs(1);

fn opts() -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::Control,
        display_id: Some(7),
        quality: Some(Quality::Clear),
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

fn frame() -> ReceivedFrame {
    ReceivedFrame {
        frame: Frame {
            frame_id: 7,
            epoch: 1,
            display_id: 2,
            flags: 0,
            keyframe: true,
            capture_ts_us: 10,
            data: vec![1, 2, 3],
        },
        received_us: 20,
        capture_local_us: Some(15),
    }
}

#[test]
fn session_ids_are_monotonic_and_summaries_start_connecting() {
    let table = SessionTable::default();
    assert!(table.is_empty());
    for id in 1..=3 {
        let entry = table.insert("h1", "Host", opts(), 123);
        assert_eq!(entry.id, id);
        assert_eq!(entry.host_id, "h1");
        assert_eq!(entry.started_at_us, 123);
        assert_eq!(entry.opts, opts());
        assert!(entry.handle().is_none());
        assert!(!entry.is_cancelled());
        assert!(Arc::ptr_eq(&table.get(id).unwrap(), &entry));
    }
    assert_eq!(table.len(), 3);
    let removed = table.remove(2).unwrap();
    assert_eq!(removed.id, 2);
    assert!(table.get(2).is_none());
    assert!(table.remove(2).is_none());
    assert_eq!(
        table
            .list()
            .iter()
            .map(|entry| entry.id)
            .collect::<Vec<_>>(),
        [1, 3]
    );
    assert_eq!(table.insert("h2", "Other", opts(), 456).id, 4);
    let summaries = table.summaries();
    assert_eq!(
        summaries
            .iter()
            .map(|entry| entry.id.as_str())
            .collect::<Vec<_>>(),
        ["1", "3", "4"]
    );
    let first = &summaries[0];
    assert_eq!(first.phase, SessionPhase::Connecting);
    assert_eq!(first.host_id, "h1");
    assert_eq!(first.host_name, "Host");
    assert_eq!(first.started_at, 123);
    assert_eq!(first.role, None);
    assert_eq!(first.path, None);
    assert_eq!(first.transport, None);
    assert_eq!(first.display_id, None);
}

#[tokio::test]
async fn summaries_update_and_cancellation_wakes_all_waiters() {
    let entry = SessionTable::default().insert("h1", "Host", opts(), 123);
    entry.update_summary(|summary| summary.phase = SessionPhase::Streaming);
    assert_eq!(entry.summary().phase, SessionPhase::Streaming);
    // Enable before signalling readiness: notify_waiters does not retain a permit.
    let mut waiters = Vec::new();
    for _ in 0..2 {
        let entry = entry.clone();
        let (ready_tx, ready_rx) = tokio::sync::oneshot::channel();
        waiters.push(tokio::spawn(async move {
            let notified = entry.cancel.notified();
            tokio::pin!(notified);
            notified.as_mut().enable();
            ready_tx.send(()).unwrap();
            notified.await;
        }));
        timeout(DEADLINE, ready_rx).await.unwrap().unwrap();
    }
    entry.cancel();
    entry.cancel();
    assert!(entry.is_cancelled());
    for waiter in waiters {
        timeout(DEADLINE, waiter).await.unwrap().unwrap();
    }
}

#[derive(Debug, Clone, PartialEq)]
enum VideoEvent {
    Frame(u32),
    Config(pb::StreamConfig),
    Cursor(CursorEvent),
    Message(pb::CursorMsg),
    Clock(u64, i64),
    Closed(String),
    Hook,
}

#[derive(Default)]
struct VideoRecorder {
    events: Arc<Mutex<Vec<VideoEvent>>>,
    on_frame: Option<Box<dyn Fn() + Send + Sync>>,
    on_config: Option<Box<dyn Fn() + Send + Sync>>,
}

impl VideoConsumer for VideoRecorder {
    fn frame(&self, f: &ReceivedFrame) {
        self.events
            .lock()
            .unwrap()
            .push(VideoEvent::Frame(f.frame.frame_id));
        if let Some(callback) = &self.on_frame {
            callback();
        }
    }

    fn stream_config(&self, c: &pb::StreamConfig) {
        self.events.lock().unwrap().push(VideoEvent::Config(*c));
        if let Some(callback) = &self.on_config {
            callback();
        }
    }

    fn cursor(&self, c: &CursorEvent) {
        self.events
            .lock()
            .unwrap()
            .push(VideoEvent::Cursor(c.clone()));
    }

    fn cursor_msg(&self, m: &pb::CursorMsg) {
        self.events
            .lock()
            .unwrap()
            .push(VideoEvent::Message(m.clone()));
    }

    fn clock(&self, rtt_us: u64, offset_us: i64) {
        self.events
            .lock()
            .unwrap()
            .push(VideoEvent::Clock(rtt_us, offset_us));
    }

    fn closed(&self, reason: &str) {
        self.events
            .lock()
            .unwrap()
            .push(VideoEvent::Closed(reason.into()));
    }
}

fn shape(id: u64) -> pb::CursorMsg {
    pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Shape(pb::CursorShape {
            id,
            png: vec![1, 2, 3],
            ..Default::default()
        })),
    }
}

fn use_shape(id: u64) -> pb::CursorMsg {
    pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Use(id)),
    }
}

#[test]
fn video_replays_latest_config_cursor_shape_and_use_before_hook() {
    let tap = VideoTap::default();
    assert!(!tap.deliver_frame(&frame()));
    let config = pb::StreamConfig {
        epoch: 2,
        width: 1920,
        ..Default::default()
    };
    let cursor = CursorEvent {
        x: 0.4,
        y: 0.6,
        visible: true,
    };
    tap.deliver_config(&pb::StreamConfig::default());
    tap.deliver_config(&config);
    tap.deliver_cursor(&CursorEvent {
        x: 0.0,
        y: 0.0,
        visible: false,
    });
    tap.deliver_cursor(&cursor);
    tap.deliver_cursor_msg(&shape(1));
    tap.deliver_cursor_msg(&use_shape(2));
    tap.deliver_cursor_msg(&shape(7));
    tap.deliver_cursor_msg(&use_shape(7));
    let consumer = Arc::new(VideoRecorder::default());
    let events = consumer.events.clone();
    tap.set_attach_hook(Box::new(move || {
        events.lock().unwrap().push(VideoEvent::Hook)
    }));
    assert!(tap.attach(consumer.clone()));
    assert!(tap.is_attached());
    assert!(!tap.is_closed());
    assert_eq!(
        *consumer.events.lock().unwrap(),
        [
            VideoEvent::Config(config),
            VideoEvent::Cursor(cursor),
            VideoEvent::Message(shape(7)),
            VideoEvent::Message(use_shape(7)),
            VideoEvent::Hook,
        ]
    );
    assert!(tap.deliver_frame(&frame()));
    tap.deliver_clock(100, -10);
    assert!(consumer
        .events
        .lock()
        .unwrap()
        .ends_with(&[VideoEvent::Frame(7), VideoEvent::Clock(100, -10)]));
}

#[test]
fn video_replay_discards_cursor_state_before_new_shape_and_keeps_last_hidden() {
    let tap = VideoTap::default();
    tap.deliver_cursor_msg(&use_shape(1));
    tap.deliver_cursor_msg(&shape(2));
    let first = Arc::new(VideoRecorder::default());
    tap.attach(first.clone());
    assert_eq!(
        *first.events.lock().unwrap(),
        [VideoEvent::Message(shape(2))]
    );
    tap.detach();
    let hidden = pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Hidden(true)),
    };
    tap.deliver_cursor_msg(&use_shape(2));
    tap.deliver_cursor_msg(&hidden);
    tap.deliver_cursor_msg(&pb::CursorMsg::default());
    let second = Arc::new(VideoRecorder::default());
    tap.attach(second.clone());
    assert_eq!(
        *second.events.lock().unwrap(),
        [VideoEvent::Message(shape(2)), VideoEvent::Message(hidden)]
    );
}

#[test]
fn video_detach_waits_for_an_inflight_frame_callback() {
    let tap = Arc::new(VideoTap::default());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    tap.attach(Arc::new(VideoRecorder {
        on_frame: Some(Box::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        })),
        ..Default::default()
    }));
    let sender_tap = tap.clone();
    let sender = thread::spawn(move || sender_tap.deliver_frame(&frame()));
    entered_rx.recv_timeout(DEADLINE).unwrap();
    let detach_tap = tap.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let detacher = thread::spawn(move || {
        started_tx.send(()).unwrap();
        detach_tap.detach();
        done_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    let blocked = done_rx.recv_timeout(Duration::from_millis(100));
    release_tx.send(()).unwrap();
    assert_eq!(blocked, Err(mpsc::RecvTimeoutError::Timeout));
    done_rx.recv_timeout(DEADLINE).unwrap();
    assert!(sender.join().unwrap());
    detacher.join().unwrap();
    assert!(!tap.is_attached());
    assert!(!tap.deliver_frame(&frame()));
}

#[test]
fn video_callback_can_detach_itself_without_deadlock() {
    let tap = Arc::new(VideoTap::default());
    let weak = Arc::downgrade(&tap);
    let consumer = Arc::new(VideoRecorder {
        on_frame: Some(Box::new(move || weak.upgrade().unwrap().detach())),
        ..Default::default()
    });
    tap.attach(consumer.clone());
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        assert!(tap.deliver_frame(&frame()));
        assert!(!tap.deliver_frame(&frame()));
        done_tx.send(()).unwrap();
    });
    done_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    assert_eq!(*consumer.events.lock().unwrap(), [VideoEvent::Frame(7)]);
}

#[test]
fn video_callback_can_replace_itself_and_replay_to_new_consumer() {
    let tap = Arc::new(VideoTap::default());
    let replacement = Arc::new(VideoRecorder::default());
    let weak = Arc::downgrade(&tap);
    let next = replacement.clone();
    let first = Arc::new(VideoRecorder {
        on_frame: Some(Box::new(move || {
            let tap = weak.upgrade().unwrap();
            tap.detach();
            assert!(tap.attach(next.clone()));
        })),
        ..Default::default()
    });
    let config = pb::StreamConfig {
        epoch: 3,
        ..Default::default()
    };
    tap.deliver_config(&config);
    tap.attach(first.clone());
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        assert!(tap.deliver_frame(&frame()));
        assert!(tap.deliver_frame(&frame()));
        done_tx.send(()).unwrap();
    });
    done_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    assert_eq!(
        *first.events.lock().unwrap(),
        [VideoEvent::Config(config), VideoEvent::Frame(7)]
    );
    assert_eq!(
        *replacement.events.lock().unwrap(),
        [VideoEvent::Config(config), VideoEvent::Frame(7)]
    );
}

#[test]
fn detaching_during_config_replay_stops_remaining_callbacks() {
    let tap = Arc::new(VideoTap::default());
    tap.deliver_config(&pb::StreamConfig::default());
    tap.deliver_cursor_msg(&shape(7));
    let weak = Arc::downgrade(&tap);
    let consumer = Arc::new(VideoRecorder {
        on_config: Some(Box::new(move || weak.upgrade().unwrap().detach())),
        ..Default::default()
    });
    let recorder = consumer.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        assert!(tap.attach(consumer));
        assert!(!tap.is_attached());
        assert!(!tap.deliver_frame(&frame()));
        done_tx.send(()).unwrap();
    });
    done_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    assert_eq!(
        *recorder.events.lock().unwrap(),
        [VideoEvent::Config(pb::StreamConfig::default())]
    );
}

#[test]
fn video_replacement_and_close_only_notify_the_current_consumer_once() {
    let tap = VideoTap::default();
    let first = Arc::new(VideoRecorder::default());
    let second = Arc::new(VideoRecorder::default());
    let third = Arc::new(VideoRecorder::default());
    assert!(tap.attach(first.clone()));
    assert!(tap.attach(second.clone()));
    assert!(tap.deliver_frame(&frame()));
    tap.close("bye");
    tap.close("again");
    assert!(!tap.attach(third.clone()));
    assert!(!tap.deliver_frame(&frame()));
    tap.deliver_config(&pb::StreamConfig::default());
    tap.deliver_cursor_msg(&shape(7));
    tap.deliver_clock(1, 2);
    assert!(tap.is_closed());
    assert!(!tap.is_attached());
    assert!(first.events.lock().unwrap().is_empty());
    assert!(third.events.lock().unwrap().is_empty());
    assert_eq!(
        *second.events.lock().unwrap(),
        [VideoEvent::Frame(7), VideoEvent::Closed("bye".into())]
    );
    let unattached = VideoTap::default();
    unattached.close("early");
    assert!(!unattached.attach(Arc::new(VideoRecorder::default())));
}

#[derive(Debug, PartialEq, Eq)]
enum TermEvent {
    Output(Vec<u8>),
    Closed,
}

#[derive(Default)]
struct TermRecorder {
    events: Mutex<Vec<TermEvent>>,
    on_output: Option<Box<dyn Fn() + Send + Sync>>,
}

impl TermOutput for TermRecorder {
    fn output(&self, bytes: &[u8]) {
        self.events
            .lock()
            .unwrap()
            .push(TermEvent::Output(bytes.to_vec()));
        if let Some(callback) = &self.on_output {
            callback();
        }
    }
    fn closed(&self) {
        self.events.lock().unwrap().push(TermEvent::Closed);
    }
}

#[tokio::test]
async fn terminal_replays_chunks_then_live_output_and_sends_input_and_sizes() {
    let (tap, mut input) = TermTap::new();
    let consumer = Arc::new(TermRecorder::default());
    for bytes in [b"one".as_slice(), b"two", b"three"] {
        tap.push_output(bytes);
    }
    assert!(tap.attach(consumer.clone()));
    tap.push_output(b"live");
    assert_eq!(tap.last_size(), None);
    assert!(tap.input(b"ls\n"));
    assert_eq!(
        timeout(DEADLINE, input.recv()).await.unwrap(),
        Some(TermInput::Bytes(b"ls\n".to_vec()))
    );
    assert!(!tap.resize(0, 24));
    assert!(!tap.resize(80, 0));
    assert!(input.try_recv().is_err());
    assert!(tap.resize(80, 24));
    assert_eq!(
        timeout(DEADLINE, input.recv()).await.unwrap(),
        Some(TermInput::Resize { cols: 80, rows: 24 })
    );
    assert_eq!(tap.last_size(), Some((80, 24)));
    tap.close();
    tap.close();
    assert!(!tap.input(b"pwd\n"));
    assert!(!tap.resize(120, 30));
    assert_eq!(tap.last_size(), Some((80, 24)));
    assert_eq!(timeout(DEADLINE, input.recv()).await.unwrap(), None);
    assert!(!tap.attach(Arc::new(TermRecorder::default())));
    tap.push_output(b"ignored");
    assert_eq!(
        *consumer.events.lock().unwrap(),
        [
            TermEvent::Output(b"one".to_vec()),
            TermEvent::Output(b"two".to_vec()),
            TermEvent::Output(b"three".to_vec()),
            TermEvent::Output(b"live".to_vec()),
            TermEvent::Closed,
        ]
    );
}

#[test]
fn terminal_buffer_retains_exactly_the_last_64_kib() {
    for chunk_size in [100 * 1024, 40 * 1024, 7 * 1024] {
        let (tap, _input) = TermTap::new();
        let bytes: Vec<_> = (0..100 * 1024).map(|index| (index % 251) as u8).collect();
        tap.push_output(b"old");
        for chunk in bytes.chunks(chunk_size) {
            tap.push_output(chunk);
        }
        let consumer = Arc::new(TermRecorder::default());
        tap.attach(consumer.clone());
        let events = consumer.events.lock().unwrap();
        let actual: Vec<_> = events
            .iter()
            .flat_map(|event| match event {
                TermEvent::Output(bytes) => bytes.clone(),
                TermEvent::Closed => panic!("unexpected close"),
            })
            .collect();
        assert_eq!(actual.len(), TERM_BUFFER_CAPACITY);
        assert_eq!(actual, bytes[bytes.len() - TERM_BUFFER_CAPACITY..]);
    }
}

#[test]
fn terminal_detach_rebuffers_and_replacement_gets_future_output() {
    let (tap, input) = TermTap::new();
    let first = Arc::new(TermRecorder::default());
    let second = Arc::new(TermRecorder::default());
    tap.attach(first.clone());
    tap.push_output(b"first");
    tap.detach();
    tap.push_output(b"buffered");
    tap.attach(second.clone());
    tap.push_output(b"second");
    tap.close();
    assert_eq!(
        *first.events.lock().unwrap(),
        [TermEvent::Output(b"first".to_vec())]
    );
    assert_eq!(
        *second.events.lock().unwrap(),
        [
            TermEvent::Output(b"buffered".to_vec()),
            TermEvent::Output(b"second".to_vec()),
            TermEvent::Closed
        ]
    );
    drop(input);
    let (tap, input) = TermTap::new();
    drop(input);
    assert!(!tap.input(b"ls\n"));
    assert!(!tap.resize(80, 24));
    assert_eq!(tap.last_size(), None);
}

#[test]
fn terminal_detach_waits_for_inflight_output() {
    let (tap, _input) = TermTap::new();
    let tap = Arc::new(tap);
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    tap.attach(Arc::new(TermRecorder {
        on_output: Some(Box::new(move || {
            entered_tx.send(()).unwrap();
            release_rx.lock().unwrap().recv_timeout(DEADLINE).unwrap();
        })),
        ..Default::default()
    }));
    let output_tap = tap.clone();
    let producer = thread::spawn(move || output_tap.push_output(b"live"));
    entered_rx.recv_timeout(DEADLINE).unwrap();
    let detach_tap = tap.clone();
    let (started_tx, started_rx) = mpsc::channel();
    let (done_tx, done_rx) = mpsc::channel();
    let detacher = thread::spawn(move || {
        started_tx.send(()).unwrap();
        detach_tap.detach();
        done_tx.send(()).unwrap();
    });
    started_rx.recv_timeout(DEADLINE).unwrap();
    let blocked = done_rx.recv_timeout(Duration::from_millis(100));
    release_tx.send(()).unwrap();
    assert_eq!(blocked, Err(mpsc::RecvTimeoutError::Timeout));
    done_rx.recv_timeout(DEADLINE).unwrap();
    producer.join().unwrap();
    detacher.join().unwrap();
    tap.push_output(b"buffered");
    let next = Arc::new(TermRecorder::default());
    tap.attach(next.clone());
    assert_eq!(
        *next.events.lock().unwrap(),
        [TermEvent::Output(b"buffered".to_vec())]
    );
}

#[test]
fn terminal_callback_detach_stops_buffer_flush_and_attach_can_replace_it() {
    let (tap, _input) = TermTap::new();
    let tap = Arc::new(tap);
    tap.push_output(b"first");
    tap.push_output(b"second");
    let weak = Arc::downgrade(&tap);
    let first = Arc::new(TermRecorder {
        on_output: Some(Box::new(move || weak.upgrade().unwrap().detach())),
        ..Default::default()
    });
    let first_record = first.clone();
    let replacement = Arc::new(TermRecorder::default());
    let replacement_record = replacement.clone();
    let weak = Arc::downgrade(&tap);
    let second = Arc::new(TermRecorder {
        on_output: Some(Box::new(move || {
            assert!(weak.upgrade().unwrap().attach(replacement.clone()))
        })),
        ..Default::default()
    });
    let second_record = second.clone();
    let (done_tx, done_rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        assert!(tap.attach(first));
        tap.push_output(b"third");
        assert!(tap.attach(second));
        tap.push_output(b"fourth");
        done_tx.send(()).unwrap();
    });
    done_rx.recv_timeout(DEADLINE).unwrap();
    worker.join().unwrap();
    assert_eq!(
        *first_record.events.lock().unwrap(),
        [TermEvent::Output(b"first".to_vec())]
    );
    assert_eq!(
        *second_record.events.lock().unwrap(),
        [TermEvent::Output(b"second".to_vec())]
    );
    assert_eq!(
        *replacement_record.events.lock().unwrap(),
        [
            TermEvent::Output(b"third".to_vec()),
            TermEvent::Output(b"fourth".to_vec())
        ]
    );
}

#[test]
fn ssh_table_has_independent_monotonic_ids_and_connecting_state() {
    let sessions = SessionTable::default();
    assert_eq!(sessions.insert("h1", "Host", opts(), 0).id, 1);
    let ssh = SshTable::default();
    assert!(ssh.is_empty());
    for id in 1..=3 {
        let (term, _input) = TermTap::new();
        let entry = ssh.insert("s1", term);
        assert_eq!(entry.id, id);
        assert_eq!(entry.server_id, "s1");
        assert_eq!(*entry.state.lock().unwrap(), SshState::Connecting);
        assert!(Arc::ptr_eq(&ssh.get(id).unwrap(), &entry));
    }
    assert_eq!(ssh.len(), 3);
    assert_eq!(ssh.remove(2).unwrap().id, 2);
    assert!(ssh.get(2).is_none());
    assert!(ssh.remove(2).is_none());
    assert_eq!(
        ssh.list().iter().map(|entry| entry.id).collect::<Vec<_>>(),
        [1, 3]
    );
    let (term, _input) = TermTap::new();
    assert_eq!(ssh.insert("s2", term).id, 4);
    assert_eq!(sessions.insert("h2", "Host", opts(), 0).id, 2);
}
