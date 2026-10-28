//! 会话控制器（07 · 电脑端）：开会话、建窗口、接视频、路由输入与状态。

use std::{
    collections::VecDeque,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc, Arc, Mutex, MutexGuard, PoisonError, Weak,
    },
    time::Duration,
};

use rs_mobile::rscore::{
    api_sessions,
    dto_core::{Codec, CoreEvent, OpenIntent, OpenOpts, Quality, SessionPhase},
    error::CoreError,
    runtime::Core,
    tables::{SessionTable, VideoConsumer},
};
use rsp_client::{SessionHandle, SessionStateKind};
use rsp_core::{hid::InputMsg, pb};
use tokio::time::MissedTickBehavior;

use crate::{
    bridge::{Bridge, CallError, EventForward},
    frames::{FrameAdapter, GateConfig, KeyframeReason, VideoControl, VideoEvents},
    keymap::MacKeyboardKind,
    keys::{KeyAction, KeyRouter, KeyRouterConfig, ModifierRemap},
    platform::{
        ClientOs, HostOs, KeyInput, MenuCommand, PointerCapture, PointerInput, ScrollInput,
        SessionMenu, SessionWindow, SessionWindowFactory, StreamInfo, VideoCodec, WindowChange,
        WindowCommand, WindowEvents, WindowSpec,
    },
    pointer::{PointerConfig, PointerOut, PointerState, Viewport},
    scroll::ScrollMapper,
    session::{CursorCache, FsmConfig, SessionCall, SessionEffect, SessionFsm, SessionInput},
    winset::{OpenDecision, WindowSet},
};

const WINDOW_WIDTH: u32 = 1280;
const WINDOW_HEIGHT: u32 = 800;
const ATTACH_TIMEOUT: Duration = Duration::from_secs(2);
const TICK: Duration = Duration::from_millis(8);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

/// 会话 I/O。生产实现 `CoreSessionIo`；测试用假实现。
pub trait SessionIo: Send + Sync + 'static {
    fn send_input(&self, msg: InputMsg);
    fn send_pointer(&self, x: f32, y: f32);
    fn call(&self, call: SessionCall);
    fn close(&self);
    fn request_keyframe(&self, reason: KeyframeReason);
    fn report_decoded(&self, decode_us: u64, latency_us: u64, queue: u32);
    fn now_us(&self) -> u64;
}

/// 经 rs-mobile 找会话 handle，只在 Streaming 时发送（同 `ffi_session::with_handle`）；
/// `call` 映射到 `api_sessions::{combo, action("ctrlAltDel"), set_quality, select_display, request_control}`，错误只记日志。
pub struct CoreSessionIo {
    core: Arc<Core>,
    session_id: u64,
}

impl CoreSessionIo {
    pub fn new(core: Arc<Core>, session_id: u64) -> Self {
        Self { core, session_id }
    }

    fn with_handle(&self, f: impl FnOnce(&SessionHandle)) {
        let Some(entry) = self.core.state::<SessionTable>().get(self.session_id) else {
            return;
        };
        if entry.video.is_closed() || entry.is_cancelled() {
            return;
        }
        if let Some(handle) = entry
            .handle()
            .filter(|handle| handle.state() == SessionStateKind::Streaming)
        {
            f(&handle);
        }
    }
}

impl SessionIo for CoreSessionIo {
    fn send_input(&self, msg: InputMsg) {
        self.with_handle(|handle| {
            if let Err(error) = handle.send_input(msg) {
                tracing::debug!(session_id = self.session_id, ?error, "input not sent");
            }
        });
    }

    fn send_pointer(&self, x: f32, y: f32) {
        self.with_handle(|handle| handle.send_pointer(x, y));
    }

    fn call(&self, call: SessionCall) {
        let (core, id) = (&self.core, self.session_id);
        let result = match &call {
            SessionCall::Combo(name) => api_sessions::combo(core, id, name),
            SessionCall::CtrlAltDel => api_sessions::action(core, id, "ctrlAltDel", None),
            SessionCall::SetQuality(quality) => api_sessions::set_quality(core, id, quality),
            SessionCall::SelectDisplay(display_id) => {
                api_sessions::select_display(core, id, *display_id)
            }
            SessionCall::RequestControl => api_sessions::request_control(core, id),
        };
        if let Err(error) = result {
            tracing::warn!(session_id = id, ?call, %error, "session call failed");
        }
    }

