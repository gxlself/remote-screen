//! 导出 MP4（M3 V5，07「会话录制」）：逐块解密一份录制，只把视频封装成 `<dataDir>/recordings/export/`
//! 里的临时文件，交给系统保存后由 discard_export 删除。同一时间只做一个导出；第一版不含声音。
use std::{
    fmt::Display,
    fs::{self, File},
    io,
    path::{Path, PathBuf},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc,
    },
};

use rs_recording::{
    crypto::{SecretWrapper, WrapError},
    file::{FileError, RecordingReader},
    mp4::{Mp4Codec, Mp4Error, Mp4Writer},
    record::{self, Body, Record},
    store::{RecordingStore, MIB},
};

use super::{dto::RecordingExport, ffi_wrap, recorder};
use crate::rscore::{error::CoreError, runtime::Core};

const NAME_PREFIX: &str = "recording-";
const NAME_SUFFIX: &str = ".mp4";
const TMP_SUFFIX: &str = ".tmp";
const MIME: &str = "video/mp4";
/// 可用空间至少要比录制文件多这么多才开始导出。
const SPARE_BYTES: u64 = 200 * MIB;

fn http(status: u16, code: &str) -> CoreError {
    CoreError::Http {
        status,
        hub_code: None,
        message: code.to_owned(),
    }
}

fn not_found() -> CoreError {
    http(404, "recording.notFound")
}

fn damaged() -> CoreError {
    http(422, "recording.damaged")
}

fn io_error(error: impl Display) -> CoreError {
    CoreError::internal(format!("recording.io: {error}"))
}

fn open_error(error: FileError) -> CoreError {
    match error {
        FileError::Wrap(WrapError::Unavailable) => CoreError::internal("recording.keyUnavailable"),
        FileError::Io(error) if error.kind() == io::ErrorKind::NotFound => not_found(),
        error => {
            tracing::warn!(%error, "recording export cannot open the recording");
            damaged()
        }
    }
}

fn mp4_error(error: Mp4Error) -> CoreError {
    match error {
        Mp4Error::Io(error) => io_error(error),
        _ => damaged(),
    }
}

/// 和 `recorder::store()` 同一个 recordings 目录下的 `export/`。
fn export_dir(core: &Core) -> PathBuf {
    core.data_dir().join("recordings").join("export")
}

fn is_export_name(name: &str) -> bool {
    name.strip_prefix(NAME_PREFIX)
        .is_some_and(|rest| rest.ends_with(NAME_SUFFIX))
}

#[derive(Default)]
struct ExportState {
    busy: AtomicBool,
}

/// 导出槽位；Drop 时释放。
pub struct ExportSlot(Arc<ExportState>);

impl Drop for ExportSlot {
    fn drop(&mut self) {
        self.0.busy.store(false, Ordering::Release);
    }
}

/// 测试用：占住导出槽位，用来测 busy。已经有导出在进行时返回 None。
#[doc(hidden)]
pub fn hold_export_slot(core: &Core) -> Option<ExportSlot> {
    let state = core.state::<ExportState>();
    state
        .busy
        .compare_exchange(false, true, Ordering::Acquire, Ordering::Relaxed)
        .ok()?;
    Some(ExportSlot(state))
}

/// 不存在就创建；已存在就删掉上一次没清理的 `recording-*.mp4` 和 `*.tmp`，别的文件不动。
fn prepare_dir(dir: &Path) -> io::Result<()> {
    fs::create_dir_all(dir)?;
    for entry in fs::read_dir(dir)? {
        let entry = entry?;
        let leftover = entry
            .file_name()
            .to_str()
            .is_some_and(|name| is_export_name(name) || name.ends_with(TMP_SUFFIX));
        if !leftover || entry.file_type()?.is_dir() {
            continue;
        }
        match fs::remove_file(entry.path()) {
            Ok(()) => {}
            Err(error) if error.kind() == io::ErrorKind::NotFound => {}
            Err(error) => {
                tracing::warn!(
                    path = %entry.path().display(),
                    %error,
                    "stale export was not deleted"
                );
            }
        }
    }
    Ok(())
}

enum Flow {
    Continue,
    /// 编码变了，或者第一条 Config 的编码不认识：后面的部分不再导出。
    Stop,
}

/// 按回放的规则把记录筛成视频样本：第一个关键帧之前的帧、epoch 低 8 位和当前配置对不上的帧都跳过。
/// Mp4Writer 在第一条 Config 时才建，编码由它决定。
struct VideoTrack {
    file: Option<File>,
    writer: Option<Mp4Writer<File>>,
    codec: u8,
    epoch: Option<u8>,
    width: u32,
    height: u32,
    need_keyframe: bool,
}

impl VideoTrack {
    fn new(file: File) -> Self {
        Self {
            file: Some(file),
            writer: None,
            codec: 0,
            epoch: None,
            width: 0,
            height: 0,
            need_keyframe: true,
        }
    }

