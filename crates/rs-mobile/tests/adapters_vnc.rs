//! VncDriver against the rs-vnc fake server.

mod support;

use std::collections::VecDeque;
use std::io::Write;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rs_mobile::rscore::adapters::{
    driver::{
        rs_e, AdapterError, Credentials, Dial, DriverHost, DriverInput, DriverParams, FrameSink,
        ProtocolDriver, SrcPixels,
    },
    dto::{AdapterOpenOpts, AdapterProtocol, AdapterScreen, AdapterTarget, CredField, ScreenGuess},
    ffi_raw::{rs_raw_copy, RsRect, RS_PIX_BGRA8},
    session::{self, NetDial, WHOLE_DESKTOP},
    vnc::{map_vnc_error, VncDriver, MAX_AUTH_ATTEMPTS},
};
use rs_mobile::rscore::runtime::{install_core, take_core};
use rs_vnc::{
    proto::ClientMsg,
    security,
    testkit::{
        listen_tcp, pair, ArdTestParams, AuthCheck, FakeHandle, FakeLog, SecurityPlan,
        ServerScript, Step, TestRect,
    },
    Rect, Screen, VncError,
};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit, TouchPhase};
use rsp_transport::net::NetStream;
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;
use zeroize::Zeroizing;

const WAIT: Duration = Duration::from_secs(5);
const LEFT: [u8; 4] = [10, 20, 30, 255];
const RIGHT: [u8; 4] = [40, 50, 60, 255];

/// install_core is process-wide.
static CORE_LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    CORE_LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

/// One credentials answer: None = the user cancelled.
type Answer = Option<(Option<&'static str>, &'static str)>;

#[derive(Default)]
struct RecHost {
    background: Option<watch::Sender<bool>>,
    answers: Mutex<VecDeque<Answer>>,
    asked: Mutex<Vec<(Vec<CredField>, bool)>>,
    log: Mutex<Vec<String>>,
    screens: Mutex<Vec<Vec<AdapterScreen>>>,
}

impl RecHost {
    fn new(answers: Vec<Answer>) -> Arc<Self> {
        Arc::new(Self {
            background: Some(watch::channel(false).0),
            answers: Mutex::new(answers.into()),
            ..Self::default()
        })
    }

    fn asked(&self) -> Vec<(Vec<CredField>, bool)> {
        self.asked.lock().unwrap().clone()
    }

    fn log(&self) -> Vec<String> {
        self.log.lock().unwrap().clone()
    }

    fn record(&self, entry: impl Into<String>) {
        self.log.lock().unwrap().push(entry.into());
    }

    fn reported_screens(&self) -> Vec<Vec<AdapterScreen>> {
        self.screens.lock().unwrap().clone()
    }

    fn streamed(&self) -> bool {
        self.log()
            .iter()
            .any(|entry| entry.starts_with("streaming"))
    }
}

#[async_trait]
impl DriverHost for RecHost {
    fn background(&self) -> watch::Receiver<bool> {
        self.background
            .as_ref()
            .map_or_else(|| watch::channel(false).1, watch::Sender::subscribe)
    }

    fn session_id(&self) -> u64 {
        42
    }

    async fn credentials(&self, fields: &[CredField], failed_before: bool) -> Option<Credentials> {
        self.asked
            .lock()
            .unwrap()
            .push((fields.to_vec(), failed_before));
        let (user, password) = self.answers.lock().unwrap().pop_front().flatten()?;
        Some(Credentials {
            user: user.map(str::to_owned),
            password: Zeroizing::new(password.to_owned()),
            domain: None,
        })
    }

    fn auth_succeeded(&self) {
        self.record("auth_succeeded");
    }

    fn authenticating(&self) {
        self.record("authenticating");
    }

    fn streaming(&self, width: u32, height: u32, server_name: &str) {
        self.record(format!("streaming {width}x{height} {server_name}"));
    }

    fn clipboard(&self, text: &str) {
        self.record(format!("clipboard {text}"));
    }

    async fn confirm_cert(&self, _fp_hex: &str) -> bool {
        self.record("confirm_cert");
        false
    }

    fn screens(&self, screens: &[AdapterScreen]) {
        self.screens.lock().unwrap().push(screens.to_vec());
    }
}

/// Log lines written on this thread while the guard lives. Tokio tests run
/// on one thread, so this includes the driver task.
#[derive(Clone, Default)]
struct Logs(Arc<Mutex<Vec<u8>>>);

impl Write for Logs {
    fn write(&mut self, bytes: &[u8]) -> std::io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> std::io::Result<()> {
        Ok(())
    }
}

impl Logs {
    fn capture() -> (Self, tracing::subscriber::DefaultGuard) {
        let logs = Self::default();
        let writer = logs.clone();
        let subscriber = tracing_subscriber::fmt()
            .without_time()
            .with_ansi(false)
            .with_max_level(tracing::Level::INFO)
            .with_writer(move || writer.clone())
            .finish();
        (logs, tracing::subscriber::set_default(subscriber))
    }

    fn lines(&self, message: &str) -> Vec<String> {
        String::from_utf8_lossy(&self.0.lock().unwrap())
            .lines()
            .filter(|line| line.contains(message))
            .map(str::to_owned)
            .collect()
    }
}

type CursorShape = (u32, u32, u32, u32, Vec<u8>);

#[derive(Default)]
struct SinkState {
    width: u32,
    height: u32,
    bgra: Vec<u8>,
    calls: Vec<String>,
    cursor: Option<CursorShape>,
}

impl SinkState {
    fn get(&self, x: u32, y: u32) -> [u8; 4] {
        if x >= self.width || y >= self.height {
            return [0; 4];
        }
        let at = ((y * self.width + x) * 4) as usize;
        self.bgra[at..at + 4].try_into().unwrap()
    }

    fn set(&mut self, x: u32, y: u32, pixel: &[u8]) {
        if x < self.width && y < self.height {
            let at = ((y * self.width + x) * 4) as usize;
            self.bgra[at..at + 4].copy_from_slice(pixel);
        }
    }
}

