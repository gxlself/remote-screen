//! RDP client session.

use std::collections::VecDeque;
use std::future::Future;
use std::io;
use std::net::{Ipv4Addr, SocketAddr};
use std::pin::Pin;
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc,
};
use std::task::{Context, Poll};
use std::time::Duration;

use ironrdp_connector::connection_activation::{
    ConnectionActivationFactory, ConnectionActivationSequence, ConnectionActivationState,
};
use ironrdp_connector::{
    BitmapConfig, ClientConnector, ClientConnectorState, Config, ConnectionResult, ConnectorError,
    ConnectorErrorKind, Credentials, DesktopSize, Sequence as _,
};
use ironrdp_core::WriteBuf;
use ironrdp_dvc::DrdynvcClient;
use ironrdp_graphics::image_processing::PixelFormat;
use ironrdp_pdu::gcc::{ConnectionType, KeyboardType};
use ironrdp_pdu::geometry::InclusiveRectangle;
use ironrdp_pdu::input::fast_path::{FastPathInputEvent, KeyboardFlags};
use ironrdp_pdu::input::mouse::PointerFlags;
use ironrdp_pdu::input::mouse_x::PointerXFlags;
use ironrdp_pdu::input::{MousePdu, MouseXPdu};
use ironrdp_pdu::nego::{FailureCode, SecurityProtocol};
use ironrdp_pdu::rdp::capability_sets::{MajorPlatformType, RailSupportLevel};
use ironrdp_session::image::DecodedImage;
use ironrdp_session::{ActiveStage, ActiveStageBuilder, ActiveStageOutput, SessionError};
use ironrdp_tokio::{FramedWrite as _, NetworkClient, TokioFramed};
use tokio::io::{AsyncRead, AsyncWrite, AsyncWriteExt as _, ReadBuf};
use tokio::sync::{mpsc, watch, Mutex, Notify};
use tracing::instrument::WithSubscriber as _;

use crate::ext::ExtCtx;
use crate::output::OutputControl;
use crate::types::{MouseButton, RdpCommand, RdpConfig, RdpError, RdpInput, RdpSink, Rect};
use crate::{audio, clip, display, gfx, tls};

pub trait AsyncReadWrite: AsyncRead + AsyncWrite + Send + Unpin {}
impl<T: AsyncRead + AsyncWrite + Send + Unpin> AsyncReadWrite for T {}

// TokioFramed at the pinned revision requires Sync, although it only polls with
// exclusive access. Mutex gives Send-only transports Sync without unsafe or a
// stronger public bound. get_mut never acquires a lock or holds one across await.
struct SyncIo<S>(Mutex<S>);

impl<S: AsyncRead + Unpin> AsyncRead for SyncIo<S> {
    fn poll_read(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &mut ReadBuf<'_>,
    ) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().0.get_mut()).poll_read(cx, buf)
    }
}

impl<S: AsyncWrite + Unpin> AsyncWrite for SyncIo<S> {
    fn poll_write(
        self: Pin<&mut Self>,
        cx: &mut Context<'_>,
        buf: &[u8],
    ) -> Poll<io::Result<usize>> {
        Pin::new(self.get_mut().0.get_mut()).poll_write(cx, buf)
    }

    fn poll_flush(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().0.get_mut()).poll_flush(cx)
    }

    fn poll_shutdown(self: Pin<&mut Self>, cx: &mut Context<'_>) -> Poll<io::Result<()>> {
        Pin::new(self.get_mut().0.get_mut()).poll_shutdown(cx)
    }
}

type Framed = TokioFramed<SyncIo<Box<dyn AsyncReadWrite>>>;
type SendResult<'a, T = ()> = Pin<Box<dyn Future<Output = Result<T, RdpError>> + Send + 'a>>;
type SendIo<'a, T> = Pin<Box<dyn Future<Output = io::Result<T>> + Send + 'a>>;
type WirePdu = (ironrdp_pdu::Action, ironrdp_tokio::bytes::Bytes);

// Erase the dependency's higher-ranked I/O future at the call site so the
// public connect/run futures can be moved into Tokio tasks.
fn read_frame<'a>(
    framed: &'a mut Framed,
    hint: Option<&'a dyn ironrdp_pdu::PduHint>,
) -> SendIo<'a, WirePdu> {
    Box::pin(async move {
        if let Some(hint) = hint {
            // Match single_sequence_step_read during reactivation: discard
            // unrelated fast-path data while waiting for activation X.224 PDUs.
            let read: SendIo<'_, _> = Box::pin(framed.read_by_hint(hint));
            let frame = read.await?;
            Ok((ironrdp_pdu::Action::X224, frame))
        } else {
            let read: SendIo<'_, _> = Box::pin(framed.read_pdu());
            read.await.map(|(action, frame)| (action, frame.freeze()))
        }
    })
}

// The pinned IronRDP logs complete ClientInfo and SaveSessionInfo PDUs at debug
// level. They can contain credentials, account names and reconnect cookies.
// Suppress those dependency events, even when the caller enables trace logging.
// Our own value-free diagnostics and typed errors remain available.
fn without_protocol_logs<T>(operation: impl FnOnce() -> T) -> T {
    tracing::subscriber::with_default(tracing::subscriber::NoSubscriber::default(), operation)
}

