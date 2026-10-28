//! Wire adapters for Conn and the retained M0 QUIC API.
use bytes::Bytes;
use rsp_core::wire::FrameDecoder;
use rsp_transport::{
    CloseCode, CloseReason, Conn, ConnError, ControlStream, DatagramKind, StreamKind, UniSend,
};
use tokio::sync::Mutex;

#[derive(Clone)]
pub(super) enum Connection {
    Current(Conn),
    Legacy(quinn::Connection),
}

pub(super) enum Datagram {
    Video(Bytes),
    Audio(Bytes),
}

pub(super) enum Control {
    Current(ControlStream),
    Legacy {
        send: Mutex<quinn::SendStream>,
        recv: Mutex<LegacyRecv>,
    },
}

pub(super) struct LegacyRecv {
    recv: quinn::RecvStream,
    decoder: FrameDecoder,
}

impl Control {
    pub fn legacy(send: quinn::SendStream, recv: quinn::RecvStream) -> Self {
        Self::Legacy {
            send: Mutex::new(send),
            recv: Mutex::new(LegacyRecv {
                recv,
                decoder: FrameDecoder::new(),
            }),
        }
    }
    pub async fn send(&self, payload: &[u8]) -> Result<(), ConnError> {
        match self {
            Self::Current(c) => c.send(payload).await,
            Self::Legacy { send, .. } => {
                send.lock()
                    .await
                    .write_all(&rsp_core::wire::frame(payload))
                    .await?;
                Ok(())
            }
        }
    }
    pub async fn recv(&self) -> Result<Option<Bytes>, ConnError> {
        match self {
            Self::Current(c) => c.recv().await,
            Self::Legacy { recv, .. } => {
                let mut r = recv.lock().await;
                let mut buf = [0; 4096];
                loop {
                    if let Some(payload) = r
                        .decoder
                        .next_frame()
                        .map_err(|e| ConnError::Protocol(e.to_string()))?
                    {
                        return Ok(Some(Bytes::from(payload)));
                    }
                    match r.recv.read(&mut buf).await? {
                        Some(n) => r.decoder.push(&buf[..n]),
                        None => return Ok(None),
                    }
                }
            }
        }
    }
    pub async fn drain(&self) {
        let _ = tokio::time::timeout(std::time::Duration::from_millis(500), async {
            match self {
                Self::Current(c) => {
                    let _ = c.finish().await;
                }
                Self::Legacy { send, .. } => {
                    let mut send = send.lock().await;
                    let _ = send.finish();
                    let _ = send.stopped().await;
                }
            }
        })
        .await;
    }
}

pub(super) enum Input {
    Current(UniSend),
    Legacy(quinn::SendStream),
}
impl Input {
    pub async fn send(&mut self, payload: &[u8]) -> Result<(), ConnError> {
        match self {
            Self::Current(s) => {
                // InputMsg and the M0 InputEvent both supply a u32 length
                // prefix. UniSend owns record framing for QUIC and TCP.
                let record = payload
                    .get(4..)
                    .ok_or_else(|| ConnError::Protocol("truncated input frame".into()))?;
                s.send_record(record).await
            }
            Self::Legacy(s) => {
                s.write_all(payload).await?;
                Ok(())
            }
        }
    }
}

impl Connection {
    pub async fn open_input(&self, legacy_quic: bool) -> Result<Input, ConnError> {
        match self {
            Self::Current(c) if legacy_quic && c.as_quic().is_some() => {
                let conn = c.as_quic().ok_or(ConnError::Closed)?;
                Ok(Input::Legacy(conn.open_uni().await?))
            }
            Self::Current(c) => Ok(Input::Current(c.open_uni(StreamKind::Input).await?)),
            Self::Legacy(c) => Ok(Input::Legacy(c.open_uni().await?)),
        }
    }
    pub async fn datagram(&self) -> Result<Datagram, ConnError> {
        match self {
            Self::Legacy(c) => {
                let wire = c.read_datagram().await?;
                if wire.first() == Some(&rsp_core::audio::AUDIO_TYPE) {
                    Ok(Datagram::Audio(wire))
                } else {
                    Ok(Datagram::Video(wire))
                }
            }
            Self::Current(c) => loop {
                let (kind, payload) = c.recv_datagram().await?;
                let ty = match kind {
                    DatagramKind::Video => rsp_core::video::VIDEO_TYPE,
                    DatagramKind::Audio => rsp_core::audio::AUDIO_TYPE,
                    _ => continue,
                };
                let mut wire = Vec::with_capacity(payload.len() + 1);
                wire.push(ty);
                wire.extend_from_slice(&payload);
                let wire = Bytes::from(wire);
                return Ok(match kind {
                    DatagramKind::Audio => Datagram::Audio(wire),
                    _ => Datagram::Video(wire),
                });
            },
        }
    }
    pub fn pointer(&self, payload: &[u8]) -> Result<(), ConnError> {
        match self {
            Self::Current(c) => {
                c.send_datagram(DatagramKind::Pointer, Bytes::copy_from_slice(&payload[1..]))
            }
            Self::Legacy(c) => {
                c.send_datagram(Bytes::copy_from_slice(payload))?;
                Ok(())
            }
        }
    }
    pub async fn closed(&self) -> CloseReason {
        match self {
            Self::Current(c) => c.closed().await,
            Self::Legacy(c) => match c.closed().await {
                quinn::ConnectionError::ApplicationClosed(e) => CloseReason {
                    code: Some(e.error_code.into_inner()),
                    reason: e.reason,
                },
                e => CloseReason {
                    code: None,
                    reason: Bytes::from(e.to_string()),
                },
            },
        }
    }
    pub fn close(&self, code: CloseCode, reason: &[u8]) {
        match self {
            Self::Current(c) => c.close(code, reason),
            Self::Legacy(c) => c.close((code as u32).into(), reason),
        }
    }
    pub fn token(&self, token: Vec<u8>) -> Result<(), ConnError> {
        match self {
            Self::Current(c) => c.set_session_token(token),
            Self::Legacy(_) => Ok(()),
        }
    }
}
