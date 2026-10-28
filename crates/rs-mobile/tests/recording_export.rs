//! ：导出 MP4。真实 Core；包裹实现和全局 Core 都是进程全局的：用例串行。
mod support;

use std::{
    fmt::Debug,
    fs,
    ops::Range,
    path::{Path, PathBuf},
    sync::{Arc, Mutex, MutexGuard},
};

use rs_mobile::rscore::{
    error::CoreError,
    recording::{
        api::{recordings_discard_export, recordings_export},
        dto::RecordingExport,
        export::{discard_export, export_mp4, hold_export_slot},
        ffi_wrap::set_wrapper,
    },
    runtime::{install_core, take_core, Core},
};
use rs_recording::{
    crypto::{testing::TestWrapper, SecretWrapper},
    file::{finalize_path, RecordingReader, RecordingWriter, CHUNK_HEADER_LEN},
    record::{AudioRec, Body, ConfigRec, Record, RecordingEnd, RecordingMeta, VideoRec},
    store::RecordingStore,
};
use serde_json::Value;

const FRAME_US: u64 = 33_333;
const WIDTH: u32 = 1280;
const HEIGHT: u32 = 720;
const EPOCH: u32 = 1;
const HEVC: u8 = 1;
const H264: u8 = 2;

static SERIAL: Mutex<()> = Mutex::new(());

// ---------- 合成的 HEVC 访问单元（写法同 rs-recording 的 mp4 测试）----------

fn escape(bytes: &[u8]) -> Vec<u8> {
    let mut output = Vec::with_capacity(bytes.len() + 4);
    let mut zeroes = 0;
    for &byte in bytes {
        if zeroes >= 2 && byte <= 3 {
            output.push(3);
            zeroes = 0;
        }
        output.push(byte);
        if byte == 0 {
            zeroes += 1;
        } else {
            zeroes = 0;
        }
    }
    output
}

fn hevc_nal(kind: u8, body: &[u8]) -> Vec<u8> {
    let mut nal = vec![kind << 1, 1];
    nal.extend_from_slice(body);
    nal
}

fn annexb(nals: &[&[u8]]) -> Vec<u8> {
    let mut output = Vec::new();
    for nal in nals {
        output.extend_from_slice(&[0, 0, 0, 1]);
        output.extend_from_slice(nal);
    }
    output
}

/// IDR 切片（类型 19）里带帧号的低 8 位。
fn idr(frame: u32) -> Vec<u8> {
    hevc_nal(19, &[frame as u8, 0x77])
}

fn slice(frame: u32) -> Vec<u8> {
    hevc_nal(1, &[frame as u8, 0x66])
}

/// 关键帧：VPS、SPS、PPS 加 IDR 切片。
fn key_au(frame: u32) -> Vec<u8> {
    let vps = hevc_nal(32, &[0, 0xaa]);
    let mut sps = vec![0x42, 0x01, 0x01];
    sps.extend_from_slice(&[
        0x01, 0x60, 0x00, 0x00, 0x00, 0x90, 0x00, 0x00, 0x00, 0x00, 0x00, 0x5d,
    ]);
    sps.extend_from_slice(&[0, 0, 0, 1, 2]);
    let sps = escape(&sps);
    let pps = hevc_nal(34, &[0, 0xcc]);
    annexb(&[&vps, &sps, &pps, &idr(frame)])
}

fn delta_au(frame: u32) -> Vec<u8> {
    annexb(&[&slice(frame)])
}

/// MP4 里的样本：4 字节长度加 NAL，参数集已经挪进 hvcC。
fn sample_of(nal: Vec<u8>) -> Vec<u8> {
    let mut sample = (nal.len() as u32).to_be_bytes().to_vec();
    sample.extend_from_slice(&nal);
    sample
}

// ---------- 录制记录 ----------

fn t(frame: u32) -> u64 {
    u64::from(frame) * FRAME_US
}

fn meta() -> Record {
    Record {
        t_us: 0,
        body: Body::Meta(RecordingMeta {
            host_id: "host-1".into(),
            host_name: "Mac".into(),
            role: "controller".into(),
            display_id: Some(1),
            started_at_us: 1,
            app_version: "test".into(),
            audio: true,
        }),
    }
}

fn config(t_us: u64, codec: u8) -> Record {
    Record {
        t_us,
        body: Body::Config(ConfigRec {
            epoch: EPOCH,
            display_id: 1,
            codec,
            width: WIDTH,
            height: HEIGHT,
            fps: 30,
            bitrate_kbps: 4000,
            cursor_x: 0.5,
            cursor_y: 0.5,
            cursor_visible: true,
        }),
    }
}

