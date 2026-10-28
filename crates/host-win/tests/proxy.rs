use std::collections::HashMap;
use std::sync::{mpsc as sync_mpsc, Arc, Mutex};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use host_core::platform::{
    AudioCaptureConfig, CaptureConfig, ClipboardItem, ConsoleState, CursorUpdate, DisplayInfo,
    EncoderConfig, HostInfo, InputEvent, InputMsg, KeyStore, Nic, PermissionKind, Permissions,
    Platform, PlatformError, Result, SessionActionKind,
};
use host_core::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};
use host_win::pipe_proto::{
    cursor_to_ctl, input_from_b64, BlobKind, BlobPacket, Ctl, DisplayWire, PipeFrame, WatchKind,
    WireError,
};
use host_win::proxy::{PipePlatform, ProxyTimeouts, ServiceLocal};
use rsp_core::hid::{Button, Mods, ScrollPhase, ScrollUnit, MAX_TEXT_BYTES};
use rsp_core::input::MouseButton;
use rsp_core::pb::{self, clipboard_header::Kind};
use sha2::{Digest, Sha256};
use tokio::sync::mpsc;

#[derive(Default)]
struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for MemoryKeyStore {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self.0.lock().unwrap().get(name).cloned())
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        self.0.lock().unwrap().insert(name.into(), value.into());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.0.lock().unwrap().remove(name);
        Ok(())
    }
}

struct FakeAuthenticator;

impl SystemAuthenticator for FakeAuthenticator {
    fn lookup(&self, _account: &str) -> std::result::Result<Option<SystemAccount>, SysAuthError> {
        Ok(None)
    }

    fn verify(&self, _short_name: &str, _password: &[u8]) -> std::result::Result<(), SysAuthError> {
        Err(SysAuthError::WrongPassword)
    }
}

#[derive(Default)]
struct LocalCalls {
    ctrl_alt_del: usize,
    wol: Vec<([u8; 6], Nic)>,
}

#[derive(Clone, Default)]
struct FakeLocal {
    calls: Arc<Mutex<LocalCalls>>,
    keys: Arc<MemoryKeyStore>,
    authenticator: Option<Arc<dyn SystemAuthenticator>>,
}

impl ServiceLocal for FakeLocal {
    fn info(&self) -> HostInfo {
        HostInfo {
            name: "test-host".into(),
            os: pb::Platform::Windows,
            os_version: "11".into(),
            model: "fake".into(),
            arch: "x86_64".into(),
        }
    }

    fn console_state(&self) -> ConsoleState {
        ConsoleState::Locked
    }

    fn nics(&self) -> Vec<Nic> {
        vec![Nic {
            name: "ethernet".into(),
            addresses: vec!["192.168.31.7".parse().unwrap()],
            networks: vec!["192.168.31.0/24".into()],
            mac: Some([2, 0, 0, 0, 0, 7]),
            tailnet: false,
        }]
    }

    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        self.calls.lock().unwrap().wol.push((mac, via.clone()));
        Ok(())
    }

    fn keystore(&self) -> &dyn KeyStore {
        self.keys.as_ref()
    }

    fn ctrl_alt_del(&self) -> Result<()> {
        self.calls.lock().unwrap().ctrl_alt_del += 1;
        Ok(())
    }

    fn system_authenticator(&self) -> Option<Arc<dyn SystemAuthenticator>> {
        self.authenticator.clone()
    }
}

type TestPlatform = PipePlatform<FakeLocal>;

fn platform() -> Arc<TestPlatform> {
    Arc::new(PipePlatform::with_timeouts(
        FakeLocal::default(),
        ProxyTimeouts {
            request: Duration::from_secs(5),
            open: Duration::from_secs(5),
        },
    ))
}

fn short_platform() -> Arc<TestPlatform> {
    Arc::new(PipePlatform::with_timeouts(
        FakeLocal::default(),
        ProxyTimeouts {
            request: Duration::from_millis(50),
            open: Duration::from_millis(50),
        },
    ))
}

