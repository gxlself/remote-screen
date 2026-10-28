//! TCP record codec, pure multiplexing state, and TLS fallback transport.

use std::collections::{BTreeMap, HashMap, HashSet};
use std::net::SocketAddr;
use std::sync::{
    atomic::{AtomicBool, AtomicU32, AtomicU64, Ordering},
    Arc, Mutex, Weak,
};
use std::time::{Duration, Instant};

use bytes::{Bytes, BytesMut};
use prost::Message;
use rustls::{
    pki_types::{CertificateDer, ServerName},
    ClientConfig, ServerConfig,
};
use tokio::io::{AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt};
use tokio::net::{TcpListener as TokioTcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, watch, Mutex as AsyncMutex, Notify};
use tokio_rustls::{TlsAcceptor, TlsConnector, TlsStream};

use crate::conn::{framed, CloseReason, Conn, ConnError, ConnInner, StreamReservation, UniRecv};
use crate::mux::{self, DatagramKind, StreamKind};
use crate::net::{system_net, NetDialer, NetStream};

pub const MAX_CONTROL_RECORD: usize = 1 << 20;
pub const MAX_DATA_RECORD: usize = 16 << 20;
// Buffer over one second of 10 ms audio packets plus 60 fps video, with
// headroom for scheduling stalls, without ever blocking control records.
const DATAGRAM_RECV_CAPACITY: usize = 256;
const MAX_VIRTUAL_STREAMS: usize = 64;
const CLIPBOARD_CHUNK: usize = 64 << 10;

fn lock<T>(value: &Mutex<T>) -> std::sync::MutexGuard<'_, T> {
    value.lock().unwrap_or_else(|e| e.into_inner())
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct TcpRecordHeader {
    pub channel: u8,
    pub length: u32,
}
impl TcpRecordHeader {
    pub fn decode(input: &[u8]) -> Result<Self, ConnError> {
        if input.len() < 5 {
            return Err(ConnError::Protocol("truncated TCP header".into()));
        }
        let header = Self {
            channel: input[0],
            length: u32::from_be_bytes([input[1], input[2], input[3], input[4]]),
        };
        validate_length(header.channel, header.length as usize)?;
        Ok(header)
    }
    pub fn encode(self) -> Result<[u8; 5], ConnError> {
        validate_length(self.channel, self.length as usize)?;
        let mut header = [0u8; 5];
        header[0] = self.channel;
        header[1..].copy_from_slice(&self.length.to_be_bytes());
        Ok(header)
    }
}

fn validate_length(channel: u8, length: usize) -> Result<(), ConnError> {
    let limit = match channel {
        mux::CONTROL_CHANNEL => MAX_CONTROL_RECORD,
        mux::FILE_CHANNEL => 5 + mux::FILE_TCP_CHUNK,
        _ => MAX_DATA_RECORD,
    };
    if length > limit {
        Err(ConnError::Protocol(
            "TCP record exceeds channel limit".into(),
        ))
    } else {
        Ok(())
    }
}

pub fn encode_tcp_record(channel: u8, payload: &[u8]) -> Result<Bytes, ConnError> {
    validate_length(channel, payload.len())?;
    let mut out = BytesMut::with_capacity(5 + payload.len());
    out.extend_from_slice(
        &TcpRecordHeader {
            channel,
            length: payload.len() as u32,
        }
        .encode()?,
    );
    out.extend_from_slice(payload);
    Ok(out.freeze())
}

/// Incremental codec. Oversize lengths fail before the payload is allocated.
#[derive(Debug, Default)]
pub struct TcpRecordDecoder {
    header: [u8; 5],
    header_len: usize,
    payload: BytesMut,
    expected: Option<usize>,
    failed: bool,
}
impl TcpRecordDecoder {
    pub fn push(&mut self, mut input: &[u8]) -> Result<Vec<(u8, Bytes)>, ConnError> {
        if self.failed {
            return Err(ConnError::Protocol("TCP decoder is poisoned".into()));
        }
        let mut out = Vec::new();
        while !input.is_empty() {
            if self.expected.is_none() {
                let n = (5 - self.header_len).min(input.len());
                self.header[self.header_len..self.header_len + n].copy_from_slice(&input[..n]);
                self.header_len += n;
                input = &input[n..];
                if self.header_len != 5 {
                    continue;
                }
                match TcpRecordHeader::decode(&self.header) {
                    Ok(header) => self.expected = Some(header.length as usize),
                    Err(error) => {
                        self.failed = true;
                        return Err(error);
                    }
                }
            }
            if let Some(expected) = self.expected {
                let n = (expected - self.payload.len()).min(input.len());
                self.payload.extend_from_slice(&input[..n]);
                input = &input[n..];
                if self.payload.len() == expected {
                    out.push((self.header[0], self.payload.split().freeze()));
                    self.header_len = 0;
                    self.expected = None;
                }
            }
        }
        Ok(out)
    }
    pub fn finish(&self) -> Result<(), ConnError> {
        if self.failed || self.header_len != 0 || self.expected.is_some() {
            Err(ConnError::Protocol(
                "truncated or invalid TCP record".into(),
            ))
        } else {
            Ok(())
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq, Hash)]
pub struct VirtualStreamId {
    pub kind: StreamKind,
    pub item_id: u32,
}

#[derive(Debug, PartialEq, Eq)]
pub enum MuxEvent {
    Control(Bytes),
    Datagram(u8, Bytes),
    Open(VirtualStreamId),
    Data(VirtualStreamId, Bytes),
    Fin(VirtualStreamId),
    Reset(VirtualStreamId, u32),
    Stop(u32, u32),
    Ignore,
}

/// Pure TCP channel state: clipboard/file streams end on FIN, cursor lasts
/// for the TLS connection. Input records are only valid on an attached socket.
#[derive(Debug, Default)]
pub struct TcpMuxState {
    streams: HashSet<VirtualStreamId>,
    // Inclusive ranges keep tombstones compact even when an early file stays
    // open while many later files finish or are locally stopped.
    retired_files: BTreeMap<u32, u32>,
}
impl TcpMuxState {
    fn retire_file(&mut self, slot: u32) {
        self.streams.remove(&VirtualStreamId {
            kind: StreamKind::File,
            item_id: slot,
        });
        let mut start = slot;
        let mut end = slot;
        if let Some((&previous_start, &previous_end)) =
            self.retired_files.range(..=slot).next_back()
        {
            if slot <= previous_end {
                return;
            }
            if previous_end.checked_add(1) == Some(slot) {
                start = previous_start;
                self.retired_files.remove(&previous_start);
            }
        }
        if let Some((&next_start, &next_end)) = self.retired_files.range(slot..).next() {
            if slot.checked_add(1) == Some(next_start) {
                end = next_end;
                self.retired_files.remove(&next_start);
            }
        }
        self.retired_files.insert(start, end);
    }

