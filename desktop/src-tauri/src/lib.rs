//! 电脑端 Tauri 外壳：启动 RsCore 门面与会话控制器，转发事件，菜单与设置。

mod commands;
mod menu;
mod menu_model;
mod observer;
mod settings;

use std::{
    collections::HashSet,
    path::PathBuf,
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard, PoisonError,
    },
};

use rs_desktop::{
    bridge::{self, Bridge, DesktopConfig},
    controller::DesktopSessions,
    platform::{
        self, PlatformError, SessionMenu, SessionWindow, SessionWindowFactory, VideoCodec,
        WindowEvents, WindowSpec,
    },
};
use serde_json::Value;
use tauri::{App, AppHandle, Manager, RunEvent, Window, WindowEvent};
use tauri_plugin_dialog::{DialogExt, MessageDialogKind};

use crate::{
    observer::{EventForwarder, Observer},
    settings::DesktopSettings,
};

pub(crate) const MAIN_WINDOW: &str = "main";
pub(crate) const MAC: bool = cfg!(target_os = "macos");

pub(crate) fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(PoisonError::into_inner)
}

pub(crate) struct AppState {
    pub bridge: Arc<Bridge>,
    pub sessions: Arc<DesktopSessions>,
    pub settings: Mutex<DesktopSettings>,
    /// `app_local_data_dir()`，放 `desktop.json`。
    pub local_dir: PathBuf,
    /// RsCore 数据目录 `<local_dir>/core`。
    pub core_dir: PathBuf,
    /// false：本平台没有会话窗口工厂。
    pub session_windows: bool,
    /// 已注册 `tauri-plugin-updater`。
    pub updater: bool,
    pub focused_session: Mutex<Option<(u64, SessionMenu)>>,
    /// 已结束但窗口还开着（显示结束文字）的会话。
    pub ended: Mutex<HashSet<u64>>,
    pub menus: Mutex<Option<menu::Handles>>,
    shut_down: AtomicBool,
}

impl AppState {
    /// 有没结束的会话。顺带清掉已移出窗口表的会话。
    pub fn has_live_sessions(&self) -> bool {
        let sessions = self.sessions.sessions();
        let mut ended = lock(&self.ended);
        ended.retain(|id| sessions.contains(id));
        sessions.iter().any(|id| !ended.contains(id))
    }

    /// 已开始退出：会话结束时不再把主窗口叫出来。
    pub fn shutting_down(&self) -> bool {
        self.shut_down.load(Ordering::Acquire)
    }
}

pub fn run() {
    let app = tauri::Builder::default()
        .plugin(tauri_plugin_single_instance::init(|app, _args, _cwd| {
            show_main(app);
        }))
        .plugin(tauri_plugin_dialog::init())
        .enable_macos_default_menu(false)
        .invoke_handler(tauri::generate_handler![
            commands::rscore_call,
            commands::desktop_info,
            commands::desktop_open_session,
            commands::desktop_settings_get,
            commands::desktop_settings_set,
        ])
        .on_menu_event(|app, event| menu::on_event(app, event.id().as_ref()))
        .on_window_event(on_window_event)
        .setup(setup)
        .build(tauri::generate_context!());
    match app {
        Ok(app) => app.run(on_run_event),
        Err(error) => {
            tracing::error!(%error, "Remote Screen failed to start");
        }
    }
}

/// 主线程。顺序：数据目录 → 设置 → 签名器 → 窗口工厂 → Bridge → 会话 → 状态 → 菜单。
fn setup(app: &mut App) -> Result<(), Box<dyn std::error::Error>> {
    let local_dir = app.path().app_local_data_dir()?;
    let core_dir = bridge::core_dir(&local_dir);
    let settings = settings::load(&local_dir.join(settings::FILE_NAME));

    let signer = match platform::device_signer(&core_dir) {
        Ok(signer) => Some(signer),
        Err(error) => {
            // 前端注册时收到「device signer not registered」，走注册失败提示。
            tracing::error!(%error, "device signer unavailable");
            None
        }
    };
    // Windows 的工厂要在主线程上建消息窗口，必须在这里取。
    let factory = platform::window_factory();
    let session_windows = factory.is_some();
    let factory =
        factory.unwrap_or_else(|| -> Arc<dyn SessionWindowFactory> { Arc::new(NoSessionWindows) });

    let config = DesktopConfig {
        data_dir: core_dir.clone(),
        app_version: app.package_info().version.to_string(),
        platform: bridge::default_platform(),
    };
    let bridge = match Bridge::start(config, signer, Default::default()) {
        Ok(bridge) => bridge,
        Err(error) => {
            fail_start(app.handle(), &error.inner.message);
            return Ok(());
        }
    };
    let handle = app.handle().clone();
    bridge.subscribe(Arc::new(EventForwarder {
        app: handle.clone(),
    }));
    let sessions = DesktopSessions::new(
        bridge.clone(),
        factory,
        (&settings).into(),
        Arc::new(Observer { app: handle }),
    );

    let updater = updater_configured(app.config().plugins.0.get("updater"));
    if updater {
        app.handle()
            .plugin(tauri_plugin_updater::Builder::new().build())?;
    }

    app.manage(AppState {
        bridge,
        sessions,
        settings: Mutex::new(settings),
        local_dir,
        core_dir,
        session_windows,
        updater,
        focused_session: Mutex::new(None),
        ended: Mutex::new(HashSet::new()),
        menus: Mutex::new(None),
        shut_down: AtomicBool::new(false),
    });
    let state = app.state::<AppState>();
    let menus = menu::install(app.handle(), &state)?;
    *lock(&state.menus) = Some(menus);
    Ok(())
}

