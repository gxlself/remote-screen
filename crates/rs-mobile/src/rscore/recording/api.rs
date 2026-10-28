//! 会话录制的 uniffi 接口（07「会话录制」）。JSON 结构见 recording::dto。
use std::{
    fmt::Display,
    sync::{Arc, Mutex},
};

use rs_recording::{
    crypto::WrapError,
    file::FileError,
    store::{StoreError, StoredRecording, StoredState},
};

use super::{
    dto::{PlaybackCommand, RecordingFileState, RecordingInfo, RecordingList, RecordingStorage},
    export, ffi_wrap, playback,
    recorder::{self, ActiveRecording},
};
use crate::rscore::{
    api_sessions::session_entry,
    error::CoreError,
    runtime::{core, Core},
    tables::SessionEntry,
};

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

fn io_error(error: impl Display) -> CoreError {
    CoreError::internal(format!("recording.io: {error}"))
}

fn map_store(error: StoreError) -> CoreError {
    match error {
        StoreError::NotFound | StoreError::InvalidId => not_found(),
        StoreError::Busy => http(409, "recording.alreadyRecording"),
        StoreError::KeyUnavailable | StoreError::File(FileError::Wrap(WrapError::Unavailable)) => {
            CoreError::internal("recording.keyUnavailable")
        }
        StoreError::Io(error) | StoreError::File(FileError::Io(error)) => io_error(error),
        StoreError::Random | StoreError::File(FileError::Random) => {
            io_error("random source failed")
        }
        StoreError::File(
            FileError::Corrupt(_)
            | FileError::Auth { .. }
            | FileError::BadMagic
            | FileError::UnsupportedVersion(_)
            | FileError::TooLarge
            | FileError::Wrap(WrapError::Rejected),
        ) => http(422, "recording.damaged"),
    }
}

fn session(core: &Arc<Core>, session_id: u64) -> Result<Arc<SessionEntry>, CoreError> {
    session_entry(core, session_id).map_err(|_| not_found())
}

/// 恢复 `.part`、开始录制和删除互斥：否则列表可能把刚建好、还没登记为正在录的 `.part` 当成崩溃残留去恢复。
#[derive(Default)]
struct StoreGate(Mutex<()>);

impl StoreGate {
    fn hold<T>(core: &Core, f: impl FnOnce() -> T) -> T {
        let gate = core.state::<StoreGate>();
        let _held = gate.0.lock().unwrap_or_else(|error| error.into_inner());
        f()
    }
}

/// 目录扫描、解密和钥匙串都可能阻塞：放到 Core 运行时的阻塞线程上。
async fn blocking<T, F>(c: Arc<Core>, f: F) -> Result<T, CoreError>
where
    T: Send + 'static,
    F: FnOnce(&Arc<Core>) -> Result<T, CoreError> + Send + 'static,
{
    c.clone()
        .run(async move {
            tokio::task::spawn_blocking(move || f(&c))
                .await
                .map_err(CoreError::internal)?
        })
        .await
}

fn stored_info(recording: StoredRecording) -> RecordingInfo {
    RecordingInfo {
        recording_id: recording.id,
        host_id: recording.meta.host_id,
        host_name: recording.meta.host_name,
        started_at: recording.meta.started_at_us,
        duration_ms: recording.end.duration_us / 1000,
        bytes: recording.bytes,
        state: match recording.state {
            StoredState::Complete => RecordingFileState::Complete,
            StoredState::Recovered => RecordingFileState::Recovered,
            StoredState::Damaged => RecordingFileState::Damaged,
        },
        has_audio: recording.has_audio,
        width: recording.end.width,
        height: recording.end.height,
    }
}

fn active_info(active: ActiveRecording) -> RecordingInfo {
    RecordingInfo {
        recording_id: active.recording_id,
        host_id: active.meta.host_id,
        host_name: active.meta.host_name,
        started_at: active.meta.started_at_us,
        duration_ms: active.duration_ms,
        bytes: active.bytes,
        state: RecordingFileState::Recording,
        has_audio: active.meta.audio,
        width: active.width,
        height: active.height,
    }
}

