//! 配置读写、默认值与热更新。见 docs/dev/06-host.md host-core。

use serde::{Deserialize, Serialize};
use serde_json::Value;
#[cfg(unix)]
use std::fs::File;
use std::fs::{self, OpenOptions};
use std::io::{self, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};
use thiserror::Error;
use tokio::sync::watch;

static TEMP_FILE_COUNTER: AtomicU64 = AtomicU64::new(0);

#[derive(Debug, Error)]
pub enum ConfigError {
    #[error("configuration I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("configuration TOML parse error: {0}")]
    Parse(#[from] toml::de::Error),
    #[error("configuration TOML serialization error: {0}")]
    Serialize(#[from] toml::ser::Error),
    #[error("invalid configuration field {field}: {reason}")]
    Invalid { field: String, reason: String },
    #[error("configuration JSON conversion error: {0}")]
    Json(#[from] serde_json::Error),
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoCodec {
    #[default]
    Auto,
    Hevc,
    H264,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum VideoEncoder {
    #[default]
    Auto,
    Nvenc,
    Amf,
    Qsv,
    Mf,
    OpenH264,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum UpdateChannel {
    #[default]
    Stable,
    Beta,
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(rename_all = "lowercase")]
pub enum LogLevel {
    Trace,
    Debug,
    #[default]
    Info,
    Warn,
    Error,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct AccessConfig {
    pub allow_remote_control: bool,
    pub allowed_users: Vec<String>,
    pub attended_mode: bool,
    pub max_viewers: u8,
    pub idle_timeout_min: u16,
    pub lock_on_end: bool,
    pub pause_on_local_input: bool,
}

impl Default for AccessConfig {
    fn default() -> Self {
        Self {
            allow_remote_control: true,
            allowed_users: Vec::new(),
            attended_mode: false,
            max_viewers: 3,
            idle_timeout_min: 30,
            lock_on_end: false,
            pause_on_local_input: true,
        }
    }
}

#[derive(Clone, Debug, Default, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct IntranetConfig {
    pub enabled: bool,
    pub interfaces: Vec<String>,
    pub allow_sources: Vec<String>,
    pub confirm_each: bool,
    pub allow_pairing: bool,
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct SecurityConfig {
    pub confirm_new_devices: bool,
    pub password_pairing: bool,
    pub password_pairing_permission: crate::pairing::PermissionName,
}

impl Default for SecurityConfig {
    fn default() -> Self {
        Self {
            confirm_new_devices: true,
            password_pairing: false,
            password_pairing_permission: crate::pairing::PermissionName::Control,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct ClipboardConfig {
    pub to_phone: bool,
    pub from_phone: bool,
}

impl Default for ClipboardConfig {
    fn default() -> Self {
        Self {
            to_phone: true,
            from_phone: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct VideoConfig {
    pub codec: VideoCodec,
    pub max_fps: u32,
    pub encoder: VideoEncoder,
}

impl Default for VideoConfig {
    fn default() -> Self {
        Self {
            codec: VideoCodec::Auto,
            max_fps: 60,
            encoder: VideoEncoder::Auto,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct UpdateConfig {
    pub channel: UpdateChannel,
    pub auto_install: bool,
}

impl Default for UpdateConfig {
    fn default() -> Self {
        Self {
            channel: UpdateChannel::Stable,
            auto_install: true,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize)]
#[serde(default)]
pub struct LogConfig {
    pub level: LogLevel,
}

impl Default for LogConfig {
    fn default() -> Self {
        Self {
            level: LogLevel::Info,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(default)]
pub struct FilesConfig {
    pub enabled: bool,             // true；总开关
    pub from_phone: bool,          // true；允许手机发文件到电脑
    pub to_phone: bool,            // true；允许手机从电脑取文件（FileRequest）
    pub browse: bool,              // true；允许 FileList
    pub dest_dir: String,          // ""；= HostServices.downloads_dir（Mac ~/Downloads）
    pub browse_roots: Vec<String>, // []；= 用户的下载、桌面、文稿目录（Windows Known Folder）
    pub max_file_mb: u64,          // 0；= 不另设上限（仍受 MAX_FILE_BYTES 1 TiB）
    pub partial_max_age_days: u32, // 7；启动时清理更旧的 .rs-*.part
}

impl Default for FilesConfig {
    fn default() -> Self {
        Self {
            enabled: true,
            from_phone: true,
            to_phone: true,
            browse: true,
            dest_dir: String::new(),
            browse_roots: Vec::new(),
            max_file_mb: 0,
            partial_max_age_days: 7,
        }
    }
}

#[derive(Clone, Debug, PartialEq, Serialize, Deserialize, Default)]
#[serde(default)]
pub struct HostConfig {
    pub access: AccessConfig,
    pub security: SecurityConfig,
    pub intranet: IntranetConfig,
    pub clipboard: ClipboardConfig,
    #[serde(default)]
    pub files: FilesConfig,
    pub video: VideoConfig,
    pub update: UpdateConfig,
    pub log: LogConfig,
}

pub type Access = AccessConfig;
pub type Security = SecurityConfig;
pub type Intranet = IntranetConfig;
pub type Clipboard = ClipboardConfig;
pub type Video = VideoConfig;
pub type Update = UpdateConfig;
pub type Log = LogConfig;

impl HostConfig {
    pub fn validate(&self) -> Result<(), ConfigError> {
        if !(1..=3).contains(&self.access.max_viewers) {
            return Err(invalid("access.max_viewers", "must be between 1 and 3"));
        }
        if self.access.idle_timeout_min > 1440 {
            return Err(invalid(
                "access.idle_timeout_min",
                "must be between 0 and 1440",
            ));
        }
        for (index, source) in self.intranet.allow_sources.iter().enumerate() {
            if !is_valid_cidr(source) {
                return Err(invalid(
                    format!("intranet.allow_sources[{index}]"),
                    "must be a valid IPv4 or IPv6 CIDR",
                ));
            }
        }
        Ok(())
    }
}

fn invalid(field: impl Into<String>, reason: impl Into<String>) -> ConfigError {
    ConfigError::Invalid {
        field: field.into(),
        reason: reason.into(),
    }
}

fn is_valid_cidr(value: &str) -> bool {
    let Some((address, prefix)) = value.rsplit_once('/') else {
        return false;
    };
    let Ok(prefix) = prefix.parse::<u16>() else {
        return false;
    };
    if address.parse::<Ipv4Addr>().is_ok() {
        prefix <= 32
    } else if address.parse::<Ipv6Addr>().is_ok() {
        prefix <= 128
    } else {
        false
    }
}

/// Loads a configuration. A missing file is initialized with the documented
/// defaults and written atomically before being returned.
pub fn load(path: impl AsRef<Path>) -> Result<HostConfig, ConfigError> {
    let path = path.as_ref();
    let text = match fs::read_to_string(path) {
        Ok(text) => text,
        Err(error) if error.kind() == io::ErrorKind::NotFound => {
            let config = HostConfig::default();
            save(path, &config)?;
            return Ok(config);
        }
        Err(error) => return Err(ConfigError::Io(error)),
    };
    let value: toml::Value = toml::from_str(&text)?;
    apply_patch(&HostConfig::default(), &serde_json::to_value(value)?)
}

/// Writes a configuration using a temporary file, fsync and rename.
pub fn save(path: impl AsRef<Path>, config: &HostConfig) -> Result<(), ConfigError> {
    config.validate()?;
    let path = path.as_ref();
    let text = toml::to_string_pretty(config)?;
    let parent = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let counter = TEMP_FILE_COUNTER.fetch_add(1, Ordering::Relaxed);
    let filename = path
        .file_name()
        .and_then(|name| name.to_str())
        .unwrap_or("config.toml");
    let temporary = parent.join(format!(".{filename}.tmp-{}-{counter}", std::process::id()));

    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let mut file = options.open(&temporary)?;
    let result = (|| -> io::Result<()> {
        file.write_all(text.as_bytes())?;
        file.sync_all()?;
        fs::rename(&temporary, path)?;
        #[cfg(unix)]
        File::open(parent)?.sync_all()?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    Ok(result?)
}

/// Merges JSON objects, checking every known leaf against the configuration
/// schema before deserializing so errors retain their complete field path.
pub fn apply_patch(base: &HostConfig, patch: &Value) -> Result<HostConfig, ConfigError> {
    if !patch.is_object() {
        return Err(invalid("$", "patch must be a JSON object"));
    }
    let mut merged = serde_json::to_value(base)?;
    merge_patch(&mut merged, patch, "")?;
    let config: HostConfig = serde_json::from_value(merged)?;
    config.validate()?;
    Ok(config)
}

fn merge_patch(current: &mut Value, patch: &Value, path: &str) -> Result<(), ConfigError> {
    let (Some(current), Some(patch)) = (current.as_object_mut(), patch.as_object()) else {
        return Err(invalid(path, "expected an object"));
    };
    for (key, value) in patch {
        let field = if path.is_empty() {
            key.clone()
        } else {
            format!("{path}.{key}")
        };
        let Some(existing) = current.get_mut(key) else {
            tracing::warn!(field = %field, "unknown configuration field ignored");
            continue;
        };
        if existing.is_object() {
            merge_patch(existing, value, &field)?;
        } else {
            validate_leaf(&field, value, existing)?;
            *existing = value.clone();
        }
    }
    Ok(())
}

fn validate_leaf(field: &str, value: &Value, existing: &Value) -> Result<(), ConfigError> {
    let expected = match existing {
        Value::Bool(_) if value.is_boolean() => "boolean",
        Value::String(_) if value.is_string() => "string",
        Value::Number(_) if value.as_u64().is_some() => "unsigned integer",
        Value::Array(_) if value.is_array() => "array of strings",
        Value::Bool(_) => return Err(invalid(field, "expected a boolean")),
        Value::String(_) => return Err(invalid(field, "expected a string")),
        Value::Number(_) => return Err(invalid(field, "expected an unsigned integer")),
        Value::Array(_) => return Err(invalid(field, "expected an array of strings")),
        _ => return Err(invalid(field, "unsupported configuration type")),
    };
    if expected == "array of strings" {
        if let Some(items) = value.as_array() {
            for (index, item) in items.iter().enumerate() {
                if !item.is_string() {
                    return Err(invalid(format!("{field}[{index}]"), "expected a string"));
                }
            }
        }
    }
    let maximum = match field {
        "access.max_viewers" => Some(u8::MAX as u64),
        "access.idle_timeout_min" => Some(u16::MAX as u64),
        "video.max_fps" => Some(u32::MAX as u64),
        _ => None,
    };
    if let (Some(maximum), Some(number)) = (maximum, value.as_u64()) {
        if number > maximum {
            return Err(invalid(field, format!("integer exceeds {maximum}")));
        }
    }
    let allowed: Option<&[&str]> = match field {
        "security.password_pairing_permission" => Some(&["view_only", "control", "unattended"]),
        "video.codec" => Some(&["auto", "hevc", "h264"]),
        "video.encoder" => Some(&["auto", "nvenc", "amf", "qsv", "mf", "openh264"]),
        "update.channel" => Some(&["stable", "beta"]),
        "log.level" => Some(&["trace", "debug", "info", "warn", "error"]),
        _ => None,
    };
    if let (Some(allowed), Some(value)) = (allowed, value.as_str()) {
        if !allowed.contains(&value) {
            return Err(invalid(
                field,
                format!("must be one of {}", allowed.join(", ")),
            ));
        }
    }
    Ok(())
}

/// Serializes concurrent updates and publishes only configurations which have
/// already passed validation and been saved successfully.
#[derive(Clone)]
pub struct ConfigStore {
    path: PathBuf,
    update_lock: Arc<Mutex<()>>,
    sender: watch::Sender<Arc<HostConfig>>,
}

impl ConfigStore {
    pub fn new(path: impl Into<PathBuf>, config: HostConfig) -> Self {
        let (sender, _) = watch::channel(Arc::new(config));
        Self {
            path: path.into(),
            update_lock: Arc::new(Mutex::new(())),
            sender,
        }
    }

    pub fn load(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        let config = load(&path)?;
        Ok(Self::new(path.as_ref().to_path_buf(), config))
    }

    pub fn from_path(path: impl AsRef<Path>) -> Result<Self, ConfigError> {
        Self::load(path)
    }

    pub fn get(&self) -> HostConfig {
        self.sender.borrow().as_ref().clone()
    }

    pub fn update(&self, patch: &Value) -> Result<HostConfig, ConfigError> {
        let _lock = self
            .update_lock
            .lock()
            .map_err(|_| invalid("$", "configuration update lock poisoned"))?;
        let updated = apply_patch(&self.get(), patch)?;
        save(&self.path, &updated)?;
        self.sender.send_replace(Arc::new(updated.clone()));
        Ok(updated)
    }

    pub fn subscribe(&self) -> watch::Receiver<Arc<HostConfig>> {
        self.sender.subscribe()
    }
}