#[derive(Default)]
struct RecSink(Mutex<SinkState>);

impl RecSink {
    fn pixel(&self, x: u32, y: u32) -> [u8; 4] {
        self.0.lock().unwrap().get(x, y)
    }

    fn calls(&self) -> Vec<String> {
        self.0.lock().unwrap().calls.clone()
    }

    fn committed(&self) -> bool {
        self.calls().iter().any(|call| call == "commit")
    }

    fn cursor(&self) -> Option<CursorShape> {
        self.0.lock().unwrap().cursor.clone()
    }
}

impl FrameSink for RecSink {
    fn resize(&self, width: u32, height: u32) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(format!("resize {width}x{height}"));
        state.width = width;
        state.height = height;
        state.bgra = [0, 0, 0, 255].repeat((width * height) as usize);
    }

    fn put(&self, x: u32, y: u32, w: u32, h: u32, src: &[u8], stride: usize, pix: SrcPixels) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(format!("put {x},{y} {w}x{h} {pix:?}"));
        for row in 0..h {
            for col in 0..w {
                let at = row as usize * stride + col as usize * 4;
                state.set(x + col, y + row, &src[at..at + 4]);
            }
        }
    }

    fn fill(&self, x: u32, y: u32, w: u32, h: u32, bgra: [u8; 4]) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(format!("fill {x},{y} {w}x{h}"));
        for row in 0..h {
            for col in 0..w {
                state.set(x + col, y + row, &bgra);
            }
        }
    }

    fn copy_within(&self, sx: u32, sy: u32, dx: u32, dy: u32, w: u32, h: u32) {
        let mut state = self.0.lock().unwrap();
        state
            .calls
            .push(format!("copy {sx},{sy} -> {dx},{dy} {w}x{h}"));
        let block: Vec<[u8; 4]> = (0..h)
            .flat_map(|row| (0..w).map(move |col| (col, row)))
            .map(|(col, row)| state.get(sx + col, sy + row))
            .collect();
        for (index, pixel) in block.iter().enumerate() {
            let (col, row) = (index as u32 % w, index as u32 / w);
            state.set(dx + col, dy + row, pixel);
        }
    }

    fn commit(&self) {
        self.0.lock().unwrap().calls.push("commit".to_owned());
    }

    fn cursor_shape(&self, w: u32, h: u32, hot_x: u32, hot_y: u32, rgba: &[u8]) {
        let mut state = self.0.lock().unwrap();
        state.calls.push(format!("cursor_shape {w}x{h}"));
        state.cursor = Some((w, h, hot_x, hot_y, rgba.to_vec()));
    }

    fn cursor_pos(&self, x: u32, y: u32) {
        self.0
            .lock()
            .unwrap()
            .calls
            .push(format!("cursor_pos {x},{y}"));
    }
}

/// Each dial connects to a fresh in-memory fake server running the next script.
struct PairDial {
    scripts: Mutex<VecDeque<ServerScript>>,
    servers: mpsc::UnboundedSender<FakeHandle>,
    dials: AtomicUsize,
}

impl PairDial {
    fn dials(&self) -> usize {
        self.dials.load(Ordering::SeqCst)
    }
}

#[async_trait]
impl Dial for PairDial {
    async fn dial(&self) -> Result<NetStream, AdapterError> {
        self.dials.fetch_add(1, Ordering::SeqCst);
        let script = self
            .scripts
            .lock()
            .unwrap()
            .pop_front()
            .expect("a server script for every dial");
        let (client, server) = pair(script);
        let _ = self.servers.send(server);
        Ok(Box::new(client))
    }
}

struct RefusedDial;

#[async_trait]
impl Dial for RefusedDial {
    async fn dial(&self) -> Result<NetStream, AdapterError> {
        Err(AdapterError::Refused)
    }
}

type DriverTask = JoinHandle<Result<(), AdapterError>>;

fn spawn_driver(
    dial: Arc<dyn Dial>,
    host: Arc<RecHost>,
    sink: Arc<RecSink>,
) -> (mpsc::Sender<DriverInput>, DriverTask) {
    let params = DriverParams {
        dial,
        server_name: "127.0.0.1".to_owned(),
        view_width: 390,
        view_height: 844,
        scale: 3.0,
        monitors: 1,
    };
    let (input, input_rx) = mpsc::channel(256);
    let task = tokio::spawn(async move {
        let driver = VncDriver;
        driver.run(params, host, sink, input_rx).await
    });
    (input, task)
}

async fn joined(task: DriverTask) -> Result<(), AdapterError> {
    tokio::time::timeout(WAIT, task)
        .await
        .expect("driver timeout")
        .expect("driver task")
}

struct Run {
    host: Arc<RecHost>,
    sink: Arc<RecSink>,
    dial: Arc<PairDial>,
    servers: mpsc::UnboundedReceiver<FakeHandle>,
    input: mpsc::Sender<DriverInput>,
    task: DriverTask,
}

impl Run {
    fn start(scripts: Vec<ServerScript>, answers: Vec<Answer>) -> Self {
        let host = RecHost::new(answers);
        let sink = Arc::new(RecSink::default());
        let (server_tx, servers) = mpsc::unbounded_channel();
        let dial = Arc::new(PairDial {
            scripts: Mutex::new(scripts.into()),
            servers: server_tx,
            dials: AtomicUsize::new(0),
        });
        let (input, task) = spawn_driver(dial.clone(), host.clone(), sink.clone());
        Self {
            host,
            sink,
            dial,
            servers,
            input,
            task,
        }
    }

    /// One unauthenticated server; returns once the driver reports streaming.
    async fn streaming(script: ServerScript) -> (Self, FakeHandle) {
        let mut run = Self::start(vec![script], Vec::new());
        let server = run.server().await;
        let host = run.host.clone();
        until("streaming", || host.streamed()).await;
        (run, server)
    }

    async fn server(&mut self) -> FakeHandle {
        tokio::time::timeout(WAIT, self.servers.recv())
            .await
            .expect("server timeout")
            .expect("server handle")
    }

