mod support;

use host_fake::{
    clip::ClipPlatform,
    harness::{self, TestHost},
};
use prost::Message;
use rs_mobile::rscore::{
    dto_core::{OpenIntent, OpenOpts},
    ffi_session::*,
    runtime::{install_core, take_core, Core},
    tables::SessionTable,
};
use rsp_client::{
    session::{SessionEvent, SessionStateKind},
    CursorEvent, ReceivedFrame,
};
use rsp_core::{
    hid::{InputMsg, ScrollPhase, ScrollUnit, TouchPhase},
    input::{InputEvent, MouseButton},
    pb,
    trust::Permission,
    video::Frame,
};
use rsp_transport::{
    dial::DefaultDialer,
    dialer::Dialer,
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    Conn, ConnError, ControlStream, DatagramKind, QuicListener, TransportKind,
};
use std::{
    ffi::{c_void, CStr},
    future::Future,
    mem::{offset_of, size_of},
    net::SocketAddr,
    pin::Pin,
    ptr, slice,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    thread::{self, ThreadId},
    time::{Duration, Instant},
};

const DEADLINE: Duration = Duration::from_secs(2);
const TEST_STEP_TIMEOUT: Duration = Duration::from_secs(20);
static GLOBAL_CORE: Mutex<()> = Mutex::new(());

async fn step<T>(name: &str, future: impl Future<Output = T>) -> T {
    tokio::time::timeout(TEST_STEP_TIMEOUT, future)
        .await
        .unwrap_or_else(|_| panic!("timed out after {TEST_STEP_TIMEOUT:?}: {name}"))
}

fn block_on_step<T>(core: &Core, name: &str, future: impl Future<Output = T>) -> T {
    core.block_on(step(name, future))
}

// A timeout can unwind while the event pump is still delivering callbacks.
// Detach before the corresponding Output is dropped, including on failure.
struct DetachOnDrop(u64);

impl Drop for DetachOnDrop {
    fn drop(&mut self) {
        rs_session_detach(self.0);
    }
}

// Decode feedback covers only the trailing 200 ms. Keep simulating a decoder
// until the host observes its reports; a single burst can expire before a late
// feedback tick. Keep the receive future alive across ticks (no cancelled reads).
async fn report_until<T>(mut report: impl FnMut(), receive: impl Future<Output = T>) -> T {
    tokio::pin!(receive);
    let mut tick = tokio::time::interval(Duration::from_millis(20));
    tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
    loop {
        tokio::select! {
            biased;
            result = &mut receive => return result,
            _ = tick.tick() => report(),
        }
    }
}

fn serial_core() -> MutexGuard<'static, ()> {
    let started = Instant::now();
    loop {
        match GLOBAL_CORE.try_lock() {
            Ok(guard) => return guard,
            Err(TryLockError::Poisoned(error)) => return error.into_inner(),
            Err(TryLockError::WouldBlock) => {
                assert!(
                    started.elapsed() < TEST_STEP_TIMEOUT,
                    "timed out waiting for GLOBAL_CORE test serialization"
                );
                thread::sleep(Duration::from_millis(10));
            }
        }
    }
}

// Keep the actual QUIC handle for outbound pointer accounting. The current
// Host's on_pointer_datagram is a stub; reliable inputs still exercise
// Host injection, while this checks that FFI emits (and filters) datagrams.
struct RecordingDialer {
    inner: DefaultDialer,
    connection: Mutex<Option<quinn::Connection>>,
}

impl Dialer for RecordingDialer {
    fn dial<'a>(
        &'a self,
        addr: SocketAddr,
        kind: TransportKind,
    ) -> Pin<Box<dyn Future<Output = Result<Conn, ConnError>> + Send + 'a>> {
        Box::pin(async move {
            let conn = self.inner.dial(addr, kind).await?;
            *self.connection.lock().unwrap() = conn.as_quic();
            Ok(conn)
        })
    }
}

impl RecordingDialer {
    fn datagrams_sent(&self) -> u64 {
        self.connection
            .lock()
            .unwrap()
            .as_ref()
            .unwrap()
            .stats()
            .frame_tx
            .datagram
    }
}

#[derive(Debug)]
struct FrameCopy {
    id: u32,
    key: u8,
    synced: u8,
    display: u8,
    epoch: u8,
    codec: u8,
    captured: u64,
    local: i64,
    received: u64,
    vps: Vec<u8>,
    sps: Vec<u8>,
    pps: Vec<u8>,
    avcc: Vec<u8>,
    empty_sets_null: bool,
}
#[derive(Debug)]
struct EventCopy {
    kind: i32,
    width: u32,
    height: u32,
    fps: u32,
    display: u32,
    epoch: u32,
    codec: u8,
    visible: u8,
    x: f32,
    y: f32,
    rtt: u64,
    offset: i64,
    shape: u64,
    hot_x: u32,
    hot_y: u32,
    scale: f32,
    data: Vec<u8>,
    message: String,
}
#[derive(Default)]
struct Output {
    frames: Mutex<Vec<FrameCopy>>,
    events: Mutex<Vec<EventCopy>>,
    threads: Mutex<Vec<ThreadId>>,
}

