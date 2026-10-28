mod support;

use std::{
    collections::HashSet,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering},
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
    audio::AudioTable,
    dto_core::{OpenIntent, OpenOpts, SessionPhase},
    error::CoreError,
    recording::{
        dto::{RecordingStatus, RecordingStatusState},
        ffi_wrap::set_wrapper,
        recorder::{self, RecorderTuning},
    },
    runtime::Core,
    store::PairedHostRow,
    tables::{SessionEntry, SessionTable, VideoConsumer},
};
use rs_recording::{
    crypto::testing::TestWrapper,
    file::RecordingReader,
    record::{self, Body, RecordingEnd, RecordingMeta},
    store::{SpaceProbe, StoreConfig, StoredState, GIB, MIB},
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

fn opts(audio: Option<bool>) -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::Control,
        display_id: None,
        quality: None,
        mode: None,
        audio,
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

fn assert_http(result: Result<RecordingStatus, CoreError>, status: u16, code: &str) {
    match result {
        Err(CoreError::Http {
            status: got,
            message,
            ..
        }) => {
            assert_eq!((got, message.as_str()), (status, code));
        }
        other => panic!("expected http {status} {code}, got {other:?}"),
    }
}

struct FakeProbe(AtomicU64);

impl FakeProbe {
    fn set(&self, free: u64) {
        self.0.store(free, Ordering::Relaxed);
    }
}

impl SpaceProbe for FakeProbe {
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        Some(self.0.load(Ordering::Relaxed))
    }
}

/// 数 attach 之后第一个关键帧起的帧数：这些帧录制一定也收到了。
#[derive(Default)]
struct FrameCounter {
    keyed: AtomicBool,
    frames: AtomicUsize,
}

