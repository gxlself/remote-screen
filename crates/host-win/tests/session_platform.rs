use std::sync::{Arc, Condvar, Mutex};
use std::thread::{self, ThreadId};
use std::time::Duration;

use host_core::platform::{
    AudioCaptureConfig, CaptureConfig, CaptureHandle, ClipboardItem, ConsoleState, CursorUpdate,
    DisplayInfo, EncodeOpts, Encoder, EncoderConfig, FrameSink, HostInfo, InputEvent, Platform,
    PlatformError, Result, SessionActionKind,
};
use host_win::pipe_proto::{input_to_b64, Ctl, PipeFrame, WireError};
use host_win::session_platform::{
    blocking, EncoderFactory, SessionOptions, SessionOs, SessionPlatform,
};
use host_win::session_server::SessionServer;
use rsp_core::hid::{InputMsg, Mods};
use rsp_core::pb;
use tokio::sync::mpsc;

struct Handle;
impl CaptureHandle for Handle {}

#[derive(Default)]
struct Calls {
    attaches: usize,
    attach_fail: bool,
    injections: Vec<(InputMsg, ThreadId, Option<String>)>,
    releases: Vec<ThreadId>,
    locks: Vec<ThreadId>,
    wakes: Vec<ThreadId>,
    clipboard_gets: usize,
    clipboard_sets: usize,
    clipboard: Option<ClipboardItem>,
    lock_sleep: Duration,
}

type EncoderCall = (u32, u32, u32, u32);

#[derive(Clone, Default)]
struct FakeOs {
    calls: Arc<Mutex<Calls>>,
    block_inject: Arc<(Mutex<bool>, Condvar)>,
    encoder_calls: Arc<Mutex<Vec<EncoderCall>>>,
}

impl FakeOs {
    fn display() -> DisplayInfo {
        DisplayInfo {
            id: 1,
            name: "fake".into(),
            width_px: 1920,
            height_px: 1080,
            scale: 1.0,
            x: 0,
            y: 0,
            refresh_hz: 60,
            primary: true,
            rotation: 0,
        }
    }

    fn set_attach_fail(&self, value: bool) {
        self.calls.lock().unwrap().attach_fail = value;
    }

    fn set_lock_sleep(&self, value: Duration) {
        self.calls.lock().unwrap().lock_sleep = value;
    }

    fn allow_inject(&self) {
        let (lock, wake) = &*self.block_inject;
        *lock.lock().unwrap() = false;
        wake.notify_all();
    }

    fn set_block_inject(&self, value: bool) {
        *self.block_inject.0.lock().unwrap() = value;
    }
}

impl SessionOs for FakeOs {
    fn host_info(&self) -> HostInfo {
        HostInfo {
            name: "fake".into(),
            os: pb::Platform::Windows,
            os_version: "test".into(),
            model: "fake".into(),
            arch: "x86_64".into(),
        }
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        vec![Self::display()]
    }

    fn watch_displays(&self, _tx: mpsc::Sender<Vec<DisplayInfo>>) {}

    fn capture(
        &self,
        _display: u32,
        _cfg: CaptureConfig,
        _sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        Ok(Box::new(Handle))
    }

    fn watch_cursor(&self, _tx: mpsc::Sender<CursorUpdate>) {}

    fn audio_capture_supported(&self) -> bool {
        false
    }

    fn capture_audio(
        &self,
        _cfg: AudioCaptureConfig,
        _sink: host_core::platform::AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        Err(PlatformError::Unsupported)
    }

    fn attach_input_desktop(&self) -> Result<String> {
        let mut calls = self.calls.lock().unwrap();
        calls.attaches += 1;
        if calls.attach_fail {
            Err(PlatformError::Platform("attach failed".into()))
        } else {
            Ok("Default".into())
        }
    }

    fn inject_input(&self, msg: &InputMsg, _display: &DisplayInfo) {
        let (lock, wake) = &*self.block_inject;
        let mut blocked = lock.lock().unwrap();
        while *blocked {
            blocked = wake.wait(blocked).unwrap();
        }
        let current = thread::current();
        self.calls.lock().unwrap().injections.push((
            msg.clone(),
            current.id(),
            current.name().map(str::to_owned),
        ));
    }

    fn release_all(&self) {
        self.calls
            .lock()
            .unwrap()
            .releases
            .push(thread::current().id());
    }

    fn lock_workstation(&self) -> Result<()> {
        let sleep = self.calls.lock().unwrap().lock_sleep;
        if !sleep.is_zero() {
            thread::sleep(sleep);
        }
        self.calls
            .lock()
            .unwrap()
            .locks
            .push(thread::current().id());
        Ok(())
    }

