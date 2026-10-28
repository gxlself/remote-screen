//! 外壳设置 `desktop.json`：读写、校验、映射到控制器设置。纯逻辑 + 文件，不依赖 Tauri。

use std::{
    ffi::OsString,
    fs::{self, File},
    io::{self, Write},
    path::{Path, PathBuf},
};

use rs_desktop::{controller::ControllerSettings, keymap::MacKeyboardKind};
use serde::{Deserialize, Serialize};

/// 在 `app_local_data_dir()` 下。
pub const FILE_NAME: &str = "desktop.json";
pub const KEYBOARDS: [&str; 3] = ["ansi", "iso", "jis"];
pub const QUALITIES: [&str; 4] = ["auto", "clear", "smooth", "eco"];

/// 与前端 `DesktopSettings`（desktop/src/core/desktopApi.ts）逐字段一致。缺的字段取默认值，
/// 不认识的字段（例如旧版本的 `hubUrl`）忽略。
#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase", default)]
pub struct DesktopSettings {
    pub mac_cmd_to_ctrl: bool,
    pub win_swap_ctrl_meta: bool,
    /// ansi / iso / jis。
    pub mac_keyboard: String,
    /// auto / clear / smooth / eco。
    pub default_quality: String,
}

impl Default for DesktopSettings {
    fn default() -> Self {
        Self {
            mac_cmd_to_ctrl: true,
            win_swap_ctrl_meta: false,
            mac_keyboard: "ansi".to_owned(),
            default_quality: "auto".to_owned(),
        }
    }
}

impl DesktopSettings {
    /// 错误文字不带字段的值。
    pub fn validate(&self) -> Result<(), String> {
        if !KEYBOARDS.contains(&self.mac_keyboard.as_str()) {
            return Err("macKeyboard must be ansi, iso or jis".to_owned());
        }
        if !QUALITIES.contains(&self.default_quality.as_str()) {
            return Err("defaultQuality must be auto, clear, smooth or eco".to_owned());
        }
        Ok(())
    }
}

impl From<&DesktopSettings> for ControllerSettings {
    fn from(settings: &DesktopSettings) -> Self {
        Self {
            mac_cmd_to_ctrl: settings.mac_cmd_to_ctrl,
            win_swap_ctrl_meta: settings.win_swap_ctrl_meta,
            mac_keyboard: match settings.mac_keyboard.as_str() {
                "iso" => MacKeyboardKind::Iso,
                "jis" => MacKeyboardKind::Jis,
                _ => MacKeyboardKind::Ansi,
            },
        }
    }
}

/// 不存在 → 默认值。解析或校验失败 → 默认值，坏文件改名为 `desktop.json.bad`。
pub fn load(path: &Path) -> DesktopSettings {
    let bytes = match fs::read(path) {
        Ok(bytes) => bytes,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return DesktopSettings::default(),
        Err(error) => {
            tracing::warn!(%error, "desktop settings not readable; using defaults");
            return DesktopSettings::default();
        }
    };
    match serde_json::from_slice::<DesktopSettings>(&bytes) {
        Ok(settings) if settings.validate().is_ok() => settings,
        _ => {
            tracing::warn!("desktop settings invalid; using defaults");
            if let Err(error) = fs::rename(path, with_suffix(path, ".bad")) {
                tracing::warn!(%error, "could not set the bad desktop settings aside");
            }
            DesktopSettings::default()
        }
    }
}

/// 先写同目录的 `.tmp` 再改名，写一半时原文件不变。
pub fn save(path: &Path, settings: &DesktopSettings) -> io::Result<()> {
    if let Some(dir) = path.parent() {
        fs::create_dir_all(dir)?;
    }
    let bytes = serde_json::to_vec_pretty(settings).map_err(io::Error::other)?;
    let tmp = with_suffix(path, ".tmp");
    let saved = write_synced(&tmp, &bytes).and_then(|()| fs::rename(&tmp, path));
    if saved.is_err() {
        let _ = fs::remove_file(&tmp);
    }
    saved
}

fn write_synced(path: &Path, bytes: &[u8]) -> io::Result<()> {
    let mut file = File::create(path)?;
    file.write_all(bytes)?;
    file.sync_all()
}

/// `desktop.json` + `.bad` → `desktop.json.bad`。
fn with_suffix(path: &Path, suffix: &str) -> PathBuf {
    let mut name = OsString::from(path.as_os_str());
    name.push(suffix);
    PathBuf::from(name)
}

#[cfg(test)]
mod tests {
    use super::*;

    fn file_names(dir: &Path) -> Vec<String> {
        let mut names: Vec<String> = fs::read_dir(dir)
            .expect("read dir")
            .map(|entry| {
                entry
                    .expect("dir entry")
                    .file_name()
                    .to_string_lossy()
                    .into_owned()
            })
            .collect();
        names.sort();
        names
    }

    #[test]
    fn settings_defaults() {
        let dir = tempfile::tempdir().expect("tempdir");
        let settings = load(&dir.path().join(FILE_NAME));
        assert_eq!(
            settings,
            DesktopSettings {
                mac_cmd_to_ctrl: true,
                win_swap_ctrl_meta: false,
                mac_keyboard: "ansi".to_owned(),
                default_quality: "auto".to_owned(),
            }
        );
        assert!(file_names(dir.path()).is_empty());
    }

