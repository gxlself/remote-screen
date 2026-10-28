//! ：录制回放「假会话」。真实 Core、真实时间；每次等待都是「等到条件成立，2 s 超时」，不 sleep 固定时长。
mod support;

use std::{
    fmt::Debug,
    fs,
    path::PathBuf,
    sync::{Arc, Mutex, MutexGuard},
    thread,
    time::{Duration, Instant},
};

use rs_mobile::rscore::{
    adapters::is_adapter_id,
    audio::AudioTable,
    error::CoreError,
    ffi_audio::{rs_audio_close, rs_audio_open},
    recording::{
        dto::PlaybackCommand,
        ffi_wrap::set_wrapper,
        playback::{attach_native, close, control, detach_native, open_playback, PLAYBACK_ID_BASE},
    },
    runtime::{install_core, take_core, Core},
    tables::VideoConsumer,
};
use rs_recording::{
    chunk::{ChunkBuilder, ChunkOut},
    crypto::{testing::TestWrapper, SecretWrapper},
    file::{finalize_path, RecordingWriter},
    record::{
        AudioRec, Body, ConfigRec, CursorShapeRec, Record, RecordingEnd, RecordingMeta, VideoRec,
    },
    store::RecordingStore,
};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::pb;
use support::EventCollector;

const DEADLINE: Duration = Duration::from_secs(2);
const FRAMES: u32 = 60;
const FRAME_US: u64 = 33_333;
const AUDIO_US: u64 = 20_000;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;

// set_wrapper 和 install_core 都是进程全局的。
static SERIAL: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    SERIAL.lock().unwrap_or_else(|error| error.into_inner())
}

#[track_caller]
fn wait_until(name: &str, f: impl Fn() -> bool) {
    let started = Instant::now();
    while !f() {
        assert!(
            started.elapsed() < DEADLINE,
            "timed out after {DEADLINE:?}: {name}"
        );
        thread::sleep(Duration::from_millis(5));
    }
}

#[track_caller]
fn assert_http<T: Debug>(result: Result<T, CoreError>, status: u16, code: &str) {
    match result {
        Err(CoreError::Http {
            status: got,
            message,
            ..
        }) => {
            assert_eq!(got, status, "{message}");
            assert!(message.starts_with(code), "{message}");
        }
        other => panic!("expected http {status} {code}, got {other:?}"),
    }
}

#[derive(Debug, Clone, PartialEq)]
enum Ev {
    Config { epoch: u32, width: u32, height: u32 },
    Frame { id: u32, epoch: u8, keyframe: bool },
    Cursor,
    CursorMsg,
    Closed(String),
}

#[derive(Default)]
struct Probe {
    events: Mutex<Vec<Ev>>,
}

impl Probe {
    fn events(&self) -> Vec<Ev> {
        self.events.lock().unwrap().clone()
    }

    fn frames(&self) -> Vec<(u32, u8, bool)> {
        self.events()
            .into_iter()
            .filter_map(|ev| match ev {
                Ev::Frame {
                    id,
                    epoch,
                    keyframe,
                } => Some((id, epoch, keyframe)),
                _ => None,
            })
            .collect()
    }

    fn push(&self, ev: Ev) {
        self.events.lock().unwrap().push(ev);
    }
}

impl VideoConsumer for Probe {
    fn frame(&self, f: &ReceivedFrame) {
        self.push(Ev::Frame {
            id: f.frame.frame_id,
            epoch: f.frame.epoch,
            keyframe: f.frame.keyframe,
        });
    }

    fn stream_config(&self, c: &pb::StreamConfig) {
        self.push(Ev::Config {
            epoch: c.epoch,
            width: c.width,
            height: c.height,
        });
    }

    fn cursor(&self, _: &CursorEvent) {
        self.push(Ev::Cursor);
    }

    fn cursor_msg(&self, _: &pb::CursorMsg) {
        self.push(Ev::CursorMsg);
    }

    fn clock(&self, _: u64, _: i64) {}

    fn closed(&self, reason: &str) {
        self.push(Ev::Closed(reason.to_owned()));
    }
}

/// 每帧的 epoch 都等于它之前最近一条配置的低 8 位。
#[track_caller]
fn assert_epochs_follow_configs(events: &[Ev]) {
    let mut config = None;
    for ev in events {
        match ev {
            Ev::Config { epoch, .. } => config = Some(*epoch),
            Ev::Frame { id, epoch, .. } => {
                let config = config.unwrap_or_else(|| panic!("frame {id} before any config"));
                assert_eq!(*epoch, config as u8, "frame {id}: {events:?}");
            }
            _ => {}
        }
    }
}

