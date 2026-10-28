use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Barrier};

use data_encoding::BASE64URL_NOPAD;
use host_core::pairfile_store::{
    PairFileError, PairFileState, PairFileStore, MAX_ACTIVE_FILES, PAIRFILES_FILE,
    PAIRFILE_LIFETIME_US, PAIRFILE_MAX_FAILURES, TERMINAL_RETENTION_US,
};
use host_core::pairing::RandomSource;
use rsp_core::trust::Permission;

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let root = std::env::var_os("TMPDIR")
            .map(PathBuf::from)
            .unwrap_or_else(|| PathBuf::from("target"));
        let path = root.join(format!(
            "rs-h20d-pairfile-store-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = std::fs::remove_dir_all(&path);
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

struct DeterministicRng {
    next: AtomicU64,
}

impl DeterministicRng {
    fn new() -> Self {
        Self {
            next: AtomicU64::new(0),
        }
    }
}

impl RandomSource for DeterministicRng {
    fn fill(&self, bytes: &mut [u8]) -> Result<(), getrandom::Error> {
        let seed = self.next.fetch_add(1, Ordering::Relaxed);
        for (index, byte) in bytes.iter_mut().enumerate() {
            *byte = (seed.wrapping_add(index as u64) % 251 + 1) as u8;
        }
        Ok(())
    }
}

fn open(dir: &TestDir) -> PairFileStore {
    PairFileStore::open(dir.path())
        .expect("open pairfile store")
        .with_rng(Arc::new(DeterministicRng::new()))
}

#[test]
fn open_accepts_a_missing_directory_and_creates_the_store_on_first_write() {
    let dir = TestDir::new();
    let store = open(&dir);
    assert!(!dir.path().join(PAIRFILES_FILE).exists());
    let created = create(&store, 1, Permission::Control, None);
    assert!(dir.path().join(PAIRFILES_FILE).exists());
    assert_eq!(
        store.list(1)[0].file_id,
        BASE64URL_NOPAD.encode(&created.file_id)
    );
}

fn create(
    store: &PairFileStore,
    now_us: u64,
    permission: Permission,
    label: Option<&str>,
) -> host_core::pairfile_store::NewPairFile {
    store
        .create(now_us, permission, label)
        .expect("create pairing file")
}

#[test]
fn create_lists_persists_and_restarts_with_private_permissions() {
    let dir = TestDir::new();
    let store = open(&dir);
    let created = create(&store, 1_000_000, Permission::Control, Some("IT"));

    let files = store.list(1_000_000);
    assert_eq!(files.len(), 1);
    assert_eq!(files[0].file_id, BASE64URL_NOPAD.encode(&created.file_id));
    assert_eq!(files[0].state, PairFileState::Active);
    assert_eq!(files[0].created_at_us, 1_000_000);
    assert_eq!(files[0].expires_at_us, 1_000_000 + PAIRFILE_LIFETIME_US);

    #[cfg(unix)]
    {
        use std::os::unix::fs::PermissionsExt;
        let mode = std::fs::metadata(dir.path().join(PAIRFILES_FILE))
            .expect("pairfile metadata")
            .permissions()
            .mode()
            & 0o777;
        assert_eq!(mode, 0o600);
    }

    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(PAIRFILES_FILE)).expect("read"))
            .expect("pairfiles json");
    assert_eq!(on_disk["v"], 1);
    assert_eq!(
        on_disk["files"][0]["secret"],
        BASE64URL_NOPAD.encode(created.secret.as_ref())
    );

    drop(store);
    let reopened = open(&dir);
    assert_eq!(reopened.list(1_000_000), files);
}

#[test]
fn list_orders_by_creation_time_descending() {
    let dir = TestDir::new();
    let store = open(&dir);
    let older = create(&store, 10, Permission::Control, None);
    let newer = create(&store, 20, Permission::Control, None);

    let files = store.list(20);
    assert_eq!(files[0].file_id, BASE64URL_NOPAD.encode(&newer.file_id));
    assert_eq!(files[1].file_id, BASE64URL_NOPAD.encode(&older.file_id));
}