fn video(t_us: u64, frame_id: u32, epoch: u8, keyframe: bool, data: Vec<u8>) -> Record {
    Record {
        t_us,
        body: Body::Video(VideoRec {
            frame_id,
            epoch,
            display_id: 1,
            flags: 0,
            keyframe,
            capture_ts_us: t_us,
            capture_local_us: None,
            data,
        }),
    }
}

fn frame(n: u32, keyframe: bool) -> Record {
    let data = if keyframe { key_au(n) } else { delta_au(n) };
    video(t(n), n, EPOCH as u8, keyframe, data)
}

fn audio(t_us: u64) -> Record {
    Record {
        t_us,
        body: Body::Audio(AudioRec {
            seq: (t_us / 1000) as u16,
            capture_ts_us: t_us,
            payload: vec![0xfc; 40],
        }),
    }
}

fn cursor(t_us: u64) -> Record {
    Record {
        t_us,
        body: Body::CursorPos {
            x: 0.25,
            y: 0.75,
            visible: true,
        },
    }
}

/// 关键帧 first 和 29 个增量帧；每隔 10 帧插一条 Audio 和一条光标。
fn gop(first: u32) -> Vec<Record> {
    (first..first + 30)
        .flat_map(|n| {
            let mut records = vec![frame(n, n == first)];
            if n % 10 == 5 {
                records.extend([audio(t(n)), cursor(t(n))]);
            }
            records
        })
        .collect()
}

/// 第 0 块：Meta、Config、开头 2 个增量帧（0、1），然后是关键帧 2 和 29 个增量帧；
/// 第 1 块（关键帧块）：状态前导、关键帧 32、一个 epoch 对不上的帧（帧号 200）和 29 个增量帧。
fn standard() -> Vec<Vec<Record>> {
    let mut first = vec![
        meta(),
        config(0, HEVC),
        cursor(0),
        frame(0, false),
        audio(t(0)),
        frame(1, false),
    ];
    first.extend(gop(2));
    let mut second = vec![config(t(32), HEVC), cursor(t(32))];
    second.extend(gop(32));
    second.insert(
        3,
        video(
            t(32),
            200,
            (EPOCH as u8).wrapping_add(1),
            false,
            delta_au(200),
        ),
    );
    vec![first, second]
}

fn encode(records: &[Record]) -> Vec<u8> {
    let mut plaintext = Vec::new();
    for record in records {
        record.encode_into(&mut plaintext);
    }
    plaintext
}

// ---------- 导出结果里的 MP4 box ----------

fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_be_bytes(buf[offset..offset + 4].try_into().unwrap())
}

fn read_u64(buf: &[u8], offset: usize) -> u64 {
    u64::from_be_bytes(buf[offset..offset + 8].try_into().unwrap())
}

/// range 里依次排列的 box：(类型, 内容范围)。支持 64 位大小。
fn boxes(buf: &[u8], range: Range<usize>) -> Vec<([u8; 4], Range<usize>)> {
    let mut result = Vec::new();
    let mut cursor = range.start;
    while cursor < range.end {
        let size32 = read_u32(buf, cursor);
        let (size, header) = if size32 == 1 {
            (read_u64(buf, cursor + 8) as usize, 16)
        } else {
            (size32 as usize, 8)
        };
        assert!(
            size >= header && cursor + size <= range.end,
            "bad box at {cursor}"
        );
        let kind = buf[cursor + 4..cursor + 8].try_into().unwrap();
        result.push((kind, cursor + header..cursor + size));
        cursor += size;
    }
    result
}

fn child(buf: &[u8], range: Range<usize>, kind: &[u8; 4]) -> Range<usize> {
    let found: Vec<_> = boxes(buf, range)
        .into_iter()
        .filter(|(found, _)| found == kind)
        .collect();
    assert_eq!(found.len(), 1, "{}", String::from_utf8_lossy(kind));
    found[0].1.clone()
}

struct Mp4 {
    top: Vec<[u8; 4]>,
    handler: [u8; 4],
    /// 每个样本的字节，按 stsz 和 co64 取出。
    samples: Vec<Vec<u8>>,
    /// stss：1 起算的样本号。
    sync: Vec<u32>,
    mdat_len: usize,
}

