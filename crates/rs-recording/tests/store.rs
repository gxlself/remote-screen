use std::collections::HashSet;
use std::fs::{self, OpenOptions};
use std::io::{Read, Seek, SeekFrom, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use data_encoding::HEXLOWER;
use rs_recording::chunk::{ChunkBuilder, ChunkOut};
use rs_recording::crypto::testing::TestWrapper;
use rs_recording::crypto::{SecretWrapper, WrapError};
use rs_recording::file::{
    finalize_path, read_header, RecordingReader, RecordingWriter, RecoverKind, CHUNK_HEADER_LEN,
};
use rs_recording::record::{
    decode, AudioRec, Body, ConfigRec, Record, RecordingEnd, RecordingMeta, VideoRec,
};
use rs_recording::store::{
    RecordingStore, RecoverReport, SpaceProbe, StartRefusal, StopReason, StoreConfig, StoreError,
    StoredRecording, StoredState, GIB, MIB,
};
use zeroize::Zeroizing;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const DAY_US: u64 = 86_400_000_000;
const NOW_US: u64 = 1_000 * DAY_US;
const FRAME_US: u64 = 100_000;
/// 文件头的 created_at 比 Meta 的 started_at 晚一点，好区分两者。
const CREATED_SKEW_US: u64 = 1_000;
const NO_PROBE: u64 = u64::MAX;

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-recording-store-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).unwrap();
        Self(path)
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

/// 可用空间；`NO_PROBE` 表示查不到。
struct FakeProbe(AtomicU64);

impl FakeProbe {
    fn set(&self, free: Option<u64>) {
        self.0.store(free.unwrap_or(NO_PROBE), Ordering::SeqCst);
    }
}

impl SpaceProbe for FakeProbe {
    fn free_bytes(&self, _dir: &Path) -> Option<u64> {
        let free = self.0.load(Ordering::SeqCst);
        (free != NO_PROBE).then_some(free)
    }
}

struct CountingWrapper {
    inner: TestWrapper,
    unwraps: AtomicUsize,
}

impl CountingWrapper {
    fn unwraps(&self) -> usize {
        self.unwraps.load(Ordering::SeqCst)
    }
}

impl SecretWrapper for CountingWrapper {
    fn wrap(&self, key: &[u8; 32]) -> Result<Vec<u8>, WrapError> {
        self.inner.wrap(key)
    }

    fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>, WrapError> {
        self.unwraps.fetch_add(1, Ordering::SeqCst);
        self.inner.unwrap(wrapped)
    }

    fn kind(&self) -> u8 {
        self.inner.kind()
    }
}

struct Fixture {
    _dir: TestDir,
    probe: Arc<FakeProbe>,
    store: RecordingStore,
    wrapper: CountingWrapper,
}

fn fixture() -> Fixture {
    let dir = TestDir::new();
    let probe = Arc::new(FakeProbe(AtomicU64::new(NO_PROBE)));
    let store = RecordingStore::open(dir.0.join("data").join("recordings"), probe.clone()).unwrap();
    Fixture {
        _dir: dir,
        probe,
        store,
        wrapper: CountingWrapper {
            inner: TestWrapper::new(7),
            unwraps: AtomicUsize::new(0),
        },
    }
}

fn none() -> HashSet<String> {
    HashSet::new()
}

fn week() -> StoreConfig {
    StoreConfig {
        retention_days: Some(7),
        ..StoreConfig::default()
    }
}

fn meta(started_at_us: u64, audio: bool) -> RecordingMeta {
    RecordingMeta {
        host_id: "host-1".into(),
        host_name: "Studio Mac".into(),
        role: "controller".into(),
        display_id: Some(1),
        started_at_us,
        app_version: "1.0.0".into(),
        audio,
    }
}

fn config() -> ConfigRec {
    ConfigRec {
        epoch: 1,
        display_id: 1,
        codec: 1,
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 4000,
        cursor_x: 0.0,
        cursor_y: 0.0,
        cursor_visible: true,
    }
}

