//! Fake RDP server for tests.
//!
//! Binds only to loopback, uses a fresh self-signed certificate, and keeps display
//! updates queued until a client activates. IronRDP is not Send, so each server
//! runs on a dedicated Tokio LocalSet thread. Dropping the handle stops it.
//!
//! A transport adapter works around two rev-2bc793ef issues: the acceptor's
//! discarded negotiation-failure response and empty hidden/default pointer
//! updates. Authentication, display encoding, and input still use IronRDP.

use std::future::Future as _;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::num::{NonZeroU16, NonZeroUsize};
use std::pin::Pin;
use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex as StdMutex};
use std::task::{Context, Poll};
use std::time::Duration;

use async_trait::async_trait;
use ironrdp_core::{encode_vec, Encode as _};
use ironrdp_pdu::fast_path::{
    EncryptionFlags, FastPathHeader, FastPathUpdatePdu, Fragmentation, UpdateCode,
};
use ironrdp_pdu::nego::{
    ConnectionConfirm, ConnectionRequest, FailureCode, ResponseFlags, SecurityProtocol,
};
use ironrdp_pdu::rdp::capability_sets::BitmapCodecs;
use ironrdp_pdu::rdp::server_error_info::ProtocolIndependentCode;
use ironrdp_pdu::x224::X224;
use ironrdp_server::{
    BitmapUpdate, CliprdrServerFactory, ConnectionPolicy, ConnectionSource, Credentials,
    DesktopSize, DisplayContext, DisplayUpdate, ErrorInfo, ErrorInfoDisconnectHandle,
    ExactMatchCredentialValidator, GfxServerFactory, IncomingConnection, KeyboardEvent,
    MouseButton, MouseEvent, PixelFormat, RGBAPointer, RdpServer, RdpServerDisplay,
    RdpServerDisplayUpdates, RdpServerInputHandler, ServerError, ServerErrorExt as _, ServerResult,
    SoundServerFactory, StaticChannelFactory, TransportTls,
};
use ironrdp_tokio::{FramedWrite as _, TokioFramed};
use rcgen::{CertificateParams, DistinguishedName, DnType, KeyPair};
use rustls::pki_types::PrivatePkcs8KeyDer;
use sha2::{Digest, Sha256};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt, DuplexStream, ReadBuf};
use tokio::net::{TcpListener, TcpStream};
use tokio::sync::{mpsc, oneshot, Mutex};

pub enum FakeSecurity {
    /// TLS only: the server selects SSL and does not perform CredSSP.
    Tls,
    /// Require HYBRID (NLA); all supplied credentials must match.
    Hybrid {
        user: String,
        password: String,
        domain: Option<String>,
    },
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum FakeInput {
    Key {
        scancode: u8,
        extended: bool,
        down: bool,
    },
    Unicode {
        code: u16,
        down: bool,
    },
    MouseMove {
        x: u16,
        y: u16,
    },
    /// 0 = left, 1 = right, 2 = middle, 3 = X1, 4 = X2.
    Button {
        button: u8,
        down: bool,
    },
    Wheel {
        horizontal: bool,
        units: i16,
    },
}

/// Channel customization applied to IronRDP's builder before `build()`.
///
/// At rev 2bc793ef, `RdpServerBuilder<BuilderDone>` is in a private upstream
/// module. This public facade exposes its channel factory hooks without naming
/// that inaccessible type. Display control is offered by the display handler;
/// additional dynamic channels can be attached through a static channel factory.
///
/// ```no_run
/// # async fn example(factory: impl ironrdp_server::SoundServerFactory + Send + 'static) -> std::io::Result<()> {
/// use rs_rdp::testkit::{FakeRdpServer, FakeSecurity};
/// let server = FakeRdpServer::start_with(FakeSecurity::Tls, 1024, 768, move |builder| {
///     builder.with_sound_factory(Some(Box::new(factory)))
/// }).await?;
/// # Ok(())
/// # }
/// ```
#[derive(Default)]
pub struct FakeRdpServerBuilder {
    static_channels: Vec<Box<dyn StaticChannelFactory>>,
    gfx: Option<Box<dyn GfxServerFactory>>,
    sound: Option<Box<dyn SoundServerFactory>>,
    clipboard: Option<Box<dyn CliprdrServerFactory>>,
    codecs: Option<BitmapCodecs>,
}

impl FakeRdpServerBuilder {
    pub fn with_static_channel_factory(mut self, factory: Box<dyn StaticChannelFactory>) -> Self {
        self.static_channels.push(factory);
        self
    }

