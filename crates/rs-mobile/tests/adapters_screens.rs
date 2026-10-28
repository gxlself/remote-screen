//! adapters_select_screen: show one remote monitor at a time.

mod support;

use std::sync::{mpsc, Arc, Mutex, MutexGuard};
use std::time::{Duration, Instant};

use async_trait::async_trait;
use rs_mobile::rscore::adapters::{
    api::{adapters_close, adapters_select_screen},
    driver::{
        AdapterError, Dial, DriverHost, DriverInput, DriverParams, FrameSink, ProtocolDriver,
        SrcPixels,
    },
    dto::{AdapterOpenOpts, AdapterProtocol, AdapterScreen, AdapterTarget},
    ffi_raw::{rs_raw_copy, RsRect, RS_PIX_BGRA8},
    rawframe::{RawConsumer, RawFrame, RawRegistry},
    session::open_with,
    ADAPTER_ID_BASE,
};
use rs_mobile::rscore::api_sessions::sessions_key;
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::ffi_session::{
    rs_input_button, rs_input_pointer, rs_input_scroll, rs_input_touch, RsEvent, RS_EV_CURSOR_POS,
    RS_EV_STREAM_CONFIG,
};
use rs_mobile::rscore::runtime::{install_core, take_core, Core};
use rsp_core::hid::{Button, InputMsg, Mods, ScrollPhase, ScrollUnit, TouchPhase};
use rsp_transport::net::NetStream;
use tokio::sync::mpsc as tokio_mpsc;

const WAIT: Duration = Duration::from_secs(10);
const LEFT: [u8; 4] = [10, 20, 30, 255];
const RIGHT: [u8; 4] = [40, 50, 60, 255];

/// install_core is process-wide.
static LOCK: Mutex<()> = Mutex::new(());

fn serial() -> MutexGuard<'static, ()> {
    LOCK.lock().unwrap_or_else(|error| error.into_inner())
}

fn screen(x: u32, y: u32, width: u32, height: u32) -> AdapterScreen {
    AdapterScreen {
        x,
        y,
        width,
        height,
        guess: None,
    }
}

fn two_screens() -> [AdapterScreen; 2] {
    [screen(0, 0, 1024, 768), screen(1024, 0, 1024, 768)]
}

fn rect(x: u32, y: u32, w: u32, h: u32) -> RsRect {
    RsRect { x, y, w, h }
}

#[derive(Debug, PartialEq)]
enum Seen {
    Config {
        size: (u32, u32),
        epoch: u32,
        codec: u8,
        src: [f32; 4],
        display: (u32, u32),
    },
    Pos(f32, f32),
    Ready(u64, u32, u32),
}

fn config(size: (u32, u32), epoch: u32) -> Seen {
    Seen::Config {
        size,
        epoch,
        codec: 0,
        src: [0.0, 0.0, 1.0, 1.0],
        display: (0, 0),
    }
}

/// The native consumer's view; cursor shapes are not under test.
#[derive(Default)]
struct Rec {
    log: Mutex<Vec<Seen>>,
}

impl Rec {
    fn take(&self) -> Vec<Seen> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }
}

impl RawConsumer for Rec {
    fn ready(&self, seq: u64, width: u32, height: u32) {
        self.log
            .lock()
            .unwrap()
            .push(Seen::Ready(seq, width, height));
    }

    fn event(&self, e: &RsEvent) {
        let seen = match e.kind {
            RS_EV_STREAM_CONFIG => Seen::Config {
                size: (e.width, e.height),
                epoch: e.epoch,
                codec: e.codec,
                src: [e.src_x, e.src_y, e.src_w, e.src_h],
                display: (e.display_w, e.display_h),
            },
            RS_EV_CURSOR_POS => Seen::Pos(e.cursor_x, e.cursor_y),
            _ => return,
        };
        self.log.lock().unwrap().push(seen);
    }
}

struct Started {
    host: Arc<dyn DriverHost>,
    sink: Arc<dyn FrameSink>,
}

/// Hands the host and sink to the test, then forwards every input.
struct FakeDriver {
    protocol: AdapterProtocol,
    started: mpsc::Sender<Started>,
    inputs: mpsc::Sender<DriverInput>,
}

