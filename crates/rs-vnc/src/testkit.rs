//! In-memory scripted RFB server for tests.

use std::{
    collections::VecDeque,
    io,
    sync::{Arc, Mutex},
    time::Duration,
};

use flate2::{Compress, Compression, FlushCompress};
use tokio::{
    io::{self as tokio_io, AsyncRead, AsyncReadExt, AsyncWrite, AsyncWriteExt},
    net::TcpListener,
    sync::{mpsc, Notify},
    task::JoinHandle,
};

use crate::{
    auth::{
        ard::{ard_decrypt, ard_server_public, encode_ard_params, ArdServerParams},
        vnc::vnc_auth_verify,
    },
    proto::{self, ClientMsg, ServerInit, Version},
    surface::Screen,
    types::{encoding, security, PixelFormat, Rect, VncError},
};

/// Parameters used to make the deterministic ARD test servers.
#[derive(Debug, Clone)]
pub struct ArdTestParams {
    pub generator: u16,
    pub prime: Vec<u8>,
    pub server_private: Vec<u8>,
}

impl ArdTestParams {
    /// The small test group p = 2^64 - 59, g = 2.
    pub fn small() -> Self {
        Self {
            generator: 2,
            prime: vec![0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0xc5],
            server_private: vec![0x0f, 0xed, 0xcb, 0xa9, 0x87, 0x65, 0x43, 0x21],
        }
    }

    /// RFC 2409 Oakley group 2 (1024 bit), g = 2.
    pub fn oakley2() -> Self {
        Self {
            generator: 2,
            prime: hex_bytes(
                "FFFFFFFFFFFFFFFFC90FDAA22168C234C4C6628B80DC1CD129024E088A67CC74020BBEA63B139B22514A08798E3404DDEF9519B3CD3A431B302B0A6DF25F14374FE1356D6D51C245E485B576625E7EC6F44C42E9A637ED6B0BFF5CB6F406B7EDEE386BFB5A899FA5AE9F24117C4B1FE649286651ECE65381FFFFFFFFFFFFFFFF",
            ),
            server_private: hex_bytes(
                "FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210FEDCBA9876543210",
            ),
        }
    }
}

/// Authentication behavior for a [`ServerScript`].
#[derive(Debug, Clone)]
pub enum AuthCheck {
    /// Accept any response. For ARD, the credentials still have to decrypt.
    Any,
    VncPassword(String),
    Ard {
        user: String,
        password: String,
        params: ArdTestParams,
    },
    /// Reject authentication, optionally with a custom RFB 3.8 reason.
    AlwaysFail(Option<String>),
}

/// Security types offered by a scripted server, or a pre-security refusal.
#[derive(Debug, Clone)]
pub enum SecurityPlan {
    Offer { types: Vec<u8>, check: AuthCheck },
    Refuse(String),
}

/// A rectangle or pseudo-rectangle in a scripted framebuffer update.
#[derive(Debug, Clone)]
pub enum TestRect {
    Raw {
        rect: Rect,
        bgra: Vec<u8>,
    },
    Solid {
        rect: Rect,
        bgra: [u8; 4],
    },
    CopyRect {
        rect: Rect,
        src_x: u16,
        src_y: u16,
    },
    Zrle {
        rect: Rect,
        bgra: Vec<u8>,
    },
    Cursor {
        hot_x: u16,
        hot_y: u16,
        w: u16,
        h: u16,
        bgra: Vec<u8>,
    },
    PointerPos {
        x: u16,
        y: u16,
    },
    DesktopSize {
        w: u16,
        h: u16,
    },
    /// `reason` and `status` travel in the header's x and y.
    ExtendedDesktopSize {
        reason: u16,
        status: u16,
        w: u16,
        h: u16,
        screens: Vec<Screen>,
    },
    LastRect,
    Bytes(Vec<u8>),
}

/// An action performed by the fake server.
#[derive(Debug, Clone)]
pub enum Step {
    Update(Vec<TestRect>),
    Bell,
    CutText(Vec<u8>),
    Close,
}

/// Complete server behavior for one connection.
#[derive(Debug, Clone)]
pub struct ServerScript {
    pub version: [u8; 12],
    pub security: SecurityPlan,
    pub challenge: [u8; 16],
    pub width: u16,
    pub height: u16,
    pub name: String,
    pub pixel_format: PixelFormat,
    pub steps: Vec<Step>,
}

