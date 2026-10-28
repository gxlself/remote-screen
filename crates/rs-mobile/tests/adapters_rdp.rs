mod support;

use std::collections::VecDeque;
use std::future::Future;
use std::net::SocketAddr;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use async_trait::async_trait;
use rs_mobile::rscore::adapters::driver::{
    AdapterError, Credentials, Dial, DriverHost, DriverInput, DriverParams, FrameSink,
    ProtocolDriver, SrcPixels,
};
use rs_mobile::rscore::adapters::dto::{
    AdapterOpenOpts, AdapterProtocol, AdapterScreen, AdapterTarget, CredField,
};
use rs_mobile::rscore::adapters::rdp::{
    default_arrow, map_rdp_error, split_domain, RdpDriver, MAX_AUTH_ATTEMPTS,
};
use rs_mobile::rscore::adapters::session::{self, NetDial, CERT_KV_PREFIX};
use rs_mobile::rscore::runtime::{install_core, take_core};
use rs_rdp::testkit::{FakeInput, FakeRdpServer, FakeSecurity};
use rs_rdp::RdpError;
use rsp_core::hid::{Button, InputMsg, Mods, TouchPhase};
use rsp_transport::net::NetStream;
use serde_json::Value;
use tokio::net::TcpStream;
use tokio::sync::mpsc;
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

const STEP: Duration = Duration::from_secs(5);

async fn wall_clock(test: impl Future<Output = ()>) {
    tokio::time::timeout(Duration::from_secs(20), test)
        .await
        .expect("RDP driver test exceeded its 20-second wall-clock budget");
}

