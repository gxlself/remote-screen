use rs_desktop::platform::{
    ClientOs, CursorImage, HostOs, MenuCommand, PointerCapture, WindowChange, WindowCommand,
};
use rs_desktop::session::{
    CursorCache, FsmConfig, SessionCall, SessionEffect, SessionFsm, SessionInput,
};
use rs_mobile::rscore::dto_core::{CoreEvent, GoodbyeReason, Role, SessionDisplay, SessionPhase};
use rsp_core::pb;

fn config(client: ClientOs, host: HostOs) -> FsmConfig {
    FsmConfig {
        session_id: 7,
        host_id: "host-1".into(),
        host_name: "客厅 Mac".into(),
        client,
        host,
        quality: "auto".into(),
    }
}

fn fsm(client: ClientOs, host: HostOs) -> SessionFsm {
    let mut fsm = SessionFsm::new(config(client, host));
    fsm.start();
    fsm
}

fn phase(phase: SessionPhase) -> SessionInput {
    SessionInput::Core(CoreEvent::SessionState {
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
    })
}

fn role(role: Role, paused: bool) -> SessionInput {
    SessionInput::Core(CoreEvent::SessionRole {
        session_id: "7".into(),
        role,
        controller: None,
        viewers: Vec::new(),
        input_paused_local: paused,
    })
}

fn error(code: Option<u32>, reason: Option<GoodbyeReason>, message: Option<&str>) -> SessionInput {
    SessionInput::Core(CoreEvent::SessionError {
        session_id: "7".into(),
        host_id: "host-1".into(),
        code,
        reason,
        message: message.map(str::to_owned),
        detail: None,
        retry_after_s: None,
    })
}

fn streaming(client: ClientOs, host: HostOs, session_role: Role) -> SessionFsm {
    let mut fsm = fsm(client, host);
    fsm.on(SessionInput::Window(WindowChange::Focus(true)));
    fsm.on(role(session_role, false));
    fsm.on(phase(SessionPhase::Streaming));
    fsm
}

fn menu(command: MenuCommand) -> SessionInput {
    SessionInput::Window(WindowChange::Menu(command))
}

fn w(command: WindowCommand) -> SessionEffect {
    SessionEffect::Window(command)
}

fn title(text: &str) -> SessionEffect {
    w(WindowCommand::SetTitle(text.into()))
}

fn overlay(text: Option<&str>) -> SessionEffect {
    w(WindowCommand::SetOverlayText(text.map(str::to_owned)))
}

fn has(effects: &[SessionEffect], expected: SessionEffect) {
    assert!(
        effects.contains(&expected),
        "missing {expected:?} in {effects:?}"
    );
}

fn cmds(effects: &[SessionEffect]) -> Vec<&WindowCommand> {
    effects
        .iter()
        .filter_map(|effect| match effect {
            SessionEffect::Window(command) => Some(command),
            _ => None,
        })
        .collect()
}

#[test]
fn start_effects() {
    let mut fsm = SessionFsm::new(config(ClientOs::Mac, HostOs::Mac));
    assert_eq!(fsm.phase(), SessionPhase::Connecting);
    assert!(!fsm.input_enabled());
    assert!(!fsm.keyboard_captured());
    assert_eq!(fsm.pointer_capture(), PointerCapture::Free);
    assert!(!fsm.is_ended());
    let effects = fsm.start();
    assert_eq!(
        effects,
        [
            title("客厅 Mac — 正在连接…"),
            overlay(Some("正在连接…")),
            SessionEffect::InputEnabled(false),
            w(WindowCommand::SetSessionMenu(fsm.menu())),
        ]
    );
    assert_eq!(fsm.menu().combos, ["cmdTab", "cmdSpace", "missionControl"]);
    assert!(fsm.start().is_empty());
}

#[test]
fn streaming_controller_enables_input() {
    let mut fsm = fsm(ClientOs::Mac, HostOs::Mac);
    assert!(fsm
        .on(SessionInput::Window(WindowChange::Focus(true)))
        .is_empty());
    assert!(fsm.on(role(Role::Controller, false)).is_empty());
    let effects = fsm.on(phase(SessionPhase::Streaming));
    assert_eq!(
        effects,
        [
            SessionEffect::InputEnabled(true),
            SessionEffect::KeyboardCapture(true),
            w(WindowCommand::SetKeyboardCapture(true)),
            title("客厅 Mac"),
            overlay(None),
            w(WindowCommand::SetSessionMenu(fsm.menu())),
        ]
    );
    assert!(fsm.input_enabled());
    assert!(fsm.keyboard_captured());
    assert_eq!(fsm.phase(), SessionPhase::Streaming);
}

