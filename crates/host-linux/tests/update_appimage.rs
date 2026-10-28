#![cfg(target_os = "linux")]

use std::fs;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

use host_core::update::install::Installer;
use host_core::update::prepare::PackageVerifier;
use host_linux::update_appimage::*;

fn temp_dir(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let base = std::env::var_os("TMPDIR")
        .map(PathBuf::from)
        .unwrap_or_else(std::env::temp_dir);
    let path = base.join(format!("rs-host-{label}-{}-{stamp}", std::process::id()));
    fs::create_dir_all(&path).expect("temp dir");
    path
}

fn executable(path: &Path, contents: &[u8]) {
    if let Some(parent) = path.parent() {
        fs::create_dir_all(parent).expect("parent");
    }
    fs::write(path, contents).expect("write");
    fs::set_permissions(path, fs::Permissions::from_mode(0o755)).expect("chmod");
}

fn payload(root: &Path, version: &str) {
    executable(&root.join("AppRun"), b"#!/bin/sh\n");
    executable(&root.join("usr/bin/rs-host"), b"#!/bin/sh\n");
    executable(&root.join("usr/bin/remote-host-ctl"), b"#!/bin/sh\n");
    fs::create_dir_all(root.join("usr/share/remote-host")).expect("version dir");
    fs::write(root.join(VERSION_FILE), version).expect("version");
}

struct FakeExtractor {
    version: String,
}

impl Extractor for FakeExtractor {
    fn extract(&self, _appimage: &Path, into: &Path) -> Result<PathBuf, String> {
        let root = into.join("squashfs-root");
        fs::create_dir_all(&root).map_err(|error| error.to_string())?;
        payload(&root, &self.version);
        Ok(root)
    }
}

#[derive(Clone, Default)]
struct FakeService(Arc<Mutex<usize>>);

impl ServiceControl for FakeService {
    fn restart_host(&self) -> Result<(), String> {
        *self.0.lock().expect("lock") += 1;
        Ok(())
    }
}