/// `frames` 帧，10 fps，每 10 帧一个关键帧，所以每个数据块正好 10 帧；有声音时每帧后跟一个声音包。
fn chunks(started_at_us: u64, frames: u64, audio: bool) -> (Vec<ChunkOut>, ChunkOut) {
    let mut builder = ChunkBuilder::new(meta(started_at_us, audio));
    let mut out = builder.push(Record {
        t_us: 0,
        body: Body::Config(config()),
    });
    for i in 0..frames {
        let t_us = i * FRAME_US;
        out.extend(builder.push(Record {
            t_us,
            body: Body::Video(VideoRec {
                frame_id: i as u32,
                epoch: 1,
                display_id: 1,
                flags: 0,
                keyframe: i % 10 == 0,
                capture_ts_us: t_us,
                capture_local_us: None,
                data: vec![i as u8; 64],
            }),
        }));
        if audio {
            out.extend(builder.push(Record {
                t_us,
                body: Body::Audio(AudioRec {
                    seq: i as u16,
                    capture_ts_us: t_us,
                    payload: vec![0xAA; 20],
                }),
            }));
        }
    }
    let (pending, final_chunk) = builder.finish(RecordingEnd {
        reason: "stopped".into(),
        ended_at_us: started_at_us + 1,
        ..RecordingEnd::default()
    });
    out.extend(pending);
    (out, final_chunk)
}

fn video_frames(chunk: &ChunkOut) -> u64 {
    decode(&chunk.plaintext)
        .unwrap()
        .iter()
        .filter(|record| matches!(record.body, Body::Video(_)))
        .count() as u64
}

fn write_chunks(
    store: &RecordingStore,
    wrapper: &dyn SecretWrapper,
    id: &str,
    started_at_us: u64,
    chunks: &[ChunkOut],
) -> (RecordingWriter, Vec<u64>) {
    let file_id: [u8; 16] = HEXLOWER.decode(id.as_bytes()).unwrap().try_into().unwrap();
    let mut writer = RecordingWriter::create(
        &store.part_path(id),
        wrapper,
        file_id,
        started_at_us + CREATED_SKEW_US,
    )
    .unwrap();
    let offsets = chunks
        .iter()
        .map(|chunk| {
            writer
                .append(chunk.t_start_us, chunk.keyframe_start, &chunk.plaintext)
                .unwrap()
        })
        .collect();
    (writer, offsets)
}

fn record_as(
    store: &RecordingStore,
    wrapper: &dyn SecretWrapper,
    id: &str,
    started_at_us: u64,
    frames: u64,
    audio: bool,
) {
    let (data, final_chunk) = chunks(started_at_us, frames, audio);
    let (writer, _) = write_chunks(store, wrapper, id, started_at_us, &data);
    writer
        .finish(final_chunk.t_start_us, &final_chunk.plaintext)
        .unwrap();
    assert_eq!(
        finalize_path(&store.part_path(id)).unwrap(),
        store.final_path(id)
    );
}

/// 一份完整录制（`<id>.rsrec`）。
fn record(
    store: &RecordingStore,
    wrapper: &dyn SecretWrapper,
    started_at_us: u64,
    frames: u64,
    audio: bool,
) -> String {
    let id = RecordingStore::new_id().unwrap();
    record_as(store, wrapper, &id, started_at_us, frames, audio);
    id
}

/// 写了一半的 `<id>.rsrec.part`：所有数据块都在，没有 FINAL 和文件尾。返回数据块和它们的偏移。
fn record_part(
    store: &RecordingStore,
    wrapper: &dyn SecretWrapper,
    started_at_us: u64,
    frames: u64,
    audio: bool,
) -> (String, Vec<ChunkOut>, Vec<u64>) {
    let id = RecordingStore::new_id().unwrap();
    let (data, _) = chunks(started_at_us, frames, audio);
    let (mut writer, offsets) = write_chunks(store, wrapper, &id, started_at_us, &data);
    writer.sync().unwrap();
    (id, data, offsets)
}

fn flip(path: &Path, offset: u64) {
    let mut file = OpenOptions::new()
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.seek(SeekFrom::Start(offset)).unwrap();
    let mut byte = [0u8];
    file.read_exact(&mut byte).unwrap();
    byte[0] ^= 0x80;
    file.seek(SeekFrom::Start(offset)).unwrap();
    file.write_all(&byte).unwrap();
}

fn corrupt_first_chunk(path: &Path, wrapper: &dyn SecretWrapper) {
    let offset = RecordingReader::open(path, wrapper).unwrap().chunks()[0].offset;
    flip(path, offset + CHUNK_HEADER_LEN as u64 + 4);
}

fn set_version(path: &Path, version: u8) {
    let mut file = OpenOptions::new().write(true).open(path).unwrap();
    file.seek(SeekFrom::Start(8)).unwrap();
    file.write_all(&[version]).unwrap();
}

fn truncate(path: &Path, len: u64) {
    OpenOptions::new()
        .write(true)
        .open(path)
        .unwrap()
        .set_len(len)
        .unwrap();
}

