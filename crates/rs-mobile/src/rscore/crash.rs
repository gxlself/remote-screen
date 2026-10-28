//! App crash reports: on-disk store, scrubbing, JS fatal errors.

use std::fs;
use std::io::{self, Write};
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU32, AtomicUsize, Ordering};
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use ring::rand::{SecureRandom, SystemRandom};
use serde::{Deserialize, Serialize};
use serde_json::Value;

use super::dto_core::DevicePlatform;
use super::logging;
use super::runtime::{core, Core};

pub const REPORT_VERSION: u32 = 1;
pub const MAX_REPORTS: usize = 10;
pub const MAX_AGE_US: u64 = 7 * 24 * 3600 * 1_000_000;
pub const TMP_MAX_AGE: Duration = Duration::from_secs(60);
pub const NAME_MAX: usize = 128;
pub const MESSAGE_MAX: usize = 1024;
pub const STACK_MAX: usize = 16 * 1024;
pub const OS_VERSION_MAX: usize = 64;
pub const REPORT_FILE_MAX: u64 = 64 * 1024;
pub const JS_INPUT_MAX: usize = 256 * 1024;
pub const JS_PER_CORE: usize = 3;
pub const JS_ERROR_PER_CORE: usize = 3;

#[derive(Debug, Clone, Copy, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub enum CrashKind {
    RustPanic,
    JsFatal,
    JsError,
    NativeCrash,
}

#[derive(Debug, Clone, PartialEq, Eq, Serialize, Deserialize)]
#[serde(rename_all = "camelCase")]
pub struct CrashReport {
    pub v: u32,
    pub kind: CrashKind,
    pub incident_at: u64,
    pub app_version: String,
    pub platform: DevicePlatform,
    pub os_version: Option<String>,
    pub thread: Option<String>,
    pub location: Option<String>,
    pub name: Option<String>,
    pub message: String,
    pub stack: Option<String>,
}

#[derive(Default)]
struct CrashState {
    js: AtomicUsize,
    js_error: AtomicUsize,
}

static FALLBACK_RANDOM: AtomicU32 = AtomicU32::new(0);

/// Replace URLs, home-directory paths, quoted text, IP addresses, controls,
/// and finally truncate a diagnostic string to a bounded UTF-8 size.
pub fn sanitize(text: &str, max_bytes: usize) -> String {
    let text = replace_urls(text);
    let text = replace_home_paths(&text);
    let text = replace_quoted_text(&text);
    let text = mask_ip_addresses(&text);
    let text: String = text
        .chars()
        .filter(|character| !character.is_control() || matches!(character, '\n' | '\t'))
        .collect();
    truncate_utf8(&text, max_bytes)
}

fn replace_urls(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let remainder = &text[index..];
        if starts_http_scheme(remainder.as_bytes()) {
            output.push_str("<url>");
            let mut end = index;
            while end < text.len() {
                let Some(character) = text[end..].chars().next() else {
                    break;
                };
                if character.is_whitespace()
                    || matches!(
                        character,
                        '"' | '\'' | '<' | '>' | '(' | ')' | '[' | ']' | '{' | '}'
                    )
                {
                    break;
                }
                end += character.len_utf8();
            }
            index = end;
        } else {
            let Some(character) = remainder.chars().next() else {
                break;
            };
            output.push(character);
            index += character.len_utf8();
        }
    }
    output
}

fn starts_http_scheme(bytes: &[u8]) -> bool {
    bytes.len() >= 7 && bytes[..7].eq_ignore_ascii_case(b"http://")
        || bytes.len() >= 8 && bytes[..8].eq_ignore_ascii_case(b"https://")
}