#[async_trait]
impl ProtocolDriver for FakeDriver {
    fn protocol(&self) -> AdapterProtocol {
        self.protocol
    }

    async fn run(
        &self,
        _params: DriverParams,
        host: Arc<dyn DriverHost>,
        sink: Arc<dyn FrameSink>,
        mut input: tokio_mpsc::Receiver<DriverInput>,
    ) -> Result<(), AdapterError> {
        let _ = self.started.send(Started { host, sink });
        while let Some(input) = input.recv().await {
            let _ = self.inputs.send(input);
        }
        Ok(())
    }
}

struct NoDial;

#[async_trait]
impl Dial for NoDial {
    async fn dial(&self) -> Result<NetStream, AdapterError> {
        Err(AdapterError::Refused)
    }
}

struct Session {
    id: u64,
    host: Arc<dyn DriverHost>,
    sink: Arc<dyn FrameSink>,
    frame: Arc<RawFrame>,
    inputs: mpsc::Receiver<DriverInput>,
    rec: Arc<Rec>,
}

impl Session {
    fn next_input(&self) -> InputMsg {
        match self.inputs.recv_timeout(WAIT).expect("driver input") {
            DriverInput::Hid(msg) => msg,
            _ => panic!("expected a HID input"),
        }
    }

    fn pointer_at(&self, x: f32, y: f32) -> (f32, f32) {
        rs_input_pointer(self.id, x, y);
        match self.next_input() {
            InputMsg::MouseMove { x, y } => (x, y),
            other => panic!("expected a mouse move, got {other:?}"),
        }
    }
}

struct Env {
    core: Arc<Core>,
    _serial: MutexGuard<'static, ()>,
}

impl Env {
    fn new() -> Self {
        let serial = serial();
        let core = support::test_core();
        install_core(core.clone());
        Self {
            core,
            _serial: serial,
        }
    }

    /// A 2048×768 desktop, left half LEFT and right half RIGHT, committed at
    /// seq 1 with a consumer attached and its replay taken.
    fn open(&self, protocol: AdapterProtocol) -> Session {
        let (started, started_rx) = mpsc::channel();
        let (inputs, inputs_rx) = mpsc::channel();
        let driver = Arc::new(FakeDriver {
            protocol,
            started,
            inputs,
        });
        let target = AdapterTarget {
            protocol,
            host_id: None,
            addr: "127.0.0.1".to_owned(),
            port: None,
        };
        let id = self
            .core
            .block_on(open_with(
                &self.core,
                target,
                AdapterOpenOpts::default(),
                driver,
                Arc::new(NoDial),
            ))
            .expect("open adapter session");
        let Started { host, sink } = started_rx.recv_timeout(WAIT).expect("driver started");
        sink.resize(2048, 768);
        sink.fill(0, 0, 1024, 768, LEFT);
        sink.fill(1024, 0, 1024, 768, RIGHT);
        sink.commit();
        let frame = self.core.state::<RawRegistry>().get(id).expect("raw frame");
        let rec = Arc::new(Rec::default());
        assert!(frame.attach(rec.clone()));
        assert_eq!(
            rec.take(),
            [config((2048, 768), 1), Seen::Ready(1, 2048, 768)]
        );
        Session {
            id,
            host,
            sink,
            frame,
            inputs: inputs_rx,
            rec,
        }
    }

    fn close(&self, session: Session) {
        assert_eq!(adapters_close(session.id), Ok(()));
        let deadline = Instant::now() + WAIT;
        while self.core.state::<RawRegistry>().get(session.id).is_some() {
            assert!(Instant::now() < deadline, "session did not end");
            std::thread::sleep(Duration::from_millis(10));
        }
    }
}

impl Drop for Env {
    fn drop(&mut self) {
        take_core();
    }
}

/// rs_raw_copy into a tightly packed BGRA buffer of w × h.
fn copy(id: u64, since: u64, w: u32, h: u32, dst: &mut [u8]) -> (u64, RsRect) {
    assert_eq!(dst.len(), w as usize * h as usize * 4);
    let mut copied = rect(9, 9, 9, 9);
    // SAFETY: dst is exclusive and exactly w * h * 4 bytes; copied is a
    // separate writable local.
    let seq = unsafe {
        rs_raw_copy(
            id,
            since,
            dst.as_mut_ptr(),
            w as usize * 4,
            w,
            h,
            RS_PIX_BGRA8,
            &mut copied,
        )
    };
    (seq, copied)
}

