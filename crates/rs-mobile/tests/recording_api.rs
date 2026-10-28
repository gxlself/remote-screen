//! ：录制的 uniffi 接口与原生密钥包裹适配器。uniffi 函数走全局 Core，包裹实现也是进程全局的：用例串行。
mod support;

use std::{
    ffi::c_void,
    fmt::Debug,
    fs::{self, OpenOptions},
    io::Write,
    path::PathBuf,
    ptr, slice,
    sync::{
        atomic::{AtomicI32, AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

use host_fake::{
    harness::{TestHost, TestHostOptions},
    FakePlatform,
};
use rs_mobile::rscore::{
    api_diag::settings_set,
    api_sessions as sessions,
    dto_core::SessionPhase,
    error::CoreError,
    recording::{
        api::{
            playback_close, playback_control, playback_open, recording_start, recording_status,
            recording_stop, recordings_delete, recordings_discard_export, recordings_export,
            recordings_list,
        },
        ffi_wrap::{rs_register_secret_wrapper, set_wrapper, wrapper},
        playback::PLAYBACK_ID_BASE,
    },
    runtime::{install_core, take_core, Core},
    store::PairedHostRow,
    tables::SessionTable,
};
use rs_recording::{
    chunk::{ChunkBuilder, ChunkOut},
    crypto::{testing::TestWrapper, SecretWrapper, WrapError},
    file::{finalize_path, RecordingWriter},
    record::{Body, ConfigRec, Record, RecordingEnd, RecordingMeta, VideoRec},
    store::{RecordingStore, MIB},
};
use rsp_core::trust::Permission;
use serde_json::{json, Value};
use tokio::{
    sync::{Mutex, MutexGuard},
    time::{sleep, timeout},
};

static GLOBAL_CORE: Mutex<()> = Mutex::const_new(());
const LIMIT: Duration = Duration::from_secs(15);
const HOUR_US: u64 = 3_600_000_000;
const DAY_US: u64 = 24 * HOUR_US;
const FRAMES: u32 = 60;
const FRAME_US: u64 = 33_333;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const MISSING_SESSION: u64 = 999_999;
const INFO_KEYS: [&str; 10] = [
    "recordingId",
    "hostId",
    "hostName",
    "startedAt",
    "durationMs",
    "bytes",
    "state",
    "hasAudio",
    "width",
    "height",
];

// ---------- 原生回调适配器 ----------

static STATUS: AtomicI32 = AtomicI32::new(0);
/// usize::MAX = 报告实际写出的长度；否则报告这个长度。
static REPORTED_LEN: AtomicUsize = AtomicUsize::new(usize::MAX);
static CAPACITY: AtomicUsize = AtomicUsize::new(0);
static CALLS: AtomicUsize = AtomicUsize::new(0);

/// wrap 输出 `[0xA5] ++ (key ^ 0x5A)`，unwrap 做反向变换；返回码取 STATUS。
extern "C" fn test_cb(
    _ctx: *mut c_void,
    op: u8,
    input: *const u8,
    input_len: usize,
    out: *mut u8,
    out_len: *mut usize,
) -> i32 {
    CALLS.fetch_add(1, Ordering::Relaxed);
    // SAFETY: FfiSecretWrapper passes input_len readable bytes, a writable output
    // buffer and its capacity in *out_len for the duration of this call.
    unsafe {
        let input = slice::from_raw_parts(input, input_len);
        let capacity = *out_len;
        CAPACITY.store(capacity, Ordering::Relaxed);
        let output: Vec<u8> = match (op, input.split_first()) {
            (1, _) => [0xA5]
                .into_iter()
                .chain(input.iter().map(|b| b ^ 0x5A))
                .collect(),
            (2, Some((&0xA5, rest))) => rest.iter().map(|b| b ^ 0x5A).collect(),
            (2, _) => return -2,
            _ => return -1,
        };
        let status = STATUS.load(Ordering::Relaxed);
        if status != 0 {
            return status;
        }
        if output.len() > capacity {
            return -3;
        }
        ptr::copy_nonoverlapping(output.as_ptr(), out, output.len());
        let reported = REPORTED_LEN.load(Ordering::Relaxed);
        *out_len = if reported == usize::MAX {
            output.len()
        } else {
            reported
        };
    }
    0
}

#[test]
fn native_wrapper_round_trips_and_maps_callback_failures() {
    let _serial = GLOBAL_CORE.blocking_lock();
    set_wrapper(None);
    // SAFETY: test_cb ignores ctx and is a plain function valid for the whole process.
    unsafe { rs_register_secret_wrapper(ptr::null_mut(), Some(test_cb)) };
    let native: Arc<dyn SecretWrapper> = wrapper().expect("registered native wrapper");
    assert_eq!(native.kind(), 1);
    let key: [u8; 32] = std::array::from_fn(|i| i as u8);
    let wrapped = native.wrap(&key).unwrap();
    assert_eq!(wrapped.len(), 33);
    assert_eq!(wrapped[0], 0xA5);
    assert_eq!(wrapped[1], 0x5A);
    assert_eq!(CAPACITY.load(Ordering::Relaxed), 256);
    assert_eq!(*native.unwrap(&wrapped).unwrap(), key);
    assert_eq!(CAPACITY.load(Ordering::Relaxed), 256);
    assert_eq!(native.unwrap(&[0x00; 33]).err(), Some(WrapError::Rejected));

    for (status, expected) in [
        (-1, WrapError::Unavailable),
        (-2, WrapError::Rejected),
        (-3, WrapError::Rejected),
        (7, WrapError::Unavailable),
    ] {
        STATUS.store(status, Ordering::Relaxed);
        assert_eq!(native.wrap(&key).err(), Some(expected), "wrap {status}");
        assert_eq!(
            native.unwrap(&wrapped).err(),
            Some(expected),
            "unwrap {status}"
        );
    }
    STATUS.store(0, Ordering::Relaxed);

    // 返回 0 但长度越界：wrap 只收 1..=255，unwrap 只收 32，超过容量一律不读。
    for (len, wrap_ok) in [
        (0, false),
        (31, true),
        (33, true),
        (255, true),
        (256, false),
        (4096, false),
    ] {
        REPORTED_LEN.store(len, Ordering::Relaxed);
        assert_eq!(native.wrap(&key).is_ok(), wrap_ok, "wrap reporting {len}");
        assert_eq!(
            native.unwrap(&wrapped).err(),
            Some(WrapError::Rejected),
            "unwrap reporting {len}"
        );
    }
    REPORTED_LEN.store(usize::MAX, Ordering::Relaxed);

    // NULL cb 被忽略：原来的回调仍然生效。
    let calls = CALLS.load(Ordering::Relaxed);
    // SAFETY: A NULL callback is ignored; ctx is never dereferenced.
    unsafe { rs_register_secret_wrapper(ptr::null_mut(), None) };
    let kept = wrapper().expect("previous registration kept");
    assert_eq!(kept.kind(), 1);
    let rewrapped = kept.wrap(&key).unwrap();
    assert_eq!(rewrapped, wrapped);
    assert_eq!(*kept.unwrap(&rewrapped).unwrap(), key);
    assert_eq!(CALLS.load(Ordering::Relaxed), calls + 2);
    set_wrapper(None);
}

// ---------- uniffi 接口 ----------

#[track_caller]
fn assert_http<T: Debug>(result: Result<T, CoreError>, status: u16, code: &str) {
    match result {
        Err(CoreError::Http {
            status: got,
            hub_code,
            message,
        }) => {
            assert_eq!((got, message.as_str()), (status, code));
            assert_eq!(hub_code, None);
        }
        other => panic!("expected http {status} {code}, got {other:?}"),
    }
}

#[track_caller]
fn assert_unsupported<T: Debug>(result: Result<T, CoreError>) {
    match result {
        Err(CoreError::Unsupported { message }) => assert_eq!(message, "recording.unsupported"),
        other => panic!("expected unsupported, got {other:?}"),
    }
}

fn parse(json: &str) -> Value {
    serde_json::from_str(json).unwrap()
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

fn append(writer: &mut RecordingWriter, chunks: impl IntoIterator<Item = ChunkOut>) {
    for chunk in chunks {
        writer
            .append(chunk.t_start_us, chunk.keyframe_start, &chunk.plaintext)
            .unwrap();
    }
}

struct Fixture {
    core: Arc<Core>,
    wrapper: Arc<TestWrapper>,
    events: Arc<support::EventCollector>,
    host: Option<TestHost>,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    async fn new() -> Self {
        Self::start(false).await
    }

    async fn with_host() -> Self {
        Self::start(true).await
    }

    async fn start(with_host: bool) -> Self {
        let serial = GLOBAL_CORE.lock().await;
        let core = support::test_core_with(|_, options| options.worker_threads = 2);
        let events = support::EventCollector::new();
        core.events().subscribe(events.clone());
        install_core(core.clone());
        let wrapper = Arc::new(TestWrapper::new(1));
        set_wrapper(Some(wrapper.clone()));
        let host = if with_host {
            Some(Self::paired_host(&core).await)
        } else {
            None
        };
        Self {
            core,
            wrapper,
            events,
            host,
            _serial: serial,
        }
    }

    async fn paired_host(core: &Arc<Core>) -> TestHost {
        support::with_identity(core, "Recording API device");
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
                name: "Recording API host".into(),
                addresses: vec![host.quic_addr],
                paired_at_us: core.now_us(),
                permission: Some("control".into()),
            })
            .unwrap();
        host
    }

    fn host_id(&self) -> &str {
        &self.host.as_ref().expect("fixture with a host").host_id
    }

    async fn streaming(&self) -> u64 {
        let opts = serde_json::from_value(json!({ "intent": "control" })).unwrap();
        let id = sessions::open(&self.core, self.host_id(), opts).unwrap();
        until("session streaming", || {
            self.core
                .state::<SessionTable>()
                .get(id)
                .is_some_and(|entry| entry.summary().phase == SessionPhase::Streaming)
        })
        .await;
        id
    }

    fn dir(&self) -> PathBuf {
        self.core.data_dir().join("recordings")
    }

    fn files(&self) -> Vec<String> {
        let mut names: Vec<_> = fs::read_dir(self.dir())
            .unwrap()
            .map(|entry| entry.unwrap().file_name().into_string().unwrap())
            .collect();
        names.sort();
        names
    }

    /// 2 秒、30 fps 的录制，关键帧在 0 和 1 s。complete = false 时不收尾：只留下写完的第一块，
    /// 后面接半个块头，像写到一半时崩溃。
    fn write(&self, host_name: &str, started_at_us: u64, complete: bool) -> String {
        let dir = self.dir();
        fs::create_dir_all(&dir).unwrap();
        let id = RecordingStore::new_id().unwrap();
        let part = dir.join(format!("{id}.rsrec.part"));
        let mut writer =
            RecordingWriter::create(&part, self.wrapper.as_ref(), [7; 16], started_at_us).unwrap();
        let mut builder = ChunkBuilder::new(RecordingMeta {
            host_id: "host-1".into(),
            host_name: host_name.into(),
            role: "controller".into(),
            display_id: Some(1),
            started_at_us,
            app_version: "test".into(),
            audio: false,
        });
        let config = Record {
            t_us: 0,
            body: Body::Config(ConfigRec {
                epoch: 1,
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
        };
        let frames = (0..FRAMES).map(|i| Record {
            t_us: u64::from(i) * FRAME_US,
            body: Body::Video(VideoRec {
                frame_id: i,
                epoch: 1,
                display_id: 1,
                flags: 0,
                keyframe: i % 30 == 0,
                capture_ts_us: u64::from(i) * FRAME_US,
                capture_local_us: None,
                data: vec![0, 0, 0, 1, 0x26, i as u8],
            }),
        });
        for record in std::iter::once(config).chain(frames) {
            append(&mut writer, builder.push(record));
        }
        if complete {
            let (pending, last) = builder.finish(RecordingEnd {
                ended_at_us: started_at_us + 2_000_000,
                reason: "user".into(),
                ..RecordingEnd::default()
            });
            append(&mut writer, pending);
            writer.finish(last.t_start_us, &last.plaintext).unwrap();
            finalize_path(&part).unwrap();
        } else {
            drop(writer);
            OpenOptions::new()
                .append(true)
                .open(&part)
                .unwrap()
                .write_all(&[0, 0, 1, 0, 0, 0, 0])
                .unwrap();
        }
        id
    }

    async fn list(&self) -> Value {
        parse(&recordings_list().await.unwrap())
    }

    async fn ids(&self) -> Vec<String> {
        self.list().await["recordings"]
            .as_array()
            .unwrap()
            .iter()
            .map(|item| item["recordingId"].as_str().unwrap().to_owned())
            .collect()
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        for entry in self.core.state::<SessionTable>().list() {
            sessions::close(&self.core, entry.id);
        }
        set_wrapper(None);
        take_core();
        self.core.shutdown();
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn list_json_recovers_part_files_and_reports_storage() {
    let fixture = Fixture::new().await;
    let now = fixture.core.now_us();
    let older = fixture.write("Older", now - 3 * HOUR_US, true);
    let crashed = fixture.write("Crashed", now - 2 * HOUR_US, false);
    let newer = fixture.write("Newer", now - HOUR_US, true);
    assert!(fixture.files().contains(&format!("{crashed}.rsrec.part")));

    let list = fixture.list().await;
    let mut keys: Vec<_> = list.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["recordings", "storage"]);
    let recordings = list["recordings"].as_array().unwrap();
    for item in recordings {
        for key in INFO_KEYS {
            assert!(item.get(key).is_some(), "{key} missing in {item}");
        }
    }
    let ids: Vec<_> = recordings
        .iter()
        .map(|item| item["recordingId"].as_str().unwrap())
        .collect();
    assert_eq!(ids, [newer.as_str(), crashed.as_str(), older.as_str()]);
    let states: Vec<_> = recordings
        .iter()
        .map(|item| item["state"].as_str().unwrap())
        .collect();
    assert_eq!(states, ["complete", "recovered", "complete"]);
    assert_eq!(
        fixture.files(),
        {
            let mut names = [&older, &crashed, &newer].map(|id| format!("{id}.rsrec"));
            names.sort();
            names
        },
        "the .part was finalized"
    );

    let newest = &recordings[0];
    assert_eq!(newest["hostId"], "host-1");
    assert_eq!(newest["hostName"], "Newer");
    assert_eq!(newest["startedAt"], now - HOUR_US);
    assert_eq!(
        newest["durationMs"],
        u64::from(FRAMES - 1) * FRAME_US / 1000
    );
    assert_eq!(
        newest["bytes"],
        fs::metadata(fixture.dir().join(format!("{newer}.rsrec")))
            .unwrap()
            .len()
    );
    assert_eq!(newest["width"], WIDTH);
    assert_eq!(newest["height"], HEIGHT);
    assert_eq!(newest["hasAudio"], false);
    // 恢复时只剩第一块（关键帧 0 到 1 s 前的 30 帧）。
    let recovered = &recordings[1];
    assert_eq!(recovered["hostName"], "Crashed");
    assert_eq!(recovered["durationMs"], 29 * FRAME_US / 1000);
    assert_eq!(recovered["width"], WIDTH);

    let storage = &list["storage"];
    assert_eq!(storage["capBytes"], 4096 * MIB);
    assert_eq!(storage["retentionDays"], 30);
    let total: u64 = recordings
        .iter()
        .map(|item| item["bytes"].as_u64().unwrap())
        .sum();
    assert_eq!(storage["usedBytes"], total);
    assert!(storage["freeBytes"].is_u64(), "{storage}");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn storage_and_retention_follow_recording_settings() {
    let fixture = Fixture::new().await;
    settings_set("recording.maxTotalMb".into(), "1024".into()).unwrap();
    settings_set("recording.retentionDays".into(), "null".into()).unwrap();
    let old = fixture.write("Old", fixture.core.now_us() - 31 * DAY_US, true);
    let list = fixture.list().await;
    assert_eq!(list["storage"]["capBytes"], 1024 * MIB);
    assert!(list["storage"].get("retentionDays").is_none(), "{list}");
    assert_eq!(fixture.ids().await, [old]);

    settings_set("recording.retentionDays".into(), "30".into()).unwrap();
    let list = fixture.list().await;
    assert_eq!(list["storage"]["retentionDays"], 30);
    assert_eq!(list["recordings"], json!([]));
    assert!(fixture.files().is_empty(), "{:?}", fixture.files());
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn delete_removes_recordings_and_rejects_unknown_ids() {
    let fixture = Fixture::new().await;
    let now = fixture.core.now_us();
    let kept = fixture.write("Kept", now - 2 * HOUR_US, true);
    let gone = fixture.write("Gone", now - HOUR_US, true);
    recordings_delete(gone.clone()).await.unwrap();
    assert_eq!(fixture.ids().await, [kept]);
    assert_http(recordings_delete(gone).await, 404, "recording.notFound");
    assert_http(
        recordings_delete("../x".into()).await,
        404,
        "recording.notFound",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn session_recording_start_status_list_and_stop() {
    let fixture = Fixture::with_host().await;
    assert_http(
        recording_start(MISSING_SESSION).await,
        404,
        "recording.notFound",
    );
    assert_http(recording_stop(MISSING_SESSION), 404, "recording.notFound");
    assert_http(recording_status(MISSING_SESSION), 404, "recording.notFound");

    let id = fixture.streaming().await;
    assert_eq!(parse(&recording_status(id).unwrap())["state"], "idle");
    let started = parse(&recording_start(id).await.unwrap());
    assert_eq!(started["state"], "recording");
    assert_eq!(started["sessionId"], id.to_string());
    let recording_id = started["recordingId"].as_str().unwrap().to_owned();
    let status = parse(&recording_status(id).unwrap());
    assert_eq!(status["state"], "recording");
    assert_eq!(status["sessionId"], id.to_string());
    assert_eq!(status["recordingId"], recording_id);
    assert_http(recording_start(id).await, 409, "recording.alreadyRecording");

    let list = fixture.list().await;
    let recordings = list["recordings"].as_array().unwrap();
    assert_eq!(recordings.len(), 1, "{list}");
    assert_eq!(recordings[0]["state"], "recording");
    assert_eq!(recordings[0]["recordingId"], recording_id);
    assert_eq!(recordings[0]["hostId"], fixture.host_id());
    assert_http(
        recordings_delete(recording_id.clone()).await,
        409,
        "recording.alreadyRecording",
    );

    recording_stop(id).unwrap();
    until("recording stopped event", || {
        fixture
            .events
            .of_type("recording.state")
            .iter()
            .any(|event| event["recordingId"] == recording_id && event["state"] == "stopped")
    })
    .await;
    assert_eq!(parse(&recording_status(id).unwrap())["state"], "idle");
    recording_stop(id).unwrap();
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn playback_ids_and_commands_go_through_json() {
    let fixture = Fixture::new().await;
    let recording_id = fixture.write("Mac", fixture.core.now_us() - HOUR_US, true);
    let info = parse(&playback_open(recording_id.clone()).await.unwrap());
    let playback_id: u64 = info["playbackId"].as_str().unwrap().parse().unwrap();
    assert!(playback_id >= 1 << 61, "{playback_id}");
    assert!(
        (PLAYBACK_ID_BASE..1 << 62).contains(&playback_id),
        "{playback_id}"
    );
    assert_eq!(info["recordingId"], recording_id);
    assert_eq!(info["width"], WIDTH);
    assert_eq!(info["height"], HEIGHT);
    assert_eq!(info["hasAudio"], false);

    playback_control(playback_id, r#"{"op":"seek","positionMs":0}"#.into()).unwrap();
    match playback_control(playback_id, r#"{"op":"jump"}"#.into()) {
        Err(CoreError::Internal { message }) => {
            assert!(message.starts_with("recording.badCommand"), "{message}");
        }
        other => panic!("expected badCommand, got {other:?}"),
    }
    playback_control(playback_id, r#"{"op":"play"}"#.into()).unwrap();
    playback_close(playback_id).unwrap();
    assert_http(
        playback_control(playback_id, r#"{"op":"pause"}"#.into()),
        404,
        "recording.notFound",
    );
    playback_close(playback_id).unwrap();
    assert_http(
        playback_open("../x".into()).await,
        404,
        "recording.notFound",
    );
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn missing_wrapper_is_unsupported() {
    let fixture = Fixture::with_host().await;
    let recording_id = fixture.write("Mac", fixture.core.now_us() - HOUR_US, true);
    let id = fixture.streaming().await;
    set_wrapper(None);
    assert_unsupported(recordings_list().await);
    assert_unsupported(recording_start(id).await);
    assert_unsupported(playback_open(recording_id).await);
    assert_eq!(parse(&recording_status(id).unwrap())["state"], "idle");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn export_returns_a_result_without_panicking() {
    let fixture = Fixture::new().await;
    let recording_id = fixture.write("Mac", fixture.core.now_us() - HOUR_US, true);
    // 导出结果与独立导出测试使用相同的状态约定。
    match recordings_export(recording_id).await {
        Ok(json) => {
            let export = parse(&json);
            assert!(export["mime"].is_string(), "{export}");
            recordings_discard_export(export["path"].as_str().unwrap().to_owned()).unwrap();
        }
        Err(error) => assert_ne!(error, CoreError::internal("task panicked")),
    }
    recordings_discard_export("/nonexistent/export.mp4".into()).unwrap();
}