#[test]
fn verifier_checks() {
    let dir = temp_dir("verifier");
    let package = dir.join("package.bin");
    let mut bytes = vec![0u8; 20];
    bytes[..4].copy_from_slice(b"\x7fELF");
    bytes[4] = 2;
    bytes[5] = 1;
    bytes[8..11].copy_from_slice(b"AI\x02");
    bytes[18..20].copy_from_slice(&62u16.to_le_bytes());
    fs::write(&package, &bytes).expect("package");
    let verifier = AppImageVerifier { machine: 62 };
    assert!(verifier.verify_platform(&package).is_ok());
    for (index, expected) in [
        (0, "not an ELF file"),
        (4, "unsupported ELF class"),
        (8, "not a type-2 AppImage"),
        (18, "wrong architecture"),
    ] {
        let mut changed = bytes.clone();
        if index == 0 {
            changed[0] = 0;
        } else if index == 4 {
            changed[4] = 1;
        } else if index == 8 {
            changed[8] = b'X';
        } else {
            changed[18] = 183;
        }
        fs::write(&package, changed).expect("package");
        assert_eq!(verifier.verify_platform(&package).unwrap_err(), expected);
    }
    fs::write(&package, [0u8; 10]).expect("short package");
    assert_eq!(
        verifier.verify_platform(&package).unwrap_err(),
        "package too short"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn install_switch_and_rollback() {
    let dir = temp_dir("install");
    let lib = dir.join("lib");
    fs::create_dir_all(lib.join("app-1.0.0")).expect("old");
    switch_current(&lib, "app-1.0.0").expect("current");
    let service = FakeService::default();
    let installer = AppImageInstaller::new(
        lib.clone(),
        FakeExtractor {
            version: "1.1.0".into(),
        },
        service.clone(),
    );
    let backup = dir.join("backup");
    installer
        .install(Path::new("package"), &backup)
        .expect("install");
    assert_eq!(current_target(&lib).as_deref(), Some("app-1.1.0"));
    assert_eq!(
        fs::read_to_string(backup.join("previous")).expect("previous"),
        "app-1.0.0"
    );
    assert!(!fs::read_dir(&lib).expect("lib").any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with("staging-")));
    installer.restart().expect("restart");
    assert_eq!(*service.0.lock().expect("lock"), 1);
    installer.rollback(&backup).expect("rollback");
    assert_eq!(current_target(&lib).as_deref(), Some("app-1.0.0"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn prune_keeps_two() {
    let dir = temp_dir("prune");
    let lib = dir.join("lib");
    for version in ["0.9.0", "1.0.0"] {
        fs::create_dir_all(lib.join(format!("app-{version}"))).expect("old");
    }
    switch_current(&lib, "app-1.0.0").expect("current");
    let installer = AppImageInstaller::new(
        lib.clone(),
        FakeExtractor {
            version: "1.1.0".into(),
        },
        FakeService::default(),
    );
    installer
        .install(Path::new("package"), &dir.join("backup"))
        .expect("install");
    assert!(!lib.join("app-0.9.0").exists());
    assert!(lib.join("app-1.0.0").exists());
    assert!(lib.join("app-1.1.0").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn bad_payload_untouched() {
    let dir = temp_dir("bad");
    let lib = dir.join("lib");
    fs::create_dir_all(lib.join("app-1.0.0")).expect("old");
    switch_current(&lib, "app-1.0.0").expect("current");
    struct BadExtractor;
    impl Extractor for BadExtractor {
        fn extract(&self, _: &Path, into: &Path) -> Result<PathBuf, String> {
            let root = into.join("squashfs-root");
            fs::create_dir_all(&root).map_err(|error| error.to_string())?;
            fs::create_dir_all(root.join("usr/share/remote-host"))
                .map_err(|error| error.to_string())?;
            fs::write(root.join(VERSION_FILE), "../x").map_err(|error| error.to_string())?;
            Ok(root)
        }
    }
    let installer = AppImageInstaller::new(lib.clone(), BadExtractor, FakeService::default());
    assert!(installer
        .install(Path::new("package"), &dir.join("backup"))
        .is_err());
    assert_eq!(current_target(&lib).as_deref(), Some("app-1.0.0"));
    assert!(!lib.join("app-../x").exists());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn same_version_rejected() {
    let dir = temp_dir("same");
    let lib = dir.join("lib");
    fs::create_dir_all(lib.join("app-1.0.0")).expect("old");
    switch_current(&lib, "app-1.0.0").expect("current");
    let installer = AppImageInstaller::new(
        lib.clone(),
        FakeExtractor {
            version: "1.0.0".into(),
        },
        FakeService::default(),
    );
    assert_eq!(
        installer
            .install(Path::new("package"), &dir.join("backup"))
            .unwrap_err(),
        "same version is already installed"
    );
    assert_eq!(current_target(&lib).as_deref(), Some("app-1.0.0"));
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn not_installed() {
    let dir = temp_dir("none");
    let installer = AppImageInstaller::new(
        dir.join("lib"),
        FakeExtractor {
            version: "1.0.0".into(),
        },
        FakeService::default(),
    );
    assert_eq!(
        installer
            .install(Path::new("package"), &dir.join("backup"))
            .unwrap_err(),
        "not an installed AppImage layout"
    );
    fs::create_dir_all(dir.join("lib/current")).expect("directory");
    assert_eq!(
        installer
            .install(Path::new("package"), &dir.join("backup"))
            .unwrap_err(),
        "not an installed AppImage layout"
    );
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn layout_detection() {
    let dir = temp_dir("layout");
    let lib = dir.join("lib");
    let exe = lib.join("app-1.0.0/usr/bin/rs-host");
    executable(&exe, b"#!/bin/sh\n");
    assert!(installed_layout(&exe, &lib));
    assert!(update_platform(&exe, &lib).is_some());
    let mounted = dir.join(".mount_x/usr/bin/rs-host");
    executable(&mounted, b"#!/bin/sh\n");
    assert!(!installed_layout(&mounted, &lib));
    assert!(update_platform(&mounted, &lib).is_none());
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn runtime_extractor() {
    let dir = temp_dir("runtime");
    let appimage = dir.join("fake.AppImage");
    fs::write(&appimage, b"#!/bin/sh\nif [ \"$1\" = \"--appimage-extract\" ]; then mkdir -p squashfs-root; printf ok > squashfs-root/VERSION; exit 0; fi\nprintf 'bad stderr'; exit 1\n").expect("script");
    fs::set_permissions(&appimage, fs::Permissions::from_mode(0o755)).expect("chmod");
    let root = RuntimeExtractor
        .extract(&appimage, &dir.join("staging"))
        .expect("extract");
    assert_eq!(root, dir.join("staging/squashfs-root"));
    let bad = dir.join("bad.AppImage");
    fs::write(&bad, b"#!/bin/sh\necho fail >&2; exit 1\n").expect("script");
    fs::set_permissions(&bad, fs::Permissions::from_mode(0o755)).expect("chmod");
    let error = RuntimeExtractor
        .extract(&bad, &dir.join("bad-staging"))
        .unwrap_err();
    assert!(error.len() <= 200);
    let _ = fs::remove_dir_all(dir);
}

#[test]
fn switch_is_atomic_shape() {
    let dir = temp_dir("atomic");
    fs::create_dir_all(dir.join("app-1.0.0")).expect("app");
    switch_current(&dir, "app-1.0.0").expect("switch");
    assert_eq!(
        fs::read_link(dir.join("current")).expect("link"),
        PathBuf::from("app-1.0.0")
    );
    assert!(!dir.join("current.new").exists());
    let _ = fs::remove_dir_all(dir);
}