#[test]
fn begin_returns_the_stored_secret_and_consume_is_one_shot() {
    let dir = TestDir::new();
    let store = open(&dir);
    let created = create(&store, 10, Permission::Unattended, None);

    let grant = store.begin(20, &created.file_id).expect("begin");
    assert_eq!(grant.secret.as_ref(), created.secret.as_ref());
    assert_eq!(grant.permission, Permission::Unattended);

    store
        .consume(30, &created.file_id, "device-1")
        .expect("consume");
    let files = store.list(30);
    assert_eq!(files[0].state, PairFileState::Used);
    assert_eq!(files[0].used_by_device_id.as_deref(), Some("device-1"));
    assert_eq!(files[0].used_at_us, Some(30));
    assert!(matches!(
        store.begin(31, &created.file_id),
        Err(PairFileError::NotUsable)
    ));
    assert!(matches!(
        store.consume(31, &created.file_id, "device-2"),
        Err(PairFileError::NotUsable)
    ));
}

#[test]
fn expiration_is_read_as_expired_and_blocks_begin() {
    let dir = TestDir::new();
    let store = open(&dir);
    let created = create(&store, 100, Permission::ViewOnly, None);

    assert!(matches!(
        store.begin(created.expires_at_us, &created.file_id),
        Err(PairFileError::NotUsable)
    ));
    assert_eq!(
        store.list(created.expires_at_us)[0].state,
        PairFileState::Expired
    );

    let _later = create(&store, created.expires_at_us, Permission::ViewOnly, None);
    drop(store);
    let reopened = open(&dir);
    assert_eq!(
        reopened.list(created.expires_at_us)[1].state,
        PairFileState::Expired
    );
    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(PAIRFILES_FILE)).expect("read"))
            .expect("pairfiles json");
    let file_id = BASE64URL_NOPAD.encode(&created.file_id);
    assert!(on_disk["files"]
        .as_array()
        .expect("files array")
        .iter()
        .any(|file| file["file_id"] == file_id && file["state"] == "expired"));
}

#[test]
fn five_failures_exhaust_and_fail_the_file() {
    let dir = TestDir::new();
    let store = open(&dir);
    let created = create(&store, 1, Permission::Control, None);

    for expected in (1..PAIRFILE_MAX_FAILURES).rev() {
        assert_eq!(store.fail(2, &created.file_id), expected);
    }
    assert_eq!(store.fail(3, &created.file_id), 0);
    assert_eq!(store.list(3)[0].state, PairFileState::Failed);
    assert!(matches!(
        store.begin(4, &created.file_id),
        Err(PairFileError::NotUsable)
    ));
    assert_eq!(store.fail(5, &created.file_id), 0);

    let file_id = BASE64URL_NOPAD.encode(&created.file_id);
    drop(store);
    let reopened = open(&dir);
    let files = reopened.list(5);
    let failed = files
        .iter()
        .find(|file| file.file_id == file_id)
        .expect("failed file is retained");
    assert_eq!(failed.state, PairFileState::Failed);
    let on_disk: serde_json::Value =
        serde_json::from_slice(&std::fs::read(dir.path().join(PAIRFILES_FILE)).expect("read"))
            .expect("pairfiles json");
    assert_eq!(on_disk["files"][0]["failures"], PAIRFILE_MAX_FAILURES);
}

#[test]
fn revoke_is_idempotent_for_terminal_records_and_rejects_unknown_ids() {
    let dir = TestDir::new();
    let store = open(&dir);
    let created = create(&store, 1, Permission::Control, None);
    let file_id = BASE64URL_NOPAD.encode(&created.file_id);

    store.revoke(&file_id).expect("revoke");
    assert_eq!(store.list(2)[0].state, PairFileState::Revoked);
    assert!(matches!(
        store.begin(2, &created.file_id),
        Err(PairFileError::NotUsable)
    ));
    store.revoke(&file_id).expect("repeat revoke");
    assert!(matches!(store.revoke(&file_id), Ok(())));

    let unknown = BASE64URL_NOPAD.encode(&[9u8; 16]);
    assert!(matches!(
        store.revoke(&unknown),
        Err(PairFileError::NotFound)
    ));
}