impl ServerScript {
    pub fn new(width: u16, height: u16) -> Self {
        Self {
            version: *b"RFB 003.008\n",
            security: SecurityPlan::Offer {
                types: vec![security::NONE],
                check: AuthCheck::Any,
            },
            challenge: std::array::from_fn(|index| index as u8),
            width,
            height,
            name: "fake".to_owned(),
            pixel_format: PixelFormat::BGRX32,
            steps: Vec::new(),
        }
    }
}

impl Default for ServerScript {
    fn default() -> Self {
        Self::new(1, 1)
    }
}

/// A snapshot of the fake server's observable state.
#[derive(Debug, Clone, Default)]
pub struct FakeLog {
    pub client_version: Option<[u8; 12]>,
    pub chosen_security: Option<u8>,
    pub auth_ok: Option<bool>,
    pub ard_credentials: Option<(String, String)>,
    pub shared: Option<bool>,
    pub msgs: Vec<ClientMsg>,
    pub error: Option<String>,
    pub closed: bool,
}

/// Control and observation handle for one fake server connection.
pub struct FakeHandle {
    log: Arc<Mutex<FakeLog>>,
    notify: Arc<Notify>,
    steps: mpsc::UnboundedSender<Step>,
    join: JoinHandle<FakeLog>,
}

impl FakeHandle {
    pub fn log(&self) -> FakeLog {
        snapshot(&self.log)
    }

    /// Wait for a predicate to become true, waking on every log mutation.
    pub async fn wait_for<F>(&self, pred: F, timeout: Duration) -> bool
    where
        F: Fn(&FakeLog) -> bool,
    {
        let deadline = tokio::time::Instant::now() + timeout;
        loop {
            let notified = self.notify.notified();
            let current = snapshot(&self.log);
            if pred(&current) {
                return true;
            }
            if tokio::time::timeout_at(deadline, notified).await.is_err() {
                return pred(&snapshot(&self.log));
            }
        }
    }

    /// Append one action to the server's queue.
    pub fn push(&self, step: Step) {
        let _ = self.steps.send(step);
    }

    /// Wait for the server task to finish and return its final log.
    pub async fn finished(self) -> FakeLog {
        let FakeHandle { log, join, .. } = self;
        match join.await {
            Ok(final_log) => final_log,
            Err(_) => snapshot(&log),
        }
    }
}

/// Start a fake RFB server over an arbitrary tokio stream.
pub fn spawn<S>(stream: S, script: ServerScript) -> FakeHandle
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let log = Arc::new(Mutex::new(FakeLog::default()));
    let notify = Arc::new(Notify::new());
    let (steps, step_rx) = mpsc::unbounded_channel();
    let task_log = Arc::clone(&log);
    let task_notify = Arc::clone(&notify);
    let join = tokio::spawn(async move {
        run_server(
            stream,
            script,
            task_log.clone(),
            task_notify.clone(),
            step_rx,
        )
        .await;
        snapshot(&task_log)
    });
    FakeHandle {
        log,
        notify,
        steps,
        join,
    }
}

/// Make an in-memory RFB connection and start its server side.
pub fn pair(script: ServerScript) -> (tokio_io::DuplexStream, FakeHandle) {
    let (server, client) = tokio_io::duplex(1 << 20);
    (client, spawn(server, script))
}

/// Listen on localhost and run the selected script for each accepted client.
pub async fn listen_tcp(
    scripts: Vec<ServerScript>,
) -> io::Result<(std::net::SocketAddr, mpsc::UnboundedReceiver<FakeHandle>)> {
    if scripts.is_empty() {
        return Err(io::Error::new(
            io::ErrorKind::InvalidInput,
            "at least one server script is required",
        ));
    }
    let listener = TcpListener::bind(("127.0.0.1", 0)).await?;
    let address = listener.local_addr()?;
    let (handles, receiver) = mpsc::unbounded_channel();
    tokio::spawn(async move {
        let mut index = 0usize;
        loop {
            let accepted = listener.accept().await;
            let (stream, _) = match accepted {
                Ok(value) => value,
                Err(_) => break,
            };
            let script_index = index.min(scripts.len() - 1);
            index = index.saturating_add(1);
            let handle = spawn(stream, scripts[script_index].clone());
            if handles.send(handle).is_err() {
                break;
            }
        }
    });
    Ok((address, receiver))
}