impl Mp4 {
    fn read(path: &Path) -> Self {
        let buf = fs::read(path).unwrap();
        let top = boxes(&buf, 0..buf.len());
        let moov = child(&buf, 0..buf.len(), b"moov");
        let mdat = child(&buf, 0..buf.len(), b"mdat");
        let trak = child(&buf, moov, b"trak");
        let mdia = child(&buf, trak, b"mdia");
        let hdlr = child(&buf, mdia.clone(), b"hdlr");
        let minf = child(&buf, mdia, b"minf");
        let stbl = child(&buf, minf, b"stbl");

        let stsz = child(&buf, stbl.clone(), b"stsz").start;
        assert_eq!(read_u32(&buf, stsz + 4), 0, "stsz sample_size");
        let count = read_u32(&buf, stsz + 8) as usize;
        let co64 = child(&buf, stbl.clone(), b"co64").start;
        assert_eq!(read_u32(&buf, co64 + 4) as usize, count);
        let samples = (0..count)
            .map(|i| {
                let size = read_u32(&buf, stsz + 12 + 4 * i) as usize;
                let offset = read_u64(&buf, co64 + 8 + 8 * i) as usize;
                assert!(mdat.contains(&offset) && offset + size <= mdat.end);
                buf[offset..offset + size].to_vec()
            })
            .collect();
        let stss = child(&buf, stbl, b"stss").start;
        let sync = (0..read_u32(&buf, stss + 4) as usize)
            .map(|i| read_u32(&buf, stss + 8 + 4 * i))
            .collect();
        Self {
            top: top.into_iter().map(|(kind, _)| kind).collect(),
            handler: buf[hdlr.start + 8..hdlr.start + 12].try_into().unwrap(),
            samples,
            sync,
            mdat_len: mdat.len(),
        }
    }

    /// 样本里切片的帧号（低 8 位）。
    fn frames(&self) -> Vec<u8> {
        self.samples.iter().map(|sample| sample[6]).collect()
    }
}

fn frames(range: Range<u32>) -> Vec<u8> {
    range.map(|n| n as u8).collect()
}

// ---------- 断言 ----------

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

fn names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(dir)
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

// ---------- 夹具 ----------

struct Fixture {
    core: Arc<Core>,
    wrapper: Arc<TestWrapper>,
    _serial: MutexGuard<'static, ()>,
}

impl Fixture {
    fn new() -> Self {
        let serial = SERIAL.lock().unwrap_or_else(|error| error.into_inner());
        let core = support::test_core();
        let wrapper = Arc::new(TestWrapper::new(1));
        set_wrapper(Some(wrapper.clone() as Arc<dyn SecretWrapper>));
        Self {
            core,
            wrapper,
            _serial: serial,
        }
    }

    fn recordings(&self) -> PathBuf {
        self.core.data_dir().join("recordings")
    }

    fn export_dir(&self) -> PathBuf {
        self.recordings().join("export")
    }

    fn recording(&self, id: &str) -> PathBuf {
        self.recordings().join(format!("{id}.rsrec"))
    }

    /// 每个 Vec 写成一块；第一帧是关键帧的块标 KEYFRAME_START。finish = false 时只留下 `.part`。
    fn write_with(&self, chunks: &[Vec<Record>], finish: bool) -> String {
        let dir = self.recordings();
        fs::create_dir_all(&dir).unwrap();
        let id = RecordingStore::new_id().unwrap();
        let part = dir.join(format!("{id}.rsrec.part"));
        let mut writer = RecordingWriter::create(&part, self.wrapper.as_ref(), [7; 16], 1).unwrap();
        let mut last_t = 0;
        for records in chunks {
            let keyframe_start = records.iter().find_map(|record| match &record.body {
                Body::Video(video) => Some(video.keyframe),
                _ => None,
            }) == Some(true);
            writer
                .append(records[0].t_us, keyframe_start, &encode(records))
                .unwrap();
            last_t = records.iter().map(|record| record.t_us).max().unwrap();
        }
        if finish {
            let end = Record {
                t_us: last_t,
                body: Body::End(RecordingEnd {
                    duration_us: last_t,
                    reason: "user".into(),
                    ..RecordingEnd::default()
                }),
            };
            writer.finish(end.t_us, &encode(&[end])).unwrap();
            finalize_path(&part).unwrap();
        }
        id
    }

    fn write(&self, chunks: &[Vec<Record>]) -> String {
        self.write_with(chunks, true)
    }

    /// 改坏这几块的密文：文件结构不变，块的 GCM 校验失败。
    fn corrupt(&self, id: &str, chunks: &[usize]) {
        let path = self.recording(id);
        let infos = RecordingReader::open(&path, self.wrapper.as_ref())
            .unwrap()
            .chunks()
            .to_vec();
        let mut bytes = fs::read(&path).unwrap();
        for &chunk in chunks {
            bytes[infos[chunk].offset as usize + CHUNK_HEADER_LEN + 3] ^= 0xff;
        }
        fs::write(&path, bytes).unwrap();
    }

