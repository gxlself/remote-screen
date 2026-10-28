#![cfg(target_os = "macos")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, AtomicUsize, Ordering};
use std::sync::Arc;

use host_core::update::install::Installer;
use host_mac::update_install::{
    app_bundle_of, kickstart_args, swap_dirs, MacAppInstaller, Relauncher, AGENT_LABEL,
};
use host_mac::update_verify::{CodeSigning, APP_BUNDLE_NAME};

const TEAM: &str = "ABCDE12345";
static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "remote-screen-update-install-{}-{sequence}",
            std::process::id()
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("create test temporary directory");
        Self { path }
    }

    fn path(&self) -> &Path {
        &self.path
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = fs::set_permissions(&self.path, fs::Permissions::from_mode(0o755));
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn info_plist() -> &'static [u8] {
    br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>ai.gxlself.remotehost</string>
<key>CFBundleExecutable</key><string>rs-host-agent</string>
</dict></plist>
"#
}

fn fake_app(root: &Path, name: &str, version: &str) -> PathBuf {
    let app = root.join(name);
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    let resources = contents.join("Resources");
    fs::create_dir_all(&macos).expect("create fake app directories");
    fs::create_dir_all(&resources).expect("create fake app resources");
    fs::write(contents.join("Info.plist"), info_plist()).expect("write fake Info.plist");
    fs::write(macos.join("rs-host-agent"), b"fake agent").expect("write fake agent");
    fs::write(resources.join("version.txt"), version).expect("write fake version");
    app
}

fn zip(app: &Path, output: &Path) {
    let status = Command::new("/usr/bin/ditto")
        .arg("-c")
        .arg("-k")
        .arg("--sequesterRsrc")
        .arg("--keepParent")
        .arg(app)
        .arg(output)
        .status()
        .expect("run ditto to create update zip");
    assert!(status.success(), "ditto failed with {status}");
}

fn ver(app: &Path) -> String {
    fs::read_to_string(app.join("Contents/Resources/version.txt")).expect("read app version")
}

#[derive(Clone)]
struct FakeSigning {
    team: Option<String>,
    check: Result<(), String>,
}

impl CodeSigning for FakeSigning {
    fn current_team_id(&self) -> Result<Option<String>, String> {
        Ok(self.team.clone())
    }

    fn team_id(&self, _path: &Path) -> Result<Option<String>, String> {
        Ok(self.team.clone())
    }

    fn check(&self, _path: &Path, _requirement: Option<&str>) -> Result<(), String> {
        self.check.clone()
    }
}

struct FakeRelauncher {
    calls: Arc<AtomicUsize>,
    result: Result<(), String>,
}

impl Relauncher for FakeRelauncher {
    fn relaunch(&self) -> Result<(), String> {
        self.calls.fetch_add(1, Ordering::SeqCst);
        self.result.clone()
    }
}

fn installer(root: &Path) -> MacAppInstaller<FakeSigning, FakeRelauncher> {
    MacAppInstaller::new(
        root.join(APP_BUNDLE_NAME),
        Some(TEAM.to_owned()),
        FakeSigning {
            team: Some(TEAM.to_owned()),
            check: Ok(()),
        },
        FakeRelauncher {
            calls: Arc::new(AtomicUsize::new(0)),
            result: Ok(()),
        },
    )
}

fn package(root: &Path, name: &str, version: &str) -> PathBuf {
    let source_root = root.join("new");
    let app = fake_app(&source_root, name, version);
    let zip_path = root.join("update.zip");
    zip(&app, &zip_path);
    zip_path
}

#[test]
fn install_replaces_app_and_keeps_backup() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let backup = temp.path().join("backup/1");
    let installer = installer(temp.path());
    assert_eq!(
        installer.staging_dir(),
        temp.path().join(".Remote Host.app.update")
    );
    let stale_backup = fake_app(&backup, APP_BUNDLE_NAME, "stale");
    let stale_marker = stale_backup.join("Contents/Resources/stale.txt");
    fs::write(&stale_marker, b"stale").expect("write stale backup marker");

    installer.install(&update, &backup).expect("install update");
    assert_eq!(ver(&app), "2");
    assert_eq!(ver(&backup.join(APP_BUNDLE_NAME)), "1");
    assert!(!stale_marker.exists());
    assert!(!installer.staging_dir().exists());
}

#[test]
fn install_without_team_touches_nothing() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let backup = temp.path().join("backup/1");
    let installer = MacAppInstaller::new(
        app.clone(),
        None,
        FakeSigning {
            team: Some(TEAM.to_owned()),
            check: Ok(()),
        },
        FakeRelauncher {
            calls: Arc::new(AtomicUsize::new(0)),
            result: Ok(()),
        },
    );

    let error = installer
        .install(&update, &backup)
        .expect_err("unsigned current app accepted");
    assert_eq!(
        error,
        "current app is not signed with a Team ID; refusing update"
    );
    assert_eq!(ver(&app), "1");
    assert!(!temp.path().join("backup").exists());
    assert!(!installer.staging_dir().exists());

    fs::create_dir(installer.staging_dir()).expect("create existing staging");
    let marker = installer.staging_dir().join("keep");
    fs::write(&marker, b"keep").expect("write staging marker");
    assert!(installer.install(&update, &backup).is_err());
    assert_eq!(fs::read(marker).expect("read staging marker"), b"keep");
}

