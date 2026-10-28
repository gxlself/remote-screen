#![cfg(unix)]

use rs_tailnet::{
    abi::{Api, ABI_VERSION},
    BackendState, Code, Tailnet, TailnetConfig, TailnetEvent,
};
use std::{
    collections::VecDeque,
    ffi::{c_char, CStr},
    io::ErrorKind,
    os::{
        fd::{IntoRawFd, OwnedFd},
        unix::net::{UnixDatagram, UnixStream},
    },
    sync::{mpsc, Arc, Mutex, MutexGuard, OnceLock},
    thread,
    time::{Duration, Instant},
};

#[derive(Default)]
struct Fake {
    config: String,
    start_rc: i32,
    full_error: bool,
    stopped: bool,
    stop_calls: usize,
    next_calls: usize,
    closed_calls: usize,
    events: VecDeque<Vec<u8>>,
    finish_events: bool,
    status: Vec<u8>,
    status_length: Option<i64>,
    status_caps: Vec<usize>,
    growing_status: bool,
    fd: Option<OwnedFd>,
    dial_rc: i32,
    dial_args: Option<(String, u16, i32)>,
    login_rc: i32,
    logout_rc: i32,
    rebind_rc: i32,
}

fn fake() -> MutexGuard<'static, Fake> {
    static FAKE: OnceLock<Mutex<Fake>> = OnceLock::new();
    FAKE.get_or_init(|| Mutex::new(Fake::default()))
        .lock()
        .unwrap()
}

fn setup() -> (MutexGuard<'static, ()>, Arc<Tailnet>) {
    static SERIAL: Mutex<()> = Mutex::new(());
    let guard = SERIAL.lock().unwrap_or_else(|e| e.into_inner());
    *fake() = Fake::default();
    (guard, Tailnet::new(api()))
}

fn api() -> &'static Api {
    static API: OnceLock<&'static Api> = OnceLock::new();
    API.get_or_init(|| {
        Box::leak(Box::new(Api {
            abi_version: version,
            start,
            stop,
            login,
            logout,
            rebind,
            status,
            next_event,
            dial_udp,
            dial_tcp,
        }))
    })
}

unsafe fn write_buf(buf: *mut c_char, cap: usize, bytes: &[u8]) {
    // SAFETY: fake ABI callers provide cap writable bytes, and the copy is bounded.
    unsafe { std::ptr::copy_nonoverlapping(bytes.as_ptr(), buf.cast(), bytes.len().min(cap)) };
}

unsafe extern "C" fn version() -> i32 {
    ABI_VERSION
}
unsafe extern "C" fn start(config: *const c_char, err: *mut c_char, cap: usize) -> i32 {
    let mut state = fake();
    // SAFETY: Tailnet::start passes a live NUL-terminated CString for this call.
    state.config = unsafe { CStr::from_ptr(config) }
        .to_str()
        .unwrap()
        .to_owned();
    state.stopped = false;
    let bytes = if state.full_error {
        vec![b'x'; cap]
    } else {
        b"bad config\0".to_vec()
    };
    // SAFETY: err/cap is the writable ABI error buffer.
    unsafe { write_buf(err, cap, &bytes) };
    state.start_rc
}
unsafe extern "C" fn stop() -> i32 {
    let mut state = fake();
    state.stopped = true;
    state.stop_calls += 1;
    0
}
unsafe extern "C" fn login(err: *mut c_char, cap: usize) -> i32 {
    // SAFETY: err/cap is the writable ABI error buffer.
    unsafe { write_buf(err, cap, b"login failure\0") };
    fake().login_rc
}
unsafe extern "C" fn logout(err: *mut c_char, cap: usize) -> i32 {
    // SAFETY: err/cap is the writable ABI error buffer.
    unsafe { write_buf(err, cap, b"logout failure\0") };
    fake().logout_rc
}
unsafe extern "C" fn rebind() -> i32 {
    fake().rebind_rc
}
unsafe extern "C" fn status(buf: *mut c_char, cap: usize) -> i64 {
    let mut state = fake();
    state.status_caps.push(cap);
    if state.growing_status {
        return cap as i64 + 32;
    }
    if let Some(length) = state.status_length {
        return length;
    }
    if state.status.len() <= cap {
        // SAFETY: this branch checks that the entire status fits in the ABI buffer.
        unsafe { write_buf(buf, cap, &state.status) };
    }
    state.status.len() as i64
}
unsafe extern "C" fn next_event(buf: *mut c_char, cap: usize, timeout: i32) -> i64 {
    {
        let mut state = fake();
        state.next_calls += 1;
        if let Some(event) = state.events.front() {
            let length = event.len();
            if length <= cap {
                // SAFETY: buf/cap is valid and the event length has been checked.
                unsafe { write_buf(buf, cap, event) };
                state.events.pop_front();
            }
            return length as i64;
        }
        if state.stopped || state.finish_events {
            state.closed_calls += 1;
            return -8;
        }
    }
    thread::sleep(Duration::from_millis(timeout.clamp(0, 20) as u64));
    0
}
unsafe extern "C" fn dial_udp(
    host: *const c_char,
    port: u16,
    timeout: i32,
    err: *mut c_char,
    cap: usize,
) -> i32 {
    // SAFETY: forwarding the identical ABI arguments to the shared fake dial.
    unsafe { dial_tcp(host, port, timeout, err, cap) }
}
unsafe extern "C" fn dial_tcp(
    host: *const c_char,
    port: u16,
    timeout: i32,
    err: *mut c_char,
    cap: usize,
) -> i32 {
    let mut state = fake();
    // SAFETY: Tailnet::dial passes a valid NUL-terminated host CString.
    state.dial_args = Some((
        unsafe { CStr::from_ptr(host) }.to_str().unwrap().to_owned(),
        port,
        timeout,
    ));
    // SAFETY: err/cap is the writable ABI error buffer.
    unsafe { write_buf(err, cap, b"fake dial failure\0") };
    if let Some(fd) = state.fd.take() {
        fd.into_raw_fd()
    } else {
        state.dial_rc
    }
}