fn pixel(dst: &[u8], w: u32, x: u32, y: u32) -> &[u8] {
    let start = (y as usize * w as usize + x as usize) * 4;
    &dst[start..start + 4]
}

fn assert_near((x, y): (f32, f32), (ex, ey): (f32, f32)) {
    assert!(
        (x - ex).abs() < 1e-4 && (y - ey).abs() < 1e-4,
        "({x}, {y}) is not ({ex}, {ey})"
    );
}

#[test]
fn select_crops_copy() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Rdp);
    session.host.screens(&two_screens());
    assert!(session.rec.take().is_empty());

    assert_eq!(adapters_select_screen(session.id, 1), Ok(()));
    assert_eq!(
        session.rec.take(),
        [config((1024, 768), 2), Seen::Ready(2, 1024, 768)]
    );
    let mut dst = vec![0xEE; 1024 * 768 * 4];
    assert_eq!(
        copy(session.id, 0, 1024, 768, &mut dst),
        (2, rect(0, 0, 1024, 768))
    );
    assert!(dst.chunks_exact(4).all(|pixel| pixel == RIGHT));

    let block: Vec<u8> = (0..64u8).flat_map(|n| [n, 255 - n, 7, 255]).collect();
    session
        .sink
        .put(1100, 10, 8, 8, &block, 8 * 4, SrcPixels::Bgra8);
    session.sink.commit();
    assert_eq!(session.rec.take(), [Seen::Ready(3, 1024, 768)]);
    assert_eq!(
        copy(session.id, 2, 1024, 768, &mut dst),
        (3, rect(76, 10, 8, 8))
    );
    for row in 0..8 {
        let start = ((10 + row) * 1024 + 76) * 4;
        assert_eq!(&dst[start..start + 32], &block[row * 32..row * 32 + 32]);
    }
    assert_eq!(pixel(&dst, 1024, 75, 10), RIGHT);
    assert_eq!(pixel(&dst, 1024, 84, 17), RIGHT);
    assert_eq!(pixel(&dst, 1024, 76, 18), RIGHT);
    env.close(session);
}

#[test]
fn wrong_size_rejected() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Rdp);
    session.host.screens(&two_screens());
    assert_eq!(adapters_select_screen(session.id, 1), Ok(()));

    let mut desktop = vec![0xEE; 2048 * 768 * 4];
    assert_eq!(
        copy(session.id, 0, 2048, 768, &mut desktop),
        (0, RsRect::default())
    );
    assert!(desktop.iter().all(|byte| *byte == 0xEE));
    let mut shown = vec![0xEE; 1024 * 768 * 4];
    assert_eq!(
        copy(session.id, 0, 1024, 768, &mut shown),
        (2, rect(0, 0, 1024, 768))
    );
    env.close(session);
}

#[test]
fn outside_changes_empty() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Rdp);
    session.host.screens(&two_screens());
    assert_eq!(adapters_select_screen(session.id, 0), Ok(()));
    let mut dst = vec![0xEE; 1024 * 768 * 4];
    assert_eq!(
        copy(session.id, 0, 1024, 768, &mut dst),
        (2, rect(0, 0, 1024, 768))
    );
    assert!(dst.chunks_exact(4).all(|pixel| pixel == LEFT));

    session.sink.fill(1500, 100, 10, 10, [1, 2, 3, 255]);
    session.sink.fill(1024, 0, 1, 1, [1, 2, 3, 255]);
    session.sink.commit();
    let before = dst.clone();
    assert_eq!(
        copy(session.id, 2, 1024, 768, &mut dst),
        (3, RsRect::default())
    );
    assert_eq!(dst, before);

    // A change across the edge is cut at the monitor.
    session.sink.fill(1020, 0, 8, 2, [4, 5, 6, 255]);
    session.sink.commit();
    assert_eq!(
        copy(session.id, 3, 1024, 768, &mut dst),
        (4, rect(1020, 0, 4, 2))
    );
    assert_eq!(pixel(&dst, 1024, 1023, 1), [4, 5, 6, 255]);
    assert_eq!(pixel(&dst, 1024, 1019, 0), LEFT);
    env.close(session);
}