#[test]
fn install_signature_failure_keeps_app() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let installer = MacAppInstaller::new(
        app.clone(),
        Some(TEAM.to_owned()),
        FakeSigning {
            team: Some(TEAM.to_owned()),
            check: Err("bad".to_owned()),
        },
        FakeRelauncher {
            calls: Arc::new(AtomicUsize::new(0)),
            result: Ok(()),
        },
    );

    let error = installer
        .install(&update, &temp.path().join("backup/1"))
        .expect_err("signature failure accepted");
    assert!(error.contains("bad"));
    assert_eq!(ver(&app), "1");
    assert!(!temp.path().join("backup").exists());
    assert!(!installer.staging_dir().exists());
}

#[test]
fn install_rejects_wrong_bundle_name() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), "Other.app", "2");
    let installer = installer(temp.path());

    assert!(installer
        .install(&update, &temp.path().join("backup/1"))
        .is_err());
    assert_eq!(ver(&app), "1");
    assert!(!installer.staging_dir().exists());
    assert!(!temp.path().join("backup").exists());
}

#[test]
fn install_rejects_renamed_current_bundle() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), "Renamed.app", "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let installer = MacAppInstaller::new(
        app.clone(),
        Some(TEAM.to_owned()),
        FakeSigning {
            team: Some(TEAM.to_owned()),
            check: Ok(()),
        },
        FakeRelauncher {
            calls: Arc::new(AtomicUsize::new(0)),
            result: Ok(()),
        },
    );

    let error = installer
        .install(&update, &temp.path().join("backup/1"))
        .expect_err("mismatched installed bundle name accepted");
    assert!(error.contains("bundle name"));
    assert_eq!(ver(&app), "1");
    assert!(!temp.path().join("backup").exists());
    assert!(!installer.staging_dir().exists());
}

#[test]
fn install_invalid_zip_keeps_app() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = temp.path().join("invalid.zip");
    fs::write(&update, b"not a zip").expect("write invalid package");
    let installer = installer(temp.path());

    assert!(installer
        .install(&update, &temp.path().join("backup/1"))
        .is_err());
    assert_eq!(ver(&app), "1");
    assert!(!temp.path().join("backup").exists());
    assert!(!installer.staging_dir().exists());
}

#[test]
fn install_backup_failure_keeps_app() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let backup = temp.path().join("backup");
    fs::write(&backup, b"keep").expect("block backup directory creation");
    let installer = installer(temp.path());

    let error = installer
        .install(&update, &backup.join("1"))
        .expect_err("backup failure accepted");
    assert!(error.contains("backup"));
    assert_eq!(ver(&app), "1");
    assert_eq!(fs::read(&backup).expect("read backup blocker"), b"keep");
    assert!(!installer.staging_dir().exists());
}

#[test]
fn install_cleans_stale_staging() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let installer = installer(temp.path());
    fs::create_dir_all(installer.staging_dir()).expect("create stale staging");
    fs::write(installer.staging_dir().join("junk"), b"junk").expect("write stale marker");

    installer
        .install(&update, &temp.path().join("backup/1"))
        .expect("install after stale staging");
    assert_eq!(ver(&app), "2");
    assert!(!installer.staging_dir().exists());
}

#[test]
fn install_fails_when_parent_not_writable() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let installer = installer(temp.path());
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o555))
        .expect("make app parent read-only");
    let result = installer.install(&update, &temp.path().join("backup/1"));
    fs::set_permissions(temp.path(), fs::Permissions::from_mode(0o755))
        .expect("restore app parent permissions");

    let error = result.expect_err("install succeeded without parent write permission");
    assert!(error.contains("无法在"));
    assert!(error.contains("下准备更新"));
    assert_eq!(ver(&app), "1");
    assert!(!temp.path().join("backup").exists());
    assert!(!installer.staging_dir().exists());
}

#[test]
fn rollback_restores_previous() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let backup = temp.path().join("backup/1");
    let installer = installer(temp.path());
    installer.install(&update, &backup).expect("install update");
    assert_eq!(ver(&app), "2");
    // Rollback must clean interrupted staging and restore the running backup
    // without applying the update candidate's signing rules to it.
    fs::create_dir(installer.staging_dir()).expect("create stale rollback staging");
    fs::write(installer.staging_dir().join("junk"), b"junk").expect("write stale rollback marker");
    let rollback_installer = MacAppInstaller::new(
        app.clone(),
        None,
        FakeSigning {
            team: None,
            check: Err("rollback must not verify signing".to_owned()),
        },
        FakeRelauncher {
            calls: Arc::new(AtomicUsize::new(0)),
            result: Ok(()),
        },
    );

    rollback_installer
        .rollback(&backup)
        .expect("rollback update");
    assert_eq!(ver(&app), "1");
    assert_eq!(ver(&backup.join(APP_BUNDLE_NAME)), "1");
    assert!(!installer.staging_dir().exists());
}

