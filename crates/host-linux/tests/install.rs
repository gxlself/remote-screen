#![cfg(target_os = "linux")]

use std::ffi::OsString;
use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use host_linux::install::*;
use host_linux::update_appimage::{Extractor, HOST_UNIT, NOTIFY_UNIT, VERSION_FILE};

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = base.join(format!(
        "rs-host-install-{label}-{}-{stamp}",
        std::process::id()
    ));
    fs::create_dir_all(&path).expect("temp");
    path
}
fn executable(path: &Path, text: &[u8]) {
    fs::create_dir_all(path.parent().expect("parent")).expect("parent");
    fs::write(path, text).expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("mode");
}
fn payload(root: &Path, version: &str) {
    executable(&root.join("AppRun"), b"#!/bin/sh\n");
    executable(&root.join("usr/bin/rs-host"), b"#!/bin/sh\n");
    executable(&root.join("usr/bin/remote-host-ctl"), b"#!/bin/sh\n");
    fs::create_dir_all(root.join("usr/share/remote-host")).expect("share");
    fs::write(root.join(VERSION_FILE), version).expect("version");
}
struct FakeExtractor {
    version: String,
}
impl Extractor for FakeExtractor {
    fn extract(&self, _: &Path, into: &Path) -> Result<PathBuf, String> {
        let root = into.join("squashfs-root");
        fs::create_dir_all(&root).map_err(|e| e.to_string())?;
        payload(&root, &self.version);
        Ok(root)
    }
}
#[derive(Clone, Default)]
struct FakeSystemctl(Arc<Mutex<Vec<Vec<String>>>>);
impl Systemctl for FakeSystemctl {
    fn user(&self, args: &[&str]) -> Result<(), String> {
        self.0
            .lock()
            .expect("lock")
            .push(args.iter().map(|arg| (*arg).to_owned()).collect());
        Ok(())
    }
}
#[test]
fn unit_text() {
    let unit = host_unit(Path::new("/home/u/.local/lib/remote-host")).expect("unit");
    assert!(unit.contains(
        "ExecStart=\"/home/u/.local/lib/remote-host/current/usr/bin/rs-host\" --platform linux"
    ));
    assert!(unit.contains("WantedBy=graphical-session.target"));
    assert!(notify_unit(Path::new("/home/u/.local/lib/remote-host"))
        .expect("notify")
        .contains("remote-host-ctl\" notify"));
    assert!(matches!(
        host_unit(Path::new("relative")),
        Err(InstallError::BadPath(_))
    ));
    assert!(matches!(
        host_unit(Path::new("/bad%path")),
        Err(InstallError::BadPath(_))
    ));
}

#[test]
fn fresh_install() {
    let home = temp_dir("fresh");
    let paths = InstallPaths::under(&home, None, None);
    let ctl = FakeSystemctl::default();
    let report = install(
        Path::new("package"),
        &paths,
        &FakeExtractor {
            version: "1.0.0".to_owned(),
        },
        &ctl,
    )
    .expect("install");
    assert_eq!(report.upgraded_from, None);
    assert!(paths.lib_dir.join("app-1.0.0").exists());
    assert_eq!(
        fs::read_link(paths.lib_dir.join("current")).expect("current"),
        PathBuf::from("app-1.0.0")
    );
    assert_eq!(
        fs::read_to_string(paths.unit_dir.join(HOST_UNIT)).expect("unit"),
        host_unit(&paths.lib_dir).expect("unit")
    );
    assert_eq!(
        fs::read_to_string(paths.unit_dir.join(NOTIFY_UNIT)).expect("notify"),
        notify_unit(&paths.lib_dir).expect("notify")
    );
    assert!(fs::read_link(paths.bin_dir.join("remote-host-ctl"))
        .expect("link")
        .ends_with("current/usr/bin/remote-host-ctl"));
    assert_eq!(
        ctl.0.lock().expect("lock").as_slice(),
        &[
            vec!["daemon-reload".to_owned()],
            vec![
                "enable".to_owned(),
                "--now".to_owned(),
                HOST_UNIT.to_owned(),
                NOTIFY_UNIT.to_owned()
            ]
        ]
    );
    let _ = fs::remove_dir_all(home);
}

#[test]
fn upgrade_install() {
    let home = temp_dir("upgrade");
    let paths = InstallPaths::under(&home, None, None);
    let ctl = FakeSystemctl::default();
    install(
        Path::new("package"),
        &paths,
        &FakeExtractor {
            version: "1.0.0".to_owned(),
        },
        &ctl,
    )
    .expect("first");
    ctl.0.lock().expect("lock").clear();
    let report = install(
        Path::new("package"),
        &paths,
        &FakeExtractor {
            version: "1.1.0".to_owned(),
        },
        &ctl,
    )
    .expect("upgrade");
    assert_eq!(report.upgraded_from.as_deref(), Some("1.0.0"));
    assert_eq!(
        ctl.0.lock().expect("lock").as_slice(),
        &[
            vec![
                "stop".to_owned(),
                NOTIFY_UNIT.to_owned(),
                HOST_UNIT.to_owned()
            ],
            vec!["daemon-reload".to_owned()],
            vec![
                "enable".to_owned(),
                HOST_UNIT.to_owned(),
                NOTIFY_UNIT.to_owned()
            ],
            vec![
                "restart".to_owned(),
                HOST_UNIT.to_owned(),
                NOTIFY_UNIT.to_owned()
            ]
        ]
    );
    let _ = fs::remove_dir_all(home);
}

