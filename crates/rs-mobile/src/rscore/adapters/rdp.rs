//! RDP driver over rs-rdp.

use std::sync::atomic::{AtomicU32, Ordering};
use std::sync::Arc;

use async_trait::async_trait;
use rs_rdp::client::{connect, RdpConnection};
use rs_rdp::desktop::landscape_desktop_size;
use rs_rdp::input::InputMapper;
use rs_rdp::scancode::ctrl_alt_end;
use rs_rdp::tls::fingerprint_hex;
use rs_rdp::{
    CertCheck, RdpAudioSink, RdpCommand, RdpConfig, RdpCredentials, RdpError, RdpInput, RdpSink,
    Rect,
};
use rsp_core::hid::{InputMsg, ScrollPhase, ScrollUnit, TouchPhase};
use tokio::sync::{mpsc, watch};

use super::driver::{
    AdapterError, Credentials, DriverHost, DriverInput, DriverParams, FrameSink, PcmSink,
    ProtocolDriver, SrcPixels,
};
use super::dto::{AdapterProtocol, AdapterScreen, CredField};

pub const MAX_AUTH_ATTEMPTS: u32 = 3;
pub const CLIENT_NAME: &str = "RemoteScreen";

const CRED_FIELDS: [CredField; 3] = [CredField::User, CredField::Password, CredField::Domain];
const COMMAND_QUEUE: usize = 256;

#[derive(Debug, Default)]
pub struct RdpDriver;

pub fn map_rdp_error(e: RdpError) -> AdapterError {
    match e {
        RdpError::Io(reason) => AdapterError::Io(reason),
        RdpError::Tls(reason) => AdapterError::Io(format!("tls: {reason}")),
        RdpError::CertRejected => AdapterError::CertRejected,
        RdpError::NlaRequired => AdapterError::NlaRequired,
        RdpError::UnsupportedSecurity(reason) => AdapterError::UnsupportedSecurity(reason),
        RdpError::AuthFailed => AdapterError::AuthFailed,
        RdpError::Protocol(reason) => AdapterError::Protocol(reason),
        RdpError::Closed(reason) => AdapterError::ServerClosed(reason),
        RdpError::Cancelled => AdapterError::Cancelled,
        // RdpDriver reconnects itself; this only surfaces if that changes.
        RdpError::Reconnect(monitors) => {
            AdapterError::Protocol(format!("reconnect with {monitors} monitors"))
        }
    }
}

/// Without a domain, "CORP\alice" → ("alice", Some("CORP")) and "\alice" →
/// ("alice", None). A filled-in domain or a name such as "alice@corp.example"
/// is kept as is. An empty domain counts as not filled in.
pub fn split_domain(user: &str, domain: Option<&str>) -> (String, Option<String>) {
    let domain = domain.filter(|domain| !domain.is_empty());
    if domain.is_none() {
        if let Some((prefix, name)) = user.split_once('\\') {
            return (
                name.to_owned(),
                (!prefix.is_empty()).then(|| prefix.to_owned()),
            );
        }
    }
    (user.to_owned(), domain.map(str::to_owned))
}

/// Built-in arrow for the server's "default pointer": (w, h, hot_x, hot_y, RGBA8).
pub fn default_arrow() -> (u32, u32, u32, u32, Vec<u8>) {
    const WIDTH: u32 = 11;
    const HEIGHT: u32 = 16;
    let mut rgba = Vec::with_capacity((WIDTH * HEIGHT * 4) as usize);
    for y in 0..HEIGHT {
        let edge = y * 10 / 15;
        for x in 0..WIDTH {
            let pixel = if x > edge {
                [0, 0, 0, 0]
            } else if x == 0 || x == edge || y == HEIGHT - 1 {
                [0, 0, 0, 255]
            } else {
                [255, 255, 255, 255]
            };
            rgba.extend_from_slice(&pixel);
        }
    }
    (WIDTH, HEIGHT, 0, 0, rgba)
}