fn recordings_dir(core: &Core) -> PathBuf {
    core.data_dir().join("recordings")
}

fn append(writer: &mut RecordingWriter, chunks: impl IntoIterator<Item = ChunkOut>) {
    for chunk in chunks {
        writer
            .append(chunk.t_start_us, chunk.keyframe_start, &chunk.plaintext)
            .unwrap();
    }
}

/// 2 秒、30 fps、带声音的录制：关键帧在 0 和 1 s，各开一块；声音每 20 ms 一包。
fn write_recording(core: &Core, wrapper: &dyn SecretWrapper) -> String {
    let dir = recordings_dir(core);
    fs::create_dir_all(&dir).unwrap();
    let id = RecordingStore::new_id().unwrap();
    let part = dir.join(format!("{id}.rsrec.part"));
    let mut writer = RecordingWriter::create(&part, wrapper, [7; 16], core.now_us()).unwrap();
    let mut builder = ChunkBuilder::new(RecordingMeta {
        host_id: "host-1".into(),
        host_name: "Mac".into(),
        role: "controller".into(),
        display_id: Some(1),
        started_at_us: core.now_us(),
        app_version: "test".into(),
        audio: true,
    });
    let mut records = vec![
        Record {
            t_us: 0,
            body: Body::Config(ConfigRec {
                epoch: 5,
                display_id: 1,
                codec: 1,
                width: WIDTH,
                height: HEIGHT,
                fps: 30,
                bitrate_kbps: 4000,
                cursor_x: 0.5,
                cursor_y: 0.5,
                cursor_visible: true,
            }),
        },
        Record {
            t_us: 0,
            body: Body::CursorPos {
                x: 0.5,
                y: 0.5,
                visible: true,
            },
        },
        Record {
            t_us: 0,
            body: Body::CursorShape(CursorShapeRec {
                id: 9,
                width: 2,
                height: 2,
                hot_x: 0,
                hot_y: 0,
                scale: 1.0,
                png: vec![0x89, b'P', b'N', b'G'],
            }),
        },
        Record {
            t_us: 0,
            body: Body::CursorUse(9),
        },
    ];
    let mut media = Vec::new();
    for i in 0..FRAMES {
        let t_us = u64::from(i) * FRAME_US;
        media.push(Record {
            t_us,
            body: Body::Video(VideoRec {
                frame_id: i,
                epoch: 5,
                display_id: 1,
                flags: 0,
                keyframe: i % 30 == 0,
                capture_ts_us: 10_000_000 + t_us,
                capture_local_us: None,
                data: vec![0, 0, 0, 1, 0x26, i as u8],
            }),
        });
    }
    let last_frame_us = u64::from(FRAMES - 1) * FRAME_US;
    for (seq, t_us) in (0..last_frame_us).step_by(AUDIO_US as usize).enumerate() {
        media.push(Record {
            t_us,
            body: Body::Audio(AudioRec {
                seq: seq as u16,
                capture_ts_us: 10_000_000 + t_us,
                payload: vec![0xf8, 0xff, 0xfe],
            }),
        });
    }
    // 同一时刻视频排在声音前面：第一个关键帧之前的声音会被 ChunkBuilder 丢掉。
    media.sort_by_key(|record| (record.t_us, !matches!(record.body, Body::Video(_))));
    records.extend(media);
    for record in records {
        append(&mut writer, builder.push(record));
    }
    let (pending, last) = builder.finish(RecordingEnd {
        ended_at_us: core.now_us(),
        reason: "user".into(),
        ..RecordingEnd::default()
    });
    append(&mut writer, pending);
    writer.finish(last.t_start_us, &last.plaintext).unwrap();
    finalize_path(&part).unwrap();
    id
}

fn states(events: &EventCollector, playback_id: &str) -> Vec<String> {
    events
        .of_type("playback.state")
        .into_iter()
        .filter(|event| event["playbackId"] == playback_id)
        .map(|event| event["state"].as_str().unwrap().to_owned())
        .collect()
}

fn has_state_after(states: &[String], first: &str, then: &str) -> bool {
    states
        .iter()
        .position(|state| state == first)
        .is_some_and(|at| states[at..].iter().any(|state| state == then))
}

