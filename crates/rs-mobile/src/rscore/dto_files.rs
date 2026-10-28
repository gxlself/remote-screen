//! File browser and transfer JSON contracts (07; C6.1).
use super::dto_core::NetPath;
use serde::{Deserialize, Serialize};

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(tag = "kind", rename_all = "camelCase")]
pub enum FileSourceRef {
    #[serde(rename_all = "camelCase")]
    Computer { host_id: String },
    #[serde(rename_all = "camelCase")]
    Server { server_id: String },
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileEntryKind {
    File,
    Dir,
    Symlink,
    Other,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileTransferDirection {
    Upload,
    Download,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileTransferState {
    Queued,
    Running,
    Paused,
    Interrupted,
    Verifying,
    Completed,
    Failed,
    Cancelled,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum FileTransferFail {
    Io,
    Sha256Mismatch,
    ResumeMismatch,
    SourceChanged,
    Protocol,
    TooLarge,
    NotFound,
    PermissionDenied,
    NotAllowed,
    NoSpace,
    AuthRequired,
    Disabled,
    Unsupported,
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileEntry {
    pub name: String,
    pub kind: FileEntryKind,
    pub size: Option<u64>,        // 目录为 None
    pub mtime_us: Option<u64>,    // 未知为 None
    pub child_count: Option<u32>, // 没数为 None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileListing {
    pub path: String,
    pub parent: Option<String>, // None = 已在根
    pub entries: Vec<FileEntry>,
    pub truncated: bool,           // 超过 10 页（1 万条）没取完
    pub net_path: Option<NetPath>, // Computer 时为当前连接路径；Server 为 None
}

#[derive(Debug, Clone, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct FileTransfer {
    pub transfer_id: String, // 本地 id；Computer 时与线上 transfer_id 相同
    pub direction: FileTransferDirection,
    pub source: FileSourceRef,
    pub name: String,
    pub remote_path: Option<String>, // download：远端文件；upload：远端目录（None = 默认）
    pub local_path: Option<String>,  // 手机上的绝对路径；download 完成后才有
    pub size: u64,
    pub done: u64,
    pub bytes_per_sec: u64,
    pub state: FileTransferState,
    pub fail: Option<FileTransferFail>,
    pub net_path: Option<NetPath>,
    pub saved_name: Option<String>, // upload 到电脑后对方实际用的名字
    pub started_at: u64,            // unix 微秒
    pub updated_at: u64,
}
