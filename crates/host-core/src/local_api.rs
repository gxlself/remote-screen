//! Platform-independent implementations of the host's local IPC operations.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use serde_json::{json, Value};
use time::{OffsetDateTime, UtcOffset};

use crate::audit::AuditQueue;
use crate::config::{ConfigError, ConfigStore};
use crate::ipc::{
    AuditListParams, AuditListResult, EmptyResult, IpcError, LogsExportParams, LogsExportResult,
    UpdateSetChannelParams, UpdateSetChannelResult,
};
use crate::logs;

pub const AUDIT_PAGE_DEFAULT: u32 = 50;
pub const AUDIT_PAGE_MAX: u32 = 200;

/// Export directory provider. `None` means no interactive user has a writable
/// download directory at the moment.
pub type ExportDir = Arc<dyn Fn() -> Option<PathBuf> + Send + Sync>;

#[derive(Clone)]
pub struct LocalApi {
    config: ConfigStore,
    audit: Option<AuditQueue>,
    logs: Option<LogsWiring>,
    locked: Vec<String>,
    force_mask: bool,
}

#[derive(Clone)]
struct LogsWiring {
    logs_dir: PathBuf,
    export_dir: ExportDir,
}

impl LocalApi {
    pub fn new(config: ConfigStore) -> Self {
        Self {
            config,
            audit: None,
            logs: None,
            locked: Vec::new(),
            force_mask: false,
        }
    }

    pub fn with_audit(mut self, audit: AuditQueue) -> Self {
        self.audit = Some(audit);
        self
    }

    pub fn with_logs(mut self, logs_dir: PathBuf, export_dir: ExportDir) -> Self {
        self.logs = Some(LogsWiring {
            logs_dir,
            export_dir,
        });
        self
    }

    pub fn with_locked_settings(mut self, keys: &[&str]) -> Self {
        self.locked = keys.iter().map(|key| (*key).to_owned()).collect();
        self
    }

    pub fn with_forced_mask(mut self) -> Self {
        self.force_mask = true;
        self
    }

    pub async fn settings_get(&self) -> Result<Value, IpcError> {
        serde_json::to_value(self.config.get())
            .map_err(|_| IpcError::business("internal", "failed to encode settings"))
    }

    pub async fn settings_set(&self, patch: Value) -> Result<Value, IpcError> {
        if !patch.is_object() {
            return Err(IpcError::business(
                "invalid_params",
                "settings patch must be an object",
            ));
        }

        let current = self.config.get();
        let current_value = serde_json::to_value(current)
            .map_err(|_| IpcError::business("internal", "failed to encode settings"))?;
        if let Some(key) = locked_change(&current_value, &patch, &self.locked) {
            return Err(IpcError::business(
                "forbidden",
                format!("{key} 只能由管理员在 config.toml 中修改"),
            ));
        }

        let config = self.config.clone();
        let updated = tokio::task::spawn_blocking(move || config.update(&patch))
            .await
            .map_err(|_| IpcError::business("internal", "configuration update task failed"))?
            .map_err(config_error)?;
        serde_json::to_value(updated)
            .map_err(|_| IpcError::business("internal", "failed to encode settings"))
    }

    pub async fn update_set_channel(
        &self,
        params: UpdateSetChannelParams,
    ) -> Result<UpdateSetChannelResult, IpcError> {
        let channel = serde_json::to_value(params.channel)
            .map_err(|_| IpcError::business("internal", "failed to encode update channel"))?;
        self.settings_set(json!({ "update": { "channel": channel } }))
            .await?;
        Ok(EmptyResult {})
    }