struct FakeSession {
    platform: Arc<TestPlatform>,
    generation: u64,
    rx: mpsc::UnboundedReceiver<PipeFrame>,
}

impl FakeSession {
    fn attach(platform: &Arc<TestPlatform>) -> Self {
        let (tx, rx) = mpsc::unbounded_channel();
        let generation = platform.attach(tx);
        Self {
            platform: platform.clone(),
            generation,
            rx,
        }
    }

    fn run(mut self, script: impl FnOnce(&mut Self) + Send + 'static) -> JoinHandle<()> {
        thread::spawn(move || script(&mut self))
    }

    fn recv(&mut self) -> PipeFrame {
        self.rx.blocking_recv().unwrap()
    }

    fn reply(&self, frame: PipeFrame) {
        self.platform.on_frame(self.generation, frame);
    }

    fn ctl(&self, ctl: Ctl) {
        self.reply(PipeFrame::Ctl(ctl));
    }

    fn assert_empty(&mut self) {
        assert!(matches!(
            self.rx.try_recv(),
            Err(mpsc::error::TryRecvError::Empty)
        ));
    }
}

fn display(id: u32) -> DisplayInfo {
    DisplayInfo {
        id,
        name: format!("display-{id}"),
        width_px: 1920,
        height_px: 1080,
        scale: 1.0,
        x: -1920,
        y: 0,
        refresh_hz: 60,
        primary: true,
        rotation: 0,
    }
}

fn item(kind: Kind, data: &[u8]) -> ClipboardItem {
    ClipboardItem {
        kind,
        data: data.to_vec(),
        sha256: Sha256::digest(data).into(),
    }
}

fn assert_platform_error(result: Result<()>, expected: &str) {
    assert!(matches!(result, Err(PlatformError::Platform(message)) if message.contains(expected)));
}

fn action_req(frame: PipeFrame, expected: SessionActionKind) -> u32 {
    match frame {
        PipeFrame::Ctl(Ctl::Action { req, kind }) => {
            assert_eq!(kind, expected as i32);
            req
        }
        _ => panic!("expected Action"),
    }
}

fn clipboard_req(frame: PipeFrame) -> u32 {
    match frame {
        PipeFrame::Ctl(Ctl::ClipboardGet { req }) => req,
        _ => panic!("expected ClipboardGet"),
    }
}

fn cursor_position(seq: u32) -> CursorUpdate {
    CursorUpdate::Position {
        display_id: 7,
        seq,
        x: 0.5,
        y: 0.25,
        visible: true,
    }
}

#[test]
fn defaults_and_disconnected_operations() {
    let timeouts = ProxyTimeouts::default();
    assert_eq!(timeouts.request, Duration::from_secs(2));
    assert_eq!(timeouts.open, Duration::from_secs(5));
    let platform = platform();
    assert!(!platform.is_attached());
    assert!(platform.displays().is_empty());
    assert_eq!(platform.desktop(), None);
    platform.inject_input(&InputMsg::Text("discarded".into()), &display(7));
    platform.release_all();
    assert!(platform.clipboard_get().is_none());
    assert_platform_error(
        platform.clipboard_set(item(Kind::Text, b"hi")),
        "not connected",
    );
    for action in [
        SessionActionKind::LockScreen,
        SessionActionKind::WakeDisplay,
    ] {
        assert_platform_error(platform.session_action(action), "not connected");
    }
}