    async fn send(&self, input: DriverInput) {
        self.input.send(input).await.expect("driver input open");
    }

    async fn hid(&self, message: InputMsg) {
        self.send(DriverInput::Hid(message)).await;
    }

    /// The driver's own result; the input channel stays open meanwhile.
    async fn result(self) -> Result<(), AdapterError> {
        let Run { input, task, .. } = self;
        let result = joined(task).await;
        drop(input);
        result
    }

    /// The user closes the session: input closes, the driver returns.
    async fn close(self) -> Result<(), AdapterError> {
        let Run { input, task, .. } = self;
        drop(input);
        joined(task).await
    }
}

async fn until(what: &str, ready: impl Fn() -> bool) {
    let deadline = Instant::now() + WAIT;
    while !ready() {
        assert!(Instant::now() < deadline, "timed out waiting for {what}");
        tokio::time::sleep(Duration::from_millis(10)).await;
    }
}

fn inputs(log: &FakeLog) -> Vec<ClientMsg> {
    log.msgs
        .iter()
        .filter(|message| {
            matches!(
                message,
                ClientMsg::KeyEvent { .. }
                    | ClientMsg::PointerEvent { .. }
                    | ClientMsg::ClientCutText(_)
            )
        })
        .cloned()
        .collect()
}

/// Exactly `expected` input messages, in order. End `expected` with a marker
/// to show nothing was sent for the inputs before it.
async fn expect_inputs(server: &FakeHandle, expected: &[ClientMsg]) {
    let arrived = server
        .wait_for(|log| inputs(log).len() >= expected.len(), WAIT)
        .await;
    assert!(arrived, "server saw {:?}", inputs(&server.log()));
    assert_eq!(inputs(&server.log()), expected);
}

fn key(down: bool, keysym: u32) -> ClientMsg {
    ClientMsg::KeyEvent { down, keysym }
}

fn ptr(mask: u8, x: u16, y: u16) -> ClientMsg {
    ClientMsg::PointerEvent { mask, x, y }
}

fn mouse_move(x: f32, y: f32) -> InputMsg {
    InputMsg::MouseMove { x, y }
}

fn mouse_button(button: Button, down: bool, x: f32, y: f32) -> InputMsg {
    InputMsg::MouseButton {
        button,
        down,
        x,
        y,
        clicks: 1,
    }
}

fn wheel(dy: f32, phase: ScrollPhase) -> InputMsg {
    InputMsg::Scroll {
        dx: 0.0,
        dy,
        phase,
        momentum: ScrollPhase::None,
        unit: ScrollUnit::Pixel,
    }
}

fn touch(id: u8, phase: TouchPhase, x: f32, y: f32) -> InputMsg {
    InputMsg::Touch {
        id,
        phase,
        x,
        y,
        pressure: 1.0,
    }
}

fn offer(types: Vec<u8>, check: AuthCheck) -> ServerScript {
    ServerScript {
        security: SecurityPlan::Offer { types, check },
        ..ServerScript::new(800, 600)
    }
}

fn vnc_password(password: &str) -> ServerScript {
    offer(
        vec![security::VNC_AUTH],
        AuthCheck::VncPassword(password.to_owned()),
    )
}

