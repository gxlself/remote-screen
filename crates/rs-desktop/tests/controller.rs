use std::sync::{
    atomic::{AtomicU64, AtomicUsize, Ordering},
    Arc, Mutex,
};

use rs_desktop::{
    controller::{
        ControllerConfig, ControllerObserver, ControllerSettings, SessionController, SessionIo,
    },
    frames::{FrameAdapter, GateConfig, KeyframeReason, VideoEvents},
    platform::{
        BitstreamFormat, ClientOs, DecodeResult, DesktopFrame, FrameDone, FrameSink, HostOs,
        KeyInput, MenuCommand, PointerInput, SessionMenu, SessionWindow, StreamInfo, VideoCodec,
        WindowChange, WindowCommand, WindowEvents,
    },
    session::SessionCall,
};
use rs_mobile::rscore::dto_core::{CoreEvent, Role, SessionPhase};
use rsp_core::{
    hid::{Button, InputMsg, Mods},
    pb,
};

const NS_CONTROL: u64 = 1 << 18;
const NS_OPTION: u64 = 1 << 19;
const NS_COMMAND: u64 = 1 << 20;

#[derive(Debug, Clone, PartialEq)]
enum Io {
    Input(InputMsg),
    Pointer(f32, f32),
    Call(SessionCall),
    Close,
    Keyframe,
    Decoded,
}

struct FakeIo {
    log: Mutex<Vec<Io>>,
    now: AtomicU64,
}

impl FakeIo {
    fn new() -> Arc<Self> {
        Arc::new(Self {
            log: Mutex::new(Vec::new()),
            now: AtomicU64::new(1_000_000),
        })
    }

    fn take(&self) -> Vec<Io> {
        std::mem::take(&mut *self.log.lock().unwrap())
    }

    fn pointers(&self) -> Vec<(f32, f32)> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|io| match io {
                Io::Pointer(x, y) => Some((*x, *y)),
                _ => None,
            })
            .collect()
    }

    fn calls(&self) -> Vec<SessionCall> {
        self.log
            .lock()
            .unwrap()
            .iter()
            .filter_map(|io| match io {
                Io::Call(call) => Some(call.clone()),
                _ => None,
            })
            .collect()
    }

    fn advance(&self, us: u64) {
        self.now.fetch_add(us, Ordering::SeqCst);
    }

    fn push(&self, io: Io) {
        self.log.lock().unwrap().push(io);
    }
}

impl SessionIo for FakeIo {
    fn send_input(&self, msg: InputMsg) {
        self.push(Io::Input(msg));
    }

    fn send_pointer(&self, x: f32, y: f32) {
        self.push(Io::Pointer(x, y));
    }

    fn call(&self, call: SessionCall) {
        self.push(Io::Call(call));
    }

    fn close(&self) {
        self.push(Io::Close);
    }

    fn request_keyframe(&self, _reason: KeyframeReason) {
        self.push(Io::Keyframe);
    }

    fn report_decoded(&self, _decode_us: u64, _latency_us: u64, _queue: u32) {
        self.push(Io::Decoded);
    }

    fn now_us(&self) -> u64 {
        self.now.load(Ordering::SeqCst)
    }
}

#[derive(Default)]
struct FakeSink {
    closes: AtomicUsize,
}

impl FrameSink for FakeSink {
    fn format(&self) -> BitstreamFormat {
        BitstreamFormat::AnnexB
    }

    fn configure(&self, _info: &StreamInfo) {}

    fn submit(&self, _frame: DesktopFrame, done: FrameDone) {
        done.finish(DecodeResult::Shown { decode_us: 1000 });
    }

    fn close(&self) {
        self.closes.fetch_add(1, Ordering::SeqCst);
    }
}

#[derive(Default)]
struct FakeWindow {
    sink: Arc<FakeSink>,
    commands: Mutex<Vec<WindowCommand>>,
}

impl FakeWindow {
    fn take(&self) -> Vec<WindowCommand> {
        std::mem::take(&mut *self.commands.lock().unwrap())
    }
}

impl SessionWindow for FakeWindow {
    fn frame_sink(&self) -> Arc<dyn FrameSink> {
        self.sink.clone()
    }

    fn apply(&self, cmd: WindowCommand) {
        self.commands.lock().unwrap().push(cmd);
    }
}

#[derive(Default)]
struct FakeObserver {
    menus: Mutex<Vec<(u64, SessionMenu)>>,
    focus: Mutex<Vec<(u64, bool)>>,
    ended: Mutex<Vec<(u64, String, bool)>>,
}

