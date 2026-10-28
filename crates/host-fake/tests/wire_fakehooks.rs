use host_core::inject::DEFAULT_PIXELS_PER_LINE;
use host_core::platform::*;
use host_fake::FakePlatform;
use rsp_core::hid::{Button, Mods, ScrollPhase, ScrollUnit, TouchPhase};
use rsp_core::input::MouseButton;
use rsp_core::pb;
use tokio::sync::mpsc;

fn key() -> InputMsg {
    InputMsg::Key {
        usage: 0x04,
        down: true,
        mods: Mods::LSHIFT,
    }
}

fn button(button: Button) -> InputMsg {
    InputMsg::MouseButton {
        button,
        down: true,
        x: 0.25,
        y: 0.75,
        clicks: 2,
    }
}

#[test]
fn input_to_m0_covers_supported_and_unsupported_events() {
    let scale = DEFAULT_PIXELS_PER_LINE;
    let mut cases = vec![
        (
            InputMsg::MouseMove { x: 0.25, y: 0.75 },
            Some(InputEvent::MouseMove { x: 0.25, y: 0.75 }),
        ),
        (
            InputMsg::Scroll {
                dx: 1.0,
                dy: -2.0,
                phase: ScrollPhase::Began,
                momentum: ScrollPhase::Changed,
                unit: ScrollUnit::Line,
            },
            Some(InputEvent::Scroll {
                dx: scale as f32,
                dy: (-2.0 * scale) as f32,
            }),
        ),
        (
            InputMsg::Scroll {
                dx: 1.25,
                dy: -2.5,
                phase: ScrollPhase::Changed,
                momentum: ScrollPhase::Ended,
                unit: ScrollUnit::Pixel,
            },
            Some(InputEvent::Scroll { dx: 1.25, dy: -2.5 }),
        ),
        (button(Button::Back), None),
        (button(Button::Forward), None),
        (key(), None),
        (InputMsg::Text("committed text".into()), None),
        (InputMsg::ReleaseAll, None),
        (
            InputMsg::Touch {
                id: 1,
                phase: TouchPhase::Began,
                x: 0.25,
                y: 0.75,
                pressure: 0.5,
            },
            None,
        ),
    ];
    for (new, old) in [
        (Button::Left, MouseButton::Left),
        (Button::Right, MouseButton::Right),
        (Button::Middle, MouseButton::Middle),
    ] {
        cases.push((
            button(new),
            Some(InputEvent::MouseButton {
                button: old,
                down: true,
                x: 0.25,
                y: 0.75,
                clicks: 2,
            }),
        ));
    }
    for (msg, expected) in cases {
        assert_eq!(input_to_m0(&msg), expected, "input: {msg:?}");
    }
}

#[test]
fn m0_events_round_trip_through_rsp1() {
    let mut events = vec![
        InputEvent::MouseMove { x: 0.25, y: 0.75 },
        InputEvent::Scroll { dx: 1.25, dy: -2.5 },
    ];
    for button in [MouseButton::Left, MouseButton::Right, MouseButton::Middle] {
        for down in [true, false] {
            events.push(InputEvent::MouseButton {
                button,
                down,
                x: 0.25,
                y: 0.75,
                clicks: 2,
            });
        }
    }
    for ev in events {
        let msg = input_from_m0(ev);
        assert_eq!(input_to_m0(&msg), Some(ev));
        assert_eq!(input_from_m0(input_to_m0(&msg).unwrap()), msg);
    }
    assert_eq!(
        input_from_m0(InputEvent::Scroll { dx: 1.25, dy: -2.5 }),
        InputMsg::Scroll {
            dx: 1.25,
            dy: -2.5,
            phase: ScrollPhase::None,
            momentum: ScrollPhase::None,
            unit: ScrollUnit::Pixel,
        }
    );
}

// Forward every required M0 method, deliberately inheriting inject_input.
struct M0Only(FakePlatform);

