use std::collections::HashSet;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::audit::AuditPath;
use host_core::session::journal::{
    JournalEntry, OpenSession, SessionJournal, MAX_ENTRIES, TOUCH_INTERVAL_US,
};

const T: u64 = 1_792_465_134_000_000;

static COUNTER: AtomicU64 = AtomicU64::new(0);

struct Dir(PathBuf);

impl Dir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "rs-b11e-endjournal-{}-{}",
            std::process::id(),
            COUNTER.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }
}

impl Drop for Dir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

fn session(id: &str) -> OpenSession {
    OpenSession {
        session_id: id.to_owned(),
        user: None,
        device_id: "d7m2k8q4v9x3tb1a".to_owned(),
        path: Some(AuditPath::Lan),
        role: "controller".to_owned(),
        since_us: T,
    }
}

fn sessions_json(dir: &Dir) -> serde_json::Value {
    let contents = std::fs::read_to_string(dir.0.join("session_journal.json")).unwrap();
    serde_json::from_str(&contents).unwrap()
}

fn write_json(dir: &Dir, value: serde_json::Value) {
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(
        dir.0.join("session_journal.json"),
        serde_json::to_vec(&value).unwrap(),
    )
    .unwrap();
}

fn journal_entry(id: &str, since_us: u64, alive_at_us: u64) -> JournalEntry {
    JournalEntry {
        session: OpenSession {
            since_us,
            ..session(id)
        },
        left_at_us: None,
        alive_at_us,
    }
}

#[test]
fn open_missing_is_empty_and_creates_nothing() {
    let dir = Dir::new();
    let journal = SessionJournal::open(&dir.0, T);
    assert!(journal.leftovers().is_empty());
    assert!(journal.entries().is_empty());
    assert!(!dir.0.join("session_journal.json").exists());
}

#[test]
fn active_session_recovers_with_alive_time() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    let recovered = SessionJournal::open(&dir.0, T).leftovers();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].end_at_us, T);
    assert_eq!(recovered[0].duration_s, 0);
    assert!(!recovered[0].lingering);
    assert_eq!(recovered[0].session, session("a"));
}

#[test]
fn lingering_session_recovers_with_left_time() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    journal.left("a", T + 5_000_000);
    let recovered = SessionJournal::open(&dir.0, T).leftovers();
    assert_eq!(recovered[0].duration_s, 5);
    assert!(recovered[0].lingering);
    assert_eq!(recovered[0].end_at_us, T + 5_000_000);
}

#[test]
fn resume_clears_left_time() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    journal.left("a", T + 3_000_000);
    journal.resumed("a", T + 20_000_000);
    let recovered = SessionJournal::open(&dir.0, T).leftovers();
    assert!(!recovered[0].lingering);
    assert_eq!(recovered[0].duration_s, 20);
}

#[test]
fn end_removes_entry() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    journal.end("a");
    assert!(SessionJournal::open(&dir.0, T).leftovers().is_empty());
    let value = sessions_json(&dir);
    assert_eq!(value["version"], 1);
    assert_eq!(value["sessions"], serde_json::json!([]));
}

#[test]
fn set_role_is_persisted() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    journal.set_role("a", "viewer");
    let recovered = SessionJournal::open(&dir.0, T).leftovers();
    assert_eq!(recovered[0].session.role, "viewer");
}

#[test]
fn touch_respects_interval() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    assert!(!journal.touch(T + 59_000_000, &HashSet::from(["a".to_owned()])));
    assert_eq!(SessionJournal::open(&dir.0, T).leftovers()[0].end_at_us, T);
    assert!(journal.touch(T + TOUCH_INTERVAL_US, &HashSet::from(["a".to_owned()])));
    assert_eq!(
        SessionJournal::open(&dir.0, T).leftovers()[0].duration_s,
        60
    );
}

#[test]
fn touch_skips_lingering_and_unknown() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    journal.start(session("b"), T);
    journal.start(session("c"), T);
    journal.left("b", T + 1_000_000);
    assert!(journal.touch(
        T + TOUCH_INTERVAL_US,
        &HashSet::from(["a".to_owned(), "b".to_owned()])
    ));
    let entries = SessionJournal::open(&dir.0, T).leftovers();
    assert_eq!(entries[0].end_at_us, T + TOUCH_INTERVAL_US);
    assert_eq!(entries[1].end_at_us, T + 1_000_000);
    assert_eq!(entries[2].end_at_us, T);
}