unsafe fn copy_bytes(ptr: *const u8, len: usize) -> Vec<u8> {
    if ptr.is_null() || len == 0 {
        Vec::new()
    } else {
        // SAFETY: Caller forwards the callback's valid pointer/length pair.
        unsafe { slice::from_raw_parts(ptr, len).to_vec() }
    }
}
extern "C" fn frame(ctx: *mut c_void, f: *const RsFrame) {
    // SAFETY: Output is held through detach; frame data is copied while borrowed.
    unsafe {
        let out = &*ctx.cast::<Output>();
        let f = &*f;
        out.frames.lock().unwrap().push(FrameCopy {
            id: f.frame_id,
            key: f.keyframe,
            synced: f.clock_synced,
            display: f.display_id,
            epoch: f.epoch,
            codec: f.codec,
            captured: f.capture_ts_us,
            local: f.capture_local_us,
            received: f.received_us,
            vps: copy_bytes(f.vps, f.vps_len),
            sps: copy_bytes(f.sps, f.sps_len),
            pps: copy_bytes(f.pps, f.pps_len),
            avcc: copy_bytes(f.avcc, f.avcc_len),
            empty_sets_null: f.vps.is_null() && f.sps.is_null() && f.pps.is_null(),
        });
        out.threads.lock().unwrap().push(thread::current().id());
    }
}
extern "C" fn event(ctx: *mut c_void, e: *const RsEvent) {
    // SAFETY: Output is live and event pointers are consumed during the callback.
    unsafe {
        let out = &*ctx.cast::<Output>();
        let e = &*e;
        out.events.lock().unwrap().push(EventCopy {
            kind: e.kind,
            width: e.width,
            height: e.height,
            fps: e.fps,
            display: e.display_id,
            epoch: e.epoch,
            codec: e.codec,
            visible: e.cursor_visible,
            x: e.cursor_x,
            y: e.cursor_y,
            rtt: e.rtt_us,
            offset: e.offset_us,
            shape: e.shape_id,
            hot_x: e.hot_x,
            hot_y: e.hot_y,
            scale: e.scale,
            data: copy_bytes(e.data, e.data_len),
            message: if e.message.is_null() {
                String::new()
            } else {
                CStr::from_ptr(e.message).to_string_lossy().into_owned()
            },
        });
        out.threads.lock().unwrap().push(thread::current().id());
    }
}

struct ScriptHost {
    addr: SocketAddr,
    fingerprint: [u8; 32],
    listener: QuicListener,
}

impl ScriptHost {
    async fn bind() -> Self {
        let identity = LocalIdentity::generate_software(CertKind::Host).unwrap();
        let listener = QuicListener::bind(
            "127.0.0.1:0".parse().unwrap(),
            server_config(&identity).unwrap(),
        )
        .await
        .unwrap();
        Self {
            addr: listener.local_addr().unwrap(),
            fingerprint: identity.fingerprint(),
            listener,
        }
    }

    async fn accept(&self) -> (Conn, ControlStream) {
        let incoming = step(
            "host.accept: incoming QUIC connection",
            self.listener.accept(),
        )
        .await
        .unwrap();
        let conn = step("host.accept: TLS handshake", incoming.accept())
            .await
            .expect("host.accept: TLS handshake failed");
        let control = step("host.accept: control stream", conn.accept_control())
            .await
            .expect("host.accept: control stream failed");
        (conn, control)
    }
}

async fn recv_message(control: &ControlStream, name: &str) -> pb::envelope::Msg {
    let payload = step(name, control.recv())
        .await
        .unwrap_or_else(|error| panic!("{name}: {error}"))
        .unwrap_or_else(|| panic!("{name}: unexpected end of control stream"));
    let envelope = pb::Envelope::decode(&payload[..]).unwrap();
    envelope.msg.unwrap()
}

async fn recv_command(control: &ControlStream, name: &str) -> pb::envelope::Msg {
    step(name, async {
        loop {
            match recv_message(control, name).await {
                pb::envelope::Msg::Ping(ping) => {
                    send_message(
                        control,
                        pb::Pong {
                            t1_us: ping.t1_us,
                            t2_us: rsp_core::clock::unix_micros(),
                        },
                    )
                    .await;
                }
                message => return message,
            }
        }
    })
    .await
}

