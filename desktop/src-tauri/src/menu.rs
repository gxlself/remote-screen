//! 菜单栏：按 `menu_model` 的描述建 `tauri::menu`，处理菜单事件。
//!
//! - Mac：App、编辑、会话、设置、窗口。编辑菜单用预置项（带 ⌘ 快捷键，WebView 输入框的 ⌘C / ⌘V 靠它）；
//!   会话菜单不设快捷键，按聚焦会话重建。
//! - Windows：主窗口只有设置、帮助，所有项不设快捷键（会话窗口在同一线程的消息循环里，
//!   加速键翻译会吃掉发给远端的按键）。会话菜单在会话窗口的系统菜单里。

use std::path::PathBuf;

use serde_json::{json, Value};
use tauri::{
    menu::{CheckMenuItem, Menu, MenuItem, MenuItemKind, PredefinedMenuItem, Submenu},
    AppHandle, Manager, Wry,
};
use tauri_plugin_dialog::{DialogExt, MessageDialogButtons, MessageDialogKind};
use tokio::sync::oneshot;

use crate::{
    commands, lock,
    menu_model::{self, Action, AppAction, ItemKind, MenuItemSpec, SettingsAction},
    AppState, MAC,
};

const TITLE: &str = "Remote Screen";

/// 会重建的两个子菜单。Windows 没有会话菜单。
pub struct Handles {
    session: Option<Submenu<Wry>>,
    settings: Submenu<Wry>,
}

/// 建菜单栏并设为应用菜单（Windows 上即主窗口菜单）。
pub fn install(app: &AppHandle, state: &AppState) -> tauri::Result<Handles> {
    let settings = Submenu::with_id(app, "settings", "设置", true)?;
    refill(&settings, &settings_items(state))?;
    let app_items = build_items(app, &menu_model::app_items(MAC, state.updater))?;

    #[cfg(target_os = "macos")]
    let (menu, session) = {
        let app_menu = Submenu::with_id(app, "app", TITLE, true)?;
        for item in &app_items {
            app_menu.append(item)?;
        }
        app_menu.append_items(&[
            &PredefinedMenuItem::separator(app)?,
            &PredefinedMenuItem::hide(app, Some("隐藏 Remote Screen"))?,
            &PredefinedMenuItem::quit(app, Some("退出 Remote Screen"))?,
        ])?;
        let edit = Submenu::with_items(
            app,
            "编辑",
            true,
            &[
                &PredefinedMenuItem::undo(app, Some("撤销"))?,
                &PredefinedMenuItem::redo(app, Some("重做"))?,
                &PredefinedMenuItem::separator(app)?,
                &PredefinedMenuItem::cut(app, Some("剪切"))?,
                &PredefinedMenuItem::copy(app, Some("拷贝"))?,
                &PredefinedMenuItem::paste(app, Some("粘贴"))?,
                &PredefinedMenuItem::select_all(app, Some("全选"))?,
            ],
        )?;
        let session = Submenu::with_id(app, "session", "会话", true)?;
        refill(&session, &session_items(state))?;
        let window = Submenu::with_items(
            app,
            "窗口",
            true,
            &[
                &PredefinedMenuItem::minimize(app, Some("最小化"))?,
                &PredefinedMenuItem::maximize(app, Some("缩放"))?,
            ],
        )?;
        window.set_as_windows_menu_for_nsapp()?;
        let menu = Menu::with_items(app, &[&app_menu, &edit, &session, &settings, &window])?;
        (menu, Some(session))
    };

    #[cfg(not(target_os = "macos"))]
    let (menu, session) = {
        let help = Submenu::with_id(app, "help", "帮助", true)?;
        for item in &app_items {
            help.append(item)?;
        }
        (Menu::with_items(app, &[&settings, &help])?, None)
    };

    app.set_menu(menu)?;
    Ok(Handles { session, settings })
}