    pub fn receive(&mut self, channel: u8, payload: Bytes) -> Result<Vec<MuxEvent>, ConnError> {
        validate_length(channel, payload.len())?;
        if channel == mux::CONTROL_CHANNEL {
            return Ok(vec![MuxEvent::Control(payload)]);
        }
        if channel >= mux::VIDEO_CHANNEL {
            return Ok(vec![MuxEvent::Datagram(channel, payload)]);
        }
        let mut reset = None;
        let (id, data, last) = match channel {
            mux::CURSOR_CHANNEL => (
                VirtualStreamId {
                    kind: StreamKind::Cursor,
                    item_id: 0,
                },
                framed(&payload),
                false,
            ),
            mux::CLIPBOARD_CHANNEL => {
                if payload.len() < 5 {
                    return Err(ConnError::Protocol("truncated clipboard chunk".into()));
                }
                if payload[4] & !1 != 0 {
                    return Err(ConnError::Protocol("invalid clipboard flags".into()));
                }
                let item_id = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                (
                    VirtualStreamId {
                        kind: StreamKind::Clipboard,
                        item_id,
                    },
                    payload.slice(5..),
                    payload[4] & 1 != 0,
                )
            }
            mux::FILE_CHANNEL => {
                if payload.len() < 5 {
                    return Err(ConnError::Protocol("truncated file chunk".into()));
                }
                let slot = u32::from_be_bytes([payload[0], payload[1], payload[2], payload[3]]);
                if slot == 0 {
                    return Err(ConnError::Protocol("invalid file slot".into()));
                }
                let flags = payload[4];
                let data = payload.slice(5..);
                match flags {
                    0 => {}
                    mux::FILE_FLAG_FIN if data.is_empty() => {}
                    mux::FILE_FLAG_RESET | mux::FILE_FLAG_STOP if data.len() == 4 => {}
                    _ => return Err(ConnError::Protocol("invalid file flags or payload".into())),
                }
                if flags == mux::FILE_FLAG_STOP {
                    let code = u32::from_be_bytes([data[0], data[1], data[2], data[3]]);
                    return Ok(vec![MuxEvent::Stop(slot, code)]);
                }
                if self
                    .retired_files
                    .range(..=slot)
                    .next_back()
                    .is_some_and(|(_, &end)| slot <= end)
                {
                    return Ok(vec![MuxEvent::Ignore]);
                }
                if flags == mux::FILE_FLAG_RESET {
                    reset = Some(u32::from_be_bytes([data[0], data[1], data[2], data[3]]));
                }
                (
                    VirtualStreamId {
                        kind: StreamKind::File,
                        item_id: slot,
                    },
                    if reset.is_some() { Bytes::new() } else { data },
                    flags == mux::FILE_FLAG_FIN || reset.is_some(),
                )
            }
            mux::INPUT_CHANNEL => {
                return Err(ConnError::Protocol(
                    "input requires a dedicated TLS connection".into(),
                ))
            }
            _ => return Ok(vec![MuxEvent::Ignore]),
        };
        let mut events = Vec::new();
        if !self.streams.contains(&id) {
            if self.streams.len() >= MAX_VIRTUAL_STREAMS {
                return Err(ConnError::Protocol("too many virtual TCP streams".into()));
            }
            self.streams.insert(id);
            events.push(MuxEvent::Open(id));
        }
        if !data.is_empty() {
            events.push(MuxEvent::Data(id, data));
        }
        if last {
            if id.kind == StreamKind::File {
                self.retire_file(id.item_id);
            } else {
                self.streams.remove(&id);
            }
            events.push(match reset {
                Some(code) => MuxEvent::Reset(id, code),
                None => MuxEvent::Fin(id),
            });
        }
        Ok(events)
    }
}

type KeyframeCallback = Arc<dyn Fn() + Send + Sync>;
type IncomingStream = (
    StreamKind,
    mpsc::Receiver<Result<Bytes, ConnError>>,
    Option<u32>,
);
type Record = (u8, Bytes);

enum WriterMessage {
    Record(Record),
    Flush(oneshot::Sender<()>),
}

struct WriterQueue {
    video: Mutex<Option<Bytes>>,
    ready: Notify,
    callback: Mutex<Option<KeyframeCallback>>,
    last_request: Mutex<Option<Instant>>,
    dropped: AtomicU64,
}
impl WriterQueue {
    fn video(&self, payload: Bytes) {
        let replaced = lock(&self.video).replace(payload).is_some();
        self.ready.notify_one();
        if !replaced {
            return;
        }
        self.dropped.fetch_add(1, Ordering::Relaxed);
        let now = Instant::now();
        let notify = {
            let mut last = lock(&self.last_request);
            if last.is_none_or(|t| now.duration_since(t) >= Duration::from_millis(500)) {
                *last = Some(now);
                true
            } else {
                false
            }
        };
        if notify {
            let callback = lock(&self.callback).clone();
            if let Some(callback) = callback {
                callback();
            }
        }
    }
}

pub(crate) struct TcpShared {
    id: u64,
    remote: SocketAddr,
    peer_certs: Vec<CertificateDer<'static>>,
    client_config: Option<(Arc<ClientConfig>, Arc<dyn NetDialer>)>,
    session_token: Mutex<Option<Vec<u8>>>,
    writer: mpsc::Sender<WriterMessage>,
    queue: Arc<WriterQueue>,
    control_tx: mpsc::Sender<Bytes>,
    control_rx: AsyncMutex<mpsc::Receiver<Bytes>>,
    datagram_tx: mpsc::Sender<(DatagramKind, Bytes)>,
    datagram_rx: AsyncMutex<mpsc::Receiver<(DatagramKind, Bytes)>>,
    uni_tx: mpsc::Sender<IncomingStream>,
    uni_rx: AsyncMutex<mpsc::Receiver<IncomingStream>>,
    closed: watch::Sender<Option<CloseReason>>,
    control_taken: AtomicBool,
    cursor_taken: AtomicBool,
    input_taken: AtomicBool,
    next_item: AtomicU32,
    next_file: AtomicU32,
    // Only live local send handles are tracked. A STOP for any other slot is
    // ignored; finish/reset/drop remove the entry, including its stopped bit.
    sending_files: Mutex<HashMap<u32, bool>>,
    mux: Mutex<TcpMuxState>,
    unknown_datagrams: AtomicU64,
    dropped_datagrams: AtomicU64,
}

fn next_clipboard_item_id(counter: &AtomicU32) -> Result<u32, ConnError> {
    counter
        .try_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
        .map_err(|_| ConnError::Protocol("clipboard item IDs exhausted".into()))
}

impl TcpShared {
    fn start<IO: AsyncRead + AsyncWrite + Send + Unpin + 'static>(
        tls: TlsStream<IO>,
        remote: SocketAddr,
        client_config: Option<(Arc<ClientConfig>, Arc<dyn NetDialer>)>,
        first: Option<Record>,
    ) -> Arc<Self> {
        static NEXT_ID: AtomicU64 = AtomicU64::new(1);
        let certs = tls
            .get_ref()
            .1
            .peer_certificates()
            .unwrap_or_default()
            .to_vec();
        let (read, write) = tokio::io::split(tls);
        let (writer, records) = mpsc::channel(16);
        let (control_tx, control_rx) = mpsc::channel(8);
        let (datagram_tx, datagram_rx) = mpsc::channel(DATAGRAM_RECV_CAPACITY);
        let (uni_tx, uni_rx) = mpsc::channel(64);
        let (closed, shutdown) = watch::channel(None);
        let queue = Arc::new(WriterQueue {
            video: Mutex::new(None),
            ready: Notify::new(),
            callback: Mutex::new(None),
            last_request: Mutex::new(None),
            dropped: AtomicU64::new(0),
        });
        let shared = Arc::new(Self {
            id: NEXT_ID.fetch_add(1, Ordering::Relaxed),
            remote,
            peer_certs: certs,
            client_config,
            session_token: Mutex::new(None),
            writer,
            queue: queue.clone(),
            control_tx,
            control_rx: AsyncMutex::new(control_rx),
            datagram_tx,
            datagram_rx: AsyncMutex::new(datagram_rx),
            uni_tx,
            uni_rx: AsyncMutex::new(uni_rx),
            closed,
            control_taken: AtomicBool::new(false),
            cursor_taken: AtomicBool::new(false),
            input_taken: AtomicBool::new(false),
            next_item: AtomicU32::new(0),
            next_file: AtomicU32::new(1),
            sending_files: Mutex::new(HashMap::new()),
            mux: Mutex::new(TcpMuxState::default()),
            unknown_datagrams: AtomicU64::new(0),
            dropped_datagrams: AtomicU64::new(0),
        });
        let weak = Arc::downgrade(&shared);
        tokio::spawn(writer_loop(
            write,
            records,
            queue,
            shutdown.clone(),
            weak.clone(),
        ));
        tokio::spawn(reader_loop(read, first, weak, shutdown));
        shared
    }
    pub(crate) fn remote_addr(&self) -> SocketAddr {
        self.remote
    }
    pub(crate) fn stable_id(&self) -> u64 {
        self.id
    }

