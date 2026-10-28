//! Unicode text clipboard over cliprdr (MS-RDPECLIP, ).

use std::collections::VecDeque;
use std::fmt;
use std::sync::Arc;

use ironrdp_cliprdr::backend::CliprdrBackend;
use ironrdp_cliprdr::pdu::{
    ClipboardFormat, ClipboardFormatId, ClipboardGeneralCapabilityFlags, FileContentsRequest,
    FileContentsResponse, FormatDataRequest, FormatDataResponse, LockDataId,
    OwnedFormatDataResponse,
};
use ironrdp_cliprdr::CliprdrClient;
use ironrdp_connector::{ClientConnector, Config};
use ironrdp_dvc::DrdynvcClient;
use ironrdp_session::ActiveStage;
use tokio::sync::Notify;
use tracing::warn;

use crate::ext::ExtCtx;
use crate::types::{RdpConfig, RdpError, RdpSink};

const MAX_TEXT_BYTES: usize = 1024 * 1024;

// The text-only subset of IronRDP's ClipboardMessage flow. Callbacks enqueue
// work; only pump calls the channel processor and encodes the resulting PDUs.
enum Message {
    Advertise,
    RequestText,
    Respond(OwnedFormatDataResponse),
    WarnTruncated,
}

struct TextClipboard {
    sink: Arc<dyn RdpSink>,
    wake: Arc<Notify>,
    pending: VecDeque<Message>,
    local_text: Option<String>,
    // Most recent local send or remote delivery. Explicit sends always advertise,
    // even when equal; only incoming deliveries are deduplicated.
    last_text: Option<String>,
    ready: bool,
    local_changed: bool,
    remote_text_pending: bool,
    requests: usize,
}

// Cliprdr requires Debug, but neither stored text nor queued data may be logged.
impl fmt::Debug for TextClipboard {
    fn fmt(&self, f: &mut fmt::Formatter<'_>) -> fmt::Result {
        f.debug_struct("TextClipboard").finish_non_exhaustive()
    }
}

ironrdp_core::impl_as_any!(TextClipboard);

impl TextClipboard {
    fn new(ctx: &ExtCtx) -> Self {
        Self {
            sink: Arc::clone(&ctx.sink),
            wake: Arc::clone(&ctx.wake),
            pending: VecDeque::new(),
            local_text: None,
            last_text: None,
            ready: false,
            local_changed: false,
            remote_text_pending: false,
            requests: 0,
        }
    }

    fn enqueue(&mut self, message: Message) {
        self.pending.push_back(message);
        self.wake.notify_one();
    }

    fn set_text(&mut self, text: &str) {
        let text = bounded_text(text).to_owned();
        self.last_text = Some(text.clone());
        self.local_text = Some(text);
        self.local_changed = true;
        if self.ready {
            self.enqueue(Message::Advertise);
        }
    }
}

impl CliprdrBackend for TextClipboard {
    fn temporary_directory(&self) -> &str {
        ""
    }

    fn client_capabilities(&self) -> ClipboardGeneralCapabilityFlags {
        ClipboardGeneralCapabilityFlags::USE_LONG_FORMAT_NAMES
    }

    fn on_ready(&mut self) {
        self.ready = true;
        // A command may arrive between our initial FormatList and its reply.
        if self.local_changed {
            self.enqueue(Message::Advertise);
        }
        if self.remote_text_pending {
            self.remote_text_pending = false;
            self.enqueue(Message::RequestText);
        }
    }

    fn on_request_format_list(&mut self) {
        // MonitorReady requires a FormatList to complete initialization, even
        // when we have no local text. In that case we advertise an empty list.
        self.enqueue(Message::Advertise);
    }

    fn on_process_negotiated_capabilities(&mut self, _: ClipboardGeneralCapabilityFlags) {}

