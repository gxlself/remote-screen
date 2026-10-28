//! Daily structured logs and privacy-preserving diagnostic exports for the app.

use serde_json::{Map, Value};
use std::fmt;
use std::fs::{self, File};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::sync::{Mutex, OnceLock};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::field::{Field, Visit};
use tracing::{Event, Level, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, FormattedFields};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use zip::write::SimpleFileOptions;

pub const LOG_PREFIX: &str = "rs";
pub const LOG_SUFFIX: &str = "jsonl";
pub const KEEP_FILES: usize = 7;
pub const JS_MSG_MAX: usize = 4096;

#[derive(Debug, Error)]
pub enum LogError {
    #[error("log I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("log archive error: {0}")]
    Zip(String),
    #[error("log subscriber initialization failed: {0}")]
    Subscriber(String),
    #[error("invalid log level: {0}")]
    BadLevel(String),
}

/// Keeps the non-blocking appender worker alive and flushes queued lines when
/// dropped. Callers that install a local dispatcher should retain this for as
/// long as the dispatcher is in use.
pub struct LogGuard {
    _worker: WorkerGuard,
}

static GLOBAL_GUARD: OnceLock<LogGuard> = OnceLock::new();
static INIT_LOCK: Mutex<()> = Mutex::new(());

fn parse_level(level: &str) -> Result<LevelFilter, LogError> {
    match level {
        "trace" => Ok(LevelFilter::TRACE),
        "debug" => Ok(LevelFilter::DEBUG),
        "info" => Ok(LevelFilter::INFO),
        "warn" => Ok(LevelFilter::WARN),
        "error" => Ok(LevelFilter::ERROR),
        other => Err(LogError::BadLevel(other.to_owned())),
    }
}

/// Builds a JSON-lines file dispatcher and a matching readable stderr layer.
/// Old daily files are pruned both here and by the rolling appender itself.
pub fn build_dispatch(dir: &Path, level: &str) -> Result<(tracing::Dispatch, LogGuard), LogError> {
    let level = parse_level(level)?;
    fs::create_dir_all(dir)?;
    prune_old(dir, KEEP_FILES)?;
    let appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix(LOG_PREFIX)
        .filename_suffix(LOG_SUFFIX)
        .max_log_files(KEEP_FILES)
        .build(dir)
        .map_err(|error| LogError::Io(io::Error::other(error)))?;
    let (writer, worker) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .finish(appender);
    let subscriber = tracing_subscriber::registry()
        .with(level)
        .with(
            tracing_subscriber::fmt::layer()
                .json()
                .event_format(JsonEvent)
                .with_writer(writer),
        )
        .with(
            tracing_subscriber::fmt::layer()
                .with_ansi(false)
                .with_timer(UnixMicros)
                .with_writer(io::stderr),
        );
    Ok((
        tracing::Dispatch::new(subscriber),
        LogGuard { _worker: worker },
    ))
}

/// Installs the process-wide subscriber once. Repeated calls (including calls
/// with a different directory after JS hot reload) are harmless no-ops.
pub fn init_global(dir: &Path, level: &str) -> Result<bool, LogError> {
    let _lock = INIT_LOCK
        .lock()
        .map_err(|_| LogError::Subscriber("log initialization lock poisoned".to_owned()))?;
    if GLOBAL_GUARD.get().is_some() {
        return Ok(false);
    }
    let (dispatch, guard) = build_dispatch(dir, level)?;
    dispatch
        .try_init()
        .map_err(|error| LogError::Subscriber(error.to_string()))?;
    let _ = GLOBAL_GUARD.set(guard);
    Ok(true)
}

/// Records a line from JavaScript in the same daily log. JavaScript must not
/// pass typed text, clipboard contents, terminal contents, passwords, pairing
/// codes, or other secrets in `msg` or `tag`: this function does not filter
/// sensitive content.
pub fn write_js(level: &str, tag: &str, msg: &str) {
    let tag = truncate_utf8(tag, 64);
    let msg = truncate_message(msg, JS_MSG_MAX);
    macro_rules! emit {
        ($level:expr) => {
            tracing::event!(target: "js", $level, comp = "js", tag = tag, "{}", msg)
        };
    }
    match level {
        "debug" => emit!(Level::DEBUG),
        "warn" => emit!(Level::WARN),
        "error" => emit!(Level::ERROR),
        _ => emit!(Level::INFO),
    }
}

fn truncate_message(text: &str, max_bytes: usize) -> std::borrow::Cow<'_, str> {
    if text.len() <= max_bytes {
        return std::borrow::Cow::Borrowed(text);
    }
    let mut truncated = truncate_utf8(text, max_bytes).to_owned();
    truncated.push('…');
    std::borrow::Cow::Owned(truncated)
}