fn start_idle(tailnet: &Tailnet) {
    tailnet
        .start(&TailnetConfig::default(), Arc::new(|_| {}))
        .unwrap();
}

#[test]
fn status_grows_buffer() {
    let (_guard, tailnet) = setup();
    fake().status = serde_json::to_vec(&serde_json::json!({"state":"Running", "peers":
        (0..200).map(|i| serde_json::json!({"hostname": format!("peer-{i}-{}", "x".repeat(100))})).collect::<Vec<_>>()
    })).unwrap();
    assert!(fake().status.len() > 20 * 1024);
    assert_eq!(tailnet.status().unwrap().peers.len(), 200);
    assert_eq!(fake().status_caps[0], 16384);
    assert!(fake().status_caps[1] > 20 * 1024);
}

#[test]
fn status_too_large() {
    let (_guard, tailnet) = setup();
    fake().status_length = Some(9 * 1024 * 1024);
    assert_eq!(tailnet.status().unwrap_err().code, Code::Io);
    assert_eq!(fake().status_caps, [16384]);
}

#[test]
fn status_invalid_negative_and_repeated_growth() {
    let (_guard, tailnet) = setup();
    fake().status = b"{bad".to_vec();
    let error = tailnet.status().unwrap_err();
    assert_eq!(error.code, Code::Io);
    assert!(error.message.contains("line 1"));
    fake().status_length = Some(-3);
    assert_eq!(tailnet.status().unwrap_err().code, Code::NotRunning);
    fake().growing_status = true;
    fake().status_caps.clear();
    assert_eq!(tailnet.status().unwrap_err().code, Code::Io);
    assert_eq!(fake().status_caps.len(), 3);
}

#[test]
fn events_in_order_and_growth() {
    let (_guard, tailnet) = setup();
    let expected = vec![
        TailnetEvent::State {
            state: BackendState::NeedsLogin,
            auth_url: None,
        },
        TailnetEvent::AuthUrl {
            url: "http://hs.example.com/auth/test".into(),
        },
        TailnetEvent::Error {
            message: "x".repeat(10 * 1024),
        },
        TailnetEvent::Netmap,
        TailnetEvent::Stopped,
    ];
    fake().events = expected
        .iter()
        .map(|event| serde_json::to_vec(event).unwrap())
        .collect();
    fake().events.insert(3, b"bad JSON".to_vec());
    fake().finish_events = true;
    let (tx, rx) = mpsc::channel();
    tailnet
        .start(
            &TailnetConfig::default(),
            Arc::new(move |event| {
                assert_eq!(thread::current().name(), Some("rs-tailnet-events"));
                tx.send(event).unwrap();
            }),
        )
        .unwrap();
    let received: Vec<_> = (0..5)
        .map(|_| rx.recv_timeout(Duration::from_secs(2)).unwrap())
        .collect();
    assert_eq!(received, expected);
    assert!(rx.recv_timeout(Duration::from_secs(2)).is_err());
    assert_eq!(fake().closed_calls, 1);
    tailnet.stop().unwrap();
}

#[test]
fn stop_joins_event_thread() {
    let (_guard, tailnet) = setup();
    start_idle(&tailnet);
    let began = Instant::now();
    tailnet.stop().unwrap();
    assert!(began.elapsed() < Duration::from_secs(2));
    let calls = fake().next_calls;
    thread::sleep(Duration::from_millis(30));
    assert_eq!(fake().next_calls, calls);
    start_idle(&tailnet);
    tailnet.stop().unwrap();
    tailnet.stop().unwrap();
    assert_eq!(fake().stop_calls, 3);
}