#[test]
fn viewer_no_input() {
    let mut fsm = fsm(ClientOs::Mac, HostOs::Mac);
    fsm.on(role(Role::Viewer, false));
    let effects = fsm.on(phase(SessionPhase::Streaming));
    assert!(!effects.contains(&SessionEffect::InputEnabled(true)));
    has(&effects, w(WindowCommand::ShowRemoteCursor(true)));
    has(&effects, title("客厅 Mac（仅观看）"));
    assert!(!fsm.input_enabled());
    assert!(fsm.menu().viewer);
}

#[test]
fn paused_local() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    let effects = fsm.on(role(Role::Controller, true));
    assert_eq!(
        effects,
        [
            SessionEffect::ReleaseAll,
            SessionEffect::InputEnabled(false),
            SessionEffect::KeyboardCapture(false),
            w(WindowCommand::SetKeyboardCapture(false)),
            w(WindowCommand::ShowRemoteCursor(true)),
            overlay(Some("对方正在使用这台电脑，输入已暂停")),
            w(WindowCommand::SetSessionMenu(fsm.menu())),
        ]
    );
    let resumed = fsm.on(role(Role::Controller, false));
    has(&resumed, SessionEffect::InputEnabled(true));
    has(&resumed, overlay(None));
}

#[test]
fn reconnecting_releases() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    let effects = fsm.on(phase(SessionPhase::Reconnecting));
    assert_eq!(effects.first(), Some(&SessionEffect::ReleaseAll));
    has(&effects, title("客厅 Mac — 正在重连…"));
    has(&effects, overlay(Some("连接中断，正在重连…")));
    let resumed = fsm.on(phase(SessionPhase::Streaming));
    has(&resumed, SessionEffect::InputEnabled(true));
    has(&resumed, SessionEffect::KeyboardCapture(true));
}

#[test]
fn focus_loss_releases() {
    for (client, capture) in [
        (ClientOs::Mac, PointerCapture::Virtual),
        (ClientOs::Windows, PointerCapture::Confine),
    ] {
        let mut fsm = streaming(client, HostOs::Mac, Role::Controller);
        fsm.on(menu(MenuCommand::TogglePointerLock));
        assert_eq!(fsm.pointer_capture(), capture);
        let effects = fsm.on(SessionInput::Window(WindowChange::Focus(false)));
        assert_eq!(effects.first(), Some(&SessionEffect::ReleaseAll));
        has(&effects, SessionEffect::KeyboardCapture(false));
        has(
            &effects,
            SessionEffect::PointerCapture(PointerCapture::Free),
        );
        assert!(fsm.input_enabled());
        assert!(!fsm.keyboard_captured());
        assert!(fsm
            .on(SessionInput::Window(WindowChange::Focus(false)))
            .is_empty());
        let resumed = fsm.on(SessionInput::Window(WindowChange::Focus(true)));
        has(&resumed, SessionEffect::KeyboardCapture(true));
        has(&resumed, SessionEffect::PointerCapture(capture));
    }
}

#[test]
fn minimize_releases_only() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    assert_eq!(
        fsm.on(SessionInput::Window(WindowChange::Minimized(true))),
        [SessionEffect::ReleaseAll]
    );
    assert!(fsm.input_enabled());
    assert!(fsm.keyboard_captured());
    assert!(fsm
        .on(SessionInput::Window(WindowChange::Minimized(true)))
        .is_empty());
    assert!(fsm
        .on(SessionInput::Window(WindowChange::Minimized(false)))
        .is_empty());
    let mut viewer = streaming(ClientOs::Mac, HostOs::Mac, Role::Viewer);
    assert!(viewer
        .on(SessionInput::Window(WindowChange::Minimized(true)))
        .is_empty());
}

