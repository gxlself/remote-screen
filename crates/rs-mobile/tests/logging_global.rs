//! Process-wide logger initialization; this file intentionally contains one test.

use rs_mobile::rscore::logging::{init_global, write_js};
use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-mobile-logging-global-{}-{label}-{}",
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

fn log_text(dir: &Path) -> String {
    let mut text = String::new();
    let Ok(entries) = fs::read_dir(dir) else {
        return text;
    };
    for entry in entries.flatten() {
        if entry
            .path()
            .extension()
            .is_some_and(|extension| extension == "jsonl")
        {
            text.push_str(&fs::read_to_string(entry.path()).unwrap_or_default());
        }
    }
    text
}

fn wait_for_tag(dir: &Path, tag: &str) -> bool {
    let deadline = Instant::now() + Duration::from_secs(2);
    loop {
        if log_text(dir).contains(tag) {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        std::thread::sleep(Duration::from_millis(20));
    }
}

#[test]
fn initializes_global_once_and_bridges_log_records() {
    let root = TempDir::new("global");
    let first = root.path().join("first");
    let second = root.path().join("second");
    let started = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_micros() as u64;

    assert!(init_global(&first, "info").expect("first initialization"));
    assert!(!init_global(&second, "info").expect("second initialization"));

    write_js("info", "T", "hello");
    log::info!("bridged log record");
    assert!(wait_for_tag(&first, "\"tag\":\"T\""), "JS log not flushed");
    assert!(
        wait_for_tag(&first, "bridged log record"),
        "log bridge missing"
    );
    let ended = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_micros() as u64;
    let text = log_text(&first);
    let js_line = text
        .lines()
        .find(|line| line.contains("\"tag\":\"T\""))
        .expect("JS line");
    let event: serde_json::Value = serde_json::from_str(js_line).expect("JSON event");
    let timestamp = event["ts"].as_u64().expect("timestamp");
    assert!(timestamp >= started && timestamp <= ended);
    assert!(
        !second.exists() || !log_text(&second).contains("\"tag\":\"T\""),
        "second initialization changed the directory"
    );
}