/// 菜单事件（主线程）。
pub fn on_event(app: &AppHandle, id: &str) {
    let Some(action) = Action::parse(id) else {
        return;
    };
    match action {
        Action::Session(command) => {
            if let Some(state) = app.try_state::<AppState>() {
                state.sessions.menu_command(command);
            }
            // 勾选项被点时 muda 自己翻转了勾；按会话的真实状态重画。
            refresh_session(app);
        }
        Action::Settings(change) => change_setting(app, &change),
        Action::App(AppAction::About) => about(app),
        Action::App(AppAction::Update) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { check_update(app).await });
        }
        Action::App(AppAction::Logs) => {
            let app = app.clone();
            tauri::async_runtime::spawn(async move { export_logs(app).await });
        }
    }
}

/// 在主线程上重建会话菜单（在主线程上调用时立即执行）。
pub fn refresh_session_later(app: &AppHandle) {
    let handle = app.clone();
    if app
        .run_on_main_thread(move || refresh_session(&handle))
        .is_err()
    {
        tracing::warn!("session menu refresh not scheduled");
    }
}

fn refresh_session(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Some(session) = lock(&state.menus)
        .as_ref()
        .and_then(|handles| handles.session.clone())
    else {
        return;
    };
    if let Err(error) = refill(&session, &session_items(&state)) {
        tracing::warn!(%error, "session menu not rebuilt");
    }
}

pub fn refresh_settings(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    let Some(settings) = lock(&state.menus)
        .as_ref()
        .map(|handles| handles.settings.clone())
    else {
        return;
    };
    if let Err(error) = refill(&settings, &settings_items(&state)) {
        tracing::warn!(%error, "settings menu not rebuilt");
    }
}

fn session_items(state: &AppState) -> Vec<MenuItemSpec> {
    match lock(&state.focused_session).as_ref() {
        Some((_, menu)) => menu_model::session_menu_items(menu, MAC),
        None => menu_model::no_session_items(),
    }
}

fn settings_items(state: &AppState) -> Vec<MenuItemSpec> {
    menu_model::settings_menu_items(&lock(&state.settings), MAC)
}

/// 清空子菜单再按描述填入。
fn refill(submenu: &Submenu<Wry>, specs: &[MenuItemSpec]) -> tauri::Result<()> {
    for item in submenu.items()? {
        submenu.remove(&item)?;
    }
    for item in build_items(submenu.app_handle(), specs)? {
        submenu.append(&item)?;
    }
    Ok(())
}

fn build_items(app: &AppHandle, specs: &[MenuItemSpec]) -> tauri::Result<Vec<MenuItemKind<Wry>>> {
    specs.iter().map(|spec| build_item(app, spec)).collect()
}

fn build_item(app: &AppHandle, spec: &MenuItemSpec) -> tauri::Result<MenuItemKind<Wry>> {
    let id = spec.id.as_str();
    let item = match &spec.kind {
        ItemKind::Normal => MenuItemKind::MenuItem(MenuItem::with_id(
            app,
            id,
            &spec.text,
            spec.enabled,
            None::<&str>,
        )?),
        ItemKind::Check(checked) | ItemKind::Radio(checked) => MenuItemKind::Check(
            CheckMenuItem::with_id(app, id, &spec.text, spec.enabled, *checked, None::<&str>)?,
        ),
        ItemKind::Separator => MenuItemKind::Predefined(PredefinedMenuItem::separator(app)?),
        ItemKind::Submenu(children) => {
            let submenu = Submenu::with_id(app, id, &spec.text, spec.enabled)?;
            for child in build_items(app, children)? {
                submenu.append(&child)?;
            }
            MenuItemKind::Submenu(submenu)
        }
    };
    Ok(item)
}

fn change_setting(app: &AppHandle, change: &SettingsAction) {
    if let Some(state) = app.try_state::<AppState>() {
        let current = lock(&state.settings).clone();
        let next = change.apply(&current);
        if next != current {
            if let Err(error) = commands::apply_settings(app, &state, next) {
                message(
                    app,
                    MessageDialogKind::Error,
                    &format!("设置没有保存：{}", error.inner.message),
                );
            }
        }
    }
    // 单选项被点时 muda 也会翻转它的勾。
    refresh_settings(app);
}

