//! 录制与回放的 JSON 结构（07「会话录制」）。字段名 camelCase，与 app/src/native/rscore/types.ts 一一对应。
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingRunState {
    Recording,
    Stopped,
    Failed,
}

/// CoreEvent `recording.state`：录制中每秒一次，结束时一次（stopped / failed）。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStateEvent {
    pub session_id: String,
    pub recording_id: String,
    pub state: RecordingRunState,
    pub bytes: u64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub reason: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum PlaybackRunState {
    Loading,
    Playing,
    Paused,
    Ended,
    Error,
}

/// CoreEvent `playback.state`：播放中每 250 ms 一次，状态变化时立即一次。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackStateEvent {
    pub playback_id: String,
    pub recording_id: String,
    pub state: PlaybackRunState,
    pub position_ms: u64,
    pub duration_ms: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub error: Option<String>,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingFileState {
    Complete,
    Recovered,
    Recording,
    Damaged,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingInfo {
    pub recording_id: String,
    pub host_id: String,
    pub host_name: String,
    /// unix 微秒，和 SessionSummary.started_at 同单位。
    pub started_at: u64,
    pub duration_ms: u64,
    pub bytes: u64,
    pub state: RecordingFileState,
    pub has_audio: bool,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStorage {
    pub used_bytes: u64,
    pub cap_bytes: u64,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub free_bytes: Option<u64>,
    /// None = 永久保留。
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub retention_days: Option<u32>,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingList {
    pub recordings: Vec<RecordingInfo>,
    pub storage: RecordingStorage,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum RecordingStatusState {
    Idle,
    Recording,
    Stopping,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingStatus {
    pub session_id: String,
    #[serde(default, skip_serializing_if = "Option::is_none")]
    pub recording_id: Option<String>,
    pub state: RecordingStatusState,
    pub bytes: u64,
    pub duration_ms: u64,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct PlaybackInfo {
    pub playback_id: String,
    pub recording_id: String,
    pub duration_ms: u64,
    pub width: u32,
    pub height: u32,
    pub has_audio: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(tag = "op", rename_all = "camelCase")]
pub enum PlaybackCommand {
    Play,
    Pause,
    Seek {
        #[serde(rename = "positionMs")]
        position_ms: u64,
    },
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct RecordingExport {
    pub path: String,
    pub bytes: u64,
    pub mime: String,
}