struct HostCertCheck(Arc<dyn DriverHost>);

#[async_trait]
impl CertCheck for HostCertCheck {
    async fn check(&self, sha256: [u8; 32]) -> bool {
        self.0.confirm_cert(&fingerprint_hex(&sha256)).await
    }
}

struct AudioBridge(Arc<dyn PcmSink>);

impl AudioBridge {
    fn wrap(pcm: Arc<dyn PcmSink>) -> Arc<dyn RdpAudioSink> {
        Arc::new(Self(pcm))
    }
}

impl RdpAudioSink for AudioBridge {
    fn format(&self, sample_rate: u32, channels: u16) {
        self.0.format(sample_rate, channels);
    }

    fn samples(&self, pcm: &[i16]) {
        self.0.samples(pcm);
    }

    fn stop(&self) {
        self.0.stop();
    }
}

struct SinkBridge {
    sink: Arc<dyn FrameSink>,
    host: Arc<dyn DriverHost>,
    /// Desktop size as `w << 16 | h`, for normalized pointer coordinates.
    size: AtomicU32,
}

impl SinkBridge {
    fn desktop_size(&self) -> (u16, u16) {
        let packed = self.size.load(Ordering::Acquire);
        ((packed >> 16) as u16, packed as u16)
    }

    fn set_desktop_size(&self, w: u16, h: u16) {
        self.size
            .store((u32::from(w) << 16) | u32::from(h), Ordering::Release);
    }
}

impl RdpSink for SinkBridge {
    fn resize(&self, w: u16, h: u16) {
        self.sink.resize(u32::from(w), u32::from(h));
        self.set_desktop_size(w, h);
    }

    fn update(&self, image: &[u8], stride: usize, rect: Rect) {
        if rect.w == 0 || rect.h == 0 {
            return;
        }
        let offset = usize::from(rect.y)
            .checked_mul(stride)
            .and_then(|offset| offset.checked_add(usize::from(rect.x) * 4));
        let end = offset.and_then(|offset| {
            stride
                .checked_mul(usize::from(rect.h) - 1)
                .and_then(|rows| rows.checked_add(usize::from(rect.w) * 4))
                .and_then(|span| span.checked_add(offset))
        });
        let (Some(offset), Some(end)) = (offset, end) else {
            tracing::warn!(stride, "ignoring RDP update with an overflowing rectangle");
            return;
        };
        if end > image.len() {
            tracing::warn!(
                x = rect.x,
                y = rect.y,
                w = rect.w,
                h = rect.h,
                stride,
                len = image.len(),
                "ignoring RDP update outside the image"
            );
            return;
        }
        self.sink.put(
            u32::from(rect.x),
            u32::from(rect.y),
            u32::from(rect.w),
            u32::from(rect.h),
            &image[offset..],
            stride,
            SrcPixels::Rgba8,
        );
    }

    fn commit(&self) {
        self.sink.commit();
    }

    fn pointer_bitmap(&self, w: u16, h: u16, hot_x: u16, hot_y: u16, rgba: &[u8]) {
        self.sink.cursor_shape(
            u32::from(w),
            u32::from(h),
            u32::from(hot_x),
            u32::from(hot_y),
            rgba,
        );
    }

    fn pointer_hidden(&self) {
        self.sink.cursor_shape(0, 0, 0, 0, &[]);
    }

    fn pointer_default(&self) {
        let (w, h, hot_x, hot_y, rgba) = default_arrow();
        self.sink.cursor_shape(w, h, hot_x, hot_y, &rgba);
    }

    fn pointer_pos(&self, x: u16, y: u16) {
        self.sink.cursor_pos(u32::from(x), u32::from(y));
    }

    fn clipboard(&self, text: &str) {
        self.host.clipboard(text);
    }

    fn clipboard_other(&self) {
        self.host.clipboard_other();
    }