    pub fn with_gfx_factory(mut self, factory: Option<Box<dyn GfxServerFactory>>) -> Self {
        self.gfx = factory;
        self
    }

    pub fn with_sound_factory(mut self, factory: Option<Box<dyn SoundServerFactory>>) -> Self {
        self.sound = factory;
        self
    }

    pub fn with_cliprdr_factory(mut self, factory: Option<Box<dyn CliprdrServerFactory>>) -> Self {
        self.clipboard = factory;
        self
    }

    pub fn with_bitmap_codecs(mut self, codecs: BitmapCodecs) -> Self {
        self.codecs = Some(codecs);
        self
    }
}

pub struct FakeRdpServer {
    addr: SocketAddr,
    fingerprint: [u8; 32],
    size: DesktopSize,
    inputs: mpsc::UnboundedReceiver<FakeInput>,
    updates: mpsc::UnboundedSender<DisplayUpdate>,
    disconnect: ErrorInfoDisconnectHandle,
    connections: Arc<AtomicUsize>,
    suppression: Arc<StdMutex<Option<Arc<AtomicBool>>>>,
    stop: Option<oneshot::Sender<()>>,
    thread: Option<std::thread::JoinHandle<()>>,
}

struct Ready {
    addr: SocketAddr,
    fingerprint: [u8; 32],
    size: DesktopSize,
    inputs: mpsc::UnboundedReceiver<FakeInput>,
    updates: mpsc::UnboundedSender<DisplayUpdate>,
    disconnect: ErrorInfoDisconnectHandle,
    connections: Arc<AtomicUsize>,
    suppression: Arc<StdMutex<Option<Arc<AtomicBool>>>>,
}

impl FakeRdpServer {
    /// Start listening on 127.0.0.1:0 with a fresh ECDSA P-256 certificate (CN=fake-rdp).
    pub async fn start(security: FakeSecurity, width: u16, height: u16) -> io::Result<Self> {
        Self::start_with(security, width, height, |builder| builder).await
    }

    /// Like [`Self::start`], invoking `customize` exactly once before building the server.
    ///
    /// `Customize: FnOnce(FakeRdpServerBuilder) -> FakeRdpServerBuilder + Send + 'static`.
    /// Construct non-Send channel factories inside this closure, on the server thread.
    pub async fn start_with<Customize>(
        security: FakeSecurity,
        width: u16,
        height: u16,
        customize: Customize,
    ) -> io::Result<Self>
    where
        Customize: FnOnce(FakeRdpServerBuilder) -> FakeRdpServerBuilder + Send + 'static,
    {
        if !(200..=8192).contains(&width) || !(200..=8192).contains(&height) {
            return Err(io::Error::new(
                io::ErrorKind::InvalidInput,
                "RDP desktop dimensions must be 200..=8192",
            ));
        }
        let (ready_tx, ready_rx) = oneshot::channel();
        let (stop, stopped) = oneshot::channel();
        let thread = std::thread::Builder::new().name("fake-rdp".into()).spawn(move || {
            let runtime = match tokio::runtime::Builder::new_current_thread().enable_all().build() {
                Ok(runtime) => runtime,
                Err(error) => { let _ = ready_tx.send(Err(error)); return; }
            };
            runtime.block_on(tokio::task::LocalSet::new().run_until(async move {
                let task = tokio::task::spawn_local(async move {
                    let (mut server, mut source, ready) = match Self::build_server(security, width, height, customize).await {
                        Ok(parts) => parts,
                        Err(error) => { let _ = ready_tx.send(Err(error)); return; }
                    };
                    // If start() was cancelled, close the listener and all handlers.
                    if ready_tx.send(Ok(ready)).is_err() { return; }
                    tokio::select! {
                        result = server.serve(&mut source) => {
                            if let Err(error) = result { tracing::debug!(%error, "fake RDP server stopped"); }
                        }
                        _ = stopped => {}
                    }
                });
                if let Err(error) = task.await { tracing::debug!(%error, "fake RDP task stopped"); }
            }));
        })?;
        let ready = ready_rx
            .await
            .map_err(|_| io::Error::other("fake RDP server stopped during startup"))??;
        Ok(Self {
            addr: ready.addr,
            fingerprint: ready.fingerprint,
            size: ready.size,
            inputs: ready.inputs,
            updates: ready.updates,
            disconnect: ready.disconnect,
            connections: ready.connections,
            suppression: ready.suppression,
            stop: Some(stop),
            thread: Some(thread),
        })
    }