    fn close(&self) {
        api_sessions::close(&self.core, self.session_id);
    }

    fn request_keyframe(&self, reason: KeyframeReason) {
        self.with_handle(|handle| handle.request_keyframe_for(reason.to_pb()));
    }

    fn report_decoded(&self, decode_us: u64, latency_us: u64, queue: u32) {
        self.with_handle(|handle| handle.report_decoded(decode_us, latency_us, queue));
    }

    fn now_us(&self) -> u64 {
        self.core.now_us()
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct ControllerSettings {
    /// Mac 客户端连 Windows Host：⌘ → Ctrl。默认 true。
    pub mac_cmd_to_ctrl: bool,
    /// Windows 客户端连 Mac Host：Ctrl ↔ ⌘。默认 false。
    pub win_swap_ctrl_meta: bool,
    pub mac_keyboard: MacKeyboardKind,
}

impl Default for ControllerSettings {
    fn default() -> Self {
        Self {
            mac_cmd_to_ctrl: true,
            win_swap_ctrl_meta: false,
            mac_keyboard: MacKeyboardKind::Ansi,
        }
    }
}

fn modifier_remap(client: ClientOs, host: HostOs, settings: ControllerSettings) -> ModifierRemap {
    match (client, host) {
        (ClientOs::Mac, HostOs::Windows) if settings.mac_cmd_to_ctrl => ModifierRemap::CmdToCtrl,
        (ClientOs::Windows, HostOs::Mac) if settings.win_swap_ctrl_meta => {
            ModifierRemap::SwapCtrlMeta
        }
        _ => ModifierRemap::None,
    }
}

pub trait ControllerObserver: Send + Sync + 'static {
    fn menu_changed(&self, session_id: u64, menu: &SessionMenu);
    fn focus_changed(&self, session_id: u64, focused: bool);
    fn ended(&self, session_id: u64, host_id: &str, by_user: bool);
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct ControllerConfig {
    pub session_id: u64,
    pub host_id: String,
    pub host_name: String,
    pub host: HostOs,
    pub client: ClientOs,
    pub quality: String,
    pub settings: ControllerSettings,
}

struct FsmState {
    fsm: SessionFsm,
    /// 绑定窗口前收到的输入；绑定时在 `start()` 之后按序重放。
    pending: Option<Vec<SessionInput>>,
    /// 待执行的效果，按 fsm 产出的顺序执行；同一时刻只有一个线程在执行。
    queue: VecDeque<SessionEffect>,
    draining: bool,
}

#[derive(Default)]
struct VideoSlot {
    adapter: Option<Arc<FrameAdapter>>,
    /// 已执行 Ended：之后绑定的 adapter 立即关闭。
    closed: bool,
}

pub struct SessionController {
    cfg: ControllerConfig,
    io: Arc<dyn SessionIo>,
    observer: Arc<dyn ControllerObserver>,
    state: Mutex<FsmState>,
    keys: Mutex<KeyRouter>,
    pointer: Mutex<PointerState>,
    scroll: Mutex<ScrollMapper>,
    cursor: Mutex<CursorCache>,
    window: Mutex<Option<Arc<dyn SessionWindow>>>,
    video: Mutex<VideoSlot>,
    focused: AtomicBool,
}

impl SessionController {
    pub fn new(
        cfg: ControllerConfig,
        io: Arc<dyn SessionIo>,
        observer: Arc<dyn ControllerObserver>,
    ) -> Arc<Self> {
        let mut keys = KeyRouter::new(KeyRouterConfig {
            client: cfg.client,
            host: cfg.host,
            remap: modifier_remap(cfg.client, cfg.host, cfg.settings),
            mac_kind: cfg.settings.mac_keyboard,
        });
        // fsm 起始时不捕获键盘（未聚焦、未在 Streaming），路由器与它一致。
        keys.set_captured(false);
        let fsm = SessionFsm::new(FsmConfig {
            session_id: cfg.session_id,
            host_id: cfg.host_id.clone(),
            host_name: cfg.host_name.clone(),
            client: cfg.client,
            host: cfg.host,
            quality: cfg.quality.clone(),
        });
        Arc::new(Self {
            cfg,
            io,
            observer,
            state: Mutex::new(FsmState {
                fsm,
                pending: Some(Vec::new()),
                queue: VecDeque::new(),
                draining: false,
            }),
            keys: Mutex::new(keys),
            pointer: Mutex::new(PointerState::new(PointerConfig::default())),
            scroll: Mutex::new(ScrollMapper::new()),
            cursor: Mutex::new(CursorCache::new()),
            window: Mutex::new(None),
            video: Mutex::new(VideoSlot::default()),
            focused: AtomicBool::new(false),
        })
    }

    /// 绑定窗口并执行 fsm.start() 的效果。
    pub fn bind_window(&self, w: Arc<dyn SessionWindow>) {
        *lock(&self.window) = Some(w);
        let mut state = self.state();
        let mut effects = state.fsm.start();
        for input in state.pending.take().unwrap_or_default() {
            effects.extend(state.fsm.on(input));
        }
        self.run(state, effects);
    }

    pub fn bind_adapter(&self, a: Arc<FrameAdapter>) {
        let mut video = lock(&self.video);
        if video.closed {
            drop(video);
            a.close();
        } else {
            video.adapter = Some(a);
        }
    }

    pub fn on_core_event(&self, ev: &CoreEvent) {
        self.feed(SessionInput::Core(ev.clone()));
    }

    /// 指针 flush + 帧闸门 tick。
    pub fn tick(&self) {
        let now = self.io.now_us();
        let out = lock(&self.pointer).flush(now);
        self.pointer_out(out);
        let adapter = lock(&self.video).adapter.clone();
        if let Some(adapter) = adapter {
            adapter.tick();
        }
    }

    pub fn menu(&self) -> SessionMenu {
        self.state().fsm.menu()
    }

    pub fn session_id(&self) -> u64 {
        self.cfg.session_id
    }

    pub fn host_id(&self) -> &str {
        &self.cfg.host_id
    }

    pub fn is_ended(&self) -> bool {
        self.state().fsm.is_ended()
    }

    pub fn window(&self) -> Option<Arc<dyn SessionWindow>> {
        lock(&self.window).clone()
    }

    fn state(&self) -> MutexGuard<'_, FsmState> {
        lock(&self.state)
    }

    fn input_enabled(&self) -> bool {
        self.state().fsm.input_enabled()
    }

    fn feed(&self, input: SessionInput) {
        let mut state = self.state();
        if let Some(pending) = &mut state.pending {
            pending.push(input);
            return;
        }
        let effects = state.fsm.on(input);
        self.run(state, effects);
    }

    /// 执行效果时不持锁。执行中再产生的效果（和弦、平台窗口在 apply 里同步回调）排到队尾，
    /// 由正在执行的那一轮接着执行，所以窗口收到的命令与 fsm 的转移同序。
    fn run<'a>(&'a self, mut state: MutexGuard<'a, FsmState>, effects: Vec<SessionEffect>) {
        state.queue.extend(effects);
        if state.draining {
            return;
        }
        state.draining = true;
        while let Some(effect) = state.queue.pop_front() {
            drop(state);
            self.apply(effect);
            state = self.state();
        }
        state.draining = false;
    }