#[test]
fn bin_conflict() {
    let home = temp_dir("conflict");
    let paths = InstallPaths::under(&home, None, None);
    fs::create_dir_all(&paths.bin_dir).expect("bin");
    fs::write(paths.bin_dir.join("remote-host-ctl"), b"keep").expect("file");
    let report = install(
        Path::new("package"),
        &paths,
        &FakeExtractor {
            version: "1.0.0".to_owned(),
        },
        &FakeSystemctl::default(),
    )
    .expect("install");
    assert!(report.bin_link_skipped);
    assert_eq!(
        fs::read(paths.bin_dir.join("remote-host-ctl")).expect("file"),
        b"keep"
    );
    let _ = fs::remove_dir_all(home);
}

#[test]
fn uninstall_keeps_state() {
    let home = temp_dir("uninstall");
    let paths = InstallPaths::under(&home, None, None);
    let ctl = FakeSystemctl::default();
    install(
        Path::new("package"),
        &paths,
        &FakeExtractor {
            version: "1.0.0".to_owned(),
        },
        &ctl,
    )
    .expect("install");
    fs::create_dir_all(&paths.state_dir).expect("state");
    uninstall(&paths, &ctl, false).expect("uninstall");
    assert!(paths.state_dir.exists());
    assert!(!paths.lib_dir.exists());
    assert!(!paths.unit_dir.join(HOST_UNIT).exists());
    install(
        Path::new("package"),
        &paths,
        &FakeExtractor {
            version: "1.0.0".to_owned(),
        },
        &ctl,
    )
    .expect("reinstall");
    fs::create_dir_all(&paths.state_dir).expect("state");
    uninstall(&paths, &ctl, true).expect("purge");
    assert!(!paths.state_dir.exists());
    let _ = fs::remove_dir_all(home);
}

#[test]
fn find_curl() {
    let dir = temp_dir("path");
    let curl = dir.join("curl");
    executable(&curl, b"#!/bin/sh\n");
    let path = OsString::from(dir.as_os_str());
    assert_eq!(find_in_path("curl", Some(&path)), Some(curl.clone()));
    fs::set_permissions(&curl, fs::Permissions::from_mode(0o644)).expect("mode");
    assert_eq!(find_in_path("curl", Some(&path)), None);
    assert_eq!(find_in_path("curl", None), None);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn apprun_dispatch() {
    let dir = temp_dir("apprun");
    let appdir = dir.join("app");
    executable(
        &appdir.join("usr/bin/rs-host"),
        b"#!/bin/sh\nprintf '%s %s\\n' \"$1\" \"$2\"\n",
    );
    fs::create_dir_all(appdir.join("usr/bin")).expect("bin");
    executable(
        &appdir.join("usr/bin/remote-host-ctl"),
        b"#!/bin/sh\nexit 0\n",
    );
    fs::create_dir_all(appdir.join("usr/share/remote-host")).expect("share");
    fs::write(appdir.join("usr/share/remote-host/VERSION"), "1.2.3\n").expect("version");
    let apprun = Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging/AppRun");
    let output = Command::new("sh")
        .arg(&apprun)
        .env("APPDIR", &appdir)
        .args(["host", "--a", "b"])
        .output()
        .expect("run");
    assert_eq!(String::from_utf8_lossy(&output.stdout), "--a b\n");
    let version = Command::new("sh")
        .arg(&apprun)
        .env("APPDIR", &appdir)
        .arg("--version")
        .output()
        .expect("run");
    assert_eq!(String::from_utf8_lossy(&version.stdout), "1.2.3\n");
    assert_eq!(
        Command::new("sh")
            .arg(&apprun)
            .env("APPDIR", &appdir)
            .status()
            .expect("run")
            .code(),
        Some(2)
    );
    assert_eq!(
        Command::new("sh")
            .arg(&apprun)
            .env("APPDIR", &appdir)
            .arg("foo")
            .status()
            .expect("run")
            .code(),
        Some(2)
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn scripts_parse() {
    let base = Path::new(env!("CARGO_MANIFEST_DIR")).join("packaging");
    assert!(Command::new("bash")
        .args(["-n", base.join("build-appimage.sh").to_str().expect("path")])
        .status()
        .expect("bash")
        .success());
    assert!(Command::new("bash")
        .args(["-n", base.join("fetch-ffmpeg.sh").to_str().expect("path")])
        .status()
        .expect("bash")
        .success());
    assert!(Command::new("sh")
        .args(["-n", base.join("AppRun").to_str().expect("path")])
        .status()
        .expect("sh")
        .success());
    let lock = temp_dir("lock").join("ffmpeg.lock");
    fs::write(&lock, "url=\nsha256=\n").expect("lock");
    let status = Command::new("bash")
        .arg(base.join("fetch-ffmpeg.sh"))
        .env("RS_FFMPEG_LOCK", &lock)
        .status()
        .expect("script");
    assert_eq!(status.code(), Some(3));
}