pub struct RdpConnection {
    framed: Framed,
    result: ConnectionResult,
    ctx: ExtCtx,
}

struct NoNetwork;

impl NetworkClient for NoNetwork {
    async fn send(
        &mut self,
        _: &ironrdp_connector::sspi::generator::NetworkRequest,
    ) -> ironrdp_connector::ConnectorResult<Vec<u8>> {
        Err(ironrdp_connector::general_err!(
            "external authentication traffic is disabled"
        ))
    }
}

fn connector_config(cfg: &RdpConfig) -> Config {
    Config {
        desktop_size: DesktopSize {
            width: cfg.width,
            height: cfg.height,
        },
        monitor_layout: None,
        desktop_scale_factor: cfg.scale_percent,
        enable_tls: true,
        enable_credssp: cfg.credentials.is_some(),
        enable_standard_rdp_security: false,
        // Install the secret only at the connector boundary, after the hooks.
        credentials: Credentials::UsernamePassword {
            username: String::new(),
            password: String::new(),
        },
        domain: None,
        client_build: 0,
        client_name: cfg.client_name.clone(),
        keyboard_type: KeyboardType::IBM_ENHANCED,
        keyboard_subtype: 0,
        keyboard_functional_keys_count: 12,
        keyboard_layout: 0,
        connection_type: ConnectionType::Lan,
        ime_file_name: String::new(),
        bitmap: Some(BitmapConfig {
            lossy_compression: false,
            color_depth: 32,
            codecs: Default::default(),
        }),
        dig_product_id: String::new(),
        client_dir: String::new(),
        alternate_shell: String::new(),
        work_dir: String::new(),
        remote_application_mode: false,
        rail_support_level: RailSupportLevel::empty(),
        platform: MajorPlatformType::UNSPECIFIED,
        hardware_id: None,
        request_data: None,
        autologon: cfg.credentials.is_some(),
        enable_audio_playback: false,
        enable_audio_capture: false,
        performance_flags: Default::default(),
        license_cache: None,
        timezone_info: Default::default(),
        compression_type: None,
        enable_server_pointer: true,
        pointer_software_rendering: false,
        multitransport_flags: None,
        support_dyn_vc_gfx_protocol: false,
    }
}

/// Negotiate X.224 security, approve TLS, authenticate with NTLM when selected,
/// and exchange capabilities. The supplied stream may also be a tunnel.
pub async fn connect<S>(
    stream: S,
    cfg: RdpConfig,
    sink: Arc<dyn RdpSink>,
) -> Result<RdpConnection, RdpError>
where
    S: AsyncRead + AsyncWrite + Send + Unpin + 'static,
{
    let has_credentials = cfg.credentials.is_some();
    let ctx = ExtCtx {
        sink,
        audio: cfg.audio.clone(),
        wake: Arc::new(Notify::new()),
        background: Arc::new(AtomicBool::new(false)),
    };
    let mut config = connector_config(&cfg);
    gfx::configure(&cfg, &ctx, &mut config);
    audio::configure(&cfg, &ctx, &mut config);
    clip::configure(&cfg, &ctx, &mut config);
    display::configure(&cfg, &ctx, &mut config);

    let drdynvc = DrdynvcClient::new();
    let drdynvc = gfx::dynamic_channels(&cfg, &ctx, drdynvc);
    let drdynvc = audio::dynamic_channels(&cfg, &ctx, drdynvc);
    let drdynvc = clip::dynamic_channels(&cfg, &ctx, drdynvc);
    let drdynvc = display::dynamic_channels(&cfg, &ctx, drdynvc);
    // AsyncRead/AsyncWrite transports need not expose a local socket address.
    let addr = SocketAddr::from((Ipv4Addr::UNSPECIFIED, 0));
    let connector = ClientConnector::new(config, addr).with_static_channel(drdynvc);
    let connector = gfx::static_channels(&cfg, &ctx, connector);
    let connector = audio::static_channels(&cfg, &ctx, connector);
    let connector = clip::static_channels(&cfg, &ctx, connector);
    let mut connector = display::static_channels(&cfg, &ctx, connector);
    // Reactivation only needs capabilities; do not retain credentials in our
    // activation factory for the whole session.
    let activation_config = connector.config.clone();
    if let Some(credentials) = &cfg.credentials {
        // IronRDP 2bc793e requires an owned String and copies it internally.
        // Our credential stays Zeroizing; this unavoidable upstream copy must
        // be removed upstream before end-to-end zeroization can be guaranteed.
        connector.config.credentials = Credentials::UsernamePassword {
            username: credentials.user.clone(),
            password: credentials.password.to_string(),
        };
        connector.config.domain = credentials.domain.clone();
    }

    let mut framed = TokioFramed::new(SyncIo(Mutex::new(stream)));
    let should_upgrade = {
        let begin: Pin<Box<dyn Future<Output = _> + Send + '_>> =
            Box::pin(ironrdp_tokio::connect_begin(&mut framed, &mut connector));
        begin
            .with_subscriber(tracing::subscriber::NoSubscriber::default())
            .await
            .map_err(|error| map_connector_error(error, has_credentials))?
    };
    // The pinned connector checks bitset intersection, so also reject invalid
    // multi-protocol selections instead of treating SSL | RDSTLS as SSL.
    if let ClientConnectorState::EnhancedSecurityUpgrade { selected_protocol } = connector.state {
        let supported = selected_protocol == SecurityProtocol::SSL
            || (has_credentials
                && matches!(
                    selected_protocol,
                    SecurityProtocol::HYBRID | SecurityProtocol::HYBRID_EX
                ));
        if !supported {
            return Err(RdpError::UnsupportedSecurity(selected_protocol.to_string()));
        }
    }
    let (stream, leftover) = framed.into_inner();
    if !leftover.is_empty() {
        return Err(RdpError::Protocol("unexpected bytes before TLS".into()));
    }
    let (stream, public_key, _) =
        tls::upgrade(stream.0.into_inner(), &cfg.server_name, cfg.cert.as_ref()).await?;
    let upgraded = ironrdp_tokio::mark_as_upgraded(should_upgrade, &mut connector);
    let stream: Box<dyn AsyncReadWrite> = Box::new(stream);
    let mut framed = TokioFramed::new(SyncIo(Mutex::new(stream)));
    let mut network = NoNetwork;
    let finalize: Pin<Box<dyn Future<Output = _> + Send + '_>> =
        Box::pin(ironrdp_tokio::connect_finalize(
            upgraded,
            connector,
            &mut framed,
            &mut network,
            cfg.server_name.clone().into(),
            public_key,
            None,
        ));
    let mut result = finalize
        .with_subscriber(tracing::subscriber::NoSubscriber::default())
        .await
        .map_err(|error| map_connector_error(error, has_credentials))?;
    result.activation_factory = ConnectionActivationFactory::new(
        activation_config,
        result.io_channel_id,
        result.user_channel_id,
    );
    Ok(RdpConnection {
        framed,
        result,
        ctx,
    })
}

