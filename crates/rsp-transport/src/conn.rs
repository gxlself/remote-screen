//! Transport-neutral connections and length-prefixed streams.

use std::net::SocketAddr;
use std::ops::Deref;
use std::sync::{
    atomic::{AtomicBool, AtomicU64, Ordering},
    Arc,
};
use std::time::Duration;

use bytes::{Buf, Bytes, BytesMut};
use rustls::pki_types::CertificateDer;
use thiserror::Error;
use tokio::sync::{mpsc, watch, Mutex};

pub use crate::mux::{DatagramKind, StreamKind};
use crate::tcp::{TcpShared, TcpUniSend};
pub use quinn::SendDatagramError;

pub const MAX_CONTROL_FRAME: usize = 1 << 20;
pub const MAX_STREAM_RECORD: usize = 16 << 20;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum TransportKind {
    Quic,
    Tcp,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
#[repr(u32)]
pub enum CloseCode {
    Normal = 0,
    Protocol = 1,
    Auth = 2,
    Replaced = 3,
    HostShutdown = 4,
    Idle = 5,
    Revoked = 6,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CloseReason {
    /// TCP EOF/TLS close_notify does not carry an RSP application close code.
    pub code: Option<u64>,
    pub reason: Bytes,
}

#[derive(Debug, Error)]
pub enum ConnError {
    #[error("I/O error: {0}")]
    Io(#[from] std::io::Error),
    #[error("TLS error: {0}")]
    Tls(String),
    #[error("QUIC error: {0}")]
    Quic(String),
    #[error("QUIC datagram send error: {0}")]
    SendDatagram(#[from] quinn::SendDatagramError),
    #[error("protocol error: {0}")]
    Protocol(String),
    #[error("TLS configuration/negotiation requires ALPN rsp/1")]
    MissingAlpn,
    #[error("connection is closed")]
    Closed,
    #[error("outbound queue is full")]
    WouldBlock,
    #[error("unsupported operation: {0}")]
    Unsupported(&'static str),
}

impl From<quinn::WriteError> for ConnError {
    fn from(error: quinn::WriteError) -> Self {
        Self::Quic(error.to_string())
    }
}
impl From<quinn::ReadError> for ConnError {
    fn from(error: quinn::ReadError) -> Self {
        Self::Quic(error.to_string())
    }
}
impl From<quinn::ConnectionError> for ConnError {
    fn from(error: quinn::ConnectionError) -> Self {
        Self::Quic(error.to_string())
    }
}

pub(crate) struct QuicShared {
    connection: quinn::Connection,
    keepalive: watch::Sender<Duration>,
    control_taken: AtomicBool,
    unknown_datagrams: AtomicU64,
}

pub(crate) struct StreamReservation<'a> {
    taken: &'a AtomicBool,
    committed: bool,
}

impl<'a> StreamReservation<'a> {
    pub(crate) fn take(taken: &'a AtomicBool) -> Result<Self, ConnError> {
        if taken.swap(true, Ordering::AcqRel) {
            Err(ConnError::Protocol("stream already opened".into()))
        } else {
            Ok(Self {
                taken,
                committed: false,
            })
        }
    }

    pub(crate) fn commit(mut self) {
        self.committed = true;
    }
}

impl Drop for StreamReservation<'_> {
    fn drop(&mut self) {
        if !self.committed {
            self.taken.store(false, Ordering::Release);
        }
    }
}

impl Deref for QuicShared {
    type Target = quinn::Connection;
    fn deref(&self) -> &Self::Target {
        &self.connection
    }
}

#[derive(Clone)]
pub(crate) enum ConnInner {
    Quic(Arc<QuicShared>),
    Tcp(Arc<TcpShared>),
}

#[derive(Clone)]
pub struct Conn {
    pub(crate) inner: ConnInner,
}

/// Wrap a compatibility endpoint without changing its configured transport or
/// adding Conn's heartbeat task to an already-established QUIC connection.
impl From<quinn::Connection> for Conn {
    fn from(connection: quinn::Connection) -> Self {
        let (keepalive, _) = watch::channel(Duration::from_secs(2));
        Self {
            inner: ConnInner::Quic(Arc::new(QuicShared {
                connection,
                keepalive,
                control_taken: AtomicBool::new(false),
                unknown_datagrams: AtomicU64::new(0),
            })),
        }
    }
}

impl Conn {
    pub(crate) fn from_quic(connection: quinn::Connection, keepalive: Duration) -> Self {
        let (tx, mut rx) = watch::channel(keepalive);
        let pulse = connection.clone();
        // Quinn has no runtime keep-alive setter. Empty DATAGRAMs provide a
        // mutable heartbeat; readers consume these internally, never as media.
        // Like Quinn, cap it at half the idle timeout to keep prewarm alive.
        tokio::spawn(async move {
            loop {
                let delay = (*rx.borrow_and_update()).min(Duration::from_secs(5));
                tokio::select! {
                    _ = pulse.closed() => return,
                    changed = rx.changed() => if changed.is_err() { return; },
                    _ = tokio::time::sleep(delay) => {
                        if pulse.send_datagram(Bytes::new()).is_err() { return; }
                    }
                }
            }
        });
        Self {
            inner: ConnInner::Quic(Arc::new(QuicShared {
                connection,
                keepalive: tx,
                control_taken: AtomicBool::new(false),
                unknown_datagrams: AtomicU64::new(0),
            })),
        }
    }

    pub(crate) fn from_tcp(shared: Arc<TcpShared>) -> Self {
        Self {
            inner: ConnInner::Tcp(shared),
        }
    }

    pub fn kind(&self) -> TransportKind {
        match self.inner {
            ConnInner::Quic(_) => TransportKind::Quic,
            ConnInner::Tcp(_) => TransportKind::Tcp,
        }
    }

    /// 已排队、尚未发出的数据报内存字节数（含 QUIC 每包元数据）。
    /// QUIC：发送缓冲容量减可用空间，再扣除 Quinn 为下一包预留的元数据（饱和减）。
    /// TCP：恒为 0（TCP 的优先级由 writer_loop 的 biased select 保证）。
    pub fn datagram_backlog_bytes(&self) -> usize {
        match &self.inner {
            ConnInner::Quic(c) => {
                // quinn-proto 0.11's Datagram wraps Bytes. Its space query reserves
                // one extra Datagram, even with an empty queue; exclude that slot.
                crate::DATAGRAM_SEND_BUFFER_BYTES
                    .saturating_sub(c.datagram_send_buffer_space())
                    .saturating_sub(std::mem::size_of::<Bytes>())
            }
            ConnInner::Tcp(_) => 0,
        }
    }

    /// Stable for clones and unique per connection within this process.
    pub fn stable_id(&self) -> String {
        match &self.inner {
            ConnInner::Quic(c) => format!("quic-{}", c.stable_id()),
            ConnInner::Tcp(c) => format!("tcp-{}", c.stable_id()),
        }
    }

    /// Adapt an accepted connection to legacy QUIC-only media consumers.
    /// TCP callers must use the transport-neutral stream APIs instead.
    pub fn as_quic(&self) -> Option<quinn::Connection> {
        match &self.inner {
            ConnInner::Quic(connection) => Some((**connection).clone()),
            ConnInner::Tcp(_) => None,
        }
    }
    pub fn remote_addr(&self) -> SocketAddr {
        match &self.inner {
            ConnInner::Quic(c) => c.remote_address(),
            ConnInner::Tcp(c) => c.remote_addr(),
        }
    }
    pub fn peer_certificates(&self) -> Vec<CertificateDer<'static>> {
        match &self.inner {
            ConnInner::Quic(c) => c
                .peer_identity()
                .and_then(|i| {
                    i.downcast::<Vec<CertificateDer<'static>>>()
                        .ok()
                        .map(|certs| *certs)
                })
                .unwrap_or_default(),
            ConnInner::Tcp(c) => c.peer_certificates(),
        }
    }

    pub async fn open_control(&self) -> Result<ControlStream, ConnError> {
        self.control(false).await
    }
    pub async fn accept_control(&self) -> Result<ControlStream, ConnError> {
        self.control(true).await
    }

    async fn control(&self, accept: bool) -> Result<ControlStream, ConnError> {
        match &self.inner {
            ConnInner::Quic(c) => {
                let reservation = StreamReservation::take(&c.control_taken)?;
                let (send, recv) = if accept {
                    c.accept_bi().await
                } else {
                    c.open_bi().await
                }
                .map_err(ConnError::from)?;
                reservation.commit();
                c.keepalive.send_replace(Duration::from_secs(2));
                Ok(ControlStream {
                    inner: ControlInner::Quic {
                        send: Mutex::new(send),
                        recv: Box::new(Mutex::new(UniRecv::from_quic(recv))),
                        owner: self.clone(),
                    },
                })
            }
            ConnInner::Tcp(c) => {
                c.take_control()?;
                Ok(ControlStream {
                    inner: ControlInner::Tcp(c.clone()),
                })
            }
        }
    }

    pub async fn open_uni(&self, kind: StreamKind) -> Result<UniSend, ConnError> {
        let inner = match &self.inner {
            ConnInner::Quic(c) => {
                let mut send = c.open_uni().await?;
                send.write_all(&[kind as u8]).await?;
                UniSendInner::Quic(send)
            }
            ConnInner::Tcp(c) => UniSendInner::Tcp(c.open_uni(kind).await?),
        };
        Ok(UniSend {
            inner,
            finished: false,
        })
    }

    pub async fn accept_uni(&self) -> Result<(StreamKind, UniRecv), ConnError> {
        match &self.inner {
            ConnInner::Quic(c) => loop {
                let mut recv = c.accept_uni().await?;
                let mut kind = [0u8; 1];
                if recv.read_exact(&mut kind).await.is_ok() {
                    if let Ok(kind) = StreamKind::try_from(kind[0]) {
                        return Ok((kind, UniRecv::from_quic(recv)));
                    }
                }
                let _ = recv.stop(quinn::VarInt::from_u32(CloseCode::Protocol as u32));
            },
            ConnInner::Tcp(c) => c.accept_uni().await,
        }
    }

    pub fn send_datagram(&self, kind: DatagramKind, payload: Bytes) -> Result<(), ConnError> {
        match &self.inner {
            ConnInner::Quic(c) => {
                let mut out = BytesMut::with_capacity(1 + payload.len());
                out.extend_from_slice(&[kind as u8]);
                out.extend_from_slice(&payload);
                c.send_datagram(out.freeze()).map_err(ConnError::from)
            }
            ConnInner::Tcp(c) => c.send_datagram(kind, payload),
        }
    }

    /// Wait for QUIC send-buffer capacity instead of evicting queued datagrams.
    /// TCP retains its existing record queue and latest-video-frame policy.
    pub async fn send_datagram_wait(
        &self,
        kind: DatagramKind,
        payload: Bytes,
    ) -> Result<(), ConnError> {
        match &self.inner {
            ConnInner::Quic(c) => {
                let mut out = BytesMut::with_capacity(1 + payload.len());
                out.extend_from_slice(&[kind as u8]);
                out.extend_from_slice(&payload);
                c.send_datagram_wait(out.freeze())
                    .await
                    .map_err(ConnError::from)
            }
            ConnInner::Tcp(c) => c.send_datagram(kind, payload),
        }
    }

    pub async fn recv_datagram(&self) -> Result<(DatagramKind, Bytes), ConnError> {
        match &self.inner {
            ConnInner::Quic(c) => loop {
                let data = c.read_datagram().await?;
                if data.is_empty() {
                    continue;
                }
                if let Ok(kind) = DatagramKind::try_from(data[0]) {
                    return Ok((kind, data.slice(1..)));
                }
                c.unknown_datagrams.fetch_add(1, Ordering::Relaxed);
            },
            ConnInner::Tcp(c) => c.recv_datagram().await,
        }
    }

    pub fn unknown_datagram_count(&self) -> u64 {
        match &self.inner {
            ConnInner::Quic(c) => c.unknown_datagrams.load(Ordering::Relaxed),
            ConnInner::Tcp(c) => c.unknown_datagram_count(),
        }
    }
    /// TCP datagrams discarded because the local receive queue was full.
    /// Shared by connection clones. QUIC returns 0; Quinn does not expose this count.
    pub fn dropped_datagram_count(&self) -> u64 {
        match &self.inner {
            ConnInner::Tcp(c) => c.dropped_datagram_count(),
            ConnInner::Quic(_) => 0,
        }
    }
    pub fn dropped_video_frames(&self) -> u64 {
        match &self.inner {
            ConnInner::Tcp(c) => c.dropped_video_frames(),
            ConnInner::Quic(_) => 0,
        }
    }
    /// Maximum payload size accepted by `send_datagram`, excluding the kind byte.
    pub fn max_datagram_size(&self) -> Option<usize> {
        match &self.inner {
            ConnInner::Quic(c) => c.max_datagram_size().and_then(|n| n.checked_sub(1)),
            ConnInner::Tcp(_) => None,
        }
    }
    pub fn close(&self, code: CloseCode, reason: &[u8]) {
        match &self.inner {
            ConnInner::Quic(c) => c.close(quinn::VarInt::from_u32(code as u32), reason),
            ConnInner::Tcp(c) => c.close(Some(code as u64), Bytes::copy_from_slice(reason)),
        }
    }
    pub async fn closed(&self) -> CloseReason {
        match &self.inner {
            ConnInner::Quic(c) => match c.closed().await {
                quinn::ConnectionError::ApplicationClosed(e) => CloseReason {
                    code: Some(e.error_code.into_inner()),
                    reason: e.reason,
                },
                e => CloseReason {
                    code: None,
                    reason: Bytes::from(e.to_string()),
                },
            },
            ConnInner::Tcp(c) => c.closed().await,
        }
    }
    pub fn set_keepalive(&self, interval: Duration) -> Result<(), ConnError> {
        if interval.is_zero() {
            return Err(ConnError::Protocol(
                "keep-alive interval must be nonzero".into(),
            ));
        }
        match &self.inner {
            ConnInner::Quic(c) => {
                c.keepalive.send_replace(interval);
                Ok(())
            }
            ConnInner::Tcp(_) => Err(ConnError::Unsupported(
                "TCP keep-alive is managed by the session",
            )),
        }
    }
    pub fn set_session_token(&self, token: Vec<u8>) -> Result<(), ConnError> {
        match &self.inner {
            ConnInner::Tcp(c) => c.set_session_token(token),
            ConnInner::Quic(_) => Ok(()),
        }
    }
    pub fn set_keyframe_request_callback(
        &self,
        callback: Arc<dyn Fn() + Send + Sync>,
    ) -> Result<(), ConnError> {
        match &self.inner {
            ConnInner::Tcp(c) => {
                c.set_keyframe_callback(callback);
                Ok(())
            }
            ConnInner::Quic(_) => Err(ConnError::Unsupported("keyframe backpressure is TCP only")),
        }
    }
}

enum ControlInner {
    Quic {
        send: Mutex<quinn::SendStream>,
        // Boxed: on macOS `Mutex` is large enough to trip clippy::large_enum_variant.
        recv: Box<Mutex<UniRecv>>,
        owner: Conn,
    },
    Tcp(Arc<TcpShared>),
}
pub struct ControlStream {
    inner: ControlInner,
}

impl ControlStream {
    /// Drain previously queued control records to TLS before closing TCP.
    /// QUIC uses its existing FIN/acknowledgement drain instead.
    pub async fn flush(&self) -> Result<(), ConnError> {
        match &self.inner {
            ControlInner::Tcp(c) => c.flush_control().await,
            ControlInner::Quic { .. } => Ok(()),
        }
    }

    /// Compatibility byte reads for sessions that own control framing and
    /// protocol-error close reasons. TCP callers must use `recv` instead.
    pub async fn read_raw(&self, buf: &mut [u8]) -> Result<Option<usize>, ConnError> {
        match &self.inner {
            ControlInner::Quic { recv, .. } => recv.lock().await.read_raw(buf).await,
            ControlInner::Tcp(_) => Err(ConnError::Unsupported("TCP control uses records")),
        }
    }

    /// Wait for the QUIC peer's STOP_SENDING or acknowledgement of all bytes
    /// after FIN, preserving the compatibility session's bounded drain.
    pub async fn stopped(&self) -> Result<Option<u64>, ConnError> {
        match &self.inner {
            ControlInner::Quic { send, .. } => {
                let stopped = send.lock().await.stopped();
                stopped
                    .await
                    .map(|code| code.map(|code| code.into_inner()))
                    .map_err(|e| ConnError::Quic(e.to_string()))
            }
            ControlInner::Tcp(_) => Err(ConnError::Unsupported(
                "TCP has no control FIN acknowledgement",
            )),
        }
    }

    pub async fn send(&self, payload: &[u8]) -> Result<(), ConnError> {
        if payload.len() > MAX_CONTROL_FRAME {
            return Err(ConnError::Protocol("control frame exceeds 1 MiB".into()));
        }
        match &self.inner {
            ControlInner::Quic { send, .. } => {
                send.lock().await.write_all(&framed(payload)).await?;
                Ok(())
            }
            ControlInner::Tcp(c) => c.send_control(Bytes::copy_from_slice(payload)).await,
        }
    }
    pub async fn recv(&self) -> Result<Option<Bytes>, ConnError> {
        match &self.inner {
            ControlInner::Quic { recv, owner, .. } => {
                let result = recv.lock().await.recv_with_limit(MAX_CONTROL_FRAME).await;
                if matches!(&result, Err(ConnError::Protocol(_))) {
                    owner.close(CloseCode::Protocol, b"invalid control frame");
                }
                result
            }
            ControlInner::Tcp(c) => c.recv_control().await,
        }
    }
    pub async fn finish(&self) -> Result<(), ConnError> {
        match &self.inner {
            ControlInner::Quic { send, .. } => send
                .lock()
                .await
                .finish()
                .map_err(|e| ConnError::Quic(e.to_string())),
            ControlInner::Tcp(_) => Err(ConnError::Unsupported(
                "TCP control FIN requires closing the connection",
            )),
        }
    }
}

pub(crate) fn framed(payload: &[u8]) -> Bytes {
    let mut out = BytesMut::with_capacity(4 + payload.len());
    out.extend_from_slice(&(payload.len() as u32).to_be_bytes());
    out.extend_from_slice(payload);
    out.freeze()
}

enum UniSendInner {
    Quic(quinn::SendStream),
    Tcp(TcpUniSend),
}
pub struct UniSend {
    inner: UniSendInner,
    finished: bool,
}
impl UniSend {
    /// Reset a QUIC stream or TCP file stream; other TCP streams are unchanged.
    pub fn reset(&mut self, code: u32) -> Result<(), ConnError> {
        match &mut self.inner {
            UniSendInner::Quic(send) => {
                send.reset(code.into())
                    .map_err(|e| ConnError::Quic(e.to_string()))?;
                self.finished = true;
                Ok(())
            }
            UniSendInner::Tcp(send) => {
                self.finished |= send.reset(code)?;
                Ok(())
            }
        }
    }

    pub async fn send_record(&mut self, payload: &[u8]) -> Result<(), ConnError> {
        if self.finished {
            return Err(ConnError::Closed);
        }
        if payload.len() > MAX_STREAM_RECORD {
            return Err(ConnError::Protocol("stream record exceeds 16 MiB".into()));
        }
        match &mut self.inner {
            UniSendInner::Quic(send) => {
                send.write_all(&framed(payload)).await?;
                Ok(())
            }
            UniSendInner::Tcp(send) => send.send_record(payload).await,
        }
    }
    pub async fn write_raw(&mut self, payload: &[u8]) -> Result<(), ConnError> {
        if self.finished {
            return Err(ConnError::Closed);
        }
        match &mut self.inner {
            UniSendInner::Quic(send) => {
                send.write_all(payload).await?;
                Ok(())
            }
            UniSendInner::Tcp(send) => send.write_raw(payload).await,
        }
    }
    pub async fn finish(&mut self) -> Result<(), ConnError> {
        if self.finished {
            return Ok(());
        }
        match &mut self.inner {
            UniSendInner::Quic(send) => {
                send.finish().map_err(|e| ConnError::Quic(e.to_string()))?
            }
            UniSendInner::Tcp(send) => send.finish().await?,
        }
        self.finished = true;
        Ok(())
    }
}

enum RecvInner {
    Quic(quinn::RecvStream),
    Tcp {
        receiver: mpsc::Receiver<Result<Bytes, ConnError>>,
        file: Option<(Arc<TcpShared>, u32)>,
    },
}
pub struct UniRecv {
    inner: RecvInner,
    buffered: BytesMut,
    eof: bool,
}

/// Preserve the host's legacy input stream dispatch and independent type-read
/// deadlines while using neutral raw reads after the type byte is consumed.
impl From<quinn::RecvStream> for UniRecv {
    fn from(recv: quinn::RecvStream) -> Self {
        Self::from_quic(recv)
    }
}
impl UniRecv {
    /// Stop a stream and discard buffered bytes. TCP file streams notify the
    /// sender with STOP; other TCP channels only close their local receiver.
    pub fn stop(&mut self, code: u32) -> Result<(), ConnError> {
        match &mut self.inner {
            RecvInner::Quic(recv) => {
                recv.stop(code.into())
                    .map_err(|e| ConnError::Quic(e.to_string()))?;
            }
            RecvInner::Tcp { receiver, file } => {
                let result = if let Some((shared, slot)) = file.take() {
                    shared.stop_file(slot, code)
                } else {
                    Ok(())
                };
                receiver.close();
                while receiver.try_recv().is_ok() {}
                self.buffered.clear();
                self.eof = true;
                result?;
            }
        }
        self.buffered.clear();
        self.eof = true;
        Ok(())
    }

    fn from_quic(recv: quinn::RecvStream) -> Self {
        Self {
            inner: RecvInner::Quic(recv),
            buffered: BytesMut::new(),
            eof: false,
        }
    }
    pub(crate) fn from_tcp(
        receiver: mpsc::Receiver<Result<Bytes, ConnError>>,
        file: Option<(Arc<TcpShared>, u32)>,
    ) -> Self {
        Self {
            inner: RecvInner::Tcp { receiver, file },
            buffered: BytesMut::new(),
            eof: false,
        }
    }
    async fn fill(&mut self) -> Result<(), ConnError> {
        let next = match &mut self.inner {
            RecvInner::Quic(recv) => recv.read_chunk(64 << 10, true).await?.map(|c| c.bytes),
            RecvInner::Tcp { receiver, .. } => receiver.recv().await.transpose()?,
        };
        match next {
            Some(next) => self.buffered.extend_from_slice(&next),
            None => self.eof = true,
        }
        Ok(())
    }
    pub async fn recv_record(&mut self) -> Result<Option<Bytes>, ConnError> {
        self.recv_with_limit(MAX_STREAM_RECORD).await
    }
    async fn recv_with_limit(&mut self, limit: usize) -> Result<Option<Bytes>, ConnError> {
        loop {
            if self.buffered.len() >= 4 {
                let len = u32::from_be_bytes([
                    self.buffered[0],
                    self.buffered[1],
                    self.buffered[2],
                    self.buffered[3],
                ]) as usize;
                if len > limit {
                    return Err(ConnError::Protocol("frame exceeds channel limit".into()));
                }
                if self.buffered.len() >= 4 + len {
                    self.buffered.advance(4);
                    return Ok(Some(self.buffered.split_to(len).freeze()));
                }
            }
            if self.eof {
                return if self.buffered.is_empty() {
                    Ok(None)
                } else {
                    Err(ConnError::Protocol("truncated stream record".into()))
                };
            }
            self.fill().await?;
        }
    }
    pub async fn read_raw(&mut self, buf: &mut [u8]) -> Result<Option<usize>, ConnError> {
        if buf.is_empty() {
            return Ok(Some(0));
        }
        while self.buffered.is_empty() && !self.eof {
            self.fill().await?;
        }
        if self.buffered.is_empty() {
            return Ok(None);
        }
        let count = buf.len().min(self.buffered.len());
        buf[..count].copy_from_slice(&self.buffered.split_to(count));
        Ok(Some(count))
    }
}