async fn eventually(what: &str, check: impl Fn() -> bool) {
    let deadline = tokio::time::Instant::now() + STEP;
    while !check() {
        assert!(
            tokio::time::Instant::now() < deadline,
            "timed out waiting for {what}"
        );
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn hybrid(password: &str, domain: Option<&str>) -> FakeSecurity {
    FakeSecurity::Hybrid {
        user: "alice".into(),
        password: password.into(),
        domain: domain.map(str::to_owned),
    }
}

type Answer = Option<(&'static str, &'static str, Option<&'static str>)>;

fn answer(user: &'static str, password: &'static str) -> Answer {
    Some((user, password, None))
}

struct RecHost {
    answers: Mutex<VecDeque<Answer>>,
    accept_cert: bool,
    asks: Mutex<Vec<(Vec<CredField>, bool)>>,
    log: Mutex<Vec<String>>,
    certs: Mutex<Vec<String>>,
    screens: Mutex<Vec<Vec<AdapterScreen>>>,
}

impl RecHost {
    fn new(answers: Vec<Answer>, accept_cert: bool) -> Arc<Self> {
        Arc::new(Self {
            answers: Mutex::new(answers.into()),
            accept_cert,
            asks: Mutex::default(),
            log: Mutex::default(),
            certs: Mutex::default(),
            screens: Mutex::default(),
        })
    }

    fn record(&self, entry: String) {
        self.log.lock().unwrap().push(entry);
    }

    fn asks(&self) -> Vec<(Vec<CredField>, bool)> {
        self.asks.lock().unwrap().clone()
    }

    fn failed_before(&self) -> Vec<bool> {
        self.asks().into_iter().map(|(_, failed)| failed).collect()
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn certs(&self) -> Vec<String> {
        self.certs.lock().unwrap().clone()
    }
}

#[async_trait]
impl DriverHost for RecHost {
    async fn credentials(&self, fields: &[CredField], failed_before: bool) -> Option<Credentials> {
        self.asks
            .lock()
            .unwrap()
            .push((fields.to_vec(), failed_before));
        let (user, password, domain) = self.answers.lock().unwrap().pop_front().flatten()?;
        Some(Credentials {
            user: Some(user.to_owned()),
            password: Zeroizing::new(password.to_owned()),
            domain: domain.map(str::to_owned),
        })
    }

    fn auth_succeeded(&self) {
        self.record("auth_succeeded".into());
    }

    fn authenticating(&self) {
        self.record("authenticating".into());
    }

    fn streaming(&self, width: u32, height: u32, server_name: &str) {
        self.record(format!("streaming {width}x{height} {server_name}"));
    }

    fn clipboard(&self, text: &str) {
        self.record(format!("clipboard {text}"));
    }

    async fn confirm_cert(&self, fp_hex: &str) -> bool {
        self.certs.lock().unwrap().push(fp_hex.to_owned());
        self.accept_cert
    }

    fn screens(&self, screens: &[AdapterScreen]) {
        self.screens.lock().unwrap().push(screens.to_vec());
    }
}

type Cursor = (u32, u32, u32, u32, Vec<u8>);

#[derive(Default)]
struct Frame {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    resizes: Vec<(u32, u32)>,
    commits: usize,
    cursor: Option<Cursor>,
}

impl Frame {
    fn write(&mut self, x: u32, y: u32, bgra: [u8; 4]) {
        if x < self.width && y < self.height {
            let at = ((y * self.width + x) * 4) as usize;
            self.bgra[at..at + 4].copy_from_slice(&bgra);
        }
    }

    fn read(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        (x < self.width && y < self.height).then(|| {
            let at = ((y * self.width + x) * 4) as usize;
            self.bgra[at..at + 4].try_into().unwrap()
        })
    }
}

/// BGRA8 framebuffer plus a record of resizes, commits and the last cursor.
#[derive(Default)]
struct RecSink(Mutex<Frame>);

impl RecSink {
    fn pixel(&self, x: u32, y: u32) -> Option<[u8; 4]> {
        self.0.lock().unwrap().read(x, y)
    }

    fn resizes(&self) -> Vec<(u32, u32)> {
        self.0.lock().unwrap().resizes.clone()
    }

    fn commits(&self) -> usize {
        self.0.lock().unwrap().commits
    }

    fn cursor(&self) -> Option<Cursor> {
        self.0.lock().unwrap().cursor.clone()
    }
}

impl FrameSink for RecSink {
    fn resize(&self, width: u32, height: u32) {
        let mut frame = self.0.lock().unwrap();
        frame.width = width;
        frame.height = height;
        frame.bgra = [0, 0, 0, 255].repeat((width * height) as usize);
        frame.resizes.push((width, height));
    }

    fn put(&self, x: u32, y: u32, w: u32, h: u32, src: &[u8], stride: usize, pix: SrcPixels) {
        let mut frame = self.0.lock().unwrap();
        for row in 0..h {
            for col in 0..w {
                let at = row as usize * stride + col as usize * 4;
                let mut pixel: [u8; 4] = src[at..at + 4].try_into().unwrap();
                if pix == SrcPixels::Rgba8 {
                    pixel.swap(0, 2);
                }
                frame.write(x + col, y + row, pixel);
            }
        }
    }

    fn fill(&self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]) {
        let mut frame = self.0.lock().unwrap();
        for row in 0..h {
            for col in 0..w {
                frame.write(x + col, y + row, bgra);
            }
        }
    }

    fn copy_within(&self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        let mut frame = self.0.lock().unwrap();
        let mut copied = Vec::new();
        for row in 0..h {
            for col in 0..w {
                if let Some(pixel) = frame.read(sx + col, sy + row) {
                    copied.push((dx + col, dy + row, pixel));
                }
            }
        }
        for (x, y, pixel) in copied {
            frame.write(x, y, pixel);
        }
    }

    fn commit(&self) {
        self.0.lock().unwrap().commits += 1;
    }

    fn cursor_shape(&self, w: u32, h: u32, hot_x: u32, hot_y: u32, rgba: &[u8]) {
        self.0.lock().unwrap().cursor = Some((w, h, hot_x, hot_y, rgba.to_vec()));
    }

    fn cursor_pos(&self, _: u32, _: u32) {}
}

struct TcpDial {
    addr: SocketAddr,
    dials: AtomicUsize,
}

impl TcpDial {
    fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Dial for TcpDial {
    async fn dial(&self) -> Result<NetStream, AdapterError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let stream = TcpStream::connect(self.addr)
            .await
            .map_err(|error| AdapterError::Io(error.to_string()))?;
        stream
            .set_nodelay(true)
            .map_err(|error| AdapterError::Io(error.to_string()))?;
        Ok(Box::new(stream))
    }
}

struct Session {
    host: Arc<RecHost>,
    sink: Arc<RecSink>,
    dial: Arc<TcpDial>,
    input: Option<mpsc::Sender<DriverInput>>,
    task: JoinHandle<Result<(), AdapterError>>,
}

impl Session {
    fn start(addr: SocketAddr, host: Arc<RecHost>) -> Self {
        let sink = Arc::new(RecSink::default());
        let dial = Arc::new(TcpDial {
            addr,
            dials: AtomicUsize::new(0),
        });
        let params = DriverParams {
            dial: dial.clone(),
            server_name: "127.0.0.1".into(),
            view_width: 1024,
            view_height: 768,
            scale: 1.0,
            monitors: 1,
        };
        let (input, receiver) = mpsc::channel(256);
        let task = tokio::spawn({
            let host = host.clone();
            let sink = sink.clone();
            async move {
                let driver = RdpDriver;
                driver.run(params, host, sink, receiver).await
            }
        });
        Self {
            host,
            sink,
            dial,
            input: Some(input),
            task,
        }
    }

    /// Hybrid alice / pw-1, answered once, streaming with the first resize seen.
    async fn streaming(server: &FakeRdpServer) -> Self {
        let session = Self::start(
            server.addr(),
            RecHost::new(vec![answer("alice", "pw-1")], true),
        );
        session.wait_streaming().await;
        session
    }

    async fn wait_streaming(&self) {
        eventually("streaming and the first resize", || {
            self.host
                .log()
                .iter()
                .any(|entry| entry.starts_with("streaming "))
                && !self.sink.resizes().is_empty()
        })
        .await;
    }

    fn size(&self) -> (u32, u32) {
        self.sink.resizes()[0]
    }

    async fn send(&self, input: DriverInput) {
        self.input
            .as_ref()
            .expect("input still open")
            .send(input)
            .await
            .expect("driver still takes input");
    }

    async fn hid(&self, msg: InputMsg) {
        self.send(DriverInput::Hid(msg)).await;
    }

    async fn result(self) -> Result<(), AdapterError> {
        tokio::time::timeout(STEP, self.task)
            .await
            .expect("driver did not return")
            .expect("driver task panicked")
    }

    /// Close the input side and return the driver's result.
    async fn finish(mut self) -> Result<(), AdapterError> {
        self.input.take();
        self.result().await
    }
}

async fn next_input(server: &mut FakeRdpServer) -> Option<FakeInput> {
    server.next_input(STEP).await
}

fn key(scancode: u8, extended: bool, down: bool) -> FakeInput {
    FakeInput::Key {
        scancode,
        extended,
        down,
    }
}

fn half(size: u32) -> u16 {
    (0.5_f32 * (size - 1) as f32).round() as u16
}

#[tokio::test]
async fn streaming() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::start(
            server.addr(),
            RecHost::new(vec![answer("alice", "pw-1")], true),
        );
        session.wait_streaming().await;
        assert_eq!(
            session.host.asks(),
            [(
                vec![CredField::User, CredField::Password, CredField::Domain],
                false
            )]
        );
        let (width, height) = session.size();
        assert_eq!(
            session.host.log(),
            [
                "authenticating".to_owned(),
                "auth_succeeded".to_owned(),
                format!("streaming {width}x{height} 127.0.0.1"),
            ]
        );
        assert_eq!(session.dial.dials(), 1);
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn domain_split_end_to_end() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw", Some("CORP")), 1024, 768)
            .await
            .unwrap();
        let session = Session::start(
            server.addr(),
            RecHost::new(vec![Some(("CORP\\alice", "pw", None))], true),
        );
        session.wait_streaming().await;
        assert_eq!(session.host.failed_before(), [false]);
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn retry_once() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::start(
            server.addr(),
            RecHost::new(
                vec![answer("alice", "wrong"), answer("alice", "pw-1")],
                true,
            ),
        );
        session.wait_streaming().await;
        assert_eq!(session.host.failed_before(), [false, true]);
        assert_eq!(server.connections(), 2);
        assert_eq!(session.dial.dials(), 2);
        let (width, height) = session.size();
        assert_eq!(
            session.host.log(),
            [
                "authenticating".to_owned(),
                "authenticating".to_owned(),
                "auth_succeeded".to_owned(),
                format!("streaming {width}x{height} 127.0.0.1"),
            ]
        );
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn three_failures() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::start(
            server.addr(),
            RecHost::new(vec![answer("alice", "wrong"); 3], true),
        );
        let host = session.host.clone();
        assert_eq!(session.result().await, Err(AdapterError::AuthFailed));
        assert_eq!(server.connections(), 3);
        assert_eq!(host.failed_before(), [false, true, true]);
        assert_eq!(host.asks().len() as u32, MAX_AUTH_ATTEMPTS);
        assert!(!host.log().iter().any(|entry| entry == "auth_succeeded"));
    })
    .await;
}

