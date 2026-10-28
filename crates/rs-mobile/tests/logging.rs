//! Local-dispatcher tests for app log rotation, JS logging, masking, and export.

use rs_mobile::rscore::logging::{
    build_dispatch, export_logs, mask_line, prune_old, write_js, LogError, LogGuard, JS_MSG_MAX,
};
use serde_json::Value;
use std::fs;
use std::io::Read;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{SystemTime, UNIX_EPOCH};
use tracing::{info, info_span, warn};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-mobile-logging-{}-{label}-{}",
            std::process::id(),
            NEXT_TEMP.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir_all(&path).expect("temporary directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn new_log_dir() -> TempDir {
    TempDir::new("logs")
}

fn log_files(dir: &Path) -> Vec<PathBuf> {
    let mut files = fs::read_dir(dir)
        .expect("log directory")
        .map(|entry| entry.expect("directory entry").path())
        .filter(|path| {
            path.extension()
                .is_some_and(|extension| extension == "jsonl")
        })
        .collect::<Vec<_>>();
    files.sort();
    files
}

fn read_log_lines(dir: &Path, guard: LogGuard) -> Vec<String> {
    drop(guard);
    let files = log_files(dir);
    assert_eq!(files.len(), 1, "expected one daily log file");
    let name = files[0]
        .file_name()
        .expect("log filename")
        .to_string_lossy();
    assert!(
        name.starts_with("rs.") && name.ends_with(".jsonl"),
        "unexpected daily log filename: {name}"
    );
    fs::read_to_string(&files[0])
        .expect("read log")
        .lines()
        .filter(|line| !line.is_empty())
        .map(str::to_owned)
        .collect()
}

fn parse_lines(lines: &[String]) -> Vec<Value> {
    lines
        .iter()
        .map(|line| serde_json::from_str(line).expect("valid JSON log line"))
        .collect()
}

fn run_with_dispatch<T>(dir: &Path, level: &str, action: impl FnOnce() -> T) -> (T, Vec<String>) {
    let (dispatch, guard) = build_dispatch(dir, level).expect("build dispatcher");
    let result = tracing::dispatcher::with_default(&dispatch, action);
    let lines = read_log_lines(dir, guard);
    (result, lines)
}

#[test]
fn writes_one_structured_json_line_with_microsecond_timestamp() {
    let root = new_log_dir();
    let before = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_micros() as u64;
    let ((), lines) = run_with_dispatch(root.path(), "info", || {
        info!(
            comp = "rsp.client",
            session = "s-1",
            code = 150u64,
            "dial failed"
        );
    });
    let after = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_micros() as u64;

    assert_eq!(lines.len(), 1);
    let line = parse_lines(&lines).pop().expect("event");
    assert_eq!(line["comp"], "rsp.client");
    assert_eq!(line["session"], "s-1");
    assert_eq!(line["code"], 150);
    assert_eq!(line["level"], "info");
    assert_eq!(line["message"], "dial failed");
    let timestamp = line["ts"].as_u64().expect("numeric timestamp");
    assert!(timestamp >= before && timestamp <= after);
}

#[test]
fn inherits_span_fields_and_event_fields_override_them() {
    let root = new_log_dir();
    let ((), lines) = run_with_dispatch(root.path(), "info", || {
        let span = info_span!("s", host = "h1");
        let _entered = span.enter();
        info!(comp = "rsp.client", "inherited");
        info!(comp = "rsp.client", host = "h2", "overridden");
    });

    let lines = parse_lines(&lines);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["host"], "h1");
    assert_eq!(lines[1]["host"], "h2");
}

#[test]
fn fills_component_from_target_when_absent() {
    let root = new_log_dir();
    let ((), lines) = run_with_dispatch(root.path(), "info", || {
        tracing::event!(
            target: "rs_mobile::rscore::sessions",
            tracing::Level::INFO,
            "from target"
        );
    });
    let line = parse_lines(&lines).pop().expect("event");
    assert_eq!(line["comp"], "rs_mobile.rscore.sessions");
}

#[test]
fn filters_by_level_and_rejects_unknown_levels() {
    let root = new_log_dir();
    let ((), lines) = run_with_dispatch(root.path(), "warn", || {
        info!(comp = "rsp.client", "hidden");
        warn!(comp = "rsp.client", "shown");
    });
    let lines = parse_lines(&lines);
    assert_eq!(lines.len(), 1);
    assert_eq!(lines[0]["message"], "shown");
    assert_eq!(lines[0]["level"], "warn");
    assert!(matches!(
        build_dispatch(root.path(), "loud"),
        Err(LogError::BadLevel(level)) if level == "loud"
    ));
}

#[test]
fn writes_js_lines_with_comp_tag_level_and_utf8_truncation() {
    let root = new_log_dir();
    let message = "汉".repeat(1600) + &"a".repeat(200);
    assert_eq!(message.len(), 5000);
    let long_tag = "t".repeat(80);
    let ((), lines) = run_with_dispatch(root.path(), "info", || {
        write_js("error", "Pairing", "boom");
        write_js("weird", "Pairing", "weird level");
        write_js("info", &long_tag, &message);
    });
    let lines = parse_lines(&lines);
    assert_eq!(lines.len(), 3);
    assert_eq!(lines[0]["comp"], "js");
    assert_eq!(lines[0]["tag"], "Pairing");
    assert_eq!(lines[0]["level"], "error");
    assert_eq!(lines[0]["message"], "boom");
    assert_eq!(lines[1]["level"], "info");
    let truncated = lines[2]["message"].as_str().expect("message");
    assert!(truncated.len() <= JS_MSG_MAX + "…".len());
    assert!(truncated.ends_with('…'));
    assert!(truncated.is_char_boundary(truncated.len()));
    assert_eq!(lines[2]["tag"].as_str().expect("tag").len(), 64);
}