#[test]
fn input_roundtrips_and_release_all() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let key = InputMsg::Key {
        usage: 4,
        down: true,
        mods: Mods::NONE,
    };
    platform.inject_input(&key, &display(7));
    let assert_input = |frame: PipeFrame, expected: InputMsg| match frame {
        PipeFrame::Ctl(Ctl::Inject { display_id, input }) => {
            assert_eq!(display_id, 7);
            assert_eq!(input_from_b64(&input).unwrap(), expected);
        }
        _ => panic!("expected Inject"),
    };
    assert_input(session.recv(), key);
    platform.inject(InputEvent::MouseMove { x: 0.5, y: 0.5 }, &display(7));
    assert_input(session.recv(), InputMsg::MouseMove { x: 0.5, y: 0.5 });
    for (old, new) in [
        (MouseButton::Left, Button::Left),
        (MouseButton::Right, Button::Right),
        (MouseButton::Middle, Button::Middle),
    ] {
        platform.inject(
            InputEvent::MouseButton {
                button: old,
                down: false,
                x: 0.2,
                y: 0.3,
                clicks: 2,
            },
            &display(7),
        );
        assert_input(
            session.recv(),
            InputMsg::MouseButton {
                button: new,
                down: false,
                x: 0.2,
                y: 0.3,
                clicks: 2,
            },
        );
    }
    platform.inject(InputEvent::Scroll { dx: 2.0, dy: -3.0 }, &display(7));
    assert_input(
        session.recv(),
        InputMsg::Scroll {
            dx: 2.0,
            dy: -3.0,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        },
    );
    platform.inject_input(&InputMsg::Text("x".repeat(MAX_TEXT_BYTES + 1)), &display(7));
    session.assert_empty();
    platform.release_all();
    assert_eq!(session.recv(), PipeFrame::Ctl(Ctl::ReleaseAll));
}

#[test]
fn actions_map_done_and_wire_errors() {
    for (kind, error) in [
        (SessionActionKind::LockScreen, None),
        (SessionActionKind::WakeDisplay, None),
        (
            SessionActionKind::LockScreen,
            Some(WireError::Platform {
                message: "x".into(),
            }),
        ),
        (
            SessionActionKind::LockScreen,
            Some(WireError::PermissionDenied {
                message: "denied".into(),
            }),
        ),
        (SessionActionKind::LockScreen, Some(WireError::Unsupported)),
    ] {
        let platform = platform();
        let reply_error = error.clone();
        let session = FakeSession::attach(&platform).run(move |session| {
            let req = action_req(session.recv(), kind);
            session.ctl(Ctl::Done {
                req,
                error: reply_error,
            });
        });
        let result = platform.session_action(kind);
        session.join().unwrap();
        match error {
            None => result.unwrap(),
            Some(WireError::Platform { message }) => assert_platform_error(result, &message),
            Some(WireError::PermissionDenied { message }) => assert!(
                matches!(result, Err(PlatformError::PermissionDenied(actual)) if actual == message)
            ),
            Some(WireError::Unsupported) => {
                assert!(matches!(result, Err(PlatformError::Unsupported)))
            }
        }
    }
}

#[test]
fn action_times_out_and_late_reply_does_not_complete_the_next_request() {
    let platform = short_platform();
    let mut session = FakeSession::attach(&platform);
    let start = Instant::now();
    assert_platform_error(
        platform.session_action(SessionActionKind::LockScreen),
        "timeout",
    );
    assert!(start.elapsed() >= Duration::from_millis(50));
    let expired_req = action_req(session.recv(), SessionActionKind::LockScreen);
    let session = session.run(move |session| {
        let req = action_req(session.recv(), SessionActionKind::WakeDisplay);
        assert_ne!(req, expired_req);
        session.ctl(Ctl::Done {
            req: expired_req,
            error: Some(WireError::Unsupported),
        });
        session.ctl(Ctl::Done { req, error: None });
    });
    platform
        .session_action(SessionActionKind::WakeDisplay)
        .unwrap();
    session.join().unwrap();
}

#[test]
fn service_actions_do_not_use_the_pipe() {
    let local = FakeLocal::default();
    let platform = Arc::new(PipePlatform::new(local.clone()));
    let mut session = FakeSession::attach(&platform);
    platform
        .session_action(SessionActionKind::CtrlAltDel)
        .unwrap();
    assert_eq!(local.calls.lock().unwrap().ctrl_alt_del, 1);
    platform
        .session_action(SessionActionKind::LockOnEnd)
        .unwrap();
    assert!(matches!(
        platform.session_action(SessionActionKind::Unspecified),
        Err(PlatformError::Unsupported)
    ));
    session.assert_empty();
    platform.detach(session.generation);
    platform
        .session_action(SessionActionKind::CtrlAltDel)
        .unwrap();
    assert_eq!(local.calls.lock().unwrap().ctrl_alt_del, 2);
}