    fn on_remote_copy(&mut self, formats: &[ClipboardFormat]) {
        self.remote_text_pending = formats
            .iter()
            .any(|format| format.id == ClipboardFormatId::CF_UNICODETEXT);
        if !self.remote_text_pending {
            self.last_text = None;
        } else if self.ready {
            self.remote_text_pending = false;
            self.enqueue(Message::RequestText);
        }
    }

    fn on_format_data_request(&mut self, request: FormatDataRequest) {
        if !self.ready {
            return;
        }
        let response = match (&self.local_text, request.format) {
            (Some(text), ClipboardFormatId::CF_UNICODETEXT) => {
                FormatDataResponse::new_data(encode_text(text))
            }
            _ => FormatDataResponse::new_error(),
        };
        self.enqueue(Message::Respond(response));
    }

    fn on_format_data_response(&mut self, response: FormatDataResponse<'_>) {
        // Ignore unsolicited data, which has no format ID of its own.
        if self.requests == 0 {
            return;
        }
        self.requests -= 1;
        if response.is_error() {
            return;
        }
        let Some((text, truncated)) = decode_text(response.data()) else {
            return;
        };
        if truncated {
            // client.rs suppresses dependency logs during ActiveStage::process,
            // including backend callbacks. Emit our warning from pump instead.
            self.enqueue(Message::WarnTruncated);
        }
        if self.last_text.as_ref() == Some(&text) {
            return;
        }
        self.sink.clipboard(&text);
        self.last_text = Some(text);
    }