    fn apply(&self, effect: SessionEffect) {
        match effect {
            SessionEffect::Window(command) => {
                let menu = match &command {
                    WindowCommand::SetSessionMenu(menu) => Some(menu.clone()),
                    _ => None,
                };
                self.apply_window(command);
                if let Some(menu) = menu {
                    self.observer.menu_changed(self.cfg.session_id, &menu);
                }
            }
            SessionEffect::InputEnabled(enabled) => {
                tracing::debug!(session_id = self.cfg.session_id, enabled, "session input");
            }
            SessionEffect::KeyboardCapture(captured) => {
                let actions = lock(&self.keys).set_captured(captured);
                self.key_actions(actions);
            }
            SessionEffect::PointerCapture(capture) => {
                let out = lock(&self.pointer).set_capture(capture);
                self.pointer_out(out);
            }
            SessionEffect::ReleaseAll => {
                // 键盘的 Send(ReleaseAll) 与指针的合并成一条。
                let keys_held = !lock(&self.keys).release_all().is_empty();
                let buttons_held = lock(&self.pointer).release_all();
                if keys_held || buttons_held {
                    self.io.send_input(InputMsg::ReleaseAll);
                }
            }
            SessionEffect::Call(call) => self.io.call(call),
            SessionEffect::CloseSession => self.io.close(),
            SessionEffect::Ended { by_user, .. } => {
                let adapter = {
                    let mut video = lock(&self.video);
                    video.closed = true;
                    video.adapter.take()
                };
                if let Some(adapter) = adapter {
                    adapter.close();
                }
                self.observer
                    .ended(self.cfg.session_id, &self.cfg.host_id, by_user);
            }
        }
    }