#[test]
fn clipboard_get_maps_blobs_and_empty_or_failed_done() {
    for kind in [BlobKind::Text, BlobKind::Png] {
        let platform = platform();
        let session = FakeSession::attach(&platform).run(move |session| {
            let req = clipboard_req(session.recv());
            session.reply(PipeFrame::Blob(BlobPacket {
                req,
                kind,
                data: b"hi".to_vec(),
            }));
        });
        let received = platform.clipboard_get().unwrap();
        assert_eq!(
            received.kind,
            if kind == BlobKind::Text {
                Kind::Text
            } else {
                Kind::Png
            }
        );
        assert_eq!(received.data, b"hi");
        // Independent known SHA-256 vector for "hi".
        assert_eq!(
            received.sha256,
            [
                0x8f, 0x43, 0x43, 0x46, 0x64, 0x8f, 0x6b, 0x96, 0xdf, 0x89, 0xdd, 0xa9, 0x01, 0xc5,
                0x17, 0x6b, 0x10, 0xa6, 0xd8, 0x39, 0x61, 0xdd, 0x3c, 0x1a, 0xc8, 0x8b, 0x59, 0xb2,
                0xdc, 0x32, 0x7a, 0xa4
            ]
        );
        session.join().unwrap();
    }
    for error in [None, Some(WireError::Unsupported)] {
        let platform = platform();
        let session = FakeSession::attach(&platform).run(move |session| {
            let req = clipboard_req(session.recv());
            session.ctl(Ctl::Done { req, error });
        });
        assert!(platform.clipboard_get().is_none());
        session.join().unwrap();
    }
}

#[test]
fn clipboard_set_maps_done_and_rejects_unsupported_kind() {
    for (kind, wire_kind, error) in [
        (Kind::Png, BlobKind::Png, None),
        (
            Kind::Text,
            BlobKind::Text,
            Some(WireError::Platform {
                message: "clipboard failed".into(),
            }),
        ),
    ] {
        let platform = platform();
        let reply_error = error.clone();
        let session = FakeSession::attach(&platform).run(move |session| {
            let PipeFrame::Blob(blob) = session.recv() else {
                panic!("expected Blob")
            };
            assert_eq!(blob.kind, wire_kind);
            assert_eq!(blob.data, b"payload");
            session.ctl(Ctl::Done {
                req: blob.req,
                error: reply_error,
            });
        });
        let result = platform.clipboard_set(item(kind, b"payload"));
        if error.is_none() {
            result.unwrap();
        } else {
            assert_platform_error(result, "clipboard failed");
        }
        session.join().unwrap();
    }
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    assert!(matches!(
        platform.clipboard_set(item(Kind::Unspecified, b"")),
        Err(PlatformError::Unsupported)
    ));
    session.assert_empty();
}

#[test]
fn clipboard_timeouts_are_bounded() {
    let platform = short_platform();
    let mut session = FakeSession::attach(&platform);
    let start = Instant::now();
    assert!(platform.clipboard_get().is_none());
    assert!(start.elapsed() >= Duration::from_millis(50));
    clipboard_req(session.recv());
    assert_platform_error(platform.clipboard_set(item(Kind::Png, b"png")), "timeout");
    assert!(matches!(session.recv(), PipeFrame::Blob(_)));

    let default_platform = Arc::new(PipePlatform::new(FakeLocal::default()));
    let _session = FakeSession::attach(&default_platform);
    let start = Instant::now();
    assert!(default_platform.clipboard_get().is_none());
    assert!(
        start.elapsed() < Duration::from_secs(1),
        "clipboard pump blocked too long"
    );
}

#[tokio::test(flavor = "current_thread")]
async fn synchronous_requests_work_on_a_tokio_thread() {
    let platform = platform();
    let session = FakeSession::attach(&platform).run(|session| {
        let req = clipboard_req(session.recv());
        session.ctl(Ctl::Done { req, error: None });
        let req = action_req(session.recv(), SessionActionKind::LockScreen);
        session.ctl(Ctl::Done { req, error: None });
    });
    assert!(platform.clipboard_get().is_none());
    platform
        .session_action(SessionActionKind::LockScreen)
        .unwrap();
    session.join().unwrap();
}

