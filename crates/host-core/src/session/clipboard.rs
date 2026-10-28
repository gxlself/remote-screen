//! 剪贴板同步策略与 0x03 流读写。
//!
//! 本模块不接线到运行时。它负责方向、功能位、大小、防回环和去重判定，
//! 以及 QUIC 流与 TCP clipboard 记录的编解码。日志只记录类型、长度和 sha256
//! 前缀，绝不记录剪贴板内容。

use std::fmt::Write as _;
use std::sync::{Arc, Mutex};

use ring::digest::{digest, SHA256};
use rsp_core::clipboard::{
    encode_stream_prefix, encode_tcp_chunks, ClipboardError, ClipboardItem, ClipboardKind,
    StreamReceiver,
};
use rsp_core::features::{CLIPBOARD_IMAGE, CLIPBOARD_TEXT};
use rsp_core::pb::clipboard_header::Kind;
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::sync::{mpsc, watch};
use tracing::debug;

use crate::config;
use crate::platform::{self, Platform, PlatformError};

/// QUIC 单向流首字节，表示剪贴板通道。
pub const CLIPBOARD_STREAM_TYPE: u8 = 0x03;
/// 接收流时每次向状态机喂入的最大字节数。
pub const READ_CHUNK: usize = 64 * 1024;

/// 本机配置允许的同步方向。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipDirections {
    pub to_phone: bool,
    pub from_phone: bool,
}

impl From<&config::ClipboardConfig> for ClipDirections {
    fn from(config: &config::ClipboardConfig) -> Self {
        Self {
            to_phone: config.to_phone,
            from_phone: config.from_phone,
        }
    }
}

/// 已通过发送策略、待交给传输层的剪贴板条目。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OutgoingClip {
    pub item: ClipboardItem,
    pub sha256: [u8; 32],
}

/// 本机剪贴板变化被策略跳过的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SkipReason {
    Disabled,
    NoController,
    Empty,
    InvalidKind,
    NotSupportedByApp,
    TooLarge,
    InvalidText,
    Echo,
    Duplicate,
}

/// App 发来的剪贴板条目被拒绝的原因。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum RejectReason {
    Viewer,
    Disabled,
}

