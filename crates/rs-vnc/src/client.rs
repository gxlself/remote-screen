//! RFB client state machine.

use async_trait::async_trait;
use std::time::Duration;
use tokio::{
    io::{split, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    sync::{mpsc, watch},
    time::{Instant, MissedTickBehavior},
};
use tracing::warn;
use zeroize::Zeroizing;

use crate::{
    auth::{ard, vnc},
    encodings::{
        basic::{
            apply_desktop_size, apply_extended_desktop_size, apply_pointer_pos, cursor_len,
            decode_copy_rect, decode_cursor, decode_raw, extended_desktop_size_len, raw_len,
        },
        zrle::ZrleDecoder,
    },
    proto::{
        self, choose_security, client_init, client_version_bytes, from_latin1,
        needs_security_result, parse_server_version, read_rect_header, read_security_result,
        read_security_types, read_server_init, read_server_msg_header, ClientMsg, ServerInit,
        ServerMsgHeader,
    },
    surface::Surface,
    types::{encoding, security, PixelFormat, Rect, VncError, MAX_CUT_TEXT, MAX_ZRLE_LEN},
};

pub use crate::proto::Version;

/// Repeat the negotiated encodings while idle, without requesting pixels.
pub const BACKGROUND_KEEPALIVE: Duration = Duration::from_secs(30);

/// Options that control the client handshake.
#[derive(Debug, Clone)]
pub struct ClientConfig {
    /// Whether the server should share the desktop with other clients.
    pub shared: bool,
}

impl Default for ClientConfig {
    fn default() -> Self {
        Self { shared: true }
    }
}

/// Credentials returned by a [`CredentialPrompt`].
pub struct AuthAnswer {
    pub user: String,
    pub password: Zeroizing<String>,
}

/// Asynchronous provider for credentials required by a security type.
#[async_trait]
pub trait CredentialPrompt: Send {
    /// Ask for credentials for VNC authentication (`2`) or Apple Remote
    /// Desktop authentication (`30`). Returning `None` cancels the handshake.
    async fn ask(&mut self, sec: u8) -> Option<AuthAnswer>;
}

/// A connection after the RFB security and ServerInit handshake completed.
pub struct Connected<S> {
    pub stream: S,
    pub version: Version,
    /// The server's banner version as sent, e.g. `003.889` for macOS Screen Sharing.
    pub server_version: String,
    pub security: u8,
    pub init: ServerInit,
}

/// Input sent from the caller to the remote desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientInput {
    Key {
        down: bool,
        keysym: u32,
    },
    Pointer {
        mask: u8,
        x: u16,
        y: u16,
    },
    /// The bytes are already encoded as Latin-1.
    CutText(Vec<u8>),
}

/// Events received from the remote desktop.
#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ClientEvent {
    Bell,
    CutText(String),
    Resized { w: u16, h: u16 },
}

/// Perform the RFB version, security, authentication, and ServerInit handshake.
pub async fn handshake<S>(
    mut stream: S,
    cfg: &ClientConfig,
    prompt: &mut dyn CredentialPrompt,
) -> Result<Connected<S>, VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send,
{
    let mut banner = [0u8; 12];
    stream.read_exact(&mut banner).await?;
    let version = parse_server_version(&banner)?;
    // parse_server_version checked these seven bytes are `ddd.ddd`.
    let server_version = String::from_utf8_lossy(&banner[4..11]).into_owned();
    stream.write_all(&client_version_bytes(version)).await?;

    let offered = read_security_types(&mut stream, version).await?;
    let security =
        choose_security(&offered).ok_or_else(|| VncError::UnsupportedSecurity(offered.clone()))?;

    if version != Version::V3_3 {
        stream.write_all(&[security]).await?;
    }

    match security {
        security::NONE => {}
        security::VNC_AUTH => {
            let answer = prompt
                .ask(security::VNC_AUTH)
                .await
                .ok_or(VncError::Cancelled)?;
            let challenge = vnc::read_challenge(&mut stream).await?;
            let response = vnc::vnc_auth_response(&answer.password, &challenge);
            stream.write_all(&response).await?;
        }
        security::ARD => {
            let answer = prompt.ask(security::ARD).await.ok_or(VncError::Cancelled)?;
            let params = ard::read_ard_params(&mut stream).await?;
            let response = ard::ard_response(&params, &answer.user, &answer.password)?;
            stream.write_all(&response).await?;
        }
        _ => {
            return Err(VncError::UnsupportedSecurity(vec![security]));
        }
    }

    if needs_security_result(version, security) {
        read_security_result(&mut stream, version).await?;
    }

    stream.write_all(&client_init(cfg.shared)).await?;
    let init = read_server_init(&mut stream).await?;
    Ok(Connected {
        stream,
        version,
        server_version,
        security,
        init,
    })
}

/// Run a connected RFB session until the input channel closes or the server
/// closes the connection.
pub async fn run<S>(
    conn: Connected<S>,
    surface: &mut dyn Surface,
    input: mpsc::Receiver<ClientInput>,
    events: mpsc::UnboundedSender<ClientEvent>,
) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let (_visibility, background) = watch::channel(false);
    run_with_background(conn, surface, input, events, background).await
}