async fn recv_keyframe_request(control: &ControlStream, name: &str) -> pb::KeyframeRequest {
    step(name, async {
        loop {
            if let pb::envelope::Msg::KeyframeRequest(request) = recv_command(control, name).await {
                return request;
            }
        }
    })
    .await
}

async fn send_message(control: &ControlStream, message: impl Into<pb::Envelope>) {
    let envelope = message.into();
    let name = match &envelope.msg {
        Some(pb::envelope::Msg::ServerHello(_)) => "send ServerHello",
        Some(pb::envelope::Msg::AuthResult(_)) => "send AuthResult",
        Some(pb::envelope::Msg::StreamConfig(_)) => "send StreamConfig",
        Some(pb::envelope::Msg::Pong(_)) => "send Pong",
        Some(pb::envelope::Msg::Goodbye(_)) => "send Goodbye",
        _ => "send control message",
    };
    let framed = rsp_core::control::encode(&envelope);
    step(name, control.send(&framed[4..]))
        .await
        .unwrap_or_else(|error| panic!("{name}: {error}"));
}

async fn hello(control: &ControlStream) {
    assert!(matches!(
        recv_message(control, "receive ClientHello").await,
        pb::envelope::Msg::ClientHello(_)
    ));
    send_message(
        control,
        pb::ServerHello {
            protocol: 1,
            features: rsp_core::features::RATE_FEEDBACK,
            host_name: "scripted-host".into(),
            ..Default::default()
        },
    )
    .await;
}

async fn auth(control: &ControlStream) {
    send_message(
        control,
        pb::AuthResult {
            status: pb::auth_result::Status::Ok as i32,
            role: pb::Role::Controller as i32,
            session_token: vec![7; 16],
            ..Default::default()
        },
    )
    .await;
}
#[track_caller]
fn wait_until(name: &str, f: impl Fn() -> bool) {
    let started = Instant::now();
    while !f() {
        assert!(
            started.elapsed() < DEADLINE,
            "timed out after {DEADLINE:?}: {name}"
        );
        thread::sleep(Duration::from_millis(10));
    }
}
fn valid_avcc(data: &[u8]) -> bool {
    let mut rest = data;
    while !rest.is_empty() {
        let Some(prefix) = rest.get(..4) else {
            return false;
        };
        let len = u32::from_be_bytes(prefix.try_into().unwrap()) as usize;
        if len == 0 || rest.len() - 4 < len {
            return false;
        }
        rest = &rest[4 + len..];
    }
    !data.is_empty()
}
fn opts() -> OpenOpts {
    OpenOpts {
        intent: OpenIntent::Control,
        display_id: None,
        quality: None,
        mode: None,
        audio: None,
        max_fps: None,
        max_width: None,
        codec: None,
    }
}

fn check_layout() {
    #[cfg(target_pointer_width = "64")]
    {
        assert_eq!(size_of::<RsFrame>(), 104);
        assert_eq!(
            [
                offset_of!(RsFrame, frame_id),
                offset_of!(RsFrame, keyframe),
                offset_of!(RsFrame, clock_synced),
                offset_of!(RsFrame, display_id),
                offset_of!(RsFrame, epoch),
                offset_of!(RsFrame, codec),
                offset_of!(RsFrame, capture_ts_us),
                offset_of!(RsFrame, capture_local_us),
                offset_of!(RsFrame, received_us),
                offset_of!(RsFrame, vps),
                offset_of!(RsFrame, vps_len),
                offset_of!(RsFrame, sps),
                offset_of!(RsFrame, sps_len),
                offset_of!(RsFrame, pps),
                offset_of!(RsFrame, pps_len),
                offset_of!(RsFrame, avcc),
                offset_of!(RsFrame, avcc_len),
            ],
            [0, 4, 5, 6, 7, 8, 16, 24, 32, 40, 48, 56, 64, 72, 80, 88, 96]
        );
        assert_eq!(size_of::<RsEvent>(), 136);
        assert_eq!(
            [
                offset_of!(RsEvent, kind),
                offset_of!(RsEvent, width),
                offset_of!(RsEvent, height),
                offset_of!(RsEvent, fps),
                offset_of!(RsEvent, display_id),
                offset_of!(RsEvent, epoch),
                offset_of!(RsEvent, codec),
                offset_of!(RsEvent, cursor_visible),
                offset_of!(RsEvent, cursor_x),
                offset_of!(RsEvent, cursor_y),
                offset_of!(RsEvent, rtt_us),
                offset_of!(RsEvent, offset_us),
                offset_of!(RsEvent, code),
                offset_of!(RsEvent, shape_id),
                offset_of!(RsEvent, hot_x),
                offset_of!(RsEvent, hot_y),
                offset_of!(RsEvent, scale),
                offset_of!(RsEvent, data),
                offset_of!(RsEvent, data_len),
                offset_of!(RsEvent, message),
            ],
            [0, 4, 8, 12, 16, 20, 24, 25, 28, 32, 40, 48, 56, 64, 72, 76, 80, 88, 96, 104]
        );
    }
}