    fn on_file_contents_request(&mut self, _: FileContentsRequest) {}
    fn on_file_contents_response(&mut self, _: FileContentsResponse<'_>) {}
    fn on_lock(&mut self, _: LockDataId) {}
    fn on_unlock(&mut self, _: LockDataId) {}
}

fn bounded_text(text: &str) -> &str {
    if text.len() <= MAX_TEXT_BYTES {
        return text;
    }
    let mut end = MAX_TEXT_BYTES;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    warn!(
        limit = MAX_TEXT_BYTES,
        "Truncating outgoing RDP clipboard text"
    );
    &text[..end]
}

fn encode_text(text: &str) -> Vec<u8> {
    bounded_text(text)
        .encode_utf16()
        .chain(std::iter::once(0))
        .flat_map(u16::to_le_bytes)
        .collect()
}

fn decode_text(mut data: &[u8]) -> Option<(String, bool)> {
    if !data.len().is_multiple_of(2) {
        warn!("Ignoring RDP clipboard text with an incomplete UTF-16 code unit");
        return None;
    }
    while data.ends_with(&[0, 0]) {
        data = &data[..data.len() - 2];
    }
    let units = data
        .chunks_exact(2)
        .map(|pair| u16::from_le_bytes([pair[0], pair[1]]));
    // Bound the UTF-8 allocation while decoding, including surrogate pairs.
    let mut text = String::new();
    for character in char::decode_utf16(units) {
        let Ok(character) = character else {
            warn!("Ignoring invalid UTF-16 RDP clipboard text");
            return None;
        };
        if text.len() + character.len_utf8() > MAX_TEXT_BYTES {
            return Some((text, true));
        }
        text.push(character);
    }
    Some((text, false))
}

/// Before `ClientConnector::new`: connector settings this feature needs.
pub fn configure(cfg: &RdpConfig, ctx: &ExtCtx, config: &mut Config) {
    let _ = (cfg, ctx, config);
}

/// Dynamic virtual channels this feature registers.
pub fn dynamic_channels(cfg: &RdpConfig, ctx: &ExtCtx, drdynvc: DrdynvcClient) -> DrdynvcClient {
    let _ = (cfg, ctx);
    drdynvc
}

/// Static virtual channels this feature registers.
pub fn static_channels(
    cfg: &RdpConfig,
    ctx: &ExtCtx,
    connector: ClientConnector,
) -> ClientConnector {
    if cfg.clipboard {
        connector.with_static_channel(CliprdrClient::new(Box::new(TextClipboard::new(ctx))))
    } else {
        connector
    }
}

/// After every `ActiveStage::process` batch and every `ctx.wake`: bytes to send.
pub fn pump(stage: &mut ActiveStage, ctx: &ExtCtx) -> Result<Vec<u8>, RdpError> {
    let _ = ctx;
    let mut bytes = Vec::new();
    while let Some(channel) = stage.get_svc_processor_mut::<CliprdrClient>() {
        let Some(backend) = channel.downcast_backend_mut::<TextClipboard>() else {
            break;
        };
        let Some(message) = backend.pending.pop_front() else {
            break;
        };
        let messages = match message {
            Message::Advertise => {
                backend.local_changed = false;
                let formats = if backend.local_text.is_some() {
                    vec![ClipboardFormat::new(ClipboardFormatId::CF_UNICODETEXT)]
                } else {
                    Vec::new()
                };
                channel.initiate_copy(&formats)
            }
            Message::RequestText => {
                backend.requests += 1;
                channel.initiate_paste(ClipboardFormatId::CF_UNICODETEXT)
            }
            Message::Respond(response) => channel.submit_format_data(response),
            Message::WarnTruncated => {
                warn!(
                    limit = MAX_TEXT_BYTES,
                    "Truncating incoming RDP clipboard text"
                );
                continue;
            }
        }
        .map_err(|error| RdpError::Protocol(format!("clipboard: {error}")))?;
        bytes.extend(
            stage
                .process_svc_processor_messages(messages)
                .map_err(|error| RdpError::Protocol(format!("clipboard: {error}")))?,
        );
    }
    Ok(bytes)
}

/// `RdpCommand::Clipboard`: announce `text` to the server; bytes to send.
pub fn send(stage: &mut ActiveStage, ctx: &ExtCtx, text: &str) -> Result<Vec<u8>, RdpError> {
    if let Some(backend) = stage
        .get_svc_processor_mut::<CliprdrClient>()
        .and_then(|channel| channel.downcast_backend_mut::<TextClipboard>())
    {
        backend.set_text(text);
    }
    pump(stage, ctx)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn decoded(data: &[u8]) -> Option<String> {
        decode_text(data).map(|(text, _)| text)
    }

    #[test]
    fn utf16_roundtrip_unit() {
        for text in ["", "你好 RDP", "café ✓", "😀", "a\0b"] {
            let encoded = encode_text(text);
            assert!(encoded.ends_with(&[0, 0]));
            assert_eq!(decode_text(&encoded), Some((text.to_owned(), false)));
            assert_eq!(
                decoded(&encoded[..encoded.len() - 2]).as_deref(),
                Some(text)
            );
        }
        assert_eq!(encode_text("😀"), [0x3d, 0xd8, 0x00, 0xde, 0, 0]);
        assert_eq!(decoded(&[b'a', 0, 0, 0, 0, 0]).as_deref(), Some("a"));
        assert_eq!(decode_text(&[0]), None);
        assert_eq!(decode_text(&[0x3d, 0xd8, 0, 0]), None);
        assert_eq!(decode_text(&[0x00, 0xde, 0, 0]), None);

        for suffix in ["é", "你", "😀"] {
            for remaining in 0..=suffix.len() {
                let prefix = "a".repeat(MAX_TEXT_BYTES - remaining);
                let text = format!("{prefix}{suffix}z");
                let expected = if remaining == suffix.len() {
                    format!("{prefix}{suffix}")
                } else {
                    prefix
                };
                assert_eq!(bounded_text(&text), expected);
                if remaining == suffix.len() - 1 || remaining == suffix.len() {
                    assert_eq!(decoded(&encode_text(&text)).unwrap(), expected);
                    // Exercise the receive cap with unbounded peer encoding too.
                    let response = FormatDataResponse::new_unicode_string(&text);
                    assert_eq!(decode_text(response.data()).unwrap(), (expected, true));
                }
            }
        }
    }
}