#[tokio::test]
async fn cancel_before_dial() {
    wall_clock(async {
        let session = Session::start(
            SocketAddr::from(([127, 0, 0, 1], 9)),
            RecHost::new(vec![None], true),
        );
        let host = session.host.clone();
        let dial = session.dial.clone();
        assert_eq!(session.result().await, Err(AdapterError::Cancelled));
        assert_eq!(dial.dials(), 0);
        assert_eq!(host.failed_before(), [false]);
        assert!(host.log().is_empty());
    })
    .await;
}

#[tokio::test]
async fn cert_rejected() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::start(
            server.addr(),
            RecHost::new(vec![answer("alice", "pw-1")], false),
        );
        let host = session.host.clone();
        assert_eq!(session.result().await, Err(AdapterError::CertRejected));
        let expected = hex(&server.cert_sha256());
        assert_eq!(expected.len(), 64);
        assert!(expected.chars().all(|c| matches!(c, '0'..='9' | 'a'..='f')));
        assert_eq!(host.certs(), [expected]);
    })
    .await;
}

#[tokio::test]
async fn bitmap() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        server.push_bitmap(0, 0, 16, 16, [0, 0, 255, 255].repeat(16 * 16));
        // BGRA storage: the server's red arrives as RGBA [255, 0, 0, 255].
        eventually("the red bitmap and a commit", || {
            session.sink.pixel(8, 8) == Some([0, 0, 255, 255]) && session.sink.commits() > 0
        })
        .await;
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn keys() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        session
            .hid(InputMsg::Key {
                usage: 0x04,
                down: true,
                mods: Mods::LSHIFT,
            })
            .await;
        assert_eq!(next_input(&mut server).await, Some(key(0x2A, false, true)));
        assert_eq!(next_input(&mut server).await, Some(key(0x1E, false, true)));
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn mouse() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        let (width, height) = session.size();
        let center = FakeInput::MouseMove {
            x: half(width),
            y: half(height),
        };
        session.hid(InputMsg::MouseMove { x: 0.5, y: 0.5 }).await;
        assert_eq!(next_input(&mut server).await, Some(center.clone()));
        session
            .hid(InputMsg::MouseButton {
                button: Button::Left,
                down: true,
                x: 0.5,
                y: 0.5,
                clicks: 1,
            })
            .await;
        assert_eq!(next_input(&mut server).await, Some(center));
        assert_eq!(
            next_input(&mut server).await,
            Some(FakeInput::Button {
                button: 0,
                down: true
            })
        );
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn ctrl_alt_end() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        session.send(DriverInput::CtrlAltDel).await;
        for expected in [
            key(0x1D, false, true),
            key(0x38, false, true),
            key(0x4F, true, true),
            key(0x4F, true, false),
            key(0x38, false, false),
            key(0x1D, false, false),
        ] {
            assert_eq!(next_input(&mut server).await, Some(expected));
        }
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn pointer() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        server.push_pointer_hidden();
        eventually("the hidden pointer", || {
            session.sink.cursor() == Some((0, 0, 0, 0, Vec::new()))
        })
        .await;
        let rgba = vec![1, 2, 3, 255, 90, 80, 70, 255, 15, 30, 60, 128, 4, 5, 6, 255];
        server.push_pointer_bitmap(2, 2, 1, 1, rgba.clone());
        eventually("the pointer bitmap", || {
            session.sink.cursor() == Some((2, 2, 1, 1, rgba.clone()))
        })
        .await;
        server.push_pointer_default();
        eventually("the default arrow", || {
            session.sink.cursor() == Some(default_arrow())
        })
        .await;
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn ignored_and_command_inputs() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        session
            .send(DriverInput::TypeSecret(Zeroizing::new("pw".into())))
            .await;
        session.send(DriverInput::Clipboard("x".into())).await;
        session
            .send(DriverInput::Resize {
                view_width: 800,
                view_height: 600,
                scale: 1.0,
            })
            .await;
        session.hid(InputMsg::MouseMove { x: 0.0, y: 0.0 }).await;
        assert_eq!(
            next_input(&mut server).await,
            Some(FakeInput::MouseMove { x: 0, y: 0 })
        );
        assert!(!session.task.is_finished());
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