impl VideoConsumer for FrameCounter {
    fn frame(&self, frame: &ReceivedFrame) {
        if frame.frame.keyframe {
            self.keyed.store(true, Ordering::Relaxed);
        }
        if self.keyed.load(Ordering::Relaxed) {
            self.frames.fetch_add(1, Ordering::Relaxed);
        }
    }
    fn stream_config(&self, _config: &pb::StreamConfig) {}
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
    probe: Arc<FakeProbe>,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        let serial = GLOBAL_WRAPPER.lock().await;
        let core = support::test_core_with(|_, options| options.worker_threads = 2);
        support::with_identity(&core, "Recording test device");
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
                name: "Recording test host".into(),
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
            probe: Arc::new(FakeProbe(AtomicU64::new(64 * GIB))),
            _serial: serial,
        }
    }

    fn tuning(&self) -> RecorderTuning {
        RecorderTuning {
            sync_every: Duration::from_millis(200),
            state_every: Duration::from_millis(100),
            keyframe_every: Duration::from_secs(1),
            store_config: Some(StoreConfig::default()),
            probe: Some(self.probe.clone()),
            ..RecorderTuning::default()
        }
    }

    async fn streaming(&self, audio: Option<bool>) -> Arc<SessionEntry> {
        let id = sessions::open(&self.core, &self.host.host_id, opts(audio)).unwrap();
        let entry = self.core.state::<SessionTable>().get(id).unwrap();
        until("streaming", || {
            entry.summary().phase == SessionPhase::Streaming && entry.summary().display_id.is_some()
        })
        .await;
        entry
    }

    async fn start(
        &self,
        entry: &Arc<SessionEntry>,
        tuning: RecorderTuning,
    ) -> Result<RecordingStatus, CoreError> {
        let (core, entry) = (self.core.clone(), entry.clone());
        tokio::task::spawn_blocking(move || recorder::start_recording_with(&core, &entry, tuning))
            .await
            .unwrap()
    }

    async fn started(&self, entry: &Arc<SessionEntry>, tuning: RecorderTuning) -> String {
        let status = self.start(entry, tuning).await.unwrap();
        assert_eq!(status.state, RecordingStatusState::Recording);
        assert_eq!(status.session_id, entry.id.to_string());
        status.recording_id.unwrap()
    }

    /// 等到录制里有了 `ms` 毫秒的媒体（已经录到关键帧）。
    async fn recorded(&self, recording_id: &str, ms: u64) {
        until("recorded media", || {
            recorder::active_recordings(&self.core)
                .iter()
                .any(|active| active.recording_id == recording_id && active.duration_ms >= ms)
        })
        .await;
    }

    fn states(&self, recording_id: &str) -> Vec<Value> {
        self.events
            .of_type("recording.state")
            .into_iter()
            .filter(|event| event["recordingId"] == recording_id)
            .collect()
    }

    /// 等到这份录制的结束事件（stopped / failed）。
    async fn ended(&self, recording_id: &str) -> Value {
        until("recording ended", || {
            self.states(recording_id)
                .iter()
                .any(|event| event["state"] != "recording")
        })
        .await;
        let ended: Vec<_> = self
            .states(recording_id)
            .into_iter()
            .filter(|event| event["state"] != "recording")
            .collect();
        assert_eq!(ended.len(), 1, "{ended:?}");
        ended[0].clone()
    }

    fn dir(&self) -> PathBuf {
        self.core.data_dir().join("recordings")
    }

    fn files(&self) -> Vec<String> {
        let Ok(dir) = std::fs::read_dir(self.dir()) else {
            return Vec::new();
        };
        let mut names: Vec<_> = dir
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    fn read(&self, recording_id: &str) -> (RecordingMeta, RecordingEnd) {
        let path = self.dir().join(format!("{recording_id}.rsrec"));
        let mut reader = RecordingReader::open(&path, self.wrapper.as_ref()).unwrap();
        let first = record::decode(&reader.read_chunk(0).unwrap()).unwrap();
        let Body::Meta(meta) = &first[0].body else {
            panic!("chunk 0 starts with {:?}", first[0].body);
        };
        let last = record::decode(&reader.read_final().unwrap()).unwrap();
        let [record::Record {
            body: Body::End(end),
            ..
        }] = last.as_slice()
        else {
            panic!("final chunk holds {last:?}");
        };
        (meta.clone(), end.clone())
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
async fn user_stop_finalizes_an_encrypted_recording() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(None).await;
    let id = fixture.started(&entry, fixture.tuning()).await;
    assert_eq!(
        recorder::active_ids(&fixture.core),
        HashSet::from([id.clone()])
    );
    assert!(entry.recorder.is_recording());
    let counter = Arc::new(FrameCounter::default());
    assert!(entry.video.attach(counter.clone()));
    until("40 frames", || counter.frames.load(Ordering::Relaxed) >= 40).await;
    until("a recording.state tick", || {
        fixture
            .states(&id)
            .iter()
            .any(|event| event["state"] == "recording" && event["bytes"].as_u64() > Some(0))
    })
    .await;

    recorder::stop_recording(&entry).unwrap();
    let ended = fixture.ended(&id).await;
    assert_eq!(ended["state"], "stopped");
    assert_eq!(ended["reason"], "user");
    assert_eq!(ended["sessionId"], entry.id.to_string());
    assert_eq!(fixture.files(), [format!("{id}.rsrec")]);
    let bytes = std::fs::metadata(fixture.dir().join(format!("{id}.rsrec")))
        .unwrap()
        .len();
    assert_eq!(ended["bytes"].as_u64(), Some(bytes));

    let (meta, end) = fixture.read(&id);
    assert_eq!(meta.host_id, fixture.host.host_id);
    assert_eq!(meta.host_name, entry.summary().host_name);
    assert_eq!(meta.role, "controller");
    assert!(meta.audio);
    assert!(end.frames >= 30, "{end:?}");
    assert!(end.keyframes >= 1, "{end:?}");
    assert_eq!(end.reason, "user");
    assert!(end.width > 0 && end.height > 0, "{end:?}");

    assert_eq!(recorder::status(&entry).state, RecordingStatusState::Idle);
    assert!(!entry.recorder.is_recording());
    assert!(recorder::active_ids(&fixture.core).is_empty());
    // 幂等：没在录时停止什么也不做。
    recorder::stop_recording(&entry).unwrap();
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn start_refusals_map_to_contract_errors() {
    let fixture = Fixture::new().await;
    let connecting = SessionTable::default().insert(
        &fixture.host.host_id,
        "Recording test host",
        opts(None),
        fixture.core.now_us(),
    );
    assert_http(
        fixture.start(&connecting, fixture.tuning()).await,
        409,
        "recording.notStreaming",
    );

    let entry = fixture.streaming(None).await;
    set_wrapper(None);
    match fixture.start(&entry, fixture.tuning()).await {
        Err(CoreError::Unsupported { message }) => assert_eq!(message, "recording.unsupported"),
        other => panic!("expected unsupported, got {other:?}"),
    }
    set_wrapper(Some(fixture.wrapper.clone()));
    fixture.wrapper.set_available(false);
    match fixture.start(&entry, fixture.tuning()).await {
        Err(CoreError::Internal { message }) => assert_eq!(message, "recording.keyUnavailable"),
        other => panic!("expected keyUnavailable, got {other:?}"),
    }
    assert!(fixture.files().is_empty(), "{:?}", fixture.files());
    assert_eq!(recorder::status(&entry).state, RecordingStatusState::Idle);

    fixture.wrapper.set_available(true);
    let id = fixture.started(&entry, fixture.tuning()).await;
    assert_http(
        fixture.start(&entry, fixture.tuning()).await,
        409,
        "recording.alreadyRecording",
    );
    assert_eq!(
        recorder::status(&entry).recording_id.as_deref(),
        Some(id.as_str())
    );
    recorder::stop_recording(&entry).unwrap();
    assert_eq!(fixture.ended(&id).await["reason"], "user");
    assert!(!fixture.files().iter().any(|name| name.ends_with(".part")));
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn closing_the_session_finalizes_the_recording() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(None).await;
    let id = fixture.started(&entry, fixture.tuning()).await;
    fixture.recorded(&id, 300).await;
    fixture.close(entry.id).await;
    let ended = fixture.ended(&id).await;
    assert_eq!(ended["state"], "stopped");
    assert_eq!(ended["reason"], "sessionEnded");
    assert_eq!(fixture.files(), [format!("{id}.rsrec")]);
    assert_eq!(fixture.read(&id).1.reason, "sessionEnded");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn max_duration_stops_automatically() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(None).await;
    let tuning = RecorderTuning {
        store_config: Some(StoreConfig {
            max_duration_us: 1_000_000,
            ..StoreConfig::default()
        }),
        ..fixture.tuning()
    };
    let id = fixture.started(&entry, tuning).await;
    let ended = fixture.ended(&id).await;
    assert_eq!(ended["state"], "stopped");
    assert_eq!(ended["reason"], "maxDuration");
    let (_, end) = fixture.read(&id);
    assert_eq!(end.reason, "maxDuration");
    assert!(end.duration_us >= 1_000_000, "{end:?}");
    assert_eq!(entry.summary().phase, SessionPhase::Streaming);
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn low_disk_refuses_to_start_and_stops_recording() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(None).await;
    fixture.probe.set(900 * MIB);
    assert_http(
        fixture.start(&entry, fixture.tuning()).await,
        507,
        "recording.diskLow",
    );
    assert!(fixture.files().is_empty(), "{:?}", fixture.files());

    fixture.probe.set(2 * GIB);
    let id = fixture.started(&entry, fixture.tuning()).await;
    fixture.recorded(&id, 300).await;
    fixture.probe.set(400 * MIB);
    let ended = fixture.ended(&id).await;
    assert_eq!(ended["state"], "stopped");
    assert_eq!(ended["reason"], "diskLow");
    assert_eq!(fixture.read(&id).1.reason, "diskLow");
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn requested_audio_is_recorded() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(Some(true)).await;
    let received = || {
        fixture
            .core
            .state::<AudioTable>()
            .get(entry.id)
            .map_or(0, |audio| audio.stats().jitter.received)
    };
    let id = fixture.started(&entry, fixture.tuning()).await;
    fixture.recorded(&id, 300).await;
    let before = received();
    until("audio packets", || received() >= before + 20).await;
    recorder::stop_recording(&entry).unwrap();
    assert_eq!(fixture.ended(&id).await["reason"], "user");
    let (meta, end) = fixture.read(&id);
    assert!(meta.audio);
    assert!(end.audio_packets > 0, "{end:?}");
    fixture.close(entry.id).await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn crashed_recording_is_recovered_from_its_part_file() {
    let fixture = Fixture::new().await;
    let entry = fixture.streaming(None).await;
    let tuning = RecorderTuning {
        crash_after_chunks: Some(2),
        ..fixture.tuning()
    };
    let id = fixture.started(&entry, tuning).await;
    until("writer crashed", || {
        !recorder::active_ids(&fixture.core).contains(&id)
    })
    .await;
    assert_eq!(fixture.files(), [format!("{id}.rsrec.part")]);
    assert!(fixture
        .states(&id)
        .iter()
        .all(|event| event["state"] == "recording"));

    let store = recorder::store(&fixture.core).unwrap();
    store.recover_all(
        fixture.wrapper.as_ref(),
        &recorder::active_ids(&fixture.core),
        fixture.core.now_us(),
    );
    assert_eq!(fixture.files(), [format!("{id}.rsrec")]);
    let listed = store.list(fixture.wrapper.as_ref()).unwrap();
    assert_eq!(listed.len(), 1);
    assert_eq!(listed[0].id, id);
    assert_eq!(listed[0].state, StoredState::Recovered);
    assert_eq!(listed[0].meta.host_id, fixture.host.host_id);
    assert!(listed[0].end.frames > 0);
    fixture.close(entry.id).await;
}