#[test]
fn playback_feeds_the_recording_through_the_video_tap_and_audio_table() {
    let _serial = serial();
    let wrapper = Arc::new(TestWrapper::new(1));
    set_wrapper(Some(wrapper.clone() as Arc<dyn SecretWrapper>));
    let core = support::test_core();
    install_core(core.clone());
    let events = EventCollector::new();
    core.events().subscribe(events.clone());
    let recording_id = write_recording(&core, wrapper.as_ref());

    // 1. 打开：时长、宽高、声音；AudioTable 里按回放 id 登记，原生的 RsAudio.start 能打开它。
    let info = core.block_on(open_playback(&core, &recording_id)).unwrap();
    assert!(
        (1900..=2000).contains(&info.duration_ms),
        "duration {}",
        info.duration_ms
    );
    assert_eq!((info.width, info.height), (WIDTH, HEIGHT));
    assert!(info.has_audio);
    assert_eq!(info.recording_id, recording_id);
    let id: u64 = info.playback_id.parse().unwrap();
    assert!(id > PLAYBACK_ID_BASE, "{id}");
    assert!(
        !is_adapter_id(id),
        "playback ids must not enter the adapter range"
    );
    let audio = core
        .state::<AudioTable>()
        .get(id)
        .expect("playback audio entry");
    let handle = rs_audio_open(id);
    assert!(!handle.is_null(), "rs_audio_open({id})");
    // SAFETY: handle came from rs_audio_open, nothing renders with it, and it is closed once.
    unsafe { rs_audio_close(handle) };
    wait_until("loading, then paused", || {
        let states = states(&events, &info.playback_id);
        states.len() >= 2 && states[0] == "loading" && states[1] == "paused"
    });

    // 2. attach：钩子触发 Reattach，暂停中也送出配置和跳转点的关键帧。
    let probe = Arc::new(Probe::default());
    assert!(attach_native(&core, id, probe.clone()));
    wait_until("config and keyframe after attach", || {
        probe.frames().iter().any(|&(_, _, keyframe)| keyframe)
    });
    let attached = probe.events();
    let config_at = attached
        .iter()
        .position(|ev| matches!(ev, Ev::Config { .. }))
        .expect("stream config");
    assert_eq!(
        attached[config_at],
        Ev::Config {
            epoch: 1,
            width: WIDTH,
            height: HEIGHT
        }
    );
    assert!(attached[..config_at].is_empty(), "{attached:?}");
    assert_eq!(probe.frames(), vec![(0, 1, true)]);
    assert!(attached.contains(&Ev::Cursor), "{attached:?}");
    assert!(attached.contains(&Ev::CursorMsg), "{attached:?}");

    // 3. 播放：收到全部帧，frame_id 递增，epoch 跟随配置；状态 playing，然后 ended。
    control(&core, id, PlaybackCommand::Play).unwrap();
    wait_until("first half of the frames", || {
        probe.frames().len() >= FRAMES as usize / 2
    });
    wait_until("every frame, then ended", || {
        probe.frames().len() >= FRAMES as usize
            && states(&events, &info.playback_id).contains(&"ended".to_owned())
    });
    let ids: Vec<u32> = probe.frames().iter().map(|&(id, _, _)| id).collect();
    assert_eq!(ids, (0..FRAMES).collect::<Vec<_>>());
    assert_epochs_follow_configs(&probe.events());
    assert!(has_state_after(
        &states(&events, &info.playback_id),
        "playing",
        "ended"
    ));
    assert!(audio.stats().jitter.received > 0, "{:?}", audio.stats());

    // 4. 播放中跳回 0：先收到 epoch 更大的配置，再收到关键帧。
    control(&core, id, PlaybackCommand::Play).unwrap();
    wait_until("playing again from the start", || {
        probe.frames().len() >= FRAMES as usize + 5
    });
    let max_epoch = |events: &[Ev]| {
        events
            .iter()
            .filter_map(|ev| match ev {
                Ev::Config { epoch, .. } => Some(*epoch),
                _ => None,
            })
            .max()
            .unwrap()
    };
    let before = probe.events();
    let old_epoch = max_epoch(&before);
    control(&core, id, PlaybackCommand::Seek { position_ms: 0 }).unwrap();
    let after_seek = |events: &[Ev]| -> Option<(u32, Ev)> {
        let events = &events[before.len()..];
        let (at, epoch) = events.iter().enumerate().find_map(|(at, ev)| match ev {
            Ev::Config { epoch, .. } if *epoch > old_epoch => Some((at, *epoch)),
            _ => None,
        })?;
        let frame = events[at..]
            .iter()
            .find(|ev| matches!(ev, Ev::Frame { .. }))?;
        Some((epoch, frame.clone()))
    };
    wait_until("new epoch, then keyframe", || {
        after_seek(&probe.events()).is_some()
    });
    let (epoch, frame) = after_seek(&probe.events()).unwrap();
    assert_eq!(
        frame,
        Ev::Frame {
            id: 0,
            epoch: epoch as u8,
            keyframe: true
        }
    );
    assert_epochs_follow_configs(&probe.events());

    // 5. 再开同一份：旧回放失效，旧 consumer 收到 closed。
    let second = core.block_on(open_playback(&core, &recording_id)).unwrap();
    let second_id: u64 = second.playback_id.parse().unwrap();
    assert!(second_id > id);
    assert_http(
        control(&core, id, PlaybackCommand::Play),
        404,
        "recording.notFound",
    );
    wait_until("old consumer closed", || {
        probe
            .events()
            .contains(&Ev::Closed("playback closed".into()))
    });
    assert!(core.state::<AudioTable>().get(id).is_none());
    assert!(!attach_native(&core, id, Arc::new(Probe::default())));
    assert!(core.state::<AudioTable>().get(second_id).is_some());

    // 6. close：AudioTable 里没有这个 id，attach_native 返回 false；再 close 也是 Ok。
    let second_probe = Arc::new(Probe::default());
    assert!(attach_native(&core, second_id, second_probe.clone()));
    wait_until("second playback keyframe", || {
        !second_probe.frames().is_empty()
    });
    detach_native(&core, second_id);
    assert!(attach_native(&core, second_id, second_probe.clone()));
    close(&core, second_id).unwrap();
    assert!(core.state::<AudioTable>().get(second_id).is_none());
    assert!(second_probe
        .events()
        .contains(&Ev::Closed("playback closed".into())));
    assert!(!attach_native(&core, second_id, Arc::new(Probe::default())));
    close(&core, second_id).unwrap();
    assert_http(
        control(&core, second_id, PlaybackCommand::Pause),
        404,
        "recording.notFound",
    );
    detach_native(&core, second_id);

    take_core();
    set_wrapper(None);
}