async fn run_server<S>(
    mut stream: S,
    script: ServerScript,
    log: Arc<Mutex<FakeLog>>,
    notify: Arc<Notify>,
    step_rx: mpsc::UnboundedReceiver<Step>,
) where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let handshake = handshake(&mut stream, &script, &log, &notify).await;
    let (_version, pixel_format) = match handshake {
        Ok(Some(value)) => value,
        Ok(None) => {
            let _ = stream.shutdown().await;
            mark_closed(&log, &notify);
            return;
        }
        Err(error) => {
            set_error(&log, &notify, error);
            let _ = stream.shutdown().await;
            mark_closed(&log, &notify);
            return;
        }
    };

    run_messages(
        stream,
        MessageContext {
            pixel_format,
            fb_width: script.width,
            fb_height: script.height,
            initial_steps: script.steps,
            step_rx,
            log,
            notify,
        },
    )
    .await;
}

async fn handshake<S>(
    stream: &mut S,
    script: &ServerScript,
    log: &Arc<Mutex<FakeLog>>,
    notify: &Arc<Notify>,
) -> Result<Option<(Version, PixelFormat)>, String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    stream.write_all(&script.version).await.map_err(io_error)?;
    let mut client_version = [0u8; 12];
    stream
        .read_exact(&mut client_version)
        .await
        .map_err(io_error)?;
    modify_log(log, notify, |entry| {
        entry.client_version = Some(client_version)
    });
    let version =
        proto::parse_server_version(&client_version).map_err(|error| error.to_string())?;

    let (chosen, check) = match &script.security {
        SecurityPlan::Refuse(reason) => {
            send_refusal(stream, version, reason).await?;
            return Ok(None);
        }
        SecurityPlan::Offer { types, check } => {
            if types.is_empty() {
                return Err("empty security offer".to_owned());
            }
            if version == Version::V3_3 {
                let chosen = types[0];
                stream
                    .write_all(&proto::encode_security_type_33(u32::from(chosen)))
                    .await
                    .map_err(io_error)?;
                (chosen, check)
            } else {
                stream
                    .write_all(&proto::encode_security_types(types))
                    .await
                    .map_err(io_error)?;
                let chosen = stream.read_u8().await.map_err(io_error)?;
                modify_log(log, notify, |entry| entry.chosen_security = Some(chosen));
                if !types.contains(&chosen) {
                    return Err(format!("client chose {chosen}"));
                }
                (chosen, check)
            }
        }
    };
    modify_log(log, notify, |entry| entry.chosen_security = Some(chosen));

    let (auth_ok, auth_reason) = authenticate(stream, chosen, check, script, log, notify).await?;
    modify_log(log, notify, |entry| entry.auth_ok = Some(auth_ok));
    if proto::needs_security_result(version, chosen) {
        let reason = if auth_ok || version != Version::V3_8 {
            None
        } else {
            Some(auth_reason.as_deref().unwrap_or("Authentication failed"))
        };
        stream
            .write_all(&proto::encode_security_result(auth_ok, reason))
            .await
            .map_err(io_error)?;
    }
    if !auth_ok {
        return Ok(None);
    }

    let shared = stream.read_u8().await.map_err(io_error)? != 0;
    modify_log(log, notify, |entry| entry.shared = Some(shared));
    let server_init = ServerInit {
        width: script.width,
        height: script.height,
        pixel_format: script.pixel_format,
        name: script.name.clone(),
    };
    stream
        .write_all(&proto::encode_server_init(&server_init))
        .await
        .map_err(io_error)?;
    Ok(Some((version, script.pixel_format)))
}

async fn send_refusal<S>(stream: &mut S, version: Version, reason: &str) -> Result<(), String>
where
    S: AsyncWrite + Unpin,
{
    if version == Version::V3_3 {
        stream
            .write_all(&proto::encode_security_type_33(0))
            .await
            .map_err(io_error)?;
    } else {
        stream.write_all(&[0]).await.map_err(io_error)?;
    }
    stream
        .write_all(&proto::encode_reason(reason))
        .await
        .map_err(io_error)
}