    fn apply_window(&self, command: WindowCommand) {
        let window = lock(&self.window).clone();
        if let Some(window) = window {
            window.apply(command);
        }
    }

    fn key_actions(&self, actions: Vec<KeyAction>) {
        for action in actions {
            match action {
                KeyAction::Send(msg) => {
                    if self.input_enabled() {
                        self.io.send_input(msg);
                    }
                }
                KeyAction::ReleaseCapture => self.feed(SessionInput::CaptureReleasedByChord),
                KeyAction::ResumeCapture => self.feed(SessionInput::CaptureResumedByChord),
            }
        }
    }

    fn pointer_out(&self, out: Vec<PointerOut>) {
        for item in out {
            match item {
                PointerOut::Pointer { x, y } => {
                    if self.input_enabled() {
                        self.io.send_pointer(x, y);
                    }
                }
                PointerOut::Input(msg) => {
                    if self.input_enabled() {
                        self.io.send_input(msg);
                    }
                }
                PointerOut::VirtualCursor { x, y } => {
                    self.apply_window(WindowCommand::MoveVirtualCursor { x, y });
                }
            }
        }
    }

    fn in_video(&self, x: f64, y: f64) -> bool {
        let pointer = lock(&self.pointer);
        pointer.capture() == PointerCapture::Virtual
            || pointer.viewport().to_normalized(x, y).is_some()
    }
}

impl WindowEvents for SessionController {
    fn key(&self, ev: KeyInput) {
        let actions = lock(&self.keys).on_key(ev);
        self.key_actions(actions);
        // 输入关闭时和弦不得把路由器单方面切回「捕获」：以 fsm 为准同步一次。
        let captured = self.state().fsm.keyboard_captured();
        let actions = {
            let mut keys = lock(&self.keys);
            if keys.captured() == captured {
                Vec::new()
            } else {
                keys.set_captured(captured)
            }
        };
        self.key_actions(actions);
    }

    fn pointer(&self, ev: PointerInput) {
        let (enabled, captured) = {
            let state = self.state();
            (state.fsm.input_enabled(), state.fsm.keyboard_captured())
        };
        if !enabled {
            return;
        }
        if let PointerInput::Button {
            down: true, x, y, ..
        } = ev
        {
            if !captured && self.focused.load(Ordering::Acquire) && self.in_video(x, y) {
                self.feed(SessionInput::ClickedInVideo);
            }
        }
        let now = self.io.now_us();
        let out = lock(&self.pointer).on_input(ev, now);
        self.pointer_out(out);
    }

    fn scroll(&self, ev: ScrollInput) {
        if !self.input_enabled() {
            return;
        }
        let msg = lock(&self.scroll).map(ev);
        if let Some(msg) = msg {
            self.io.send_input(msg);
        }
    }

    fn window(&self, ev: WindowChange) {
        match ev {
            WindowChange::Resized { width, height, .. } => {
                let mut pointer = lock(&self.pointer);
                let viewport = Viewport {
                    view_w: width,
                    view_h: height,
                    ..pointer.viewport()
                };
                pointer.set_viewport(viewport);
            }
            WindowChange::Focus(focused) => {
                self.focused.store(focused, Ordering::Release);
                self.observer.focus_changed(self.cfg.session_id, focused);
            }
            _ => {}
        }
        self.feed(SessionInput::Window(ev));
    }
}

impl VideoEvents for SessionController {
    fn stream(&self, info: &StreamInfo) {
        let mut pointer = lock(&self.pointer);
        let viewport = Viewport {
            video_w: info.width,
            video_h: info.height,
            ..pointer.viewport()
        };
        pointer.set_viewport(viewport);
    }