fn modified(path: &Path) -> SystemTime {
    fs::metadata(path).unwrap().modified().unwrap()
}

/// 把 mtime 定到一个固定的旧时刻，好让两个先后写出的文件有相同的 mtime。
/// GPFS 上刚写完的文件，把 mtime 往回拨几毫秒会被忽略，拨到很久以前才生效。
fn pin_mtime(path: &Path) {
    let pinned = UNIX_EPOCH + Duration::from_secs(1_600_000_000);
    fs::File::open(path).unwrap().set_modified(pinned).unwrap();
    assert_eq!(modified(path), pinned);
}

fn len(path: &Path) -> u64 {
    fs::metadata(path).unwrap().len()
}

fn ids(list: &[StoredRecording]) -> Vec<String> {
    list.iter().map(|recording| recording.id.clone()).collect()
}

fn find<'a>(list: &'a [StoredRecording], id: &str) -> &'a StoredRecording {
    list.iter()
        .find(|recording| recording.id == id)
        .unwrap_or_else(|| panic!("{id} is not listed"))
}

fn damaged_end() -> RecordingEnd {
    RecordingEnd {
        reason: "damaged".into(),
        ..RecordingEnd::default()
    }
}

#[test]
fn ids_are_random_lowercase_hex_and_paths_stay_in_the_directory() {
    let fx = fixture();
    assert!(fx.store.dir().is_dir());
    assert!(fx.store.dir().ends_with("data/recordings"));
    let a = RecordingStore::new_id().unwrap();
    let b = RecordingStore::new_id().unwrap();
    assert_eq!(a.len(), 32);
    assert!(a.bytes().all(|c| matches!(c, b'0'..=b'9' | b'a'..=b'f')));
    assert_ne!(a, b);
    assert!(RecordingStore::valid_id(&a));
    assert!(RecordingStore::valid_id(&b));
    let upper = a.to_uppercase();
    let longer = format!("{a}0");
    let traversal = format!("../{}", &a[3..]);
    let non_hex = "g".repeat(32);
    for bad in [
        "../x",
        "",
        upper.as_str(),
        &a[..31],
        longer.as_str(),
        traversal.as_str(),
        non_hex.as_str(),
    ] {
        assert!(!RecordingStore::valid_id(bad), "{bad:?}");
    }
    assert_eq!(
        fx.store.final_path(&a),
        fx.store.dir().join(format!("{a}.rsrec"))
    );
    assert_eq!(
        fx.store.part_path(&a),
        fx.store.dir().join(format!("{a}.rsrec.part"))
    );
}

#[test]
fn list_orders_newest_first_with_complete_fields() {
    let fx = fixture();
    let first = record(&fx.store, &fx.wrapper, 1, 12, false);
    let third = record(&fx.store, &fx.wrapper, 3, 5, true);
    let second = record(&fx.store, &fx.wrapper, 2, 30, false);
    let list = fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(ids(&list), [third.clone(), second.clone(), first.clone()]);
    for (recording, started_at_us, frames, audio) in [
        (&list[0], 3, 5, true),
        (&list[1], 2, 30, false),
        (&list[2], 1, 12, false),
    ] {
        assert_eq!(recording.meta, meta(started_at_us, audio));
        assert_eq!(recording.state, StoredState::Complete);
        assert_eq!(recording.has_audio, audio);
        assert_eq!(recording.bytes, len(&fx.store.final_path(&recording.id)));
        let end = &recording.end;
        assert_eq!(end.reason, "stopped");
        assert_eq!(end.ended_at_us, started_at_us + 1);
        assert_eq!(end.frames, frames);
        assert_eq!(end.keyframes, frames.div_ceil(10));
        assert_eq!(end.audio_packets, if audio { frames } else { 0 });
        assert_eq!(end.duration_us, (frames - 1) * FRAME_US);
        assert_eq!((end.width, end.height, end.codec), (1280, 720, 1));
    }
    assert_eq!(fx.store.get(&second, &fx.wrapper).unwrap(), list[1]);
    assert!(matches!(
        fx.store
            .get(&RecordingStore::new_id().unwrap(), &fx.wrapper),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        fx.store.get("../x", &fx.wrapper),
        Err(StoreError::InvalidId)
    ));
}