    pub(crate) async fn flush_control(&self) -> Result<(), ConnError> {
        let (tx, rx) = oneshot::channel();
        tokio::select! {
            biased;
            _ = self.closed() => return Err(ConnError::Closed),
            result = self.writer.send(WriterMessage::Flush(tx)) => result.map_err(|_| ConnError::Closed)?,
        }
        tokio::select! {
            biased;
            result = rx => result.map_err(|_| ConnError::Closed),
            _ = self.closed() => Err(ConnError::Closed),
        }
    }
    pub(crate) fn peer_certificates(&self) -> Vec<CertificateDer<'static>> {
        self.peer_certs.clone()
    }
    pub(crate) fn take_control(&self) -> Result<(), ConnError> {
        if self.control_taken.swap(true, Ordering::AcqRel) {
            Err(ConnError::Protocol("control stream already opened".into()))
        } else {
            Ok(())
        }
    }
    pub(crate) fn set_session_token(&self, token: Vec<u8>) -> Result<(), ConnError> {
        validate_token(&token)?;
        *lock(&self.session_token) = Some(token);
        Ok(())
    }
    pub(crate) fn set_keyframe_callback(&self, callback: KeyframeCallback) {
        *lock(&self.queue.callback) = Some(callback);
    }
    pub(crate) fn unknown_datagram_count(&self) -> u64 {
        self.unknown_datagrams.load(Ordering::Relaxed)
    }
    pub(crate) fn dropped_datagram_count(&self) -> u64 {
        self.dropped_datagrams.load(Ordering::Relaxed)
    }
    pub(crate) fn dropped_video_frames(&self) -> u64 {
        self.queue.dropped.load(Ordering::Relaxed)
    }
    pub(crate) fn close(&self, code: Option<u64>, reason: Bytes) {
        self.closed.send_if_modified(|closed| {
            if closed.is_none() {
                *closed = Some(CloseReason { code, reason });
                true
            } else {
                false
            }
        });
    }
    pub(crate) async fn closed(&self) -> CloseReason {
        let mut closed = self.closed.subscribe();
        loop {
            if let Some(reason) = closed.borrow().clone() {
                return reason;
            }
            if closed.changed().await.is_err() {
                return CloseReason {
                    code: None,
                    reason: Bytes::new(),
                };
            }
        }
    }
    async fn send(&self, channel: u8, payload: Bytes) -> Result<(), ConnError> {
        validate_length(channel, payload.len())?;
        if self.closed.borrow().is_some() {
            return Err(ConnError::Closed);
        }
        tokio::select! { biased; _ = self.closed() => Err(ConnError::Closed), result = self.writer.send(WriterMessage::Record((channel, payload))) => result.map_err(|_| ConnError::Closed) }
    }
    pub(crate) async fn send_control(&self, payload: Bytes) -> Result<(), ConnError> {
        self.send(mux::CONTROL_CHANNEL, payload).await
    }
    pub(crate) async fn recv_control(&self) -> Result<Option<Bytes>, ConnError> {
        let mut rx = self.control_rx.lock().await;
        tokio::select! { biased; next = rx.recv() => Ok(next), _ = self.closed() => Ok(None) }
    }
    pub(crate) async fn accept_uni(self: &Arc<Self>) -> Result<(StreamKind, UniRecv), ConnError> {
        let mut rx = self.uni_rx.lock().await;
        let (kind, rx, slot) = tokio::select! { biased; next = rx.recv() => next.ok_or(ConnError::Closed)?, _ = self.closed() => return Err(ConnError::Closed) };
        Ok((
            kind,
            UniRecv::from_tcp(rx, slot.map(|slot| (self.clone(), slot))),
        ))
    }
    pub(crate) fn send_datagram(
        &self,
        kind: DatagramKind,
        payload: Bytes,
    ) -> Result<(), ConnError> {
        validate_length(kind as u8, payload.len())?;
        if self.closed.borrow().is_some() {
            return Err(ConnError::Closed);
        }
        if kind == DatagramKind::Video {
            self.queue.video(payload);
            return Ok(());
        }
        self.writer
            .try_send(WriterMessage::Record((kind as u8, payload)))
            .map_err(|e| match e {
                mpsc::error::TrySendError::Full(_) => ConnError::WouldBlock,
                mpsc::error::TrySendError::Closed(_) => ConnError::Closed,
            })
    }
    pub(crate) async fn recv_datagram(&self) -> Result<(DatagramKind, Bytes), ConnError> {
        let mut rx = self.datagram_rx.lock().await;
        tokio::select! { biased; next = rx.recv() => next.ok_or(ConnError::Closed), _ = self.closed() => Err(ConnError::Closed) }
    }
    pub(crate) async fn open_uni(
        self: &Arc<Self>,
        kind: StreamKind,
    ) -> Result<TcpUniSend, ConnError> {
        if self.closed.borrow().is_some() {
            return Err(ConnError::Closed);
        }
        match kind {
            StreamKind::Input => {
                let (cfg, net) = self
                    .client_config
                    .clone()
                    .ok_or(ConnError::Unsupported("only the client can open TCP input"))?;
                let token = lock(&self.session_token)
                    .clone()
                    .ok_or_else(|| ConnError::Protocol("session token is not set".into()))?;
                let reservation = StreamReservation::take(&self.input_taken)?;
                let result = async {
                    let mut tls = client_tls(net.as_ref(), self.remote, cfg).await?;
                    let attach = rsp_core::pb::Envelope {
                        msg: Some(rsp_core::pb::envelope::Msg::AttachInput(
                            rsp_core::pb::AttachInput {
                                session_token: token,
                            },
                        )),
                    }
                    .encode_to_vec();
                    write_record(&mut tls, mux::CONTROL_CHANNEL, &attach).await?;
                    Ok(TcpUniSend::Input {
                        tls: Box::new(AsyncMutex::new(tls)),
                        parent: self.clone(),
                    })
                }
                .await;
                if result.is_ok() {
                    reservation.commit();
                }
                result
            }
            StreamKind::Cursor => {
                if self.cursor_taken.swap(true, Ordering::AcqRel) {
                    return Err(ConnError::Protocol(
                        "TCP has one persistent cursor stream".into(),
                    ));
                }
                Ok(TcpUniSend::Cursor(self.clone()))
            }
            StreamKind::Clipboard => {
                let item_id = next_clipboard_item_id(&self.next_item)?;
                Ok(TcpUniSend::Clipboard {
                    shared: self.clone(),
                    item_id,
                })
            }
            StreamKind::File => {
                let slot = self
                    .next_file
                    .try_update(Ordering::AcqRel, Ordering::Acquire, |id| id.checked_add(1))
                    .map_err(|_| ConnError::Protocol("file slots exhausted".into()))?;
                lock(&self.sending_files).insert(slot, false);
                Ok(TcpUniSend::File {
                    shared: self.clone(),
                    slot,
                })
            }
        }
    }

    // reset/stop have synchronous public APIs. Keep the bounded writer queue:
    // when it is full, wait for capacity in a task instead of losing a signal.
    fn file_signal(self: &Arc<Self>, slot: u32, flags: u8, code: u32) -> Result<(), ConnError> {
        if self.closed.borrow().is_some() {
            return Err(ConnError::Closed);
        }
        let message = WriterMessage::Record((
            mux::FILE_CHANNEL,
            mux::file_chunk(slot, flags, &code.to_be_bytes()),
        ));
        match self.writer.try_send(message) {
            Ok(()) => Ok(()),
            Err(mpsc::error::TrySendError::Closed(_)) => Err(ConnError::Closed),
            Err(mpsc::error::TrySendError::Full(message)) => {
                let shared = self.clone();
                tokio::spawn(async move {
                    tokio::select! {
                        biased;
                        _ = shared.closed() => {},
                        _ = shared.writer.send(message) => {},
                    }
                });
                Ok(())
            }
        }
    }

    pub(crate) fn stop_file(self: &Arc<Self>, slot: u32, code: u32) -> Result<(), ConnError> {
        lock(&self.mux).retire_file(slot);
        self.file_signal(slot, mux::FILE_FLAG_STOP, code)
    }

    fn check_file(&self, slot: u32) -> Result<(), ConnError> {
        if lock(&self.sending_files).get(&slot) == Some(&false) {
            Ok(())
        } else {
            Err(ConnError::Closed)
        }
    }
}