#[test]
fn start_errors_and_config_json() {
    let (_guard, tailnet) = setup();
    fake().start_rc = -1;
    let cfg = TailnetConfig {
        auth_key: Some("tskey-fake".into()),
        control_url: "https://hs.example.com".into(),
        disable_netns: true,
        ..Default::default()
    };
    let error = tailnet.start(&cfg, Arc::new(|_| {})).unwrap_err();
    assert_eq!(error.code, Code::Invalid);
    assert_eq!(error.message, "bad config");
    assert_eq!(fake().next_calls, 0);
    let config: serde_json::Value = serde_json::from_str(&fake().config).unwrap();
    assert_eq!(config["authKey"], "tskey-fake");
    assert_eq!(config["controlUrl"], "https://hs.example.com");
    assert_eq!(config["disableNetns"], true);
    assert_eq!(config["verbose"], 0);
    fake().full_error = true;
    assert_eq!(
        tailnet
            .start(&cfg, Arc::new(|_| {}))
            .unwrap_err()
            .message
            .len(),
        512
    );
    fake().start_rc = 0;
    start_idle(&tailnet);
    assert_eq!(
        tailnet.start(&cfg, Arc::new(|_| {})).unwrap_err().code,
        Code::State
    );
    tailnet.stop().unwrap();
}

#[test]
fn dial_hands_over_fd() {
    let (_guard, tailnet) = setup();
    let (left, right) = UnixDatagram::pair().unwrap();
    fake().fd = Some(left.into());
    let fd = tailnet
        .dial_udp(
            "[fd7a:115c:a1e0::1]:7400".parse().unwrap(),
            Duration::from_secs(5),
        )
        .unwrap();
    assert_eq!(
        fake().dial_args.as_ref().unwrap(),
        &("fd7a:115c:a1e0::1".into(), 7400, 5000)
    );
    let socket = UnixDatagram::from(fd);
    socket.send(b"ping").unwrap();
    right
        .set_read_timeout(Some(Duration::from_secs(1)))
        .unwrap();
    let mut buf = [0; 10];
    let length = right.recv(&mut buf).unwrap();
    assert_eq!(&buf[..length], b"ping");
    drop(socket);
    assert!(right.send(b"closed").is_err());
}

#[test]
fn dial_error_kinds() {
    let (_guard, tailnet) = setup();
    for (raw, kind) in [
        (-1, ErrorKind::InvalidInput),
        (-2, ErrorKind::Other),
        (-3, ErrorKind::NotConnected),
        (-4, ErrorKind::TimedOut),
        (-5, ErrorKind::ConnectionRefused),
        (-6, ErrorKind::HostUnreachable),
        (-7, ErrorKind::Other),
        (-8, ErrorKind::BrokenPipe),
        (-99, ErrorKind::Other),
    ] {
        fake().dial_rc = raw;
        let error = tailnet
            .dial_tcp("rs-peer", 7400, Duration::from_secs(10))
            .unwrap_err();
        assert_eq!(error.kind(), kind);
        assert!(error.to_string().contains("fake dial failure"));
    }
    let error = tailnet
        .dial_tcp("bad\0host", 7400, Duration::MAX)
        .unwrap_err();
    assert_eq!(error.kind(), ErrorKind::InvalidInput);
    let _ = tailnet.dial_tcp("rs-peer", 7400, Duration::MAX);
    assert_eq!(fake().dial_args.as_ref().unwrap().2, i32::MAX);
}

#[test]
fn fd_source_via_net_dialer() {
    let (_guard, tailnet) = setup();
    tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap()
        .block_on(async {
            let peer = "100.64.0.1:7400".parse().unwrap();
            let net = tailnet.net_dialer();
            let (left, _right) = UnixDatagram::pair().unwrap();
            fake().fd = Some(left.into());
            net.dial_udp(peer).await.unwrap();
            assert_eq!(fake().dial_args.as_ref().unwrap().2, 5000);
            let (left, _right) = UnixStream::pair().unwrap();
            fake().fd = Some(left.into());
            assert_eq!(
                net.dial_udp(peer).await.unwrap_err().kind(),
                ErrorKind::InvalidInput
            );
            let (left, _right) = UnixStream::pair().unwrap();
            fake().fd = Some(left.into());
            net.dial_tcp_host("rs-peer", 7402).await.unwrap();
            assert_eq!(
                fake().dial_args.as_ref().unwrap(),
                &("rs-peer".into(), 7402, 10_000)
            );
            assert!(format!("{:?}", tailnet.fd_source()).contains("udp_timeout"));
        });
}