pub async fn run_with_background<S>(
    conn: Connected<S>,
    surface: &mut dyn Surface,
    input: mpsc::Receiver<ClientInput>,
    events: mpsc::UnboundedSender<ClientEvent>,
    background: watch::Receiver<bool>,
) -> Result<(), VncError>
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let Connected { stream, init, .. } = conn;
    surface.resize(init.width, init.height);

    let (reader, writer) = split(stream);
    let (request_tx, request_rx) = mpsc::unbounded_channel();
    let writer_task = writer_loop(
        writer,
        input,
        request_rx,
        init.width,
        init.height,
        background,
    );
    tokio::pin!(writer_task);

    tokio::select! {
        biased;
        result = &mut writer_task => {
            result.map_err(|error| match error {
                VncError::Io(reason) => VncError::Closed(reason),
                error => error,
            })
        }
        result = read_loop(reader, surface, events, request_tx) => {
            result
        }
    }
}

type FramebufferRequest = (bool, Rect);

async fn writer_loop<W>(
    mut writer: W,
    mut input: mpsc::Receiver<ClientInput>,
    mut requests: mpsc::UnboundedReceiver<FramebufferRequest>,
    width: u16,
    height: u16,
    mut background: watch::Receiver<bool>,
) -> Result<(), VncError>
where
    W: AsyncWrite + Unpin,
{
    write_message(&mut writer, ClientMsg::SetPixelFormat(PixelFormat::BGRX32)).await?;
    write_message(
        &mut writer,
        ClientMsg::SetEncodings(encoding::PREFERRED.to_vec()),
    )
    .await?;
    let mut paused = *background.borrow_and_update();
    let mut rect = Rect::new(0, 0, width, height);
    if !paused {
        write_message(
            &mut writer,
            ClientMsg::FramebufferUpdateRequest {
                incremental: false,
                rect,
            },
        )
        .await?;
    }
    let mut visibility_open = true;
    let mut keepalive =
        tokio::time::interval_at(Instant::now() + BACKGROUND_KEEPALIVE, BACKGROUND_KEEPALIVE);
    keepalive.set_missed_tick_behavior(MissedTickBehavior::Delay);

    let mut requests_open = true;
    loop {
        tokio::select! {
            biased;
            changed = background.changed(), if visibility_open => {
                visibility_open = changed.is_ok();
                let next = *background.borrow_and_update();
                if paused != next {
                    paused = next;
                    // An update may have completed while this branch was waking.
                    // Consume its size, but never send its stale incremental request.
                    while let Ok((_, latest)) = requests.try_recv() {
                        rect = latest;
                    }
                    if paused {
                        keepalive.reset();
                    } else {
                        write_message(&mut writer, ClientMsg::FramebufferUpdateRequest { incremental: false, rect }).await?;
                    }
                }
            }
            _ = keepalive.tick(), if paused => {
                write_message(&mut writer, ClientMsg::SetEncodings(encoding::PREFERRED.to_vec())).await?;
            }
            input_message = input.recv() => {
                let Some(input_message) = input_message else {
                    return Ok(());
                };
                if *background.borrow() && !matches!(&input_message, ClientInput::CutText(_)) {
                    continue;
                }
                let message = match input_message {
                    ClientInput::Key { down, keysym } => ClientMsg::KeyEvent { down, keysym },
                    ClientInput::Pointer { mask, x, y } => ClientMsg::PointerEvent { mask, x, y },
                    ClientInput::CutText(text) => ClientMsg::ClientCutText(text),
                };
                match message.encode() {
                    Ok(bytes) => writer.write_all(&bytes).await?,
                    Err(error) => {
                        warn!(error = %error, "discarding invalid client input");
                    }
                }
            }
            request = requests.recv(), if requests_open => {
                match request {
                    Some((incremental, latest)) => {
                        rect = latest;
                        if *background.borrow() {
                            continue;
                        }
                        write_message(
                            &mut writer,
                            ClientMsg::FramebufferUpdateRequest { incremental, rect },
                        ).await?;
                    }
                    None => requests_open = false,
                }
            }
            else => return Ok(()),
        }
    }
}

async fn write_message<W>(writer: &mut W, message: ClientMsg) -> Result<(), VncError>
where
    W: AsyncWrite + Unpin,
{
    let bytes = message.encode()?;
    writer.write_all(&bytes).await?;
    Ok(())
}