    pub async fn logs_export(
        &self,
        params: LogsExportParams,
    ) -> Result<LogsExportResult, IpcError> {
        let Some(logs) = self.logs.clone() else {
            return Err(IpcError::business(
                "unsupported",
                "logs.export is not wired in this agent",
            ));
        };
        let Some(export_dir) = (logs.export_dir)() else {
            return Err(IpcError::business(
                "unavailable",
                "no user download directory",
            ));
        };
        let now_us = rsp_core::clock::unix_micros();
        let Some(output) = next_log_path(&export_dir, now_us) else {
            return Err(IpcError::business(
                "io",
                "unable to choose log archive path",
            ));
        };
        let mask = self.force_mask || params.mask;
        let logs_dir = logs.logs_dir;
        let written = tokio::task::spawn_blocking(move || logs::export(&logs_dir, &output, mask))
            .await
            .map_err(|_| IpcError::business("internal", "log export task failed"))?
            .map_err(|_| IpcError::business("io", "failed to export logs"))?;
        Ok(LogsExportResult {
            path: written.to_string_lossy().into_owned(),
        })
    }

    pub async fn audit_list(&self, params: AuditListParams) -> Result<AuditListResult, IpcError> {
        let Some(audit) = &self.audit else {
            return Ok(AuditListResult {
                entries: Vec::new(),
            });
        };
        let limit = params
            .limit
            .unwrap_or(AUDIT_PAGE_DEFAULT)
            .clamp(1, AUDIT_PAGE_MAX) as usize;
        let entries = audit
            .history(limit, params.before_us)
            .into_iter()
            .map(|event| {
                serde_json::to_value(event)
                    .map_err(|_| IpcError::business("internal", "failed to encode audit entry"))
            })
            .collect::<Result<Vec<_>, _>>()?;
        Ok(AuditListResult { entries })
    }
}

fn config_error(error: ConfigError) -> IpcError {
    match error {
        ConfigError::Invalid { field, reason } => {
            IpcError::business("invalid_params", format!("{field}: {reason}"))
        }
        ConfigError::Json(_) | ConfigError::Parse(_) => {
            IpcError::business("invalid_params", "invalid configuration")
        }
        ConfigError::Io(_) | ConfigError::Serialize(_) => {
            IpcError::business("io", "failed to save configuration")
        }
    }
}

fn next_log_path(export_dir: &Path, now_us: u64) -> Option<PathBuf> {
    let base = logs_zip_name(now_us);
    let stem = base.strip_suffix(".zip").unwrap_or("RemoteHost-logs");
    (1..=99)
        .map(|number| {
            let name = if number == 1 {
                base.clone()
            } else {
                format!("{stem}-{number}.zip")
            };
            export_dir.join(name)
        })
        .find(|path| !path.exists())
}

/// Formats a UTC timestamp as `RemoteHost-logs-<yyyyMMdd-HHmmss>.zip`.
pub fn logs_zip_name(unix_us: u64) -> String {
    let seconds = (unix_us / 1_000_000).min(i64::MAX as u64) as i64;
    let timestamp = match OffsetDateTime::from_unix_timestamp(seconds) {
        Ok(timestamp) => timestamp.to_offset(UtcOffset::UTC),
        Err(_) => OffsetDateTime::UNIX_EPOCH,
    };
    let date = timestamp.date();
    let clock = timestamp.time();
    format!(
        "RemoteHost-logs-{:04}{:02}{:02}-{:02}{:02}{:02}.zip",
        date.year(),
        date.month() as u8,
        date.day(),
        clock.hour(),
        clock.minute(),
        clock.second()
    )
}

/// Returns the first locked setting changed by a patch, if any.
pub fn locked_change(current: &Value, patch: &Value, locked: &[String]) -> Option<String> {
    for key in locked {
        let parts: Vec<_> = key.split('.').collect();
        let Some(last) = parts.last().copied() else {
            continue;
        };
        let Some(patch_parent) = lookup_parent(patch, &parts) else {
            continue;
        };
        let Some(patch_value) = patch_parent.get(last) else {
            continue;
        };
        let Some(current_parent) = lookup_parent(current, &parts) else {
            continue;
        };
        if current_parent.get(last) != Some(patch_value) {
            return Some(key.clone());
        }
    }
    None
}

fn lookup_parent<'a>(
    root: &'a Value,
    parts: &[&str],
) -> Option<&'a serde_json::Map<String, Value>> {
    let (_, parents) = parts.split_last()?;
    let mut value = root;
    for part in parents {
        value = value.as_object()?.get(*part)?;
    }
    value.as_object()
}