    async fn build_server<Customize>(
        security: FakeSecurity,
        width: u16,
        height: u16,
        customize: Customize,
    ) -> io::Result<(RdpServer, Listener, Ready)>
    where
        Customize: FnOnce(FakeRdpServerBuilder) -> FakeRdpServerBuilder,
    {
        let key = KeyPair::generate().map_err(io::Error::other)?;
        let mut params =
            CertificateParams::new(vec!["fake-rdp".into()]).map_err(io::Error::other)?;
        params.distinguished_name = DistinguishedName::new();
        params
            .distinguished_name
            .push(DnType::CommonName, "fake-rdp");
        let cert = params.self_signed(&key).map_err(io::Error::other)?;
        let fingerprint = Sha256::digest(cert.der().as_ref()).into();
        let (_, parsed) = x509_parser::parse_x509_certificate(cert.der().as_ref())
            .map_err(|error| io::Error::other(error.to_string()))?;
        // CredSSP binds to subjectPublicKey BIT STRING bytes, not the SPKI DER wrapper.
        let public_key = parsed.public_key().subject_public_key.data.to_vec();
        let config = rustls::ServerConfig::builder_with_provider(Arc::new(
            rustls::crypto::ring::default_provider(),
        ))
        .with_safe_default_protocol_versions()
        .map_err(io::Error::other)?
        .with_no_client_auth()
        .with_single_cert(
            vec![cert.der().clone()],
            PrivatePkcs8KeyDer::from(key.serialize_der()).into(),
        )
        .map_err(io::Error::other)?;
        // Binding here makes readiness race-free and never creates a probe connection.
        let listener = TcpListener::bind((Ipv4Addr::LOCALHOST, 0)).await?;
        let addr = listener.local_addr()?;
        let acceptor = tokio_rustls::TlsAcceptor::from(Arc::new(config));
        let transport_acceptor = acceptor.clone();
        let protocols = match &security {
            FakeSecurity::Tls => SecurityProtocol::SSL,
            FakeSecurity::Hybrid { .. } => SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX,
        };
        let builder = RdpServer::builder().with_addr(addr);
        let (builder, credentials) = match security {
            FakeSecurity::Tls => (builder.with_tls(acceptor), None),
            FakeSecurity::Hybrid {
                user,
                password,
                domain,
            } => (
                builder.with_hybrid(acceptor, public_key),
                Some(Credentials {
                    username: user,
                    password,
                    domain,
                }),
            ),
        };
        let (input_tx, inputs) = mpsc::unbounded_channel();
        let (updates, update_rx) = mpsc::unbounded_channel();
        let wire = Arc::new(StdMutex::new(None));
        let size = DesktopSize { width, height };
        let suppression = Arc::new(StdMutex::new(None));
        let mut builder = builder
            .with_input_handler(InputHandler(input_tx))
            .with_display_handler(DisplayHandler {
                size,
                updates: Arc::new(Mutex::new(update_rx)),
                wire: Arc::clone(&wire),
                suppression: suppression.clone(),
            })
            .with_connection_policy(ConnectionPolicy::Queue);
        let customization = customize(FakeRdpServerBuilder::default());
        for factory in customization.static_channels {
            builder = builder.with_static_channel_factory(factory);
        }
        if let Some(codecs) = customization.codecs {
            builder = builder.with_bitmap_codecs(codecs);
        }
        let mut server = builder
            .with_gfx_factory(customization.gfx)
            .with_sound_factory(customization.sound)
            .with_cliprdr_factory(customization.clipboard)
            .build();
        // Upstream's NTLM credential proxy checks the account name but accepts
        // the peer's domain. Also validate the delegated identity to enforce
        // this testkit's exact user/password/domain contract.
        if let Some(expected) = &credentials {
            server.set_credential_validator(Some(Arc::new(ExactMatchCredentialValidator::new(
                expected.clone(),
            ))));
        }
        server.set_credentials(credentials);
        let disconnect = server.error_info_disconnect_handle();
        let connections = Arc::new(AtomicUsize::new(0));
        let (incoming_tx, incoming) = mpsc::unbounded_channel();
        let accepted = Arc::clone(&connections);
        // LocalSet owns these tasks, including failed/unfinished TLS handshakes.
        // Leaving run_until drops them and releases the listener.
        tokio::task::spawn_local(async move {
            loop {
                let (stream, _) = match listener.accept().await {
                    Ok(connection) => connection,
                    Err(error) => {
                        tracing::debug!(%error, "fake RDP accept stopped");
                        return;
                    }
                };
                accepted.fetch_add(1, Ordering::SeqCst);
                let acceptor = transport_acceptor.clone();
                let incoming = incoming_tx.clone();
                tokio::task::spawn_local(async move {
                    if let Err(error) = serve_transport(stream, acceptor, protocols, incoming).await
                    {
                        tracing::debug!(%error, "fake RDP transport closed");
                    }
                });
            }
        });
        let source = Listener { incoming, wire };
        let ready = Ready {
            addr,
            fingerprint,
            size,
            inputs,
            updates,
            disconnect,
            connections,
            suppression,
        };
        Ok((server, source, ready))
    }