async fn authenticate<S>(
    stream: &mut S,
    chosen: u8,
    check: &AuthCheck,
    script: &ServerScript,
    log: &Arc<Mutex<FakeLog>>,
    notify: &Arc<Notify>,
) -> Result<(bool, Option<String>), String>
where
    S: AsyncRead + AsyncWrite + Unpin,
{
    match chosen {
        security::NONE => Ok((true, None)),
        security::VNC_AUTH => {
            stream
                .write_all(&script.challenge)
                .await
                .map_err(io_error)?;
            let mut response = [0u8; 16];
            stream.read_exact(&mut response).await.map_err(io_error)?;
            let ok = match check {
                AuthCheck::Any => true,
                AuthCheck::VncPassword(password) => {
                    vnc_auth_verify(password, &script.challenge, &response)
                }
                AuthCheck::Ard { .. } | AuthCheck::AlwaysFail(_) => false,
            };
            let reason = if ok {
                None
            } else {
                Some(auth_failure_reason(check))
            };
            Ok((ok, reason))
        }
        security::ARD => {
            let test_params = match check {
                AuthCheck::Ard { params, .. } => params.clone(),
                _ => ArdTestParams::small(),
            };
            let key_len = test_params.prime.len();
            let key_len_u16 = u16::try_from(key_len)
                .map_err(|_| "ARD prime is longer than the wire length".to_owned())?;
            if key_len == 0 || key_len_u16 == 0 {
                return Err("ARD prime is empty".to_owned());
            }
            let server_public = ard_server_public(
                test_params.generator,
                &test_params.prime,
                &test_params.server_private,
            );
            if server_public.len() != key_len || server_public.iter().all(|byte| *byte == 0) {
                return Err("invalid ARD server public value".to_owned());
            }
            let params = ArdServerParams {
                generator: test_params.generator,
                key_len: key_len_u16,
                prime: test_params.prime,
                server_public,
            };
            stream
                .write_all(&encode_ard_params(&params))
                .await
                .map_err(io_error)?;
            let response_len = 128usize
                .checked_add(key_len)
                .ok_or_else(|| "ARD response length overflow".to_owned())?;
            let mut response = vec![0u8; response_len];
            stream.read_exact(&mut response).await.map_err(io_error)?;
            let credentials = ard_decrypt(&params, &test_params.server_private, &response).ok();
            if let Some(credentials) = &credentials {
                modify_log(log, notify, |entry| {
                    entry.ard_credentials = Some(credentials.clone())
                });
            }
            let ok = match check {
                AuthCheck::Any => credentials.is_some(),
                AuthCheck::Ard { user, password, .. } => {
                    credentials
                        .as_ref()
                        .is_some_and(|(actual_user, actual_password)| {
                            actual_user == user && actual_password == password
                        })
                }
                AuthCheck::VncPassword(_) | AuthCheck::AlwaysFail(_) => false,
            };
            let reason = if ok {
                None
            } else {
                Some(auth_failure_reason(check))
            };
            Ok((ok, reason))
        }
        other => {
            modify_log(log, notify, |entry| {
                entry.error = Some(format!("unsupported security type {other}"))
            });
            Err(format!("unsupported security type {other}"))
        }
    }
}

fn auth_failure_reason(check: &AuthCheck) -> String {
    match check {
        AuthCheck::AlwaysFail(Some(reason)) => reason.clone(),
        _ => "Authentication failed".to_owned(),
    }
}

struct MessageContext {
    pixel_format: PixelFormat,
    fb_width: u16,
    fb_height: u16,
    initial_steps: Vec<Step>,
    step_rx: mpsc::UnboundedReceiver<Step>,
    log: Arc<Mutex<FakeLog>>,
    notify: Arc<Notify>,
}

