//! 剪贴板通道 0x03 的客户端收发。见 docs/dev/03-protocol.md §8.1。

use std::fmt;
use std::future::Future;

use rsp_core::clipboard::{
    encode_stream_prefix, ClipboardError, ClipboardItem, ClipboardKind, StreamReceiver,
};
use rsp_core::features::{CLIPBOARD_IMAGE, CLIPBOARD_TEXT};
use rsp_core::pb;
use rsp_transport::{ConnError, UniRecv, UniSend};
use thiserror::Error;

const WRITE_CHUNK_BYTES: usize = 64 << 10;
const PREVIEW_CHARS: usize = 100;

/// Raw byte sink used to write one clipboard stream.
pub trait RawSink: Send {
    fn write_raw(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send;

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send;
}

impl RawSink for UniSend {
    fn write_raw(&mut self, bytes: &[u8]) -> impl Future<Output = Result<(), ConnError>> + Send {
        UniSend::write_raw(self, bytes)
    }

    fn finish(&mut self) -> impl Future<Output = Result<(), ConnError>> + Send {
        UniSend::finish(self)
    }
}

/// Raw byte source used to read one clipboard stream.
pub trait RawSource: Send {
    fn read_raw(
        &mut self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<Option<usize>, ConnError>> + Send;
}

impl RawSource for UniRecv {
    fn read_raw(
        &mut self,
        buf: &mut [u8],
    ) -> impl Future<Output = Result<Option<usize>, ConnError>> + Send {
        UniRecv::read_raw(self, buf)
    }
}

/// Session settings that gate clipboard traffic in both directions.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ClipboardPolicy {
    pub peer_features: u64,
    pub role: pb::Role,
    /// Host setting `[clipboard].to_phone`.
    pub to_phone: bool,
    /// Host setting `[clipboard].from_phone`.
    pub from_phone: bool,
}

#[derive(Debug, Error)]
pub enum ClipboardClientError {
    #[error("clipboard requires the controller role")]
    NotController,
    #[error("clipboard is disabled in this direction")]
    Disabled,
    #[error("peer does not support {0} clipboard")]
    FeatureUnsupported(ClipboardKind),
    #[error("clipboard codec error: {0}")]
    Codec(#[from] ClipboardError),
    #[error("clipboard transport error: {0}")]
    Transport(#[from] ConnError),
}

pub(crate) fn check_send_policy(
    kind: ClipboardKind,
    policy: &ClipboardPolicy,
) -> Result<(), ClipboardClientError> {
    if policy.role != pb::Role::Controller {
        return Err(ClipboardClientError::NotController);
    }
    if !policy.from_phone {
        return Err(ClipboardClientError::Disabled);
    }
    let feature = match kind {
        ClipboardKind::Text => CLIPBOARD_TEXT,
        ClipboardKind::Png => CLIPBOARD_IMAGE,
    };
    if policy.peer_features & feature == 0 {
        return Err(ClipboardClientError::FeatureUnsupported(kind));
    }
    Ok(())
}

/// Sends one clipboard item as a single unidirectional stream.
pub async fn send<S: RawSink>(
    sink: &mut S,
    item: &ClipboardItem,
    policy: &ClipboardPolicy,
) -> Result<(), ClipboardClientError> {
    check_send_policy(item.kind, policy)?;

    let prefix = encode_stream_prefix(item)?;
    sink.write_raw(&prefix).await?;
    for chunk in item.data.chunks(WRITE_CHUNK_BYTES) {
        sink.write_raw(chunk).await?;
    }
    sink.finish().await?;
    Ok(())
}

/// Builds a text item for `RsCore.sessions.clipboardSend(id, text)`.
pub fn text_item(text: &str) -> Result<ClipboardItem, ClipboardClientError> {
    ClipboardItem::new(ClipboardKind::Text, text.as_bytes().to_vec())
        .map_err(ClipboardClientError::Codec)
}

/// Receives one clipboard stream and returns it only after FIN and validation.
pub async fn receive<R: RawSource>(
    source: &mut R,
    policy: &ClipboardPolicy,
) -> Result<ClipboardItem, ClipboardClientError> {
    if policy.role != pb::Role::Controller {
        return Err(ClipboardClientError::NotController);
    }
    if !policy.to_phone {
        return Err(ClipboardClientError::Disabled);
    }

    let mut receiver = StreamReceiver::new();
    let mut buf = [0u8; 64 << 10];
    loop {
        match source.read_raw(&mut buf).await? {
            Some(0) => continue,
            Some(count) => receiver.feed(&buf[..count])?,
            None => break,
        }
    }
    receiver.finish().map_err(ClipboardClientError::Codec)
}

/// Metadata shown by the clipboard panel before the user accepts an item.
#[derive(Clone, PartialEq, Eq)]
pub struct ClipboardOffer {
    pub kind: ClipboardKind,
    pub size: usize,
    pub sha256: [u8; 32],
    pub preview: Option<String>,
}

impl fmt::Debug for ClipboardOffer {
    fn fmt(&self, formatter: &mut fmt::Formatter<'_>) -> fmt::Result {
        formatter
            .debug_struct("ClipboardOffer")
            .field("kind", &self.kind)
            .field("size", &self.size)
            .field("sha256", &self.sha256)
            .field("preview", &self.preview.as_ref().map(|_| "<redacted>"))
            .finish()
    }
}

struct InboxEntry {
    item: ClipboardItem,
    offer: ClipboardOffer,
}

/// Holds at most one received item until the user accepts it.
#[derive(Default)]
pub struct ClipboardInbox {
    pending: Option<InboxEntry>,
    last_sent_sha256: Option<[u8; 32]>,
}

impl ClipboardInbox {
    pub fn new() -> Self {
        Self::default()
    }

    /// Adds a received item unless it is the current item or an echo of a
    /// recently sent item. `Some` means the caller should emit
    /// `session.clipboard` for the UI.
    pub fn on_received(&mut self, item: ClipboardItem) -> Option<ClipboardOffer> {
        let sha256 = sha256(&item.data);
        if self
            .pending
            .as_ref()
            .is_some_and(|entry| entry.offer.sha256 == sha256)
            || self.last_sent_sha256 == Some(sha256)
        {
            return None;
        }

        let offer = ClipboardOffer {
            kind: item.kind,
            size: item.data.len(),
            sha256,
            preview: preview(&item),
        };
        let emitted = offer.clone();
        self.pending = Some(InboxEntry { item, offer });
        Some(emitted)
    }

    /// Remembers the most recently sent item so its Host echo is ignored.
    pub fn on_sent(&mut self, item: &ClipboardItem) {
        self.last_sent_sha256 = Some(sha256(&item.data));
    }

    pub fn latest(&self) -> Option<&ClipboardOffer> {
        self.pending.as_ref().map(|entry| &entry.offer)
    }

    /// Removes the current item for writing into the phone clipboard.
    pub fn take(&mut self) -> Option<ClipboardItem> {
        self.pending.take().map(|entry| entry.item)
    }
}

fn sha256(data: &[u8]) -> [u8; 32] {
    let digest = ring::digest::digest(&ring::digest::SHA256, data);
    let mut out = [0u8; 32];
    out.copy_from_slice(digest.as_ref());
    out
}

fn preview(item: &ClipboardItem) -> Option<String> {
    if item.kind != ClipboardKind::Text {
        return None;
    }
    let text = std::str::from_utf8(&item.data).ok()?;
    Some(
        text.chars()
            .take(PREVIEW_CHARS)
            .map(|character| match character {
                '\n' | '\r' | '\t' => ' ',
                other => other,
            })
            .collect(),
    )
}