#[test]
fn chord_release_title() {
    for (client, chord) in [(ClientOs::Mac, "⌃⌥"), (ClientOs::Windows, "Ctrl+Alt")] {
        let mut fsm = streaming(client, HostOs::Mac, Role::Controller);
        let effects = fsm.on(SessionInput::CaptureReleasedByChord);
        has(&effects, SessionEffect::KeyboardCapture(false));
        has(
            &effects,
            title(&format!("客厅 Mac — 键盘已释放（按 {chord} 恢复）")),
        );
        assert!(fsm.input_enabled());
        has(
            &fsm.on(SessionInput::ClickedInVideo),
            SessionEffect::KeyboardCapture(true),
        );
        assert!(fsm.on(SessionInput::ClickedInVideo).is_empty());
        fsm.on(SessionInput::CaptureReleasedByChord);
        has(
            &fsm.on(SessionInput::CaptureResumedByChord),
            SessionEffect::KeyboardCapture(true),
        );
        assert!(fsm.on(SessionInput::CaptureResumedByChord).is_empty());
    }
}

#[test]
fn windows_fullscreen_grabs() {
    let mut fsm = streaming(ClientOs::Windows, HostOs::Windows, Role::Controller);
    let effects = fsm.on(SessionInput::Window(WindowChange::Fullscreen(true)));
    assert_eq!(
        effects,
        [
            w(WindowCommand::SetSystemKeyGrab(true)),
            SessionEffect::PointerCapture(PointerCapture::Confine),
            w(WindowCommand::SetPointerCapture(PointerCapture::Confine)),
            w(WindowCommand::SetSessionMenu(fsm.menu())),
        ]
    );
    assert!(fsm.menu().fullscreen);
    let effects = fsm.on(SessionInput::CaptureReleasedByChord);
    has(&effects, w(WindowCommand::SetSystemKeyGrab(false)));
    assert_eq!(fsm.pointer_capture(), PointerCapture::Confine);
    fsm.on(SessionInput::CaptureResumedByChord);
    let effects = fsm.on(SessionInput::Window(WindowChange::Focus(false)));
    has(&effects, w(WindowCommand::SetSystemKeyGrab(false)));
    has(
        &effects,
        SessionEffect::PointerCapture(PointerCapture::Free),
    );
}

#[test]
fn mac_never_grabs() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    let effects = fsm.on(SessionInput::Window(WindowChange::Fullscreen(true)));
    assert!(!cmds(&effects)
        .iter()
        .any(|c| matches!(c, WindowCommand::SetSystemKeyGrab(_))));
    assert_eq!(fsm.pointer_capture(), PointerCapture::Free);
    let effects = fsm.on(SessionInput::Window(WindowChange::Fullscreen(false)));
    assert!(!cmds(&effects)
        .iter()
        .any(|c| matches!(c, WindowCommand::SetSystemKeyGrab(_))));
}

#[test]
fn mac_pointer_lock() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    let effects = fsm.on(menu(MenuCommand::TogglePointerLock));
    assert_eq!(
        effects,
        [
            SessionEffect::PointerCapture(PointerCapture::Virtual),
            w(WindowCommand::SetPointerCapture(PointerCapture::Virtual)),
            w(WindowCommand::ShowRemoteCursor(true)),
            w(WindowCommand::SetSessionMenu(fsm.menu())),
        ]
    );
    assert!(fsm.menu().pointer_locked);
    let effects = fsm.on(menu(MenuCommand::TogglePointerLock));
    has(
        &effects,
        SessionEffect::PointerCapture(PointerCapture::Free),
    );
    has(&effects, w(WindowCommand::ShowRemoteCursor(false)));
    assert!(!fsm.menu().pointer_locked);
}

#[test]
fn windows_pointer_lock_is_confine() {
    let mut fsm = streaming(ClientOs::Windows, HostOs::Mac, Role::Controller);
    let effects = fsm.on(menu(MenuCommand::TogglePointerLock));
    has(
        &effects,
        SessionEffect::PointerCapture(PointerCapture::Confine),
    );
    assert!(!cmds(&effects)
        .iter()
        .any(|c| matches!(c, WindowCommand::ShowRemoteCursor(true))));
    assert!(fsm.menu().pointer_locked);
}

#[test]
fn no_duplicate_commands() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    assert!(!fsm.on(role(Role::Viewer, false)).is_empty());
    assert!(fsm.on(role(Role::Viewer, false)).is_empty());
    assert!(fsm.on(phase(SessionPhase::Streaming)).is_empty());
    let effects = fsm.on(SessionInput::Window(WindowChange::Focus(false)));
    assert!(!effects.contains(&SessionEffect::ReleaseAll));
    assert!(fsm
        .on(SessionInput::Window(WindowChange::Focus(false)))
        .is_empty());
}