fn truncate_utf8(text: &str, max_bytes: usize) -> &str {
    if text.len() <= max_bytes {
        return text;
    }
    let mut end = max_bytes;
    while !text.is_char_boundary(end) {
        end -= 1;
    }
    &text[..end]
}

fn is_log_file_name(name: &str) -> bool {
    let bytes = name.as_bytes();
    bytes.len() == 19
        && bytes.starts_with(b"rs.")
        && bytes.ends_with(b".jsonl")
        && bytes[7] == b'-'
        && bytes[10] == b'-'
        && bytes[3..7].iter().all(u8::is_ascii_digit)
        && bytes[8..10].iter().all(u8::is_ascii_digit)
        && bytes[11..13].iter().all(u8::is_ascii_digit)
}

/// Deletes the oldest `rs.YYYY-MM-DD.jsonl` regular files until at most
/// `keep` remain. Unrelated files, directories, and symlinks are untouched.
pub fn prune_old(dir: &Path, keep: usize) -> Result<usize, LogError> {
    let entries = match fs::read_dir(dir) {
        Ok(entries) => entries,
        Err(error) if error.kind() == io::ErrorKind::NotFound => return Ok(0),
        Err(error) => return Err(LogError::Io(error)),
    };
    let mut files = Vec::new();
    for entry in entries {
        let entry = entry?;
        if !entry.file_type()?.is_file() {
            continue;
        }
        let name = entry.file_name();
        if name.to_str().is_some_and(is_log_file_name) {
            files.push((name, entry.path()));
        }
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));
    let remove_count = files.len().saturating_sub(keep);
    for (_, path) in files.into_iter().take(remove_count) {
        fs::remove_file(path)?;
    }
    Ok(remove_count)
}

fn unix_micros() -> u64 {
    match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => u64::try_from(duration.as_micros()).unwrap_or(u64::MAX),
        Err(_) => 0,
    }
}

struct UnixMicros;

impl tracing_subscriber::fmt::time::FormatTime for UnixMicros {
    fn format_time(&self, writer: &mut Writer<'_>) -> fmt::Result {
        write!(writer, "{}", unix_micros())
    }
}

struct JsonEvent;

impl<S, N> FormatEvent<S, N> for JsonEvent
where
    S: Subscriber + for<'lookup> LookupSpan<'lookup>,
    N: for<'writer> FormatFields<'writer> + 'static,
{
    fn format_event(
        &self,
        context: &FmtContext<'_, S, N>,
        mut writer: Writer<'_>,
        event: &Event<'_>,
    ) -> fmt::Result {
        let mut fields = Map::new();
        if let Some(scope) = context.event_scope() {
            for span in scope.from_root() {
                let extensions = span.extensions();
                if let Some(formatted) = extensions.get::<FormattedFields<N>>() {
                    if let Ok(Value::Object(values)) =
                        serde_json::from_str(formatted.fields.as_str())
                    {
                        fields.extend(values);
                    }
                }
            }
        }
        event.record(&mut JsonVisitor(&mut fields));
        fields.insert("ts".to_owned(), unix_micros().into());
        fields.insert(
            "level".to_owned(),
            event.metadata().level().as_str().to_lowercase().into(),
        );
        fields
            .entry("comp")
            .or_insert_with(|| Value::String(event.metadata().target().replace("::", ".")));
        let line = serde_json::to_string(&fields).map_err(|_| fmt::Error)?;
        writeln!(writer, "{line}")
    }
}

struct JsonVisitor<'a>(&'a mut Map<String, Value>);

impl Visit for JsonVisitor<'_> {
    fn record_debug(&mut self, field: &Field, value: &dyn fmt::Debug) {
        self.0
            .insert(field.name().to_owned(), format!("{value:?}").into());
    }

    fn record_str(&mut self, field: &Field, value: &str) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_u64(&mut self, field: &Field, value: u64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_i64(&mut self, field: &Field, value: i64) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_bool(&mut self, field: &Field, value: bool) {
        self.0.insert(field.name().to_owned(), value.into());
    }

    fn record_f64(&mut self, field: &Field, value: f64) {
        self.0.insert(field.name().to_owned(), value.into());
    }
}

/// Masks identity fields recursively and IPv4/IPv6 literals in all strings.
/// Non-JSON lines still have their addresses masked. In addition to the
/// documented Host identity keys, app logs mask `server` and `ssh_user`.
pub fn mask_line(line: &str) -> String {
    if let Ok(mut value) = serde_json::from_str::<Value>(line) {
        mask_json(&mut value);
        if let Ok(masked) = serde_json::to_string(&value) {
            return masked;
        }
    }
    mask_ips(line)
}