    fn monitors(&self, layout: &[Rect]) {
        let screens = layout
            .iter()
            .map(|rect| AdapterScreen {
                x: u32::from(rect.x),
                y: u32::from(rect.y),
                width: u32::from(rect.w),
                height: u32::from(rect.h),
                guess: None,
            })
            .collect::<Vec<_>>();
        self.host.screens(&screens);
    }
}

/// View size in points the desktop size derives from.
#[derive(Debug, Clone, Copy)]
struct View {
    width: u32,
    height: u32,
    scale: f32,
}

/// Connects and authenticates. `known` (the credentials of the connection
/// being replaced) is tried first without asking; the credentials that
/// worked come back for the next reconnect.
async fn connect_with_retries(
    params: &DriverParams,
    view: View,
    monitors: u8,
    host: &Arc<dyn DriverHost>,
    bridge: &Arc<SinkBridge>,
    mut known: Option<Credentials>,
) -> Result<(RdpConnection, Credentials), AdapterError> {
    let (width, height) = landscape_desktop_size(view.width as f32, view.height as f32, view.scale);
    let mut failed_before = false;
    let mut attempt = 1;
    loop {
        let credentials = match known.take() {
            Some(credentials) => credentials,
            None => {
                let Some(credentials) = host.credentials(&CRED_FIELDS, failed_before).await else {
                    return Err(AdapterError::Cancelled);
                };
                credentials
            }
        };
        host.authenticating();
        let (user, domain) = split_domain(
            credentials.user.as_deref().unwrap_or_default(),
            credentials.domain.as_deref(),
        );
        let stream = params.dial.dial().await?;
        let config = RdpConfig {
            server_name: params.server_name.clone(),
            credentials: Some(RdpCredentials {
                user,
                password: credentials.password.clone(),
                domain,
            }),
            width,
            height,
            scale_percent: 100,
            client_name: CLIENT_NAME.to_owned(),
            cert: Arc::new(HostCertCheck(host.clone())),
            gfx: true,
            monitors: monitors.clamp(1, 4),
            audio: host.audio().map(AudioBridge::wrap),
            clipboard: true,
        };
        let sink: Arc<dyn RdpSink> = bridge.clone();
        match connect(stream, config, sink).await {
            Ok(connection) => return Ok((connection, credentials)),
            Err(RdpError::AuthFailed) if attempt < MAX_AUTH_ATTEMPTS => {
                tracing::info!(attempt, "RDP authentication failed; asking again");
                failed_before = true;
                attempt += 1;
            }
            Err(error) => return Err(map_rdp_error(error)),
        }
    }
}

fn hid_inputs(mapper: &mut InputMapper, touch: &mut Option<u8>, msg: InputMsg) -> Vec<RdpInput> {
    match msg {
        InputMsg::MouseMove { x, y } => mapper.mouse_move(x, y),
        InputMsg::MouseButton {
            button, down, x, y, ..
        } => mapper.button(button as u8, down, x, y),
        InputMsg::Scroll {
            dx,
            dy,
            phase,
            unit,
            ..
        } => {
            if phase == ScrollPhase::Began {
                mapper.reset_scroll();
            }
            mapper.scroll(dx, dy, unit == ScrollUnit::Line)
        }
        InputMsg::Key { usage, down, mods } => mapper.key(usage, down, mods.bits()),
        InputMsg::Text(text) => mapper.text(&text),
        InputMsg::ReleaseAll => mapper.release_all(),
        // Only the first finger down acts as the left button.
        InputMsg::Touch {
            id, phase, x, y, ..
        } => match phase {
            TouchPhase::Began if touch.is_none() => {
                *touch = Some(id);
                mapper.button(0, true, x, y)
            }
            TouchPhase::Moved if *touch == Some(id) => mapper.mouse_move(x, y),
            TouchPhase::Ended | TouchPhase::Cancelled if *touch == Some(id) => {
                *touch = None;
                mapper.button(0, false, x, y)
            }
            _ => Vec::new(),
        },
    }
}