#[test]
fn combo_routing() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    assert_eq!(
        fsm.on(menu(MenuCommand::SendCombo("cmdTab".into()))),
        [SessionEffect::Call(SessionCall::Combo("cmdTab".into()))]
    );
    assert!(fsm
        .on(menu(MenuCommand::SendCombo("winKey".into())))
        .is_empty());
    fsm.on(role(Role::Viewer, false));
    assert!(fsm
        .on(menu(MenuCommand::SendCombo("cmdTab".into())))
        .is_empty());
    let mut unknown = streaming(ClientOs::Windows, HostOs::Unknown, Role::Controller);
    for combo in unknown.menu().combos {
        assert_eq!(
            unknown.on(menu(MenuCommand::SendCombo(combo.clone()))),
            [SessionEffect::Call(SessionCall::Combo(combo))]
        );
    }
}

#[test]
fn ctrl_alt_del_windows_only() {
    for host in [HostOs::Windows, HostOs::Mac, HostOs::Unknown] {
        let mut fsm = streaming(ClientOs::Mac, host, Role::Controller);
        assert_eq!(fsm.menu().ctrl_alt_del, host == HostOs::Windows);
        let effects = fsm.on(menu(MenuCommand::CtrlAltDel));
        if host == HostOs::Windows {
            assert_eq!(effects, [SessionEffect::Call(SessionCall::CtrlAltDel)]);
        } else {
            assert!(effects.is_empty());
        }
        fsm.on(role(Role::Viewer, false));
        assert!(fsm.on(menu(MenuCommand::CtrlAltDel)).is_empty());
    }
}

#[test]
fn quality_and_display() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    let effects = fsm.on(menu(MenuCommand::SetQuality("smooth".into())));
    assert_eq!(
        effects,
        [
            SessionEffect::Call(SessionCall::SetQuality("smooth".into())),
            w(WindowCommand::SetSessionMenu(fsm.menu())),
        ]
    );
    assert_eq!(fsm.menu().quality, "smooth");
    assert!(fsm
        .on(menu(MenuCommand::SetQuality("ultra".into())))
        .is_empty());
    let displays = [(1, "主屏"), (2, "副屏")]
        .into_iter()
        .map(|(id, name)| SessionDisplay {
            id,
            name: name.into(),
            width_px: 1920,
            height_px: 1080,
            primary: id == 1,
            refresh_hz: None,
            is_virtual: false,
        })
        .collect();
    let event = SessionInput::Core(CoreEvent::SessionDisplays {
        session_id: "7".into(),
        displays,
        current_id: 1,
    });
    let effects = fsm.on(event.clone());
    assert_eq!(effects, [w(WindowCommand::SetSessionMenu(fsm.menu()))]);
    assert_eq!(
        fsm.menu().displays,
        [(1, "主屏".into()), (2, "副屏".into())]
    );
    assert_eq!(fsm.menu().current_display, Some(1));
    assert!(fsm.on(event).is_empty());
    assert_eq!(
        fsm.on(menu(MenuCommand::SelectDisplay(2))),
        [SessionEffect::Call(SessionCall::SelectDisplay(2))]
    );
    assert_eq!(fsm.menu().current_display, Some(1));
    assert!(fsm.on(menu(MenuCommand::SelectDisplay(1))).is_empty());
    assert!(fsm.on(menu(MenuCommand::SelectDisplay(3))).is_empty());
}

#[test]
fn request_control_viewer_only() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Viewer);
    assert_eq!(
        fsm.on(menu(MenuCommand::RequestControl)),
        [SessionEffect::Call(SessionCall::RequestControl)]
    );
    fsm.on(role(Role::Controller, false));
    assert!(fsm.on(menu(MenuCommand::RequestControl)).is_empty());
}

#[test]
fn toggle_fullscreen() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    for _ in 0..2 {
        assert_eq!(
            fsm.on(menu(MenuCommand::ToggleFullscreen)),
            [w(WindowCommand::SetFullscreen(true))]
        );
        assert!(!fsm.menu().fullscreen);
    }
    fsm.on(SessionInput::Window(WindowChange::Fullscreen(true)));
    assert_eq!(
        fsm.on(menu(MenuCommand::ToggleFullscreen)),
        [w(WindowCommand::SetFullscreen(false))]
    );
}

