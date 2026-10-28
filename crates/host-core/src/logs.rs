//! Daily structured logs and privacy-preserving diagnostic exports.

use crate::config::LogLevel;
use serde_json::{Map, Value};
use std::fmt;
use std::fs::{self, File, OpenOptions};
use std::io::{self, BufRead, BufReader, Write};
use std::net::{Ipv4Addr, Ipv6Addr};
use std::path::{Path, PathBuf};
use std::time::{SystemTime, UNIX_EPOCH};
use thiserror::Error;
use tracing::field::{Field, Visit};
use tracing::{Event, Subscriber};
use tracing_appender::non_blocking::WorkerGuard;
use tracing_subscriber::filter::LevelFilter;
use tracing_subscriber::fmt::format::{FormatEvent, FormatFields, Writer};
use tracing_subscriber::fmt::{FmtContext, FormattedFields};
use tracing_subscriber::prelude::*;
use tracing_subscriber::registry::LookupSpan;
use tracing_subscriber::{reload, Registry};
use zip::write::SimpleFileOptions;

#[derive(Debug, Error)]
pub enum LogError {
    #[error("log I/O error: {0}")]
    Io(#[from] io::Error),
    #[error("log appender initialization failed: {0}")]
    Appender(#[from] tracing_appender::rolling::InitError),
    #[error("invalid log level: {0}")]
    InvalidLevel(String),
    #[error("log subscriber initialization failed: {0}")]
    Subscriber(String),
    #[error("log level reload failed: {0}")]
    Reload(#[from] reload::Error),
    #[error("log archive error: {0}")]
    Zip(#[from] zip::result::ZipError),
}

impl AsRef<str> for LogLevel {
    fn as_ref(&self) -> &str {
        match self {
            Self::Trace => "trace",
            Self::Debug => "debug",
            Self::Info => "info",
            Self::Warn => "warn",
            Self::Error => "error",
        }
    }
}

/// Keep this guard alive for the lifetime of the agent so queued log lines are
/// flushed on shutdown.
pub struct LogGuard {
    level: reload::Handle<LevelFilter, Registry>,
    _worker: WorkerGuard,
}

impl LogGuard {
    pub fn set_level(&self, level: impl AsRef<str>) -> Result<(), LogError> {
        self.level.reload(parse_level(level.as_ref())?)?;
        Ok(())
    }
}

fn parse_level(level: &str) -> Result<LevelFilter, LogError> {
    match level {
        "trace" => Ok(LevelFilter::TRACE),
        "debug" => Ok(LevelFilter::DEBUG),
        "info" => Ok(LevelFilter::INFO),
        "warn" => Ok(LevelFilter::WARN),
        "error" => Ok(LevelFilter::ERROR),
        _ => Err(LogError::InvalidLevel(level.to_owned())),
    }
}

/// Installs the process-wide tracing subscriber. File logs contain JSON lines;
/// stderr remains readable to an operator. Only this appender's 14 latest daily
/// files are retained.
pub fn init(dir: &Path, level: impl AsRef<str>) -> Result<LogGuard, LogError> {
    let level = parse_level(level.as_ref())?;
    let appender = tracing_appender::rolling::RollingFileAppender::builder()
        .rotation(tracing_appender::rolling::Rotation::DAILY)
        .filename_prefix("host")
        .filename_suffix("jsonl")
        .max_log_files(14)
        .build(dir)?;
    let (writer, worker) = tracing_appender::non_blocking::NonBlockingBuilder::default()
        .lossy(false)
        .finish(appender);
    let (filter, handle) = reload::Layer::new(level);
    let subscriber = tracing_subscriber::registry()
        .with(filter)
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
    subscriber
        .try_init()
        .map_err(|error| LogError::Subscriber(error.to_string()))?;
    Ok(LogGuard {
        level: handle,
        _worker: worker,
    })
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
            .or_insert_with(|| event.metadata().target().into());
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

/// Masks structured identity fields recursively and addresses in all strings.
/// Non-JSON log lines are still scanned for IPv4 and IPv6 literals.
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
                    "host" | "node" | "user" | "device_name" | "host_name"
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
    // A final full stop or colon can be sentence punctuation rather than part
    // of the address. Preserve it after masking the address itself.
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

/// Archives regular files recursively. Symlinks are skipped to avoid including
/// files outside the log directory. An existing output archive is replaced only
/// after a complete ZIP has been written and synced.
pub fn export(dir: &Path, out_zip: &Path, mask: bool) -> Result<PathBuf, LogError> {
    let root = fs::canonicalize(dir)?;
    let parent = out_zip
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
        .unwrap_or_else(|| Path::new("."));
    fs::create_dir_all(parent)?;
    let output = fs::canonicalize(parent)?.join(out_zip.file_name().ok_or_else(|| {
        io::Error::new(io::ErrorKind::InvalidInput, "archive path needs a filename")
    })?);
    let mut files = Vec::new();
    collect_files(&root, &root, &output, &mut files)?;
    let temporary =
        output.with_extension(format!("zip.tmp-{}-{}", std::process::id(), unix_micros()));
    let mut options = OpenOptions::new();
    options.write(true).create_new(true);
    #[cfg(unix)]
    {
        use std::os::unix::fs::OpenOptionsExt;
        options.mode(0o600);
    }
    let file = options.open(&temporary)?;
    let result = write_archive(file, &files, mask).and_then(|()| {
        fs::rename(&temporary, &output)?;
        Ok(())
    });
    if result.is_err() {
        let _ = fs::remove_file(&temporary);
    }
    result?;
    Ok(out_zip.to_path_buf())
}

fn collect_files(
    root: &Path,
    dir: &Path,
    output: &Path,
    files: &mut Vec<(PathBuf, String)>,
) -> Result<(), LogError> {
    let mut entries = fs::read_dir(dir)?.collect::<Result<Vec<_>, _>>()?;
    entries.sort_by_key(|entry| entry.file_name());
    for entry in entries {
        let path = entry.path();
        let kind = entry.file_type()?;
        if path == output || kind.is_symlink() {
            continue;
        }
        if kind.is_dir() {
            collect_files(root, &path, output, files)?;
        } else if kind.is_file() {
            let name = path
                .strip_prefix(root)
                .map_err(io::Error::other)?
                .to_string_lossy()
                .replace('\\', "/");
            files.push((path, name));
        }
    }
    Ok(())
}

fn write_archive(file: File, files: &[(PathBuf, String)], mask: bool) -> Result<(), LogError> {
    let mut archive = zip::ZipWriter::new(file);
    let options = SimpleFileOptions::default()
        .compression_method(zip::CompressionMethod::Deflated)
        .unix_permissions(0o600);
    for (path, name) in files {
        archive.start_file(name, options)?;
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
    archive.finish()?.sync_all()?;
    Ok(())
}
