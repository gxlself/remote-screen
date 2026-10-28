use std::fs;
use std::path::{Path, PathBuf};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use host_core::update::install::Installer;
use host_win::msi::{
    install_args, log_file_name, msiexec_path, subject_matches, Launch, MsiInstaller,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);
type LaunchCall = (PathBuf, Vec<String>);

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "remote-host-win-msi-{}-{sequence}",
            std::process::id()
        ));
        fs::create_dir_all(&path).expect("create test directory");
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

#[derive(Clone, Default)]
struct RecordingLauncher {
    calls: Arc<Mutex<Vec<LaunchCall>>>,
    error: Option<String>,
}

impl RecordingLauncher {
    fn failing(message: &str) -> Self {
        Self {
            calls: Arc::new(Mutex::new(Vec::new())),
            error: Some(message.to_owned()),
        }
    }

    fn calls(&self) -> Vec<LaunchCall> {
        self.calls.lock().expect("calls lock").clone()
    }
}

impl Launch for RecordingLauncher {
    fn launch(&self, exe: &Path, args: &[String]) -> Result<(), String> {
        self.calls
            .lock()
            .expect("calls lock")
            .push((exe.to_owned(), args.to_vec()));
        self.error.clone().map_or(Ok(()), Err)
    }
}

fn write_file(path: &Path, content: &str) {
    fs::write(path, content).expect("write test file");
}

fn installer(root: &Path, launcher: RecordingLauncher) -> MsiInstaller<RecordingLauncher> {
    MsiInstaller::new(
        root.join("update"),
        root.join("logs"),
        PathBuf::from(r"C:\Windows\System32\msiexec.exe"),
        launcher,
    )
}

#[test]
fn helper_paths_and_arguments() {
    assert_eq!(
        msiexec_path(None),
        PathBuf::from(r"C:\Windows\System32\msiexec.exe")
    );
    assert_eq!(
        msiexec_path(Some(r"D:\Win")),
        PathBuf::from(r"D:\Win\System32\msiexec.exe")
    );
    assert_eq!(
        install_args(Path::new("a.msi"), Path::new("l.log")),
        vec!["/i", "a.msi", "/qn", "/norestart", "/l*v", "l.log"]
    );
    assert_eq!(
        log_file_name("install", 1_700_000_000),
        "update-install-1700000000.log"
    );
}

#[test]
fn install_backs_up_current_and_publishes_staged_atomically() {
    let temp = TempDir::new();
    let update = temp.path().join("update");
    fs::create_dir_all(&update).expect("create update directory");
    write_file(&update.join("installed.msi"), "old");
    let package = temp.path().join("download.msi");
    write_file(&package, "new");
    let backup = temp.path().join("backup");
    let installer = installer(temp.path(), RecordingLauncher::default());

    installer
        .install(&package, &backup)
        .expect("install package");

    assert_eq!(fs::read(backup.join("previous.msi")).unwrap(), b"old");
    assert_eq!(fs::read(update.join("staged.msi")).unwrap(), b"new");
    assert!(!update.join("staged.msi.tmp").exists());
}

#[test]
fn install_without_current_or_package_fails_as_expected() {
    let temp = TempDir::new();
    let package = temp.path().join("download.msi");
    write_file(&package, "new");
    let backup = temp.path().join("backup");
    let installer = installer(temp.path(), RecordingLauncher::default());

    installer
        .install(&package, &backup)
        .expect("install without old package");
    assert!(!backup.join("previous.msi").exists());

    let missing = temp.path().join("missing.msi");
    let error = installer
        .install(&missing, &backup)
        .expect_err("missing package must fail");
    assert!(error.contains("package does not exist"));
}

#[test]
fn restart_promotes_staged_and_launches_msiexec() {
    let temp = TempDir::new();
    let package = temp.path().join("download.msi");
    write_file(&package, "new");
    let backup = temp.path().join("backup");
    let launcher = RecordingLauncher::default();
    let initial_installer = installer(temp.path(), launcher.clone());
    initial_installer
        .install(&package, &backup)
        .expect("stage package");

    initial_installer.restart().expect("restart installer");

    assert_eq!(
        fs::read(temp.path().join("update/installed.msi")).unwrap(),
        b"new"
    );
    assert!(!temp.path().join("update/staged.msi").exists());
    let calls = launcher.calls();
    assert_eq!(calls.len(), 1);
    assert_eq!(
        calls[0].0,
        PathBuf::from(r"C:\Windows\System32\msiexec.exe")
    );
    assert_eq!(
        calls[0].1[1],
        temp.path()
            .join("update")
            .join("installed.msi")
            .to_string_lossy()
    );
    assert!(Path::new(&calls[0].1[5])
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.starts_with("update-install-")));
}

#[test]
fn restart_reports_missing_staged_and_launcher_errors() {
    let temp = TempDir::new();
    let launcher = RecordingLauncher::default();
    let missing_installer = installer(temp.path(), launcher.clone());
    let error = missing_installer
        .restart()
        .expect_err("missing staged must fail");
    assert_eq!(error, "no staged installer");
    assert!(launcher.calls().is_empty());

    let package = temp.path().join("download.msi");
    write_file(&package, "new");
    let stage_installer = installer(temp.path(), RecordingLauncher::default());
    stage_installer
        .install(&package, &temp.path().join("backup"))
        .expect("stage package");
    let failing_launcher = RecordingLauncher::failing("launcher failed");
    let failing_installer = installer(temp.path(), failing_launcher.clone());
    let error = failing_installer
        .restart()
        .expect_err("launcher error must return");
    assert_eq!(error, "launcher failed");
    assert_eq!(failing_launcher.calls().len(), 1);
}

#[test]
fn rollback_restores_previous_and_launches_rollback_log() {
    let temp = TempDir::new();
    let package = temp.path().join("download.msi");
    write_file(&package, "new");
    let backup = temp.path().join("backup");
    let launcher = RecordingLauncher::default();
    let installer = installer(temp.path(), launcher.clone());
    installer.install(&package, &backup).expect("stage package");
    write_file(&backup.join("previous.msi"), "old");

    installer.rollback(&backup).expect("rollback package");

    assert_eq!(
        fs::read(temp.path().join("update/installed.msi")).unwrap(),
        b"old"
    );
    let calls = launcher.calls();
    assert_eq!(calls.len(), 1);
    assert!(calls[0].1[5].contains("update-rollback-"));

    let no_previous = temp.path().join("empty-backup");
    let error = installer
        .rollback(&no_previous)
        .expect_err("missing backup must fail");
    assert!(error.contains("no previous installer"));
}

#[test]
fn subjects_match_after_whitespace_normalization_with_case_sensitivity() {
    assert!(subject_matches(
        Some("CN=Example Ltd, O=Example"),
        Some("  CN=Example Ltd,  O=Example ")
    )
    .is_ok());
    assert!(subject_matches(None, Some("CN=Example"))
        .unwrap_err()
        .contains("not signed"));
    assert!(subject_matches(Some("CN=Example"), None)
        .unwrap_err()
        .contains("package is not signed"));
    assert!(subject_matches(Some("CN=A"), Some("CN=B"))
        .unwrap_err()
        .contains("mismatch"));
    assert!(subject_matches(Some("CN=a"), Some("CN=A")).is_err());
}