pub(crate) enum TcpUniSend {
    Input {
        // NetStream is only Send. Preserve UniSend's Sync bound with exclusive
        // get_mut access to the TLS stream; no runtime locking is needed.
        tls: Box<AsyncMutex<tokio_rustls::client::TlsStream<NetStream>>>,
        parent: Arc<TcpShared>,
    },
    Cursor(Arc<TcpShared>),
    Clipboard {
        shared: Arc<TcpShared>,
        item_id: u32,
    },
    File {
        shared: Arc<TcpShared>,
        slot: u32,
    },
}
impl TcpUniSend {
    pub(crate) fn reset(&mut self, code: u32) -> Result<bool, ConnError> {
        let Self::File { shared, slot } = self else {
            return Ok(false);
        };
        if lock(&shared.sending_files).remove(slot).is_some() {
            shared.file_signal(*slot, mux::FILE_FLAG_RESET, code)?;
        }
        Ok(true)
    }
    pub(crate) async fn send_record(&mut self, payload: &[u8]) -> Result<(), ConnError> {
        match self {
            Self::Input { tls, parent } => {
                tokio::select! { _ = parent.closed() => Err(ConnError::Closed), result = write_record(tls.get_mut(), mux::INPUT_CHANNEL, payload) => result }
            }
            Self::Cursor(shared) => {
                shared
                    .send(mux::CURSOR_CHANNEL, Bytes::copy_from_slice(payload))
                    .await
            }
            Self::Clipboard { .. } | Self::File { .. } => self.write_raw(&framed(payload)).await,
        }
    }
    pub(crate) async fn write_raw(&mut self, payload: &[u8]) -> Result<(), ConnError> {
        match self {
            Self::Clipboard { shared, item_id } => {
                for chunk in payload.chunks(CLIPBOARD_CHUNK) {
                    shared
                        .send(
                            mux::CLIPBOARD_CHANNEL,
                            mux::clipboard_chunk(*item_id, false, chunk),
                        )
                        .await?;
                }
                Ok(())
            }
            Self::File { shared, slot } => {
                shared.check_file(*slot)?;
                for chunk in payload.chunks(mux::FILE_TCP_CHUNK) {
                    // Reserve capacity before checking STOP so a blocked write
                    // cannot enqueue another chunk after STOP was processed.
                    let permit = tokio::select! {
                        biased;
                        _ = shared.closed() => return Err(ConnError::Closed),
                        permit = shared.writer.reserve() => permit.map_err(|_| ConnError::Closed)?,
                    };
                    shared.check_file(*slot)?;
                    permit.send(WriterMessage::Record((
                        mux::FILE_CHANNEL,
                        mux::file_chunk(*slot, 0, chunk),
                    )));
                }
                Ok(())
            }
            _ => Err(ConnError::Unsupported(
                "TCP input/cursor carry records, not raw byte chunks",
            )),
        }
    }
    pub(crate) async fn finish(&mut self) -> Result<(), ConnError> {
        match self {
            Self::Input { tls, .. } => {
                tls.get_mut().shutdown().await?;
                Ok(())
            }
            Self::Clipboard { shared, item_id } => {
                shared
                    .send(
                        mux::CLIPBOARD_CHANNEL,
                        mux::clipboard_chunk(*item_id, true, &[]),
                    )
                    .await
            }
            Self::File { shared, slot } => {
                shared.check_file(*slot)?;
                shared
                    .send(
                        mux::FILE_CHANNEL,
                        mux::file_chunk(*slot, mux::FILE_FLAG_FIN, &[]),
                    )
                    .await?;
                lock(&shared.sending_files).remove(slot);
                Ok(())
            }
            Self::Cursor(_) => Err(ConnError::Unsupported(
                "TCP cursor FIN is not defined; cursor lasts for the connection",
            )),
        }
    }
}