impl Platform for M0Only {
    fn info(&self) -> HostInfo {
        self.0.info()
    }
    fn displays(&self) -> Vec<DisplayInfo> {
        self.0.displays()
    }
    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        self.0.watch_displays(tx);
    }
    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        self.0.capture(display, cfg, sink)
    }
    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>> {
        self.0.encoder(cfg)
    }
    fn inject(&self, ev: InputEvent, display: &DisplayInfo) {
        self.0.inject(ev, display);
    }
    fn release_all(&self) {
        self.0.release_all();
    }
    fn session_action(&self, kind: SessionActionKind) -> Result<()> {
        self.0.session_action(kind)
    }
    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        self.0.watch_cursor(tx);
    }
    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        self.0.watch_local_activity(tx);
    }
    fn clipboard_get(&self) -> Option<ClipboardItem> {
        self.0.clipboard_get()
    }
    fn clipboard_set(&self, item: ClipboardItem) -> Result<()> {
        self.0.clipboard_set(item)
    }
    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        self.0.watch_clipboard(tx);
    }
    fn power_keep_awake(&self, on: bool) {
        self.0.power_keep_awake(on);
    }
    fn console_state(&self) -> ConsoleState {
        self.0.console_state()
    }
    fn permissions(&self) -> Permissions {
        self.0.permissions()
    }
    fn request_permission(&self, permission: PermissionKind) {
        self.0.request_permission(permission);
    }
    fn nics(&self) -> Vec<Nic> {
        self.0.nics()
    }
    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        self.0.send_wol(mac, via)
    }
    fn keystore(&self) -> &dyn KeyStore {
        self.0.keystore()
    }
}

#[test]
fn default_inject_input_supports_m0_and_release_all() {
    let platform = M0Only(FakePlatform::new());
    let display = &platform.displays()[0];
    platform.inject_input(&InputMsg::MouseMove { x: 0.25, y: 0.75 }, display);
    assert_eq!(
        platform.0.injected_events(),
        vec![InputEvent::MouseMove { x: 0.25, y: 0.75 }]
    );
    platform.inject_input(&key(), display);
    platform.inject_input(&InputMsg::Text("text".into()), display);
    assert_eq!(platform.0.injected_events().len(), 1);
    assert!(platform.0.injected_msgs().is_empty());
    platform.inject_input(&InputMsg::ReleaseAll, display);
    assert_eq!(platform.0.release_count(), 1);
}

#[test]
fn fake_records_rsp1_input_display_and_legacy_mouse_events() {
    let platform = FakePlatform::new();
    let clone = platform.clone();
    let mut display = platform.displays()[0].clone();
    display.id = 77;
    let msgs = vec![
        key(),
        InputMsg::Text("committed text".into()),
        InputMsg::MouseMove { x: 0.25, y: 0.75 },
    ];
    for msg in &msgs {
        clone.inject_input(msg, &display);
    }
    assert_eq!(platform.injected_msgs(), msgs);
    assert_eq!(
        platform.injected_with_display(),
        msgs.into_iter().map(|msg| (msg, 77)).collect::<Vec<_>>()
    );
    assert_eq!(
        platform.injected_events(),
        vec![InputEvent::MouseMove { x: 0.25, y: 0.75 }]
    );
    clone.inject_input(&InputMsg::ReleaseAll, &display);
    assert_eq!(platform.release_count(), 1);
    assert_eq!(platform.injected_msgs().last(), Some(&InputMsg::ReleaseAll));
    clone.clear_injected();
    assert!(platform.injected_msgs().is_empty());
    assert!(platform.injected_with_display().is_empty());
    assert!(platform.injected_events().is_empty());
    platform.inject(InputEvent::MouseMove { x: 0.5, y: 0.5 }, &display);
    assert_eq!(platform.injected_events().len(), 1);
    assert!(platform.injected_msgs().is_empty());
}

#[tokio::test]
async fn local_activity_notifies_all_observers_and_handles_backpressure() {
    let platform = FakePlatform::new();
    let (tx_a, mut rx_a) = mpsc::channel(1);
    let (tx_b, mut rx_b) = mpsc::channel(1);
    platform.watch_local_activity(tx_a);
    platform.watch_local_activity(tx_b);
    assert_eq!(platform.clone().local_input(), 2);
    assert!(rx_a.try_recv().unwrap());
    assert!(rx_b.try_recv().unwrap());
    drop(rx_b);
    assert_eq!(platform.local_input(), 1);
    assert_eq!(platform.local_input(), 0); // The full receiver stays registered.
    assert!(rx_a.try_recv().unwrap());
    assert_eq!(platform.local_input(), 1);
    assert!(rx_a.try_recv().unwrap());
    drop(rx_a);
    assert_eq!(platform.local_input(), 0);
}

