mod support;

use std::{
    collections::BTreeSet,
    path::Path,
    sync::{
        atomic::{AtomicU8, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_sessions as sessions,
    dto_core::{OpenIntent, OpenOpts, SessionPhase},
    recording::{
        dto::RecordingStatusState,
        ffi_wrap::set_wrapper,
        notice,
        recorder::{self, RecorderTuning},
    },
    runtime::Core,
    store::PairedHostRow,
    tables::{SessionEntry, SessionTable, VideoConsumer},
};
use rs_recording::{
    crypto::testing::TestWrapper,
    file::RecordingReader,
    record::{self, Body, RecordingEnd},
    store::{SpaceProbe, StoreConfig, GIB},
};
use rsp_client::{CursorEvent, ReceivedFrame};
use rsp_core::{pb, trust::Permission};
use serde_json::Value;
use tokio::{
    sync::{Mutex, MutexGuard},
    time::{sleep, timeout},
};

// ffi_wrap 的包裹实现是进程全局的：同一进程里的用例串行执行。
static GLOBAL_WRAPPER: Mutex<()> = Mutex::const_new(());
const LIMIT: Duration = Duration::from_secs(15);

fn opts() -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::Control,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

async fn until(what: &str, predicate: impl Fn() -> bool) {
    timeout(LIMIT, async {
        while !predicate() {
            sleep(Duration::from_millis(10)).await;
        }
    })
    .await
    .unwrap_or_else(|_| panic!("{what} within {LIMIT:?}"));
}

struct RoomyProbe;

impl SpaceProbe for RoomyProbe {
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        Some(64 * GIB)
    }
}

/// 数会话泵送出的帧和关键帧，记下最近一条 StreamConfig。
#[derive(Default)]
struct PumpWatch {
    frames: AtomicUsize,
    keyframes: AtomicUsize,
    key_epoch: AtomicU8,
    config: std::sync::Mutex<Option<pb::StreamConfig>>,
}

impl PumpWatch {
    fn frames(&self) -> usize {
        self.frames.load(Ordering::Relaxed)
    }

    fn keyframes(&self) -> usize {
        self.keyframes.load(Ordering::Relaxed)
    }

    /// 最近一个关键帧的 epoch（视频帧头里只有低 8 位）。
    fn key_epoch(&self) -> u8 {
        self.key_epoch.load(Ordering::Relaxed)
    }

    fn config(&self) -> pb::StreamConfig {
        self.config.lock().unwrap().expect("a StreamConfig")
    }
}

impl VideoConsumer for PumpWatch {
    fn frame(&self, frame: &ReceivedFrame) {
        if frame.frame.keyframe {
            self.key_epoch.store(frame.frame.epoch, Ordering::Relaxed);
            self.keyframes.fetch_add(1, Ordering::Relaxed);
        }
        self.frames.fetch_add(1, Ordering::Relaxed);
    }
    fn stream_config(&self, config: &pb::StreamConfig) {
        *self.config.lock().unwrap() = Some(*config);
    }
    fn cursor(&self, _cursor: &CursorEvent) {}
    fn cursor_msg(&self, _message: &pb::CursorMsg) {}
    fn clock(&self, _rtt_us: u64, _offset_us: i64) {}
    fn closed(&self, _reason: &str) {}
}

struct Fixture {
    core: Arc<Core>,
    events: Arc<support::EventCollector>,
    host: TestHost,
    wrapper: Arc<TestWrapper>,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        let serial = GLOBAL_WRAPPER.lock().await;
        let core = support::test_core_with(|_, options| options.worker_threads = 2);
        support::with_identity(&core, "Recording notice test device");
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        let wrapper = Arc::new(TestWrapper::new(1));
        set_wrapper(Some(wrapper.clone()));
        let host = TestHost::start_with_platform(
            FakePlatform::new(),
            TestHostOptions {
                tcp: false,
                ..Default::default()
            },
        )
        .await;
        host.pair(&core.local_identity().unwrap(), Permission::Control);
        core.store()
            .upsert_paired_host(&PairedHostRow {
                host_id: host.host_id.clone(),
                fp: host.fp,
                name: "Recording notice test host".into(),
                addresses: vec![host.quic_addr],
                paired_at_us: core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
        Self {
            core,
            events,
            host,
            wrapper,
            _serial: serial,
        }
    }

    async fn streaming(&self) -> (Arc<SessionEntry>, Arc<PumpWatch>) {
        let id = sessions::open(&self.core, &self.host.host_id, opts()).unwrap();
        let entry = self.core.state::<SessionTable>().get(id).unwrap();
        until("streaming", || {
            entry.summary().phase == SessionPhase::Streaming && entry.summary().display_id.is_some()
        })
        .await;
        let watch = Arc::new(PumpWatch::default());
        assert!(entry.video.attach(watch.clone()));
        until("a StreamConfig", || watch.config.lock().unwrap().is_some()).await;
        (entry, watch)
    }

    async fn start(&self, entry: &Arc<SessionEntry>) -> String {
        let tuning = RecorderTuning {
            sync_every: Duration::from_millis(200),
            state_every: Duration::from_millis(100),
            keyframe_every: Duration::from_secs(1),
            store_config: Some(StoreConfig::default()),
            probe: Some(Arc::new(RoomyProbe)),
            ..RecorderTuning::default()
        };
        let (core, entry) = (self.core.clone(), entry.clone());
        let status = tokio::task::spawn_blocking(move || {
            recorder::start_recording_with(&core, &entry, tuning)
        })
        .await
        .unwrap()
        .unwrap();
        assert_eq!(status.state, RecordingStatusState::Recording);
        status.recording_id.unwrap()
    }

    /// 换画质，等 Host 按新画质重建流发来新 epoch 的 StreamConfig。
    async fn new_config(&self, entry: &SessionEntry, watch: &PumpWatch, quality: &str) {
        let epoch = watch.config().epoch;
        sessions::set_quality(&self.core, entry.id, quality).unwrap();
        until("a StreamConfig with a new epoch", || {
            watch.config().epoch > epoch
        })
        .await;
    }

    /// 这个会话收到的 ended / error 事件。
    fn broken(&self, entry: &SessionEntry) -> Vec<Value> {
        let session_id = entry.id.to_string();
        let ended = self
            .events
            .of_type("session.state")
            .into_iter()
            .filter(|event| event["phase"] == "ended");
        ended
            .chain(self.events.of_type("session.error"))
            .filter(|event| event["sessionId"] == session_id.as_str())
            .collect()
    }

    async fn ended(&self, recording_id: &str) -> Value {
        let ended = || {
            self.events
                .of_type("recording.state")
                .into_iter()
                .filter(|event| event["recordingId"] == recording_id)
                .filter(|event| event["state"] != "recording")
                .collect::<Vec<_>>()
        };
        until("recording ended", || !ended().is_empty()).await;
        let ended = ended();
        assert_eq!(ended.len(), 1, "{ended:?}");
        ended[0].clone()
    }

    /// 录制文件里所有 Config 记录的 epoch，以及收尾记录。
    fn read(&self, recording_id: &str) -> (BTreeSet<u32>, RecordingEnd) {
        let path = self
            .core
            .data_dir()
            .join("recordings")
            .join(format!("{recording_id}.rsrec"));
        let mut reader = RecordingReader::open(&path, self.wrapper.as_ref()).unwrap();
        let mut epochs = BTreeSet::new();
        for index in 0..reader.chunks().len() {
            for record in record::decode(&reader.read_chunk(index).unwrap()).unwrap() {
                if let Body::Config(config) = record.body {
                    epochs.insert(config.epoch);
                }
            }
        }
        let last = record::decode(&reader.read_final().unwrap()).unwrap();
        let [record::Record {
            body: Body::End(end),
            ..
        }] = last.as_slice()
        else {
            panic!("final chunk holds {last:?}");
        };
        (epochs, end.clone())
    }

    async fn close(&self, id: u64) {
        sessions::close(&self.core, id);
        until("session removed", || {
            self.core.state::<SessionTable>().get(id).is_none()
        })
        .await;
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            sessions::close(&self.core, entry.id);
        }
        set_wrapper(None);
        self.core.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn repeated_notices_keep_the_session_streaming() {
    let fixture = Fixture::new().await;
    let (entry, watch) = fixture.streaming().await;
    let id = fixture.start(&entry).await;
    // 录制只从关键帧开始写，前导里才有当前 epoch 的 Config：等到一个录制开始之后的关键帧再换画质，
    // 否则这个 epoch 一帧都没录进去，文件里也就没有它的 Config。
    let keyed = watch.keyframes();
    until("a keyframe after recording started", || {
        watch.keyframes() > keyed
    })
    .await;
    let mut seen = watch.frames();
    until("frames after recording started", || watch.frames() > seen).await;

    // 录制中每条新的 StreamConfig 都会再告知一次 Host。
    for quality in ["eco", "clear"] {
        seen = watch.frames();
        fixture.new_config(&entry, &watch, quality).await;
        let epoch = watch.config().epoch as u8;
        until("a keyframe of the new epoch", || watch.key_epoch() == epoch).await;
        until("frames after the new StreamConfig", || {
            watch.frames() > seen
        })
        .await;
    }
    seen = watch.frames();
    until("frames before stopping", || watch.frames() > seen).await;

    recorder::stop_recording(&entry).unwrap();
    let ended = fixture.ended(&id).await;
    assert_eq!(ended["state"], "stopped");
    assert_eq!(ended["reason"], "user");
    let (epochs, end) = fixture.read(&id);
    assert!(epochs.len() >= 3, "{epochs:?}");
    assert_eq!(end.reason, "user");
    assert!(end.frames > 0, "{end:?}");

    let frames = watch.frames();
    until("frames after stopping", || watch.frames() > frames).await;
    assert_eq!(entry.summary().phase, SessionPhase::Streaming);
    assert!(
        fixture.broken(&entry).is_empty(),
        "{:?}",
        fixture.broken(&entry)
    );
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn sync_without_a_handle_does_nothing() {
    let fixture = Fixture::new().await;
    let connecting = SessionTable::default().insert(
        &fixture.host.host_id,
        "Recording notice test host",
        opts(),
        fixture.core.now_us(),
    );
    assert!(connecting.handle().is_none());
    notice::sync(&connecting, true);
    notice::sync(&connecting, false);

    let (entry, watch) = fixture.streaming().await;
    let handle = entry.handle().unwrap();
    entry.set_handle(None);
    notice::sync(&entry, true);
    notice::sync(&entry, false);
    entry.set_handle(Some(handle));
    let frames = watch.frames();
    until("frames after the handleless sync", || {
        watch.frames() > frames
    })
    .await;
    assert_eq!(entry.summary().phase, SessionPhase::Streaming);
    assert!(
        fixture.broken(&entry).is_empty(),
        "{:?}",
        fixture.broken(&entry)
    );
    fixture.close(entry.id).await;
}