#[tokio::test]
async fn none_streaming() {
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::Solid {
            rect: Rect::new(0, 0, 2, 2),
            bgra: [1, 2, 3, 255],
        }])],
        ..ServerScript::new(800, 600)
    };
    let (run, _server) = Run::streaming(script).await;
    let sink = run.sink.clone();
    until("first update", || sink.committed()).await;
    assert_eq!(sink.calls()[0], "resize 800x600");
    assert_eq!(sink.pixel(1, 1), [1, 2, 3, 255]);
    assert_eq!(run.host.log(), ["streaming 800x600 fake"]);
    assert!(run.host.asked().is_empty());
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn ard_flow() {
    let script = offer(
        vec![security::ARD],
        AuthCheck::Ard {
            user: "alice".to_owned(),
            password: "s3cret!".to_owned(),
            params: ArdTestParams::small(),
        },
    );
    let mut run = Run::start(vec![script], vec![Some((Some("alice"), "s3cret!"))]);
    let server = run.server().await;
    let host = run.host.clone();
    until("streaming", || host.streamed()).await;
    assert_eq!(
        host.asked(),
        [(vec![CredField::User, CredField::Password], false)]
    );
    assert_eq!(
        host.log(),
        ["authenticating", "auth_succeeded", "streaming 800x600 fake"]
    );
    assert_eq!(
        server.log().ard_credentials,
        Some(("alice".to_owned(), "s3cret!".to_owned()))
    );
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn vnc_password_only() {
    let mut run = Run::start(vec![vnc_password("pw")], vec![Some((None, "pw"))]);
    let server = run.server().await;
    let host = run.host.clone();
    until("streaming", || host.streamed()).await;
    assert_eq!(host.asked(), [(vec![CredField::Password], false)]);
    assert_eq!(
        host.log(),
        ["authenticating", "auth_succeeded", "streaming 800x600 fake"]
    );
    assert_eq!(server.log().auth_ok, Some(true));
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn retry_once() {
    let mut run = Run::start(
        vec![vnc_password("pw"), vnc_password("pw")],
        vec![Some((None, "bad")), Some((None, "pw"))],
    );
    let first = run.server().await;
    let second = run.server().await;
    let host = run.host.clone();
    until("streaming", || host.streamed()).await;
    assert_eq!(
        host.asked(),
        [
            (vec![CredField::Password], false),
            (vec![CredField::Password], true)
        ]
    );
    assert_eq!(run.dial.dials(), 2);
    assert_eq!(
        host.log(),
        [
            "authenticating",
            "authenticating",
            "auth_succeeded",
            "streaming 800x600 fake"
        ]
    );
    assert_eq!(first.log().auth_ok, Some(false));
    assert_eq!(second.log().auth_ok, Some(true));
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn three_failures() {
    let run = Run::start(
        vec![vnc_password("pw"), vnc_password("pw"), vnc_password("pw")],
        vec![Some((None, "a")), Some((None, "b")), Some((None, "c"))],
    );
    let (host, dial) = (run.host.clone(), run.dial.clone());
    assert_eq!(run.result().await, Err(AdapterError::AuthFailed));
    assert_eq!(dial.dials(), 3);
    assert_eq!(host.asked().len(), MAX_AUTH_ATTEMPTS as usize);
    let failed_before: Vec<bool> = host.asked().iter().map(|(_, failed)| *failed).collect();
    assert_eq!(failed_before, [false, true, true]);
    assert_eq!(
        host.log(),
        ["authenticating", "authenticating", "authenticating"]
    );
}

#[tokio::test]
async fn cancel() {
    let run = Run::start(vec![vnc_password("pw")], vec![None]);
    let host = run.host.clone();
    assert_eq!(run.result().await, Err(AdapterError::Cancelled));
    assert_eq!(host.asked(), [(vec![CredField::Password], false)]);
    assert!(host.log().is_empty());
}

#[tokio::test]
async fn unsupported() {
    let run = Run::start(vec![offer(vec![35], AuthCheck::Any)], Vec::new());
    let host = run.host.clone();
    let result = run.result().await;
    assert!(
        matches!(result, Err(AdapterError::UnsupportedSecurity(_))),
        "{result:?}"
    );
    assert_eq!(result.unwrap_err().rs_code(), rs_e::UNSUPPORTED);
    assert!(host.asked().is_empty());
}

#[tokio::test]
async fn dial_error() {
    let host = RecHost::new(Vec::new());
    let sink = Arc::new(RecSink::default());
    let (input, task) = spawn_driver(Arc::new(RefusedDial), host.clone(), sink.clone());
    assert_eq!(joined(task).await, Err(AdapterError::Refused));
    drop(input);
    assert!(host.log().is_empty());
    assert!(sink.calls().is_empty());
}

#[tokio::test]
async fn keys_with_shift() {
    let (run, server) = Run::streaming(ServerScript::new(800, 600)).await;
    run.hid(InputMsg::Key {
        usage: 0x04,
        down: true,
        mods: Mods::LSHIFT,
    })
    .await;
    expect_inputs(&server, &[key(true, 0xffe1), key(true, 0x41)]).await;
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn pointer() {
    let (run, server) = Run::streaming(ServerScript::new(801, 601)).await;
    run.hid(mouse_move(0.5, 0.5)).await;
    run.hid(mouse_button(Button::Right, true, 0.5, 0.5)).await;
    run.hid(mouse_button(Button::Back, true, 0.5, 0.5)).await;
    run.hid(mouse_move(0.0, 0.0)).await;
    expect_inputs(&server, &[ptr(0, 400, 300), ptr(4, 400, 300), ptr(4, 0, 0)]).await;
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn scroll() {
    let (run, server) = Run::streaming(ServerScript::new(801, 601)).await;
    run.hid(mouse_move(0.5, 0.5)).await;
    run.hid(wheel(10.0, ScrollPhase::Began)).await;
    // Half a step stays as a remainder; the next Began drops it.
    run.hid(wheel(5.0, ScrollPhase::Changed)).await;
    run.hid(wheel(5.0, ScrollPhase::Began)).await;
    run.hid(mouse_move(0.0, 0.0)).await;
    expect_inputs(
        &server,
        &[
            ptr(0, 400, 300),
            ptr(0b1000, 400, 300),
            ptr(0, 400, 300),
            ptr(0, 0, 0),
        ],
    )
    .await;
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn text_cad_secret() {
    let (run, server) = Run::streaming(ServerScript::new(800, 600)).await;
    run.hid(InputMsg::Text("aé".to_owned())).await;
    run.send(DriverInput::CtrlAltDel).await;
    run.send(DriverInput::TypeSecret(Zeroizing::new("pw".to_owned())))
        .await;
    run.hid(mouse_move(0.0, 0.0)).await;
    expect_inputs(
        &server,
        &[
            key(true, 0x61),
            key(false, 0x61),
            key(true, 0xe9),
            key(false, 0xe9),
            key(true, 0xffe3),
            key(true, 0xffe9),
            key(true, 0xffff),
            key(false, 0xffff),
            key(false, 0xffe9),
            key(false, 0xffe3),
            key(true, 0x70),
            key(false, 0x70),
            key(true, 0x77),
            key(false, 0x77),
            ptr(0, 0, 0),
        ],
    )
    .await;
    assert!(!inputs(&server.log()).contains(&key(true, 0xff0d)));
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn clipboard() {
    let (run, server) = Run::streaming(ServerScript::new(800, 600)).await;
    run.send(DriverInput::Clipboard("café".to_owned())).await;
    run.send(DriverInput::Clipboard("你好".to_owned())).await;
    run.hid(mouse_move(0.0, 0.0)).await;
    expect_inputs(
        &server,
        &[ClientMsg::ClientCutText(b"caf\xe9".to_vec()), ptr(0, 0, 0)],
    )
    .await;
    server.push(Step::CutText(b"hi".to_vec()));
    let host = run.host.clone();
    until("remote clipboard", || {
        host.log().iter().any(|entry| entry == "clipboard hi")
    })
    .await;
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn cursor_rgba() {
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::Cursor {
            hot_x: 0,
            hot_y: 0,
            w: 1,
            h: 1,
            bgra: vec![1, 2, 3, 255],
        }])],
        ..ServerScript::new(800, 600)
    };
    let (run, _server) = Run::streaming(script).await;
    let sink = run.sink.clone();
    until("cursor shape", || sink.cursor().is_some()).await;
    assert_eq!(sink.cursor(), Some((1, 1, 0, 0, vec![3, 2, 1, 255])));
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn touch_as_left_button() {
    let (run, server) = Run::streaming(ServerScript::new(801, 601)).await;
    run.hid(touch(7, TouchPhase::Began, 0.0, 0.0)).await;
    run.hid(touch(8, TouchPhase::Began, 0.5, 0.5)).await;
    run.hid(touch(8, TouchPhase::Moved, 0.25, 0.25)).await;
    run.hid(touch(7, TouchPhase::Moved, 0.5, 0.5)).await;
    run.hid(touch(8, TouchPhase::Ended, 0.5, 0.5)).await;
    run.hid(touch(7, TouchPhase::Ended, 0.0, 0.0)).await;
    expect_inputs(&server, &[ptr(1, 0, 0), ptr(1, 400, 300), ptr(0, 0, 0)]).await;
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn surface_mapping() {
    let script = ServerScript {
        steps: vec![Step::Update(vec![
            TestRect::Zrle {
                rect: Rect::new(0, 0, 2, 2),
                bgra: [5, 6, 7, 255].repeat(4),
            },
            TestRect::CopyRect {
                rect: Rect::new(2, 1, 2, 2),
                src_x: 0,
                src_y: 0,
            },
            TestRect::PointerPos { x: 3, y: 1 },
        ])],
        ..ServerScript::new(4, 3)
    };
    let (run, _server) = Run::streaming(script).await;
    let sink = run.sink.clone();
    until("update", || sink.committed()).await;
    assert_eq!(
        sink.calls(),
        [
            "resize 4x3",
            "fill 0,0 2x2",
            "copy 0,0 -> 2,1 2x2",
            "cursor_pos 3,1",
            "commit"
        ]
    );
    assert_eq!(sink.pixel(3, 2), [5, 6, 7, 255]);
    assert_eq!(sink.pixel(3, 0), [0, 0, 0, 255]);
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn server_close() {
    let script = ServerScript {
        steps: vec![Step::Close],
        ..ServerScript::new(800, 600)
    };
    let run = Run::start(vec![script], Vec::new());
    assert_eq!(
        run.result().await,
        Err(AdapterError::ServerClosed("eof".to_owned()))
    );
}

#[test]
fn map_vnc_error_table() {
    let cases = [
        (
            VncError::Io("reset".into()),
            AdapterError::Io("reset".into()),
        ),
        (
            VncError::Closed("eof".into()),
            AdapterError::ServerClosed("eof".into()),
        ),
        (
            VncError::Protocol("bad rect".into()),
            AdapterError::Protocol("bad rect".into()),
        ),
        (
            VncError::UnsupportedVersion("RFB 004.000".into()),
            AdapterError::UnsupportedSecurity("version RFB 004.000".into()),
        ),
        (
            VncError::UnsupportedSecurity(vec![33, 35]),
            AdapterError::UnsupportedSecurity("[33, 35]".into()),
        ),
        (
            VncError::AuthFailed("Authentication failed".into()),
            AdapterError::AuthFailed,
        ),
        (
            VncError::Refused("too many viewers".into()),
            AdapterError::ServerClosed("too many viewers".into()),
        ),
        (VncError::Cancelled, AdapterError::Cancelled),
    ];
    for (vnc, adapter) in cases {
        assert_eq!(map_vnc_error(vnc), adapter);
    }
}

#[test]
fn end_to_end_open() {
    let _serial = serial();
    let core = support::test_core();
    install_core(core.clone());
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    let script = ServerScript {
        steps: vec![Step::Update(vec![TestRect::Solid {
            rect: Rect::new(0, 0, 4, 3),
            bgra: [9, 8, 7, 255],
        }])],
        ..ServerScript::new(4, 3)
    };
    let (addr, _servers) = core
        .block_on(listen_tcp(vec![script]))
        .expect("listen on localhost");
    let target = AdapterTarget {
        protocol: AdapterProtocol::Vnc,
        host_id: None,
        addr: "127.0.0.1".to_owned(),
        port: Some(addr.port()),
    };
    let dial = Arc::new(NetDial::new(&core, &target, Duration::from_secs(15)));
    let id = core
        .block_on(session::open_with(
            &core,
            target,
            AdapterOpenOpts::default(),
            Arc::new(VncDriver),
            dial,
        ))
        .expect("open adapter session");
    let session_id = id.to_string();
    let state = |phase: &'static str| {
        let session_id = session_id.clone();
        move |events: &[serde_json::Value]| {
            events.iter().any(|event| {
                event["type"] == "adapter.state"
                    && event["sessionId"].as_str() == Some(session_id.as_str())
                    && event["phase"] == phase
            })
        }
    };
    let streaming = state("streaming");
    assert!(events.wait_for(&streaming, WAIT));
    let streamed = events
        .of_type("adapter.state")
        .into_iter()
        .find(|event| streaming(std::slice::from_ref(event)))
        .expect("streaming event");
    assert_eq!(streamed["width"], 4);
    assert_eq!(streamed["height"], 3);

    let mut buf = vec![0u8; 16 * 3];
    let mut copied = RsRect::default();
    let deadline = Instant::now() + WAIT;
    let seq = loop {
        // SAFETY: buf holds stride 16 x 3 rows; copied is a separate local.
        let seq =
            unsafe { rs_raw_copy(id, 0, buf.as_mut_ptr(), 16, 4, 3, RS_PIX_BGRA8, &mut copied) };
        if seq != 0 || Instant::now() >= deadline {
            break seq;
        }
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_ne!(seq, 0);
    assert_eq!(
        copied,
        RsRect {
            x: 0,
            y: 0,
            w: 4,
            h: 3
        }
    );
    assert!(buf.chunks_exact(4).all(|pixel| pixel == [9, 8, 7, 255]));

    assert!(session::close(&core, id));
    let ended = state("ended");
    assert!(events.wait_for(&ended, WAIT));
    let last = events
        .of_type("adapter.state")
        .into_iter()
        .find(|event| ended(std::slice::from_ref(event)))
        .expect("ended event");
    assert!(last.get("error").is_none(), "{last}");
    take_core();
}

fn vnc_screen(id: u32, x: u16, y: u16, w: u16, h: u16) -> Screen {
    Screen {
        id,
        rect: Rect::new(x, y, w, h),
        flags: 0,
    }
}

fn adapter_screen(x: u32, y: u32, width: u32, height: u32) -> AdapterScreen {
    AdapterScreen {
        x,
        y,
        width,
        height,
        guess: None,
    }
}

/// A 30×10 desktop: screen 1 is the left 20×10 (LEFT), screen 2 the right
/// 10×10 (RIGHT).
fn two_screen_update() -> Step {
    Step::Update(vec![
        TestRect::ExtendedDesktopSize {
            reason: 0,
            status: 0,
            w: 30,
            h: 10,
            screens: vec![vnc_screen(1, 0, 0, 20, 10), vnc_screen(2, 20, 0, 10, 10)],
        },
        TestRect::Solid {
            rect: Rect::new(0, 0, 20, 10),
            bgra: LEFT,
        },
        TestRect::Solid {
            rect: Rect::new(20, 0, 10, 10),
            bgra: RIGHT,
        },
    ])
}

fn apple_script(steps: Vec<Step>) -> ServerScript {
    ServerScript {
        version: *b"RFB 003.889\n",
        steps,
        ..ServerScript::new(30, 10)
    }
}

#[tokio::test]
async fn screens_reported_per_layout_change() {
    let (logs, _guard) = Logs::capture();
    let one_screen = Step::Update(vec![TestRect::ExtendedDesktopSize {
        reason: 0,
        status: 0,
        w: 30,
        h: 10,
        screens: vec![vnc_screen(1, 0, 0, 30, 10)],
    }]);
    let script = apple_script(vec![two_screen_update(), two_screen_update(), one_screen]);
    let (run, _server) = Run::streaming(script).await;
    let host = run.host.clone();
    until("two layouts", || host.reported_screens().len() == 2).await;
    let two = vec![adapter_screen(0, 0, 20, 10), adapter_screen(20, 0, 10, 10)];
    let one = vec![adapter_screen(0, 0, 30, 10)];
    // The repeated two-screen layout is not reported again.
    assert_eq!(host.reported_screens(), [two, one]);

    let reported = logs.lines("VNC screens reported");
    assert_eq!(reported.len(), 2, "{reported:?}");
    assert!(reported[0].contains("count=2"), "{reported:?}");
    assert!(reported[1].contains("count=1"), "{reported:?}");
    for line in &reported {
        assert!(line.contains("rfb=003.889"), "{line}");
        assert!(!line.contains("127.0.0.1"), "{line}");
    }
    assert!(logs.lines("VNC server sent no screen layout").is_empty());
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn no_screen_layout_logged_once() {
    let (logs, _guard) = Logs::capture();
    let update = || {
        Step::Update(vec![TestRect::Solid {
            rect: Rect::new(0, 0, 2, 2),
            bgra: LEFT,
        }])
    };
    let (run, server) = Run::streaming(apple_script(vec![update(), update()])).await;
    let requested = server
        .wait_for(
            |log| {
                log.msgs
                    .iter()
                    .filter(|msg| matches!(msg, ClientMsg::FramebufferUpdateRequest { .. }))
                    .count()
                    >= 3
            },
            WAIT,
        )
        .await;
    assert!(requested, "both updates applied");
    assert!(run.host.reported_screens().is_empty());
    let lines = logs.lines("VNC server sent no screen layout");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("rfb=003.889"), "{lines:?}");
    assert!(!lines[0].contains("127.0.0.1"), "{lines:?}");
    assert_eq!(run.close().await, Ok(()));
}

/// A 1600×720 bounding box without a server layout: 960×720 on the left
/// (LEFT) and 640×480 top-aligned on its right (RIGHT), black below it.
fn guess_update() -> Step {
    Step::Update(vec![
        TestRect::Solid {
            rect: Rect::new(0, 0, 960, 720),
            bgra: LEFT,
        },
        TestRect::Solid {
            rect: Rect::new(960, 0, 640, 480),
            bgra: RIGHT,
        },
    ])
}

fn guess_script(steps: Vec<Step>) -> ServerScript {
    ServerScript {
        version: *b"RFB 003.889\n",
        steps,
        ..ServerScript::new(1600, 720)
    }
}

fn guessed(x: u32, y: u32, width: u32, height: u32) -> AdapterScreen {
    AdapterScreen {
        guess: Some(ScreenGuess::BlackGap),
        ..adapter_screen(x, y, width, height)
    }
}

#[tokio::test]
async fn screens_guessed_without_server_layout() {
    let (logs, _guard) = Logs::capture();
    let (run, _server) = Run::streaming(guess_script(vec![guess_update()])).await;
    let host = run.host.clone();
    until("guessed layout", || !host.reported_screens().is_empty()).await;
    // Not the 800 + 800 halves the 1600 / 720 aspect alone would give.
    assert_eq!(
        host.reported_screens(),
        [vec![guessed(0, 0, 960, 720), guessed(960, 0, 640, 480)]]
    );
    let lines = logs.lines("VNC screens guessed");
    assert_eq!(lines.len(), 1, "{lines:?}");
    assert!(lines[0].contains("count=2"), "{lines:?}");
    assert!(lines[0].contains("black-gap"), "{lines:?}");
    assert!(!lines[0].contains("127.0.0.1"), "{lines:?}");
    assert!(logs.lines("VNC screens reported").is_empty());
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn server_layout_is_not_guessed() {
    let (logs, _guard) = Logs::capture();
    let mut update = vec![TestRect::ExtendedDesktopSize {
        reason: 0,
        status: 0,
        w: 1600,
        h: 720,
        screens: vec![vnc_screen(1, 0, 0, 1600, 720)],
    }];
    let Step::Update(pixels) = guess_update() else {
        unreachable!()
    };
    update.extend(pixels);
    let (run, server) = Run::streaming(guess_script(vec![Step::Update(update)])).await;
    let painted = server
        .wait_for(
            |log| {
                log.msgs
                    .iter()
                    .filter(|msg| matches!(msg, ClientMsg::FramebufferUpdateRequest { .. }))
                    .count()
                    >= 2
            },
            WAIT,
        )
        .await;
    assert!(painted, "the update was applied");
    // Longer than two guess samples take.
    tokio::time::sleep(Duration::from_millis(2_500)).await;
    assert_eq!(
        run.host.reported_screens(),
        [vec![adapter_screen(0, 0, 1600, 720)]]
    );
    assert!(logs.lines("VNC screens guessed").is_empty());
    assert_eq!(run.close().await, Ok(()));
}

/// rs_raw_copy into a tightly packed w × h BGRA buffer.
fn raw_copy(id: u64, w: u32, h: u32) -> (u64, RsRect, Vec<u8>) {
    let mut buf = vec![0xEE; w as usize * h as usize * 4];
    let mut copied = RsRect::default();
    // SAFETY: buf holds w * 4 bytes per row for h rows; copied is a separate local.
    let seq = unsafe {
        rs_raw_copy(
            id,
            0,
            buf.as_mut_ptr(),
            w as usize * 4,
            w,
            h,
            RS_PIX_BGRA8,
            &mut copied,
        )
    };
    (seq, copied, buf)
}

fn raw_pixel(buf: &[u8], w: u32, x: u32, y: u32) -> [u8; 4] {
    let at = (y as usize * w as usize + x as usize) * 4;
    buf[at..at + 4].try_into().unwrap()
}

#[test]
fn select_screen_crops_vnc_layout() {
    let _serial = serial();
    let core = support::test_core();
    install_core(core.clone());
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    let (servers_tx, _servers) = mpsc::unbounded_channel();
    let dial = Arc::new(PairDial {
        scripts: Mutex::new(vec![apple_script(vec![two_screen_update()])].into()),
        servers: servers_tx,
        dials: AtomicUsize::new(0),
    });
    let target = AdapterTarget {
        protocol: AdapterProtocol::Vnc,
        host_id: None,
        addr: "127.0.0.1".to_owned(),
        port: None,
    };
    let id = core
        .block_on(session::open_with(
            &core,
            target,
            AdapterOpenOpts::default(),
            Arc::new(VncDriver),
            dial,
        ))
        .expect("open adapter session");
    let session_id = id.to_string();
    assert!(events.wait_for(
        |events| events
            .iter()
            .any(|event| event["type"] == "adapter.screens" && event["sessionId"] == session_id),
        WAIT
    ));
    let screens = events.of_type("adapter.screens");
    assert_eq!(screens.len(), 1, "{screens:?}");
    assert_eq!(
        screens[0]["screens"],
        serde_json::json!([
            { "x": 0, "y": 0, "width": 20, "height": 10 },
            { "x": 20, "y": 0, "width": 10, "height": 10 },
        ])
    );

    let deadline = Instant::now() + WAIT;
    let whole = loop {
        let (seq, copied, buf) = raw_copy(id, 30, 10);
        if seq != 0 && raw_pixel(&buf, 30, 29, 9) == RIGHT {
            break (copied, buf);
        }
        assert!(Instant::now() < deadline, "first frame not copied");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        whole.0,
        RsRect {
            x: 0,
            y: 0,
            w: 30,
            h: 10
        }
    );

    assert_eq!(session::select_screen(&core, id, 1), Ok(()));
    let (seq, copied, buf) = raw_copy(id, 10, 10);
    assert_ne!(seq, 0);
    assert_eq!(
        copied,
        RsRect {
            x: 0,
            y: 0,
            w: 10,
            h: 10
        }
    );
    assert!(buf.chunks_exact(4).all(|pixel| pixel == RIGHT));
    assert_eq!(raw_copy(id, 30, 10).0, 0, "the whole desktop is not shown");

    assert_eq!(session::select_screen(&core, id, WHOLE_DESKTOP), Ok(()));
    let (seq, copied, buf) = raw_copy(id, 30, 10);
    assert_ne!(seq, 0);
    assert_eq!(
        copied,
        RsRect {
            x: 0,
            y: 0,
            w: 30,
            h: 10
        }
    );
    for y in 0..10 {
        for x in 0..30 {
            let expected = if x < 20 { LEFT } else { RIGHT };
            assert_eq!(raw_pixel(&buf, 30, x, y), expected, "({x}, {y})");
        }
    }

    assert!(session::close(&core, id));
    assert!(events.wait_for(
        |events| events.iter().any(|event| event["type"] == "adapter.state"
            && event["sessionId"] == session_id
            && event["phase"] == "ended"),
        WAIT
    ));
    take_core();
}

#[test]
fn select_screen_crops_guessed_layout() {
    let _serial = serial();
    let core = support::test_core();
    install_core(core.clone());
    let events = support::EventCollector::new();
    core.events().subscribe(events.clone());
    let (servers_tx, _servers) = mpsc::unbounded_channel();
    let dial = Arc::new(PairDial {
        scripts: Mutex::new(vec![guess_script(vec![guess_update()])].into()),
        servers: servers_tx,
        dials: AtomicUsize::new(0),
    });
    let target = AdapterTarget {
        protocol: AdapterProtocol::Vnc,
        host_id: None,
        addr: "127.0.0.1".to_owned(),
        port: None,
    };
    let id = core
        .block_on(session::open_with(
            &core,
            target,
            AdapterOpenOpts::default(),
            Arc::new(VncDriver),
            dial,
        ))
        .expect("open adapter session");
    let session_id = id.to_string();
    assert!(events.wait_for(
        |events| events
            .iter()
            .any(|event| event["type"] == "adapter.screens" && event["sessionId"] == session_id),
        WAIT
    ));
    let screens = events.of_type("adapter.screens");
    assert_eq!(screens.len(), 1, "{screens:?}");
    assert_eq!(
        screens[0]["screens"],
        serde_json::json!([
            { "x": 0, "y": 0, "width": 960, "height": 720, "guess": "blackGap" },
            { "x": 960, "y": 0, "width": 640, "height": 480, "guess": "blackGap" },
        ])
    );

    assert_eq!(session::select_screen(&core, id, 1), Ok(()));
    let deadline = Instant::now() + WAIT;
    let (copied, buf) = loop {
        let (seq, copied, buf) = raw_copy(id, 640, 480);
        if seq != 0 {
            break (copied, buf);
        }
        assert!(Instant::now() < deadline, "second screen not copied");
        std::thread::sleep(Duration::from_millis(10));
    };
    assert_eq!(
        copied,
        RsRect {
            x: 0,
            y: 0,
            w: 640,
            h: 480
        }
    );
    assert!(buf.chunks_exact(4).all(|pixel| pixel == RIGHT));
    assert_eq!(
        raw_copy(id, 1600, 720).0,
        0,
        "the whole desktop is not shown"
    );

    assert_eq!(session::select_screen(&core, id, WHOLE_DESKTOP), Ok(()));
    let (seq, _, buf) = raw_copy(id, 1600, 720);
    assert_ne!(seq, 0);
    assert_eq!(raw_pixel(&buf, 1600, 0, 0), LEFT);
    assert_eq!(raw_pixel(&buf, 1600, 1599, 0), RIGHT);
    assert_eq!(raw_pixel(&buf, 1600, 1599, 719), [0, 0, 0, 255]);

    assert!(session::close(&core, id));
    assert!(events.wait_for(
        |events| events.iter().any(|event| event["type"] == "adapter.state"
            && event["sessionId"] == session_id
            && event["phase"] == "ended"),
        WAIT
    ));
    take_core();
}

fn requests(log: &FakeLog) -> Vec<bool> {
    log.msgs
        .iter()
        .filter_map(|message| match message {
            ClientMsg::FramebufferUpdateRequest { incremental, .. } => Some(*incremental),
            _ => None,
        })
        .collect()
}

#[tokio::test(start_paused = true)]
async fn background_stops_requests_and_sink_but_keeps_clipboard_and_connection() {
    let (logs, _guard) = Logs::capture();
    let rect = Rect::new(0, 0, 8, 4);
    let update = |pixel: [u8; 4]| {
        Step::Update(vec![TestRect::Zrle {
            rect,
            bgra: pixel.repeat(32),
        }])
    };
    let (run, server) = Run::streaming(ServerScript {
        width: 8,
        height: 4,
        steps: vec![update(LEFT)],
        ..ServerScript::default()
    })
    .await;
    assert!(
        server
            .wait_for(|log| requests(log) == [false, true], WAIT)
            .await
    );
    until("first frame", || run.sink.committed()).await;
    let before = run.sink.calls();
    let visibility = run.host.background.as_ref().unwrap();
    visibility.send_replace(true);
    visibility.send_replace(true);
    run.hid(mouse_move(0.5, 0.5)).await;
    run.send(DriverInput::Clipboard("pause-barrier".into()))
        .await;
    assert!(
        server
            .wait_for(
                |log| inputs(log) == [ClientMsg::ClientCutText(b"pause-barrier".to_vec())],
                WAIT
            )
            .await
    );
    // Finish the request already in flight. ZRLE history must survive the pause.
    server.push(update(RIGHT));
    server.push(Step::CutText(b"still-connected".to_vec()));
    until("paused clipboard", || {
        run.host.log().contains(&"clipboard still-connected".into())
    })
    .await;
    assert_eq!(run.sink.calls(), before);
    assert_eq!(requests(&server.log()), [false, true]);
    assert_eq!(run.sink.pixel(7, 3), LEFT);

    // More than ten minutes of virtual time, observing every keepalive on wire.
    let encodings = |log: &FakeLog| {
        log.msgs
            .iter()
            .filter(|msg| matches!(msg, ClientMsg::SetEncodings(_)))
            .count()
    };
    for count in 2..=22 {
        tokio::time::advance(rs_vnc::client::BACKGROUND_KEEPALIVE).await;
        assert!(server.wait_for(|log| encodings(log) >= count, WAIT).await);
        assert!(!server.log().closed);
        assert_eq!(requests(&server.log()), [false, true]);
        assert_eq!(run.sink.calls(), before);
    }
    server.push(update([90, 80, 70, 255]));
    visibility.send_replace(false);
    visibility.send_replace(false);
    assert!(server.wait_for(|log| requests(log).len() == 4, WAIT).await);
    until("resumed full frame", || {
        run.sink.pixel(7, 3) == [90, 80, 70, 255]
    })
    .await;
    assert_eq!(requests(&server.log()), [false, true, false, true]);
    run.send(DriverInput::Clipboard("resume-barrier".into()))
        .await;
    assert!(server.wait_for(|log| inputs(log).len() == 2, WAIT).await);
    assert_eq!(requests(&server.log()), [false, true, false, true]);
    let changes = logs.lines("adapter background changed");
    assert_eq!(changes.len(), 2, "{changes:?}");
    assert!(changes.iter().all(|line| line.contains("session_id=42")
        && line.contains("stop-requests")
        && !line.contains("127.0.0.1")));
    assert_eq!(run.close().await, Ok(()));
}

#[tokio::test]
async fn visibility_set_before_connect_prevents_the_initial_frame_request() {
    let mut run = Run::start(vec![ServerScript::new(8, 4)], Vec::new());
    run.host.background.as_ref().unwrap().send_replace(true);
    let server = run.server().await;
    run.send(DriverInput::Clipboard("hidden-from-start".into()))
        .await;
    assert!(
        server
            .wait_for(
                |log| inputs(log) == [ClientMsg::ClientCutText(b"hidden-from-start".to_vec())],
                WAIT
            )
            .await
    );
    assert!(requests(&server.log()).is_empty());
    assert!(run.sink.calls().is_empty());
    server.push(Step::Update(vec![TestRect::Solid {
        rect: Rect::new(0, 0, 8, 4),
        bgra: LEFT,
    }]));
    run.host.background.as_ref().unwrap().send_replace(false);
    assert!(
        server
            .wait_for(|log| requests(log) == [false, true], WAIT)
            .await
    );
    until("initial full frame after resume", || {
        run.sink.pixel(7, 3) == LEFT
    })
    .await;
    assert_eq!(run.close().await, Ok(()));
}