#[test]
fn native_session_frames_inputs_events_detach_and_layout() {
    let _serial = serial_core();
    check_layout();
    let out = Arc::new(Output::default());
    let ctx = Arc::as_ptr(&out).cast_mut().cast::<c_void>();
    // SAFETY: Context and callback functions remain alive throughout the test.
    unsafe {
        assert!(!rs_session_attach(99, ctx, Some(frame), Some(event)));
    }
    rs_session_detach(99);
    rs_input_pointer(99, 0.1, 0.2);
    rs_input_release_all(99);
    let c = support::test_core_with(|_, opts| opts.worker_threads = 2);
    let _signer = support::with_identity(&c, "ffi");
    install_core(c.clone());
    // The default pixel-free encoder emits no parameter sets. The bundled
    // clip supplies real HEVC VPS/SPS/PPS while retaining FakePlatform input logs.
    let video_platform = ClipPlatform::new();
    let platform = video_platform.inner().clone();
    let (host, entry, pump, dialer) = block_on_step(&c, "open clip session", async {
        let host = TestHost::start_with_platform(video_platform.clone(), Default::default()).await;
        let identity = c.local_identity().unwrap();
        host.pair(&identity, Permission::Control);
        let mut params = harness::direct_params(&host, identity);
        params.start.fps = 30;
        let endpoint = quinn::Endpoint::client(([127, 0, 0, 1], 0).into()).unwrap();
        let dialer = Arc::new(RecordingDialer {
            inner: DefaultDialer::new(endpoint, &params.local_identity).unwrap(),
            connection: Mutex::new(None),
        });
        params.dialer = Some(dialer.clone());
        let (handle, mut events) = harness::open(params).await.unwrap();
        let entry = c
            .state::<SessionTable>()
            .insert(&host.host_id, "Host", opts(), c.now_us());
        entry.set_handle(Some(handle));
        let e = entry.clone();
        let pump = c.spawn(async move {
            while let Some(ev) = step("native event pump: receive event", events.recv()).await {
                match ev {
                    SessionEvent::Frame(f) => {
                        e.video.deliver_frame(&f);
                    }
                    SessionEvent::StreamConfig(config) => e.video.deliver_config(&config),
                    SessionEvent::Cursor(cursor) => e.video.deliver_cursor(&cursor),
                    SessionEvent::CursorMessage(msg) => e.video.deliver_cursor_msg(&msg),
                    SessionEvent::ClockSync { rtt_us, offset_us } => {
                        e.video.deliver_clock(rtt_us, offset_us)
                    }
                    SessionEvent::Ended { reason, .. } => {
                        e.video.close(&reason);
                        break;
                    }
                    _ => {}
                }
            }
        });
        (host, entry, pump, dialer)
    });
    let id = entry.id;
    wait_until("native session becomes Streaming", || {
        entry.handle().unwrap().state() == SessionStateKind::Streaming
    });
    let handle = entry.handle().unwrap().clone();
    entry
        .video
        .set_attach_hook(Box::new(move || handle.request_keyframe()));
    let unconnected = c
        .state::<SessionTable>()
        .insert("other", "Other", opts(), c.now_us());
    // SAFETY: All contexts are live; NULL callbacks are deliberately rejected.
    unsafe {
        assert!(!rs_session_attach(99, ctx, Some(frame), Some(event)));
        assert!(!rs_session_attach(
            unconnected.id,
            ctx,
            Some(frame),
            Some(event)
        ));
        assert!(!rs_session_attach(id, ctx, None, Some(event)));
        assert!(rs_session_attach(id, ctx, Some(frame), Some(event)));
    }
    let _detach = DetachOnDrop(id);
    wait_until("receive native frames and StreamConfig", || {
        let frames = out.frames.lock().unwrap();
        frames.len() >= 5
            && frames
                .iter()
                .any(|f| f.key == 1 && !f.vps.is_empty() && !f.sps.is_empty() && !f.pps.is_empty())
            && out
                .events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.kind == RS_EV_STREAM_CONFIG)
    });
    {
        let frames = out.frames.lock().unwrap();
        assert!(frames.iter().all(|f| f.codec == 1 && valid_avcc(&f.avcc)));
        assert!(frames
            .iter()
            .any(|f| f.key == 1 && !f.vps.is_empty() && !f.sps.is_empty() && !f.pps.is_empty()));
        assert!(frames
            .iter()
            .filter(|f| f.key == 0)
            .all(|f| f.empty_sets_null));
        let events = out.events.lock().unwrap();
        let config = events
            .iter()
            .find(|e| e.kind == RS_EV_STREAM_CONFIG)
            .unwrap();
        assert_eq!(
            (config.width, config.height, config.fps, config.codec),
            (640, 360, 30, 1)
        );
        assert_eq!(config.display, 1);
        assert!(frames
            .iter()
            .all(|f| u32::from(f.display) == config.display && u32::from(f.epoch) == config.epoch));
    }
    let release_before = platform.release_count();
    let datagrams_before = dialer.datagrams_sent();
    rs_input_pointer(id, 0.25, 0.75);
    rs_input_button(id, 0, true, 0.5, 0.5, 1);
    rs_input_button(id, 0, false, 0.5, 0.5, 1);
    rs_input_release_all(id);
    rs_input_button(id, 9, true, 0.5, 0.5, 1);
    rs_input_button(id, -1, true, 0.5, 0.5, 1);
    rs_input_pointer(id, f32::NAN, 0.1);
    rs_input_scroll(id, 2.0, 3.0, 2, 0);
    rs_input_scroll(id, 1.0, 1.0, 4, 0);
    rs_input_scroll(id, 1.0, 1.0, 0, 4);
    rs_input_scroll(id, f32::INFINITY, 1.0, 0, 0);
    rs_input_touch(id, 7, 0, 0.2, 0.3, 0.6);
    rs_input_touch(id, 7, 9, 0.2, 0.3, 0.6);
    rs_input_touch(id, 7, 0, 0.2, 0.3, f32::NAN);
    wait_until("host receives valid FFI inputs", || {
        let events = platform.injected_events();
        dialer.datagrams_sent() > datagrams_before
            && [true, false].iter().all(|down| {
                events.contains(&InputEvent::MouseButton {
                    button: MouseButton::Left,
                    down: *down,
                    x: 0.5,
                    y: 0.5,
                    clicks: 1,
                })
            })
            && platform.release_count() > release_before
            && platform
                .injected_msgs()
                .iter()
                .any(|m| matches!(m, InputMsg::Touch { id: 7, .. }))
    });
    let datagrams_after = dialer.datagrams_sent();
    rs_input_pointer(id, f32::NAN, 0.1);
    rs_input_pointer(id, 0.1, f32::INFINITY);
    rs_input_pointer(99, 0.1, 0.2);
    rs_input_pointer(unconnected.id, 0.1, 0.2);
    thread::sleep(Duration::from_millis(50));
    assert_eq!(dialer.datagrams_sent(), datagrams_after);
    let msgs = platform.injected_msgs();
    assert_eq!(
        msgs.iter()
            .filter(|m| matches!(m, InputMsg::MouseButton { .. }))
            .count(),
        2
    );
    assert_eq!(
        msgs.iter()
            .filter(|m| matches!(m, InputMsg::Touch { .. }))
            .count(),
        1
    );
    assert_eq!(
        msgs.iter()
            .filter(|m| matches!(m, InputMsg::Scroll { .. }))
            .count(),
        1
    );
    assert!(msgs.contains(&InputMsg::Scroll {
        dx: 2.0,
        dy: 3.0,
        phase: ScrollPhase::Changed,
        momentum: ScrollPhase::None,
        unit: ScrollUnit::Pixel
    }));
    assert!(msgs.contains(&InputMsg::Touch {
        id: 7,
        phase: TouchPhase::Began,
        x: 0.2,
        y: 0.3,
        pressure: 0.6
    }));
    rs_session_detach(id);
    let count = out.threads.lock().unwrap().len();
    thread::sleep(Duration::from_millis(300));
    assert_eq!(out.threads.lock().unwrap().len(), count);

    // Reattach for the actual session Ended event.
    // SAFETY: Same live context.
    unsafe {
        assert!(rs_session_attach(id, ctx, Some(frame), Some(event)));
    }
    entry.handle().unwrap().close();
    wait_until("receive session Closed callback", || {
        out.events
            .lock()
            .unwrap()
            .iter()
            .any(|e| e.kind == RS_EV_CLOSED)
    });
    block_on_step(&c, "join session event pump", pump).unwrap();
    // SAFETY: Closed session rejects attachment.
    unsafe {
        assert!(!rs_session_attach(id, ctx, Some(frame), Some(event)));
    }
    assert_eq!(
        out.events
            .lock()
            .unwrap()
            .iter()
            .filter(|e| e.kind == RS_EV_CLOSED)
            .count(),
        1
    );

    // Synthetic events on an independent tap with a connected handle cover H.264
    // conversion, exact timestamps, every cursor variant and embedded-NUL close.
    // The closed original tap remains closed; this does not use the JS session API.
    block_on_step(&c, "stop fake host", host.stop());
    let (host, synthetic, _synthetic_events) = block_on_step(&c, "open synthetic session", async {
        let host = TestHost::start_with_platform(platform, Default::default()).await;
        let identity = c.local_identity().unwrap();
        host.pair(&identity, Permission::Control);
        let (handle, events) = harness::open(harness::direct_params(&host, identity))
            .await
            .unwrap();
        let e = c
            .state::<SessionTable>()
            .insert(&host.host_id, "Synthetic", opts(), c.now_us());
        e.set_handle(Some(handle));
        (host, e, events)
    });
    wait_until("synthetic session becomes Streaming", || {
        synthetic.handle().unwrap().state() == SessionStateKind::Streaming
    });
    let panicking = c
        .state::<SessionTable>()
        .insert("panic", "Panic", opts(), c.now_us());
    panicking.set_handle(synthetic.handle());
    panicking
        .video
        .set_attach_hook(Box::new(|| panic!("test attach hook panic")));
    // SAFETY: A failed attachment must release its use of this live context.
    unsafe {
        assert!(!rs_session_attach(
            panicking.id,
            ctx,
            Some(frame),
            Some(event)
        ));
    }
    assert!(!panicking.video.is_attached());
    c.state::<SessionTable>().remove(panicking.id);
    let synthetic_out = Arc::new(Output::default());
    let synthetic_ctx = Arc::as_ptr(&synthetic_out).cast_mut().cast::<c_void>();
    // SAFETY: Context remains live until the synthetic tap is closed/detached.
    unsafe {
        assert!(rs_session_attach(
            synthetic.id,
            synthetic_ctx,
            Some(frame),
            Some(event)
        ));
    }
    let _synthetic_detach = DetachOnDrop(synthetic.id);
    let e = synthetic.clone();
    let delivery = c.handle().spawn(async move {
        e.video.deliver_config(&pb::StreamConfig {
            codec: pb::Codec::H264 as i32,
            width: 640,
            height: 480,
            fps: 30,
            display_id: 2,
            epoch: 3,
            cursor_x: 0.1,
            cursor_y: 0.9,
            cursor_visible: true,
            ..Default::default()
        });
        let mut f = ReceivedFrame {
            frame: Frame {
                frame_id: 123,
                epoch: 3,
                display_id: 2,
                flags: 0,
                keyframe: true,
                capture_ts_us: 100,
                data: vec![
                    0, 0, 0, 1, 0x67, 0x11, 0, 0, 0, 1, 0x68, 0x22, 0, 0, 1, 0x65, 0x33,
                ],
            },
            received_us: 120,
            capture_local_us: Some(110),
        };
        e.video.deliver_frame(&f);
        f.frame.keyframe = false;
        f.capture_local_us = None;
        e.video.deliver_frame(&f);
        e.video.deliver_cursor(&CursorEvent {
            x: 0.4,
            y: 0.6,
            visible: true,
        });
        e.video.deliver_cursor_msg(&pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Shape(pb::CursorShape {
                id: 17,
                width: 10,
                height: 20,
                hot_x: 2,
                hot_y: 3,
                scale: 2.0,
                png: vec![1, 2, 3],
            })),
        });
        e.video.deliver_cursor_msg(&pb::CursorMsg {
            msg: Some(pb::cursor_msg::Msg::Use(17)),
        });
        for hidden in [true, false] {
            e.video.deliver_cursor_msg(&pb::CursorMsg {
                msg: Some(pb::cursor_msg::Msg::Hidden(hidden)),
            });
        }
        e.video.deliver_cursor_msg(&pb::CursorMsg::default());
        e.video.deliver_clock(50, -10);
        e.video.close("bye\0reason");
    });
    block_on_step(&c, "join synthetic event delivery", delivery).unwrap();
    {
        let frames = synthetic_out.frames.lock().unwrap();
        assert_eq!(frames.len(), 2);
        let f = &frames[0];
        assert_eq!(
            (f.id, f.display, f.epoch, f.codec, f.synced, f.captured, f.local, f.received),
            (123, 2, 3, 2, 1, 100, 110, 120)
        );
        assert!(f.vps.is_empty());
        assert_eq!(f.sps, [0x67, 0x11]);
        assert_eq!(f.pps, [0x68, 0x22]);
        assert_eq!(f.avcc, [0, 0, 0, 2, 0x65, 0x33]);
        assert!(valid_avcc(&f.avcc));
        assert!(frames[1].empty_sets_null);
        assert_eq!((frames[1].synced, frames[1].local), (0, 0));
        let events = synthetic_out.events.lock().unwrap();
        assert_eq!(
            events.iter().map(|e| e.kind).collect::<Vec<_>>(),
            [2, 6, 7, 8, 9, 9, 3, 5]
        );
        assert_eq!((events[0].x, events[0].y, events[0].visible), (0.1, 0.9, 1));
        assert_eq!((events[1].x, events[1].y, events[1].visible), (0.4, 0.6, 1));
        assert_eq!(
            (
                events[2].shape,
                events[2].width,
                events[2].height,
                events[2].hot_x,
                events[2].hot_y,
                events[2].scale
            ),
            (17, 10, 20, 2, 3, 2.0)
        );
        assert_eq!(events[2].data, [1, 2, 3]);
        assert_eq!(events[3].shape, 17);
        assert_eq!((events[4].visible, events[5].visible), (0, 1));
        assert_eq!((events[6].rtt, events[6].offset), (50, -10));
        assert_eq!(events[7].message, "bye?reason");
    }
    assert!(out
        .threads
        .lock()
        .unwrap()
        .iter()
        .chain(synthetic_out.threads.lock().unwrap().iter())
        .all(|id| *id != thread::current().id()));
    rs_session_detach(synthetic.id);
    // SAFETY: Missing/closed IDs are safe, including NULL context for rejection.
    unsafe {
        assert!(!rs_session_attach(
            synthetic.id,
            ptr::null_mut(),
            Some(frame),
            Some(event)
        ));
    }
    synthetic.handle().unwrap().close();
    block_on_step(&c, "stop fake host", host.stop());
    take_core();
    c.shutdown();
}