fn message(app: &AppHandle, kind: MessageDialogKind, text: &str) {
    app.dialog()
        .message(text)
        .title(TITLE)
        .kind(kind)
        .show(|_| {});
}

/// 不阻塞调用线程；用户关掉对话框后返回是否点了确认。
async fn confirm(app: &AppHandle, text: String, ok: &str) -> bool {
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .message(text)
        .title(TITLE)
        .buttons(MessageDialogButtons::OkCancelCustom(
            ok.to_owned(),
            "以后".to_owned(),
        ))
        .show(move |accepted| {
            let _ = tx.send(accepted);
        });
    rx.await.unwrap_or(false)
}

fn about(app: &AppHandle) {
    let mut text = format!("Remote Screen {}", app.package_info().version);
    let updater = app
        .try_state::<AppState>()
        .is_some_and(|state| state.updater);
    if !updater {
        text.push_str("\n\n新版本请到下载页获取。");
    }
    message(app, MessageDialogKind::Info, &text);
}

async fn check_update(app: AppHandle) {
    use tauri_plugin_updater::UpdaterExt;

    let checked = match app.updater() {
        Ok(updater) => updater.check().await,
        Err(error) => Err(error),
    };
    match checked {
        Ok(Some(update)) => {
            let text = format!("有新版本 {}，现在下载并安装吗？", update.version);
            if !confirm(&app, text, "下载并安装").await {
                return;
            }
            match update.download_and_install(|_, _| {}, || {}).await {
                Ok(()) => message(
                    &app,
                    MessageDialogKind::Info,
                    "更新已安装，重启 Remote Screen 后生效。",
                ),
                Err(error) => {
                    tracing::warn!(%error, "update install failed");
                    message(
                        &app,
                        MessageDialogKind::Error,
                        &format!("更新失败：{error}"),
                    );
                }
            }
        }
        Ok(None) => message(&app, MessageDialogKind::Info, "已经是最新版本。"),
        Err(error) => {
            tracing::warn!(%error, "update check failed");
            message(
                &app,
                MessageDialogKind::Error,
                &format!("检查更新失败：{error}"),
            );
        }
    }
}

/// `logsExport(true)` 打包（脱敏）到数据目录，再让用户选位置复制过去。
async fn export_logs(app: AppHandle) {
    let Some(bridge) = app
        .try_state::<AppState>()
        .map(|state| state.bridge.clone())
    else {
        return;
    };
    let source = match bridge.call("logsExport", json!([true])).await {
        Ok(Value::String(path)) => PathBuf::from(path),
        Ok(_) => {
            message(&app, MessageDialogKind::Error, "导出日志失败。");
            return;
        }
        Err(error) => {
            message(
                &app,
                MessageDialogKind::Error,
                &format!("导出日志失败：{}", error.inner.message),
            );
            return;
        }
    };
    let file_name = source
        .file_name()
        .map(|name| name.to_string_lossy().into_owned())
        .unwrap_or_else(|| "remote-screen-logs.zip".to_owned());
    let (tx, rx) = oneshot::channel();
    app.dialog()
        .file()
        .set_title("导出日志")
        .set_file_name(file_name)
        .add_filter("ZIP", &["zip"])
        .save_file(move |path| {
            let _ = tx.send(path);
        });
    let Some(target) = rx.await.ok().flatten() else {
        return;
    };
    let copied = target
        .into_path()
        .map_err(|error| error.to_string())
        .and_then(|target| {
            std::fs::copy(&source, target)
                .map(|_| ())
                .map_err(|error| error.to_string())
        });
    if let Err(error) = copied {
        message(
            &app,
            MessageDialogKind::Error,
            &format!("导出日志失败：{error}"),
        );
    }
}