    pub fn addr(&self) -> SocketAddr {
        self.addr
    }

    /// SHA-256 of the leaf certificate DER, as observed by TOFU clients.
    pub fn cert_sha256(&self) -> [u8; 32] {
        self.fingerprint
    }

    /// Queue tightly packed BGRA8 pixels. Invalid rectangles/buffer sizes are ignored.
    pub fn push_bitmap(&self, x: u16, y: u16, w: u16, h: u16, bgra: Vec<u8>) {
        let (Some(width), Some(height)) = (NonZeroU16::new(w), NonZeroU16::new(h)) else {
            return;
        };
        let stride = NonZeroUsize::from(width).get() * 4;
        if usize::from(x) + usize::from(w) > usize::from(self.size.width)
            || usize::from(y) + usize::from(h) > usize::from(self.size.height)
            || bgra.len() != stride * usize::from(h)
        {
            return;
        }
        let Some(stride) = NonZeroUsize::new(stride) else {
            return;
        };
        let _ = self.updates.send(DisplayUpdate::Bitmap(BitmapUpdate {
            x,
            y,
            width,
            height,
            format: PixelFormat::BgrA32,
            data: bgra.into(),
            stride,
        }));
    }

    /// Queue an RGBA8 pointer (up to 96×96). Invalid shapes are ignored.
    pub fn push_pointer_bitmap(&self, w: u16, h: u16, hot_x: u16, hot_y: u16, rgba: Vec<u8>) {
        if w == 0
            || h == 0
            || w > 96
            || h > 96
            || hot_x >= w
            || hot_y >= h
            || rgba.len() != usize::from(w) * usize::from(h) * 4
        {
            return;
        }
        // Despite its name, upstream RGBAPointer serializes its buffer verbatim
        // as the wire XOR mask: bottom-up BGRA, rather than top-down RGBA.
        let data = rgba
            .chunks_exact(usize::from(w) * 4)
            .rev()
            .flat_map(|row| row.chunks_exact(4).flat_map(|p| [p[2], p[1], p[0], p[3]]))
            .collect();
        let _ = self.updates.send(DisplayUpdate::RGBAPointer(RGBAPointer {
            cache_index: 0,
            width: w,
            height: h,
            hot_x,
            hot_y,
            data,
        }));
    }

    pub fn push_pointer_hidden(&self) {
        let _ = self.updates.send(DisplayUpdate::HidePointer);
    }

    pub fn push_pointer_default(&self) {
        let _ = self.updates.send(DisplayUpdate::DefaultPointer);
    }