    fn cursor_msg(&self, m: &pb::CursorMsg) {
        let commands = lock(&self.cursor).on_msg(m);
        for command in commands {
            self.apply_window(command);
        }
    }

    fn host_pointer(&self, x: f32, y: f32, visible: bool) {
        lock(&self.pointer).on_host_pointer(x, y);
        let commands = lock(&self.cursor).on_visible(visible);
        for command in commands {
            self.apply_window(command);
        }
        // 观看者（或输入暂停）时窗口画 Host 光标；Virtual 时虚拟光标跟本机位移走。
        let remote = {
            let state = self.state();
            state.fsm.phase() == SessionPhase::Streaming && !state.fsm.input_enabled()
        };
        if remote
            && x.is_finite()
            && y.is_finite()
            && lock(&self.pointer).capture() != PointerCapture::Virtual
        {
            self.apply_window(WindowCommand::MoveVirtualCursor {
                x: x.clamp(0.0, 1.0),
                y: y.clamp(0.0, 1.0),
            });
        }
    }

    /// 结束由 Core 事件驱动。
    fn closed(&self, _reason: &str) {}
}

/// FrameAdapter 的 VideoControl：转给 io。
impl VideoControl for SessionController {
    fn request_keyframe(&self, reason: KeyframeReason) {
        self.io.request_keyframe(reason);
    }

    fn report_decoded(&self, decode_us: u64, latency_us: u64, queue: u32) {
        self.io.report_decoded(decode_us, latency_us, queue);
    }

    fn now_us(&self) -> u64 {
        self.io.now_us()
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct OpenRequest {
    pub host_id: String,
    pub host_name: String,
    /// 已配对主机的系统；不知道时 Unknown。
    pub host: HostOs,
    /// false = 只看。
    pub control: bool,
    pub display_id: Option<u32>,
    pub quality: Option<String>,
}

/// 窗口表兼事件路由表：`open` 在同一把锁里开会话并登记控制器，之后的会话事件都能找到它。
struct Shared {
    windows: Mutex<WindowSet<Arc<SessionController>>>,
    observer: Arc<dyn ControllerObserver>,
}

impl Shared {
    fn windows(&self) -> MutexGuard<'_, WindowSet<Arc<SessionController>>> {
        lock(&self.windows)
    }

    /// 已结束的会话（窗口显示结束文字）在用户关窗时移出。
    fn route_window(&self, controller: &SessionController, ev: WindowChange) {
        let closing = matches!(
            ev,
            WindowChange::CloseRequested | WindowChange::Menu(MenuCommand::Disconnect)
        );
        let ended = controller.is_ended();
        WindowEvents::window(controller, ev);
        if closing && ended {
            let removed = self.windows().remove_session(controller.session_id());
            drop(removed);
        }
    }
}

impl EventForward for Shared {
    fn raw(&self, _seq: u64, _payload: &str) {}

    fn typed(&self, event: &CoreEvent) {
        let session_id = match event {
            CoreEvent::SessionState { session_id, .. }
            | CoreEvent::SessionRole { session_id, .. }
            | CoreEvent::SessionError { session_id, .. }
            | CoreEvent::SessionDisplays { session_id, .. } => session_id,
            _ => return,
        };
        let Ok(session_id) = session_id.parse::<u64>() else {
            return;
        };
        // 主窗口直接开的会话、已移出的会话：没有控制器，忽略。
        let controller = self.windows().get_by_session(session_id);
        if let Some(controller) = controller {
            controller.on_core_event(event);
        }
    }
}

/// 控制器的观察者：先维护窗口表（聚焦、用户结束时移出），再转给外壳。
struct Relay(Weak<Shared>);

impl ControllerObserver for Relay {
    fn menu_changed(&self, session_id: u64, menu: &SessionMenu) {
        if let Some(shared) = self.0.upgrade() {
            shared.observer.menu_changed(session_id, menu);
        }
    }

    fn focus_changed(&self, session_id: u64, focused: bool) {
        if let Some(shared) = self.0.upgrade() {
            shared.windows().set_focused(session_id, focused);
            shared.observer.focus_changed(session_id, focused);
        }
    }

