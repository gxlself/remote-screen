//! 会话窗口状态机（07 · 电脑端）。

use std::collections::VecDeque;

use rs_mobile::rscore::dto_core::{CoreEvent, GoodbyeReason, Role, SessionPhase};
use rsp_core::pb;

use crate::platform::{
    ClientOs, CursorImage, HostOs, MenuCommand, PointerCapture, SessionMenu, WindowChange,
    WindowCommand,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FsmConfig {
    pub session_id: u64,
    pub host_id: String,
    pub host_name: String,
    pub client: ClientOs,
    pub host: HostOs,
    /// OpenOpts.quality，省略时 "auto"。
    pub quality: String,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionInput {
    /// session_id 不是本会话的事件忽略。
    Core(CoreEvent),
    Window(WindowChange),
    CaptureReleasedByChord,
    CaptureResumedByChord,
    /// 键盘已释放时在画面上点击 → 恢复捕获。
    ClickedInVideo,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SessionCall {
    Combo(String),
    CtrlAltDel,
    SetQuality(String),
    SelectDisplay(u32),
    RequestControl,
}

#[derive(Debug, Clone, PartialEq)]
pub enum SessionEffect {
    Window(WindowCommand),
    InputEnabled(bool),
    /// 交给 `KeyRouter::set_captured`。
    KeyboardCapture(bool),
    /// 交给 `PointerState::set_capture`。
    PointerCapture(PointerCapture),
    /// 键盘与指针各自 release_all；有按住的就发一条 `InputMsg::ReleaseAll`。
    ReleaseAll,
    Call(SessionCall),
    /// 调 `api_sessions::close`。
    CloseSession,
    /// 只发一次。`by_user`：用户关窗或菜单断开导致的结束。
    Ended {
        by_user: bool,
        code: Option<u32>,
        reason: Option<GoodbyeReason>,
        message: Option<String>,
    },
}

#[derive(Default)]
struct LastError {
    code: Option<u32>,
    reason: Option<GoodbyeReason>,
    message: Option<String>,
}

struct Derived {
    input_enabled: bool,
    keyboard_captured: bool,
    system_grab: bool,
    pointer_capture: PointerCapture,
    remote_cursor: bool,
    title: String,
    overlay: Option<String>,
    menu: SessionMenu,
}

/// 一个会话窗口的纯逻辑状态；所有副作用均交由调用者执行。
pub struct SessionFsm {
    cfg: FsmConfig,
    phase: SessionPhase,
    role: Option<Role>,
    paused_local: bool,
    focused: bool,
    fullscreen: bool,
    minimized: bool,
    released: bool,
    pointer_lock: bool,
    displays: Vec<(u32, String)>,
    current_display: Option<u32>,
    quality: String,
    last_error: LastError,
    closing_by_user: bool,
    ended: bool,
    started: bool,
}

impl SessionFsm {
    pub fn new(cfg: FsmConfig) -> Self {
        let quality = if cfg.quality.is_empty() {
            "auto".to_owned()
        } else {
            cfg.quality.clone()
        };
        Self {
            cfg,
            phase: SessionPhase::Connecting,
            role: None,
            paused_local: false,
            focused: false,
            fullscreen: false,
            minimized: false,
            released: false,
            pointer_lock: false,
            displays: Vec::new(),
            current_display: None,
            quality,
            last_error: LastError::default(),
            closing_by_user: false,
            ended: false,
            started: false,
        }
    }

    /// 初始效果：标题、叠加文字、InputEnabled(false)、菜单。重复调用不再输出。
    pub fn start(&mut self) -> Vec<SessionEffect> {
        if self.started || self.ended || self.closing_by_user {
            return Vec::new();
        }
        self.started = true;
        let state = self.derived();
        vec![
            SessionEffect::Window(WindowCommand::SetTitle(state.title)),
            SessionEffect::Window(WindowCommand::SetOverlayText(state.overlay)),
            SessionEffect::InputEnabled(state.input_enabled),
            SessionEffect::Window(WindowCommand::SetSessionMenu(state.menu)),
        ]
    }

    pub fn on(&mut self, input: SessionInput) -> Vec<SessionEffect> {
        if self.ended {
            return if is_close_input(&input) {
                vec![SessionEffect::Window(WindowCommand::Close)]
            } else {
                Vec::new()
            };
        }

        let before = self.derived();
        let was_focused = self.focused;
        let was_minimized = self.minimized;
        let mut effects = Vec::new();
        match input {
            SessionInput::Core(event) => {
                if !self.on_core(event) {
                    return effects;
                }
            }
            SessionInput::Window(WindowChange::Focus(focused)) => self.focused = focused,
            SessionInput::Window(WindowChange::Fullscreen(fullscreen)) => {
                self.fullscreen = fullscreen;
            }
            SessionInput::Window(WindowChange::Minimized(minimized)) => self.minimized = minimized,
            SessionInput::Window(WindowChange::Resized { .. }) => return effects,
            SessionInput::Window(WindowChange::CloseRequested)
            | SessionInput::Window(WindowChange::Menu(MenuCommand::Disconnect)) => {
                self.closing_by_user = true;
                // Stop input immediately. Wait for Core's Ended event before
                // emitting Ended, so an intervening SessionError is retained.
                self.phase = SessionPhase::Ended;
                effects.push(SessionEffect::CloseSession);
                effects.push(SessionEffect::Window(WindowCommand::Close));
            }
            SessionInput::Window(WindowChange::Menu(command)) => {
                self.on_menu(command, &mut effects);
            }
            SessionInput::CaptureReleasedByChord => self.released = true,
            SessionInput::CaptureResumedByChord | SessionInput::ClickedInVideo => {
                self.released = false;
            }
        }

        let after = self.derived();
        self.append_changes(
            before,
            after,
            (was_focused && !self.focused) || (!was_minimized && self.minimized),
            &mut effects,
        );
        if self.ended {
            effects.push(SessionEffect::Ended {
                by_user: self.closing_by_user,
                code: self.last_error.code,
                reason: self.last_error.reason,
                message: self.last_error.message.clone(),
            });
        }
        effects
    }

    /// Returns false for events that cannot affect this window.
    fn on_core(&mut self, event: CoreEvent) -> bool {
        match event {
            CoreEvent::SessionState {
                session_id, phase, ..
            } if self.owns_session(&session_id) => {
                if self.closing_by_user && phase != SessionPhase::Ended {
                    return false;
                }
                self.phase = phase;
                self.ended = phase == SessionPhase::Ended;
            }
            CoreEvent::SessionRole {
                session_id,
                role,
                input_paused_local,
                ..
            } if self.owns_session(&session_id) => {
                self.role = Some(role);
                self.paused_local = input_paused_local;
            }
            CoreEvent::SessionError {
                session_id,
                code,
                reason,
                message,
                ..
            } if self.owns_session(&session_id) => {
                self.last_error = LastError {
                    code,
                    reason,
                    message,
                };
            }
            CoreEvent::SessionDisplays {
                session_id,
                displays,
                current_id,
            } if self.owns_session(&session_id) => {
                self.displays = displays.into_iter().map(|d| (d.id, d.name)).collect();
                self.current_display = Some(current_id);
            }
            _ => return false,
        }
        true
    }

    fn owns_session(&self, session_id: &str) -> bool {
        session_id == self.cfg.session_id.to_string()
    }

    fn on_menu(&mut self, command: MenuCommand, effects: &mut Vec<SessionEffect>) {
        let call = match command {
            MenuCommand::SendCombo(name)
                if self.input_enabled() && self.menu().combos.contains(&name) =>
            {
                Some(SessionCall::Combo(name))
            }
            MenuCommand::CtrlAltDel if self.input_enabled() && self.cfg.host == HostOs::Windows => {
                Some(SessionCall::CtrlAltDel)
            }
            MenuCommand::SetQuality(quality)
                if matches!(quality.as_str(), "auto" | "clear" | "smooth" | "eco") =>
            {
                self.quality = quality.clone();
                Some(SessionCall::SetQuality(quality))
            }
            MenuCommand::SelectDisplay(id)
                if self
                    .displays
                    .iter()
                    .any(|(display_id, _)| *display_id == id)
                    && self.current_display != Some(id) =>
            {
                Some(SessionCall::SelectDisplay(id))
            }
            MenuCommand::RequestControl if self.role == Some(Role::Viewer) => {
                Some(SessionCall::RequestControl)
            }
            MenuCommand::ToggleFullscreen => {
                effects.push(SessionEffect::Window(WindowCommand::SetFullscreen(
                    !self.fullscreen,
                )));
                None
            }
            MenuCommand::ToggleKeyboardCapture => {
                self.released = !self.released;
                None
            }
            MenuCommand::TogglePointerLock => {
                self.pointer_lock = !self.pointer_lock;
                None
            }
            _ => None,
        };
        if let Some(call) = call {
            effects.push(SessionEffect::Call(call));
        }
    }

    fn derived(&self) -> Derived {
        let input_enabled = self.input_enabled();
        let keyboard_captured = self.keyboard_captured();
        let pointer_capture = self.pointer_capture();
        let (title, overlay) = self.text();
        Derived {
            input_enabled,
            keyboard_captured,
            system_grab: keyboard_captured && self.fullscreen,
            pointer_capture,
            remote_cursor: pointer_capture == PointerCapture::Virtual
                || (self.phase == SessionPhase::Streaming && !input_enabled),
            title,
            overlay,
            menu: self.menu(),
        }
    }

    fn append_changes(
        &self,
        before: Derived,
        after: Derived,
        focus_lost_or_minimized: bool,
        effects: &mut Vec<SessionEffect>,
    ) {
        if before.input_enabled && (!after.input_enabled || focus_lost_or_minimized) {
            effects.push(SessionEffect::ReleaseAll);
        }
        if before.input_enabled != after.input_enabled {
            effects.push(SessionEffect::InputEnabled(after.input_enabled));
        }
        if before.keyboard_captured != after.keyboard_captured {
            effects.push(SessionEffect::KeyboardCapture(after.keyboard_captured));
            effects.push(SessionEffect::Window(WindowCommand::SetKeyboardCapture(
                after.keyboard_captured,
            )));
        }
        if self.cfg.client == ClientOs::Windows && before.system_grab != after.system_grab {
            effects.push(SessionEffect::Window(WindowCommand::SetSystemKeyGrab(
                after.system_grab,
            )));
        }
        if before.pointer_capture != after.pointer_capture {
            effects.push(SessionEffect::PointerCapture(after.pointer_capture));
            effects.push(SessionEffect::Window(WindowCommand::SetPointerCapture(
                after.pointer_capture,
            )));
        }
        if before.remote_cursor != after.remote_cursor {
            effects.push(SessionEffect::Window(WindowCommand::ShowRemoteCursor(
                after.remote_cursor,
            )));
        }
        if !(self.phase == SessionPhase::Ended && self.closing_by_user) {
            if before.title != after.title {
                effects.push(SessionEffect::Window(WindowCommand::SetTitle(after.title)));
            }
            if before.overlay != after.overlay {
                effects.push(SessionEffect::Window(WindowCommand::SetOverlayText(
                    after.overlay,
                )));
            }
        }
        if before.menu != after.menu {
            effects.push(SessionEffect::Window(WindowCommand::SetSessionMenu(
                after.menu,
            )));
        }
    }

    fn text(&self) -> (String, Option<String>) {
        let name = &self.cfg.host_name;
        let (title, overlay) = match self.phase {
            SessionPhase::Connecting | SessionPhase::Authenticating => {
                (format!("{name} — 正在连接…"), Some("正在连接…"))
            }
            SessionPhase::PendingApproval => (
                format!("{name} — 等待对方同意"),
                Some("等待对方在电脑上同意连接…"),
            ),
            SessionPhase::Streaming => match self.role {
                Some(Role::Controller) if self.released => {
                    let chord = match self.cfg.client {
                        ClientOs::Mac => "⌃⌥",
                        ClientOs::Windows => "Ctrl+Alt",
                    };
                    (format!("{name} — 键盘已释放（按 {chord} 恢复）"), None)
                }
                Some(Role::Controller) if self.paused_local => {
                    (name.clone(), Some("对方正在使用这台电脑，输入已暂停"))
                }
                Some(Role::Viewer) => (format!("{name}（仅观看）"), None),
                _ => (name.clone(), None),
            },
            SessionPhase::Reconnecting => {
                (format!("{name} — 正在重连…"), Some("连接中断，正在重连…"))
            }
            SessionPhase::Ended => (
                format!("{name} — 已断开"),
                Some(self.last_error.message.as_deref().unwrap_or("连接已结束")),
            ),
        };
        (title, overlay.map(str::to_owned))
    }

    pub fn phase(&self) -> SessionPhase {
        self.phase
    }

    pub fn input_enabled(&self) -> bool {
        self.phase == SessionPhase::Streaming
            && self.role == Some(Role::Controller)
            && !self.paused_local
            && !self.ended
    }

    pub fn keyboard_captured(&self) -> bool {
        self.input_enabled() && self.focused && !self.released
    }

    pub fn pointer_capture(&self) -> PointerCapture {
        if !self.input_enabled() || !self.focused {
            return PointerCapture::Free;
        }
        match self.cfg.client {
            ClientOs::Mac if self.pointer_lock => PointerCapture::Virtual,
            ClientOs::Windows if self.fullscreen || self.pointer_lock => PointerCapture::Confine,
            _ => PointerCapture::Free,
        }
    }

    pub fn menu(&self) -> SessionMenu {
        let combos = match self.cfg.host {
            HostOs::Mac => &["cmdTab", "cmdSpace", "missionControl"][..],
            HostOs::Windows => &["altTab", "winKey"][..],
            HostOs::Unknown => &["cmdTab", "cmdSpace", "missionControl", "altTab", "winKey"][..],
        };
        SessionMenu {
            displays: self.displays.clone(),
            current_display: self.current_display,
            quality: self.quality.clone(),
            viewer: self.role == Some(Role::Viewer),
            fullscreen: self.fullscreen,
            keyboard_captured: self.keyboard_captured(),
            pointer_locked: self.pointer_lock,
            combos: combos.iter().map(|combo| (*combo).to_owned()).collect(),
            ctrl_alt_del: self.cfg.host == HostOs::Windows,
        }
    }

    pub fn is_ended(&self) -> bool {
        self.ended
    }
}

fn is_close_input(input: &SessionInput) -> bool {
    matches!(
        input,
        SessionInput::Window(WindowChange::CloseRequested)
            | SessionInput::Window(WindowChange::Menu(MenuCommand::Disconnect))
    )
}

/// 最多缓存 64 个光标形状，队首为最久未使用的形状。
#[derive(Default)]
pub struct CursorCache {
    shapes: VecDeque<CursorImage>,
    current_id: Option<u64>,
    hidden: Option<bool>,
    warned_missing: bool,
}

impl CursorCache {
    pub fn new() -> Self {
        Self::default()
    }

    /// Shape / Use 选中形状；缺失时恢复系统箭头；Hidden 独立更新可见性。
    pub fn on_msg(&mut self, message: &pb::CursorMsg) -> Vec<WindowCommand> {
        match &message.msg {
            Some(pb::cursor_msg::Msg::Shape(shape)) => {
                if let Some(index) = self.shapes.iter().position(|image| image.id == shape.id) {
                    self.shapes.remove(index);
                }
                let image = CursorImage {
                    id: shape.id,
                    width: shape.width,
                    height: shape.height,
                    hot_x: shape.hot_x,
                    hot_y: shape.hot_y,
                    scale: shape.scale,
                    png: shape.png.clone(),
                };
                self.shapes.push_back(image.clone());
                if self.shapes.len() > 64 {
                    self.shapes.pop_front();
                }
                self.current_id = Some(image.id);
                vec![WindowCommand::SetCursorImage(Some(image))]
            }
            Some(pb::cursor_msg::Msg::Use(id)) => {
                let image = self
                    .shapes
                    .iter()
                    .position(|image| image.id == *id)
                    .and_then(|index| self.shapes.remove(index));
                if let Some(image) = &image {
                    self.shapes.push_back(image.clone());
                } else if !self.warned_missing {
                    tracing::warn!(cursor_id = *id, "cursor shape cache miss");
                    self.warned_missing = true;
                }
                self.current_id = image.as_ref().map(|image| image.id);
                vec![WindowCommand::SetCursorImage(image)]
            }
            Some(pb::cursor_msg::Msg::Hidden(hidden)) => self.on_visible(!hidden),
            None => Vec::new(),
        }
    }

    /// `CursorEvent.visible` / `StreamConfig.cursor_visible`：与 Hidden 共用去重状态。
    pub fn on_visible(&mut self, visible: bool) -> Vec<WindowCommand> {
        let hidden = !visible;
        if self.hidden == Some(hidden) {
            Vec::new()
        } else {
            self.hidden = Some(hidden);
            vec![WindowCommand::SetCursorHidden(hidden)]
        }
    }

    pub fn current(&self) -> Option<&CursorImage> {
        self.current_id
            .and_then(|id| self.shapes.iter().find(|image| image.id == id))
    }
}