fn assert_clipboard(actual: &ClipboardItem, expected: &ClipboardItem) {
    assert_eq!(actual.kind, expected.kind);
    assert_eq!(actual.data, expected.data);
    assert_eq!(actual.sha256, expected.sha256);
}

#[tokio::test]
async fn clipboard_updates_notify_observers_including_remote_writes() {
    let platform = FakePlatform::new();
    assert!(platform.clipboard_get().is_none());
    let (tx_a, mut rx_a) = mpsc::channel(4);
    let (tx_b, mut rx_b) = mpsc::channel(4);
    platform.watch_clipboard(tx_a);
    platform.watch_clipboard(tx_b);
    let text = ClipboardItem {
        kind: pb::clipboard_header::Kind::Text,
        data: b"clipboard text".to_vec(),
        sha256: [1; 32],
    };
    platform.clone().set_clipboard(Some(text.clone()));
    rx_a.try_recv().unwrap();
    rx_b.try_recv().unwrap();
    assert_clipboard(&platform.clipboard_get().unwrap(), &text);
    assert!(platform.clipboard_writes().is_empty());

    let png = ClipboardItem {
        kind: pb::clipboard_header::Kind::Png,
        data: vec![0x89, b'P', b'N', b'G'],
        sha256: [2; 32],
    };
    platform.clone().clipboard_set(png.clone()).unwrap();
    rx_a.try_recv().unwrap();
    rx_b.try_recv().unwrap();
    assert_eq!(platform.clipboard_writes().len(), 1);
    assert_clipboard(&platform.clipboard_writes()[0], &png);
    assert_clipboard(&platform.clipboard_get().unwrap(), &png);
    drop(rx_b);
    platform.set_clipboard(None);
    rx_a.try_recv().unwrap();
    assert!(platform.clipboard_get().is_none());
    assert_eq!(platform.clipboard_writes().len(), 1);
}

#[tokio::test]
async fn display_hotplug_updates_capture_and_all_observers() {
    let platform = FakePlatform::new();
    let displays = platform.displays();
    assert_eq!(displays.len(), 1);
    assert_eq!(
        (displays[0].id, displays[0].width_px, displays[0].height_px),
        (1, 1920, 1080)
    );
    let (tx_a, mut rx_a) = mpsc::channel(4);
    let (tx_b, mut rx_b) = mpsc::channel(4);
    platform.watch_displays(tx_a);
    platform.watch_displays(tx_b);
    assert_eq!(rx_a.try_recv().unwrap(), displays);
    assert_eq!(rx_b.try_recv().unwrap(), displays);
    let mut displays = displays;
    let mut secondary = displays[0].clone();
    secondary.id = 77;
    secondary.primary = false;
    secondary.x = 1920;
    displays.push(secondary.clone());
    platform.clone().set_displays(displays.clone());
    assert_eq!(rx_a.try_recv().unwrap(), displays);
    assert_eq!(rx_b.try_recv().unwrap(), displays);
    assert_eq!(platform.displays(), displays);

    let cfg = CaptureConfig {
        fps: 30,
        max_width: 0,
        max_height: 0,
    };
    let (tx, mut frames) = mpsc::channel(1);
    let capture = platform.capture(77, cfg, tx.clone()).unwrap();
    assert_eq!(platform.capture_displays(), vec![77]);
    assert_eq!(platform.active_captures(), 1);
    let frame = frames.recv().await.unwrap();
    assert_eq!(
        (frame.display_id, frame.width, frame.height),
        (77, 1920, 1080)
    );
    assert!(platform.capture(5, cfg, tx).is_err());
    assert_eq!(platform.capture_displays(), vec![77]);
    drop(capture);
    assert_eq!(platform.active_captures(), 0);

    assert_eq!(
        FakePlatform::with_displays(vec![secondary.clone()]).displays(),
        vec![secondary]
    );
    assert!(FakePlatform::with_displays(Vec::new())
        .displays()
        .is_empty());
    drop(rx_b);
    platform.set_displays(Vec::new());
    assert!(rx_a.try_recv().unwrap().is_empty());
}

