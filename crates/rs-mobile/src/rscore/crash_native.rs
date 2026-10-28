//! Native crash inbox: files written by iOS/Android collectors, imported at Core start.

use std::fs;
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};

use serde_json::Value;

use super::crash::{
    self, CrashKind, CrashReport, MESSAGE_MAX, NAME_MAX, OS_VERSION_MAX, STACK_MAX,
};
use super::dto_core::CoreConfig;

pub const NATIVE_INBOX: &str = "native";
pub const NATIVE_INPUT_MAX: u64 = 256 * 1024;
pub const NATIVE_PER_INGEST: usize = 20;
pub const APP_VERSION_MAX: usize = 64;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq)]
pub struct IngestStats {
    pub ingested: u32,
    pub dropped: u32,
}

pub fn inbox_dir(crash_dir: &Path) -> PathBuf {
    crash_dir.join(NATIVE_INBOX)
}

pub fn native_report(json: &[u8], cfg: &CoreConfig) -> Option<CrashReport> {
    let value: Value = serde_json::from_slice(json).ok()?;
    let object = value.as_object()?;
    if object.get("v")?.as_u64()? != 1 {
        return None;
    }
    let incident_at = object.get("incidentAt")?.as_u64()?;
    if incident_at == 0 {
        return None;
    }
    let message = object.get("message")?.as_str()?;
    let optional_string = |key: &str| match object.get(key) {
        None | Some(Value::Null) => Some(None),
        Some(Value::String(value)) => Some(Some(value.as_str())),
        _ => None,
    };
    let sanitized = |value: Option<&str>, max| {
        value
            .map(|value| crash::sanitize(value, max))
            .filter(|value| !value.is_empty())
    };

    Some(CrashReport {
        v: crash::REPORT_VERSION,
        kind: CrashKind::NativeCrash,
        incident_at,
        app_version: sanitized(optional_string("appVersion")?, APP_VERSION_MAX)
            .unwrap_or_else(|| cfg.app_version.clone()),
        platform: cfg.platform,
        os_version: sanitized(optional_string("osVersion")?, OS_VERSION_MAX),
        thread: sanitized(optional_string("thread")?, NAME_MAX),
        location: None,
        name: sanitized(optional_string("name")?, NAME_MAX),
        message: crash::sanitize(message, MESSAGE_MAX),
        stack: sanitized(optional_string("stack")?, STACK_MAX),
    })
}

pub fn ingest_native(cfg: &CoreConfig, crash_dir: &Path, now_us: u64) -> IngestStats {
    let Ok(entries) = fs::read_dir(inbox_dir(crash_dir)) else {
        return IngestStats::default();
    };
    let system_now = SystemTime::now();
    let mut candidates = Vec::new();
    for entry in entries.flatten() {
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let bytes = name.as_encoded_bytes();
        if bytes.starts_with(b".") {
            let stale = match entry.metadata().and_then(|metadata| metadata.modified()) {
                Ok(modified) => system_now
                    .duration_since(modified)
                    .is_ok_and(|age| age >= crash::TMP_MAX_AGE),
                Err(_) => true,
            };
            if stale {
                let _ = fs::remove_file(entry.path());
            }
        } else if bytes.ends_with(b".json") {
            candidates.push((name, entry));
        }
    }
    candidates.sort_by(|left, right| left.0.cmp(&right.0));

    let mut stats = IngestStats::default();
    for (_, entry) in candidates.into_iter().take(NATIVE_PER_INGEST) {
        let path = entry.path();
        let report = entry
            .metadata()
            .ok()
            .filter(|metadata| metadata.len() <= NATIVE_INPUT_MAX)
            .and_then(|_| fs::read(&path).ok())
            .and_then(|bytes| native_report(&bytes, cfg))
            .filter(|report| now_us.saturating_sub(report.incident_at) <= crash::MAX_AGE_US);
        let Some(report) = report else {
            let _ = fs::remove_file(&path);
            stats.dropped += 1;
            continue;
        };
        if crash::write_report(crash_dir, &report).is_ok() {
            let _ = fs::remove_file(&path);
            stats.ingested += 1;
        }
    }
    if stats.ingested + stats.dropped > 0 {
        tracing::debug!(
            ingested = stats.ingested,
            dropped = stats.dropped,
            "native crash inbox processed"
        );
    }
    stats
}

/// Imports the native crash inbox once per Core start.
pub fn ingest_at_start(crash_dir: &Path, cfg: &CoreConfig) {
    let now_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .ok()
        .and_then(|elapsed| u64::try_from(elapsed.as_micros()).ok())
        .unwrap_or(0);
    if now_us == 0 {
        return;
    }
    let _ = ingest_native(cfg, crash_dir, now_us);
}
