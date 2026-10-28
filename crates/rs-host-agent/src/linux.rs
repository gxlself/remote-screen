#![cfg(target_os = "linux")]

//! Linux-specific wiring for the host agent.

use std::path::{Path, PathBuf};
use std::time::Duration;

use anyhow::Result;
use host_core::update::agent::UpdatePlatform;
use tracing::info;

/// Parse the XDG download directory entry from a `user-dirs.dirs` file.
pub(crate) fn parse_download_dir(text: &str, home: &Path) -> Option<PathBuf> {
    let value = text.lines().find_map(|line| {
        let line = line.trim();
        if line.is_empty() || line.starts_with('#') {
            return None;
        }
        let (key, value) = line.split_once('=')?;
        (key.trim() == "XDG_DOWNLOAD_DIR").then_some(value.trim())
    })?;
    let value = if let Some(quoted) = value.strip_prefix('"') {
        let mut chars = quoted.char_indices();
        let end = loop {
            let (index, ch) = chars.next()?;
            match ch {
                '\\' => {
                    chars.next()?;
                }
                '"' => break index,
                _ => {}
            }
        };
        let (value, suffix) = quoted.split_at(end);
        let suffix = suffix.strip_prefix('"')?.trim();
        if !suffix.is_empty() && !suffix.starts_with('#') {
            return None;
        }
        value
    } else {
        value
            .split_once('#')
            .map_or(value, |(value, _)| value)
            .trim()
    };

    let relative = value.strip_prefix("$HOME/");
    let path = relative.unwrap_or(value);
    if relative.is_none() && !Path::new(path).is_absolute() {
        return None;
    }

    // Decode the escapes written by xdg-user-dirs without evaluating shell code.
    let mut decoded = String::new();
    let mut chars = path.chars();
    while let Some(ch) = chars.next() {
        if ch == '\\' {
            let escaped = chars.next()?;
            if !matches!(escaped, '"' | '\\' | '$' | '\x60') {
                decoded.push('\\');
            }
            decoded.push(escaped);
        } else {
            decoded.push(ch);
        }
    }

    if relative.is_some() {
        let relative = decoded.trim_start_matches('/');
        (!relative.is_empty()).then(|| home.join(relative))
    } else {
        Some(PathBuf::from(decoded))
    }
}

/// Resolve the user's XDG download directory, falling back to `~/Downloads`.
pub(crate) fn downloads_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    let config_home = std::env::var_os("XDG_CONFIG_HOME")
        .filter(|value| !value.is_empty())
        .map(PathBuf::from)
        .unwrap_or_else(|| home.join(".config"));
    let user_dirs = config_home.join("user-dirs.dirs");
    std::fs::read_to_string(user_dirs)
        .ok()
        .and_then(|text| parse_download_dir(&text, &home))
        .unwrap_or_else(|| home.join("Downloads"))
}

/// Construct the Linux platform and wait briefly for a display/portal grant.
pub(crate) async fn platform(
    state_dir: PathBuf,
    view_only: bool,
) -> Result<host_linux::platform::LinuxPlatform> {
    let options = host_linux::platform::LinuxOptions::from_env(state_dir, view_only);
    let platform = host_linux::platform::LinuxPlatform::new(options).await?;
    if !platform.wait_ready(Duration::from_secs(5)).await {
        info!("等待屏幕共享授权（Wayland 首次需要在电脑前点允许）");
    }
    Ok(platform)
}

/// Return the AppImage update platform when this executable uses an installed layout.
pub(crate) fn update_platform() -> Option<UpdatePlatform> {
    let platform = std::env::current_exe().ok().and_then(|exe| {
        let home = std::env::var_os("HOME").filter(|value| !value.is_empty())?;
        let lib_dir = host_linux::update_appimage::default_lib_dir(Path::new(&home));
        host_linux::update_appimage::update_platform(&exe, &lib_dir)
    });
    if platform.is_none() {
        info!(
            error_kind = "not_installed",
            "automatic updates disabled: not running from an installed AppImage"
        );
    }
    platform
}