#[test]
fn list_reuses_cached_entries_until_the_file_changes() {
    let fx = fixture();
    let a = record(&fx.store, &fx.wrapper, 5, 10, false);
    let b = record(&fx.store, &fx.wrapper, 6, 10, false);
    pin_mtime(&fx.store.final_path(&b));
    let c = record(&fx.store, &fx.wrapper, 7, 10, false);
    let first = fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(fx.wrapper.unwraps(), 3);
    assert_eq!(fx.store.list(&fx.wrapper).unwrap(), first);
    assert_eq!(fx.store.get(&b, &fx.wrapper).unwrap(), *find(&first, &b));
    assert_eq!(fx.wrapper.unwraps(), 3);

    let path = fx.store.final_path(&a);
    let mut file = OpenOptions::new().append(true).open(&path).unwrap();
    file.write_all(&[0]).unwrap();
    drop(file);
    let list = fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(fx.wrapper.unwraps(), 4);
    let damaged = find(&list, &a);
    assert_eq!(damaged.state, StoredState::Damaged);
    assert_eq!(damaged.bytes, len(&path));
    assert_eq!(damaged.end, damaged_end());
    assert_eq!(find(&list, &b).state, StoredState::Complete);
    assert_eq!(find(&list, &c).state, StoredState::Complete);
    fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(fx.wrapper.unwraps(), 4);

    // 文件消失后缓存条目跟着删掉：同 id、同长度、同 mtime 的新文件必须重新读。
    let path = fx.store.final_path(&b);
    let old_len = len(&path);
    fs::remove_file(&path).unwrap();
    // a 损坏后开始时间取文件头的 created_at，排到最前。
    assert_eq!(ids(&fx.store.list(&fx.wrapper).unwrap()), [a, c]);
    record_as(&fx.store, &fx.wrapper, &b, 8, 10, false);
    pin_mtime(&path);
    assert_eq!(len(&path), old_len);
    let list = fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(fx.wrapper.unwraps(), 5);
    assert_eq!(find(&list, &b).meta, meta(8, false));
}

#[test]
fn corrupt_first_chunk_is_listed_as_damaged_with_header_time() {
    let fx = fixture();
    let good = record(&fx.store, &fx.wrapper, 10, 10, false);
    let bad = record(&fx.store, &fx.wrapper, 20, 10, true);
    let path = fx.store.final_path(&bad);
    corrupt_first_chunk(&path, &fx.wrapper);
    let list = fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(ids(&list), [bad.clone(), good.clone()]);
    let damaged = find(&list, &bad);
    assert_eq!(damaged.state, StoredState::Damaged);
    assert_eq!(
        damaged.meta.started_at_us,
        read_header(&path).unwrap().created_at_us
    );
    assert_eq!(damaged.meta.started_at_us, 20 + CREATED_SKEW_US);
    assert_eq!(damaged.meta.host_id, "");
    assert_eq!(damaged.meta.host_name, "");
    assert!(!damaged.meta.audio);
    assert_eq!(damaged.end, damaged_end());
    assert!(!damaged.has_audio);
    assert_eq!(damaged.bytes, len(&path));
    let good = find(&list, &good);
    assert_eq!(good.state, StoredState::Complete);
    assert_eq!(good.meta, meta(10, false));
    assert_eq!(good.end.frames, 10);
}

#[test]
fn unavailable_master_key_fails_the_whole_list() {
    let fx = fixture();
    let id = record(&fx.store, &fx.wrapper, NOW_US - 30 * DAY_US, 10, false);
    fx.wrapper.inner.set_available(false);
    assert!(matches!(
        fx.store.list(&fx.wrapper),
        Err(StoreError::KeyUnavailable)
    ));
    assert!(matches!(
        fx.store.get(&id, &fx.wrapper),
        Err(StoreError::KeyUnavailable)
    ));
    assert!(fx
        .store
        .apply_retention(&week(), &fx.wrapper, NOW_US, &none())
        .is_empty());
    assert!(fx.store.final_path(&id).exists());
    fx.wrapper.inner.set_available(true);
    let list = fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(ids(&list), [id]);
    assert_eq!(list[0].state, StoredState::Complete);
}

