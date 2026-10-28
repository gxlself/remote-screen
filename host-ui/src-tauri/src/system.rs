//! Small, side-effecting system integrations used by the web UI.

use std::path::Path;
use std::process::Command;

use tauri_plugin_notification::NotificationExt;

/// The two macOS preference panes exposed by the setup/settings pages.
#[allow(dead_code)]
pub(crate) const SYSTEM_SETTINGS_URLS: [&str; 2] = [
    "x-apple.systempreferences:com.apple.preference.security?Privacy_ScreenCapture",
    "x-apple.systempreferences:com.apple.preference.security?Privacy_Accessibility",
];

fn command_succeeded(command: &mut Command) -> Result<(), String> {
    let status = command
        .status()
        .map_err(|_| "system command failed".to_string())?;
    if status.success() {
        Ok(())
    } else {
        Err("system command failed".to_string())
    }
}

fn is_allowed_url(url: &str) -> bool {
    url.starts_with("x-apple.systempreferences:") || url.starts_with("https://")
}

#[tauri::command]
pub fn notify(app: tauri::AppHandle, title: String, body: String) -> Result<(), String> {
    if app
        .notification()
        .builder()
        .title(title)
        .body(body)
        .show()
        .is_err()
    {
        // Permission denial is expected on first launch and must not break the
        // agent/UI connection.  Do not include notification contents here.
        tracing::warn!("system notification unavailable");
    }
    Ok(())
}

#[tauri::command]
pub fn open_url(_app: tauri::AppHandle, url: String) -> Result<(), String> {
    if !is_allowed_url(&url) {
        return Err("url not allowed".to_string());
    }
    command_succeeded(Command::new("open").arg(url))
}

#[tauri::command]
pub fn reveal_path(_app: tauri::AppHandle, path: String) -> Result<(), String> {
    let path_ref = Path::new(&path);
    if !path_ref.is_absolute() || !path_ref.exists() {
        return Err("path must be an existing absolute path".to_string());
    }
    command_succeeded(Command::new("open").arg("-R").arg(path_ref))
}

#[tauri::command]
pub fn quit_app(app: tauri::AppHandle) -> Result<(), String> {
    app.exit(0);
    Ok(())
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn allowed_urls_are_limited_to_preferences_and_https() {
        assert!(SYSTEM_SETTINGS_URLS.iter().all(|url| is_allowed_url(url)));
        assert!(is_allowed_url("https://hs.example.com/x"));
        assert!(!is_allowed_url("http://hs.example.com/x"));
        assert!(!is_allowed_url("file:///etc/passwd"));
        assert!(!is_allowed_url("javascript:alert(1)"));
        assert!(!is_allowed_url(""));
    }
}