fn translate(
    mapper: &mut InputMapper,
    touch: &mut Option<u8>,
    input: DriverInput,
) -> Vec<RdpCommand> {
    let inputs = match input {
        DriverInput::Hid(msg) => hid_inputs(mapper, touch, msg),
        DriverInput::CtrlAltDel => ctrl_alt_end(),
        DriverInput::Clipboard(text) => return vec![RdpCommand::Clipboard(text)],
        DriverInput::Monitors(count) => return vec![RdpCommand::Monitors(count)],
        DriverInput::Resize {
            view_width,
            view_height,
            scale,
        } => {
            let (width, height) =
                landscape_desktop_size(view_width as f32, view_height as f32, scale);
            return vec![RdpCommand::Resize {
                width,
                height,
                scale_percent: 100,
            }];
        }
        DriverInput::TypeSecret(_) => {
            tracing::debug!("ignoring type-password input in an RDP session");
            return Vec::new();
        }
    };
    inputs.into_iter().map(RdpCommand::Input).collect()
}

/// Returns when `input` closes (dropping `commands` ends `RdpConnection::run`)
/// or the connection stopped taking commands.
async fn forward_input(
    input: &mut mpsc::Receiver<DriverInput>,
    commands: mpsc::Sender<RdpCommand>,
    bridge: &SinkBridge,
    mut size: (u16, u16),
    background: watch::Receiver<bool>,
    view: &mut View,
) {
    let mut mapper = InputMapper::new(size.0, size.1);
    let mut touch = None;
    while let Some(input) = input.recv().await {
        if *background.borrow() && !matches!(&input, DriverInput::Clipboard(_)) {
            continue;
        }
        if let DriverInput::Resize {
            view_width,
            view_height,
            scale,
        } = input
        {
            *view = View {
                width: view_width,
                height: view_height,
                scale,
            };
        }
        let current = bridge.desktop_size();
        if current != size {
            size = current;
            mapper.set_desktop_size(size.0, size.1);
        }
        for command in translate(&mut mapper, &mut touch, input) {
            if commands.send(command).await.is_err() {
                return;
            }
        }
    }
}

#[async_trait]
impl ProtocolDriver for RdpDriver {
    fn protocol(&self) -> AdapterProtocol {
        AdapterProtocol::Rdp
    }

    async fn run(
        &self,
        params: DriverParams,
        host: Arc<dyn DriverHost>,
        sink: Arc<dyn FrameSink>,
        mut input: mpsc::Receiver<DriverInput>,
    ) -> Result<(), AdapterError> {
        let bridge = Arc::new(SinkBridge {
            sink,
            host: host.clone(),
            size: AtomicU32::new(0),
        });
        let mut view = View {
            width: params.view_width,
            height: params.view_height,
            scale: params.scale,
        };
        let (mut connection, mut credentials) =
            connect_with_retries(&params, view, params.monitors, &host, &bridge, None).await?;
        host.auth_succeeded();
        loop {
            let (width, height) = connection.desktop_size();
            bridge.set_desktop_size(width, height);
            host.streaming(u32::from(width), u32::from(height), &params.server_name);
            tracing::info!(width, height, "RDP session streaming");

            let result = {
                let (commands, command_receiver) = mpsc::channel(COMMAND_QUEUE);
                let background = host.background();
                let run = connection.run_with_background(
                    command_receiver,
                    background.clone(),
                    host.session_id(),
                );
                let forward = forward_input(
                    &mut input,
                    commands,
                    &bridge,
                    (width, height),
                    background,
                    &mut view,
                );
                tokio::pin!(run, forward);
                tokio::select! {
                    result = &mut run => result,
                    () = &mut forward => run.await,
                }
            };
            let monitors = match result {
                Err(RdpError::Reconnect(monitors)) => monitors,
                result => return result.map_err(map_rdp_error),
            };
            // No displaycontrol: the server takes a new monitor count only
            // when connecting. Windows hands the reconnect the same session.
            tracing::info!(monitors, "RDP reconnecting with a new monitor count");
            (connection, credentials) =
                connect_with_retries(&params, view, monitors, &host, &bridge, Some(credentials))
                    .await?;
        }
    }
}