#[test]
fn cursor_fanout_subscribes_once_and_unwatches_after_all_receivers_close() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx1, mut rx1) = mpsc::channel(1);
    let (tx2, mut rx2) = mpsc::channel(1);
    platform.watch_cursor(tx1);
    platform.watch_cursor(tx2);
    assert_eq!(
        session.recv(),
        PipeFrame::Ctl(Ctl::Watch {
            what: WatchKind::Cursor
        })
    );
    session.assert_empty();
    for update in [
        cursor_position(1),
        CursorUpdate::Shape(pb::CursorShape {
            id: 2,
            width: 1,
            height: 1,
            hot_x: 0,
            hot_y: 0,
            scale: 1.0,
            png: vec![1, 2, 3],
        }),
        CursorUpdate::Use(2),
        CursorUpdate::Hidden(true),
    ] {
        session.ctl(cursor_to_ctl(&update));
        assert_eq!(rx1.try_recv().unwrap(), update);
        assert_eq!(rx2.try_recv().unwrap(), update);
    }
    drop(rx1);
    drop(rx2);
    session.ctl(cursor_to_ctl(&cursor_position(2)));
    assert_eq!(
        session.recv(),
        PipeFrame::Ctl(Ctl::Unwatch {
            what: WatchKind::Cursor
        })
    );
    session.ctl(cursor_to_ctl(&cursor_position(3)));
    session.assert_empty();
}

#[test]
fn full_watchers_are_retained_and_closed_watchers_are_pruned() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (tx1, rx1) = mpsc::channel(1);
    let (tx2, mut rx2) = mpsc::channel(1);
    platform.watch_cursor(tx1);
    platform.watch_cursor(tx2);
    session.recv();
    session.ctl(cursor_to_ctl(&cursor_position(1)));
    session.ctl(cursor_to_ctl(&cursor_position(2)));
    session.assert_empty();
    assert_eq!(rx2.try_recv().unwrap(), cursor_position(1));
    drop(rx1);
    session.ctl(cursor_to_ctl(&cursor_position(3)));
    assert_eq!(rx2.try_recv().unwrap(), cursor_position(3));
    session.assert_empty();
    drop(rx2);
    session.ctl(cursor_to_ctl(&cursor_position(4)));
    assert_eq!(
        session.recv(),
        PipeFrame::Ctl(Ctl::Unwatch {
            what: WatchKind::Cursor
        })
    );
}

#[test]
fn activity_and_clipboard_watchers_fanout_and_unsubscribe() {
    let platform = platform();
    let mut session = FakeSession::attach(&platform);
    let (activity1, mut activity_rx1) = mpsc::channel(1);
    let (activity2, mut activity_rx2) = mpsc::channel(1);
    let (clip1, mut clip_rx1) = mpsc::channel(1);
    let (clip2, mut clip_rx2) = mpsc::channel(1);
    platform.watch_local_activity(activity1);
    platform.watch_local_activity(activity2);
    platform.watch_clipboard(clip1);
    platform.watch_clipboard(clip2);
    assert_eq!(
        session.recv(),
        PipeFrame::Ctl(Ctl::Watch {
            what: WatchKind::Activity
        })
    );
    assert_eq!(
        session.recv(),
        PipeFrame::Ctl(Ctl::Watch {
            what: WatchKind::Clipboard
        })
    );
    session.assert_empty();
    session.ctl(Ctl::Activity { active: true });
    assert!(activity_rx1.try_recv().unwrap());
    assert!(activity_rx2.try_recv().unwrap());
    session.ctl(Ctl::ClipboardChanged);
    clip_rx1.try_recv().unwrap();
    clip_rx2.try_recv().unwrap();
    drop((activity_rx1, activity_rx2, clip_rx1, clip_rx2));
    session.ctl(Ctl::Activity { active: false });
    session.ctl(Ctl::ClipboardChanged);
    assert_eq!(
        session.recv(),
        PipeFrame::Ctl(Ctl::Unwatch {
            what: WatchKind::Activity
        })
    );
    assert_eq!(
        session.recv(),
        PipeFrame::Ctl(Ctl::Unwatch {
            what: WatchKind::Clipboard
        })
    );
}