#[test]
fn pointer_offset() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Rdp);
    session.host.screens(&two_screens());
    assert_eq!(adapters_select_screen(session.id, 1), Ok(()));

    assert_near(
        session.pointer_at(10.0 / 1024.0, 20.0 / 768.0),
        (1034.0 / 2048.0, 20.0 / 768.0),
    );
    // Out-of-range input stays on the shown monitor.
    assert_near(session.pointer_at(1.5, -0.5), (1.0, 0.0));

    rs_input_button(session.id, 0, true, 0.5, 0.25, 1);
    match session.next_input() {
        InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x,
            y,
            clicks: 1,
        } => assert_near((x, y), (0.75, 0.25)),
        other => panic!("expected a button, got {other:?}"),
    }
    rs_input_touch(session.id, 3, 0, 0.0, 1.0, 0.5);
    match session.next_input() {
        InputMsg::Touch {
            id: 3,
            phase: TouchPhase::Began,
            x,
            y,
            pressure,
        } => {
            assert_near((x, y), (0.5, 1.0));
            assert_eq!(pressure, 0.5);
        }
        other => panic!("expected a touch, got {other:?}"),
    }
    rs_input_scroll(session.id, 0.25, -0.5, 1, 0);
    assert_eq!(
        session.next_input(),
        InputMsg::Scroll {
            dx: 0.25,
            dy: -0.5,
            phase: ScrollPhase::Began,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        }
    );
    sessions_key(session.id, 0x04, true, 0).unwrap();
    assert_eq!(
        session.next_input(),
        InputMsg::Key {
            usage: 0x04,
            down: true,
            mods: Mods::NONE,
        }
    );

    assert_eq!(adapters_select_screen(session.id, 0), Ok(()));
    assert_near(
        session.pointer_at(10.0 / 1024.0, 20.0 / 768.0),
        (10.0 / 2048.0, 20.0 / 768.0),
    );
    assert_eq!(adapters_select_screen(session.id, 255), Ok(()));
    assert_eq!(session.pointer_at(0.125, 0.5), (0.125, 0.5));
    env.close(session);
}

#[test]
fn cursor_pos_offset() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Rdp);
    session.host.screens(&two_screens());
    assert_eq!(adapters_select_screen(session.id, 1), Ok(()));
    session.rec.take();

    session.sink.cursor_pos(1030, 5);
    assert_eq!(session.rec.take(), [Seen::Pos(6.0 / 1023.0, 5.0 / 767.0)]);
    session.sink.cursor_pos(10, 5);
    assert_eq!(session.rec.take(), [Seen::Pos(-1.0, -1.0)]);

    // A new consumer gets the hidden position too.
    session.frame.detach();
    assert!(session.frame.attach(session.rec.clone()));
    assert_eq!(
        session.rec.take(),
        [
            config((1024, 768), 2),
            Seen::Pos(-1.0, -1.0),
            Seen::Ready(2, 1024, 768)
        ]
    );

    // Switching monitors reports the pointer relative to the new one.
    assert_eq!(adapters_select_screen(session.id, 0), Ok(()));
    assert_eq!(
        session.rec.take(),
        [
            config((1024, 768), 3),
            Seen::Pos(10.0 / 1023.0, 5.0 / 767.0),
            Seen::Ready(3, 1024, 768)
        ]
    );
    assert_eq!(adapters_select_screen(session.id, 255), Ok(()));
    assert_eq!(
        session.rec.take(),
        [
            config((2048, 768), 4),
            Seen::Pos(10.0 / 2047.0, 5.0 / 767.0),
            Seen::Ready(4, 2048, 768)
        ]
    );
    env.close(session);
}