async fn run_messages<S>(stream: S, context: MessageContext)
where
    S: AsyncRead + AsyncWrite + Unpin + Send + 'static,
{
    let MessageContext {
        pixel_format,
        fb_width,
        fb_height,
        initial_steps,
        step_rx,
        log,
        notify,
    } = context;
    let mut pixel_format = pixel_format;
    let mut fb_width = fb_width;
    let mut fb_height = fb_height;
    let mut step_rx = step_rx;
    let (mut reader, mut writer) = tokio_io::split(stream);
    let (msg_tx, mut msg_rx) = mpsc::unbounded_channel();
    let reader_task = tokio::spawn(async move {
        loop {
            match proto::read_client_msg(&mut reader).await {
                Ok(message) => {
                    if msg_tx.send(Ok(message)).is_err() {
                        break;
                    }
                }
                Err(error) => {
                    let _ = msg_tx.send(Err(error));
                    break;
                }
            }
        }
    });

    let mut steps = VecDeque::from(initial_steps);
    let mut pending_requests = 0usize;
    let mut steps_open = true;
    let mut compressor = Compress::new(Compression::default(), true);
    let mut done = false;

    while !done {
        loop {
            let action = match steps.front() {
                Some(Step::Bell) => Some(Immediate::Bell),
                Some(Step::CutText(_)) => Some(Immediate::CutText),
                Some(Step::Close) => Some(Immediate::Close),
                _ => None,
            };
            match action {
                Some(Immediate::Bell) => {
                    steps.pop_front();
                    if let Err(error) = writer.write_all(&proto::encode_bell()).await {
                        set_error(&log, &notify, io_error(error));
                        done = true;
                        break;
                    }
                }
                Some(Immediate::CutText) => {
                    let text = match steps.pop_front() {
                        Some(Step::CutText(text)) => text,
                        _ => Vec::new(),
                    };
                    if let Err(error) = writer
                        .write_all(&proto::encode_server_cut_text(&text))
                        .await
                    {
                        set_error(&log, &notify, io_error(error));
                        done = true;
                        break;
                    }
                }
                Some(Immediate::Close) => {
                    steps.pop_front();
                    done = true;
                    break;
                }
                None => break,
            }
        }
        if done {
            break;
        }

        if pending_requests != 0 {
            if let Some(Step::Update(_)) = steps.front() {
                let update = match steps.pop_front() {
                    Some(Step::Update(rects)) => rects,
                    _ => Vec::new(),
                };
                let encoded = encode_update(
                    &update,
                    pixel_format,
                    &mut compressor,
                    &mut fb_width,
                    &mut fb_height,
                );
                match encoded {
                    Ok(bytes) => {
                        if let Err(error) = writer.write_all(&bytes).await {
                            set_error(&log, &notify, io_error(error));
                            break;
                        }
                        pending_requests -= 1;
                        continue;
                    }
                    Err(error) => {
                        set_error(&log, &notify, error);
                        break;
                    }
                }
            }
        }

        if !steps_open && msg_rx.is_closed() {
            break;
        }
        tokio::select! {
            biased;
            message = msg_rx.recv() => {
                match message {
                    Some(Ok(message)) => {
                        let request = match &message {
                            ClientMsg::SetPixelFormat(format) => {
                                pixel_format = *format;
                                false
                            }
                            ClientMsg::FramebufferUpdateRequest { .. } => true,
                            _ => false,
                        };
                        modify_log(&log, &notify, |entry| entry.msgs.push(message));
                        if request {
                            pending_requests = pending_requests.saturating_add(1);
                        }
                    }
                    Some(Err(error)) => {
                        if !matches!(error, VncError::Closed(_)) {
                            set_error(&log, &notify, format!("client: {error}"));
                        }
                        done = true;
                    }
                    None => {
                        done = true;
                    }
                }
            }
            step = step_rx.recv(), if steps_open => {
                match step {
                    Some(step) => steps.push_back(step),
                    None => steps_open = false,
                }
            }
        }
    }

    reader_task.abort();
    let _ = reader_task.await;
    if let Err(error) = writer.shutdown().await {
        set_error(&log, &notify, io_error(error));
    }
    mark_closed(&log, &notify);
}

#[derive(Clone, Copy)]
enum Immediate {
    Bell,
    CutText,
    Close,
}

fn encode_update(
    rects: &[TestRect],
    pixel_format: PixelFormat,
    compressor: &mut Compress,
    fb_width: &mut u16,
    fb_height: &mut u16,
) -> Result<Vec<u8>, String> {
    let last_rect = rects
        .iter()
        .position(|rect| matches!(rect, TestRect::LastRect));
    if let Some(index) = last_rect {
        if index + 1 != rects.len() {
            return Err("LastRect must be the final rectangle".to_owned());
        }
    }
    let count = match last_rect {
        Some(_) => 0xffff,
        None => {
            u16::try_from(rects.len()).map_err(|_| "too many framebuffer rectangles".to_owned())?
        }
    };
    let mut output = proto::encode_fb_update_header(count).to_vec();
    for rect in rects {
        let (encoded, new_size) = encode_rect(rect, pixel_format, compressor)?;
        output.extend_from_slice(&encoded);
        if let Some((width, height)) = new_size {
            *fb_width = width;
            *fb_height = height;
        }
    }
    Ok(output)
}

