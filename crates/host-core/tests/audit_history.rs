use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::audit::{AuditEvent, AuditQueue, HISTORY_MAX};

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/rs-h23a-tests");
        let path = root.join(format!(
            "audit-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("test directory is created");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn event(ts: u64) -> AuditEvent {
    AuditEvent {
        ts,
        user: Some("alice".to_owned()),
        device_id: None,
        host_id: None,
        server: None,
        event: "session_start".to_owned(),
        path: None,
        duration_s: None,
        detail: None,
    }
}

#[test]
fn history_orders_filters_and_survives_ack_and_restart() {
    let dir = TestDir::new();
    let queue = AuditQueue::open(dir.path()).expect("queue opens");
    queue.push(event(1)).expect("push");
    queue.push(event(3)).expect("push");
    queue.push(event(2)).expect("push");
    assert_eq!(
        queue
            .history(50, None)
            .iter()
            .map(|entry| entry.ts)
            .collect::<Vec<_>>(),
        [3, 2, 1]
    );
    let batch = queue.batch(3).expect("batch");
    queue.acknowledge(&batch).expect("acknowledge");
    assert_eq!(queue.len(), 0);
    assert_eq!(queue.history(50, None).len(), 3);
    drop(queue);

    let queue = AuditQueue::open(dir.path()).expect("queue reopens");
    assert_eq!(
        queue
            .history(2, Some(3))
            .iter()
            .map(|entry| entry.ts)
            .collect::<Vec<_>>(),
        [2, 1]
    );
}

#[test]
fn history_rewrites_old_lines_and_skips_bad_records() {
    let dir = TestDir::new();
    let history_path = dir.path().join("audit_queue.history.jsonl");
    let line = serde_json::to_string(&event(1)).expect("event serializes");
    let mut contents = String::new();
    for ts in 0..(HISTORY_MAX as u64 + 10) {
        contents.push_str(&serde_json::to_string(&event(ts)).expect("event serializes"));
        contents.push('\n');
    }
    contents.push_str("not-json\n");
    contents.push_str(&line[..line.len() / 2]);
    fs::write(&history_path, contents).expect("history is written");

    let queue = AuditQueue::open(dir.path()).expect("queue opens with bad history");
    let entries = queue.history(HISTORY_MAX + 1, None);
    assert_eq!(entries.len(), HISTORY_MAX);
    assert_eq!(
        entries.first().expect("latest entry").ts,
        HISTORY_MAX as u64 + 9
    );
    assert_eq!(entries.last().expect("oldest entry").ts, 10);
    assert_eq!(
        fs::read_to_string(queue.history_path())
            .expect("history is readable")
            .lines()
            .count(),
        HISTORY_MAX
    );
}

#[test]
fn history_append_failure_does_not_fail_push() {
    let dir = TestDir::new();
    let queue = AuditQueue::open(dir.path()).expect("queue opens");
    fs::create_dir(queue.history_path()).expect("history path is occupied");
    queue.push(event(42)).expect("history is best effort");
    assert_eq!(
        queue
            .history(10, None)
            .iter()
            .map(|entry| entry.ts)
            .collect::<Vec<_>>(),
        [42]
    );
}

#[cfg(unix)]
#[test]
fn history_file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = TestDir::new();
    let queue = AuditQueue::open(dir.path()).expect("queue opens");
    queue.push(event(1)).expect("push");
    assert_eq!(
        fs::metadata(queue.history_path())
            .expect("history metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}