#[test]
fn prunes_only_old_daily_logs() {
    let root = new_log_dir();
    for day in 1..=9 {
        fs::write(
            root.path().join(format!("rs.2026-01-0{day}.jsonl")),
            "line\n",
        )
        .expect("daily log");
    }
    fs::write(root.path().join("other.txt"), "keep").expect("other file");

    assert_eq!(prune_old(root.path(), 7).expect("prune"), 2);
    let names = fs::read_dir(root.path())
        .expect("log directory")
        .map(|entry| {
            entry
                .expect("entry")
                .file_name()
                .to_string_lossy()
                .into_owned()
        })
        .collect::<Vec<_>>();
    assert!(!names.iter().any(|name| name == "rs.2026-01-01.jsonl"));
    assert!(!names.iter().any(|name| name == "rs.2026-01-02.jsonl"));
    for day in 3..=9 {
        assert!(names
            .iter()
            .any(|name| name == &format!("rs.2026-01-0{day}.jsonl")));
    }
    assert!(names.iter().any(|name| name == "other.txt"));
}

#[test]
fn masks_identity_fields_and_addresses() {
    let json_line = r#"{"host":"h7v2","peer":"100.64.0.12:51820","msg":"from 192.168.31.5"}"#;
    let masked: Value = serde_json::from_str(&mask_line(json_line)).expect("masked JSON");
    assert_eq!(masked["host"], "•••");
    assert_eq!(masked["peer"], "100.64.•.•:51820");
    assert_eq!(masked["msg"], "from 192.168.•.•");

    let app_keys = r#"{"server":"build-01","ssh_user":"root"}"#;
    let masked: Value = serde_json::from_str(&mask_line(app_keys)).expect("masked JSON");
    assert_eq!(masked["server"], "•••");
    assert_eq!(masked["ssh_user"], "•••");

    assert_eq!(
        mask_line("connect 10.30.1.2 failed"),
        "connect 10.30.•.• failed"
    );
}

#[test]
fn exports_top_level_jsonl_files_sorted_and_masked() {
    let root = TempDir::new("export-source");
    let out = TempDir::new("export-output");
    fs::write(
        root.path().join("rs.2026-01-02.jsonl"),
        "{\"host\":\"h7v2\",\"peer\":\"100.64.0.12:51820\"}\n",
    )
    .expect("second log");
    fs::write(
        root.path().join("rs.2026-01-01.jsonl"),
        "connect 10.30.1.2 failed\n",
    )
    .expect("first log");
    fs::create_dir(root.path().join("nested")).expect("nested directory");
    fs::write(root.path().join("nested/rs.2026-01-03.jsonl"), "ignored").expect("nested log");

    #[cfg(unix)]
    {
        let outside = TempDir::new("export-outside");
        let outside_log = outside.path().join("outside.jsonl");
        fs::write(&outside_log, "external secret").expect("outside log");
        std::os::unix::fs::symlink(outside_log, root.path().join("linked.jsonl")).expect("symlink");
    }

    let archive = export_logs(root.path(), out.path(), true, 42).expect("export");
    assert_eq!(archive.file_name().expect("name"), "rs-logs-42.zip");
    assert_eq!(
        fs::read_dir(out.path()).expect("output directory").count(),
        1,
        "no temporary archive remains"
    );

    let file = fs::File::open(&archive).expect("archive");
    let mut archive = zip::ZipArchive::new(file).expect("zip archive");
    let names = archive
        .file_names()
        .map(|name| name.expect("entry name").into_owned())
        .collect::<Vec<_>>();
    assert_eq!(names, vec!["rs.2026-01-01.jsonl", "rs.2026-01-02.jsonl"]);
    let mut first = String::new();
    archive
        .by_name("rs.2026-01-01.jsonl")
        .expect("first entry")
        .read_to_string(&mut first)
        .expect("read first entry");
    assert_eq!(first, "connect 10.30.•.• failed\n");
    let mut second = String::new();
    archive
        .by_name("rs.2026-01-02.jsonl")
        .expect("second entry")
        .read_to_string(&mut second)
        .expect("read second entry");
    let second: Value = serde_json::from_str(second.trim_end()).expect("masked line");
    assert_eq!(second["host"], "•••");
    assert_eq!(second["peer"], "100.64.•.•:51820");
}

#[test]
fn exports_unmasked_contents_byte_for_byte() {
    let root = TempDir::new("export-plain-source");
    let out = TempDir::new("export-plain-output");
    let contents = "{\"host\":\"h7v2\",\"peer\":\"100.64.0.12:51820\"}\n";
    fs::write(root.path().join("rs.2026-01-01.jsonl"), contents).expect("log");

    let archive_path = export_logs(root.path(), out.path(), false, 7).expect("export");
    let file = fs::File::open(archive_path).expect("archive");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    let mut entry = String::new();
    archive
        .by_name("rs.2026-01-01.jsonl")
        .expect("entry")
        .read_to_string(&mut entry)
        .expect("read");
    assert_eq!(entry, contents);
}

#[test]
fn exports_readme_for_missing_log_directory() {
    let root = TempDir::new("missing-source");
    let out = TempDir::new("missing-output");
    let archive_path =
        export_logs(&root.path().join("does-not-exist"), out.path(), true, 9).expect("export");

    let file = fs::File::open(archive_path).expect("archive");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    assert_eq!(archive.len(), 1);
    let mut readme = String::new();
    archive
        .by_name("README.txt")
        .expect("readme")
        .read_to_string(&mut readme)
        .expect("read readme");
    assert_eq!(readme, "no logs\n");
}

#[test]
fn write_js_without_global_subscriber_does_not_panic() {
    write_js("info", "T", "discarded");
}