/// The fake server has no displaycontrol, so a new monitor count reconnects
/// with the credentials that worked, without asking again.
#[tokio::test]
async fn monitors_without_displaycontrol_reconnect_with_same_credentials() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        session.send(DriverInput::Monitors(2)).await;
        eventually("a second streaming state", || {
            session
                .host
                .log()
                .iter()
                .filter(|entry| entry.starts_with("streaming "))
                .count()
                == 2
        })
        .await;
        assert_eq!(session.host.failed_before(), [false]);
        assert_eq!(server.connections(), 2);
        assert_eq!(session.dial.dials(), 2);
        let log = session.host.log();
        assert_eq!(
            log[..4],
            [
                "authenticating".to_owned(),
                "auth_succeeded".to_owned(),
                log[2].clone(),
                "authenticating".to_owned(),
            ]
        );
        assert!(log[2].starts_with("streaming "));
        assert!(log[4].starts_with("streaming "));
        eventually("two monitors declared", || {
            session.host.screens.lock().unwrap().iter().any(|screens| {
                screens.len() == 2
                    && screens[0].x == 0
                    && screens[1].x == screens[0].width
                    && screens[1].y == 0
            })
        })
        .await;
        // The input side survives the reconnect.
        session.hid(InputMsg::MouseMove { x: 0.0, y: 0.0 }).await;
        assert_eq!(
            next_input(&mut server).await,
            Some(FakeInput::MouseMove { x: 0, y: 0 })
        );
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn touch_as_left_button() {
    wall_clock(async {
        let mut server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        let touch = |id, phase, x| InputMsg::Touch {
            id,
            phase,
            x,
            y: x,
            pressure: 1.0,
        };
        let origin = FakeInput::MouseMove { x: 0, y: 0 };
        session.hid(touch(7, TouchPhase::Began, 0.0)).await;
        assert_eq!(next_input(&mut server).await, Some(origin.clone()));
        assert_eq!(
            next_input(&mut server).await,
            Some(FakeInput::Button {
                button: 0,
                down: true
            })
        );
        // A second finger would move the pointer to the centre first.
        session.hid(touch(8, TouchPhase::Began, 0.5)).await;
        session.hid(touch(7, TouchPhase::Ended, 0.0)).await;
        assert_eq!(next_input(&mut server).await, Some(origin));
        assert_eq!(
            next_input(&mut server).await,
            Some(FakeInput::Button {
                button: 0,
                down: false
            })
        );
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[test]
fn default_arrow_shape() {
    let (width, height, hot_x, hot_y, rgba) = default_arrow();
    assert_eq!((width, height, hot_x, hot_y), (11, 16, 0, 0));
    assert_eq!(rgba.len(), 11 * 16 * 4);
    let pixel = |x: usize, y: usize| &rgba[(y * 11 + x) * 4..][..4];
    assert_eq!(pixel(0, 0), [0, 0, 0, 255]);
    assert_eq!(pixel(1, 8), [255, 255, 255, 255]);
    assert_eq!(pixel(10, 15), [0, 0, 0, 255]);
    assert_eq!(pixel(5, 3), [0, 0, 0, 0]);
}

#[test]
fn split_domain_table() {
    let owned = |user: &str, domain: Option<&str>| (user.to_owned(), domain.map(str::to_owned));
    for (user, domain, expected) in [
        ("CORP\\alice", None, owned("alice", Some("CORP"))),
        ("\\alice", None, owned("alice", None)),
        ("CORP\\alice", Some("X"), owned("CORP\\alice", Some("X"))),
        (
            "alice@corp.example",
            None,
            owned("alice@corp.example", None),
        ),
        ("alice", None, owned("alice", None)),
    ] {
        assert_eq!(split_domain(user, domain), expected, "{user:?} {domain:?}");
    }
}

#[tokio::test]
async fn input_closed_ok() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        assert_eq!(session.finish().await, Ok(()));
    })
    .await;
}