#[test]
fn displays_are_cached_and_only_changes_are_notified() {
    let platform = platform();
    let session = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(1);
    let (closed_tx, closed_rx) = mpsc::channel(1);
    platform.watch_displays(tx);
    platform.watch_displays(closed_tx);
    drop(closed_rx);
    let list = vec![DisplayWire::from(&display(7))];
    session.ctl(Ctl::Displays { list: list.clone() });
    assert_eq!(platform.displays(), vec![display(7)]);
    assert_eq!(rx.try_recv().unwrap(), vec![display(7)]);
    session.ctl(Ctl::Displays { list });
    assert!(rx.try_recv().is_err());
    session.ctl(Ctl::Displays {
        list: vec![DisplayWire::from(&display(8))],
    });
    session.ctl(Ctl::Displays {
        list: vec![DisplayWire::from(&display(9))],
    });
    assert_eq!(rx.try_recv().unwrap(), vec![display(8)]);
    assert_eq!(platform.displays(), vec![display(9)]);
    session.ctl(Ctl::Displays { list: vec![] });
    assert!(rx.try_recv().unwrap().is_empty());
}

#[test]
fn reconnect_replays_watch_and_keep_awake_and_ignores_stale_frames_and_detach() {
    let platform = platform();
    let mut old = FakeSession::attach(&platform);
    let (tx, mut rx) = mpsc::channel(2);
    platform.watch_cursor(tx);
    platform.power_keep_awake(true);
    assert_eq!(
        old.recv(),
        PipeFrame::Ctl(Ctl::Watch {
            what: WatchKind::Cursor
        })
    );
    assert_eq!(old.recv(), PipeFrame::Ctl(Ctl::KeepAwake { on: true }));
    old.ctl(Ctl::Desktop {
        name: "default".into(),
    });
    platform.detach(old.generation);
    assert!(!platform.is_attached());
    old.ctl(cursor_to_ctl(&cursor_position(0)));
    assert!(rx.try_recv().is_err());
    let mut new = FakeSession::attach(&platform);
    assert_eq!(new.generation, old.generation + 1);
    assert_eq!(
        new.recv(),
        PipeFrame::Ctl(Ctl::Watch {
            what: WatchKind::Cursor
        })
    );
    assert_eq!(new.recv(), PipeFrame::Ctl(Ctl::KeepAwake { on: true }));
    old.ctl(cursor_to_ctl(&cursor_position(1)));
    old.ctl(Ctl::Displays {
        list: vec![DisplayWire::from(&display(1))],
    });
    old.ctl(Ctl::Desktop {
        name: "stale".into(),
    });
    platform.detach(old.generation);
    assert!(platform.is_attached());
    assert!(rx.try_recv().is_err());
    assert!(platform.displays().is_empty());
    assert_eq!(platform.desktop().as_deref(), Some("default"));
    new.ctl(cursor_to_ctl(&cursor_position(2)));
    assert_eq!(rx.try_recv().unwrap(), cursor_position(2));
    new.ctl(Ctl::Desktop {
        name: "Winlogon".into(),
    });
    assert_eq!(platform.desktop().as_deref(), Some("Winlogon"));
}