#[test]
fn ended_with_error() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    assert!(fsm
        .on(error(
            Some(4),
            Some(GoodbyeReason::HostShutdown),
            Some("对方已断开")
        ))
        .is_empty());
    let effects = fsm.on(phase(SessionPhase::Ended));
    assert_eq!(effects.first(), Some(&SessionEffect::ReleaseAll));
    assert_eq!(
        effects.last(),
        Some(&SessionEffect::Ended {
            by_user: false,
            code: Some(4),
            reason: Some(GoodbyeReason::HostShutdown),
            message: Some("对方已断开".into()),
        })
    );
    has(&effects, title("客厅 Mac — 已断开"));
    has(&effects, overlay(Some("对方已断开")));
    assert!(fsm.is_ended());
    assert!(!fsm.input_enabled());
    assert_eq!(fsm.phase(), SessionPhase::Ended);
}

#[test]
fn ended_terminal() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    fsm.on(phase(SessionPhase::Ended));
    for input in [
        role(Role::Controller, false),
        phase(SessionPhase::Streaming),
        phase(SessionPhase::Ended),
        error(Some(4), None, Some("迟到的错误")),
        SessionInput::Window(WindowChange::Focus(true)),
        SessionInput::Window(WindowChange::Fullscreen(true)),
        menu(MenuCommand::ToggleFullscreen),
        menu(MenuCommand::ToggleKeyboardCapture),
        menu(MenuCommand::SetQuality("eco".into())),
        SessionInput::CaptureReleasedByChord,
        SessionInput::CaptureResumedByChord,
        SessionInput::ClickedInVideo,
    ] {
        assert!(fsm.on(input.clone()).is_empty(), "terminal input {input:?}");
    }
    for input in [
        SessionInput::Window(WindowChange::CloseRequested),
        menu(MenuCommand::Disconnect),
    ] {
        assert_eq!(fsm.on(input), [w(WindowCommand::Close)]);
    }
}

#[test]
fn user_close() {
    for close in [
        SessionInput::Window(WindowChange::CloseRequested),
        menu(MenuCommand::Disconnect),
    ] {
        let mut fsm = streaming(ClientOs::Windows, HostOs::Mac, Role::Controller);
        fsm.on(SessionInput::Window(WindowChange::Fullscreen(true)));
        let effects = fsm.on(close);
        assert_eq!(
            &effects[..3],
            [
                SessionEffect::CloseSession,
                w(WindowCommand::Close),
                SessionEffect::ReleaseAll
            ]
        );
        has(&effects, SessionEffect::InputEnabled(false));
        has(&effects, SessionEffect::KeyboardCapture(false));
        has(&effects, w(WindowCommand::SetSystemKeyGrab(false)));
        has(
            &effects,
            SessionEffect::PointerCapture(PointerCapture::Free),
        );
        assert!(!fsm.input_enabled());
        assert!(!fsm.is_ended());
        assert!(!effects
            .iter()
            .any(|effect| matches!(effect, SessionEffect::Ended { .. })));
        assert!(!cmds(&effects).iter().any(|c| matches!(
            c,
            WindowCommand::SetTitle(_) | WindowCommand::SetOverlayText(_)
        )));
        assert!(fsm.on(phase(SessionPhase::Streaming)).is_empty());
        assert!(!fsm.input_enabled());
        assert!(fsm
            .on(error(None, Some(GoodbyeReason::UserClosed), Some("closed")))
            .is_empty());
        let ended = fsm.on(phase(SessionPhase::Ended));
        assert_eq!(
            ended,
            [SessionEffect::Ended {
                by_user: true,
                code: None,
                reason: Some(GoodbyeReason::UserClosed),
                message: Some("closed".into()),
            }]
        );
        assert!(fsm.on(phase(SessionPhase::Ended)).is_empty());
    }
}