impl ControllerObserver for FakeObserver {
    fn menu_changed(&self, session_id: u64, menu: &SessionMenu) {
        self.menus.lock().unwrap().push((session_id, menu.clone()));
    }

    fn focus_changed(&self, session_id: u64, focused: bool) {
        self.focus.lock().unwrap().push((session_id, focused));
    }

    fn ended(&self, session_id: u64, host_id: &str, by_user: bool) {
        self.ended
            .lock()
            .unwrap()
            .push((session_id, host_id.to_owned(), by_user));
    }
}

struct Ctl {
    c: Arc<SessionController>,
    io: Arc<FakeIo>,
    window: Arc<FakeWindow>,
    observer: Arc<FakeObserver>,
}

impl Ctl {
    /// 未绑定窗口。
    fn unbound(client: ClientOs, host: HostOs, settings: ControllerSettings) -> Self {
        let io = FakeIo::new();
        let observer = Arc::new(FakeObserver::default());
        let c = SessionController::new(
            ControllerConfig {
                session_id: 7,
                host_id: "host-1".into(),
                host_name: "客厅 Mac".into(),
                host,
                client,
                quality: "auto".into(),
                settings,
            },
            io.clone(),
            observer.clone(),
        );
        Self {
            c,
            io,
            window: Arc::new(FakeWindow::default()),
            observer,
        }
    }

    fn bound(client: ClientOs, host: HostOs, settings: ControllerSettings) -> Self {
        let t = Self::unbound(client, host, settings);
        t.c.bind_window(t.window.clone());
        t
    }

    fn event(&self, ev: CoreEvent) {
        self.c.on_core_event(&ev);
    }

    fn change(&self, ev: WindowChange) {
        WindowEvents::window(&*self.c, ev);
    }

    fn key(&self, ev: KeyInput) {
        self.c.key(ev);
    }

    fn pointer(&self, ev: PointerInput) {
        self.c.pointer(ev);
    }

    /// LCtrl + LAlt 按下再松开（⌃⌥）。
    fn chord(&self) {
        self.key(KeyInput::MacFlags {
            vk: 0x3B,
            flags: NS_CONTROL | 0x0001,
        });
        self.key(KeyInput::MacFlags {
            vk: 0x3A,
            flags: NS_CONTROL | 0x0001 | NS_OPTION | 0x0020,
        });
        self.key(KeyInput::MacFlags {
            vk: 0x3B,
            flags: NS_OPTION | 0x0020,
        });
        self.key(KeyInput::MacFlags { vk: 0x3A, flags: 0 });
    }
}

/// 绑定窗口，再依次喂 Focus(true)、role Controller、phase Streaming，使输入开启。
fn ctl_with(client: ClientOs, host: HostOs, settings: ControllerSettings) -> Ctl {
    let t = Ctl::bound(client, host, settings);
    t.change(WindowChange::Focus(true));
    t.event(role(Role::Controller));
    t.event(phase(SessionPhase::Streaming));
    t.io.take();
    t
}

fn ctl(client: ClientOs, host: HostOs) -> Ctl {
    ctl_with(client, host, ControllerSettings::default())
}

fn phase(phase: SessionPhase) -> CoreEvent {
    CoreEvent::SessionState {
        session_id: "7".into(),
        host_id: "host-1".into(),
        phase,
        path: None,
        transport: None,
        derp_region: None,
        dial: None,
        approval_timeout_s: None,
        resumed: None,
        reconnect_reason: None,
    }
}

fn role(role: Role) -> CoreEvent {
    CoreEvent::SessionRole {
        session_id: "7".into(),
        role,
        controller: None,
        viewers: Vec::new(),
        input_paused_local: false,
    }
}

fn error(message: &str) -> CoreEvent {
    CoreEvent::SessionError {
        session_id: "7".into(),
        host_id: "host-1".into(),
        code: Some(3),
        reason: None,
        message: Some(message.into()),
        detail: None,
        retry_after_s: None,
    }
}

fn mac_key(vk: u16, down: bool) -> KeyInput {
    KeyInput::MacKey {
        vk,
        down,
        repeat: false,
    }
}

fn key(usage: u16, down: bool, mods: Mods) -> Io {
    Io::Input(InputMsg::Key { usage, down, mods })
}

fn has_key(log: &[Io]) -> bool {
    log.iter()
        .any(|io| matches!(io, Io::Input(InputMsg::Key { .. })))
}

fn titles(commands: &[WindowCommand]) -> Vec<String> {
    commands
        .iter()
        .filter_map(|command| match command {
            WindowCommand::SetTitle(title) => Some(title.clone()),
            _ => None,
        })
        .collect()
}