    fn wake_display(&self) -> Result<()> {
        self.calls
            .lock()
            .unwrap()
            .wakes
            .push(thread::current().id());
        Ok(())
    }

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        let mut calls = self.calls.lock().unwrap();
        calls.clipboard_gets += 1;
        calls.clipboard.clone()
    }

    fn clipboard_set(&self, item: &ClipboardItem) -> Result<()> {
        let mut calls = self.calls.lock().unwrap();
        calls.clipboard_sets += 1;
        calls.clipboard = Some(item.clone());
        Ok(())
    }

    fn watch_clipboard(&self, _tx: mpsc::Sender<()>) {}
    fn watch_local_activity(&self, _tx: mpsc::Sender<bool>) {}
    fn set_keep_awake(&self, _on: bool) {}
    fn console_state(&self) -> ConsoleState {
        ConsoleState::LoggedIn
    }
}

struct CountingEncoder;
impl Encoder for CountingEncoder {
    fn encode(
        &mut self,
        _frame: host_core::platform::CapturedFrame,
        _opts: EncodeOpts,
    ) -> Result<()> {
        Ok(())
    }
    fn set_rate(&mut self, _kbps: u32, _fps: u32) {}
    fn force_keyframe(&mut self) {}
    fn set_quality_boost(&mut self, _on: bool) {}
}

fn platform(os: Arc<FakeOs>) -> SessionPlatform<FakeOs> {
    SessionPlatform::new(os, None).unwrap()
}

fn msg() -> InputMsg {
    InputMsg::Key {
        usage: 4,
        down: true,
        mods: Mods::from_bits(0),
    }
}

#[test]
fn input_thread_serializes_and_follows_desktop() {
    let os = Arc::new(FakeOs::default());
    let platform = platform(Arc::clone(&os));
    let display = FakeOs::display();
    platform.inject_input(&msg(), &display);
    platform.inject(InputEvent::MouseMove { x: 0.1, y: 0.2 }, &display);
    platform.inject_input(&msg(), &display);
    platform.release_all();
    let calls = os.calls.lock().unwrap();
    assert_eq!(calls.injections.len(), 3);
    assert_eq!(calls.releases.len(), 1);
    let thread_id = calls.injections[0].1;
    assert!(calls.injections.iter().all(|call| call.1 == thread_id));
    assert_eq!(calls.injections[0].2.as_deref(), Some("rs-session-input"));
    assert_eq!(calls.releases[0], thread_id);
    assert_eq!(platform.stats().desktop_attaches, 1);
    drop(calls);
    platform.set_desktop(Some("Winlogon".into()));
    platform.inject_input(&msg(), &display);
    platform.release_all();
    assert_eq!(platform.stats().desktop_attaches, 2);
    platform.set_desktop(Some("Winlogon".into()));
    platform.inject_input(&msg(), &display);
    platform.release_all();
    assert_eq!(platform.stats().desktop_attaches, 2);
}

#[test]
fn actions_clipboard_encoder_and_drop() {
    let os = Arc::new(FakeOs::default());
    let platform = platform(Arc::clone(&os));
    assert!(platform
        .session_action(SessionActionKind::LockScreen)
        .is_ok());
    assert!(platform
        .session_action(SessionActionKind::WakeDisplay)
        .is_ok());
    assert!(platform
        .session_action(SessionActionKind::LockOnEnd)
        .is_ok());
    assert!(matches!(
        platform.session_action(SessionActionKind::CtrlAltDel),
        Err(PlatformError::Unsupported)
    ));
    assert!(matches!(
        platform.session_action(SessionActionKind::Unspecified),
        Err(PlatformError::Unsupported)
    ));
    assert_eq!(os.calls.lock().unwrap().locks.len(), 1);
    assert_eq!(os.calls.lock().unwrap().wakes.len(), 1);

    let item = ClipboardItem {
        kind: pb::clipboard_header::Kind::Text,
        data: b"test".to_vec(),
        sha256: [0; 32],
    };
    assert!(platform.clipboard_allowed());
    assert!(platform.clipboard_set(item.clone()).is_ok());
    assert_eq!(
        platform.clipboard_get().map(|value| value.data),
        Some(item.data.clone())
    );
    platform.set_desktop(Some("Winlogon".into()));
    assert!(!platform.clipboard_allowed());
    assert!(platform.clipboard_get().is_none());
    let error = platform.clipboard_set(item).unwrap_err();
    assert!(error.to_string().contains("secure desktop"));
    assert_eq!(os.calls.lock().unwrap().clipboard_gets, 1);
    assert_eq!(os.calls.lock().unwrap().clipboard_sets, 1);

    let encoder_calls = Arc::clone(&os.encoder_calls);
    let calls = Arc::clone(&encoder_calls);
    let factory: EncoderFactory = Arc::new(move |cfg| {
        calls
            .lock()
            .unwrap()
            .push((cfg.width, cfg.height, cfg.fps, cfg.bitrate_kbps));
        Ok(Box::new(CountingEncoder))
    });
    let platform = SessionPlatform::new(os, Some(factory)).unwrap();
    let (sink, _) = mpsc::channel(1);
    assert!(platform
        .encoder(EncoderConfig {
            codec: pb::Codec::H264,
            width: 1280,
            height: 720,
            fps: 30,
            bitrate_kbps: 4000,
            sink,
        })
        .is_ok());
    assert_eq!(
        encoder_calls.lock().unwrap().as_slice(),
        &[(1280, 720, 30, 4000)]
    );
}