#[test]
fn active_file_limit_ignores_expired_records() {
    let dir = TestDir::new();
    let store = open(&dir);
    let mut first = None;
    for index in 0..MAX_ACTIVE_FILES {
        let created = create(&store, 10, Permission::Control, None);
        if index == 0 {
            first = Some(created);
        }
    }
    assert!(matches!(
        store.create(10, Permission::Control, None),
        Err(PairFileError::TooMany)
    ));

    let first = first.expect("first file");
    let _replacement = create(&store, first.expires_at_us, Permission::Control, None);
    assert_eq!(store.list(first.expires_at_us).len(), MAX_ACTIVE_FILES + 1);
}

#[test]
fn terminal_cleanup_uses_the_retention_boundary() {
    let dir = TestDir::new();
    let store = open(&dir);
    let used = create(&store, 1_000, Permission::Control, None);
    store
        .consume(1_000, &used.file_id, "phone")
        .expect("consume");
    let terminal_at = used.expires_at_us.max(1_000);

    let _before = create(
        &store,
        terminal_at + TERMINAL_RETENTION_US,
        Permission::Control,
        None,
    );
    assert!(store
        .list(terminal_at + TERMINAL_RETENTION_US)
        .iter()
        .any(|file| file.file_id == BASE64URL_NOPAD.encode(&used.file_id)));

    let latest = create(
        &store,
        terminal_at + TERMINAL_RETENTION_US + 1,
        Permission::Control,
        None,
    );
    let files = store.list(terminal_at + TERMINAL_RETENTION_US + 1);
    assert!(!files
        .iter()
        .any(|file| file.file_id == BASE64URL_NOPAD.encode(&used.file_id)));
    assert!(files
        .iter()
        .any(|file| file.file_id == BASE64URL_NOPAD.encode(&latest.file_id)));
}

#[test]
fn wrong_length_ids_are_rejected_without_panicking() {
    let dir = TestDir::new();
    let store = open(&dir);
    for length in [0usize, 15, 17] {
        let file_id = vec![7u8; length];
        assert!(matches!(
            store.begin(1, &file_id),
            Err(PairFileError::NotUsable)
        ));
        assert_eq!(store.fail(1, &file_id), 0);
    }
    assert!(matches!(
        store.begin(1, &[7u8; 16]),
        Err(PairFileError::NotUsable)
    ));
}

#[test]
fn concurrent_consume_has_exactly_one_winner() {
    let dir = TestDir::new();
    let store = Arc::new(open(&dir));
    let created = create(&store, 10, Permission::Control, None);
    let barrier = Arc::new(Barrier::new(2));
    let handles = ["device-a", "device-b"].map(|device_id| {
        let store = Arc::clone(&store);
        let barrier = Arc::clone(&barrier);
        let file_id = created.file_id;
        std::thread::spawn(move || {
            barrier.wait();
            store.consume(20, &file_id, device_id)
        })
    });
    let results = handles.map(|handle| handle.join().expect("consume thread"));
    assert_eq!(results.iter().filter(|result| result.is_ok()).count(), 1);
    assert_eq!(
        results
            .iter()
            .filter(|result| matches!(result, Err(PairFileError::NotUsable)))
            .count(),
        1
    );
    assert_eq!(store.list(20)[0].state, PairFileState::Used);
}

#[test]
fn corrupt_file_is_reported_and_left_untouched() {
    let dir = TestDir::new();
    std::fs::create_dir_all(dir.path()).expect("create dir");
    let path = dir.path().join(PAIRFILES_FILE);
    let original = b"{not valid pairfiles json";
    std::fs::write(&path, original).expect("write corrupt file");

    let error = PairFileStore::open(dir.path()).expect_err("corrupt store");
    assert!(matches!(error, PairFileError::Corrupt));
    assert_eq!(std::fs::read(path).expect("read corrupt file"), original);
}