type EncodedRect = (Vec<u8>, Option<(u16, u16)>);

fn encode_rect(
    rect: &TestRect,
    pixel_format: PixelFormat,
    compressor: &mut Compress,
) -> Result<EncodedRect, String> {
    match rect {
        TestRect::Raw { rect, bgra } => {
            let payload = encode_bgra(pixel_format, rect, bgra)?;
            Ok((with_header(*rect, encoding::RAW, payload), None))
        }
        TestRect::Solid { rect, bgra } => {
            let pixels = pixel_count(*rect)?;
            let encoded_pixel = encode_pixel(pixel_format, bgra)?;
            let byte_count = pixels
                .checked_mul(encoded_pixel.len())
                .ok_or_else(|| "solid rectangle size overflow".to_owned())?;
            let mut payload = Vec::with_capacity(byte_count);
            for _ in 0..pixels {
                payload.extend_from_slice(&encoded_pixel);
            }
            Ok((with_header(*rect, encoding::RAW, payload), None))
        }
        TestRect::CopyRect { rect, src_x, src_y } => {
            let mut payload = Vec::with_capacity(4);
            payload.extend_from_slice(&src_x.to_be_bytes());
            payload.extend_from_slice(&src_y.to_be_bytes());
            Ok((with_header(*rect, encoding::COPY_RECT, payload), None))
        }
        TestRect::Zrle { rect, bgra } => {
            let payload = encode_zrle(pixel_format, *rect, bgra, compressor)?;
            Ok((with_header(*rect, encoding::ZRLE, payload), None))
        }
        TestRect::Cursor {
            hot_x,
            hot_y,
            w,
            h,
            bgra,
        } => {
            let rect = Rect::new(*hot_x, *hot_y, *w, *h);
            let mut payload = encode_bgra(pixel_format, &rect, bgra)?;
            let pixels = pixel_count(rect)?;
            let mask_stride = usize::from(*w).div_ceil(8);
            let mask_len = mask_stride
                .checked_mul(usize::from(*h))
                .ok_or_else(|| "cursor mask size overflow".to_owned())?;
            payload.reserve(mask_len);
            for y in 0..usize::from(*h) {
                let row_start = y
                    .checked_mul(usize::from(*w))
                    .and_then(|value| value.checked_mul(4))
                    .ok_or_else(|| "cursor pixel offset overflow".to_owned())?;
                for byte_index in 0..mask_stride {
                    let mut mask = 0u8;
                    for bit in 0..8 {
                        let x = byte_index * 8 + bit;
                        if x >= usize::from(*w) {
                            break;
                        }
                        let alpha = bgra[row_start + x * 4 + 3];
                        if alpha != 0 {
                            mask |= 1 << (7 - bit);
                        }
                    }
                    payload.push(mask);
                }
            }
            if pixels == 0 && !payload.is_empty() {
                return Err("empty cursor has a payload".to_owned());
            }
            Ok((with_header(rect, encoding::CURSOR, payload), None))
        }
        TestRect::PointerPos { x, y } => Ok((
            with_header(Rect::new(*x, *y, 0, 0), encoding::POINTER_POS, Vec::new()),
            None,
        )),
        TestRect::DesktopSize { w, h } => Ok((
            with_header(Rect::new(0, 0, *w, *h), encoding::DESKTOP_SIZE, Vec::new()),
            Some((*w, *h)),
        )),
        TestRect::ExtendedDesktopSize {
            reason,
            status,
            w,
            h,
            screens,
        } => {
            let count = u8::try_from(screens.len()).map_err(|_| "too many screens".to_owned())?;
            let mut payload = vec![count, 0, 0, 0];
            for screen in screens {
                payload.extend_from_slice(&screen.id.to_be_bytes());
                for value in [screen.rect.x, screen.rect.y, screen.rect.w, screen.rect.h] {
                    payload.extend_from_slice(&value.to_be_bytes());
                }
                payload.extend_from_slice(&screen.flags.to_be_bytes());
            }
            Ok((
                with_header(
                    Rect::new(*reason, *status, *w, *h),
                    encoding::EXTENDED_DESKTOP_SIZE,
                    payload,
                ),
                (*status == 0).then_some((*w, *h)),
            ))
        }
        TestRect::LastRect => Ok((
            with_header(Rect::default(), encoding::LAST_RECT, Vec::new()),
            None,
        )),
        TestRect::Bytes(bytes) => Ok((bytes.clone(), None)),
    }
}