#[test]
fn other_session_ignored() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    let mut events = [
        phase(SessionPhase::Ended),
        role(Role::Viewer, true),
        error(Some(4), Some(GoodbyeReason::Revoked), Some("ignored")),
        SessionInput::Core(CoreEvent::SessionDisplays {
            session_id: "7".into(),
            displays: Vec::new(),
            current_id: 9,
        }),
    ];
    for input in &mut events {
        match input {
            SessionInput::Core(CoreEvent::SessionState { session_id, .. })
            | SessionInput::Core(CoreEvent::SessionRole { session_id, .. })
            | SessionInput::Core(CoreEvent::SessionError { session_id, .. })
            | SessionInput::Core(CoreEvent::SessionDisplays { session_id, .. }) => {
                *session_id = "8".into()
            }
            _ => unreachable!(),
        }
        assert!(fsm.on(input.clone()).is_empty());
    }
    assert!(fsm.input_enabled());
    assert!(!fsm.is_ended());
    assert_eq!(fsm.menu().current_display, None);
}

fn shape(id: u64) -> pb::CursorMsg {
    pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Shape(pb::CursorShape {
            id,
            width: 16,
            height: 32,
            hot_x: 3,
            hot_y: 4,
            scale: 2.0,
            // The cache must preserve the payload without decoding it.
            png: vec![11, 22, 33],
        })),
    }
}

fn use_shape(id: u64) -> pb::CursorMsg {
    pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Use(id)),
    }
}

#[test]
fn cursor_cache() {
    let mut cache = CursorCache::new();
    assert!(cache.current().is_none());
    let image = CursorImage {
        id: 1,
        width: 16,
        height: 32,
        hot_x: 3,
        hot_y: 4,
        scale: 2.0,
        png: vec![11, 22, 33],
    };
    let command = WindowCommand::SetCursorImage(Some(image.clone()));
    assert_eq!(cache.on_msg(&shape(1)), std::slice::from_ref(&command));
    assert_eq!(cache.current(), Some(&image));
    assert_eq!(cache.on_msg(&use_shape(1)), [command]);
    for _ in 0..2 {
        assert_eq!(
            cache.on_msg(&use_shape(99)),
            [WindowCommand::SetCursorImage(None)]
        );
        assert!(cache.current().is_none());
    }
    let hidden = pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Hidden(true)),
    };
    assert_eq!(
        cache.on_msg(&hidden),
        [WindowCommand::SetCursorHidden(true)]
    );
    assert!(cache.on_msg(&hidden).is_empty());
    assert!(cache.on_msg(&pb::CursorMsg::default()).is_empty());

    let mut cache = CursorCache::new();
    for id in 1..=65 {
        cache.on_msg(&shape(id));
    }
    assert_eq!(
        cache.on_msg(&use_shape(1)),
        [WindowCommand::SetCursorImage(None)]
    );
    let mut cache = CursorCache::new();
    for id in 1..=64 {
        cache.on_msg(&shape(id));
    }
    cache.on_msg(&use_shape(1));
    cache.on_msg(&shape(65));
    assert_eq!(cache.current().map(|image| image.id), Some(65));
    assert!(matches!(
        cache.on_msg(&use_shape(1)).as_slice(),
        [WindowCommand::SetCursorImage(Some(image))] if image.id == 1
    ));
    assert_eq!(
        cache.on_msg(&use_shape(2)),
        [WindowCommand::SetCursorImage(None)]
    );
}

#[test]
fn cursor_visible() {
    let mut cache = CursorCache::new();
    assert_eq!(
        cache.on_visible(false),
        [WindowCommand::SetCursorHidden(true)]
    );
    assert!(cache.on_visible(false).is_empty());
    let hidden = pb::CursorMsg {
        msg: Some(pb::cursor_msg::Msg::Hidden(true)),
    };
    assert!(cache.on_msg(&hidden).is_empty());
    cache.on_msg(&shape(1));
    cache.on_msg(&use_shape(1));
    assert!(cache.on_visible(false).is_empty());
    assert_eq!(cache.current().map(|image| image.id), Some(1));
    assert_eq!(
        cache.on_visible(true),
        [WindowCommand::SetCursorHidden(false)]
    );
    assert!(cache.on_visible(true).is_empty());
}

#[test]
fn connection_phases_and_default_error() {
    let mut fsm = fsm(ClientOs::Mac, HostOs::Mac);
    assert!(fsm.on(phase(SessionPhase::Authenticating)).is_empty());
    assert_eq!(
        fsm.on(phase(SessionPhase::PendingApproval)),
        [
            title("客厅 Mac — 等待对方同意"),
            overlay(Some("等待对方在电脑上同意连接…"))
        ]
    );
    let effects = fsm.on(phase(SessionPhase::Ended));
    has(&effects, overlay(Some("连接已结束")));
    assert_eq!(
        effects.last(),
        Some(&SessionEffect::Ended {
            by_user: false,
            code: None,
            reason: None,
            message: None
        })
    );
}