fn mask_json(value: &mut Value) {
    match value {
        Value::Object(fields) => {
            for (key, value) in fields {
                if matches!(
                    key.as_str(),
                    "host" | "node" | "user" | "device_name" | "host_name" | "server" | "ssh_user"
                ) {
                    *value = "•••".into();
                } else {
                    mask_json(value);
                }
            }
        }
        Value::Array(items) => items.iter_mut().for_each(mask_json),
        Value::String(text) => *text = mask_ips(text),
        _ => {}
    }
}

fn mask_ips(text: &str) -> String {
    let mut masked = String::with_capacity(text.len());
    let mut token = String::new();
    for character in text.chars() {
        if character.is_ascii_alphanumeric() || matches!(character, '.' | ':' | '%' | '_' | '-') {
            token.push(character);
        } else {
            masked.push_str(&mask_address_token(&token));
            token.clear();
            masked.push(character);
        }
    }
    masked.push_str(&mask_address_token(&token));
    masked
}

fn mask_address_token(token: &str) -> String {
    let core = token.trim_end_matches('.');
    let suffix = &token[core.len()..];
    if let Ok(address) = core.parse::<Ipv4Addr>() {
        let [first, second, _, _] = address.octets();
        return format!("{first}.{second}.•.•{suffix}");
    }
    let (address, zone) = match core.split_once('%') {
        Some((address, zone)) => (address, format!("%{zone}")),
        None => (core, String::new()),
    };
    if let Ok(address) = address.parse::<Ipv6Addr>() {
        let groups = address.segments();
        return format!("{:x}:{:x}:•:•:•:•:•:•{zone}{suffix}", groups[0], groups[1]);
    }
    if let Some((address, port)) = core.rsplit_once(':') {
        if address.parse::<Ipv4Addr>().is_ok() && port.parse::<u16>().is_ok() {
            return format!("{}:{port}{suffix}", mask_address_token(address));
        }
    }
    token.to_owned()
}

/// Writes a diagnostic archive atomically. Only top-level regular `*.jsonl`
/// files are included; directories and symlinks are skipped. A missing log
/// directory still produces an archive containing a short README.
pub fn export_logs(
    log_dir: &Path,
    out_dir: &Path,
    mask: bool,
    now_us: u64,
) -> Result<PathBuf, LogError> {
    fs::create_dir_all(out_dir)?;
    let output = out_dir.join(format!("rs-logs-{now_us}.zip"));
    let mut temporary = output.as_os_str().to_os_string();
    temporary.push(".tmp");
    let temporary = PathBuf::from(temporary);

    let mut files = Vec::new();
    let mut missing_log_dir = false;
    match fs::read_dir(log_dir) {
        Ok(entries) => {
            for entry in entries {
                let entry = entry?;
                let file_type = entry.file_type()?;
                if !file_type.is_file() {
                    continue;
                }
                let name = entry.file_name();
                if name
                    .to_str()
                    .is_some_and(|name| name.ends_with(&format!(".{LOG_SUFFIX}")))
                {
                    files.push((name, entry.path()));
                }
            }
        }
        Err(error) if error.kind() == io::ErrorKind::NotFound => missing_log_dir = true,
        Err(error) => return Err(LogError::Io(error)),
    }
    files.sort_by(|left, right| left.0.cmp(&right.0));

    let result = (|| -> Result<(), LogError> {
        let file = File::create(&temporary)?;
        write_archive(file, &files, missing_log_dir, mask)?;
        fs::rename(&temporary, &output)?;
        Ok(())
    })();
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(output)
}

fn write_archive(
    file: File,
    files: &[(std::ffi::OsString, PathBuf)],
    include_readme: bool,
    mask: bool,
) -> Result<(), LogError> {
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    if include_readme {
        archive
            .start_file("README.txt", options)
            .map_err(|error| LogError::Zip(error.to_string()))?;
        archive.write_all(b"no logs\n")?;
    }
    for (name, path) in files {
        archive
            .start_file(name.to_string_lossy(), options)
            .map_err(|error| LogError::Zip(error.to_string()))?;
        let mut reader = BufReader::new(File::open(path)?);
        if mask {
            let mut line = String::new();
            while reader.read_line(&mut line)? != 0 {
                let newline = if line.ends_with("\r\n") {
                    "\r\n"
                } else if line.ends_with('\n') {
                    "\n"
                } else {
                    ""
                };
                let contents = &line[..line.len() - newline.len()];
                archive.write_all(mask_line(contents).as_bytes())?;
                archive.write_all(newline.as_bytes())?;
                line.clear();
            }
        } else {
            io::copy(&mut reader, &mut archive)?;
        }
    }
    archive
        .finish()
        .map_err(|error| LogError::Zip(error.to_string()))?
        .sync_all()?;
    Ok(())
}