#[test]
fn session_keyframe_and_decode_reports_reach_the_host() {
    let _serial = serial_core();
    let out = Arc::new(Output::default());
    let ctx = Arc::as_ptr(&out).cast_mut().cast::<c_void>();
    let c = support::test_core_with(|_, opts| opts.worker_threads = 2);
    let _signer = support::with_identity(&c, "ffi");
    install_core(c.clone());
    let host = block_on_step(&c, "bind scripted host", ScriptHost::bind());
    let (opened_tx, opened_rx) = std::sync::mpsc::channel();
    c.spawn({
        let identity = c.local_identity().unwrap();
        let addr = host.addr;
        let fingerprint = host.fingerprint;
        async move {
            let mut params = rsp_client::SessionParams::single_shared(addr, fingerprint, identity);
            // Match the host's step timeout: saturated test CPUs can exhaust
            // the default 3 s dial budget before the TLS handshake completes.
            params.dial_options.timeout = TEST_STEP_TIMEOUT;
            let result = step(
                "open scripted client session",
                rsp_client::Session::open(params),
            )
            .await;
            let _ = opened_tx.send(result);
        }
    });
    let (conn, control) = block_on_step(&c, "host.accept scripted session", host.accept());
    block_on_step(&c, "exchange ClientHello/ServerHello", hello(&control));
    block_on_step(&c, "send AuthResult", auth(&control));
    assert!(matches!(
        block_on_step(
            &c,
            "receive StartStream",
            recv_message(&control, "receive StartStream")
        ),
        pb::envelope::Msg::StartStream(_)
    ));
    let (handle, mut events) = opened_rx
        .recv_timeout(TEST_STEP_TIMEOUT)
        .expect("session opened")
        .expect("session open");
    let entry = c
        .state::<SessionTable>()
        .insert("scripted", "Host", opts(), c.now_us());
    entry.set_handle(Some(handle));
    let id = entry.id;
    let e = entry.clone();
    let pump = c.spawn(async move {
        while let Some(event) = step("scripted event pump: receive event", events.recv()).await {
            match event {
                SessionEvent::Frame(frame) => {
                    e.video.deliver_frame(&frame);
                }
                SessionEvent::StreamConfig(config) => e.video.deliver_config(&config),
                SessionEvent::Ended { reason, .. } => {
                    e.video.close(&reason);
                    break;
                }
                _ => {}
            }
        }
    });
    wait_until("scripted session becomes Streaming", || {
        c.state::<SessionTable>()
            .get(id)
            .is_some_and(|entry| entry.handle().unwrap().state() == SessionStateKind::Streaming)
    });
    // SAFETY: Context and callbacks remain live until detach and session close.
    unsafe {
        assert!(rs_session_attach(id, ctx, Some(frame), Some(event)));
    }
    let _detach = DetachOnDrop(id);

    block_on_step(&c, "send config and receive frame 5", async {
        send_message(
            &control,
            pb::StreamConfig {
                epoch: 1,
                fps: 30,
                codec: pb::Codec::Hevc as i32,
                ..Default::default()
            },
        )
        .await;
        // Control streams and QUIC datagrams have no cross-channel ordering.
        // Wait for the config callback before sending the only test frame.
        wait_until("receive scripted StreamConfig callback", || {
            out.events
                .lock()
                .unwrap()
                .iter()
                .any(|e| e.kind == RS_EV_STREAM_CONFIG)
        });
        let packets = rsp_core::video::packetize(
            &[0, 0, 0, 1, 0x65, 0x11],
            rsp_core::video::FrameMeta {
                flags: rsp_core::video::FLAG_KEYFRAME,
                display_id: 0,
                epoch: 1,
                frame_id: 5,
                capture_ts_us: rsp_core::clock::unix_micros(),
            },
            1150,
            0.0,
        )
        .unwrap();
        for packet in packets {
            conn.send_datagram(DatagramKind::Video, packet.slice(1..))
                .unwrap();
        }
        wait_until("receive scripted frame 5 callback", || {
            out.frames.lock().unwrap().len() == 1
        });
    });
    block_on_step(&c, "receive BACKLOG keyframe request", async {
        rs_session_request_keyframe(999, 4);
        rs_session_report_decoded(999, 1, 2, 3);
        rs_session_request_keyframe(id, 1);
        rs_session_request_keyframe(id, 9);
        rs_session_request_keyframe(id, 0);
        tokio::time::sleep(Duration::from_millis(50)).await;
        rs_session_request_keyframe(id, 4);
        let first = recv_keyframe_request(&control, "receive BACKLOG keyframe request").await;
        assert_eq!(first.reason, pb::keyframe_request::Reason::Backlog as i32);
        assert_eq!(first.last_good_frame, 5);
    });
    block_on_step(&c, "receive DECODER_RESET keyframe request", async {
        tokio::time::sleep(Duration::from_millis(350)).await;
        rs_session_request_keyframe(id, 2);
        let second =
            recv_keyframe_request(&control, "receive DECODER_RESET keyframe request").await;
        assert_eq!(
            second.reason,
            pb::keyframe_request::Reason::DecoderReset as i32
        );
        assert_eq!(second.last_good_frame, 5);
    });
    block_on_step(&c, "receive nonzero decode feedback", async {
        let feedback = report_until(|| rs_session_report_decoded(id, 3000, 20000, 1), async {
            loop {
                match recv_command(&control, "receive decode feedback message").await {
                    pb::envelope::Msg::Feedback(feedback)
                        if feedback.decode_p50_us > 0 && feedback.render_fps > 0 =>
                    {
                        break feedback;
                    }
                    pb::envelope::Msg::Feedback(feedback) => {
                        eprintln!(
                            "waiting for decode feedback: decode_p50_us={}, render_fps={}, decode_queue={}",
                            feedback.decode_p50_us, feedback.render_fps, feedback.decode_queue
                        );
                    }
                    _ => {}
                }
            }
        })
        .await;
        assert_eq!(feedback.decode_p50_us, 3000);
        assert_eq!(feedback.decode_queue, 1);
        assert!(feedback.render_fps > 0);
    });
    block_on_step(&c, "close scripted session", async {
        send_message(
            &control,
            pb::Goodbye {
                reason: pb::goodbye::Reason::UserClosed as i32,
                message: String::new(),
            },
        )
        .await;
        let _ = step("finish scripted control stream", control.finish()).await;
        step("wait for scripted connection close", conn.closed()).await;
    });
    block_on_step(&c, "join session event pump", pump).unwrap();
    take_core();
    c.shutdown();
}