async fn read_loop<R>(
    mut reader: R,
    surface: &mut dyn Surface,
    events: mpsc::UnboundedSender<ClientEvent>,
    requests: mpsc::UnboundedSender<FramebufferRequest>,
) -> Result<(), VncError>
where
    R: AsyncRead + Unpin,
{
    let mut zrle = ZrleDecoder::new();
    loop {
        match read_server_msg_header(&mut reader).await? {
            ServerMsgHeader::FramebufferUpdate { rects } => {
                let mut dirty = Vec::new();
                let mut resized = false;
                if rects == u16::MAX {
                    loop {
                        let header = read_rect_header(&mut reader).await?;
                        if header.encoding == encoding::LAST_RECT {
                            break;
                        }
                        decode_rect_message(
                            &mut reader,
                            header,
                            surface,
                            &mut zrle,
                            &events,
                            &mut dirty,
                            &mut resized,
                        )
                        .await?;
                    }
                } else {
                    for _ in 0..rects {
                        let header = read_rect_header(&mut reader).await?;
                        if header.encoding == encoding::LAST_RECT {
                            break;
                        }
                        decode_rect_message(
                            &mut reader,
                            header,
                            surface,
                            &mut zrle,
                            &events,
                            &mut dirty,
                            &mut resized,
                        )
                        .await?;
                    }
                }
                surface.commit(&dirty);
                let (width, height) = surface.size();
                if requests
                    .send((!resized, Rect::new(0, 0, width, height)))
                    .is_err()
                {
                    return Ok(());
                }
            }
            ServerMsgHeader::SetColourMapEntries { count, .. } => {
                let bytes = u64::from(count)
                    .checked_mul(6)
                    .ok_or_else(|| protocol("colour map length overflow"))?;
                proto::skip(&mut reader, bytes).await?;
            }
            ServerMsgHeader::Bell => {
                let _ = events.send(ClientEvent::Bell);
            }
            ServerMsgHeader::ServerCutText { len } => {
                if len > MAX_CUT_TEXT {
                    warn!(
                        len,
                        max = MAX_CUT_TEXT,
                        "discarding oversized server clipboard text"
                    );
                    proto::skip(&mut reader, u64::from(len)).await?;
                } else {
                    let mut text = vec![0u8; len as usize];
                    reader.read_exact(&mut text).await?;
                    let _ = events.send(ClientEvent::CutText(from_latin1(&text)));
                }
            }
        }
    }
}

async fn decode_rect_message<R>(
    reader: &mut R,
    header: proto::RectHeader,
    surface: &mut dyn Surface,
    zrle: &mut ZrleDecoder,
    events: &mpsc::UnboundedSender<ClientEvent>,
    dirty: &mut Vec<Rect>,
    resized: &mut bool,
) -> Result<(), VncError>
where
    R: AsyncRead + Unpin,
{
    let rect = header.rect;
    match header.encoding {
        encoding::RAW => {
            ensure_rect_fits(rect, surface)?;
            let length = raw_len(&PixelFormat::BGRX32, rect);
            let mut data = vec![0u8; length];
            reader.read_exact(&mut data).await?;
            decode_raw(&PixelFormat::BGRX32, rect, &data, surface)?;
            dirty.push(rect);
        }
        encoding::COPY_RECT => {
            ensure_rect_fits(rect, surface)?;
            let mut data = [0u8; 4];
            reader.read_exact(&mut data).await?;
            decode_copy_rect(rect, &data, surface)?;
            dirty.push(rect);
        }
        encoding::ZRLE => {
            ensure_rect_fits(rect, surface)?;
            let length = reader.read_u32().await?;
            if length > MAX_ZRLE_LEN {
                return Err(protocol("zrle payload too long"));
            }
            let mut data = vec![0u8; length as usize];
            reader.read_exact(&mut data).await?;
            zrle.decode(&PixelFormat::BGRX32, rect, &data, surface)?;
            dirty.push(rect);
        }
        encoding::CURSOR => {
            let length = cursor_len(&PixelFormat::BGRX32, rect.w, rect.h);
            let mut data = vec![0u8; length];
            reader.read_exact(&mut data).await?;
            decode_cursor(&PixelFormat::BGRX32, rect, &data, surface)?;
        }
        encoding::DESKTOP_SIZE => {
            let before = surface.size();
            apply_desktop_size(rect, surface)?;
            let after = surface.size();
            if before != after {
                *resized = true;
                let _ = events.send(ClientEvent::Resized {
                    w: after.0,
                    h: after.1,
                });
            }
        }
        encoding::EXTENDED_DESKTOP_SIZE => {
            let mut first = [0u8; 4];
            reader.read_exact(&mut first).await?;
            let length = extended_desktop_size_len(first[0]);
            let mut data = vec![0u8; length];
            data[..4].copy_from_slice(&first);
            reader.read_exact(&mut data[4..]).await?;
            let before = surface.size();
            apply_extended_desktop_size(rect, &data, surface)?;
            let after = surface.size();
            if before != after {
                *resized = true;
                let _ = events.send(ClientEvent::Resized {
                    w: after.0,
                    h: after.1,
                });
            }
        }
        encoding::POINTER_POS => {
            apply_pointer_pos(rect, surface);
        }
        encoding::LAST_RECT => {}
        other => {
            return Err(protocol(format!("unsupported encoding {other}")));
        }
    }
    Ok(())
}

fn ensure_rect_fits(rect: Rect, surface: &dyn Surface) -> Result<(), VncError> {
    let (width, height) = surface.size();
    if rect.fits(width, height) {
        Ok(())
    } else {
        Err(protocol("rect out of bounds"))
    }
}

fn protocol(message: impl Into<String>) -> VncError {
    VncError::Protocol(message.into())
}