fn resize(t: &Ctl, width: f64, height: f64) {
    t.change(WindowChange::Resized {
        width,
        height,
        scale: 2.0,
    });
}

#[test]
fn start_applies_title() {
    let t = Ctl::bound(ClientOs::Mac, HostOs::Mac, ControllerSettings::default());
    let commands = t.window.take();
    assert_eq!(titles(&commands), ["客厅 Mac — 正在连接…"]);
    assert!(commands.contains(&WindowCommand::SetOverlayText(Some("正在连接…".into()))));
    // SetSessionMenu 同时通知外壳。
    assert_eq!(t.observer.menus.lock().unwrap().len(), 1);
    assert!(t.io.take().is_empty());
}

#[test]
fn events_before_window_replayed() {
    let t = Ctl::unbound(ClientOs::Mac, HostOs::Mac, ControllerSettings::default());
    t.event(role(Role::Viewer));
    t.event(phase(SessionPhase::Streaming));
    t.c.bind_window(t.window.clone());
    let commands = t.window.take();
    assert_eq!(
        titles(&commands),
        ["客厅 Mac — 正在连接…", "客厅 Mac（仅观看）"]
    );
    assert!(commands.contains(&WindowCommand::ShowRemoteCursor(true)));
}

#[test]
fn key_routed_when_enabled() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.key(mac_key(0x00, true));
    assert_eq!(t.io.take(), [key(0x04, true, Mods::NONE)]);
}

#[test]
fn key_dropped_when_viewer() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.event(role(Role::Viewer));
    t.io.take();
    t.key(mac_key(0x00, true));
    t.key(mac_key(0x00, false));
    assert!(t.io.take().is_empty());
}

#[test]
fn chord_cannot_enable_when_disabled() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.event(role(Role::Viewer));
    t.io.take();
    t.chord();
    t.key(mac_key(0x00, true));
    assert!(!has_key(&t.io.take()));
    assert!(!t.c.menu().keyboard_captured);
}

#[test]
fn chord_cannot_capture_unfocused() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.change(WindowChange::Focus(false));
    t.io.take();
    // 输入开着、键盘没捕获：和弦把路由器切到「捕获」后要按 fsm 切回去。
    t.chord();
    t.key(mac_key(0x00, true));
    assert!(!has_key(&t.io.take()));
    assert!(!t.c.menu().keyboard_captured);
}

#[test]
fn chord_release_updates_title() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.window.take();
    t.chord();
    let commands = t.window.take();
    assert!(commands.contains(&WindowCommand::SetKeyboardCapture(false)));
    assert!(titles(&commands)
        .iter()
        .any(|title| title.contains("键盘已释放")));
    let log = t.io.take();
    let releases = log
        .iter()
        .filter(|io| **io == Io::Input(InputMsg::ReleaseAll))
        .count();
    assert_eq!(releases, 1, "{log:?}");
    assert!(!t.c.menu().keyboard_captured);
}

#[test]
fn click_recaptures() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    resize(&t, 1000.0, 1000.0);
    t.chord();
    t.window.take();
    t.io.take();
    t.pointer(PointerInput::Button {
        button: 0,
        down: true,
        x: 500.0,
        y: 500.0,
        clicks: 0,
    });
    assert!(t
        .window
        .take()
        .contains(&WindowCommand::SetKeyboardCapture(true)));
    assert_eq!(
        t.io.take(),
        [Io::Input(InputMsg::MouseButton {
            button: Button::Left,
            down: true,
            x: 0.5,
            y: 0.5,
            clicks: 1,
        })]
    );
    assert!(t.c.menu().keyboard_captured);
    t.key(mac_key(0x00, true));
    assert_eq!(t.io.take(), [key(0x04, true, Mods::NONE)]);
}

#[test]
fn pointer_throttled_and_flushed() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    resize(&t, 1000.0, 1000.0);
    t.c.stream(&StreamInfo {
        codec: VideoCodec::Hevc,
        width: 1000,
        height: 1000,
        display_id: 0,
    });
    let moved = |x, y| PointerInput::Move {
        x,
        y,
        dx: 0.0,
        dy: 0.0,
    };
    t.pointer(moved(100.0, 100.0));
    t.io.advance(500);
    t.pointer(moved(200.0, 200.0));
    assert_eq!(t.io.pointers(), [(0.1, 0.1)]);
    t.c.tick();
    assert_eq!(t.io.pointers(), [(0.1, 0.1)]);
    t.io.advance(9_000);
    t.c.tick();
    assert_eq!(t.io.pointers(), [(0.1, 0.1), (0.2, 0.2)]);
}