impl RdpConnection {
    /// The server's negotiated desktop size, which may differ from the request.
    pub fn desktop_size(&self) -> (u16, u16) {
        (
            self.result.desktop_size.width,
            self.result.desktop_size.height,
        )
    }

    /// Run until the server ends the session or all command senders are dropped.
    pub async fn run(self, commands: mpsc::Receiver<RdpCommand>) -> Result<(), RdpError> {
        let (_visibility, background) = watch::channel(false);
        self.run_with_background(commands, background, 0).await
    }

    pub async fn run_with_background(
        self,
        mut commands: mpsc::Receiver<RdpCommand>,
        mut background: watch::Receiver<bool>,
        session_id: u64,
    ) -> Result<(), RdpError> {
        let Self {
            mut framed,
            result,
            ctx,
        } = self;
        let mut image = DecodedImage::new(
            PixelFormat::RgbA32,
            result.desktop_size.width,
            result.desktop_size.height,
        );
        let mut output = OutputControl::new(
            result.suppress_output_support,
            result.refresh_rect_support,
            result.io_channel_id,
        );
        let mut visible_size = None;
        let mut visibility_open = true;
        if let Some(layout) = result.monitor_layout {
            display::server_layout(&ctx, &layout.monitors);
        }
        let activation_factory = result.activation_factory;
        let mut stage = ActiveStageBuilder {
            static_channels: result.static_channels,
            user_channel_id: result.user_channel_id,
            io_channel_id: result.io_channel_id,
            message_channel_id: result.message_channel_id,
            share_id: result.share_id,
            compression_type: result.compression_type,
            enable_server_pointer: result.enable_server_pointer,
            pointer_software_rendering: false,
        }
        .build();
        stage.set_window_support_level(result.window_support_level);
        let mut activation: Option<ConnectionActivationSequence> = None;
        let mut pending = VecDeque::new();
        let mut buf = WriteBuf::new();

        loop {
            let paused = *background.borrow_and_update();
            ctx.background.store(paused, Ordering::Release);
            if output.paused != paused {
                output.paused = paused;
                tracing::info!(
                    session_id,
                    protocol = "rdp",
                    method = output.method(),
                    paused,
                    "adapter background changed"
                );
            }
            publish_size(&image, &ctx, &mut visible_size);
            if activation.is_none() {
                let bytes = without_protocol_logs(|| {
                    output.apply(&mut stage, image.width(), image.height())
                })?;
                write_frame(&mut framed, &bytes).await?;
            }
            if let Some(sequence) = activation.as_mut() {
                if sequence.next_pdu_hint().is_none() {
                    advance_activation(sequence, &[], None, &mut buf)?;
                    write_frame(&mut framed, buf.filled()).await?;
                    if finish_activation(sequence, &mut stage, &mut image, &ctx)? {
                        reactivated(sequence, &mut output);
                        activation = None;
                    }
                    continue;
                }
            } else if let Some(command) = pending.pop_front() {
                command_outputs(command, &mut framed, &mut stage, &mut image, &ctx).await?;
                continue;
            }

            tokio::select! {
                biased;
                changed = background.changed(), if visibility_open => visibility_open = changed.is_ok(),
                command = commands.recv() => {
                    match command {
                        None => {
                            shutdown(&mut framed, &mut stage, &mut image, &ctx).await;
                            return Ok(());
                        }
                        Some(command) if ctx.background.load(Ordering::Acquire) && !matches!(command, RdpCommand::Clipboard(_)) => {},
                        Some(command) if activation.is_some() => pending.push_back(command),
                        Some(command) => {
                            command_outputs(command, &mut framed, &mut stage, &mut image, &ctx).await?;
                        }
                    }
                }
                incoming = read_frame(&mut framed, activation.as_ref().and_then(|sequence| sequence.next_pdu_hint())) => {
                    let (action, frame) = incoming.map_err(map_read_error)?;
                    if let Some(sequence) = activation.as_mut() {
                        advance_activation(sequence, &frame, framed.last_read_at(), &mut buf)?;
                        write_frame(&mut framed, buf.filled()).await?;
                        if finish_activation(sequence, &mut stage, &mut image, &ctx)? {
                            reactivated(sequence, &mut output);
                            activation = None;
                        }
                    } else {
                        let outputs = without_protocol_logs(|| output.process(&mut stage, &mut image, action, &frame, framed.last_read_at()))?;
                        publish_size(&image, &ctx, &mut visible_size);
                        let dispatched = dispatch_outputs(&mut framed, &image, &ctx, outputs).await;
                        let pumped = pump(&mut framed, &mut stage, &ctx).await;
                        let deactivate = dispatched?;
                        pumped?;
                        if deactivate {
                            activation = Some(activation_factory.create());
                        }
                    }
                }
                _ = ctx.wake.notified() => pump(&mut framed, &mut stage, &ctx).await?,
            }
        }
    }
}