fn with_header(rect: Rect, encoding_number: i32, payload: Vec<u8>) -> Vec<u8> {
    let mut output = proto::encode_rect_header(&proto::RectHeader {
        rect,
        encoding: encoding_number,
    })
    .to_vec();
    output.extend_from_slice(&payload);
    output
}

fn pixel_count(rect: Rect) -> Result<usize, String> {
    usize::from(rect.w)
        .checked_mul(usize::from(rect.h))
        .ok_or_else(|| "rectangle size overflow".to_owned())
}

fn encode_bgra(pixel_format: PixelFormat, rect: &Rect, bgra: &[u8]) -> Result<Vec<u8>, String> {
    let pixels = pixel_count(*rect)?;
    let expected = pixels
        .checked_mul(4)
        .ok_or_else(|| "rectangle pixel size overflow".to_owned())?;
    if bgra.len() != expected {
        return Err(format!("BGRA length {}, expected {expected}", bgra.len()));
    }
    let bytes_per_pixel = validate_pixel_format(pixel_format)?;
    let capacity = pixels
        .checked_mul(bytes_per_pixel)
        .ok_or_else(|| "encoded pixel size overflow".to_owned())?;
    let mut output = Vec::with_capacity(capacity);
    for pixel in bgra.chunks_exact(4) {
        let pixel: [u8; 4] = match pixel.try_into() {
            Ok(value) => value,
            Err(_) => return Err("invalid BGRA pixel".to_owned()),
        };
        output.extend_from_slice(&encode_pixel(pixel_format, &pixel)?);
    }
    Ok(output)
}

fn validate_pixel_format(pixel_format: PixelFormat) -> Result<usize, String> {
    let bytes_per_pixel = match pixel_format.bits_per_pixel {
        8 => 1,
        16 => 2,
        32 => 4,
        bits => return Err(format!("unsupported pixel format bpp {bits}")),
    };
    if !pixel_format.true_colour {
        return Err("indexed pixel formats are unsupported".to_owned());
    }
    for (max, shift) in [
        (pixel_format.red_max, pixel_format.red_shift),
        (pixel_format.green_max, pixel_format.green_shift),
        (pixel_format.blue_max, pixel_format.blue_shift),
    ] {
        if max == 0 {
            return Err("invalid true-colour pixel format".to_owned());
        }
        let bits = u32::from(max).ilog2() + 1;
        if u32::from(shift) + bits > u32::from(pixel_format.bits_per_pixel) {
            return Err("invalid true-colour pixel format".to_owned());
        }
    }
    Ok(bytes_per_pixel)
}

fn encode_pixel(pixel_format: PixelFormat, bgra: &[u8; 4]) -> Result<Vec<u8>, String> {
    let bytes_per_pixel = validate_pixel_format(pixel_format)?;
    let red = (u32::from(bgra[2]) * u32::from(pixel_format.red_max)) / 255;
    let green = (u32::from(bgra[1]) * u32::from(pixel_format.green_max)) / 255;
    let blue = (u32::from(bgra[0]) * u32::from(pixel_format.blue_max)) / 255;
    let value = (red << pixel_format.red_shift)
        | (green << pixel_format.green_shift)
        | (blue << pixel_format.blue_shift);
    let bytes = if pixel_format.big_endian {
        let bytes = value.to_be_bytes();
        bytes[4 - bytes_per_pixel..].to_vec()
    } else {
        let bytes = value.to_le_bytes();
        bytes[..bytes_per_pixel].to_vec()
    };
    Ok(bytes)
}