#[test]
fn recover_all_finishes_a_half_written_part() {
    let fx = fixture();
    let (id, data, offsets) = record_part(&fx.store, &fx.wrapper, 40, 35, true);
    assert_eq!(data.len(), 4);
    let part = fx.store.part_path(&id);
    // 第 4 块只写了一半。
    truncate(&part, offsets[3] + 10);
    let reports = fx.store.recover_all(&fx.wrapper, &none(), NOW_US);
    assert_eq!(
        reports,
        [RecoverReport::Finished {
            id: id.clone(),
            kind: RecoverKind::Recovered,
        }]
    );
    assert!(!part.exists());
    assert!(fx
        .store
        .recover_all(&fx.wrapper, &none(), NOW_US)
        .is_empty());

    // 用新的 store 读，确认结果来自文件本身。
    let store = RecordingStore::open(fx.store.dir().to_path_buf(), fx.probe.clone()).unwrap();
    let list = store.list(&fx.wrapper).unwrap();
    assert_eq!(ids(&list), [id.as_str()]);
    let recording = &list[0];
    let valid = &data[..3];
    let frames: u64 = valid.iter().map(video_frames).sum();
    assert_eq!(frames, 30);
    assert_eq!(recording.state, StoredState::Recovered);
    assert_eq!(recording.meta, meta(40, true));
    assert!(recording.has_audio);
    assert_eq!(recording.bytes, len(&store.final_path(&id)));
    assert_eq!(
        recording.end,
        RecordingEnd {
            duration_us: (frames - 1) * FRAME_US,
            frames,
            keyframes: 3,
            audio_packets: frames,
            gaps: 0,
            dropped: 0,
            plain_bytes: valid.iter().map(|chunk| chunk.plaintext.len() as u64).sum(),
            ended_at_us: NOW_US,
            reason: "recovered".into(),
            width: 1280,
            height: 720,
            codec: 1,
        }
    );
    let reader = RecordingReader::open(&store.final_path(&id), &fx.wrapper).unwrap();
    assert_eq!(reader.chunks().len(), 3);
    assert_eq!(reader.final_chunk().t_start_us, (frames - 1) * FRAME_US);
}

#[test]
fn recover_all_handles_damaged_active_empty_and_unreadable_parts() {
    let fx = fixture();
    let (damaged, data, offsets) = record_part(&fx.store, &fx.wrapper, 50, 35, false);
    flip(
        &fx.store.part_path(&damaged),
        offsets[1] + CHUNK_HEADER_LEN as u64 + 4,
    );
    let (busy, _, _) = record_part(&fx.store, &fx.wrapper, 60, 25, false);
    let (header_only, _, _) = record_part(&fx.store, &fx.wrapper, 70, 0, false);
    let no_video = RecordingStore::new_id().unwrap();
    let mut plaintext = Vec::new();
    for body in [Body::Meta(meta(80, false)), Body::Config(config())] {
        Record { t_us: 0, body }.encode_into(&mut plaintext);
    }
    let chunk = ChunkOut {
        t_start_us: 0,
        keyframe_start: false,
        plaintext,
    };
    drop(write_chunks(
        &fx.store,
        &fx.wrapper,
        &no_video,
        80,
        &[chunk],
    ));
    let tiny = RecordingStore::new_id().unwrap();
    fs::write(fx.store.part_path(&tiny), [0u8; 39]).unwrap();
    let garbage = RecordingStore::new_id().unwrap();
    fs::write(fx.store.part_path(&garbage), [0xA5u8; 64]).unwrap();
    let (future, _, _) = record_part(&fx.store, &fx.wrapper, 90, 15, false);
    set_version(&fx.store.part_path(&future), 2);
    let untouched: Vec<(String, Vec<u8>)> = [&garbage, &future]
        .into_iter()
        .map(|id| (id.clone(), fs::read(fx.store.part_path(id)).unwrap()))
        .collect();

    let active = HashSet::from([busy.clone()]);
    let reports = fx.store.recover_all(&fx.wrapper, &active, NOW_US);
    assert_eq!(reports.len(), 6, "{reports:?}");
    for expected in [
        RecoverReport::Finished {
            id: damaged.clone(),
            kind: RecoverKind::Damaged,
        },
        RecoverReport::RemovedEmpty {
            id: header_only.clone(),
        },
        RecoverReport::RemovedEmpty {
            id: no_video.clone(),
        },
        RecoverReport::RemovedEmpty { id: tiny.clone() },
    ] {
        assert!(reports.contains(&expected), "{expected:?} in {reports:?}");
    }
    for (id, bytes) in &untouched {
        assert!(
            reports
                .iter()
                .any(|report| matches!(report, RecoverReport::Skipped { id: skipped, .. } if skipped == id)),
            "{id} skipped in {reports:?}"
        );
        assert_eq!(fs::read(fx.store.part_path(id)).unwrap(), *bytes);
        assert!(!fx.store.final_path(id).exists());
    }
    assert!(!reports.iter().any(|report| matches!(
        report,
        RecoverReport::Finished { id, .. }
            | RecoverReport::RemovedEmpty { id }
            | RecoverReport::Skipped { id, .. } if *id == busy
    )));
    assert!(fx.store.part_path(&busy).exists());
    assert!(!fx.store.final_path(&busy).exists());
    for id in [&header_only, &no_video, &tiny] {
        assert!(!fx.store.part_path(id).exists());
        assert!(!fx.store.final_path(id).exists());
    }

    assert!(!fx.store.part_path(&damaged).exists());
    let recording = fx.store.get(&damaged, &fx.wrapper).unwrap();
    assert_eq!(recording.state, StoredState::Damaged);
    assert_eq!(recording.end.reason, "damaged");
    assert_eq!(recording.end.frames, video_frames(&data[0]));
    assert_eq!(recording.end.frames, 10);
    assert_eq!(recording.end.ended_at_us, NOW_US);
    assert_eq!(recording.meta, meta(50, false));
    assert_eq!(ids(&fx.store.list(&fx.wrapper).unwrap()), [damaged]);
}