#[test]
fn timeout_and_attach_retry_are_recorded() {
    let os = Arc::new(FakeOs::default());
    os.set_lock_sleep(Duration::from_millis(100));
    os.set_attach_fail(true);
    let platform = SessionPlatform::with_options(
        Arc::clone(&os),
        None,
        SessionOptions {
            input_queue: 4,
            reply_timeout: Duration::from_millis(20),
            attach_retry: Duration::from_millis(50),
        },
    )
    .unwrap();
    assert!(matches!(
        platform.session_action(SessionActionKind::LockScreen),
        Err(PlatformError::Platform(_))
    ));
    assert_eq!(platform.stats().input_timeouts, 1);
    let display = FakeOs::display();
    platform.inject_input(&msg(), &display);
    platform.release_all();
    assert_eq!(platform.stats().desktop_attach_failures, 1);
    thread::sleep(Duration::from_millis(60));
    platform.release_all();
    assert_eq!(platform.stats().desktop_attach_failures, 2);
}

#[test]
fn full_input_queue_drops_without_blocking() {
    let os = Arc::new(FakeOs::default());
    os.set_block_inject(true);
    let platform = SessionPlatform::with_options(
        Arc::clone(&os),
        None,
        SessionOptions {
            input_queue: 4,
            ..SessionOptions::default()
        },
    )
    .unwrap();
    let display = FakeOs::display();
    let started = std::time::Instant::now();
    for _ in 0..20 {
        platform.inject_input(&msg(), &display);
    }
    assert!(started.elapsed() < Duration::from_millis(100));
    assert!(platform.stats().dropped_inputs >= 15);
    os.allow_inject();
    platform.release_all();
}

#[test]
fn null_keystore_is_unsupported() {
    let os = Arc::new(FakeOs::default());
    let platform = platform(os);
    assert!(matches!(
        platform.keystore().load("host-key"),
        Err(PlatformError::Unsupported)
    ));
    assert!(matches!(
        platform.keystore().store("host-key", b"value"),
        Err(PlatformError::Unsupported)
    ));
    assert!(matches!(
        platform.keystore().delete("host-key"),
        Err(PlatformError::Unsupported)
    ));
}

#[test]
fn blocking_is_safe_without_and_with_tokio_runtimes() {
    assert_eq!(blocking(|| 42), 42);
    let current = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(current.block_on(async { blocking(|| 42) }), 42);
    let multi = tokio::runtime::Builder::new_multi_thread()
        .worker_threads(2)
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(multi.block_on(async { blocking(|| 42) }), 42);
}

#[tokio::test]
async fn session_server_uses_session_platform() {
    let os = Arc::new(FakeOs::default());
    let session_platform = Arc::new(platform(Arc::clone(&os)));
    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut server = SessionServer::new(Arc::clone(&session_platform), out_tx);
    server.start();
    assert!(matches!(
        out_rx.recv().await,
        Some(PipeFrame::Ctl(Ctl::Displays { .. }))
    ));

    server.handle(PipeFrame::Ctl(Ctl::Action {
        req: 1,
        kind: SessionActionKind::LockScreen as i32,
    }));
    assert_eq!(
        out_rx.recv().await,
        Some(PipeFrame::Ctl(Ctl::Done {
            req: 1,
            error: None
        }))
    );
    let input = InputMsg::MouseMove { x: 0.1, y: 0.2 };
    server.handle(PipeFrame::Ctl(Ctl::Inject {
        display_id: 1,
        input: input_to_b64(&input).unwrap(),
    }));
    server.handle(PipeFrame::Ctl(Ctl::ReleaseAll));
    assert_eq!(os.calls.lock().unwrap().injections[0].0, input);

    let (out_tx, mut out_rx) = mpsc::unbounded_channel();
    let mut server = SessionServer::new(Arc::new(platform(Arc::clone(&os))), out_tx);
    server.start();
    let _ = out_rx.recv().await;
    server.handle(PipeFrame::Ctl(Ctl::EncoderOpen {
        req: 2,
        enc: 1,
        codec: pb::Codec::H264 as i32,
        width: 1280,
        height: 720,
        fps: 30,
        bitrate_kbps: 4000,
    }));
    assert_eq!(
        out_rx.recv().await,
        Some(PipeFrame::Ctl(Ctl::Done {
            req: 2,
            error: Some(WireError::Unsupported),
        }))
    );
}

#[test]
fn drop_releases_and_does_not_retain_os() {
    let os = Arc::new(FakeOs::default());
    let platform = platform(Arc::clone(&os));
    platform.inject_input(&msg(), &FakeOs::display());
    drop(platform);
    assert_eq!(os.calls.lock().unwrap().releases.len(), 1);
    assert_eq!(Arc::strong_count(&os), 1);
}
