use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};

use host_core::platform::KeyStore;
use host_linux::keystore::FileKeyStore;

static NEXT_TEMP: AtomicU64 = AtomicU64::new(0);

struct TempDir(PathBuf);

impl TempDir {
    fn new(label: &str) -> Self {
        let id = NEXT_TEMP.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "remote-host-linux-{label}-{}-{id}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create temporary directory");
        Self(path)
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

#[test]
fn roundtrip() {
    let root = TempDir::new("keystore");
    let store = FileKeyStore::new(&root.0);
    assert_eq!(store.load("key").expect("load missing"), None);
    store.store("key", b"first").expect("store first");
    assert_eq!(
        store.load("key").expect("load first"),
        Some(b"first".to_vec())
    );
    store.delete("key").expect("delete key");
    store.delete("key").expect("delete missing");
    assert_eq!(store.load("key").expect("load deleted"), None);
}

#[cfg(unix)]
#[test]
fn permissions() {
    use std::os::unix::fs::PermissionsExt;

    let root = TempDir::new("permissions");
    let store = FileKeyStore::new(&root.0);
    store.store("key", b"value").expect("store key");
    assert_eq!(
        fs::metadata(&root.0)
            .expect("directory metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    let key = root.0.join("key");
    assert_eq!(
        fs::metadata(&key)
            .expect("key metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
    fs::set_permissions(&key, fs::Permissions::from_mode(0o644)).expect("loosen key mode");
    assert_eq!(
        store.load("key").expect("reload key"),
        Some(b"value".to_vec())
    );
    assert_eq!(
        fs::metadata(key)
            .expect("tightened metadata")
            .permissions()
            .mode()
            & 0o777,
        0o600
    );
}

#[test]
fn bad_names() {
    let root = TempDir::new("bad-names");
    let store = FileKeyStore::new(&root.0);
    for name in ["", ".", "..", "a/b", "a\\b", "a\0b"] {
        assert!(store.store(name, b"value").is_err());
        assert!(store.load(name).is_err());
        assert!(store.delete(name).is_err());
    }
}

#[test]
fn overwrite_is_atomic() {
    let root = TempDir::new("atomic");
    let store = FileKeyStore::new(&root.0);
    store.store("key", b"first").expect("store first");
    store.store("key", b"second").expect("store second");
    assert_eq!(
        store.load("key").expect("load second"),
        Some(b"second".to_vec())
    );
    let names = fs::read_dir(&root.0)
        .expect("read key directory")
        .map(|entry| entry.expect("directory entry").file_name())
        .collect::<Vec<_>>();
    assert!(names
        .iter()
        .all(|name| !name.to_string_lossy().contains(".tmp")));
}

#[test]
fn in_state_dir() {
    let root = PathBuf::from("/state");
    assert_eq!(
        FileKeyStore::in_state_dir(&root).directory(),
        root.join("keys").as_path()
    );
}
