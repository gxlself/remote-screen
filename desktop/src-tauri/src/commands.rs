//! 前端可调的外壳命令（名字与载荷见 desktop/src/core/{bindings,desktopApi}.ts）。错误一律是 `CallError`。

use rs_desktop::{
    bridge::{CallError, CallErrorInner},
    controller::OpenRequest,
    platform::HostOs,
};
use serde::{Deserialize, Serialize};
use serde_json::Value;
use tauri::{AppHandle, State};

use crate::{
    lock, menu,
    settings::{self, DesktopSettings},
    AppState,
};

pub const NO_SESSION_WINDOWS: &str = "会话窗口只在 macOS / Windows 上可用";
const FALLBACK_DEVICE_NAME: &str = "我的电脑";

#[derive(Debug, Clone, Serialize)]
#[serde(rename_all = "camelCase")]
pub struct DesktopInfo {
    pub version: String,
    /// macos / windows。
    pub platform: &'static str,
    pub device_name: String,
    pub data_dir: String,
}

#[derive(Debug, Clone, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct OpenSessionArgs {
    pub host_id: String,
    pub host_name: String,
    /// macos / windows / unknown。
    pub host_os: String,
    pub control: bool,
    #[serde(default)]
    pub display_id: Option<u32>,
    #[serde(default)]
    pub quality: Option<String>,
}

pub fn internal_error(message: impl Into<String>) -> CallError {
    CallError {
        tag: "Internal".to_owned(),
        inner: CallErrorInner {
            message: message.into(),
            status: None,
            hub_code: None,
            current_json: None,
        },
    }
}

#[tauri::command]
pub async fn rscore_call(
    state: State<'_, AppState>,
    name: String,
    args: Value,
) -> Result<Value, CallError> {
    state.bridge.call(&name, args).await
}

#[tauri::command]
pub async fn desktop_info(
    app: AppHandle,
    state: State<'_, AppState>,
) -> Result<DesktopInfo, CallError> {
    let device_name = tauri::async_runtime::spawn_blocking(device_name)
        .await
        .unwrap_or_else(|_| FALLBACK_DEVICE_NAME.to_owned());
    Ok(DesktopInfo {
        version: app.package_info().version.to_string(),
        platform: if cfg!(windows) { "windows" } else { "macos" },
        device_name,
        data_dir: state.core_dir.display().to_string(),
    })
}

/// `factory.create` 要等主线程建窗口，所以 `open` 放到阻塞线程上，不能在主线程调。
#[tauri::command]
pub async fn desktop_open_session(
    state: State<'_, AppState>,
    req: OpenSessionArgs,
) -> Result<String, CallError> {
    if !state.session_windows {
        return Err(CallError::unsupported(NO_SESSION_WINDOWS));
    }
    let quality = req
        .quality
        .or_else(|| Some(lock(&state.settings).default_quality.clone()));
    let request = OpenRequest {
        host_id: req.host_id,
        host_name: req.host_name,
        host: HostOs::from_os_name(&req.host_os),
        control: req.control,
        display_id: req.display_id,
        quality,
    };
    let sessions = state.sessions.clone();
    tauri::async_runtime::spawn_blocking(move || sessions.open(request))
        .await
        .map_err(|_| internal_error("open session task failed"))?
}

#[tauri::command]
pub fn desktop_settings_get(state: State<'_, AppState>) -> DesktopSettings {
    lock(&state.settings).clone()
}

/// 只影响之后打开的会话。
#[tauri::command]
pub fn desktop_settings_set(
    app: AppHandle,
    state: State<'_, AppState>,
    settings: DesktopSettings,
) -> Result<(), CallError> {
    apply_settings(&app, &state, settings)
}

/// 校验 → 原子写入 `desktop.json` → 交给控制器 → 重建「设置」菜单。
pub fn apply_settings(
    app: &AppHandle,
    state: &AppState,
    settings: DesktopSettings,
) -> Result<(), CallError> {
    settings
        .validate()
        .map_err(|why| CallError::bad_args("desktop_settings_set", &why))?;
    settings::save(&state.local_dir.join(settings::FILE_NAME), &settings)
        .map_err(|error| internal_error(format!("save desktop settings: {error}")))?;
    state.sessions.set_settings((&settings).into());
    *lock(&state.settings) = settings;
    menu::refresh_settings(app);
    Ok(())
}

fn device_name() -> String {
    device_name_from(read_device_name)
}

/// 读不到或为空 → 「我的电脑」。
fn device_name_from(read: impl FnOnce() -> Option<String>) -> String {
    read()
        .map(|name| name.trim().to_owned())
        .filter(|name| !name.is_empty())
        .unwrap_or_else(|| FALLBACK_DEVICE_NAME.to_owned())
}

#[cfg(target_os = "macos")]
fn read_device_name() -> Option<String> {
    let output = std::process::Command::new("/usr/sbin/scutil")
        .args(["--get", "ComputerName"])
        .output()
        .ok()?;
    output
        .status
        .success()
        .then(|| String::from_utf8_lossy(&output.stdout).into_owned())
}

#[cfg(windows)]
fn read_device_name() -> Option<String> {
    std::env::var("COMPUTERNAME").ok()
}

#[cfg(not(any(target_os = "macos", windows)))]
fn read_device_name() -> Option<String> {
    None
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn device_name_fallback() {
        assert_eq!(device_name_from(|| None), "我的电脑");
        assert_eq!(device_name_from(|| Some(" \n".to_owned())), "我的电脑");
        assert_eq!(
            device_name_from(|| Some("Studio Mac\n".to_owned())),
            "Studio Mac"
        );
    }

    #[test]
    fn open_session_args_from_frontend() {
        let args: OpenSessionArgs = serde_json::from_value(serde_json::json!({
            "hostId": "h7v2m9q3k8x4tb1a",
            "hostName": "Studio",
            "hostOs": "macos",
            "control": false,
            "quality": "eco",
        }))
        .expect("deserialize");
        assert_eq!(args.host_id, "h7v2m9q3k8x4tb1a");
        assert_eq!(args.host_name, "Studio");
        assert_eq!(HostOs::from_os_name(&args.host_os), HostOs::Mac);
        assert!(!args.control);
        assert_eq!(args.display_id, None);
        assert_eq!(args.quality.as_deref(), Some("eco"));
    }

    #[test]
    fn desktop_info_json_matches_frontend() {
        let info = DesktopInfo {
            version: "0.1.0".to_owned(),
            platform: "macos",
            device_name: "我的电脑".to_owned(),
            data_dir: "/tmp/core".to_owned(),
        };
        assert_eq!(
            serde_json::to_value(info).expect("serialize"),
            serde_json::json!({
                "version": "0.1.0",
                "platform": "macos",
                "deviceName": "我的电脑",
                "dataDir": "/tmp/core",
            })
        );
    }
}