impl Drop for TcpUniSend {
    fn drop(&mut self) {
        // Abandoning a file must release the peer's route rather than leave it
        // waiting forever for FIN, and discard local STOP bookkeeping.
        if let Self::File { shared, slot } = self {
            if lock(&shared.sending_files).remove(slot).is_some() {
                let _ = shared.file_signal(*slot, mux::FILE_FLAG_RESET, 0);
            }
        }
    }
}

async fn read_record<R: AsyncRead + Unpin>(read: &mut R) -> Result<Option<Record>, ConnError> {
    let mut header = [0u8; 5];
    if read.read(&mut header[..1]).await? == 0 {
        return Ok(None);
    }
    read.read_exact(&mut header[1..])
        .await
        .map_err(|_| ConnError::Protocol("truncated TCP header".into()))?;
    let header = TcpRecordHeader::decode(&header)?;
    let mut payload = vec![0u8; header.length as usize];
    read.read_exact(&mut payload)
        .await
        .map_err(|_| ConnError::Protocol("truncated TCP payload".into()))?;
    Ok(Some((header.channel, Bytes::from(payload))))
}
async fn write_record<W: AsyncWrite + Unpin>(
    write: &mut W,
    channel: u8,
    payload: &[u8],
) -> Result<(), ConnError> {
    validate_length(channel, payload.len())?;
    let header = TcpRecordHeader {
        channel,
        length: payload.len() as u32,
    }
    .encode()?;
    write.write_all(&header).await?;
    write.write_all(payload).await?;
    write.flush().await?;
    Ok(())
}