#[test]
fn initial_attach_replays_all_live_subscriptions_and_latest_power_intent() {
    let platform = platform();
    let (cursor, _cursor_rx) = mpsc::channel(1);
    let (activity, _activity_rx) = mpsc::channel(1);
    let (clipboard, _clipboard_rx) = mpsc::channel(1);
    platform.watch_cursor(cursor);
    platform.watch_local_activity(activity);
    platform.watch_clipboard(clipboard);
    platform.power_keep_awake(true);
    let mut session = FakeSession::attach(&platform);
    for what in [WatchKind::Cursor, WatchKind::Activity, WatchKind::Clipboard] {
        assert_eq!(session.recv(), PipeFrame::Ctl(Ctl::Watch { what }));
    }
    assert_eq!(session.recv(), PipeFrame::Ctl(Ctl::KeepAwake { on: true }));
    platform.power_keep_awake(false);
    assert_eq!(session.recv(), PipeFrame::Ctl(Ctl::KeepAwake { on: false }));
    platform.detach(session.generation);
    drop((_cursor_rx, _activity_rx, _clipboard_rx));
    let mut session = FakeSession::attach(&platform);
    session.assert_empty();
    let (closed, closed_rx) = mpsc::channel(1);
    drop(closed_rx);
    platform.watch_cursor(closed);
    session.assert_empty();
}

#[test]
fn detach_wakes_waiter_immediately() {
    let platform = Arc::new(PipePlatform::new(FakeLocal::default()));
    let mut session = FakeSession::attach(&platform);
    let (tx, rx) = sync_mpsc::channel();
    let waiting_platform = platform.clone();
    let waiter = thread::spawn(move || {
        tx.send(waiting_platform.session_action(SessionActionKind::LockScreen))
            .unwrap();
    });
    action_req(session.recv(), SessionActionKind::LockScreen);
    let start = Instant::now();
    platform.detach(session.generation);
    assert_platform_error(
        rx.recv_timeout(Duration::from_millis(20)).unwrap(),
        "disconnected",
    );
    assert!(start.elapsed() < Duration::from_millis(20));
    waiter.join().unwrap();
}

#[test]
fn replacing_connection_wakes_all_waiters_and_rejects_old_replies() {
    let platform = Arc::new(PipePlatform::new(FakeLocal::default()));
    let mut old = FakeSession::attach(&platform);
    let waiting_platform = platform.clone();
    let action =
        thread::spawn(move || waiting_platform.session_action(SessionActionKind::LockScreen));
    let old_req = action_req(old.recv(), SessionActionKind::LockScreen);
    let waiting_platform = platform.clone();
    let clipboard = thread::spawn(move || waiting_platform.clipboard_set(item(Kind::Text, b"hi")));
    assert!(matches!(old.recv(), PipeFrame::Blob(_)));
    let mut new = FakeSession::attach(&platform);
    assert_platform_error(action.join().unwrap(), "disconnected");
    assert_platform_error(clipboard.join().unwrap(), "disconnected");
    let waiting_platform = platform.clone();
    let (tx, rx) = sync_mpsc::channel();
    let action = thread::spawn(move || {
        tx.send(waiting_platform.session_action(SessionActionKind::WakeDisplay))
            .unwrap()
    });
    let req = action_req(new.recv(), SessionActionKind::WakeDisplay);
    assert_ne!(req, old_req);
    old.ctl(Ctl::Done {
        req,
        error: Some(WireError::Unsupported),
    });
    assert!(matches!(rx.try_recv(), Err(sync_mpsc::TryRecvError::Empty)));
    new.ctl(Ctl::Done { req, error: None });
    rx.recv_timeout(Duration::from_secs(1)).unwrap().unwrap();
    action.join().unwrap();
}

#[test]
fn failed_outlet_send_disconnects_and_wakes_pending_requests() {
    let platform = Arc::new(PipePlatform::new(FakeLocal::default()));
    let mut session = FakeSession::attach(&platform);
    let waiting_platform = platform.clone();
    let (tx, rx) = sync_mpsc::channel();
    let action = thread::spawn(move || {
        tx.send(waiting_platform.session_action(SessionActionKind::LockScreen))
            .unwrap()
    });
    action_req(session.recv(), SessionActionKind::LockScreen);
    drop(session);
    platform.release_all();
    assert_platform_error(
        rx.recv_timeout(Duration::from_millis(20)).unwrap(),
        "disconnected",
    );
    assert!(!platform.is_attached());
    assert_platform_error(
        platform.session_action(SessionActionKind::LockScreen),
        "not connected",
    );
    assert!(platform.clipboard_get().is_none());
    action.join().unwrap();
}