fn publish_size(image: &DecodedImage, ctx: &ExtCtx, visible_size: &mut Option<(u16, u16)>) {
    let size = (image.width(), image.height());
    if !ctx.background.load(Ordering::Acquire) && *visible_size != Some(size) {
        ctx.sink.resize(size.0, size.1);
        *visible_size = Some(size);
    }
}

fn reactivated(sequence: &ConnectionActivationSequence, output: &mut OutputControl) {
    if let ConnectionActivationState::Finalized {
        suppress_output_support,
        refresh_rect_support,
        ..
    } = sequence.connection_activation_state()
    {
        output.reactivated(
            suppress_output_support,
            refresh_rect_support,
            sequence.io_channel_id(),
        );
    }
}

fn advance_activation(
    sequence: &mut ConnectionActivationSequence,
    frame: &[u8],
    received_at: Option<ironrdp_core::MonotonicInstant>,
    buf: &mut WriteBuf,
) -> Result<(), RdpError> {
    buf.clear();
    without_protocol_logs(|| sequence.step(frame, received_at, buf))
        .map_err(|error| map_connector_error(error, false))?;
    Ok(())
}

fn finish_activation(
    sequence: &ConnectionActivationSequence,
    stage: &mut ActiveStage,
    image: &mut DecodedImage,
    ctx: &ExtCtx,
) -> Result<bool, RdpError> {
    let ConnectionActivationState::Finalized {
        desktop_size,
        share_id,
        enable_server_pointer,
        static_channel_chunk_size,
        window_support_level,
        ..
    } = sequence.connection_activation_state()
    else {
        return Ok(false);
    };
    if !stage.reactivate(
        sequence.io_channel_id(),
        sequence.user_channel_id(),
        share_id,
        enable_server_pointer,
        false,
        static_channel_chunk_size,
    ) {
        return Err(RdpError::Protocol(
            "invalid static channel chunk size".into(),
        ));
    }
    stage.set_window_support_level(window_support_level);
    *image = DecodedImage::new(PixelFormat::RgbA32, desktop_size.width, desktop_size.height);
    if let Some(layout) = sequence.monitor_layout() {
        display::server_layout(ctx, &layout.monitors);
    }
    Ok(true)
}

async fn write_frame(framed: &mut Framed, frame: &[u8]) -> Result<(), RdpError> {
    if !frame.is_empty() {
        framed
            .write_all(frame)
            .await
            .map_err(|error| RdpError::Io(error.to_string()))?;
    }
    Ok(())
}

fn pump<'a>(framed: &'a mut Framed, stage: &'a mut ActiveStage, ctx: &'a ExtCtx) -> SendResult<'a> {
    Box::pin(async move {
        write_frame(framed, &gfx::pump(stage, ctx)?).await?;
        write_frame(framed, &audio::pump(stage, ctx)?).await?;
        write_frame(framed, &clip::pump(stage, ctx)?).await?;
        write_frame(framed, &display::pump(stage, ctx)?).await
    })
}

fn clip_rect(rect: InclusiveRectangle, image: &DecodedImage) -> Option<Rect> {
    let right = u32::from(rect.right)
        .saturating_add(1)
        .min(u32::from(image.width()));
    let bottom = u32::from(rect.bottom)
        .saturating_add(1)
        .min(u32::from(image.height()));
    if right <= u32::from(rect.left) || bottom <= u32::from(rect.top) {
        return None;
    }
    Some(Rect {
        x: rect.left,
        y: rect.top,
        w: (right - u32::from(rect.left)) as u16,
        h: (bottom - u32::from(rect.top)) as u16,
    })
}