async fn shutdown(mut closed: watch::Receiver<Option<CloseReason>>) {
    loop {
        if closed.borrow().is_some() {
            return;
        }
        if closed.changed().await.is_err() {
            return;
        }
    }
}
async fn writer_loop<W: AsyncWrite + Unpin>(
    mut write: W,
    mut records: mpsc::Receiver<WriterMessage>,
    queue: Arc<WriterQueue>,
    closed: watch::Receiver<Option<CloseReason>>,
    shared: Weak<TcpShared>,
) {
    let run = async {
        loop {
            let (channel, payload) = tokio::select! {
                biased;
                record = records.recv() => match record {
                    Some(WriterMessage::Record(record)) => record,
                    Some(WriterMessage::Flush(done)) => {
                        write.flush().await?;
                        let _ = done.send(());
                        continue;
                    }
                    None => return Ok::<(),ConnError>(()),
                },
                _ = queue.ready.notified() => { let frame = lock(&queue.video).take(); if let Some(frame) = frame { (mux::VIDEO_CHANNEL, frame) } else { continue; } }
            };
            write_record(&mut write, channel, &payload).await?;
        }
    };
    let result = tokio::select! { _ = shutdown(closed) => Ok(()), result = run => result };
    if let Some(shared) = shared.upgrade() {
        shared.close(
            None,
            result
                .err()
                .map(|e| Bytes::from(e.to_string()))
                .unwrap_or_default(),
        );
    }
    let _ = tokio::time::timeout(Duration::from_secs(1), write.shutdown()).await;
}

async fn reader_loop<R: AsyncRead + Unpin>(
    mut read: R,
    mut first: Option<Record>,
    shared: Weak<TcpShared>,
    closed: watch::Receiver<Option<CloseReason>>,
) {
    let run = async {
        let mut last_datagram_drop_log = None;
        let mut routes: HashMap<VirtualStreamId, mpsc::Sender<Result<Bytes, ConnError>>> =
            HashMap::new();
        loop {
            let next = if let Some(first) = first.take() {
                Some(first)
            } else {
                read_record(&mut read).await?
            };
            let Some((channel, payload)) = next else {
                return Ok::<(), ConnError>(());
            };
            let Some(shared) = shared.upgrade() else {
                return Ok(());
            };
            routes.retain(|_, route| !route.is_closed());
            let events = lock(&shared.mux).receive(channel, payload)?;
            for event in events {
                match event {
                    MuxEvent::Control(data) => {
                        shared
                            .control_tx
                            .send(data)
                            .await
                            .map_err(|_| ConnError::Closed)?;
                    }
                    MuxEvent::Datagram(channel, data) => match DatagramKind::try_from(channel) {
                        Ok(kind) => match shared.datagram_tx.try_send((kind, data)) {
                            Ok(()) => {}
                            Err(mpsc::error::TrySendError::Full(_)) => {
                                let dropped = shared
                                    .dropped_datagrams
                                    .fetch_add(1, Ordering::Relaxed)
                                    .saturating_add(1);
                                let now = Instant::now();
                                if last_datagram_drop_log.is_none_or(|last| {
                                    now.duration_since(last) >= Duration::from_secs(1)
                                }) {
                                    tracing::debug!(
                                        ?kind,
                                        dropped,
                                        capacity = DATAGRAM_RECV_CAPACITY,
                                        "TCP datagram receive queue full"
                                    );
                                    last_datagram_drop_log = Some(now);
                                }
                            }
                            Err(mpsc::error::TrySendError::Closed(_)) => {
                                return Err(ConnError::Closed);
                            }
                        },
                        Err(()) => {
                            shared.unknown_datagrams.fetch_add(1, Ordering::Relaxed);
                        }
                    },
                    MuxEvent::Open(id) => {
                        let (tx, rx) = mpsc::channel(4);
                        routes.insert(id, tx);
                        shared
                            .uni_tx
                            .send((
                                id.kind,
                                rx,
                                (id.kind == StreamKind::File).then_some(id.item_id),
                            ))
                            .await
                            .map_err(|_| ConnError::Closed)?;
                    }
                    MuxEvent::Data(id, data) => {
                        if let Some(route) = routes.get(&id) {
                            // Small owned chunks keep many stalled clipboard
                            // streams from each retaining whole 16 MiB records.
                            for chunk in data.chunks(CLIPBOARD_CHUNK) {
                                if route.send(Ok(Bytes::copy_from_slice(chunk))).await.is_err() {
                                    break;
                                }
                            }
                        }
                    }
                    MuxEvent::Fin(id) => {
                        routes.remove(&id);
                    }
                    MuxEvent::Reset(id, code) => {
                        if let Some(route) = routes.remove(&id) {
                            let _ = route
                                .send(Err(ConnError::Protocol(format!(
                                    "file stream reset ({code})"
                                ))))
                                .await;
                        }
                    }
                    MuxEvent::Stop(slot, _code) => {
                        if let Some(stopped) = lock(&shared.sending_files).get_mut(&slot) {
                            *stopped = true;
                        }
                    }
                    MuxEvent::Ignore => {}
                }
            }
        }
    };
    let result = tokio::select! { _ = shutdown(closed) => Ok(()), result = run => result };
    if let Some(shared) = shared.upgrade() {
        let code = if matches!(result, Err(ConnError::Protocol(_))) {
            Some(1)
        } else {
            None
        };
        shared.close(
            code,
            result
                .err()
                .map(|e| Bytes::from(e.to_string()))
                .unwrap_or_default(),
        );
    }
}