#[test]
fn concurrent_requests_match_replies_by_request_id() {
    let platform = Arc::new(PipePlatform::new(FakeLocal::default()));
    let mut session = FakeSession::attach(&platform);
    let caller = platform.clone();
    let action = thread::spawn(move || caller.session_action(SessionActionKind::LockScreen));
    let action_id = action_req(session.recv(), SessionActionKind::LockScreen);
    let caller = platform.clone();
    let clipboard = thread::spawn(move || caller.clipboard_get());
    let clip_id = clipboard_req(session.recv());
    assert_ne!(action_id, clip_id);
    session.reply(PipeFrame::Blob(BlobPacket {
        req: clip_id,
        kind: BlobKind::Text,
        data: b"hi".to_vec(),
    }));
    session.ctl(Ctl::Done {
        req: action_id,
        error: Some(WireError::Platform {
            message: "action failed".into(),
        }),
    });
    assert_eq!(clipboard.join().unwrap().unwrap().data, b"hi");
    assert_platform_error(action.join().unwrap(), "action failed");
}

#[test]
fn permissions_and_disconnected_media_and_default_queries() {
    let platform = platform();
    assert_eq!(
        platform.permissions(),
        Permissions {
            screen_recording: true,
            accessibility: true
        }
    );
    platform.request_permission(PermissionKind::ScreenRecording);
    platform.request_permission(PermissionKind::Accessibility);
    assert!(platform.audio_capture_supported());
    assert_eq!(platform.screen_sharing_enabled(), None);
    assert_eq!(platform.filevault_enabled(), None);
    let (sink, _rx) = mpsc::channel(1);
    assert!(matches!(
        platform.capture(
            7,
            CaptureConfig {
                fps: 60,
                max_width: 0,
                max_height: 0
            },
            sink
        ),
        Err(PlatformError::Platform(message)) if message == "session process not connected"
    ));
    let (sink, _rx) = mpsc::channel(1);
    assert!(matches!(
        platform.encoder(EncoderConfig {
            codec: pb::Codec::H264,
            width: 1920,
            height: 1080,
            fps: 60,
            bitrate_kbps: 4000,
            sink
        }),
        Err(PlatformError::Platform(message)) if message == "session process not connected"
    ));
    let (sink, _rx) = mpsc::channel(1);
    assert!(matches!(
        platform.capture_audio(
            AudioCaptureConfig {
                sample_rate: 48000,
                channels: 2,
                exclude_self: true
            },
            sink
        ),
        Err(PlatformError::Platform(message)) if message == "session process not connected"
    ));
}

#[test]
fn service_local_queries_wol_storage_and_authenticator_are_delegated() {
    let authenticator: Arc<dyn SystemAuthenticator> = Arc::new(FakeAuthenticator);
    let local = FakeLocal {
        authenticator: Some(authenticator.clone()),
        ..Default::default()
    };
    let platform = PipePlatform::new(local.clone());
    assert_eq!(platform.info(), local.info());
    assert_eq!(platform.console_state(), ConsoleState::Locked);
    assert_eq!(platform.nics(), local.nics());
    let nic = platform.nics().remove(0);
    let mac = [2, 0, 0, 0, 0, 8];
    platform.send_wol(mac, &nic).unwrap();
    assert_eq!(local.calls.lock().unwrap().wol, vec![(mac, nic)]);
    assert_eq!(platform.keystore().load("test-key").unwrap(), None);
    platform
        .keystore()
        .store("test-key", b"test-value")
        .unwrap();
    assert_eq!(
        platform.keystore().load("test-key").unwrap().unwrap(),
        b"test-value"
    );
    assert_eq!(local.keys.load("test-key").unwrap().unwrap(), b"test-value");
    platform.keystore().delete("test-key").unwrap();
    assert_eq!(local.keys.load("test-key").unwrap(), None);
    assert!(Arc::ptr_eq(
        &platform.system_authenticator().unwrap(),
        &authenticator
    ));
    assert!(PipePlatform::new(FakeLocal::default())
        .system_authenticator()
        .is_none());
}