    fn export(&self, id: &str) -> Result<RecordingExport, CoreError> {
        self.core.block_on(export_mp4(&self.core, id))
    }
}

impl Drop for Fixture {
    fn drop(&mut self) {
        set_wrapper(None);
        take_core();
    }
}

// ---------- 用例 ----------

#[test]
fn export_writes_the_video_track_into_the_export_dir() {
    let fixture = Fixture::new();
    let id = fixture.write(&standard());

    let export = fixture.export(&id).unwrap();
    let path = Path::new(&export.path);
    assert!(path.is_absolute(), "{}", export.path);
    assert_eq!(
        fs::canonicalize(path.parent().unwrap()).unwrap(),
        fs::canonicalize(fixture.export_dir()).unwrap()
    );
    assert_eq!(
        path.file_name().unwrap().to_str().unwrap(),
        format!("recording-{}.mp4", &id[..8])
    );
    assert_eq!(export.mime, "video/mp4");
    assert_eq!(export.bytes, fs::metadata(path).unwrap().len());
    assert_eq!(
        names(&fixture.export_dir()),
        [format!("recording-{}.mp4", &id[..8])]
    );
    assert!(fixture.recording(&id).is_file(), "the recording is kept");

    let mp4 = Mp4::read(path);
    assert_eq!(mp4.top, [*b"ftyp", *b"mdat", *b"moov"]);
    assert_eq!(&mp4.handler, b"vide");
    // 开头 2 个增量帧和 epoch 对不上的帧被跳过。
    assert_eq!(mp4.samples.len(), 60);
    assert_eq!(mp4.sync, [1, 31]);
    assert_eq!(mp4.frames(), [frames(2..32), frames(32..62)].concat());
    assert_eq!(mp4.samples[0], sample_of(idr(2)));
    assert_eq!(mp4.samples[1], sample_of(slice(3)));
    assert_eq!(mp4.samples[30], sample_of(idr(32)));
    let total: usize = mp4.samples.iter().map(Vec::len).sum();
    assert_eq!(mp4.mdat_len, total, "mdat holds exactly the samples");
}

#[test]
fn export_stops_at_a_damaged_chunk_or_a_codec_change() {
    let fixture = Fixture::new();
    let id = fixture.write(&standard());
    fixture.corrupt(&id, &[1]);
    let export = fixture.export(&id).unwrap();
    let mp4 = Mp4::read(Path::new(&export.path));
    assert_eq!(mp4.frames(), frames(2..32));
    assert_eq!(mp4.sync, [1]);

    // 第 1 块的配置换成 H.264：从那里停止。
    let mut chunks = standard();
    chunks[1][0] = config(t(32), H264);
    let id = fixture.write(&chunks);
    let export = fixture.export(&id).unwrap();
    assert_eq!(Mp4::read(Path::new(&export.path)).frames(), frames(2..32));
}

#[test]
fn export_without_any_sample_is_damaged_and_leaves_nothing() {
    let fixture = Fixture::new();
    let all_bad = fixture.write(&standard());
    fixture.corrupt(&all_bad, &[0, 1]);
    assert_http(fixture.export(&all_bad), 422, "recording.damaged");
    assert!(names(&fixture.export_dir()).is_empty());

    let mut deltas = vec![meta(), config(0, HEVC)];
    deltas.extend((0..30).map(|n| frame(n, false)));
    let no_keyframe = fixture.write(&[deltas]);
    assert_http(fixture.export(&no_keyframe), 422, "recording.damaged");
    assert!(names(&fixture.export_dir()).is_empty());

    // 出错返回时槽位也放出来了。
    assert!(hold_export_slot(&fixture.core).is_some());
}

#[test]
fn export_is_busy_while_the_slot_is_held() {
    let fixture = Fixture::new();
    let id = fixture.write(&standard());
    let slot = hold_export_slot(&fixture.core).expect("free slot");
    assert!(hold_export_slot(&fixture.core).is_none());
    assert_http(fixture.export(&id), 429, "recording.busy");
    drop(slot);
    let export = fixture.export(&id).unwrap();
    assert_eq!(Mp4::read(Path::new(&export.path)).samples.len(), 60);
    assert!(
        hold_export_slot(&fixture.core).is_some(),
        "released after success"
    );
}