#[test]
fn back_to_desktop() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Rdp);
    session.host.screens(&two_screens());
    assert_eq!(adapters_select_screen(session.id, 1), Ok(()));
    let mut shown = vec![0xEE; 1024 * 768 * 4];
    assert_eq!(
        copy(session.id, 0, 1024, 768, &mut shown),
        (2, rect(0, 0, 1024, 768))
    );
    session.rec.take();

    assert_eq!(adapters_select_screen(session.id, 255), Ok(()));
    assert_eq!(
        session.rec.take(),
        [config((2048, 768), 3), Seen::Ready(3, 2048, 768)]
    );
    let mut desktop = vec![0xEE; 2048 * 768 * 4];
    assert_eq!(
        copy(session.id, 2, 2048, 768, &mut desktop),
        (3, rect(0, 0, 2048, 768))
    );
    assert_eq!(pixel(&desktop, 2048, 0, 0), LEFT);
    assert_eq!(pixel(&desktop, 2048, 2047, 767), RIGHT);
    assert_eq!(copy(session.id, 3, 1024, 768, &mut shown).0, 0);

    // Showing the whole desktop again changes nothing.
    assert_eq!(adapters_select_screen(session.id, 255), Ok(()));
    assert!(session.rec.take().is_empty());
    env.close(session);
}

#[test]
fn out_of_range() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Rdp);
    session.host.screens(&two_screens());
    let out_of_range = Err(CoreError::internal("screen index out of range"));
    assert_eq!(adapters_select_screen(session.id, 2), out_of_range);
    assert!(session.rec.take().is_empty());
    assert_eq!(session.frame.view(), None);

    assert_eq!(adapters_select_screen(session.id, 1), Ok(()));
    session.rec.take();
    // The desktop shrinks before the new layout arrives: the view is clamped.
    session.sink.resize(1280, 720);
    assert_eq!(session.rec.take(), [config((256, 720), 3)]);
    assert_eq!(
        session.frame.view(),
        Some((rect(1024, 0, 256, 720), 1280, 720))
    );
    session.sink.fill(0, 0, 1280, 720, LEFT);
    session.sink.commit();
    assert_eq!(session.rec.take(), [Seen::Ready(3, 256, 720)]);

    session.host.screens(&[screen(0, 0, 1280, 720)]);
    assert_eq!(
        session.rec.take(),
        [config((1280, 720), 4), Seen::Ready(4, 1280, 720)]
    );
    let mut dst = vec![0xEE; 1280 * 720 * 4];
    assert_eq!(
        copy(session.id, 3, 1280, 720, &mut dst),
        (4, rect(0, 0, 1280, 720))
    );
    assert_eq!(adapters_select_screen(session.id, 1), out_of_range);

    // The selection survives a new layout that still has the monitor.
    session
        .host
        .screens(&[screen(0, 0, 640, 720), screen(640, 0, 640, 720)]);
    assert_eq!(
        session.rec.take(),
        [config((640, 720), 5), Seen::Ready(5, 640, 720)]
    );
    assert_eq!(
        session.frame.view(),
        Some((rect(0, 0, 640, 720), 1280, 720))
    );
    env.close(session);
}

#[test]
fn vnc_noop() {
    let env = Env::new();
    let session = env.open(AdapterProtocol::Vnc);
    assert_eq!(adapters_select_screen(session.id, 0), Ok(()));
    assert_eq!(adapters_select_screen(session.id, 255), Ok(()));
    assert!(session.rec.take().is_empty());
    assert_eq!(session.frame.view(), None);
    assert_eq!(
        adapters_select_screen(session.id, 1),
        Err(CoreError::internal("screen index out of range"))
    );
    assert_eq!(session.pointer_at(0.25, 0.75), (0.25, 0.75));
    let mut dst = vec![0xEE; 2048 * 768 * 4];
    assert_eq!(
        copy(session.id, 0, 2048, 768, &mut dst),
        (1, rect(0, 0, 2048, 768))
    );
    env.close(session);
}

#[test]
fn unknown_id() {
    let id = ADAPTER_ID_BASE + 999;
    {
        let env = Env::new();
        for index in [0, 1, 255] {
            assert_eq!(adapters_select_screen(id, index), adapters_close(id));
            assert_eq!(adapters_select_screen(id, index), Ok(()));
        }
        let session = env.open(AdapterProtocol::Rdp);
        session.host.screens(&two_screens());
        let ended = session.id;
        env.close(session);
        assert_eq!(adapters_select_screen(ended, 1), Ok(()));
    }
    let _serial = serial();
    take_core();
    assert_eq!(adapters_select_screen(id, 0), adapters_close(id));
    assert_eq!(
        adapters_select_screen(id, 0),
        Err(CoreError::internal("core not initialized"))
    );
}