/// 剪贴板流读写的统一错误。
#[derive(Debug, thiserror::Error)]
pub enum ClipStreamError {
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Clipboard(#[from] ClipboardError),
}

/// 计算剪贴板内容的 SHA-256。
pub fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = digest(&SHA256, data);
    let mut out = [0_u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

/// 无 I/O 的剪贴板同步策略。
#[derive(Debug)]
pub struct ClipboardSync {
    directions: ClipDirections,
    last_pushed: Option<[u8; 32]>,
    last_from_phone: Option<[u8; 32]>,
    next_tcp_item_id: u32,
}

impl ClipboardSync {
    pub fn new(directions: ClipDirections) -> Self {
        Self {
            directions,
            last_pushed: None,
            last_from_phone: None,
            next_tcp_item_id: 1,
        }
    }

    pub fn set_directions(&mut self, directions: ClipDirections) {
        self.directions = directions;
    }

    /// 新控制者不会自动收到当前剪贴板；它之后触发的变化仍会正常推送。
    /// 防回环记录保留，因为同一份内容仍可能由平台回报。
    pub fn on_controller_changed(&mut self) {
        self.last_pushed = None;
    }

    /// 判定本机剪贴板变化是否应推送给当前控制者。
    pub fn outbound(
        &mut self,
        item: Option<&platform::ClipboardItem>,
        controller_features: Option<u64>,
    ) -> Result<OutgoingClip, SkipReason> {
        if !self.directions.to_phone {
            return Err(SkipReason::Disabled);
        }
        let features = controller_features.ok_or(SkipReason::NoController)?;
        let item = item.ok_or(SkipReason::Empty)?;
        let kind = clipboard_kind(item.kind).ok_or(SkipReason::InvalidKind)?;
        if !app_supports(features, kind) {
            return Err(SkipReason::NotSupportedByApp);
        }
        if item.data.len() > kind.max_bytes() {
            return Err(SkipReason::TooLarge);
        }

        let clipboard_item =
            ClipboardItem::new(kind, item.data.clone()).map_err(skip_reason_for_clipboard_error)?;
        let item_sha = sha256(&item.data);

        if self.last_from_phone == Some(item_sha) {
            self.last_from_phone = None;
            return Err(SkipReason::Echo);
        }
        // 本机内容已经变化，旧的回环记录不再对应平台当前内容。
        self.last_from_phone = None;

        if self.last_pushed == Some(item_sha) {
            return Err(SkipReason::Duplicate);
        }
        self.last_pushed = Some(item_sha);
        Ok(OutgoingClip {
            item: clipboard_item,
            sha256: item_sha,
        })
    }

    /// 校验控制者发来的剪贴板条目，并转换成待写入平台的形式。
    pub fn inbound(
        &mut self,
        from_controller: bool,
        item: ClipboardItem,
    ) -> Result<platform::ClipboardItem, RejectReason> {
        if !from_controller {
            return Err(RejectReason::Viewer);
        }
        if !self.directions.from_phone {
            return Err(RejectReason::Disabled);
        }

        let item_sha = sha256(&item.data);
        self.last_from_phone = Some(item_sha);
        // 手机现在已经持有这份内容，下一次平台回报不应再推回手机。
        self.last_pushed = Some(item_sha);
        Ok(platform::ClipboardItem {
            kind: item.kind.into(),
            data: item.data.to_vec(),
            sha256: item_sha,
        })
    }

    /// 分配 TCP 回退使用的剪贴板条目编号；从 1 开始并回绕。
    pub fn next_tcp_item_id(&mut self) -> u32 {
        let item_id = self.next_tcp_item_id;
        self.next_tcp_item_id = self.next_tcp_item_id.wrapping_add(1);
        item_id
    }
}

fn clipboard_kind(kind: Kind) -> Option<ClipboardKind> {
    match kind {
        Kind::Text => Some(ClipboardKind::Text),
        Kind::Png => Some(ClipboardKind::Png),
        Kind::Unspecified => None,
    }
}

fn app_supports(features: u64, kind: ClipboardKind) -> bool {
    let feature = match kind {
        ClipboardKind::Text => CLIPBOARD_TEXT,
        ClipboardKind::Png => CLIPBOARD_IMAGE,
    };
    features & feature != 0
}

fn kind_field(kind: Kind) -> &'static str {
    match kind {
        Kind::Text => "TEXT",
        Kind::Png => "PNG",
        Kind::Unspecified => "KIND_UNSPECIFIED",
    }
}

fn skip_reason_for_clipboard_error(error: ClipboardError) -> SkipReason {
    match error {
        ClipboardError::DataTooLarge { .. } => SkipReason::TooLarge,
        ClipboardError::InvalidUtf8 => SkipReason::InvalidText,
        _ => SkipReason::InvalidKind,
    }
}

/// 编码一条完整的 QUIC 剪贴板流（含 0x03 类型字节）。
pub fn quic_stream_bytes(item: &ClipboardItem) -> Result<Vec<u8>, ClipboardError> {
    let prefix = encode_stream_prefix(item)?;
    let mut out = Vec::with_capacity(1 + prefix.len() + item.data.len());
    out.push(CLIPBOARD_STREAM_TYPE);
    out.extend_from_slice(&prefix);
    out.extend_from_slice(&item.data);
    Ok(out)
}

/// 编码 TCP clipboard 记录；channel 字节由 TCP 分帧层负责，不包含在这里。
pub fn tcp_records(
    item_id: u32,
    item: &ClipboardItem,
    max_chunk: usize,
) -> Result<Vec<Vec<u8>>, ClipboardError> {
    let prefix = encode_stream_prefix(item)?;
    let mut prefix_and_data = Vec::with_capacity(prefix.len() + item.data.len());
    prefix_and_data.extend_from_slice(&prefix);
    prefix_and_data.extend_from_slice(&item.data);
    Ok(encode_tcp_chunks(item_id, &prefix_and_data, max_chunk))
}

/// 写一条 QUIC 剪贴板流并发送 FIN。
pub async fn write_stream<W: AsyncWrite + Unpin>(
    writer: &mut W,
    item: &ClipboardItem,
) -> Result<(), ClipStreamError> {
    let bytes = quic_stream_bytes(item)?;
    writer.write_all(&bytes).await?;
    writer.shutdown().await?;
    Ok(())
}

/// 读取一条已经剥离 0x03 类型字节的 QUIC 剪贴板流。
///
/// 头部一旦声明超限尺寸，`StreamReceiver` 会立即报错，本函数不会继续等待
/// 或分配声明的数据长度。
pub async fn read_stream<R: AsyncRead + Unpin>(
    reader: &mut R,
) -> Result<ClipboardItem, ClipStreamError> {
    let mut receiver = StreamReceiver::new();
    let mut chunk = vec![0_u8; READ_CHUNK];
    loop {
        let read = reader.read(&mut chunk).await?;
        if read == 0 {
            break;
        }
        receiver.feed(&chunk[..read])?;
    }
    receiver.finish().map_err(ClipStreamError::from)
}

/// Write clipboard bytes after `Conn::open_uni` has written the channel type.
pub async fn write_uni_stream(
    writer: &mut rsp_transport::UniSend,
    item: &ClipboardItem,
) -> Result<(), ClipStreamError> {
    let mut bytes = encode_stream_prefix(item)?;
    bytes.extend_from_slice(&item.data);
    writer
        .write_raw(&bytes)
        .await
        .map_err(std::io::Error::other)?;
    writer.finish().await.map_err(std::io::Error::other)?;
    Ok(())
}

/// Read raw clipboard bytes without requiring a transport-specific AsyncRead.
pub async fn read_uni_stream(
    reader: &mut rsp_transport::UniRecv,
) -> Result<ClipboardItem, ClipStreamError> {
    let mut receiver = StreamReceiver::new();
    let mut chunk = vec![0_u8; READ_CHUNK];
    while let Some(read) = reader
        .read_raw(&mut chunk)
        .await
        .map_err(std::io::Error::other)?
    {
        receiver.feed(&chunk[..read])?;
    }
    receiver.finish().map_err(ClipStreamError::from)
}

/// 转换成平台层条目；sha256 始终由内容重新计算。
pub fn to_platform(item: &ClipboardItem) -> platform::ClipboardItem {
    platform::ClipboardItem {
        kind: item.kind.into(),
        data: item.data.to_vec(),
        sha256: sha256(&item.data),
    }
}

/// 校验并转换平台层条目，不信任平台提供的 sha256。
pub fn from_platform(item: &platform::ClipboardItem) -> Result<ClipboardItem, SkipReason> {
    let kind = clipboard_kind(item.kind).ok_or(SkipReason::InvalidKind)?;
    ClipboardItem::new(kind, item.data.clone()).map_err(skip_reason_for_clipboard_error)
}

/// 泵与平台适配使用的同步剪贴板接口。
pub trait ClipboardPort: Send + Sync {
    fn get(&self) -> Option<platform::ClipboardItem>;
    fn set(&self, item: platform::ClipboardItem) -> Result<(), PlatformError>;
}

/// 把 [`Platform`] 的剪贴板方法适配成 [`ClipboardPort`]。
pub struct PlatformClipboard<P: Platform>(pub Arc<P>);

impl<P: Platform> ClipboardPort for PlatformClipboard<P> {
    fn get(&self) -> Option<platform::ClipboardItem> {
        self.0.clipboard_get()
    }