    fn ended(&self, session_id: u64, host_id: &str, by_user: bool) {
        if let Some(shared) = self.0.upgrade() {
            if by_user {
                let removed = shared.windows().remove_session(session_id);
                drop(removed);
            }
            shared.observer.ended(session_id, host_id, by_user);
        }
    }
}

/// 交给平台窗口的事件入口。
struct WindowRoute {
    controller: Arc<SessionController>,
    shared: Weak<Shared>,
}

impl WindowEvents for WindowRoute {
    fn key(&self, ev: KeyInput) {
        self.controller.key(ev);
    }

    fn pointer(&self, ev: PointerInput) {
        self.controller.pointer(ev);
    }

    fn scroll(&self, ev: ScrollInput) {
        self.controller.scroll(ev);
    }

    fn window(&self, ev: WindowChange) {
        match self.shared.upgrade() {
            Some(shared) => shared.route_window(&self.controller, ev),
            None => WindowEvents::window(&*self.controller, ev),
        }
    }
}

pub struct DesktopSessions {
    bridge: Arc<Bridge>,
    factory: Arc<dyn SessionWindowFactory>,
    settings: Mutex<ControllerSettings>,
    shared: Arc<Shared>,
    subscription: u64,
}

impl DesktopSessions {
    /// 订阅 bridge 的 typed 事件，按 session_id 分给控制器。
    pub fn new(
        bridge: Arc<Bridge>,
        factory: Arc<dyn SessionWindowFactory>,
        settings: ControllerSettings,
        observer: Arc<dyn ControllerObserver>,
    ) -> Arc<Self> {
        let shared = Arc::new(Shared {
            windows: Mutex::new(WindowSet::new()),
            observer,
        });
        let subscription = bridge.subscribe(shared.clone());
        Arc::new(Self {
            bridge,
            factory,
            settings: Mutex::new(settings),
            shared,
            subscription,
        })
    }

    /// 返回会话 id（十进制字符串，与 RsCore 的 SessionId 相同）。
    ///
    /// 会阻塞（建窗口、等视频接上），不要在主线程调用。
    pub fn open(&self, req: OpenRequest) -> Result<String, CallError> {
        if let Some(id) = self.reuse(&req.host_id) {
            return Ok(id.to_string());
        }
        let codec = if self.factory.can_decode(VideoCodec::Hevc) {
            Codec::Hevc
        } else if self.factory.can_decode(VideoCodec::H264) {
            Codec::H264
        } else {
            return Err(CallError::unsupported("本机没有可用的硬件解码器"));
        };
        let max_width = self.factory.screen_pixel_width().clamp(640, 3840);
        let quality = req
            .quality
            .as_deref()
            .map(api_sessions::parse_enum::<Quality>)
            .transpose()?;
        let opts = OpenOpts {
            intent: if req.control {
                OpenIntent::Control
            } else {
                OpenIntent::View
            },
            display_id: req.display_id,
            quality,
            mode: None,
            audio: None,
            max_fps: None,
            max_width: Some(max_width),
            codec: Some(codec),
        };
        let core = self.bridge.core();
        let controller = {
            let mut windows = self.shared.windows();
            if let OpenDecision::FocusExisting(id) = windows.decide(&req.host_id) {
                // 并发的另一次 open 刚登记了这台 Host。
                return Ok(id.to_string());
            }
            let id = api_sessions::open(&core, &req.host_id, opts)?;
            let controller = SessionController::new(
                ControllerConfig {
                    session_id: id,
                    host_id: req.host_id.clone(),
                    host_name: req.host_name.clone(),
                    host: req.host,
                    client: ClientOs::current(),
                    quality: req.quality.clone().unwrap_or_else(|| "auto".to_owned()),
                    settings: *lock(&self.settings),
                },
                Arc::new(CoreSessionIo::new(core.clone(), id)),
                Arc::new(Relay(Arc::downgrade(&self.shared))),
            );
            // decide 刚在同一把锁里确认过没有这台 Host。
            let _ = windows.insert(&req.host_id, id, controller.clone());
            controller
        };
        let id = controller.session_id();

        let events = Arc::new(WindowRoute {
            controller: controller.clone(),
            shared: Arc::downgrade(&self.shared),
        });
        let spec = WindowSpec {
            session_id: id,
            host_id: req.host_id.clone(),
            title: req.host_name.clone(),
            width: WINDOW_WIDTH,
            height: WINDOW_HEIGHT,
        };
        let window = match self.factory.create(&spec, events) {
            Ok(window) => window,
            Err(error) => {
                let removed = self.shared.windows().remove_session(id);
                drop(removed);
                api_sessions::close(&core, id);
                return Err(CoreError::internal(format!("create session window: {error}")).into());
            }
        };
        controller.bind_window(window.clone());

        let adapter = FrameAdapter::new(
            window.frame_sink(),
            controller.clone(),
            controller.clone(),
            GateConfig::default(),
        );
        if !attach_video(&core, id, adapter.clone()) {
            // 会话已结束：结束事件会来。
            tracing::debug!(session_id = id, "session video not attached");
        }
        controller.bind_adapter(adapter);
        spawn_ticker(&core, &controller);
        Ok(id.to_string())
    }