#[derive(Default)]
struct TcpRegistry {
    sessions: Mutex<HashMap<Vec<u8>, Weak<TcpShared>>>,
}
fn validate_token(token: &[u8]) -> Result<(), ConnError> {
    if token.len() == 16 {
        Ok(())
    } else {
        Err(ConnError::Protocol("session token must be 16 bytes".into()))
    }
}

pub struct TcpListener {
    inner: TokioTcpListener,
    acceptor: TlsAcceptor,
    registry: Arc<TcpRegistry>,
}
pub struct TcpIncoming {
    stream: TcpStream,
    remote: SocketAddr,
    acceptor: TlsAcceptor,
    registry: Arc<TcpRegistry>,
}
impl TcpListener {
    pub async fn bind(addr: SocketAddr, server_cfg: ServerConfig) -> Result<Self, ConnError> {
        check_alpn(&server_cfg.alpn_protocols)?;
        Ok(Self {
            inner: TokioTcpListener::bind(addr).await?,
            acceptor: TlsAcceptor::from(Arc::new(server_cfg)),
            registry: Arc::new(TcpRegistry::default()),
        })
    }
    pub fn local_addr(&self) -> Result<SocketAddr, ConnError> {
        Ok(self.inner.local_addr()?)
    }
    pub async fn accept(&self) -> Result<TcpIncoming, ConnError> {
        let (stream, remote) = self.inner.accept().await?;
        Ok(TcpIncoming {
            stream,
            remote,
            acceptor: self.acceptor.clone(),
            registry: self.registry.clone(),
        })
    }
    /// Clone the input-token table without retaining the listening socket.
    pub fn token_registry(&self) -> TcpTokenRegistry {
        TcpTokenRegistry {
            registry: self.registry.clone(),
        }
    }
    pub fn register_token(&self, token: Vec<u8>, conn: &Conn) -> Result<(), ConnError> {
        self.token_registry().register_token(token, conn)
    }
    pub fn unregister_token(&self, token: &[u8]) {
        self.token_registry().unregister_token(token);
    }
}

/// Input attachments remain scoped to the listener that accepted the parent.
#[derive(Clone)]
pub struct TcpTokenRegistry {
    registry: Arc<TcpRegistry>,
}
impl TcpTokenRegistry {
    pub fn register_token(&self, token: Vec<u8>, conn: &Conn) -> Result<(), ConnError> {
        validate_token(&token)?;
        let ConnInner::Tcp(shared) = &conn.inner else {
            return Err(ConnError::Unsupported("token registration requires TCP"));
        };
        if shared.closed.borrow().is_some() {
            return Err(ConnError::Closed);
        }
        let mut sessions = lock(&self.registry.sessions);
        sessions.retain(|_, s| s.upgrade().is_some_and(|s| s.closed.borrow().is_none()));
        if sessions
            .get(&token)
            .and_then(Weak::upgrade)
            .is_some_and(|existing| !Arc::ptr_eq(&existing, shared))
        {
            return Err(ConnError::Protocol(
                "session token already registered".into(),
            ));
        }
        sessions.insert(token, Arc::downgrade(shared));
        Ok(())
    }
    pub fn unregister_token(&self, token: &[u8]) {
        lock(&self.registry.sessions).remove(token);
    }
}
impl TcpIncoming {
    pub fn remote_addr(&self) -> SocketAddr {
        self.remote
    }
    pub fn ignore(self) {
        drop(self.stream);
    }
    /// Waits for the first record after TLS. Main connections return Some;
    /// authenticated input attachments are routed to their parent and return None.
    pub async fn accept(self) -> Result<Option<Conn>, ConnError> {
        let mut tls =
            tokio::time::timeout(Duration::from_secs(10), self.acceptor.accept(self.stream))
                .await
                .map_err(|_| ConnError::Tls("TLS handshake timed out".into()))?
                .map_err(|e| ConnError::Tls(e.to_string()))?;
        verify_tls(tls.get_ref().1)?;
        let first = tokio::time::timeout(Duration::from_secs(60), read_record(&mut tls))
            .await
            .map_err(|_| ConnError::Protocol("first TCP record timed out".into()))??
            .ok_or(ConnError::Closed)?;
        let attach = if first.0 == mux::CONTROL_CHANNEL {
            rsp_core::pb::Envelope::decode(first.1.clone())
                .ok()
                .and_then(|envelope| match envelope.msg {
                    Some(rsp_core::pb::envelope::Msg::AttachInput(attach)) => {
                        Some(attach.session_token)
                    }
                    _ => None,
                })
        } else {
            None
        };
        if let Some(token) = attach {
            validate_token(&token)?;
            let target = lock(&self.registry.sessions)
                .get(&token)
                .and_then(Weak::upgrade)
                .ok_or_else(|| ConnError::Protocol("unknown input session token".into()))?;
            let certs = tls.get_ref().1.peer_certificates().unwrap_or_default();
            if target.peer_certs != certs {
                return Err(ConnError::Protocol(
                    "input certificate does not match the session".into(),
                ));
            }
            if target.closed.borrow().is_some() {
                return Err(ConnError::Closed);
            }
            let reservation = StreamReservation::take(&target.input_taken)?;
            let (tx, rx) = mpsc::channel(8);
            target
                .uni_tx
                .send((StreamKind::Input, rx, None))
                .await
                .map_err(|_| ConnError::Closed)?;
            tokio::spawn(input_reader(
                tls,
                tx,
                Arc::downgrade(&target),
                target.closed.subscribe(),
            ));
            reservation.commit();
            return Ok(None);
        }
        Ok(Some(Conn::from_tcp(TcpShared::start(
            TlsStream::Server(tls),
            self.remote,
            None,
            Some(first),
        ))))
    }
}