fn dispatch_outputs<'a>(
    framed: &'a mut Framed,
    image: &'a DecodedImage,
    ctx: &'a ExtCtx,
    outputs: Vec<ActiveStageOutput>,
) -> SendResult<'a, bool> {
    Box::pin(async move {
        let mut updated = false;
        let mut deactivate = false;
        let mut terminated = None;
        for output in outputs {
            if ctx.background.load(Ordering::Acquire)
                && matches!(
                    &output,
                    ActiveStageOutput::GraphicsUpdate(_)
                        | ActiveStageOutput::PointerDefault
                        | ActiveStageOutput::PointerHidden
                        | ActiveStageOutput::PointerPosition { .. }
                        | ActiveStageOutput::PointerBitmap(_)
                )
            {
                continue;
            }
            match output {
                ActiveStageOutput::ResponseFrame(frame) => write_frame(framed, &frame).await?,
                ActiveStageOutput::GraphicsUpdate(rect) => {
                    if let Some(rect) = clip_rect(rect, image) {
                        ctx.sink.update(image.data(), image.stride(), rect);
                        updated = true;
                    }
                }
                ActiveStageOutput::PointerDefault => ctx.sink.pointer_default(),
                ActiveStageOutput::PointerHidden => ctx.sink.pointer_hidden(),
                ActiveStageOutput::PointerPosition { x, y } => ctx.sink.pointer_pos(x, y),
                ActiveStageOutput::PointerBitmap(pointer) => {
                    // pointer_software_rendering=false selects PointerBitmapTarget::Accelerated:
                    // tightly packed, top-down, non-premultiplied RGBA (including partial alpha).
                    ctx.sink.pointer_bitmap(
                        pointer.width,
                        pointer.height,
                        pointer.hotspot_x,
                        pointer.hotspot_y,
                        &pointer.bitmap_data,
                    );
                }
                ActiveStageOutput::MonitorLayout(monitors) => {
                    display::server_layout(ctx, &monitors)
                }
                ActiveStageOutput::Terminate(reason) => terminated = Some(reason.to_string()),
                ActiveStageOutput::DeactivateAll => deactivate = true,
                // No RemoteApp, multitransport or automatic reconnect is advertised here.
                ActiveStageOutput::WindowingOrders(_)
                | ActiveStageOutput::SaveSessionInfo { .. }
                | ActiveStageOutput::MultitransportRequest(_)
                | ActiveStageOutput::AutoDetect(_)
                | ActiveStageOutput::AutoReconnectCookie(_)
                | ActiveStageOutput::AutoReconnectFailed => {}
            }
        }
        if updated {
            ctx.sink.commit();
        }
        if let Some(reason) = terminated {
            return Err(RdpError::Closed(reason));
        }
        Ok(deactivate)
    })
}

fn command_outputs<'a>(
    command: RdpCommand,
    framed: &'a mut Framed,
    stage: &'a mut ActiveStage,
    image: &'a mut DecodedImage,
    ctx: &'a ExtCtx,
) -> SendResult<'a> {
    Box::pin(async move {
        if ctx.background.load(Ordering::Acquire) && !matches!(&command, RdpCommand::Clipboard(_)) {
            return Ok(());
        }
        match command {
            RdpCommand::Input(input) => {
                if let Some(event) = input_event(input)? {
                    let outputs =
                        without_protocol_logs(|| stage.process_fastpath_input(image, &[event]))
                            .map_err(map_session_error)?;
                    dispatch_outputs(framed, image, ctx, outputs).await?;
                }
            }
            RdpCommand::Clipboard(text) => {
                write_frame(framed, &clip::send(stage, ctx, &text)?).await?
            }
            RdpCommand::Resize {
                width,
                height,
                scale_percent,
            } => {
                write_frame(
                    framed,
                    &display::resize(stage, ctx, width, height, scale_percent)?,
                )
                .await?;
            }
            RdpCommand::Monitors(count) => match display::set_monitors(stage, ctx, count)? {
                Some(bytes) => write_frame(framed, &bytes).await?,
                None => {
                    // The remote session stays logged on; the caller reconnects to it.
                    shutdown(framed, stage, image, ctx).await;
                    return Err(RdpError::Reconnect(count.clamp(1, 4)));
                }
            },
        }
        Ok(())
    })
}