#[test]
fn rollback_without_backup() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let installer = installer(temp.path());
    let backup = temp.path().join("empty-backup");
    fs::create_dir(&backup).expect("create empty backup directory");

    let error = installer
        .rollback(&backup)
        .expect_err("rollback without backup accepted");
    assert!(error.contains("no previous app"));
    assert_eq!(ver(&app), "1");
}

#[test]
fn restart_uses_relauncher() {
    let temp = TempDir::new();
    let calls = Arc::new(AtomicUsize::new(0));
    let installer = MacAppInstaller::new(
        temp.path().join(APP_BUNDLE_NAME),
        Some(TEAM.to_owned()),
        FakeSigning {
            team: Some(TEAM.to_owned()),
            check: Ok(()),
        },
        FakeRelauncher {
            calls: Arc::clone(&calls),
            result: Ok(()),
        },
    );
    assert!(installer.restart().is_ok());
    assert_eq!(calls.load(Ordering::SeqCst), 1);

    let error_installer = MacAppInstaller::new(
        temp.path().join(APP_BUNDLE_NAME),
        Some(TEAM.to_owned()),
        FakeSigning {
            team: Some(TEAM.to_owned()),
            check: Ok(()),
        },
        FakeRelauncher {
            calls: Arc::clone(&calls),
            result: Err("x".to_owned()),
        },
    );
    assert_eq!(error_installer.restart(), Err("x".to_owned()));
    assert_eq!(calls.load(Ordering::SeqCst), 2);
}

#[test]
fn kickstart_args_format() {
    assert_eq!(
        kickstart_args(501, AGENT_LABEL),
        vec![
            "kickstart".to_owned(),
            "-k".to_owned(),
            "gui/501/ai.gxlself.remotehost.agent".to_owned()
        ]
    );
}

#[test]
fn app_bundle_of_paths() {
    assert_eq!(
        app_bundle_of(Path::new(
            "/Applications/Remote Host.app/Contents/MacOS/rs-host-agent",
        )),
        Some(PathBuf::from("/Applications/Remote Host.app"))
    );
    assert_eq!(
        app_bundle_of(Path::new("/usr/local/bin/rs-host-agent")),
        None
    );
    assert_eq!(
        app_bundle_of(Path::new("/x/Foo.app/Contents/Resources/rs-host-agent")),
        None
    );
    assert_eq!(
        app_bundle_of(Path::new("/x/Foo/Contents/MacOS/rs-host-agent")),
        None
    );
    assert_eq!(
        app_bundle_of(Path::new("/x/Foo.app/Contents/MacOS/nested/rs-host-agent")),
        None
    );
    assert_eq!(
        app_bundle_of(Path::new("Foo.app/Contents/MacOS/arbitrary-name")),
        Some(PathBuf::from("Foo.app"))
    );
}

#[test]
fn swap_dirs_swaps() {
    let temp = TempDir::new();
    let a = temp.path().join("a");
    let b = temp.path().join("b");
    fs::create_dir(&a).expect("create first directory");
    fs::create_dir(&b).expect("create second directory");
    fs::write(a.join("marker"), b"a").expect("write first marker");
    fs::write(b.join("marker"), b"b").expect("write second marker");

    swap_dirs(&a, &b).expect("swap directories");
    assert_eq!(
        fs::read(a.join("marker")).expect("read swapped first marker"),
        b"b"
    );
    assert_eq!(
        fs::read(b.join("marker")).expect("read swapped second marker"),
        b"a"
    );
    assert!(swap_dirs(&a, &temp.path().join("missing")).is_err());
    assert_eq!(
        fs::read(a.join("marker")).expect("read unswapped marker"),
        b"b"
    );
    assert_eq!(fs::read(b.join("marker")).expect("read other marker"), b"a");
}

#[test]
fn installer_is_object_safe() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME, "1");
    let update = package(temp.path(), APP_BUNDLE_NAME, "2");
    let concrete = installer(temp.path());
    let staging = concrete.staging_dir();
    let installer: Arc<dyn Installer> = Arc::new(concrete);

    installer
        .install(&update, &temp.path().join("backup/1"))
        .expect("install through trait object");
    assert_eq!(ver(&app), "2");
    assert_eq!(
        ver(&temp.path().join("backup/1").join(APP_BUNDLE_NAME)),
        "1"
    );
    assert!(!staging.exists());
}