    /// Wait for the next input; timeout or a stopped server returns `None`.
    pub async fn next_input(&mut self, timeout: Duration) -> Option<FakeInput> {
        tokio::time::timeout(timeout, self.inputs.recv())
            .await
            .ok()
            .flatten()
    }

    /// End the current session; the listener remains available for reconnects.
    pub fn disconnect(&self) {
        let _ = self
            .disconnect
            .disconnect(ErrorInfo::ProtocolIndependentCode(
                ProtocolIndependentCode::RpcInitiatedDisconnect,
            ));
    }

    /// Number of accepted TCP connections, including failed authentication attempts.
    pub fn display_suppressed(&self) -> Option<bool> {
        self.suppression
            .lock()
            .unwrap()
            .as_ref()
            .map(|flag| flag.load(Ordering::Acquire))
    }

    pub fn connections(&self) -> usize {
        self.connections.load(Ordering::SeqCst)
    }
}

impl Drop for FakeRdpServer {
    fn drop(&mut self) {
        if let Some(stop) = self.stop.take() {
            let _ = stop.send(());
        }
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

struct Listener {
    incoming: mpsc::UnboundedReceiver<IncomingConnection<RdpPipe>>,
    wire: CurrentWire,
}

#[async_trait]
impl ConnectionSource for Listener {
    type Stream = RdpPipe;

    async fn accept(&mut self) -> io::Result<Option<IncomingConnection<RdpPipe>>> {
        // Receiver::recv is cancel safe; LocalSet retains unfinished handshakes.
        let Some(connection) = self.incoming.recv().await else {
            return Ok(None);
        };
        *self
            .wire
            .lock()
            .map_err(|_| io::Error::other("fake RDP wire lock poisoned"))? =
            Some(connection.stream.output.downgrade());
        Ok(Some(connection))
    }
}

struct InputHandler(mpsc::UnboundedSender<FakeInput>);

impl RdpServerInputHandler for InputHandler {
    fn keyboard(&mut self, event: KeyboardEvent) {
        let input = match event {
            KeyboardEvent::Pressed { code, extended } => FakeInput::Key {
                scancode: code,
                extended,
                down: true,
            },
            KeyboardEvent::Released { code, extended } => FakeInput::Key {
                scancode: code,
                extended,
                down: false,
            },
            KeyboardEvent::UnicodePressed(code) => FakeInput::Unicode { code, down: true },
            KeyboardEvent::UnicodeReleased(code) => FakeInput::Unicode { code, down: false },
            KeyboardEvent::Synchronize(_) => return,
        };
        let _ = self.0.send(input);
    }

    fn mouse(&mut self, event: MouseEvent) {
        let input = match event {
            MouseEvent::Move { x, y } => FakeInput::MouseMove { x, y },
            MouseEvent::Button {
                button, pressed, ..
            } => {
                let button = match button {
                    MouseButton::Left => 0,
                    MouseButton::Right => 1,
                    MouseButton::Middle => 2,
                    MouseButton::X1 => 3,
                    MouseButton::X2 => 4,
                    _ => return,
                };
                FakeInput::Button {
                    button,
                    down: pressed,
                }
            }
            MouseEvent::VerticalScroll { value } => FakeInput::Wheel {
                horizontal: false,
                units: value,
            },
            MouseEvent::HorizontalScroll { value } => FakeInput::Wheel {
                horizontal: true,
                units: value,
            },
            _ => return,
        };
        let _ = self.0.send(input);
    }
}

type UpdateReceiver = Arc<Mutex<mpsc::UnboundedReceiver<DisplayUpdate>>>;
struct DisplayHandler {
    suppression: Arc<StdMutex<Option<Arc<AtomicBool>>>>,
    size: DesktopSize,
    updates: UpdateReceiver,
    wire: CurrentWire,
}
struct DisplayUpdates {
    updates: UpdateReceiver,
    wire: CurrentWire,
}

#[async_trait]
impl RdpServerDisplay for DisplayHandler {
    async fn size(&mut self) -> DesktopSize {
        self.size
    }

    async fn updates(
        &mut self,
        context: DisplayContext,
    ) -> ServerResult<Box<dyn RdpServerDisplayUpdates>> {
        *self.suppression.lock().unwrap() = Some(context.display_suppressed);
        Ok(Box::new(DisplayUpdates {
            updates: Arc::clone(&self.updates),
            wire: Arc::clone(&self.wire),
        }))
    }
}

#[async_trait]
impl RdpServerDisplayUpdates for DisplayUpdates {
    async fn next_update(&mut self) -> ServerResult<Option<DisplayUpdate>> {
        loop {
            let Some(update) = self.updates.lock().await.recv().await else {
                return Ok(None);
            };
            let code = match update {
                DisplayUpdate::HidePointer => UpdateCode::HiddenPointer,
                DisplayUpdate::DefaultPointer => UpdateCode::DefaultPointer,
                other => return Ok(Some(other)),
            };
            // Upstream UpdateFragmenter treats an empty payload as end-of-stream,
            // so these two legitimate empty updates never reach its writer.
            let update = FastPathUpdatePdu {
                fragmentation: Fragmentation::Single,
                update_code: code,
                compression_flags: None,
                compression_type: None,
                data: &[],
            };
            let mut frame = encode_vec(&FastPathHeader::new(
                EncryptionFlags::empty(),
                update.size(),
            ))
            .map_err(ServerError::encode)?;
            frame.extend_from_slice(&encode_vec(&update).map_err(ServerError::encode)?);
            let output = self
                .wire
                .lock()
                .map_err(|_| ServerError::channel("fake RDP wire lock poisoned"))?
                .as_ref()
                .and_then(mpsc::WeakUnboundedSender::upgrade)
                .ok_or_else(|| ServerError::channel("fake RDP has no active transport"))?;
            output
                .send(WireMessage::Data(frame))
                .map_err(|_| ServerError::channel("fake RDP transport closed"))?;
        }
    }
}

// Display injection must not keep the connection open after RdpServer drops it.
type CurrentWire = Arc<StdMutex<Option<mpsc::WeakUnboundedSender<WireMessage>>>>;

enum WireMessage {
    Data(Vec<u8>),
    Flush(oneshot::Sender<io::Result<()>>),
    Shutdown(oneshot::Sender<io::Result<()>>),
}

/// Plain RDP bytes over application-owned TLS. Every write is queued atomically
/// with display injections. Flush acknowledgements ensure disconnect PDUs reach
/// the client before IronRDP drops its session.
struct RdpPipe {
    input: DuplexStream,
    prefix: std::io::Cursor<Vec<u8>>,
    output: mpsc::UnboundedSender<WireMessage>,
    flush: Option<oneshot::Receiver<io::Result<()>>>,
    shutdown: Option<oneshot::Receiver<io::Result<()>>>,
}

impl AsyncRead for RdpPipe {
    fn poll_read(
        mut self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        let pos = self.prefix.position() as usize;
        let left = &self.prefix.get_ref()[pos..];
        if !left.is_empty() {
            let len = left.len().min(buf.remaining());
            buf.put_slice(&left[..len]);
            self.prefix.set_position((pos + len) as u64);
            Poll::Ready(Ok(()))
        } else {
            Pin::new(&mut self.input).poll_read(cx, buf)
        }
    }
}

fn transport_closed() -> io::Error {
    io::Error::new(io::ErrorKind::BrokenPipe, "fake RDP transport closed")
}

fn poll_ack(
    ack: &mut Option<oneshot::Receiver<io::Result<()>>>,
    cx: &mut Context<'_>,
) -> Poll<io::Result<()>> {
    let Some(receiver) = ack else {
        return Poll::Ready(Err(transport_closed()));
    };
    match Pin::new(receiver).poll(cx) {
        Poll::Pending => Poll::Pending,
        Poll::Ready(result) => {
            *ack = None;
            Poll::Ready(result.unwrap_or_else(|_| Err(transport_closed())))
        }
    }
}

impl AsyncWrite for RdpPipe {
    fn poll_write(
        self: Pin<&mut Self>,
        _: &mut Context<'_>,
        bytes: &[u8],
    ) -> Poll<io::Result<usize>> {
        Poll::Ready(
            self.output
                .send(WireMessage::Data(bytes.to_vec()))
                .map(|()| bytes.len())
                .map_err(|_| transport_closed()),
        )
    }

    fn poll_flush(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.flush.is_none() {
            let (tx, rx) = oneshot::channel();
            if self.output.send(WireMessage::Flush(tx)).is_err() {
                return Poll::Ready(Err(transport_closed()));
            }
            self.flush = Some(rx);
        }
        poll_ack(&mut self.flush, cx)
    }

    fn poll_shutdown(mut self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        if self.shutdown.is_none() {
            let (tx, rx) = oneshot::channel();
            if self.output.send(WireMessage::Shutdown(tx)).is_err() {
                return Poll::Ready(Err(transport_closed()));
            }
            self.shutdown = Some(rx);
        }
        poll_ack(&mut self.shutdown, cx)
    }
}

async fn serve_transport(
    stream: TcpStream,
    acceptor: tokio_rustls::TlsAcceptor,
    protocols: SecurityProtocol,
    incoming: mpsc::UnboundedSender<IncomingConnection<RdpPipe>>,
) -> io::Result<()> {
    stream.set_nodelay(true)?;
    let peer = stream.peer_addr()?;
    let local = stream.local_addr()?;
    let mut framed = TokioFramed::new(stream);
    let (_, request) = framed.read_pdu().await?;
    let requested = ironrdp_core::decode::<X224<ConnectionRequest>>(&request)
        .map_err(io::Error::other)?
        .0
        .protocol;
    let common = requested & protocols;
    let selected = [
        SecurityProtocol::HYBRID_EX,
        SecurityProtocol::HYBRID,
        SecurityProtocol::SSL,
    ]
    .into_iter()
    .find(|protocol| common.contains(*protocol));
    // The acceptor encodes this response then returns Err, and its async driver
    // discards the output on error. Send it before closing the socket here.
    let confirm = match selected {
        Some(protocol) => ConnectionConfirm::Response {
            flags: ResponseFlags::EXTENDED_CLIENT_DATA_SUPPORTED,
            protocol,
        },
        None => ConnectionConfirm::Failure {
            code: if protocols.contains(SecurityProtocol::SSL) {
                FailureCode::SSL_REQUIRED_BY_SERVER
            } else {
                FailureCode::HYBRID_REQUIRED_BY_SERVER
            },
        },
    };
    let confirm = encode_vec(&X224(confirm)).map_err(io::Error::other)?;
    framed.write_all(&confirm).await?;
    if selected.is_none() {
        return Ok(());
    }
    let (stream, leftover) = framed.into_inner();
    if !leftover.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "unexpected bytes before fake RDP TLS",
        ));
    }
    let stream = acceptor.accept(stream).await?;
    let (mut tls_reader, mut tls_writer) = tokio::io::split(stream);
    let (input, mut input_writer) = tokio::io::duplex(64 * 1024);
    let (output, mut messages) = mpsc::unbounded_channel();
    let pipe = RdpPipe {
        input,
        prefix: std::io::Cursor::new(request.to_vec()),
        output,
        flush: None,
        shutdown: None,
    };
    // Replay the request into IronRDP's unchanged acceptor and consume its
    // duplicate confirm. The application performed exactly that TLS upgrade.
    if incoming
        .send(
            IncomingConnection::new(pipe, TransportTls::AlreadyDone, Some(peer))
                .with_local_addr(local),
        )
        .is_err()
    {
        return Ok(());
    }
    let receive = tokio::io::copy(&mut tls_reader, &mut input_writer);
    let send = async move {
        let mut first = true;
        while let Some(message) = messages.recv().await {
            match message {
                WireMessage::Data(bytes) if first => {
                    first = false;
                    if bytes != confirm {
                        return Err(io::Error::new(
                            io::ErrorKind::InvalidData,
                            "fake RDP negotiation replay differed",
                        ));
                    }
                }
                WireMessage::Data(bytes) => tls_writer.write_all(&bytes).await?,
                WireMessage::Flush(ack) => {
                    let _ = ack.send(tls_writer.flush().await);
                }
                WireMessage::Shutdown(ack) => {
                    let _ = ack.send(tls_writer.shutdown().await);
                    return Ok(());
                }
            }
        }
        Ok(())
    };
    tokio::select! { result = receive => result.map(|_| ()), result = send => result }
}