#[test]
fn keyboard_menu_and_paused_release_precedence() {
    let mut fsm = streaming(ClientOs::Mac, HostOs::Mac, Role::Controller);
    let effects = fsm.on(menu(MenuCommand::ToggleKeyboardCapture));
    has(&effects, SessionEffect::KeyboardCapture(false));
    assert!(!fsm.menu().keyboard_captured);
    assert!(fsm
        .on(role(Role::Controller, true))
        .iter()
        .all(|e| !matches!(
            e,
            SessionEffect::Window(WindowCommand::SetTitle(_) | WindowCommand::SetOverlayText(_))
        )));
    let effects = fsm.on(menu(MenuCommand::ToggleKeyboardCapture));
    has(&effects, title("客厅 Mac"));
    has(&effects, overlay(Some("对方正在使用这台电脑，输入已暂停")));
    assert!(!fsm.keyboard_captured());
}

#[test]
fn derived_effect_order_windows() {
    let mut fsm = streaming(ClientOs::Windows, HostOs::Windows, Role::Controller);
    fsm.on(SessionInput::Window(WindowChange::Fullscreen(true)));
    let effects = fsm.on(role(Role::Controller, true));
    assert_eq!(
        effects,
        [
            SessionEffect::ReleaseAll,
            SessionEffect::InputEnabled(false),
            SessionEffect::KeyboardCapture(false),
            w(WindowCommand::SetKeyboardCapture(false)),
            w(WindowCommand::SetSystemKeyGrab(false)),
            SessionEffect::PointerCapture(PointerCapture::Free),
            w(WindowCommand::SetPointerCapture(PointerCapture::Free)),
            w(WindowCommand::ShowRemoteCursor(true)),
            overlay(Some("对方正在使用这台电脑，输入已暂停")),
            w(WindowCommand::SetSessionMenu(fsm.menu())),
        ]
    );
}

#[test]
fn ignored_events_and_quality_defaults() {
    let mut cfg = config(ClientOs::Mac, HostOs::Unknown);
    cfg.quality.clear();
    let mut fsm = SessionFsm::new(cfg);
    assert_eq!(fsm.menu().quality, "auto");
    assert_eq!(
        fsm.menu().combos,
        ["cmdTab", "cmdSpace", "missionControl", "altTab", "winKey"]
    );
    assert!(fsm
        .on(SessionInput::Core(CoreEvent::FileTransferRemoved {
            transfer_id: "ignored".into()
        }))
        .is_empty());
    assert!(fsm
        .on(SessionInput::Window(WindowChange::Resized {
            width: 1280.0,
            height: 720.0,
            scale: 2.0
        }))
        .is_empty());
    for quality in ["clear", "eco", "auto"] {
        let effects = fsm.on(menu(MenuCommand::SetQuality(quality.into())));
        assert_eq!(
            effects,
            [
                SessionEffect::Call(SessionCall::SetQuality(quality.into())),
                w(WindowCommand::SetSessionMenu(fsm.menu()))
            ]
        );
    }
    assert_eq!(
        fsm.on(menu(MenuCommand::SetQuality("auto".into()))),
        [SessionEffect::Call(SessionCall::SetQuality("auto".into()))]
    );
}

#[test]
fn cursor_shape_replacement_updates_lru() {
    let mut cache = CursorCache::new();
    for id in 1..=64 {
        cache.on_msg(&shape(id));
    }
    let mut replacement = shape(1);
    if let Some(pb::cursor_msg::Msg::Shape(shape)) = &mut replacement.msg {
        shape.png = vec![44, 55];
        shape.scale = 1.5;
    }
    cache.on_msg(&replacement);
    cache.on_msg(&shape(65));
    assert!(
        matches!(cache.on_msg(&use_shape(1)).as_slice(), [WindowCommand::SetCursorImage(Some(image))] if image.png == [44, 55] && image.scale == 1.5)
    );
    assert_eq!(
        cache.on_msg(&use_shape(2)),
        [WindowCommand::SetCursorImage(None)]
    );
    assert!(cache.on_msg(&pb::CursorMsg::default()).is_empty());
}