#[tokio::test]
async fn server_disconnect() {
    wall_clock(async {
        let server = FakeRdpServer::start(hybrid("pw-1", None), 1024, 768)
            .await
            .unwrap();
        let session = Session::streaming(&server).await;
        server.disconnect();
        assert!(matches!(
            session.result().await,
            Err(AdapterError::ServerClosed(_))
        ));
    })
    .await;
}

fn state_event(collector: &support::EventCollector, id: u64, phase: &str) -> Option<Value> {
    let session_id = id.to_string();
    collector
        .of_type("adapter.state")
        .into_iter()
        .find(|event| {
            event["sessionId"].as_str() == Some(session_id.as_str()) && event["phase"] == phase
        })
}

fn wait_state(collector: &support::EventCollector, id: u64, phase: &str) -> Value {
    assert!(
        collector.wait_for(
            |_| state_event(collector, id, phase).is_some(),
            Duration::from_secs(20)
        ),
        "no {phase} event"
    );
    state_event(collector, id, phase).unwrap()
}

#[test]
fn end_to_end_open() {
    let core = support::test_core();
    install_core(core.clone());
    let server = core
        .block_on(FakeRdpServer::start(hybrid("pw-1", None), 1024, 768))
        .unwrap();
    let port = server.addr().port();
    let target = AdapterTarget {
        protocol: AdapterProtocol::Rdp,
        host_id: None,
        addr: "127.0.0.1".into(),
        port: Some(port),
    };
    let opts = AdapterOpenOpts {
        view_width: 1024,
        view_height: 768,
        scale: 1.0,
        ..AdapterOpenOpts::default()
    };
    let collector = support::EventCollector::new();
    core.events().subscribe(collector.clone());
    let dial = Arc::new(NetDial::new(&core, &target, Duration::from_secs(15)));
    let id = core
        .block_on(session::open_with(
            &core,
            target,
            opts,
            Arc::new(RdpDriver),
            dial,
        ))
        .unwrap();

    let asked = wait_state(&collector, id, "needsCredentials");
    assert_eq!(
        asked["credFields"],
        serde_json::json!(["user", "password", "domain"])
    );
    assert_eq!(asked["credError"], false);
    session::submit_credentials(
        &core,
        id,
        Credentials {
            user: Some("alice".into()),
            password: Zeroizing::new("pw-1".into()),
            domain: None,
        },
        false,
    )
    .unwrap();
    let streaming = wait_state(&collector, id, "streaming");
    assert_eq!(streaming["protocol"], "rdp");
    assert_eq!(streaming["serverName"], "127.0.0.1");
    assert_eq!(streaming["width"], 1024);
    assert_eq!(streaming["height"], 768);
    // First use: the certificate is trusted silently and pinned.
    assert_eq!(
        core.store()
            .kv_get(&format!("{CERT_KV_PREFIX}rdp:127.0.0.1:{port}"))
            .unwrap(),
        Some(hex(&server.cert_sha256()))
    );
    assert!(collector.of_type("adapter.certPrompt").is_empty());

    assert!(session::close(&core, id));
    let ended = wait_state(&collector, id, "ended");
    assert!(ended.get("error").is_none(), "{ended}");
    drop(server);
    take_core();
}

#[test]
fn map_rdp_error_table() {
    for (error, expected) in [
        (
            RdpError::Io("reset".into()),
            AdapterError::Io("reset".into()),
        ),
        (
            RdpError::Tls("bad record".into()),
            AdapterError::Io("tls: bad record".into()),
        ),
        (RdpError::CertRejected, AdapterError::CertRejected),
        (RdpError::NlaRequired, AdapterError::NlaRequired),
        (
            RdpError::UnsupportedSecurity("RDSTLS".into()),
            AdapterError::UnsupportedSecurity("RDSTLS".into()),
        ),
        (RdpError::AuthFailed, AdapterError::AuthFailed),
        (
            RdpError::Protocol("bad pdu".into()),
            AdapterError::Protocol("bad pdu".into()),
        ),
        (
            RdpError::Closed("eof".into()),
            AdapterError::ServerClosed("eof".into()),
        ),
        (RdpError::Cancelled, AdapterError::Cancelled),
    ] {
        assert_eq!(map_rdp_error(error), expected);
    }
}