fn replace_home_paths(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let remainder = &text[index..];
        let prefix_len = if remainder.starts_with("/Users/") {
            7
        } else if remainder.starts_with("/home/") {
            6
        } else {
            0
        };
        if prefix_len != 0 {
            let after = index + prefix_len;
            let next = text[after..].chars().next();
            if next.is_some_and(|character| character != '/' && !character.is_whitespace()) {
                output.push('~');
                index = after;
                while index < text.len() {
                    let Some(character) = text[index..].chars().next() else {
                        break;
                    };
                    if character == '/' || character.is_whitespace() {
                        break;
                    }
                    index += character.len_utf8();
                }
                continue;
            }
        }
        let Some(character) = remainder.chars().next() else {
            break;
        };
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn replace_quoted_text(text: &str) -> String {
    let mut output = String::with_capacity(text.len());
    let mut index = 0;
    while index < text.len() {
        let Some(character) = text[index..].chars().next() else {
            break;
        };
        if matches!(character, '"' | '\'') && can_open_quote(text, index) {
            let quote = character;
            let mut end = index + character.len_utf8();
            let mut closed = false;
            while end < text.len() {
                let Some(inner) = text[end..].chars().next() else {
                    break;
                };
                if inner == '\n' {
                    break;
                }
                if inner == '\\' {
                    end += inner.len_utf8();
                    if end < text.len() {
                        let Some(escaped) = text[end..].chars().next() else {
                            break;
                        };
                        if escaped == '\n' {
                            break;
                        }
                        end += escaped.len_utf8();
                    }
                    continue;
                }
                end += inner.len_utf8();
                if inner == quote {
                    closed = true;
                    break;
                }
            }
            output.push(quote);
            output.push_str("<redacted>");
            if closed {
                output.push(quote);
            }
            index = end;
            continue;
        }
        output.push(character);
        index += character.len_utf8();
    }
    output
}

fn can_open_quote(text: &str, index: usize) -> bool {
    match text[..index].chars().next_back() {
        None => true,
        Some(previous) => {
            previous.is_whitespace() || matches!(previous, '(' | '[' | '{' | ':' | '=' | ',')
        }
    }
}

fn mask_ip_addresses(text: &str) -> String {
    let Ok(json) = serde_json::to_string(text) else {
        return text.to_owned();
    };
    let masked = logging::mask_line(&json);
    serde_json::from_str::<String>(&masked).unwrap_or_else(|_| text.to_owned())
}

fn truncate_utf8(text: &str, max_bytes: usize) -> String {
    if text.len() <= max_bytes {
        return text.to_owned();
    }
    let mut end = max_bytes.min(text.len());
    while end > 0 && !text.is_char_boundary(end) {
        end -= 1;
    }
    let mut output = text[..end].to_owned();
    output.push('…');
    output
}

pub fn crash_dir(core: &Core) -> PathBuf {
    core.data_dir().join("crashes")
}

pub fn report_file_name(report: &CrashReport) -> String {
    let tag = match report.kind {
        CrashKind::RustPanic => "rust",
        CrashKind::JsFatal => "js",
        CrashKind::JsError => "jserr",
        CrashKind::NativeCrash => "native",
    };
    let mut random = [0_u8; 4];
    let value = if SystemRandom::new().fill(&mut random).is_ok() {
        u32::from_be_bytes(random)
    } else {
        FALLBACK_RANDOM.fetch_add(1, Ordering::Relaxed)
    };
    format!("{:020}-{tag}-{value:08x}.json", report.incident_at)
}

pub fn is_report_name(name: &str) -> bool {
    let Some((incident, rest)) = name.split_once('-') else {
        return false;
    };
    if incident.len() != 20 || !incident.as_bytes().iter().all(u8::is_ascii_digit) {
        return false;
    }
    let Some((tag, suffix)) = rest.split_once('-') else {
        return false;
    };
    let Some(digits) = suffix.strip_suffix(".json") else {
        return false;
    };
    matches!(tag, "rust" | "js" | "jserr" | "native")
        && digits.len() == 8
        && digits
            .bytes()
            .all(|digit| matches!(digit, b'0'..=b'9' | b'a'..=b'f'))
}

pub fn incident_from_name(name: &str) -> Option<u64> {
    if !is_report_name(name) {
        return None;
    }
    name.as_bytes()[..20]
        .iter()
        .try_fold(0_u64, |value, digit| {
            value
                .checked_mul(10)
                .and_then(|value| value.checked_add(u64::from(digit - b'0')))
        })
}

pub fn write_report(dir: &Path, report: &CrashReport) -> io::Result<PathBuf> {
    fs::create_dir_all(dir)?;
    let bytes = serde_json::to_vec(report).map_err(io::Error::other)?;
    if u64::try_from(bytes.len()).unwrap_or(u64::MAX) > REPORT_FILE_MAX {
        return Err(io::Error::new(
            io::ErrorKind::InvalidData,
            "crash report exceeds size limit",
        ));
    }

    let name = report_file_name(report);
    let path = dir.join(&name);
    let temporary = dir.join(format!(".tmp-{name}"));
    let result = (|| {
        let mut file = fs::File::create(&temporary)?;
        file.write_all(&bytes)?;
        file.sync_all()?;
        drop(file);
        fs::rename(&temporary, &path)?;
        Ok::<(), io::Error>(())
    })();
    if let Err(error) = result {
        let _ = fs::remove_file(&temporary);
        return Err(error);
    }

    let reports = list_reports(dir);
    let remove_count = reports.len().saturating_sub(MAX_REPORTS);
    for old in reports.into_iter().take(remove_count) {
        let _ = fs::remove_file(old);
    }
    Ok(path)
}

pub fn list_reports(dir: &Path) -> Vec<PathBuf> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return Vec::new(),
    };
    let mut reports = Vec::new();
    for entry in entries {
        let Ok(entry) = entry else {
            return Vec::new();
        };
        let Ok(file_type) = entry.file_type() else {
            return Vec::new();
        };
        let name = entry.file_name();
        if file_type.is_file() && name.to_str().is_some_and(is_report_name) {
            reports.push((name, entry.path()));
        }
    }
    reports.sort_by(|left, right| left.0.cmp(&right.0));
    reports.into_iter().map(|(_, path)| path).collect()
}