#[test]
fn recover_all_skips_parts_while_the_key_is_unavailable() {
    let fx = fixture();
    let (id, _, _) = record_part(&fx.store, &fx.wrapper, 40, 15, false);
    let part = fx.store.part_path(&id);
    let before = fs::read(&part).unwrap();
    fx.wrapper.inner.set_available(false);
    let reports = fx.store.recover_all(&fx.wrapper, &none(), NOW_US);
    assert!(
        matches!(reports.as_slice(), [RecoverReport::Skipped { id: skipped, .. }] if *skipped == id),
        "{reports:?}"
    );
    assert_eq!(fs::read(&part).unwrap(), before);
    fx.wrapper.inner.set_available(true);
    assert_eq!(
        fx.store.recover_all(&fx.wrapper, &none(), NOW_US),
        [RecoverReport::Finished {
            id: id.clone(),
            kind: RecoverKind::Recovered,
        }]
    );
    assert_eq!(fx.store.get(&id, &fx.wrapper).unwrap().end.frames, 15);
}

#[test]
fn retention_deletes_only_expired_finished_recordings() {
    let fx = fixture();
    let old = record(&fx.store, &fx.wrapper, NOW_US - 8 * DAY_US, 10, false);
    let recent = record(&fx.store, &fx.wrapper, NOW_US - 6 * DAY_US, 10, false);
    // started_at + 7 天正好等于 now：还没过期。
    let boundary = record(&fx.store, &fx.wrapper, NOW_US - 7 * DAY_US, 10, false);
    let old_active = record(&fx.store, &fx.wrapper, NOW_US - 8 * DAY_US, 10, false);
    let (old_part, _, _) = record_part(&fx.store, &fx.wrapper, NOW_US - 8 * DAY_US, 15, false);

    let forever = StoreConfig {
        retention_days: None,
        ..StoreConfig::default()
    };
    assert!(fx
        .store
        .apply_retention(&forever, &fx.wrapper, NOW_US, &none())
        .is_empty());
    assert_eq!(fx.store.list(&fx.wrapper).unwrap().len(), 4);

    let active = HashSet::from([old_active.clone()]);
    assert_eq!(
        fx.store
            .apply_retention(&week(), &fx.wrapper, NOW_US, &active),
        [old.as_str()]
    );
    assert!(!fx.store.final_path(&old).exists());
    for id in [&recent, &boundary, &old_active] {
        assert!(fx.store.final_path(id).exists());
    }
    assert!(fx.store.part_path(&old_part).exists());
    assert_eq!(
        ids(&fx.store.list(&fx.wrapper).unwrap()),
        [recent, boundary, old_active]
    );
    assert!(matches!(
        fx.store.get(&old, &fx.wrapper),
        Err(StoreError::NotFound)
    ));
}

