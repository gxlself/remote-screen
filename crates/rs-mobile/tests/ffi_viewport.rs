mod support;

use prost::Message;
use rs_mobile::rscore::{
    dto_core::{OpenIntent, OpenOpts},
    ffi_session::{
        rs_session_attach, rs_session_detach, RsEvent, RsFrame, RS_EV_CLOSED, RS_EV_STREAM_CONFIG,
    },
    ffi_viewport::{crop_src, rs_session_set_viewport, RS_VIEWPORT_ACTIVE, RS_VIEWPORT_CROP_OK},
    runtime::{install_core, take_core, Core},
    tables::SessionTable,
};
use rsp_client::session::{SessionEvent, SessionStateKind};
use rsp_core::pb;
use rsp_transport::{
    identity::{CertKind, LocalIdentity},
    tls::server_config,
    Conn, ControlStream, QuicListener,
};
use std::{
    ffi::c_void,
    future::Future,
    mem::{offset_of, size_of},
    net::SocketAddr,
    sync::{Arc, Mutex, MutexGuard, TryLockError},
    thread,
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

// Detach before Output is dropped, including when a test assertion unwinds.
struct DetachOnDrop(u64);

impl Drop for DetachOnDrop {
    fn drop(&mut self) {
        rs_session_detach(self.0);
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

#[derive(Debug, Clone, Copy)]
struct EventCopy {
    kind: i32,
    epoch: u32,
    src_x: f32,
    src_y: f32,
    src_w: f32,
    src_h: f32,
    display_w: u32,
    display_h: u32,
}

impl EventCopy {
    fn src(self) -> [f32; 4] {
        [self.src_x, self.src_y, self.src_w, self.src_h]
    }
}

#[derive(Default)]
struct Output {
    events: Mutex<Vec<EventCopy>>,
}

extern "C" fn frame(_ctx: *mut c_void, _f: *const RsFrame) {}

extern "C" fn event(ctx: *mut c_void, e: *const RsEvent) {
    // SAFETY: Output is held through detach; the borrowed event is copied here.
    unsafe {
        let out = &*ctx.cast::<Output>();
        let e = &*e;
        out.events.lock().unwrap().push(EventCopy {
            kind: e.kind,
            epoch: e.epoch,
            src_x: e.src_x,
            src_y: e.src_y,
            src_w: e.src_w,
            src_h: e.src_h,
            display_w: e.display_w,
            display_h: e.display_h,
        });
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
                pb::envelope::Msg::Feedback(_) => {}
                message => return message,
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
            features: rsp_core::features::RATE_FEEDBACK | rsp_core::features::VIEWPORT,
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

fn with_scripted_session(test: impl FnOnce(&Core, u64, &ControlStream, &Output)) {
    let _serial = serial_core();
    let out = Arc::new(Output::default());
    let ctx = Arc::as_ptr(&out).cast_mut().cast::<c_void>();
    let c = support::test_core_with(|_, opts| opts.worker_threads = 2);
    let _signer = support::with_identity(&c, "ffi-viewport");
    install_core(c.clone());
    let host = block_on_step(&c, "bind scripted host", ScriptHost::bind());
    let (opened_tx, opened_rx) = std::sync::mpsc::channel();
    c.spawn({
        let identity = c.local_identity().unwrap();
        let addr = host.addr;
        let fingerprint = host.fingerprint;
        async move {
            let mut params = rsp_client::SessionParams::single_shared(addr, fingerprint, identity);
            // Match the host's timeout when test CPUs are saturated.
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
            recv_command(&control, "receive StartStream")
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

    test(&c, id, &control, &out);

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
    // Non-config events must retain empty_event's zeros for all six new fields.
    let closed = *out
        .events
        .lock()
        .unwrap()
        .iter()
        .find(|e| e.kind == RS_EV_CLOSED)
        .expect("receive CLOSED callback");
    assert_eq!(closed.src(), [0.0; 4]);
    assert_eq!((closed.display_w, closed.display_h), (0, 0));
    take_core();
    c.shutdown();
}

fn cfg(crop: Option<pb::PixelRect>, w: u32, h: u32) -> pb::StreamConfig {
    pb::StreamConfig {
        epoch: 1,
        fps: 30,
        codec: pb::Codec::Hevc as i32,
        display_id: 1,
        width: 1280,
        height: 720,
        crop,
        display_width_px: w,
        display_height_px: h,
        ..Default::default()
    }
}

fn send_config(
    core: &Core,
    control: &ControlStream,
    out: &Output,
    config: pb::StreamConfig,
) -> EventCopy {
    let epoch = config.epoch;
    block_on_step(core, "send StreamConfig", send_message(control, config));
    let config_event = || {
        out.events
            .lock()
            .unwrap()
            .iter()
            .find(|e| e.kind == RS_EV_STREAM_CONFIG && e.epoch == epoch)
            .copied()
    };
    wait_until("receive StreamConfig callback", || config_event().is_some());
    config_event().unwrap()
}

#[track_caller]
fn assert_src(actual: [f32; 4], expected: [f32; 4]) {
    for (actual, expected) in actual.into_iter().zip(expected) {
        assert!(
            (actual - expected).abs() <= 1e-6,
            "expected {expected}, got {actual}"
        );
    }
}

#[test]
#[cfg(target_pointer_width = "64")]
fn layout() {
    assert_eq!(size_of::<RsEvent>(), 136);
    assert_eq!(offset_of!(RsEvent, src_x), 112);
    assert_eq!(offset_of!(RsEvent, src_y), 116);
    assert_eq!(offset_of!(RsEvent, src_w), 120);
    assert_eq!(offset_of!(RsEvent, src_h), 124);
    assert_eq!(offset_of!(RsEvent, display_w), 128);
    assert_eq!(offset_of!(RsEvent, display_h), 132);
}

#[test]
fn crop_src_values() {
    let crop = pb::PixelRect {
        x: 480,
        y: 270,
        w: 960,
        h: 540,
    };
    let full = [0.0, 0.0, 1.0, 1.0];
    let cases = [
        (cfg(None, 1920, 1080), full),
        (cfg(Some(crop), 1920, 1080), [0.25, 0.25, 0.5, 0.5]),
        (cfg(Some(crop), 0, 0), full),
        (cfg(Some(crop), 0, 1080), full),
        (cfg(Some(crop), 1920, 0), full),
        (
            cfg(
                Some(pb::PixelRect {
                    x: 1800,
                    y: 0,
                    w: 400,
                    h: 1080,
                }),
                1920,
                1080,
            ),
            [0.9375, 0.0, 0.0625, 1.0],
        ),
        (cfg(Some(pb::PixelRect { w: 0, ..crop }), 1920, 1080), full),
        (cfg(Some(pb::PixelRect { h: 0, ..crop }), 1920, 1080), full),
        (
            cfg(
                Some(pb::PixelRect {
                    y: 810,
                    h: 540,
                    ..crop
                }),
                1920,
                1080,
            ),
            [0.25, 0.75, 0.5, 0.25],
        ),
        (
            cfg(
                Some(pb::PixelRect {
                    x: u32::MAX,
                    y: u32::MAX,
                    w: u32::MAX,
                    h: u32::MAX,
                }),
                1920,
                1080,
            ),
            [1.0, 1.0, 0.0, 0.0],
        ),
    ];
    for (config, expected) in cases {
        assert_src(crop_src(&config), expected);
    }
}

#[test]
fn stream_config_event_has_src() {
    with_scripted_session(|c, _id, control, out| {
        let cropped = send_config(
            c,
            control,
            out,
            cfg(
                Some(pb::PixelRect {
                    x: 480,
                    y: 270,
                    w: 960,
                    h: 540,
                }),
                1920,
                1080,
            ),
        );
        assert_src(cropped.src(), [0.25, 0.25, 0.5, 0.5]);
        assert_eq!((cropped.display_w, cropped.display_h), (1920, 1080));
        let full = send_config(
            c,
            control,
            out,
            pb::StreamConfig {
                epoch: 2,
                ..cfg(None, 1920, 1080)
            },
        );
        assert_src(full.src(), [0.0, 0.0, 1.0, 1.0]);
        assert_eq!((full.display_w, full.display_h), (1920, 1080));
    });
}

fn report_and_settle(core: &Core, id: u64, control: &ControlStream, out: &Output) {
    send_config(core, control, out, cfg(None, 1920, 1080));
    block_on_step(core, "receive active and settled Viewport", async {
        rs_session_set_viewport(
            id,
            0.25,
            0.25,
            0.5,
            0.5,
            2532,
            1170,
            RS_VIEWPORT_ACTIVE | RS_VIEWPORT_CROP_OK,
        );
        let first = recv_command(control, "receive active Viewport").await;
        let pb::envelope::Msg::Viewport(first) = first else {
            panic!("expected Viewport, got {first:?}");
        };
        assert_eq!(
            first,
            pb::Viewport {
                seq: 1,
                display_id: 1,
                rect: Some(pb::PixelRect {
                    x: 480,
                    y: 256,
                    w: 960,
                    h: 560,
                }),
                settled: false,
                view_width_px: 2528,
                view_height_px: 1168,
                crop_ok: true,
            }
        );
        rs_session_set_viewport(id, 0.25, 0.25, 0.5, 0.5, 2532, 1170, RS_VIEWPORT_CROP_OK);
        let settled =
            tokio::time::timeout(DEADLINE, recv_command(control, "receive settled Viewport"))
                .await
                .expect("receive settled Viewport within 2 seconds");
        let pb::envelope::Msg::Viewport(settled) = settled else {
            panic!("expected settled Viewport, got {settled:?}");
        };
        assert_eq!(
            settled,
            pb::Viewport {
                seq: 2,
                settled: true,
                ..first
            }
        );
    });
}

#[test]
fn set_viewport_reaches_host() {
    with_scripted_session(report_and_settle);
}

#[test]
fn invalid_calls_are_ignored() {
    with_scripted_session(|c, id, control, out| {
        report_and_settle(c, id, control, out);
        assert_ne!(id, 99);
        let unconnected =
            c.state::<SessionTable>()
                .insert("unconnected", "Host", opts(), c.now_us());
        rs_session_set_viewport(99, 0.25, 0.25, 0.5, 0.5, 2532, 1170, RS_VIEWPORT_CROP_OK);
        rs_session_set_viewport(
            unconnected.id,
            0.25,
            0.25,
            0.5,
            0.5,
            2532,
            1170,
            RS_VIEWPORT_CROP_OK,
        );
        rs_session_set_viewport(
            id,
            f32::NAN,
            0.25,
            0.5,
            0.5,
            2532,
            1170,
            RS_VIEWPORT_CROP_OK,
        );
        rs_session_set_viewport(id, 0.25, 0.25, 0.0, 0.5, 2532, 1170, RS_VIEWPORT_CROP_OK);
        rs_session_set_viewport(id, 0.25, 0.25, 0.5, 0.5, 2532, 1170, 0xFFFF_FFFE);
        block_on_step(c, "invalid calls produce no Viewport for 1 second", async {
            assert!(
                tokio::time::timeout(
                    Duration::from_secs(1),
                    recv_command(control, "unexpected Viewport after invalid calls"),
                )
                .await
                .is_err(),
                "invalid or unchanged inputs emitted a new Viewport"
            );
        });
    });
}