fn list(core: &Arc<Core>) -> Result<RecordingList, CoreError> {
    let wrapper =
        ffi_wrap::wrapper().ok_or_else(|| CoreError::unsupported("recording.unsupported"))?;
    let cfg = recorder::store_config(core);
    let store = recorder::store(core)?;
    let stored = StoreGate::hold(core, || {
        let active = recorder::active_ids(core);
        let now = core.now_us();
        store.recover_all(wrapper.as_ref(), &active, now);
        store.apply_retention(&cfg, wrapper.as_ref(), now, &active);
        store.list(wrapper.as_ref())
    })
    .map_err(map_store)?;
    // 收尾时先改名、后注销登记：两边都有的以正在录的为准。
    let active = recorder::active_recordings(core);
    let stored: Vec<_> = stored
        .into_iter()
        .filter(|recording| !active.iter().any(|live| live.recording_id == recording.id))
        .map(stored_info)
        .collect();
    let mut recordings: Vec<_> = active.into_iter().map(active_info).collect();
    recordings.extend(stored);
    Ok(RecordingList {
        recordings,
        storage: RecordingStorage {
            used_bytes: store.used_bytes(),
            cap_bytes: cfg.max_total_bytes,
            free_bytes: store.free_bytes(),
            retention_days: cfg.retention_days,
        },
    })
}

/// 开始录制；返回 RecordingStatus JSON。
#[uniffi::export(async_runtime = "tokio")]
pub async fn recording_start(session_id: u64) -> Result<String, CoreError> {
    let c = core()?;
    let entry = session(&c, session_id)?;
    let status = blocking(c, move |core| {
        StoreGate::hold(core, || recorder::start_recording(core, &entry))
    })
    .await?;
    Ok(serde_json::to_string(&status)?)
}

/// 请求停止；收尾在写盘线程完成，结果以 `recording.state` 事件给出。
#[uniffi::export]
pub fn recording_stop(session_id: u64) -> Result<(), CoreError> {
    let entry = session(&core()?, session_id)?;
    recorder::stop_recording(&entry)
}

/// RecordingStatus JSON；会话不存在返回 Http 404 `recording.notFound`。
#[uniffi::export]
pub fn recording_status(session_id: u64) -> Result<String, CoreError> {
    let entry = session(&core()?, session_id)?;
    let status = recorder::status(&entry);
    Ok(serde_json::to_string(&status)?)
}

/// RecordingList JSON（含恢复 .part 和保留期清理）。
#[uniffi::export(async_runtime = "tokio")]
pub async fn recordings_list() -> Result<String, CoreError> {
    let list = blocking(core()?, list).await?;
    Ok(serde_json::to_string(&list)?)
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn recordings_delete(recording_id: String) -> Result<(), CoreError> {
    blocking(core()?, move |core| {
        let store = recorder::store(core)?;
        StoreGate::hold(core, || {
            store.delete(&recording_id, &recorder::active_ids(core))
        })
        .map_err(map_store)
    })
    .await
}

/// 打开回放；返回 PlaybackInfo JSON。之后原生用 playbackId 调 rs_session_attach、RsAudio.start。
#[uniffi::export(async_runtime = "tokio")]
pub async fn playback_open(recording_id: String) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move {
            let info = playback::open_playback(&c, &recording_id).await?;
            Ok(serde_json::to_string(&info)?)
        })
        .await
}

/// command_json：PlaybackCommand。
#[uniffi::export]
pub fn playback_control(playback_id: u64, command_json: String) -> Result<(), CoreError> {
    let c = core()?;
    let command: PlaybackCommand = serde_json::from_str(&command_json)
        .map_err(|error| CoreError::internal(format!("recording.badCommand: {error}")))?;
    playback::control(&c, playback_id, command)
}

#[uniffi::export]
pub fn playback_close(playback_id: u64) -> Result<(), CoreError> {
    playback::close(&core()?, playback_id)
}

/// 解密并封装成临时 MP4；返回 RecordingExport JSON。
#[uniffi::export(async_runtime = "tokio")]
pub async fn recordings_export(recording_id: String) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move {
            let export = export::export_mp4(&c, &recording_id).await?;
            Ok(serde_json::to_string(&export)?)
        })
        .await
}

/// 删除 recordings_export 产生的临时文件；路径不在导出目录里则忽略。
#[uniffi::export]
pub fn recordings_discard_export(path: String) -> Result<(), CoreError> {
    export::discard_export(&core()?, &path)
}
