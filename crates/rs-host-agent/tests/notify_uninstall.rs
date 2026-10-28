//! `--notify-uninstall` is a hidden no-op kept for old uninstallers.

use std::collections::BTreeSet;
use std::fs;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new(label: &str) -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/notify-tests");
        fs::create_dir_all(&root).expect("test root");
        let path = root.join(format!(
            "{label}-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn entries(root: &Path) -> BTreeSet<PathBuf> {
    fn visit(root: &Path, current: &Path, output: &mut BTreeSet<PathBuf>) {
        let Ok(items) = fs::read_dir(current) else {
            return;
        };
        for item in items {
            let item = item.expect("directory entry");
            let path = item.path();
            output.insert(path.strip_prefix(root).expect("relative path").to_owned());
            if path.is_dir() {
                visit(root, &path, output);
            }
        }
    }
    let mut output = BTreeSet::new();
    visit(root, root, &mut output);
    output
}

#[test]
fn legacy_notify_uninstall_exits_without_touching_state() {
    let dir = TestDir::new("legacy");
    let contents = "";
    fs::write(dir.path().join("config.toml"), contents).expect("config");
    let missing = dir.path().join("missing.toml");
    let before = entries(dir.path());

    for config in [dir.path().join("config.toml"), missing.clone()] {
        let output = Command::new(env!("CARGO_BIN_EXE_rs-host"))
            .args(["--platform", "fake", "--log", "error", "--state-dir"])
            .arg(dir.path())
            .arg("--config")
            .arg(&config)
            .arg("--notify-uninstall")
            .output()
            .expect("run rs-host");
        assert_eq!(output.status.code(), Some(0), "{output:?}");
        assert!(output.stdout.is_empty(), "{output:?}");
    }
    assert_eq!(entries(dir.path()), before);
    assert!(!missing.exists());
    assert_eq!(
        fs::read_to_string(dir.path().join("config.toml")).expect("config"),
        contents
    );

    let output = Command::new(env!("CARGO_BIN_EXE_rs-host"))
        .args(["--platform", "fake", "--notify-uninstall", "--service"])
        .arg("--state-dir")
        .arg(dir.path())
        .output()
        .expect("run rs-host");
    assert_ne!(output.status.code(), Some(0), "{output:?}");
    assert_eq!(entries(dir.path()), before);
}