    pub fn menu_command(&self, cmd: MenuCommand) {
        let focused = self.shared.windows().focused();
        if let Some((_, controller)) = focused {
            self.shared
                .route_window(&controller, WindowChange::Menu(cmd));
        }
    }

    pub fn focused_menu(&self) -> Option<(u64, SessionMenu)> {
        let (id, controller) = self.shared.windows().focused()?;
        Some((id, controller.menu()))
    }

    /// 之后打开的会话使用新设置。
    pub fn set_settings(&self, s: ControllerSettings) {
        *lock(&self.settings) = s;
    }

    /// 关闭所有会话与窗口（退出时）。
    pub fn close_all(&self) {
        let controllers: Vec<_> = {
            let windows = self.shared.windows();
            windows
                .sessions()
                .into_iter()
                .filter_map(|id| windows.get_by_session(id))
                .collect()
        };
        for controller in controllers {
            self.shared
                .route_window(&controller, WindowChange::CloseRequested);
        }
    }

    pub fn sessions(&self) -> Vec<u64> {
        self.shared.windows().sessions()
    }

    /// 同一 Host 已有窗口：没结束就聚焦并返回它；已结束就关掉移出，返回 None 让调用方新开。
    /// 窗口命令在锁外发：平台可能在 apply 里同步回调。
    fn reuse(&self, host_id: &str) -> Option<u64> {
        let existing = {
            let windows = self.shared.windows();
            match windows.decide(host_id) {
                OpenDecision::FocusExisting(id) => windows.get_by_session(id),
                OpenDecision::OpenNew => None,
            }
        }?;
        let window = existing.window();
        if !existing.is_ended() {
            if let Some(window) = window {
                window.apply(WindowCommand::Focus);
            }
            return Some(existing.session_id());
        }
        if let Some(window) = window {
            window.apply(WindowCommand::Close);
        }
        let removed = self.shared.windows().remove_session(existing.session_id());
        drop(removed);
        None
    }
}

impl Drop for DesktopSessions {
    fn drop(&mut self) {
        self.bridge.unsubscribe(self.subscription);
    }
}

/// `VideoTap::attach` 同步回放，要在库的工作线程上调用；最多等 2 s。false = 会话已结束。
fn attach_video(core: &Arc<Core>, session_id: u64, adapter: Arc<FrameAdapter>) -> bool {
    let Ok(entry) = api_sessions::session_entry(core, session_id) else {
        return false;
    };
    let (tx, rx) = mpsc::sync_channel(1);
    core.handle().spawn_blocking(move || {
        let consumer: Arc<dyn VideoConsumer> = adapter;
        let _ = tx.send(entry.video.attach(consumer));
    });
    rx.recv_timeout(ATTACH_TIMEOUT).unwrap_or(false)
}

/// 每 8 ms 调一次 `tick()`；控制器结束或被释放后退出。
fn spawn_ticker(core: &Core, controller: &Arc<SessionController>) {
    let controller = Arc::downgrade(controller);
    core.spawn(async move {
        let mut interval = tokio::time::interval(TICK);
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            match controller.upgrade() {
                Some(controller) if !controller.is_ended() => controller.tick(),
                _ => break,
            }
        }
    });
}