/// RsCore 起不来：隐藏主窗口，弹错误框，用户确认后退出。
fn fail_start(app: &AppHandle, message: &str) {
    tracing::error!("RsCore failed to start");
    if let Some(main) = app.get_webview_window(MAIN_WINDOW) {
        let _ = main.hide();
    }
    let handle = app.clone();
    app.dialog()
        .message(format!("Remote Screen 无法启动：{message}"))
        .title("Remote Screen")
        .kind(MessageDialogKind::Error)
        .show(move |_| handle.exit(1));
}

/// `plugins.updater.pubkey` 非空才注册更新器。
fn updater_configured(config: Option<&Value>) -> bool {
    config
        .and_then(|updater| updater.get("pubkey"))
        .and_then(Value::as_str)
        .is_some_and(|key| !key.trim().is_empty())
}

pub(crate) fn show_main(app: &AppHandle) {
    if let Some(main) = app.get_webview_window(MAIN_WINDOW) {
        let _ = main.unminimize();
        let _ = main.show();
        let _ = main.set_focus();
    }
}

/// 主窗口关闭：还有会话 → 只隐藏；没有 → 退出。
fn on_window_event(window: &Window, event: &WindowEvent) {
    if window.label() != MAIN_WINDOW {
        return;
    }
    if let WindowEvent::CloseRequested { api, .. } = event {
        api.prevent_close();
        let live = window
            .try_state::<AppState>()
            .is_some_and(|state| state.has_live_sessions());
        if live {
            let _ = window.hide();
        } else {
            window.app_handle().exit(0);
        }
    }
}

fn on_run_event(app: &AppHandle, event: RunEvent) {
    match event {
        RunEvent::ExitRequested { .. } | RunEvent::Exit => shutdown(app),
        #[cfg(target_os = "macos")]
        RunEvent::Reopen { .. } => show_main(app),
        _ => {}
    }
}

/// 关掉所有会话，再停 RsCore。只做一次。
fn shutdown(app: &AppHandle) {
    let Some(state) = app.try_state::<AppState>() else {
        return;
    };
    if state.shut_down.swap(true, Ordering::AcqRel) {
        return;
    }
    state.sessions.close_all();
    state.bridge.shutdown();
}

/// 没有平台会话窗口时（Linux 开发）的工厂。`desktop_open_session` 先报不支持，不会走到这里。
struct NoSessionWindows;

impl SessionWindowFactory for NoSessionWindows {
    fn create(
        &self,
        _spec: &WindowSpec,
        _events: Arc<dyn WindowEvents>,
    ) -> Result<Arc<dyn SessionWindow>, PlatformError> {
        Err(PlatformError::Unsupported(
            commands::NO_SESSION_WINDOWS.to_owned(),
        ))
    }

    fn can_decode(&self, _codec: VideoCodec) -> bool {
        false
    }

    fn screen_pixel_width(&self) -> u32 {
        1920
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn updater_needs_pubkey() {
        assert!(!updater_configured(None));
        let shipped: Value =
            serde_json::from_str(include_str!("../tauri.conf.json")).expect("tauri.conf.json");
        assert!(!updater_configured(shipped["plugins"].get("updater")));
        assert!(!updater_configured(Some(
            &serde_json::json!({"pubkey": "  "})
        )));
        assert!(updater_configured(Some(&serde_json::json!({
            "pubkey": "dW50cnVzdGVkIGNvbW1lbnQ6IG1pbmlzaWdu",
            "endpoints": [],
        }))));
    }

    #[test]
    fn no_session_windows_factory() {
        let factory = NoSessionWindows;
        assert!(!factory.can_decode(VideoCodec::Hevc));
        assert!(!factory.can_decode(VideoCodec::H264));
    }
}