    fn set(&self, item: platform::ClipboardItem) -> Result<(), PlatformError> {
        self.0.clipboard_set(item)
    }
}

/// 消费平台剪贴板变化通知，合并连发后把可推送条目送到 `out`。
pub async fn pump_local_changes(
    port: Arc<dyn ClipboardPort>,
    mut changes: mpsc::Receiver<()>,
    sync: Arc<Mutex<ClipboardSync>>,
    controller: watch::Receiver<Option<u64>>,
    out: mpsc::Sender<OutgoingClip>,
) {
    while changes.recv().await.is_some() {
        while changes.try_recv().is_ok() {}

        let item = port.get();
        let features = *controller.borrow();
        let outcome = {
            let Ok(mut sync) = sync.lock() else {
                debug!("clipboard sync lock poisoned; skipping clipboard change");
                continue;
            };
            sync.outbound(item.as_ref(), features)
        };

        match outcome {
            Ok(outgoing) => {
                debug!(
                    kind = kind_field(outgoing.item.kind.into()),
                    len = outgoing.item.data.len(),
                    sha = %sha_prefix(&outgoing.sha256),
                    "clipboard change ready"
                );
                if out.send(outgoing).await.is_err() {
                    break;
                }
            }
            Err(reason) => {
                let (kind, len) = item.as_ref().map_or((None, 0), |item| {
                    (Some(kind_field(item.kind)), item.data.len())
                });
                debug!(?reason, kind, len, "clipboard change skipped");
            }
        }
    }
}

fn sha_prefix(sha: &[u8; 32]) -> String {
    let mut out = String::with_capacity(16);
    for byte in &sha[..8] {
        let _ = write!(out, "{byte:02x}");
    }
    out
}

/// 策略拒绝或平台写入失败。
#[derive(Debug, thiserror::Error)]
pub enum InboundError {
    #[error("clipboard item rejected ({0:?})")]
    Rejected(RejectReason),
    #[error(transparent)]
    Platform(#[from] PlatformError),
}

/// 把一条已通过流校验的 App 剪贴板条目写入平台。
pub fn apply_inbound(
    port: &dyn ClipboardPort,
    sync: &Mutex<ClipboardSync>,
    from_controller: bool,
    item: ClipboardItem,
) -> Result<(), InboundError> {
    let platform_item = {
        let Ok(mut sync) = sync.lock() else {
            return Ok(());
        };
        sync.inbound(from_controller, item)
            .map_err(InboundError::Rejected)?
    };
    port.set(platform_item)?;
    Ok(())
}