#[test]
fn export_clears_stale_exports_but_keeps_other_files() {
    let fixture = Fixture::new();
    let id = fixture.write(&standard());
    let dir = fixture.export_dir();
    fs::create_dir_all(&dir).unwrap();
    for name in ["recording-old.mp4", "x.tmp", "notes.txt"] {
        fs::write(dir.join(name), b"stale").unwrap();
    }
    fixture.export(&id).unwrap();
    assert_eq!(
        names(&dir),
        [
            "notes.txt".to_owned(),
            format!("recording-{}.mp4", &id[..8])
        ]
    );
}

#[test]
fn discard_export_only_deletes_files_in_the_export_dir() {
    let fixture = Fixture::new();
    let core = &fixture.core;
    let id = fixture.write(&standard());
    let recording = fixture.recording(&id);
    let export = fixture.export(&id).unwrap();

    discard_export(core, &export.path).unwrap();
    assert!(!Path::new(&export.path).exists());
    discard_export(core, &export.path).unwrap();

    discard_export(core, recording.to_str().unwrap()).unwrap();
    assert!(recording.is_file());

    let hosts = Path::new("/etc/hosts");
    let hosts_existed = hosts.exists();
    discard_export(core, "/etc/hosts").unwrap();
    assert_eq!(hosts.exists(), hosts_existed);

    let dotdot = format!("{}/../{id}.rsrec", fixture.export_dir().display());
    discard_export(core, &dotdot).unwrap();
    assert!(recording.is_file());

    // 名字像导出文件，但不在导出目录里。
    let outside = fixture.recordings().join("recording-outside.mp4");
    fs::write(&outside, b"keep").unwrap();
    discard_export(core, outside.to_str().unwrap()).unwrap();
    assert!(outside.is_file());

    // 导出目录里的符号链接指向录制文件：按链接目标判断，录制文件不删。
    #[cfg(unix)]
    {
        let link = fixture.export_dir().join("recording-link.mp4");
        std::os::unix::fs::symlink(&recording, &link).unwrap();
        discard_export(core, link.to_str().unwrap()).unwrap();
        assert!(recording.is_file());
    }

    // 导出目录里不叫 recording-*.mp4 的文件也不删。
    let notes = fixture.export_dir().join("notes.mp4");
    fs::write(&notes, b"keep").unwrap();
    discard_export(core, notes.to_str().unwrap()).unwrap();
    assert!(notes.is_file());
}

#[test]
fn export_rejects_missing_recording_and_unavailable_wrapper() {
    let fixture = Fixture::new();
    let id = fixture.write(&standard());

    assert_http(fixture.export(&"0".repeat(32)), 404, "recording.notFound");
    assert_http(fixture.export("../rs.db"), 404, "recording.notFound");
    assert_http(
        fixture.export(&id.to_uppercase()),
        404,
        "recording.notFound",
    );
    // 正在录的只有 .part。
    let part = fixture.write_with(&standard(), false);
    assert!(fixture
        .recordings()
        .join(format!("{part}.rsrec.part"))
        .is_file());
    assert_http(fixture.export(&part), 404, "recording.notFound");

    fixture.wrapper.set_available(false);
    match fixture.export(&id) {
        Err(CoreError::Internal { message }) => assert_eq!(message, "recording.keyUnavailable"),
        other => panic!("expected recording.keyUnavailable, got {other:?}"),
    }
    fixture.wrapper.set_available(true);
    assert!(names(&fixture.export_dir()).is_empty());

    set_wrapper(None);
    match fixture.export(&id) {
        Err(CoreError::Unsupported { message }) => assert_eq!(message, "recording.unsupported"),
        other => panic!("expected recording.unsupported, got {other:?}"),
    }
}

#[test]
fn uniffi_export_and_discard_go_through_export_rs() {
    let fixture = Fixture::new();
    install_core(fixture.core.clone());
    let id = fixture.write(&standard());

    let json = fixture
        .core
        .block_on(recordings_export(id.clone()))
        .unwrap();
    let value: Value = serde_json::from_str(&json).unwrap();
    let mut keys: Vec<_> = value.as_object().unwrap().keys().cloned().collect();
    keys.sort();
    assert_eq!(keys, ["bytes", "mime", "path"]);
    assert_eq!(value["mime"], "video/mp4");
    let path = value["path"].as_str().unwrap().to_owned();
    assert_eq!(value["bytes"], fs::metadata(&path).unwrap().len());

    recordings_discard_export(path.clone()).unwrap();
    assert!(!Path::new(&path).exists());
    assert!(fixture.recording(&id).is_file());
}