#[tokio::test(start_paused = true)]
async fn decode_reports_survive_a_missed_feedback_window() {
    use rsp_core::stats::{ClientStatsCollector, FEEDBACK_WINDOW_US};

    let started = tokio::time::Instant::now();
    let now_us = || started.elapsed().as_micros() as u64;
    let collector = Mutex::new(ClientStatsCollector::new());
    for _ in 0..5 {
        collector.lock().unwrap().on_decoded(now_us(), 3000, 20000);
    }
    // Deterministically reproduce the old one-shot burst expiring before the
    // feedback timer runs, as can happen when the runtime is descheduled.
    tokio::time::sleep(Duration::from_micros(FEEDBACK_WINDOW_US + 1)).await;
    let stale = collector.lock().unwrap().feedback(now_us(), 1, 1, 0);
    assert_eq!(stale.decode_p50_us, 0);
    assert_eq!(stale.render_fps, 0);

    let feedback = step(
        "receive feedback after a missed sampling window",
        report_until(
            || collector.lock().unwrap().on_decoded(now_us(), 3000, 20000),
            async {
                // The next observation is also more than a window away.
                tokio::time::sleep(Duration::from_micros(2 * FEEDBACK_WINDOW_US)).await;
                collector.lock().unwrap().feedback(now_us(), 1, 1, 0)
            },
        ),
    )
    .await;
    assert_eq!(feedback.decode_p50_us, 3000);
    assert_eq!(feedback.decode_queue, 1);
    assert!(feedback.render_fps > 0);
}