fn input_event(input: RdpInput) -> Result<Option<FastPathInputEvent>, RdpError> {
    Ok(Some(match input {
        RdpInput::Key {
            scancode,
            extended,
            down,
        } => {
            let mut flags = KeyboardFlags::empty();
            flags.set(KeyboardFlags::RELEASE, !down);
            flags.set(KeyboardFlags::EXTENDED, extended);
            FastPathInputEvent::KeyboardEvent(flags, scancode)
        }
        RdpInput::Unicode { code, down } => {
            let mut flags = KeyboardFlags::empty();
            flags.set(KeyboardFlags::RELEASE, !down);
            FastPathInputEvent::UnicodeKeyboardEvent(flags, code)
        }
        RdpInput::MouseMove { x, y } => FastPathInputEvent::MouseEvent(MousePdu {
            flags: PointerFlags::MOVE,
            number_of_wheel_rotation_units: 0,
            x_position: x,
            y_position: y,
        }),
        RdpInput::MouseButton {
            button: button @ (MouseButton::X1 | MouseButton::X2),
            down,
            x,
            y,
        } => {
            let mut flags = if button == MouseButton::X1 {
                PointerXFlags::BUTTON1
            } else {
                PointerXFlags::BUTTON2
            };
            flags.set(PointerXFlags::DOWN, down);
            FastPathInputEvent::MouseEventEx(MouseXPdu {
                flags,
                x_position: x,
                y_position: y,
            })
        }
        RdpInput::MouseButton { button, down, x, y } => {
            let mut flags = match button {
                MouseButton::Left => PointerFlags::LEFT_BUTTON,
                MouseButton::Right => PointerFlags::RIGHT_BUTTON,
                MouseButton::Middle => PointerFlags::MIDDLE_BUTTON_OR_WHEEL,
                MouseButton::X1 | MouseButton::X2 => unreachable!(),
            };
            flags.set(PointerFlags::DOWN, down);
            FastPathInputEvent::MouseEvent(MousePdu {
                flags,
                number_of_wheel_rotation_units: 0,
                x_position: x,
                y_position: y,
            })
        }
        RdpInput::Wheel { horizontal, units } => {
            if !(-255..=255).contains(&units) {
                return Err(RdpError::Protocol("wheel units outside -255..=255".into()));
            }
            FastPathInputEvent::MouseEvent(MousePdu {
                flags: if horizontal {
                    PointerFlags::HORIZONTAL_WHEEL
                } else {
                    PointerFlags::VERTICAL_WHEEL
                },
                number_of_wheel_rotation_units: units,
                x_position: 0,
                y_position: 0,
            })
        }
        RdpInput::ReleaseAll => {
            tracing::debug!("RDP release-all marker (releases already sent)");
            return Ok(None);
        }
    }))
}

fn shutdown<'a>(
    framed: &'a mut Framed,
    stage: &'a mut ActiveStage,
    image: &'a mut DecodedImage,
    ctx: &'a ExtCtx,
) -> Pin<Box<dyn Future<Output = ()> + Send + 'a>> {
    Box::pin(async move {
        // Give the server a chance to send ShutdownDenied/MCS disconnect, but a
        // silent peer must not keep a user-closed session alive indefinitely.
        let _ = tokio::time::timeout(
            Duration::from_secs(2),
            drain_shutdown(framed, stage, image, ctx),
        )
        .await;
        let _ = tokio::time::timeout(
            Duration::from_millis(250),
            framed.get_inner_mut().0.shutdown(),
        )
        .await;
    })
}

fn drain_shutdown<'a>(
    framed: &'a mut Framed,
    stage: &'a mut ActiveStage,
    image: &'a mut DecodedImage,
    ctx: &'a ExtCtx,
) -> SendResult<'a> {
    Box::pin(async move {
        let outputs = stage.graceful_shutdown().map_err(map_session_error)?;
        dispatch_outputs(framed, image, ctx, outputs).await?;
        loop {
            let (action, frame) = read_frame(framed, None).await.map_err(map_read_error)?;
            let outputs = without_protocol_logs(|| {
                stage.process(image, action, &frame, framed.last_read_at())
            })
            .map_err(map_session_error)?;
            dispatch_outputs(framed, image, ctx, outputs).await?;
            pump(framed, stage, ctx).await?;
        }
    })
}

fn map_read_error(error: io::Error) -> RdpError {
    match error.kind() {
        io::ErrorKind::UnexpectedEof => RdpError::Closed("eof".into()),
        io::ErrorKind::ConnectionReset
        | io::ErrorKind::ConnectionAborted
        | io::ErrorKind::BrokenPipe => RdpError::Closed(error.to_string()),
        _ => map_source_error(&error).unwrap_or_else(|| RdpError::Io(error.to_string())),
    }
}