    fn push(&mut self, record: Record) -> Result<Flow, Mp4Error> {
        match record.body {
            Body::Config(config) => {
                if let Some(writer) = &mut self.writer {
                    if config.codec != self.codec {
                        return Ok(Flow::Stop);
                    }
                    if (config.width, config.height) != (self.width, self.height) {
                        writer.set_dimensions(config.width, config.height);
                    }
                } else {
                    let (Some(codec), Some(file)) =
                        (Mp4Codec::from_proto(config.codec), self.file.take())
                    else {
                        return Ok(Flow::Stop);
                    };
                    self.writer = Some(Mp4Writer::new(file, codec, config.width, config.height)?);
                    self.codec = config.codec;
                }
                self.width = config.width;
                self.height = config.height;
                self.epoch = Some(config.epoch as u8);
            }
            Body::Video(video) => {
                let Some(writer) = &mut self.writer else {
                    return Ok(Flow::Continue);
                };
                if self.epoch != Some(video.epoch) || (self.need_keyframe && !video.keyframe) {
                    return Ok(Flow::Continue);
                }
                match writer.push_sample(record.t_us, video.keyframe, &video.data) {
                    Ok(()) if video.keyframe => self.need_keyframe = false,
                    Ok(()) => {}
                    Err(Mp4Error::Io(error)) => return Err(Mp4Error::Io(error)),
                    // 这一帧封装不了（比如关键帧缺参数集）：等下一个关键帧再接着导出。
                    Err(error) => {
                        tracing::warn!(
                            frame = video.frame_id,
                            %error,
                            "recording export skipped a frame"
                        );
                        self.need_keyframe = true;
                    }
                }
            }
            _ => {}
        }
        Ok(Flow::Continue)
    }

    /// 写完 moov。一个样本都没写进去 → damaged。
    fn finish(self) -> Result<(), CoreError> {
        match self.writer {
            Some(writer) if writer.sample_count() > 0 => {
                writer.finish().map_err(mp4_error)?;
                Ok(())
            }
            _ => Err(damaged()),
        }
    }
}

/// 从第 0 块起逐块解密；遇到校验失败或解不开的块就停在那里，前面写进去的照常导出。
fn write_mp4(reader: &mut RecordingReader, tmp: &Path) -> Result<(), CoreError> {
    let mut track = VideoTrack::new(File::create(tmp).map_err(io_error)?);
    'chunks: for index in 0..reader.chunks().len() {
        let records = match reader.read_chunk(index) {
            Ok(plaintext) => record::decode(&plaintext).map_err(|error| error.to_string()),
            Err(FileError::Io(error)) => return Err(io_error(error)),
            Err(error) => Err(error.to_string()),
        };
        let records = match records {
            Ok(records) => records,
            Err(error) => {
                tracing::warn!(chunk = index, %error, "recording export stops at a damaged chunk");
                break;
            }
        };
        for record in records {
            if let Flow::Stop = track.push(record).map_err(mp4_error)? {
                break 'chunks;
            }
        }
    }
    track.finish()
}

fn export_blocking(
    store: &RecordingStore,
    source: &Path,
    dir: &Path,
    name: &str,
    wrapper: &dyn SecretWrapper,
) -> Result<RecordingExport, CoreError> {
    prepare_dir(dir).map_err(io_error)?;
    let source_bytes = fs::metadata(source).map_err(io_error)?.len();
    if store
        .free_bytes()
        .is_some_and(|free| free < source_bytes.saturating_add(SPARE_BYTES))
    {
        return Err(http(507, "recording.diskLow"));
    }
    let mut reader = RecordingReader::open(source, wrapper).map_err(open_error)?;
    let tmp = dir.join(format!("{name}{TMP_SUFFIX}"));
    let path = dir.join(name);
    let written = write_mp4(&mut reader, &tmp).and_then(|()| {
        fs::rename(&tmp, &path).map_err(io_error)?;
        fs::metadata(&path).map_err(io_error)
    });
    let metadata = match written {
        Ok(metadata) => metadata,
        Err(error) => {
            let _ = fs::remove_file(&tmp);
            return Err(error);
        }
    };
    Ok(RecordingExport {
        path: path.to_string_lossy().into_owned(),
        bytes: metadata.len(),
        mime: MIME.to_owned(),
    })
}

/// 解密并封装成 `<dataDir>/recordings/export/recording-<id 前 8 位>.mp4`，返回它的绝对路径。
pub async fn export_mp4(
    core: &Arc<Core>,
    recording_id: &str,
) -> Result<RecordingExport, CoreError> {
    let wrapper =
        ffi_wrap::wrapper().ok_or_else(|| CoreError::unsupported("recording.unsupported"))?;
    if !RecordingStore::valid_id(recording_id) {
        return Err(not_found());
    }
    let store = recorder::store(core)?;
    // 只导出已完成的 .rsrec：正在录的只有 .part。
    let source = store.final_path(recording_id);
    if !fs::symlink_metadata(&source).is_ok_and(|metadata| metadata.is_file()) {
        return Err(not_found());
    }
    let slot = hold_export_slot(core).ok_or_else(|| http(429, "recording.busy"))?;
    let dir = std::path::absolute(export_dir(core)).map_err(io_error)?;
    let name = format!("{NAME_PREFIX}{}{NAME_SUFFIX}", &recording_id[..8]);
    core.handle()
        .spawn_blocking(move || {
            // 调用方不再等待时，这次导出也要做完才放出槽位。
            let _slot = slot;
            export_blocking(&store, &source, &dir, &name, wrapper.as_ref())
        })
        .await
        .map_err(CoreError::internal)?
}

/// 删除 export_mp4 产生的临时文件；路径不在导出目录里则忽略。
pub fn discard_export(core: &Arc<Core>, path: &str) -> Result<(), CoreError> {
    // 解开符号链接和 `..` 之后，父目录必须正好是导出目录，文件名必须是 recording-*.mp4。
    let (Ok(dir), Ok(file)) = (fs::canonicalize(export_dir(core)), fs::canonicalize(path)) else {
        return Ok(());
    };
    let named = file
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(is_export_name);
    if !named || file.parent() != Some(dir.as_path()) || !file.is_file() {
        return Ok(());
    }
    match fs::remove_file(&file) {
        Ok(()) => Ok(()),
        Err(error) if error.kind() == io::ErrorKind::NotFound => Ok(()),
        Err(error) => Err(io_error(error)),
    }
}