#[tokio::test]
async fn cursor_updates_reach_all_live_observers_after_initial_position() {
    let platform = FakePlatform::new();
    let (tx_a, mut rx_a) = mpsc::channel(1);
    let (tx_b, mut rx_b) = mpsc::channel(1);
    platform.watch_cursor(tx_a);
    platform.watch_cursor(tx_b);
    let initial = CursorUpdate::Position {
        display_id: 1,
        seq: 0,
        x: 0.5,
        y: 0.5,
        visible: true,
    };
    assert_eq!(rx_a.try_recv().unwrap(), initial);
    assert_eq!(rx_b.try_recv().unwrap(), initial);
    let update = CursorUpdate::Position {
        display_id: 77,
        seq: 42,
        x: 0.25,
        y: 0.75,
        visible: true,
    };
    assert_eq!(platform.clone().push_cursor(update.clone()), 2);
    assert_eq!(rx_a.try_recv().unwrap(), update);
    assert_eq!(rx_b.try_recv().unwrap(), update);
    drop(rx_b);
    assert_eq!(platform.push_cursor(CursorUpdate::Hidden(true)), 1);
    assert_eq!(rx_a.try_recv().unwrap(), CursorUpdate::Hidden(true));
}

#[test]
fn console_and_power_hooks_share_state_across_clones() {
    let platform = FakePlatform::new();
    assert_eq!(platform.console_state(), ConsoleState::LoggedIn);
    assert!(!platform.keep_awake());
    assert!(platform.keep_awake_calls().is_empty());
    let clone = platform.clone();
    clone.set_console_state(ConsoleState::LoginWindow);
    assert_eq!(platform.console_state(), ConsoleState::LoginWindow);
    clone.power_keep_awake(true);
    assert!(platform.keep_awake());
    clone.power_keep_awake(false);
    assert_eq!(platform.keep_awake_calls(), vec![true, false]);
    assert!(!platform.keep_awake());
}

#[test]
fn session_actions_record_attempts_and_can_fail() {
    let platform = FakePlatform::new();
    platform
        .session_action(SessionActionKind::LockScreen)
        .unwrap();
    assert_eq!(
        platform.session_actions(),
        vec![SessionActionKind::LockScreen]
    );
    platform.clone().fail_session_actions(true);
    assert!(matches!(
        platform.session_action(SessionActionKind::LockScreen),
        Err(PlatformError::Platform(message)) if message == "fake failure"
    ));
    assert_eq!(
        platform.session_actions(),
        vec![SessionActionKind::LockScreen; 2]
    );
    platform.fail_session_actions(false);
    platform
        .session_action(SessionActionKind::LockScreen)
        .unwrap();
}

#[test]
fn wol_records_the_mac_and_interface_name() {
    let nic = Nic {
        name: "fake-ethernet".into(),
        addresses: vec!["10.30.0.1".parse().unwrap()],
        networks: vec!["10.30.0.0/24".into()],
        mac: Some([6, 5, 4, 3, 2, 1]),
        tailnet: false,
    };
    let platform = FakePlatform::with_nics(vec![nic.clone()]);
    assert_eq!(platform.nics(), vec![nic.clone()]);
    platform.clone().send_wol([1, 2, 3, 4, 5, 6], &nic).unwrap();
    assert_eq!(platform.wol_sends(), vec![([1, 2, 3, 4, 5, 6], nic.name)]);
}

#[tokio::test]
async fn encoder_hooks_record_start_rate_and_forced_keyframes() {
    let platform = FakePlatform::new();
    let (sink, _output) = mpsc::channel(1);
    let cfg = EncoderConfig {
        codec: pb::Codec::Hevc,
        width: 1280,
        height: 720,
        fps: 60,
        bitrate_kbps: 6000,
        sink,
    };
    let mut encoder = platform.clone().encoder(cfg.clone()).unwrap();
    assert_eq!(platform.encoders_started(), vec![(1280, 720, 60, 6000)]);
    assert_eq!(platform.forced_keyframes(), 0);
    encoder.set_rate(3000, 30);
    encoder.force_keyframe();
    assert_eq!(platform.encoder_rates(), vec![(3000, 30)]);
    assert_eq!(platform.forced_keyframes(), 1);
    encoder.set_rate(0, 0);
    encoder.set_rate(200_000, 200);
    assert_eq!(
        platform.encoder_rates(),
        vec![(3000, 30), (0, 0), (200_000, 200)]
    );
    let mut other = platform.encoder(cfg.clone()).unwrap();
    other.force_keyframe();
    assert_eq!(platform.forced_keyframes(), 2);
    assert_eq!(platform.encoders_started().len(), 2);
    assert!(platform.encoder(EncoderConfig { fps: 0, ..cfg }).is_err());
    assert_eq!(platform.encoders_started().len(), 2);
}