pub fn prune(dir: &Path, now_us: u64) -> usize {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(_) => return 0,
    };
    let system_now = SystemTime::now();
    let mut removed = 0;
    for entry in entries {
        let Ok(entry) = entry else {
            continue;
        };
        let Ok(file_type) = entry.file_type() else {
            continue;
        };
        if !file_type.is_file() {
            continue;
        }
        let name = entry.file_name();
        let Some(name) = name.to_str() else {
            continue;
        };
        let should_remove = if let Some(incident) = incident_from_name(name) {
            now_us.saturating_sub(incident) > MAX_AGE_US
        } else if name.starts_with(".tmp-") {
            match entry.metadata().and_then(|metadata| metadata.modified()) {
                Ok(modified) => system_now
                    .duration_since(modified)
                    .is_ok_and(|age| age >= TMP_MAX_AGE),
                Err(_) => true,
            }
        } else {
            false
        };
        if should_remove && fs::remove_file(entry.path()).is_ok() {
            removed += 1;
        }
    }
    removed
}

pub fn record_js(core: &Arc<Core>, report_json: &str) -> bool {
    if report_json.len() > JS_INPUT_MAX {
        return false;
    }
    let Ok(value) = serde_json::from_str::<Value>(report_json) else {
        return false;
    };
    let Some(object) = value.as_object() else {
        return false;
    };
    let fatal = match object.get("isFatal") {
        Some(Value::Bool(fatal)) => *fatal,
        _ => return false,
    };
    let Some(message) = object.get("message").and_then(Value::as_str) else {
        return false;
    };
    let name = match object.get("name") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.as_str()),
        _ => return false,
    };
    let stack = match object.get("stack") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.as_str()),
        _ => return false,
    };
    let os_version = match object.get("osVersion") {
        None | Some(Value::Null) => None,
        Some(Value::String(value)) => Some(value.as_str()),
        _ => return false,
    };

    let state = core.state::<CrashState>();
    let (quota, limit, kind) = if fatal {
        (&state.js, JS_PER_CORE, CrashKind::JsFatal)
    } else {
        (&state.js_error, JS_ERROR_PER_CORE, CrashKind::JsError)
    };
    if quota
        .fetch_update(Ordering::AcqRel, Ordering::Acquire, |count| {
            (count < limit).then_some(count + 1)
        })
        .is_err()
    {
        return false;
    }

    let config = core.config();
    let now = core.now_us();
    let report = CrashReport {
        v: REPORT_VERSION,
        kind,
        incident_at: now,
        app_version: config.app_version,
        platform: config.platform,
        os_version: os_version
            .map(|value| sanitize(value, OS_VERSION_MAX))
            .filter(|value| !value.is_empty()),
        thread: None,
        location: None,
        name: name
            .map(|value| sanitize(value, NAME_MAX))
            .filter(|value| !value.is_empty()),
        message: sanitize(message, MESSAGE_MAX),
        stack: stack
            .map(|value| sanitize(value, STACK_MAX))
            .filter(|value| !value.is_empty()),
    };
    let dir = crash_dir(core);
    prune(&dir, now);
    match write_report(&dir, &report) {
        Ok(_) => {
            tracing::debug!("js crash recorded");
            true
        }
        Err(_) => false,
    }
}

#[uniffi::export]
pub fn crash_record_js(report_json: String) -> bool {
    core()
        .map(|core| record_js(&core, &report_json))
        .unwrap_or(false)
}
