//! 菜单的纯逻辑：菜单 id ↔ 动作，会话 / 设置 / 帮助菜单的描述。`menu.rs` 把描述翻译成 `tauri::menu`。
//!
//! 描述里没有快捷键字段：会话菜单与 Windows 菜单都不设快捷键（tao 的加速键翻译会吃掉发给远端的按键）。

use rs_desktop::{
    platform::{MenuCommand, SessionMenu},
    win_model::{combo_text, QUALITY_ORDER},
};

use crate::settings::{DesktopSettings, KEYBOARDS, QUALITIES};

/// `SessionCombo` 的 JSON 名。
pub const COMBOS: [&str; 5] = ["cmdTab", "cmdSpace", "missionControl", "altTab", "winKey"];
const KEYBOARD_TEXT: [(&str, &str); 3] = [("ansi", "ANSI"), ("iso", "ISO"), ("jis", "JIS")];
/// 没有聚焦会话时，会话菜单里唯一的一项（置灰）。
pub const NO_SESSION_ID: &str = "session.none";

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum Action {
    /// 发给聚焦的会话。
    Session(MenuCommand),
    Settings(SettingsAction),
    App(AppAction),
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum SettingsAction {
    /// Mac：连 Windows 时 ⌘ 当 Ctrl。
    CmdToCtrl,
    /// Windows：连 Mac 时 Ctrl 与 Win 互换。
    SwapCtrlMeta,
    Keyboard(String),
    Quality(String),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum AppAction {
    About,
    Update,
    Logs,
}

impl Action {
    pub fn parse(id: &str) -> Option<Self> {
        let (group, rest) = id.split_once('.')?;
        match group {
            "session" => parse_session(rest).map(Self::Session),
            "settings" => parse_settings(rest).map(Self::Settings),
            "app" => match rest {
                "about" => Some(Self::App(AppAction::About)),
                "update" => Some(Self::App(AppAction::Update)),
                "logs" => Some(Self::App(AppAction::Logs)),
                _ => None,
            },
            _ => None,
        }
    }

    /// `parse` 的逆。
    pub fn id(&self) -> String {
        match self {
            Self::Session(command) => match command {
                MenuCommand::SendCombo(name) => format!("session.combo.{name}"),
                MenuCommand::CtrlAltDel => "session.cad".to_owned(),
                MenuCommand::ToggleFullscreen => "session.fullscreen".to_owned(),
                MenuCommand::ToggleKeyboardCapture => "session.keyboard".to_owned(),
                MenuCommand::TogglePointerLock => "session.pointer".to_owned(),
                MenuCommand::SetQuality(quality) => format!("session.quality.{quality}"),
                MenuCommand::SelectDisplay(id) => format!("session.display.{id}"),
                MenuCommand::RequestControl => "session.request".to_owned(),
                MenuCommand::Disconnect => "session.disconnect".to_owned(),
            },
            Self::Settings(change) => match change {
                SettingsAction::CmdToCtrl => "settings.cmdctrl".to_owned(),
                SettingsAction::SwapCtrlMeta => "settings.swap".to_owned(),
                SettingsAction::Keyboard(kind) => format!("settings.kbd.{kind}"),
                SettingsAction::Quality(quality) => format!("settings.quality.{quality}"),
            },
            Self::App(AppAction::About) => "app.about".to_owned(),
            Self::App(AppAction::Update) => "app.update".to_owned(),
            Self::App(AppAction::Logs) => "app.logs".to_owned(),
        }
    }
}

fn parse_session(rest: &str) -> Option<MenuCommand> {
    let command = match rest {
        "cad" => MenuCommand::CtrlAltDel,
        "fullscreen" => MenuCommand::ToggleFullscreen,
        "keyboard" => MenuCommand::ToggleKeyboardCapture,
        "pointer" => MenuCommand::TogglePointerLock,
        "request" => MenuCommand::RequestControl,
        "disconnect" => MenuCommand::Disconnect,
        _ => {
            let (kind, value) = rest.split_once('.')?;
            match kind {
                "combo" if COMBOS.contains(&value) => MenuCommand::SendCombo(value.to_owned()),
                "quality" if QUALITIES.contains(&value) => {
                    MenuCommand::SetQuality(value.to_owned())
                }
                "display" if is_decimal(value) => MenuCommand::SelectDisplay(value.parse().ok()?),
                _ => return None,
            }
        }
    };
    Some(command)
}

fn parse_settings(rest: &str) -> Option<SettingsAction> {
    let change = match rest {
        "cmdctrl" => SettingsAction::CmdToCtrl,
        "swap" => SettingsAction::SwapCtrlMeta,
        _ => {
            let (kind, value) = rest.split_once('.')?;
            match kind {
                "kbd" if KEYBOARDS.contains(&value) => SettingsAction::Keyboard(value.to_owned()),
                "quality" if QUALITIES.contains(&value) => {
                    SettingsAction::Quality(value.to_owned())
                }
                _ => return None,
            }
        }
    };
    Some(change)
}

fn is_decimal(text: &str) -> bool {
    !text.is_empty() && text.bytes().all(|byte| byte.is_ascii_digit())
}

impl SettingsAction {
    /// 菜单项被点后的新设置。
    pub fn apply(&self, settings: &DesktopSettings) -> DesktopSettings {
        let mut next = settings.clone();
        match self {
            Self::CmdToCtrl => next.mac_cmd_to_ctrl = !settings.mac_cmd_to_ctrl,
            Self::SwapCtrlMeta => next.win_swap_ctrl_meta = !settings.win_swap_ctrl_meta,
            Self::Keyboard(kind) => next.mac_keyboard.clone_from(kind),
            Self::Quality(quality) => next.default_quality.clone_from(quality),
        }
        next
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MenuItemSpec {
    /// 分隔线为空串；子菜单的 id 不对应动作。
    pub id: String,
    pub text: String,
    pub kind: ItemKind,
    pub enabled: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum ItemKind {
    Normal,
    Check(bool),
    /// 单选组里的一项；Tauri 没有单选项，画成勾选项，同组只有一项为 true。
    Radio(bool),
    Separator,
    Submenu(Vec<MenuItemSpec>),
}

fn item(action: Action, text: &str, kind: ItemKind, enabled: bool) -> MenuItemSpec {
    MenuItemSpec {
        id: action.id(),
        text: text.to_owned(),
        kind,
        enabled,
    }
}

fn separator() -> MenuItemSpec {
    MenuItemSpec {
        id: String::new(),
        text: String::new(),
        kind: ItemKind::Separator,
        enabled: true,
    }
}

fn submenu(id: &str, text: &str, items: Vec<MenuItemSpec>, enabled: bool) -> MenuItemSpec {
    MenuItemSpec {
        id: id.to_owned(),
        text: text.to_owned(),
        kind: ItemKind::Submenu(items),
        enabled,
    }
}

fn session(command: MenuCommand) -> Action {
    Action::Session(command)
}

/// 聚焦会话的菜单。观看者：组合键、Ctrl+Alt+Del、捕获键盘、锁定鼠标置灰，多一项「请求控制」。
/// `mac`：Mac 菜单栏有「锁定鼠标」（隐藏光标、虚拟光标）；Windows 全屏时自动限制光标，外壳不画会话菜单。
pub fn session_menu_items(menu: &SessionMenu, mac: bool) -> Vec<MenuItemSpec> {
    let control = !menu.viewer;
    let mut items = Vec::new();
    if !menu.combos.is_empty() {
        let combos = menu
            .combos
            .iter()
            .map(|name| {
                item(
                    session(MenuCommand::SendCombo(name.clone())),
                    combo_text(name),
                    ItemKind::Normal,
                    control,
                )
            })
            .collect();
        items.push(submenu("session.combos", "发送组合键", combos, control));
    }
    if menu.ctrl_alt_del {
        items.push(item(
            session(MenuCommand::CtrlAltDel),
            "Ctrl+Alt+Del",
            ItemKind::Normal,
            control,
        ));
    }
    items.push(separator());
    items.push(item(
        session(MenuCommand::ToggleFullscreen),
        "全屏",
        ItemKind::Check(menu.fullscreen),
        true,
    ));
    items.push(item(
        session(MenuCommand::ToggleKeyboardCapture),
        "捕获键盘",
        ItemKind::Check(menu.keyboard_captured),
        control,
    ));
    if mac {
        items.push(item(
            session(MenuCommand::TogglePointerLock),
            "锁定鼠标",
            ItemKind::Check(menu.pointer_locked),
            control,
        ));
    }
    let qualities = QUALITY_ORDER
        .iter()
        .map(|&(quality, text)| {
            item(
                session(MenuCommand::SetQuality(quality.to_owned())),
                text,
                ItemKind::Radio(menu.quality == quality),
                true,
            )
        })
        .collect();
    items.push(submenu("session.qualities", "画质", qualities, true));
    if menu.displays.len() >= 2 {
        let displays = menu
            .displays
            .iter()
            .map(|(id, name)| {
                item(
                    session(MenuCommand::SelectDisplay(*id)),
                    name,
                    ItemKind::Radio(menu.current_display == Some(*id)),
                    true,
                )
            })
            .collect();
        items.push(submenu("session.displays", "显示器", displays, true));
    }
    items.push(separator());
    if menu.viewer {
        items.push(item(
            session(MenuCommand::RequestControl),
            "请求控制",
            ItemKind::Normal,
            true,
        ));
    }
    items.push(item(
        session(MenuCommand::Disconnect),
        "断开",
        ItemKind::Normal,
        true,
    ));
    items
}

/// 没有聚焦会话时的会话菜单。
pub fn no_session_items() -> Vec<MenuItemSpec> {
    vec![MenuItemSpec {
        id: NO_SESSION_ID.to_owned(),
        text: "没有活动的会话".to_owned(),
        kind: ItemKind::Normal,
        enabled: false,
    }]
}

/// 「设置」菜单（Mac 和 Windows 都有）。
pub fn settings_menu_items(settings: &DesktopSettings, mac: bool) -> Vec<MenuItemSpec> {
    let setting = Action::Settings;
    let mut items = Vec::new();
    if mac {
        items.push(item(
            setting(SettingsAction::CmdToCtrl),
            "连 Windows 时 ⌘ 当 Ctrl",
            ItemKind::Check(settings.mac_cmd_to_ctrl),
            true,
        ));
        let keyboards = KEYBOARD_TEXT
            .iter()
            .map(|&(kind, text)| {
                item(
                    setting(SettingsAction::Keyboard(kind.to_owned())),
                    text,
                    ItemKind::Radio(settings.mac_keyboard == kind),
                    true,
                )
            })
            .collect();
        items.push(submenu("settings.keyboards", "键盘布局", keyboards, true));
    } else {
        items.push(item(
            setting(SettingsAction::SwapCtrlMeta),
            "连 Mac 时 Ctrl 与 Win 互换",
            ItemKind::Check(settings.win_swap_ctrl_meta),
            true,
        ));
    }
    let qualities = QUALITY_ORDER
        .iter()
        .map(|&(quality, text)| {
            item(
                setting(SettingsAction::Quality(quality.to_owned())),
                text,
                ItemKind::Radio(settings.default_quality == quality),
                true,
            )
        })
        .collect();
    items.push(submenu("settings.qualities", "默认画质", qualities, true));
    items
}

/// 外壳自己的 App 项：Mac 放在 App 菜单（关于、检查更新…、导出日志…），Windows 放在「帮助」
/// （检查更新、导出日志、关于）。没注册更新器时没有「检查更新」。
pub fn app_items(mac: bool, updater: bool) -> Vec<MenuItemSpec> {
    let about = item(
        Action::App(AppAction::About),
        "关于 Remote Screen",
        ItemKind::Normal,
        true,
    );
    let update = updater.then(|| {
        item(
            Action::App(AppAction::Update),
            if mac {
                "检查更新…"
            } else {
                "检查更新"
            },
            ItemKind::Normal,
            true,
        )
    });
    let logs = item(
        Action::App(AppAction::Logs),
        if mac {
            "导出日志…"
        } else {
            "导出日志"
        },
        ItemKind::Normal,
        true,
    );
    if mac {
        [Some(about), update, Some(logs)]
            .into_iter()
            .flatten()
            .collect()
    } else {
        [update, Some(logs), Some(about)]
            .into_iter()
            .flatten()
            .collect()
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn find<'a>(items: &'a [MenuItemSpec], text: &str) -> Option<&'a MenuItemSpec> {
        items.iter().find(|item| item.text == text)
    }

    fn children<'a>(items: &'a [MenuItemSpec], text: &str) -> &'a [MenuItemSpec] {
        match &find(items, text)
            .unwrap_or_else(|| panic!("no {text}"))
            .kind
        {
            ItemKind::Submenu(children) => children,
            other => panic!("{text} is {other:?}"),
        }
    }

    fn texts(items: &[MenuItemSpec]) -> Vec<&str> {
        items.iter().map(|item| item.text.as_str()).collect()
    }

    /// 能点的项的 id 都能解析回同一个动作。
    fn assert_ids_parse(items: &[MenuItemSpec]) {
        for item in items {
            match &item.kind {
                ItemKind::Separator => {}
                ItemKind::Submenu(children) => {
                    assert_eq!(Action::parse(&item.id), None, "{}", item.id);
                    assert_ids_parse(children);
                }
                _ => {
                    let action = Action::parse(&item.id)
                        .unwrap_or_else(|| panic!("{} does not parse", item.id));
                    assert_eq!(action.id(), item.id);
                }
            }
        }
    }

    fn mac_host_menu() -> SessionMenu {
        SessionMenu {
            displays: vec![(1, "内建".to_owned()), (2, "外接".to_owned())],
            current_display: Some(2),
            quality: "smooth".to_owned(),
            viewer: false,
            fullscreen: true,
            keyboard_captured: true,
            pointer_locked: false,
            combos: vec![
                "cmdTab".to_owned(),
                "cmdSpace".to_owned(),
                "missionControl".to_owned(),
            ],
            ctrl_alt_del: false,
        }
    }

    #[test]
    fn menu_action_parse() {
        assert_eq!(
            Action::parse("session.combo.cmdTab"),
            Some(Action::Session(MenuCommand::SendCombo("cmdTab".to_owned())))
        );
        assert_eq!(
            Action::parse("session.display.2"),
            Some(Action::Session(MenuCommand::SelectDisplay(2)))
        );
        assert_eq!(Action::parse("session.quality.ultra"), None);
        assert_eq!(
            Action::parse("session.quality.eco"),
            Some(Action::Session(MenuCommand::SetQuality("eco".to_owned())))
        );
        assert_eq!(
            Action::parse("settings.kbd.iso"),
            Some(Action::Settings(SettingsAction::Keyboard("iso".to_owned())))
        );
        assert_eq!(
            Action::parse("settings.quality.clear"),
            Some(Action::Settings(SettingsAction::Quality(
                "clear".to_owned()
            )))
        );
        assert_eq!(
            Action::parse("session.cad"),
            Some(Action::Session(MenuCommand::CtrlAltDel))
        );
        assert_eq!(
            Action::parse("app.logs"),
            Some(Action::App(AppAction::Logs))
        );
        for bad in [
            "",
            "session",
            "session.",
            "session.combo.ctrlEsc",
            "session.display.",
            "session.display.-1",
            "session.display.+2",
            "session.display.4294967296",
            "settings.kbd.dvorak",
            "settings.quality.ultra",
            // 旧版本的「使用 Hub」。
            "settings.hub",
            "app.quit",
            NO_SESSION_ID,
            "session.combos",
            "window.minimize",
        ] {
            assert_eq!(Action::parse(bad), None, "{bad}");
        }
    }

    #[test]
    fn session_menu_items_mac() {
        let items = session_menu_items(&mac_host_menu(), true);
        assert_ids_parse(&items);

        assert_eq!(
            texts(children(&items, "发送组合键")),
            ["⌘Tab", "⌘空格", "调度中心"]
        );
        let displays = children(&items, "显示器");
        assert_eq!(texts(displays), ["内建", "外接"]);
        assert_eq!(displays[0].kind, ItemKind::Radio(false));
        assert_eq!(displays[1].kind, ItemKind::Radio(true));
        assert_eq!(displays[1].id, "session.display.2");

        let qualities = children(&items, "画质");
        let selected: Vec<&str> = qualities
            .iter()
            .filter(|item| item.kind == ItemKind::Radio(true))
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(selected, ["session.quality.smooth"]);
        assert_eq!(qualities.len(), 4);

        assert_eq!(
            find(&items, "全屏").map(|item| &item.kind),
            Some(&ItemKind::Check(true))
        );
        assert_eq!(
            find(&items, "捕获键盘").map(|item| &item.kind),
            Some(&ItemKind::Check(true))
        );
        assert_eq!(
            find(&items, "锁定鼠标").map(|item| &item.kind),
            Some(&ItemKind::Check(false))
        );
        assert!(find(&items, "Ctrl+Alt+Del").is_none());
        assert!(find(&items, "请求控制").is_none());
        assert_eq!(
            items.last().map(|item| item.id.as_str()),
            Some("session.disconnect")
        );
        assert!(items.iter().all(|item| item.enabled));
    }

    #[test]
    fn session_menu_items_windows_host() {
        let menu = SessionMenu {
            combos: vec!["altTab".to_owned(), "winKey".to_owned()],
            ctrl_alt_del: true,
            ..mac_host_menu()
        };
        let items = session_menu_items(&menu, true);
        assert_ids_parse(&items);
        assert_eq!(texts(children(&items, "发送组合键")), ["Alt+Tab", "Win 键"]);
        let cad = find(&items, "Ctrl+Alt+Del").expect("ctrl alt del");
        assert_eq!(cad.id, "session.cad");
        assert!(cad.enabled);
    }

    #[test]
    fn session_menu_items_viewer() {
        let menu = SessionMenu {
            viewer: true,
            ctrl_alt_del: true,
            ..mac_host_menu()
        };
        let items = session_menu_items(&menu, true);
        assert_ids_parse(&items);
        let combos = find(&items, "发送组合键").expect("combos");
        assert!(!combos.enabled);
        assert!(children(&items, "发送组合键")
            .iter()
            .all(|item| !item.enabled));
        assert!(!find(&items, "Ctrl+Alt+Del").expect("cad").enabled);
        assert!(!find(&items, "捕获键盘").expect("keyboard").enabled);
        let request = find(&items, "请求控制").expect("request control");
        assert_eq!(request.id, "session.request");
        assert!(request.enabled);
        assert!(find(&items, "全屏").expect("fullscreen").enabled);
        assert!(find(&items, "断开").expect("disconnect").enabled);
    }

    #[test]
    fn session_menu_single_display() {
        let menu = SessionMenu {
            displays: vec![(1, "内建".to_owned())],
            current_display: Some(1),
            ..mac_host_menu()
        };
        let items = session_menu_items(&menu, true);
        assert!(find(&items, "显示器").is_none());
        let none = SessionMenu {
            displays: Vec::new(),
            current_display: None,
            ..mac_host_menu()
        };
        assert!(find(&session_menu_items(&none, true), "显示器").is_none());
    }

    #[test]
    fn session_menu_pointer_lock_mac_only() {
        let items = session_menu_items(&mac_host_menu(), false);
        assert!(find(&items, "锁定鼠标").is_none());
        assert!(find(&items, "捕获键盘").is_some());
    }

    #[test]
    fn no_session_menu() {
        let items = no_session_items();
        assert_eq!(texts(&items), ["没有活动的会话"]);
        assert!(!items[0].enabled);
        assert_eq!(Action::parse(&items[0].id), None);
    }

    #[test]
    fn settings_menu_mac() {
        let settings = DesktopSettings {
            mac_keyboard: "jis".to_owned(),
            default_quality: "eco".to_owned(),
            ..DesktopSettings::default()
        };
        let items = settings_menu_items(&settings, true);
        assert_ids_parse(&items);
        assert_eq!(
            texts(&items),
            ["连 Windows 时 ⌘ 当 Ctrl", "键盘布局", "默认画质"]
        );
        assert_eq!(
            find(&items, "连 Windows 时 ⌘ 当 Ctrl").map(|item| &item.kind),
            Some(&ItemKind::Check(true))
        );
        assert!(find(&items, "连 Mac 时 Ctrl 与 Win 互换").is_none());
        let keyboards = children(&items, "键盘布局");
        assert_eq!(texts(keyboards), ["ANSI", "ISO", "JIS"]);
        assert_eq!(keyboards[2].kind, ItemKind::Radio(true));
        let selected: Vec<&str> = children(&items, "默认画质")
            .iter()
            .filter(|item| item.kind == ItemKind::Radio(true))
            .map(|item| item.id.as_str())
            .collect();
        assert_eq!(selected, ["settings.quality.eco"]);
    }

    #[test]
    fn settings_menu_windows() {
        let settings = DesktopSettings {
            win_swap_ctrl_meta: true,
            ..DesktopSettings::default()
        };
        let items = settings_menu_items(&settings, false);
        assert_ids_parse(&items);
        assert_eq!(texts(&items), ["连 Mac 时 Ctrl 与 Win 互换", "默认画质"]);
        assert_eq!(
            find(&items, "连 Mac 时 Ctrl 与 Win 互换").map(|item| &item.kind),
            Some(&ItemKind::Check(true))
        );
        assert!(find(&items, "连 Windows 时 ⌘ 当 Ctrl").is_none());
        assert!(find(&items, "键盘布局").is_none());
        assert!(find(&items, "默认画质").is_some());
    }

    #[test]
    fn settings_action_apply() {
        let defaults = DesktopSettings::default();
        assert!(!SettingsAction::CmdToCtrl.apply(&defaults).mac_cmd_to_ctrl);
        assert!(
            SettingsAction::SwapCtrlMeta
                .apply(&defaults)
                .win_swap_ctrl_meta
        );
        assert_eq!(
            SettingsAction::Keyboard("iso".to_owned())
                .apply(&defaults)
                .mac_keyboard,
            "iso"
        );
        let clear = SettingsAction::Quality("clear".to_owned()).apply(&defaults);
        assert_eq!(clear.default_quality, "clear");
        assert!(clear.validate().is_ok());
    }

    #[test]
    fn app_items_follow_updater() {
        let mac = app_items(true, false);
        assert_ids_parse(&mac);
        assert_eq!(texts(&mac), ["关于 Remote Screen", "导出日志…"]);
        assert_eq!(
            texts(&app_items(true, true)),
            ["关于 Remote Screen", "检查更新…", "导出日志…"]
        );
        assert_eq!(
            texts(&app_items(false, true)),
            ["检查更新", "导出日志", "关于 Remote Screen"]
        );
        assert_eq!(
            texts(&app_items(false, false)),
            ["导出日志", "关于 Remote Screen"]
        );
    }

    #[test]
    fn qualities_agree_with_settings() {
        let mut names: Vec<&str> = QUALITY_ORDER.iter().map(|&(name, _)| name).collect();
        names.sort_unstable();
        let mut known = QUALITIES.to_vec();
        known.sort_unstable();
        assert_eq!(names, known);
    }
}