fn encode_zrle(
    pixel_format: PixelFormat,
    rect: Rect,
    bgra: &[u8],
    compressor: &mut Compress,
) -> Result<Vec<u8>, String> {
    if pixel_format != PixelFormat::BGRX32 {
        return Err("ZRLE requires BGRX32 pixel format".to_owned());
    }
    let pixels = pixel_count(rect)?;
    let expected = pixels
        .checked_mul(4)
        .ok_or_else(|| "ZRLE pixel size overflow".to_owned())?;
    if bgra.len() != expected {
        return Err(format!("BGRA length {}, expected {expected}", bgra.len()));
    }
    let width = usize::from(rect.w);
    let height = usize::from(rect.h);
    let mut plain = Vec::new();
    let mut tile_y = 0usize;
    while tile_y < height {
        let tile_h = (height - tile_y).min(64);
        let mut tile_x = 0usize;
        while tile_x < width {
            let tile_w = (width - tile_x).min(64);
            let first_offset = (tile_y * width + tile_x) * 4;
            let first = [
                bgra[first_offset],
                bgra[first_offset + 1],
                bgra[first_offset + 2],
            ];
            let mut solid = true;
            'rows: for y in 0..tile_h {
                for x in 0..tile_w {
                    let offset = ((tile_y + y) * width + tile_x + x) * 4;
                    if bgra[offset..offset + 3] != first {
                        solid = false;
                        break 'rows;
                    }
                }
            }
            if solid {
                plain.push(1);
                plain.extend_from_slice(&first);
            } else {
                plain.push(0);
                for y in 0..tile_h {
                    for x in 0..tile_w {
                        let offset = ((tile_y + y) * width + tile_x + x) * 4;
                        plain.extend_from_slice(&bgra[offset..offset + 3]);
                    }
                }
            }
            tile_x += tile_w;
        }
        tile_y += tile_h;
    }
    let mut compressed = Vec::with_capacity(
        plain
            .len()
            .checked_add(plain.len() / 100)
            .and_then(|length| length.checked_add(128))
            .ok_or_else(|| "ZRLE compressed capacity overflow".to_owned())?,
    );
    let mut consumed = 0usize;
    while let flate2::Status::BufError = {
        let before = compressor.total_in();
        let status = compressor
            .compress_vec(&plain[consumed..], &mut compressed, FlushCompress::Sync)
            .map_err(|error| format!("ZRLE compression failed: {error}"))?;
        let added = compressor
            .total_in()
            .saturating_sub(before)
            .try_into()
            .map_err(|_| "ZRLE input length overflow".to_owned())?;
        consumed = consumed
            .checked_add(added)
            .ok_or_else(|| "ZRLE input length overflow".to_owned())?;
        status
    } {
        let additional = compressed
            .capacity()
            .max(128)
            .checked_add(1)
            .ok_or_else(|| "ZRLE compressed capacity overflow".to_owned())?;
        compressed.reserve(additional);
    }
    let compressed_len = u32::try_from(compressed.len())
        .map_err(|_| "ZRLE payload exceeds u32 length".to_owned())?;
    let mut payload = compressed_len.to_be_bytes().to_vec();
    payload.extend_from_slice(&compressed);
    Ok(payload)
}

fn modify_log<F>(log: &Arc<Mutex<FakeLog>>, notify: &Arc<Notify>, update: F)
where
    F: FnOnce(&mut FakeLog),
{
    match log.lock() {
        Ok(mut entry) => update(&mut entry),
        Err(poisoned) => update(&mut poisoned.into_inner()),
    }
    notify.notify_waiters();
}

fn set_error(log: &Arc<Mutex<FakeLog>>, notify: &Arc<Notify>, error: String) {
    modify_log(log, notify, |entry| {
        if entry.error.is_none() {
            entry.error = Some(error);
        }
    });
}

fn mark_closed(log: &Arc<Mutex<FakeLog>>, notify: &Arc<Notify>) {
    modify_log(log, notify, |entry| entry.closed = true);
}

fn snapshot(log: &Arc<Mutex<FakeLog>>) -> FakeLog {
    match log.lock() {
        Ok(entry) => entry.clone(),
        Err(poisoned) => poisoned.into_inner().clone(),
    }
}

fn io_error(error: io::Error) -> String {
    format!("io: {error}")
}

fn hex_bytes(value: &str) -> Vec<u8> {
    let bytes = value.as_bytes();
    let mut output = Vec::with_capacity(bytes.len() / 2);
    for pair in bytes.chunks_exact(2) {
        let high = hex_nibble(pair[0]);
        let low = hex_nibble(pair[1]);
        output.push((high << 4) | low);
    }
    output
}

fn hex_nibble(value: u8) -> u8 {
    match value {
        b'0'..=b'9' => value - b'0',
        b'a'..=b'f' => value - b'a' + 10,
        b'A'..=b'F' => value - b'A' + 10,
        _ => 0,
    }
}