#[test]
fn structurally_invalid_records_are_corrupt() {
    for (name, contents) in [
        ("version", r#"{"v":2,"files":[]}"#.to_owned()),
        (
            "permission",
            format!(
                r#"{{"v":1,"files":[{{"file_id":"{}","secret":"{}","label":null,"permission":"admin","created_at_us":1,"expires_at_us":2,"state":"active","failures":0,"used_by":null,"used_at_us":null}}]}}"#,
                BASE64URL_NOPAD.encode(&[1u8; 16]),
                BASE64URL_NOPAD.encode(&[2u8; 32])
            ),
        ),
        (
            "state",
            format!(
                r#"{{"v":1,"files":[{{"file_id":"{}","secret":"{}","label":null,"permission":"control","created_at_us":1,"expires_at_us":2,"state":"unknown","failures":0,"used_by":null,"used_at_us":null}}]}}"#,
                BASE64URL_NOPAD.encode(&[1u8; 16]),
                BASE64URL_NOPAD.encode(&[2u8; 32])
            ),
        ),
    ] {
        let dir = TestDir::new();
        std::fs::create_dir_all(dir.path()).expect("create dir");
        let path = dir.path().join(PAIRFILES_FILE);
        std::fs::write(&path, contents.as_bytes()).expect("write file");
        assert!(
            matches!(PairFileStore::open(dir.path()), Err(PairFileError::Corrupt)),
            "{name}"
        );
        assert_eq!(
            std::fs::read(path).expect("read file"),
            contents.as_bytes(),
            "{name}"
        );
    }
}

#[test]
fn labels_are_trimmed_and_validated() {
    let dir = TestDir::new();
    let store = open(&dir);

    create(&store, 1, Permission::Control, Some("  IT 分发 10 月 "));
    assert_eq!(store.list(1)[0].label.as_deref(), Some("IT 分发 10 月"));

    assert!(matches!(
        store.create(2, Permission::Control, Some(&"a".repeat(65))),
        Err(PairFileError::InvalidLabel)
    ));
    assert!(matches!(
        store.create(2, Permission::Control, Some("IT\nlab")),
        Err(PairFileError::InvalidLabel)
    ));
    let empty = list_by_id(&store, create(&store, 2, Permission::Control, Some("")));
    assert_eq!(empty.label, None);
    let blank = list_by_id(&store, create(&store, 2, Permission::Control, Some("   ")));
    assert_eq!(blank.label, None);
    let longest = create(&store, 2, Permission::Control, Some(&"a".repeat(64)));
    assert_eq!(list_by_id(&store, longest).label, Some("a".repeat(64)));
}

fn list_by_id(
    store: &PairFileStore,
    created: host_core::pairfile_store::NewPairFile,
) -> host_core::pairfile_store::PairFileSummary {
    let file_id = BASE64URL_NOPAD.encode(&created.file_id);
    store
        .list(created.created_at_us)
        .into_iter()
        .find(|file| file.file_id == file_id)
        .expect("created file is listed")
}

#[test]
fn debug_and_summaries_never_expose_secrets() {
    let dir = TestDir::new();
    let store = open(&dir);
    let created = create(&store, 1, Permission::Control, Some("IT"));
    let secret_bytes = *created.secret;
    let secret = BASE64URL_NOPAD.encode(created.secret.as_ref());
    let byte_array = format!("{secret_bytes:?}");

    assert!(!format!("{store:?}").contains(&secret));
    assert!(!format!("{:?}", store.list(1)).contains(&secret));
    assert!(!format!("{created:?}").contains(&secret));
    assert!(!format!("{created:?}").contains(&byte_array));

    let grant = store.begin(1, &created.file_id).expect("begin");
    assert!(!format!("{grant:?}").contains(&secret));
    assert!(!format!("{grant:?}").contains(&byte_array));
}