#[cfg(test)]
mod tests {
    use std::sync::Mutex;

    use super::*;

    #[derive(Default)]
    struct Rec {
        log: Mutex<Vec<String>>,
        screens: Mutex<Vec<Vec<AdapterScreen>>>,
    }

    impl Rec {
        fn record(&self, entry: String) {
            self.log.lock().unwrap().push(entry);
        }

        fn log(&self) -> Vec<String> {
            self.log.lock().unwrap().clone()
        }
    }

    #[async_trait]
    impl DriverHost for Rec {
        async fn credentials(&self, _: &[CredField], _: bool) -> Option<Credentials> {
            None
        }

        fn auth_succeeded(&self) {}

        fn authenticating(&self) {}

        fn streaming(&self, _: u32, _: u32, _: &str) {}

        fn clipboard(&self, text: &str) {
            self.record(format!("clipboard({text})"));
        }

        async fn confirm_cert(&self, _: &str) -> bool {
            false
        }

        fn screens(&self, screens: &[AdapterScreen]) {
            self.screens.lock().unwrap().push(screens.to_vec());
        }
    }

    impl PcmSink for Rec {
        fn format(&self, sample_rate: u32, channels: u16) {
            self.record(format!("format({sample_rate}, {channels})"));
        }

        fn samples(&self, pcm: &[i16]) {
            self.record(format!("samples({pcm:?})"));
        }

        fn stop(&self) {
            self.record("stop".into());
        }
    }

    struct NoFrames;

    impl FrameSink for NoFrames {
        fn resize(&self, _: u32, _: u32) {}
        fn put(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8], _: usize, _: SrcPixels) {}
        fn fill(&self, _: u32, _: u32, _: u32, _: u32, _: [u8; 4]) {}
        fn copy_within(&self, _: u32, _: u32, _: u32, _: u32, _: u32, _: u32) {}
        fn commit(&self) {}
        fn cursor_shape(&self, _: u32, _: u32, _: u32, _: u32, _: &[u8]) {}
        fn cursor_pos(&self, _: u32, _: u32) {}
    }

    #[test]
    fn sink_bridge_forwards() {
        let host = Arc::new(Rec::default());
        let bridge = SinkBridge {
            sink: Arc::new(NoFrames),
            host: host.clone(),
            size: AtomicU32::new(0),
        };
        bridge.clipboard("hi");
        assert_eq!(host.log(), ["clipboard(hi)"]);
        bridge.monitors(&[
            Rect {
                x: 0,
                y: 0,
                w: 1024,
                h: 768,
            },
            Rect {
                x: 1024,
                y: 0,
                w: 1024,
                h: 768,
            },
        ]);
        assert_eq!(
            *host.screens.lock().unwrap(),
            [vec![
                AdapterScreen {
                    x: 0,
                    y: 0,
                    width: 1024,
                    height: 768,
                    guess: None,
                },
                AdapterScreen {
                    x: 1024,
                    y: 0,
                    width: 1024,
                    height: 768,
                    guess: None,
                },
            ]]
        );

        let pcm = Arc::new(Rec::default());
        let audio = AudioBridge::wrap(pcm.clone());
        audio.format(48_000, 2);
        audio.samples(&[1, -2, 3, -4]);
        audio.stop();
        assert_eq!(
            pcm.log(),
            ["format(48000, 2)", "samples([1, -2, 3, -4])", "stop"]
        );
    }
}