#[test]
fn check_can_start_refuses_when_full_or_disk_low() {
    let fx = fixture();
    let cfg = StoreConfig::default();
    assert_eq!(
        cfg,
        StoreConfig {
            max_total_bytes: 4096 * MIB,
            retention_days: Some(30),
            min_free_to_start: GIB,
            min_free_while_recording: 500 * MIB,
            max_duration_us: 2 * 3600 * 1_000_000,
        }
    );
    assert_eq!(StoreConfig::from_settings(4096, Some(30)), cfg);
    assert_eq!(
        StoreConfig::from_settings(1024, None),
        StoreConfig {
            max_total_bytes: GIB,
            retention_days: None,
            ..StoreConfig::default()
        }
    );

    assert_eq!(fx.store.used_bytes(), 0);
    let id = record(&fx.store, &fx.wrapper, 1, 10, false);
    let (part, _, _) = record_part(&fx.store, &fx.wrapper, 2, 15, false);
    let used = fx.store.used_bytes();
    assert_eq!(
        used,
        len(&fx.store.final_path(&id)) + len(&fx.store.part_path(&part))
    );

    fx.probe.set(None);
    assert_eq!(fx.store.free_bytes(), None);
    assert_eq!(fx.store.check_can_start(&cfg), Ok(()));
    fx.probe.set(Some(900 * MIB));
    assert_eq!(fx.store.free_bytes(), Some(900 * MIB));
    assert_eq!(fx.store.check_can_start(&cfg), Err(StartRefusal::DiskLow));
    fx.probe.set(Some(GIB));
    assert_eq!(fx.store.check_can_start(&cfg), Ok(()));

    let full = StoreConfig {
        max_total_bytes: used,
        ..cfg.clone()
    };
    assert_eq!(
        fx.store.check_can_start(&full),
        Err(StartRefusal::StorageFull)
    );
    fx.probe.set(Some(900 * MIB));
    assert_eq!(
        fx.store.check_can_start(&full),
        Err(StartRefusal::StorageFull)
    );
    fx.probe.set(None);
    let roomy = StoreConfig {
        max_total_bytes: used + 1,
        ..cfg
    };
    assert_eq!(fx.store.check_can_start(&roomy), Ok(()));
}

#[test]
fn should_stop_checks_duration_cap_and_free_space() {
    let cfg = StoreConfig::default();
    let cap = cfg.max_total_bytes;
    let two_hours = 2 * 3600 * 1_000_000;
    let stop = RecordingStore::should_stop;
    assert_eq!(
        stop(&cfg, 0, 0, two_hours, None),
        Some(StopReason::MaxDuration)
    );
    assert_eq!(stop(&cfg, 0, 0, two_hours - 1, None), None);
    assert_eq!(
        stop(&cfg, cap - 10, 10, 0, None),
        Some(StopReason::StorageFull)
    );
    assert_eq!(stop(&cfg, cap - 10, 9, 0, None), None);
    assert_eq!(
        stop(&cfg, 0, 0, 0, Some(499 * MIB)),
        Some(StopReason::DiskLow)
    );
    assert_eq!(stop(&cfg, 0, 0, 0, Some(500 * MIB)), None);
    assert_eq!(stop(&cfg, 0, 0, 0, None), None);
    assert_eq!(
        stop(&cfg, cap, 1, two_hours, Some(0)),
        Some(StopReason::MaxDuration)
    );
    assert_eq!(
        stop(&cfg, cap, 1, 0, Some(0)),
        Some(StopReason::StorageFull)
    );
    assert_eq!(
        stop(&cfg, u64::MAX, u64::MAX, 0, None),
        Some(StopReason::StorageFull)
    );
}

#[test]
fn delete_checks_id_and_activity_and_clears_the_cache() {
    let fx = fixture();
    let id = record(&fx.store, &fx.wrapper, 5, 10, false);
    let other = record(&fx.store, &fx.wrapper, 3, 10, false);
    assert!(matches!(
        fx.store.delete(&RecordingStore::new_id().unwrap(), &none()),
        Err(StoreError::NotFound)
    ));
    assert!(matches!(
        fx.store.delete("../x", &none()),
        Err(StoreError::InvalidId)
    ));
    assert!(matches!(
        fx.store.delete(&id.to_uppercase(), &none()),
        Err(StoreError::InvalidId)
    ));
    assert!(matches!(
        fx.store.delete(&id, &HashSet::from([id.clone()])),
        Err(StoreError::Busy)
    ));
    let path = fx.store.final_path(&id);
    assert!(path.exists());
    pin_mtime(&path);
    assert_eq!(fx.store.list(&fx.wrapper).unwrap().len(), 2);
    assert_eq!(fx.wrapper.unwraps(), 2);

    let old_len = len(&path);
    fx.store.delete(&id, &none()).unwrap();
    assert!(!path.exists());
    assert!(matches!(
        fx.store.delete(&id, &none()),
        Err(StoreError::NotFound)
    ));
    // 同 id、同长度、同 mtime 的新文件：缓存已清掉，必须重新读出来。
    record_as(&fx.store, &fx.wrapper, &id, 6, 10, false);
    pin_mtime(&path);
    assert_eq!(len(&path), old_len);
    assert_eq!(fx.store.get(&id, &fx.wrapper).unwrap().meta, meta(6, false));
    assert_eq!(fx.wrapper.unwraps(), 3);
    fx.store.delete(&id, &none()).unwrap();
    assert_eq!(ids(&fx.store.list(&fx.wrapper).unwrap()), [other]);

    let (part, _, _) = record_part(&fx.store, &fx.wrapper, 7, 15, false);
    assert!(matches!(
        fx.store.delete(&part, &HashSet::from([part.clone()])),
        Err(StoreError::Busy)
    ));
    fx.store.delete(&part, &none()).unwrap();
    assert!(!fx.store.part_path(&part).exists());
}