#[test]
fn open_playback_maps_missing_damaged_and_locked_recordings() {
    let _serial = serial();
    let wrapper = Arc::new(TestWrapper::new(1));
    set_wrapper(Some(wrapper.clone() as Arc<dyn SecretWrapper>));
    let core = support::test_core();
    let recording_id = write_recording(&core, wrapper.as_ref());
    let dir = recordings_dir(&core);
    let open = |id: &str| core.block_on(open_playback(&core, id));

    assert_http(open("../rs.db"), 404, "recording.notFound");
    assert_http(open(&"0".repeat(32)), 404, "recording.notFound");
    assert_http(
        open(&recording_id.to_uppercase()),
        404,
        "recording.notFound",
    );

    // 正在录的 .part 不能回放。
    let part_id = RecordingStore::new_id().unwrap();
    fs::copy(
        dir.join(format!("{recording_id}.rsrec")),
        dir.join(format!("{part_id}.rsrec.part")),
    )
    .unwrap();
    assert_http(open(&part_id), 404, "recording.notFound");

    let bad_id = RecordingStore::new_id().unwrap();
    let mut bytes = fs::read(dir.join(format!("{recording_id}.rsrec"))).unwrap();
    bytes[0] ^= 0xff;
    fs::write(dir.join(format!("{bad_id}.rsrec")), bytes).unwrap();
    assert_http(open(&bad_id), 422, "recording.damaged");

    wrapper.set_available(false);
    match open(&recording_id) {
        Err(CoreError::Internal { message }) => {
            assert!(message.starts_with("recording.keyUnavailable"), "{message}")
        }
        other => panic!("expected recording.keyUnavailable, got {other:?}"),
    }
    wrapper.set_available(true);

    set_wrapper(None);
    match open(&recording_id) {
        Err(CoreError::Unsupported { message }) => assert_eq!(message, "recording.unsupported"),
        other => panic!("expected recording.unsupported, got {other:?}"),
    }

    let unknown = PLAYBACK_ID_BASE + 1_000;
    assert_http(
        control(&core, unknown, PlaybackCommand::Play),
        404,
        "recording.notFound",
    );
    close(&core, unknown).unwrap();

    set_wrapper(Some(wrapper.clone() as Arc<dyn SecretWrapper>));
    let info = open(&recording_id).unwrap();
    close(&core, info.playback_id.parse().unwrap()).unwrap();
    set_wrapper(None);
}

#[test]
fn attach_native_rejects_ids_that_are_not_playbacks() {
    let core = support::test_core();
    let probe = Arc::new(Probe::default());
    assert!(!attach_native(&core, 5, probe.clone()));
    assert!(!attach_native(&core, PLAYBACK_ID_BASE, probe.clone()));
    assert!(!attach_native(&core, PLAYBACK_ID_BASE + 1, probe.clone()));
    detach_native(&core, 5);
    detach_native(&core, PLAYBACK_ID_BASE + 1);
    assert!(probe.events().is_empty());
}