#[test]
fn focus_loss_release_all() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.key(mac_key(0x00, true));
    t.io.take();
    t.change(WindowChange::Focus(false));
    assert_eq!(t.io.take(), [Io::Input(InputMsg::ReleaseAll)]);
    assert_eq!(t.observer.focus.lock().unwrap().last(), Some(&(7, false)));
}

#[test]
fn menu_combo() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.change(WindowChange::Menu(MenuCommand::SendCombo("cmdTab".into())));
    assert_eq!(t.io.calls(), [SessionCall::Combo("cmdTab".into())]);
    assert_eq!(t.io.take().len(), 1);
}

#[test]
fn cursor_shape_to_window() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.window.take();
    t.c.cursor_msg(&pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Shape(pb::CursorShape {
            id: 9,
            width: 16,
            height: 16,
            hot_x: 1,
            hot_y: 2,
            scale: 2.0,
            png: vec![1, 2, 3],
        })),
    });
    let commands = t.window.take();
    assert!(
        commands.iter().any(|command| matches!(
            command,
            WindowCommand::SetCursorImage(Some(image)) if image.id == 9
        )),
        "{commands:?}"
    );
}

#[test]
fn viewer_remote_cursor() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.event(role(Role::Viewer));
    t.window.take();
    t.c.host_pointer(0.3, 0.4, true);
    assert!(t
        .window
        .take()
        .contains(&WindowCommand::MoveVirtualCursor { x: 0.3, y: 0.4 }));

    // 控制者在自由模式下本机光标就是远端光标，不画。
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    t.window.take();
    t.c.host_pointer(0.3, 0.4, true);
    assert!(!t
        .window
        .take()
        .iter()
        .any(|command| matches!(command, WindowCommand::MoveVirtualCursor { .. })));
}

#[test]
fn ended_notifies() {
    let t = ctl(ClientOs::Mac, HostOs::Mac);
    let adapter = FrameAdapter::new(
        t.window.sink.clone(),
        t.c.clone(),
        t.c.clone(),
        GateConfig::default(),
    );
    t.c.bind_adapter(adapter);
    t.event(error("对方已断开"));
    assert!(t.observer.ended.lock().unwrap().is_empty());
    t.event(phase(SessionPhase::Ended));
    assert_eq!(
        *t.observer.ended.lock().unwrap(),
        [(7, "host-1".to_owned(), false)]
    );
    assert_eq!(t.window.sink.closes.load(Ordering::SeqCst), 1);
    assert!(t.c.is_ended());
    assert!(titles(&t.window.take()).contains(&"客厅 Mac — 已断开".to_owned()));

    t.io.take();
    t.key(mac_key(0x00, true));
    t.pointer(PointerInput::Button {
        button: 0,
        down: true,
        x: 1.0,
        y: 1.0,
        clicks: 0,
    });
    assert!(t.io.take().is_empty());

    // 结束后才绑定的 adapter 立即关闭。
    let late = FrameAdapter::new(
        t.window.sink.clone(),
        t.c.clone(),
        t.c.clone(),
        GateConfig::default(),
    );
    t.c.bind_adapter(late);
    assert_eq!(t.window.sink.closes.load(Ordering::SeqCst), 2);
    assert_eq!(t.observer.ended.lock().unwrap().len(), 1);
}

#[test]
fn remap_selection() {
    let command_down = KeyInput::MacFlags {
        vk: 0x37,
        flags: NS_COMMAND | 0x0008,
    };

    let t = ctl(ClientOs::Mac, HostOs::Windows);
    t.key(command_down);
    assert_eq!(t.io.take(), [key(0xE0, true, Mods::LCTRL)]);

    let t = ctl_with(
        ClientOs::Mac,
        HostOs::Windows,
        ControllerSettings {
            mac_cmd_to_ctrl: false,
            ..ControllerSettings::default()
        },
    );
    t.key(command_down);
    assert_eq!(t.io.take(), [key(0xE3, true, Mods::LMETA)]);

    let left_ctrl = KeyInput::WinKey {
        scan: 0x1D,
        extended: false,
        vk: 0x11,
        down: true,
        repeat: false,
    };
    let t = ctl(ClientOs::Windows, HostOs::Mac);
    t.key(left_ctrl);
    assert_eq!(t.io.take(), [key(0xE0, true, Mods::LCTRL)]);

    let t = ctl_with(
        ClientOs::Windows,
        HostOs::Mac,
        ControllerSettings {
            win_swap_ctrl_meta: true,
            ..ControllerSettings::default()
        },
    );
    t.key(left_ctrl);
    assert_eq!(t.io.take(), [key(0xE3, true, Mods::LMETA)]);
}