    #[test]
    fn settings_bad_file_renamed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, b"{\"macKeyboard\": \xff\x00 not json").expect("write");
        assert_eq!(load(&path), DesktopSettings::default());
        assert_eq!(file_names(dir.path()), ["desktop.json.bad"]);
    }

    #[test]
    fn settings_invalid_value_renamed() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, br#"{"macKeyboard": "dvorak"}"#).expect("write");
        assert_eq!(load(&path), DesktopSettings::default());
        assert_eq!(file_names(dir.path()), ["desktop.json.bad"]);
    }

    #[test]
    fn settings_missing_fields_default() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(FILE_NAME);
        fs::write(&path, br#"{"macKeyboard": "jis"}"#).expect("write");
        let settings = load(&path);
        assert_eq!(settings.mac_keyboard, "jis");
        assert!(settings.mac_cmd_to_ctrl);
        assert_eq!(settings.default_quality, "auto");
        assert_eq!(file_names(dir.path()), [FILE_NAME]);
    }

    /// 旧版本存的 `hubUrl`（包括当时就不合法的值）忽略，不当坏文件改名；再存一次就不带它了。
    #[test]
    fn settings_legacy_hub_url_ignored() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join(FILE_NAME);
        for legacy_url in [
            r#""http://hs.example.com:7300""#,
            r#""""#,
            "null",
            r#""rs-hub:7300""#,
        ] {
            let legacy = format!(
                r#"{{"hubUrl": {legacy_url}, "macKeyboard": "iso", "defaultQuality": "eco"}}"#
            );
            fs::write(&path, legacy).expect("write");
            assert_eq!(
                load(&path),
                DesktopSettings {
                    mac_keyboard: "iso".to_owned(),
                    default_quality: "eco".to_owned(),
                    ..DesktopSettings::default()
                },
                "{legacy_url}"
            );
            assert_eq!(file_names(dir.path()), [FILE_NAME], "{legacy_url}");
        }

        save(&path, &load(&path)).expect("save");
        let saved = fs::read_to_string(&path).expect("read");
        assert!(!saved.contains("hubUrl"), "{saved}");
    }

    #[test]
    fn settings_roundtrip_atomic() {
        let dir = tempfile::tempdir().expect("tempdir");
        let path = dir.path().join("nested").join(FILE_NAME);
        let first = DesktopSettings {
            mac_cmd_to_ctrl: false,
            win_swap_ctrl_meta: true,
            mac_keyboard: "iso".to_owned(),
            default_quality: "smooth".to_owned(),
        };
        save(&path, &first).expect("save");
        assert_eq!(load(&path), first);

        let second = DesktopSettings::default();
        save(&path, &second).expect("save over");
        assert_eq!(load(&path), second);
        assert_eq!(file_names(&dir.path().join("nested")), [FILE_NAME]);
    }

    #[test]
    fn settings_json_matches_frontend() {
        let value = serde_json::to_value(DesktopSettings::default()).expect("serialize");
        assert_eq!(
            value,
            serde_json::json!({
                "macCmdToCtrl": true,
                "winSwapCtrlMeta": false,
                "macKeyboard": "ansi",
                "defaultQuality": "auto",
            })
        );
    }

    #[test]
    fn settings_validate() {
        let with = |f: fn(&mut DesktopSettings)| {
            let mut settings = DesktopSettings::default();
            f(&mut settings);
            settings.validate()
        };
        assert!(with(|_| {}).is_ok());
        assert!(with(|s| s.mac_keyboard = "dvorak".to_owned()).is_err());
        assert!(with(|s| s.default_quality = "ultra".to_owned()).is_err());
        for keyboard in KEYBOARDS {
            let settings = DesktopSettings {
                mac_keyboard: keyboard.to_owned(),
                ..DesktopSettings::default()
            };
            assert!(settings.validate().is_ok(), "{keyboard}");
        }
        for quality in QUALITIES {
            let settings = DesktopSettings {
                default_quality: quality.to_owned(),
                ..DesktopSettings::default()
            };
            assert!(settings.validate().is_ok(), "{quality}");
        }
    }

    #[test]
    fn settings_to_controller() {
        let defaults = ControllerSettings::from(&DesktopSettings::default());
        assert_eq!(defaults, ControllerSettings::default());

        let settings = DesktopSettings {
            mac_cmd_to_ctrl: false,
            win_swap_ctrl_meta: true,
            mac_keyboard: "jis".to_owned(),
            ..DesktopSettings::default()
        };
        assert_eq!(
            ControllerSettings::from(&settings),
            ControllerSettings {
                mac_cmd_to_ctrl: false,
                win_swap_ctrl_meta: true,
                mac_keyboard: MacKeyboardKind::Jis,
            }
        );
        let iso = DesktopSettings {
            mac_keyboard: "iso".to_owned(),
            ..DesktopSettings::default()
        };
        assert_eq!(
            ControllerSettings::from(&iso).mac_keyboard,
            MacKeyboardKind::Iso
        );
    }
}
