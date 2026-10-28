//! Length-prefixed protobuf messages carried on the RSP control stream.

use prost::Message;
use thiserror::Error;

use crate::pb;

/// Largest encoded protobuf envelope accepted on the control stream.
pub const MAX_CONTROL_FRAME: usize = 1 << 20;

/// Errors reported while framing or decoding a control envelope.
#[derive(Debug, Error, Clone, PartialEq, Eq)]
pub enum ControlError {
    #[error("control frame of {0} bytes exceeds limit")]
    FrameTooLarge(usize),
    #[error("invalid protobuf envelope: {0}")]
    Protobuf(String),
}

/// Encodes an envelope as a four-byte big-endian length followed by protobuf bytes.
pub fn encode(env: &pb::Envelope) -> Vec<u8> {
    let payload = env.encode_to_vec();
    let mut out = Vec::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(&payload);
    out
}

/// Incremental decoder for control stream records.
#[derive(Default)]
pub struct EnvelopeDecoder {
    buf: Vec<u8>,
    error: Option<ControlError>,
}

impl EnvelopeDecoder {
    pub fn new() -> Self {
        Self::default()
    }

    /// Appends bytes from the stream. Bytes received after an error are ignored.
    pub fn push(&mut self, data: &[u8]) {
        if self.error.is_none() {
            self.buf.extend_from_slice(data);
        }
    }

    /// Returns the next complete envelope, or `None` while more bytes are needed.
    /// Once an error occurs, every subsequent call returns the same error.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Result<Option<pb::Envelope>, ControlError> {
        if let Some(error) = &self.error {
            return Err(error.clone());
        }
        if self.buf.len() < 4 {
            return Ok(None);
        }
        let len = u32::from_be_bytes([self.buf[0], self.buf[1], self.buf[2], self.buf[3]]) as usize;
        if len > MAX_CONTROL_FRAME {
            let error = ControlError::FrameTooLarge(len);
            self.error = Some(error.clone());
            return Err(error);
        }
        if self.buf.len() < 4 + len {
            return Ok(None);
        }
        let payload = &self.buf[4..4 + len];
        let env = match pb::Envelope::decode(payload) {
            Ok(env) => env,
            Err(error) => {
                let error = ControlError::Protobuf(error.to_string());
                self.error = Some(error.clone());
                return Err(error);
            }
        };
        self.buf.drain(..4 + len);
        Ok(Some(env))
    }
}

/// The protobuf codec enum is the public codec type used by clients and hosts.
pub use pb::Codec;

macro_rules! envelope_from {
    ($(($ty:ident, $variant:ident)),+ $(,)?) => {
        $(
            impl From<pb::$ty> for pb::Envelope {
                fn from(value: pb::$ty) -> Self {
                    Self { msg: Some(pb::envelope::Msg::$variant(value)) }
                }
            }
        )+
    };
}

envelope_from!(
    (ClientHello, ClientHello),
    (ServerHello, ServerHello),
    (AuthResult, AuthResult),
    (SessionState, SessionState),
    (StartStream, StartStream),
    (StreamConfig, StreamConfig),
    (KeyframeRequest, KeyframeRequest),
    (Feedback, Feedback),
    (RateUpdate, RateUpdate),
    (Ping, Ping),
    (Pong, Pong),
    (DisplayList, DisplayList),
    (SelectDisplay, SelectDisplay),
    (RequestControl, RequestControl),
    (ControlRequest, ControlRequest),
    (ControlResponse, ControlResponse),
    (LocalActivity, LocalActivity),
    (IdleWarning, IdleWarning),
    (PauseStream, PauseStream),
    (ResumeStream, ResumeStream),
    (HostStats, HostStats),
    (SessionAction, SessionAction),
    (SetQuality, SetQuality),
    (SetAudio, SetAudio),
    (RecordingNotice, RecordingNotice),
    (CreateVirtualDisplay, CreateVirtualDisplay),
    (VirtualDisplayResult, VirtualDisplayResult),
    (ReleaseVirtualDisplay, ReleaseVirtualDisplay),
    (Viewport, Viewport),
    (ViewportAck, ViewportAck),
    (AttachInput, AttachInput),
    (PairStart, PairStart),
    (PairSpake, PairSpake),
    (PairConfirm, PairConfirm),
    (PairResult, PairResult),
    (PairPassword, PairPassword),
    (ErrorInfo, Error),
    (Goodbye, Goodbye),
    (FileList, FileList),
    (FileListing, FileListing),
    (FileRequest, FileRequest),
    (FileQuery, FileQuery),
    (FileStatus, FileStatus),
    (FileCancel, FileCancel),
);