async fn input_reader<R: AsyncRead + Unpin>(
    mut read: R,
    sender: mpsc::Sender<Result<Bytes, ConnError>>,
    shared: Weak<TcpShared>,
    closed: watch::Receiver<Option<CloseReason>>,
) {
    let run = async {
        while let Some((channel, payload)) = read_record(&mut read).await? {
            if channel != mux::INPUT_CHANNEL {
                return Err(ConnError::Protocol(
                    "non-input record on input connection".into(),
                ));
            }
            for chunk in framed(&payload).chunks(CLIPBOARD_CHUNK) {
                sender
                    .send(Ok(Bytes::copy_from_slice(chunk)))
                    .await
                    .map_err(|_| ConnError::Closed)?;
            }
        }
        Ok::<(), ConnError>(())
    };
    let result = tokio::select! { _ = shutdown(closed) => Ok(()), result = run => result };
    if let Some(shared) = shared.upgrade() {
        shared.input_taken.store(false, Ordering::Release);
        if matches!(result, Err(ConnError::Protocol(_))) {
            shared.close(Some(1), Bytes::from_static(b"invalid input record"));
        }
    }
}

fn check_alpn(alpn: &[Vec<u8>]) -> Result<(), ConnError> {
    if alpn.iter().any(|p| p.as_slice() == b"rsp/1") {
        Ok(())
    } else {
        Err(ConnError::MissingAlpn)
    }
}
fn verify_tls(tls: &rustls::CommonState) -> Result<(), ConnError> {
    if tls.alpn_protocol() != Some(b"rsp/1") {
        return Err(ConnError::MissingAlpn);
    }
    if tls.protocol_version() != Some(rustls::ProtocolVersion::TLSv1_3) {
        return Err(ConnError::Tls("RSP requires TLS 1.3".into()));
    }
    Ok(())
}
async fn client_tls(
    net: &dyn NetDialer,
    addr: SocketAddr,
    cfg: Arc<ClientConfig>,
) -> Result<tokio_rustls::client::TlsStream<NetStream>, ConnError> {
    check_alpn(&cfg.alpn_protocols)?;
    let socket = net.dial_tcp(addr).await?;
    let name = ServerName::try_from("rs-host")
        .map_err(|_| ConnError::Tls("invalid server name".into()))?;
    let tls = tokio::time::timeout(
        Duration::from_secs(10),
        TlsConnector::from(cfg).connect(name, socket),
    )
    .await
    .map_err(|_| ConnError::Tls("TLS handshake timed out".into()))?
    .map_err(|e| ConnError::Tls(e.to_string()))?;
    verify_tls(tls.get_ref().1)?;
    Ok(tls)
}
pub async fn connect_tcp(addr: SocketAddr, cfg: Arc<ClientConfig>) -> Result<Conn, ConnError> {
    connect_tcp_via(system_net(), addr, cfg).await
}

pub async fn connect_tcp_via(
    net: Arc<dyn NetDialer>,
    addr: SocketAddr,
    cfg: Arc<ClientConfig>,
) -> Result<Conn, ConnError> {
    let tls = client_tls(net.as_ref(), addr, cfg.clone()).await?;
    Ok(Conn::from_tcp(TcpShared::start(
        TlsStream::Client(tls),
        addr,
        Some((cfg, net)),
        None,
    )))
}

#[cfg(test)]
mod tests {
    use std::collections::HashSet;

    use super::*;

    #[test]
    fn retired_file_ranges_merge_without_closing_live_gaps() {
        let mut mux = TcpMuxState::default();
        for slot in [3, 1, 5, 2, 4, u32::MAX, u32::MAX - 1] {
            mux.retire_file(slot);
        }
        assert_eq!(
            mux.retired_files,
            BTreeMap::from([(1, 5), (u32::MAX - 1, u32::MAX)])
        );
        let events = mux
            .receive(mux::FILE_CHANNEL, mux::file_chunk(6, 0, b"live"))
            .unwrap();
        assert!(matches!(events[0], MuxEvent::Open(_)));
        for slot in [1, 3, 5, u32::MAX] {
            assert_eq!(
                mux.receive(mux::FILE_CHANNEL, mux::file_chunk(slot, 0, b"late"))
                    .unwrap(),
                vec![MuxEvent::Ignore]
            );
        }
    }

    #[test]
    fn clipboard_item_ids_start_at_zero_and_increment() {
        let counter = AtomicU32::new(0);

        assert_eq!(next_clipboard_item_id(&counter).unwrap(), 0);
        assert_eq!(next_clipboard_item_id(&counter).unwrap(), 1);
        assert_eq!(next_clipboard_item_id(&counter).unwrap(), 2);
        assert_eq!(counter.load(Ordering::Acquire), 3);
    }

    #[test]
    fn clipboard_item_ids_report_exhaustion_without_changing_counter() {
        let counter = AtomicU32::new(u32::MAX - 1);

        assert_eq!(next_clipboard_item_id(&counter).unwrap(), u32::MAX - 1);
        match next_clipboard_item_id(&counter) {
            Err(ConnError::Protocol(message)) => {
                assert_eq!(message, "clipboard item IDs exhausted")
            }
            other => panic!("expected protocol error, got {other:?}"),
        }
        assert_eq!(counter.load(Ordering::Acquire), u32::MAX);
    }

    #[test]
    fn clipboard_item_ids_are_unique_across_threads() {
        let counter = AtomicU32::new(0);
        let ids = std::thread::scope(|scope| {
            let handles: Vec<_> = (0..8)
                .map(|_| {
                    scope.spawn(|| {
                        (0..1000)
                            .map(|_| next_clipboard_item_id(&counter).unwrap())
                            .collect::<Vec<_>>()
                    })
                })
                .collect();
            handles
                .into_iter()
                .flat_map(|handle| handle.join().unwrap())
                .collect::<Vec<_>>()
        });

        assert_eq!(ids.len(), 8000);
        let unique = ids.into_iter().collect::<HashSet<_>>();
        assert_eq!(unique.len(), 8000);
    }
}