fn map_source_error(error: &(dyn std::error::Error + 'static)) -> Option<RdpError> {
    let mut current = Some(error);
    let mut io_error = None;
    while let Some(source) = current {
        if let Some(error) = source.downcast_ref::<RdpError>() {
            return Some(error.clone());
        }
        if let Some(error) = source.downcast_ref::<rustls::Error>() {
            return Some(RdpError::Tls(error.to_string()));
        }
        if let Some(error) = source.downcast_ref::<ironrdp_core::DecodeError>() {
            return Some(RdpError::Protocol(error.to_string()));
        }
        if let Some(error) = source.downcast_ref::<io::Error>() {
            io_error = Some(RdpError::Io(error.to_string()));
            // io::Error::source() delegates to its payload's source, skipping
            // the payload itself. Inspect get_ref() to retain rustls errors.
            if let Some(inner) = error.get_ref() {
                current = Some(inner);
                continue;
            }
        }
        current = source.source();
    }
    io_error
}

// F's probe returns Credssp(InvalidToken, nstatus=LOGON_FAILURE). Prefer typed
// statuses; these fallbacks cover servers/dependency wrappers exposing text only.
const AUTH_FAILURE_MARKERS: &[&str] = &[
    "sec_e_logon_denied",
    "status_logon_failure",
    "0x8009030c",
    "0xc000006d",
];
const UNADVERTISED_PROTOCOL_PREFIX: &str = "client advertised ";
const UNADVERTISED_PROTOCOL_SEPARATOR: &str = ", but server selected ";
const ACTIVATION_DISCONNECT_PREFIX: &str = "server ended the session with error info: ";

fn map_connector_error(error: ConnectorError, has_credentials: bool) -> RdpError {
    use ironrdp_connector::sspi::{credssp::NStatusCode, ErrorKind};

    match error.kind() {
        ConnectorErrorKind::Negotiation(failure) => {
            if failure.code() == FailureCode::HYBRID_REQUIRED_BY_SERVER && !has_credentials {
                return RdpError::NlaRequired;
            }
            return RdpError::UnsupportedSecurity(format!("0x{:08x}", u32::from(failure.code())));
        }
        ConnectorErrorKind::AccessDenied => return RdpError::AuthFailed,
        ConnectorErrorKind::Credssp(error) => {
            if error.error_type == ErrorKind::LogonDenied
                || matches!(
                    error.nstatus,
                    Some(NStatusCode::LOGON_FAILURE | NStatusCode::WRONG_PASSWORD)
                )
            {
                return RdpError::AuthFailed;
            }
        }
        ConnectorErrorKind::Reason(reason) if reason.starts_with(UNADVERTISED_PROTOCOL_PREFIX) => {
            if let Some((_, protocol)) = reason.split_once(UNADVERTISED_PROTOCOL_SEPARATOR) {
                return RdpError::UnsupportedSecurity(protocol.to_owned());
            }
        }
        ConnectorErrorKind::Reason(reason) if reason.starts_with(ACTIVATION_DISCONNECT_PREFIX) => {
            return RdpError::Closed(reason[ACTIVATION_DISCONNECT_PREFIX.len()..].to_owned());
        }
        _ => {}
    }
    if let Some(mapped) = map_source_error(&error) {
        return mapped;
    }
    let mut source: Option<&(dyn std::error::Error + 'static)> = Some(&error);
    while let Some(current) = source {
        let text = current.to_string().to_ascii_lowercase();
        if AUTH_FAILURE_MARKERS
            .iter()
            .any(|marker| text.contains(marker))
        {
            return RdpError::AuthFailed;
        }
        source = current.source();
    }
    // Avoid including the SSPI description: it can contain the account name.
    if matches!(error.kind(), ConnectorErrorKind::Credssp(_)) {
        RdpError::Protocol("CredSSP failed".into())
    } else {
        RdpError::Protocol(error.to_string())
    }
}

fn map_session_error(error: SessionError) -> RdpError {
    map_source_error(&error).unwrap_or_else(|| RdpError::Protocol(error.to_string()))
}

#[cfg(test)]
mod tests {
    use super::*;
    use ironrdp_connector::sspi::{credssp::NStatusCode, Error as SspiError, ErrorKind};
    use ironrdp_connector::ConnectorErrorExt as _;

    #[test]
    fn map_connector_error_table() {
        for (code, has_credentials, expected) in [
            (
                FailureCode::HYBRID_REQUIRED_BY_SERVER,
                false,
                RdpError::NlaRequired,
            ),
            (
                FailureCode::HYBRID_REQUIRED_BY_SERVER,
                true,
                RdpError::UnsupportedSecurity("0x00000005".into()),
            ),
            (
                FailureCode::SSL_NOT_ALLOWED_BY_SERVER,
                false,
                RdpError::UnsupportedSecurity("0x00000002".into()),
            ),
        ] {
            assert_eq!(
                map_connector_error(
                    ConnectorError::new(
                        "negotiation",
                        ConnectorErrorKind::Negotiation(code.into())
                    ),
                    has_credentials
                ),
                expected
            );
        }
        let table = [
            (
                ConnectorError::new("CredSSP", ConnectorErrorKind::AccessDenied),
                RdpError::AuthFailed,
            ),
            (
                ConnectorError::new(
                    "CredSSP",
                    ConnectorErrorKind::Credssp(SspiError::new(ErrorKind::LogonDenied, "denied")),
                ),
                RdpError::AuthFailed,
            ),
            (
                ConnectorError::new(
                    "CredSSP",
                    ConnectorErrorKind::Credssp(SspiError::new_with_nstatus(
                        ErrorKind::InvalidToken,
                        "CredSSP server returned an error status",
                        NStatusCode::LOGON_FAILURE,
                    )),
                ),
                RdpError::AuthFailed,
            ),
            (
                ConnectorError::new(
                    "CredSSP",
                    ConnectorErrorKind::Credssp(SspiError::new_with_nstatus(
                        ErrorKind::InvalidToken,
                        "denied",
                        NStatusCode::WRONG_PASSWORD,
                    )),
                ),
                RdpError::AuthFailed,
            ),
            (
                ConnectorError::new(
                    "CredSSP",
                    ConnectorErrorKind::Credssp(SspiError::new(
                        ErrorKind::InvalidToken,
                        "must not expose account details",
                    )),
                ),
                RdpError::Protocol("CredSSP failed".into()),
            ),
            (
                ConnectorError::reason(
                    "Initiation",
                    "client advertised SSL, but server selected RDSTLS",
                ),
                RdpError::UnsupportedSecurity("RDSTLS".into()),
            ),
            (
                ConnectorError::custom("read", io::Error::other("broken transport")),
                RdpError::Io("broken transport".into()),
            ),
            (
                ConnectorError::custom("TLS", rustls::Error::NoCertificatesPresented),
                RdpError::Tls(rustls::Error::NoCertificatesPresented.to_string()),
            ),
            (
                ConnectorError::custom(
                    "TLS",
                    io::Error::other(rustls::Error::NoCertificatesPresented),
                ),
                RdpError::Tls(rustls::Error::NoCertificatesPresented.to_string()),
            ),
            (
                ConnectorError::custom("trust", RdpError::CertRejected),
                RdpError::CertRejected,
            ),
            (
                ConnectorError::general("bad PDU"),
                RdpError::Protocol("[bad PDU] general error".into()),
            ),
            (
                ConnectorError::reason(
                    "activation",
                    format!("{ACTIVATION_DISCONNECT_PREFIX}server closed"),
                ),
                RdpError::Closed("server closed".into()),
            ),
        ];
        for (error, expected) in table {
            assert_eq!(map_connector_error(error, true), expected);
        }
        for marker in AUTH_FAILURE_MARKERS {
            let error = ConnectorError::reason("CredSSP", marker.to_ascii_uppercase());
            assert_eq!(map_connector_error(error, true), RdpError::AuthFailed);
        }
    }

    #[test]
    fn graphics_rectangles_are_clipped_without_overflow() {
        let image = DecodedImage::new(PixelFormat::RgbA32, 10, 8);
        assert_eq!(
            clip_rect(
                InclusiveRectangle {
                    left: 8,
                    top: 6,
                    right: u16::MAX,
                    bottom: u16::MAX
                },
                &image
            ),
            Some(Rect {
                x: 8,
                y: 6,
                w: 2,
                h: 2
            })
        );
        assert_eq!(
            clip_rect(
                InclusiveRectangle {
                    left: 10,
                    top: 0,
                    right: 20,
                    bottom: 7
                },
                &image
            ),
            None
        );
        assert_eq!(
            clip_rect(
                InclusiveRectangle {
                    left: 5,
                    top: 5,
                    right: 4,
                    bottom: 4
                },
                &image
            ),
            None
        );
    }

    #[test]
    fn eof_and_wheel_boundaries() {
        assert_eq!(
            map_read_error(io::Error::from(io::ErrorKind::UnexpectedEof)),
            RdpError::Closed("eof".into())
        );
        for units in [-255, -120, -1, 0, 1, 120, 255] {
            let Some(FastPathInputEvent::MouseEvent(event)) = input_event(RdpInput::Wheel {
                horizontal: false,
                units,
            })
            .unwrap() else {
                panic!("missing wheel event")
            };
            let encoded = ironrdp_core::encode_vec(&event).unwrap();
            let decoded: MousePdu = ironrdp_core::decode(&encoded).unwrap();
            assert_eq!(decoded.number_of_wheel_rotation_units, units);
        }
        assert!(input_event(RdpInput::Wheel {
            horizontal: false,
            units: 256
        })
        .is_err());
        assert!(input_event(RdpInput::ReleaseAll).unwrap().is_none());
    }

    #[tokio::test]
    async fn reactivation_filters_non_x224_frames() {
        tokio::time::timeout(Duration::from_secs(20), async {
            let (stream, mut peer) = tokio::io::duplex(4096);
            let stream: Box<dyn AsyncReadWrite> = Box::new(stream);
            let mut framed = TokioFramed::new(SyncIo(Mutex::new(stream)));
            let expected = ironrdp_core::encode_vec(&ironrdp_pdu::x224::X224(
                ironrdp_pdu::nego::ConnectionConfirm::Response {
                    flags: ironrdp_pdu::nego::ResponseFlags::empty(),
                    protocol: SecurityProtocol::SSL,
                },
            ))
            .unwrap();
            let ((), received) = tokio::join!(
                async {
                    // A complete fast-path frame followed by a fragmented X.224 frame.
                    peer.write_all(&[0, 2]).await.unwrap();
                    peer.write_all(&expected[..3]).await.unwrap();
                    tokio::task::yield_now().await;
                    peer.write_all(&expected[3..]).await.unwrap();
                },
                read_frame(&mut framed, Some(&ironrdp_pdu::X224_HINT))
            );
            let (action, frame) = received.unwrap();
            assert_eq!(action, ironrdp_pdu::Action::X224);
            assert_eq!(&frame[..], &expected);
        })
        .await
        .unwrap();
    }
}