#[test]
fn foreign_files_are_ignored() {
    let fx = fixture();
    let dir = fx.store.dir().to_path_buf();
    let id = record(&fx.store, &fx.wrapper, NOW_US - 100 * DAY_US, 10, false);
    fs::create_dir(dir.join("export")).unwrap();
    fs::write(dir.join("export").join("a.mp4"), vec![0u8; 4096]).unwrap();
    fs::write(dir.join("notes.txt"), b"notes").unwrap();
    fs::write(dir.join("old.rsrec"), vec![0u8; 512]).unwrap();
    let upper = RecordingStore::new_id().unwrap().to_uppercase();
    fs::write(dir.join(format!("{upper}.rsrec")), vec![0u8; 512]).unwrap();
    let dir_named_like_id = dir.join(format!("{}.rsrec", RecordingStore::new_id().unwrap()));
    fs::create_dir(&dir_named_like_id).unwrap();
    let foreign = [
        dir.join("export").join("a.mp4"),
        dir.join("notes.txt"),
        dir.join("old.rsrec"),
        dir.join(format!("{upper}.rsrec")),
        dir_named_like_id,
    ];

    assert_eq!(fx.store.used_bytes(), len(&fx.store.final_path(&id)));
    assert_eq!(ids(&fx.store.list(&fx.wrapper).unwrap()), [id.as_str()]);
    assert!(fx
        .store
        .recover_all(&fx.wrapper, &none(), NOW_US)
        .is_empty());
    assert_eq!(
        fx.store
            .apply_retention(&week(), &fx.wrapper, NOW_US, &none()),
        [id]
    );
    for path in &foreign {
        assert!(path.exists(), "{}", path.display());
    }
    assert_eq!(fx.store.used_bytes(), 0);
    assert!(fx.store.list(&fx.wrapper).unwrap().is_empty());
}

#[test]
fn unknown_version_is_kept_but_expired_damaged_files_are_deleted() {
    let fx = fixture();
    let old = NOW_US - 30 * DAY_US;
    let future = record(&fx.store, &fx.wrapper, old, 10, false);
    let future_path = fx.store.final_path(&future);
    set_version(&future_path, 2);
    let broken_old = record(&fx.store, &fx.wrapper, old, 10, false);
    corrupt_first_chunk(&fx.store.final_path(&broken_old), &fx.wrapper);
    let broken_recent = record(&fx.store, &fx.wrapper, NOW_US - DAY_US, 10, false);
    corrupt_first_chunk(&fx.store.final_path(&broken_recent), &fx.wrapper);

    let list = fx.store.list(&fx.wrapper).unwrap();
    assert_eq!(list.len(), 3);
    let unknown = find(&list, &future);
    assert_eq!(unknown.state, StoredState::Damaged);
    assert_eq!(unknown.end, damaged_end());
    // 文件头读不出，开始时间退回到 mtime。
    let mtime_us = modified(&future_path)
        .duration_since(UNIX_EPOCH)
        .unwrap()
        .as_micros() as u64;
    assert_eq!(unknown.meta.started_at_us, mtime_us);
    assert_eq!(
        find(&list, &broken_old).meta.started_at_us,
        old + CREATED_SKEW_US
    );
    assert_eq!(find(&list, &broken_old).state, StoredState::Damaged);

    assert_eq!(
        fx.store
            .apply_retention(&week(), &fx.wrapper, NOW_US, &none()),
        [broken_old.as_str()]
    );
    assert!(!fx.store.final_path(&broken_old).exists());
    assert!(fx.store.final_path(&broken_recent).exists());
    assert_eq!(
        fx.store
            .apply_retention(&week(), &fx.wrapper, u64::MAX, &none()),
        [broken_recent]
    );
    assert!(future_path.exists());
    assert_eq!(ids(&fx.store.list(&fx.wrapper).unwrap()), [future]);
}