#[test]
fn touch_after_clock_rewind() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    assert!(journal.touch(T - 1, &HashSet::from(["a".to_owned()])));
    let recovered = SessionJournal::open(&dir.0, T).leftovers();
    assert_eq!(recovered[0].duration_s, 0);
    assert_eq!(recovered[0].end_at_us, T);
}

#[test]
fn leftovers_survive_writes_until_cleared() {
    let dir = Dir::new();
    let mut first = SessionJournal::open(&dir.0, T);
    first.start(session("x"), T);
    drop(first);

    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("y"), T);
    assert_eq!(sessions_json(&dir)["sessions"].as_array().unwrap().len(), 2);
    journal.clear_leftovers();
    assert_eq!(sessions_json(&dir)["sessions"].as_array().unwrap().len(), 1);
    assert_eq!(
        SessionJournal::open(&dir.0, T).leftovers()[0]
            .session
            .session_id,
        "y"
    );
}

#[test]
fn corrupt_file_is_ignored_then_replaced() {
    let dir = Dir::new();
    std::fs::create_dir_all(&dir.0).unwrap();
    std::fs::write(dir.0.join("session_journal.json"), b"not json").unwrap();
    let mut journal = SessionJournal::open(&dir.0, T);
    assert!(journal.leftovers().is_empty());
    journal.clear_leftovers();
    assert!(serde_json::from_str::<serde_json::Value>(
        &std::fs::read_to_string(dir.0.join("session_journal.json")).unwrap()
    )
    .is_ok());
    assert!(sessions_json(&dir)["sessions"]
        .as_array()
        .unwrap()
        .is_empty());
}

#[test]
fn unknown_version_is_ignored() {
    let dir = Dir::new();
    let entry = serde_json::to_value(journal_entry("a", T, T)).unwrap();
    write_json(&dir, serde_json::json!({"version": 2, "sessions": [entry]}));
    assert!(SessionJournal::open(&dir.0, T).leftovers().is_empty());
}

#[test]
fn duplicate_ids_keep_first() {
    let dir = Dir::new();
    let first = serde_json::to_value(journal_entry("a", T, T)).unwrap();
    let second = serde_json::to_value(journal_entry("a", T + 1, T + 1)).unwrap();
    write_json(
        &dir,
        serde_json::json!({"version": 1, "sessions": [first, second]}),
    );
    let recovered = SessionJournal::open(&dir.0, T).leftovers();
    assert_eq!(recovered.len(), 1);
    assert_eq!(recovered[0].end_at_us, T);
}

#[test]
fn disabled_is_inert() {
    let dir = Dir::new();
    let mut disabled = SessionJournal::disabled();
    disabled.start(session("a"), T);
    disabled.set_role("a", "viewer");
    disabled.left("a", T + 1);
    disabled.resumed("a", T + 2);
    disabled.end("a");
    assert!(!disabled.touch(u64::MAX, &HashSet::from(["a".to_owned()])));
    assert!(!disabled.is_enabled());
    assert!(disabled.leftovers().is_empty());
    assert!(disabled.entries().is_empty());
    let _enabled = SessionJournal::open(&dir.0, T);
    assert!(!dir.0.join("session_journal.json").exists());
}

#[test]
fn entries_are_capped() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    for index in 0..=MAX_ENTRIES {
        let mut current = session(&format!("s-{index}"));
        current.since_us = T + index as u64;
        journal.start(current, T);
    }
    assert_eq!(journal.entries().len(), MAX_ENTRIES);
    assert!(!journal
        .entries()
        .iter()
        .any(|entry| entry.session.since_us == T));
}

#[test]
fn unknown_ids_are_noops() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    let before = journal.entries().to_vec();
    journal.set_role("nope", "viewer");
    journal.left("nope", T + 1);
    journal.resumed("nope", T + 2);
    journal.end("nope");
    assert_eq!(journal.entries(), before.as_slice());
}

#[test]
fn duration_floors_seconds() {
    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    journal.left("a", T + 5_999_999);
    assert_eq!(SessionJournal::open(&dir.0, T).leftovers()[0].duration_s, 5);
}

#[cfg(unix)]
#[test]
fn file_is_private() {
    use std::os::unix::fs::PermissionsExt;

    let dir = Dir::new();
    let mut journal = SessionJournal::open(&dir.0, T);
    journal.start(session("a"), T);
    let mode = std::fs::metadata(dir.0.join("session_journal.json"))
        .unwrap()
        .permissions()
        .mode();
    assert_eq!(mode & 0o777, 0o600);
}