#[test]
fn callback_panic_does_not_end_events() {
    let (_guard, tailnet) = setup();
    fake().events = [
        br#"{"type":"netmap"}"#.to_vec(),
        br#"{"type":"stopped"}"#.to_vec(),
    ]
    .into();
    fake().finish_events = true;
    let (tx, rx) = mpsc::channel();
    tailnet
        .start(
            &TailnetConfig::default(),
            Arc::new(move |event| {
                if event == TailnetEvent::Netmap {
                    panic!("intentional callback failure");
                }
                tx.send(event).unwrap();
            }),
        )
        .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        TailnetEvent::Stopped
    );
    tailnet.stop().unwrap();
}

#[test]
fn lifecycle_errors_and_nul_config() {
    let (_guard, tailnet) = setup();
    fake().login_rc = -3;
    fake().logout_rc = -7;
    fake().rebind_rc = -2;
    assert_eq!(tailnet.login().unwrap_err().message, "login failure");
    assert_eq!(tailnet.logout().unwrap_err().code, Code::Io);
    assert_eq!(tailnet.rebind().unwrap_err().code, Code::State);
    let cfg = TailnetConfig {
        hostname: "bad\0host".into(),
        ..Default::default()
    };
    assert_eq!(
        tailnet.start(&cfg, Arc::new(|_| {})).unwrap_err().code,
        Code::Invalid
    );
    assert!(fake().config.is_empty());
}

#[test]
fn global_is_unique_and_rejects_other_tables() {
    let (_guard, _) = setup();
    assert!(Arc::ptr_eq(
        &Tailnet::global(api()),
        &Tailnet::global(api())
    ));
    let other = Box::leak(Box::new(Api {
        abi_version: version,
        start,
        stop,
        login,
        logout,
        rebind,
        status,
        next_event,
        dial_udp,
        dial_tcp,
    }));
    assert!(std::panic::catch_unwind(|| Tailnet::global(other)).is_err());
}

#[test]
fn concurrent_stop_waits_for_callback() {
    let (_guard, tailnet) = setup();
    fake().events.push_back(br#"{"type":"netmap"}"#.to_vec());
    let (entered_tx, entered_rx) = mpsc::channel();
    let (release_tx, release_rx) = mpsc::channel();
    let release_rx = Mutex::new(release_rx);
    let weak = Arc::downgrade(&tailnet);
    tailnet
        .start(
            &TailnetConfig::default(),
            Arc::new(move |_| {
                entered_tx.send(()).unwrap();
                release_rx
                    .lock()
                    .unwrap()
                    .recv_timeout(Duration::from_secs(2))
                    .unwrap();
                // The lifecycle mutex must be available while external stop is joining.
                assert_eq!(
                    weak.upgrade()
                        .unwrap()
                        .start(&TailnetConfig::default(), Arc::new(|_| {}))
                        .unwrap_err()
                        .code,
                    Code::State
                );
            }),
        )
        .unwrap();
    entered_rx.recv_timeout(Duration::from_secs(2)).unwrap();
    let node = tailnet.clone();
    let first = thread::spawn(move || node.stop());
    let deadline = Instant::now() + Duration::from_secs(2);
    while fake().stop_calls == 0 {
        assert!(Instant::now() < deadline);
        thread::sleep(Duration::from_millis(1));
    }
    let node = tailnet.clone();
    let second = thread::spawn(move || node.stop());
    assert!(!first.is_finished());
    release_tx.send(()).unwrap();
    first.join().unwrap().unwrap();
    second.join().unwrap().unwrap();
    assert_eq!(fake().stop_calls, 2);
}

#[test]
fn callback_cannot_join_itself() {
    let (_guard, tailnet) = setup();
    fake().events.push_back(br#"{"type":"netmap"}"#.to_vec());
    let weak = Arc::downgrade(&tailnet);
    let (tx, rx) = mpsc::channel();
    tailnet
        .start(
            &TailnetConfig::default(),
            Arc::new(move |_| {
                tx.send(weak.upgrade().unwrap().stop().unwrap_err().code)
                    .unwrap();
            }),
        )
        .unwrap();
    assert_eq!(
        rx.recv_timeout(Duration::from_secs(2)).unwrap(),
        Code::State
    );
    tailnet.stop().unwrap();
}

#[test]
fn public_verbose_bool_maps_to_go_level() {
    let (_guard, tailnet) = setup();
    fake().start_rc = -1;
    for (verbose, level) in [(false, 0), (true, 1)] {
        let cfg = TailnetConfig {
            verbose,
            ..Default::default()
        };
        assert_eq!(serde_json::to_value(&cfg).unwrap()["verbose"], verbose);
        assert!(tailnet.start(&cfg, Arc::new(|_| {})).is_err());
        let wire: serde_json::Value = serde_json::from_str(&fake().config).unwrap();
        assert_eq!(wire["verbose"], level);
    }
}
