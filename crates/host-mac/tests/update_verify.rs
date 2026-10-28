#![cfg(target_os = "macos")]

use std::fs::{self, OpenOptions};
use std::io::Write;
use std::os::unix::fs::PermissionsExt;
use std::path::{Path, PathBuf};
use std::process::Command;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::{Arc, Mutex};

use host_core::update::prepare::PackageVerifier;
use host_mac::update_verify::{
    app_requirement, extract_update_zip, team_matches, validate_layout, CodeSigning,
    MacCodeVerifier, SecCodeSigning, APP_BUNDLE_NAME,
};

static TEMP_SEQUENCE: AtomicU64 = AtomicU64::new(0);

struct TempDir {
    path: PathBuf,
}

impl TempDir {
    fn new() -> Self {
        let sequence = TEMP_SEQUENCE.fetch_add(1, Ordering::Relaxed);
        let path = std::env::temp_dir().join(format!(
            "remote-screen-update-verify-{}-{sequence}",
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
        let _ = fs::remove_dir_all(&self.path);
    }
}

fn info_plist() -> &'static [u8] {
    br#"<?xml version="1.0" encoding="UTF-8"?>
<!DOCTYPE plist PUBLIC "-//Apple//DTD PLIST 1.0//EN" "http://www.apple.com/DTDs/PropertyList-1.0.dtd">
<plist version="1.0"><dict>
<key>CFBundleIdentifier</key><string>ai.gxlself.remotehost</string>
<key>CFBundleExecutable</key><string>rs-host-agent</string>
<key>CFBundlePackageType</key><string>APPL</string>
<key>CFBundleName</key><string>Remote Host</string>
</dict></plist>
"#
}

fn fake_app(root: &Path, name: &str) -> PathBuf {
    let app = root.join(name);
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    fs::create_dir_all(&macos).expect("create fake app directories");
    fs::write(contents.join("Info.plist"), info_plist()).expect("write fake Info.plist");
    fs::copy("/usr/bin/true", macos.join("rs-host-agent")).expect("copy true into fake app");
    app
}

fn unsigned_fake_app(root: &Path, name: &str) -> PathBuf {
    let app = root.join(name);
    let contents = app.join("Contents");
    let macos = contents.join("MacOS");
    fs::create_dir_all(&macos).expect("create unsigned app directories");
    fs::write(contents.join("Info.plist"), info_plist()).expect("write unsigned Info.plist");
    fs::write(macos.join("rs-host-agent"), b"#!/bin/sh\nexit 0\n")
        .expect("write unsigned executable");
    app
}

fn zip_keep_parent(app: &Path, output: &Path) {
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

fn zip_without_keep_parent(source: &Path, output: &Path) {
    let status = Command::new("/usr/bin/ditto")
        .arg("-c")
        .arg("-k")
        .arg("--sequesterRsrc")
        .arg(source)
        .arg(output)
        .status()
        .expect("run ditto to create malformed update zip");
    assert!(status.success(), "ditto failed with {status}");
}

fn set_test_extended_attribute(path: &Path) {
    let status = Command::new("/usr/bin/xattr")
        .args(["-w", "com.apple.quarantine", "0081;65000000;Remote Host;"])
        .arg(path)
        .status()
        .expect("set test extended attribute");
    assert!(status.success(), "xattr failed with {status}");
}

fn crc32(data: &[u8]) -> u32 {
    let mut crc = u32::MAX;
    for byte in data {
        crc ^= u32::from(*byte);
        for _ in 0..8 {
            let mask = 0_u32.wrapping_sub(crc & 1);
            crc = (crc >> 1) ^ (0xedb8_8320 & mask);
        }
    }
    !crc
}

fn push_u16(buffer: &mut Vec<u8>, value: u16) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

fn push_u32(buffer: &mut Vec<u8>, value: u32) {
    buffer.extend_from_slice(&value.to_le_bytes());
}

/// Write a minimal stored-only archive so malformed path names can be tested
/// without a zip library or a command that normalizes `..` and `/` entries.
fn write_stored_zip(output: &Path, entries: &[(&str, &[u8])]) {
    let mut archive = Vec::new();
    let mut local_offsets = Vec::with_capacity(entries.len());
    for (name, data) in entries {
        let name = name.as_bytes();
        let offset = u32::try_from(archive.len()).expect("zip offset fits in u32");
        let size = u32::try_from(data.len()).expect("zip entry fits in u32");
        local_offsets.push(offset);
        push_u32(&mut archive, 0x0403_4b50);
        push_u16(&mut archive, 20);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u32(&mut archive, crc32(data));
        push_u32(&mut archive, size);
        push_u32(&mut archive, size);
        push_u16(
            &mut archive,
            u16::try_from(name.len()).expect("zip name fits in u16"),
        );
        push_u16(&mut archive, 0);
        archive.extend_from_slice(name);
        archive.extend_from_slice(data);
    }

    let central_offset = u32::try_from(archive.len()).expect("zip offset fits in u32");
    for ((name, data), local_offset) in entries.iter().zip(local_offsets) {
        let name = name.as_bytes();
        let size = u32::try_from(data.len()).expect("zip entry fits in u32");
        push_u32(&mut archive, 0x0201_4b50);
        push_u16(&mut archive, 20);
        push_u16(&mut archive, 20);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u32(&mut archive, crc32(data));
        push_u32(&mut archive, size);
        push_u32(&mut archive, size);
        push_u16(
            &mut archive,
            u16::try_from(name.len()).expect("zip name fits in u16"),
        );
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u16(&mut archive, 0);
        push_u32(&mut archive, 0);
        push_u32(&mut archive, local_offset);
        archive.extend_from_slice(name);
    }
    let central_size = u32::try_from(archive.len())
        .expect("zip offset fits in u32")
        .saturating_sub(central_offset);
    let count = u16::try_from(entries.len()).expect("zip entry count fits in u16");
    push_u32(&mut archive, 0x0605_4b50);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, 0);
    push_u16(&mut archive, count);
    push_u16(&mut archive, count);
    push_u32(&mut archive, central_size);
    push_u32(&mut archive, central_offset);
    push_u16(&mut archive, 0);
    fs::write(output, archive).expect("write stored zip");
}

fn assert_destination_empty(path: &Path) {
    match fs::read_dir(path) {
        Ok(entries) => assert!(
            entries
                .collect::<Result<Vec<_>, _>>()
                .expect("read extraction destination")
                .is_empty(),
            "failed extraction left files in destination"
        ),
        Err(error) if error.kind() == std::io::ErrorKind::NotFound => {}
        Err(error) => panic!("could not inspect extraction destination: {error}"),
    }
}

fn adhoc_sign(app: &Path) {
    let status = Command::new("/usr/bin/codesign")
        .arg("--force")
        .arg("--deep")
        .arg("--sign")
        .arg("-")
        .arg(app)
        .status()
        .expect("run codesign");
    assert!(status.success(), "codesign failed with {status}");
}

fn assert_no_verify_directories(root: &Path) {
    for entry in fs::read_dir(root).expect("read test package directory") {
        let entry = entry.expect("read test package entry");
        let name = entry.file_name();
        assert!(
            !name.to_string_lossy().starts_with("verify-"),
            "temporary verification directory left behind: {}",
            name.to_string_lossy()
        );
    }
}

#[derive(Clone)]
struct FakeSigning {
    current: Option<String>,
    team: Option<String>,
    check: Result<(), String>,
    requirements: Arc<Mutex<Vec<Option<String>>>>,
}

impl FakeSigning {
    fn new(
        team: Option<&str>,
        check: Result<(), String>,
    ) -> (Self, Arc<Mutex<Vec<Option<String>>>>) {
        let requirements = Arc::new(Mutex::new(Vec::new()));
        (
            Self {
                current: None,
                team: team.map(str::to_owned),
                check,
                requirements: Arc::clone(&requirements),
            },
            requirements,
        )
    }
}

impl CodeSigning for FakeSigning {
    fn current_team_id(&self) -> Result<Option<String>, String> {
        Ok(self.current.clone())
    }

    fn team_id(&self, _path: &Path) -> Result<Option<String>, String> {
        Ok(self.team.clone())
    }

    fn check(&self, _path: &Path, requirement: Option<&str>) -> Result<(), String> {
        self.requirements
            .lock()
            .expect("lock fake signing requirements")
            .push(requirement.map(str::to_owned));
        self.check.clone()
    }
}

#[test]
fn requirement_is_exact() {
    assert_eq!(
        app_requirement("ABCDE12345").expect("valid Team ID"),
        "identifier \"ai.gxlself.remotehost\" and anchor apple generic and certificate 1[field.1.2.840.113635.100.6.2.6] exists and certificate leaf[field.1.2.840.113635.100.6.1.13] exists and certificate leaf[subject.OU] = \"ABCDE12345\""
    );
    for invalid in ["abcde12345", "ABCDE1234", "ABCDE12345\"", ""] {
        assert!(
            app_requirement(invalid).is_err(),
            "accepted invalid Team ID {invalid:?}"
        );
    }
}

#[test]
fn team_matches_rules() {
    let error = team_matches(None, Some("ABCDE12345")).expect_err("unsigned current app accepted");
    assert!(error.contains("refusing update"));
    let error = team_matches(Some("ABCDE12345"), None).expect_err("unsigned package accepted");
    assert!(error.contains("not signed"));
    let error = team_matches(Some("ABCDE12345"), Some("ZZZZZ99999"))
        .expect_err("mismatched package accepted");
    assert!(error.contains("mismatch"));
    assert!(team_matches(Some("ABCDE12345"), Some("ABCDE12345")).is_ok());
}

#[test]
fn extract_valid_zip() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME);
    let zip = temp.path().join("update.zip");
    zip_keep_parent(&app, &zip);
    let dest = temp.path().join("extract");
    let extracted = extract_update_zip(&zip, &dest).expect("extract valid update zip");
    assert_eq!(extracted, dest.join(APP_BUNDLE_NAME));
    assert!(extracted.join("Contents/MacOS/rs-host-agent").is_file());
    assert_eq!(
        fs::metadata(&dest)
            .expect("stat extraction destination")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
}

#[test]
fn extract_accepts_ditto_appledouble_entries() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME);
    set_test_extended_attribute(&app.join("Contents/Info.plist"));
    let zip = temp.path().join("appledouble.zip");
    zip_keep_parent(&app, &zip);
    let listing = Command::new("/usr/bin/zipinfo")
        .arg("-1")
        .arg(&zip)
        .output()
        .expect("list AppleDouble update zip");
    assert!(listing.status.success(), "zipinfo failed");
    assert!(
        String::from_utf8_lossy(&listing.stdout)
            .lines()
            .any(|name| name == "__MACOSX/"),
        "ditto archive did not contain __MACOSX/"
    );
    let dest = temp.path().join("appledouble-extract");
    let extracted = extract_update_zip(&zip, &dest).expect("extract AppleDouble update zip");
    assert_eq!(extracted, dest.join(APP_BUNDLE_NAME));
    assert!(!dest.join("__MACOSX").exists());
}

#[test]
fn extract_rejects_bad_layouts() {
    let temp = TempDir::new();

    let other = fake_app(temp.path(), "Other.app");
    let other_zip = temp.path().join("other.zip");
    zip_keep_parent(&other, &other_zip);
    assert!(extract_update_zip(&other_zip, &temp.path().join("other-extract")).is_err());

    let multiple = temp.path().join("multiple-source");
    fs::create_dir_all(&multiple).expect("create multiple-entry source");
    fake_app(&multiple, APP_BUNDLE_NAME);
    fs::write(multiple.join("extra.txt"), b"extra").expect("write extra top-level entry");
    let multiple_zip = temp.path().join("multiple.zip");
    zip_without_keep_parent(&multiple, &multiple_zip);
    assert!(extract_update_zip(&multiple_zip, &temp.path().join("multiple-extract")).is_err());

    let missing = fake_app(temp.path(), "Missing.app");
    fs::remove_file(missing.join("Contents/MacOS/rs-host-agent")).expect("remove agent");
    let missing_zip = temp.path().join("missing.zip");
    zip_keep_parent(&missing, &missing_zip);
    assert!(extract_update_zip(&missing_zip, &temp.path().join("missing-extract")).is_err());

    let not_zip = temp.path().join("not.zip");
    fs::write(&not_zip, b"not a zip").expect("write invalid zip");
    assert!(extract_update_zip(&not_zip, &temp.path().join("not-extract")).is_err());

    let valid = fake_app(temp.path(), "Valid.app");
    let valid_zip = temp.path().join("valid.zip");
    zip_keep_parent(&valid, &valid_zip);
    let nonempty = temp.path().join("nonempty");
    fs::create_dir_all(&nonempty).expect("create nonempty destination");
    let marker = nonempty.join("keep.txt");
    fs::write(&marker, b"keep").expect("write destination marker");
    assert!(extract_update_zip(&valid_zip, &nonempty).is_err());
    assert_eq!(fs::read(&marker).expect("read destination marker"), b"keep");

    let symlink_layout = temp.path().join("symlink-layout");
    fs::create_dir_all(&symlink_layout).expect("create symlink layout");
    let target_root = temp.path().join("symlink-target-root");
    fs::create_dir_all(&target_root).expect("create symlink target root");
    let symlink_target = fake_app(&target_root, APP_BUNDLE_NAME);
    std::os::unix::fs::symlink(&symlink_target, symlink_layout.join(APP_BUNDLE_NAME))
        .expect("create app symlink");
    assert!(validate_layout(&symlink_layout).is_err());
}

#[test]
fn extract_rejects_symlink_escape_before_writing() {
    let temp = TempDir::new();
    let outside_temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME);
    let resources = app.join("Contents/Resources");
    fs::create_dir_all(&resources).expect("create app resources");
    let outside = outside_temp.path();
    std::os::unix::fs::symlink(outside, resources.join("link"))
        .expect("create escaping app symlink");

    let zip = temp.path().join("symlink-escape.zip");
    let status = Command::new("/usr/bin/zip")
        .arg("-q")
        .arg("-y")
        .arg("-r")
        .arg(&zip)
        .arg(APP_BUNDLE_NAME)
        .current_dir(temp.path())
        .status()
        .expect("create symlink archive");
    assert!(status.success(), "zip failed with {status}");

    let append_root = temp.path().join("append");
    let append_file = append_root.join("Remote Host.app/Contents/Resources/link/evil");
    fs::create_dir_all(append_file.parent().expect("append file parent"))
        .expect("create appended file directory");
    fs::write(&append_file, b"evil").expect("write appended file");
    let status = Command::new("/usr/bin/zip")
        .arg("-q")
        .arg("-r")
        .arg(&zip)
        .arg("Remote Host.app/Contents/Resources/link/evil")
        .current_dir(&append_root)
        .status()
        .expect("append symlink escape entry");
    assert!(status.success(), "zip append failed with {status}");

    let dest = temp.path().join("symlink-escape-extract");
    assert!(extract_update_zip(&zip, &dest).is_err());
    assert!(!outside.join("evil").exists());
    assert_destination_empty(&dest);
}

#[test]
fn extract_rejects_parent_entry() {
    let temp = TempDir::new();
    let zip = temp.path().join("parent.zip");
    write_stored_zip(&zip, [("../evil", b"evil".as_slice())].as_slice());
    let dest = temp.path().join("parent-extract");
    assert!(extract_update_zip(&zip, &dest).is_err());
    assert_destination_empty(&dest);
}

#[test]
fn extract_rejects_absolute_entry() {
    let temp = TempDir::new();
    let zip = temp.path().join("absolute.zip");
    write_stored_zip(&zip, [("/tmp/evil", b"evil".as_slice())].as_slice());
    let dest = temp.path().join("absolute-extract");
    assert!(extract_update_zip(&zip, &dest).is_err());
    assert_destination_empty(&dest);
}

#[test]
fn extract_rejects_invalid_macosx_entries() {
    let temp = TempDir::new();
    let parent_zip = temp.path().join("macosx-parent.zip");
    write_stored_zip(
        &parent_zip,
        [("__MACOSX/../evil", b"evil".as_slice())].as_slice(),
    );
    let parent_dest = temp.path().join("macosx-parent-extract");
    assert!(extract_update_zip(&parent_zip, &parent_dest).is_err());
    assert_destination_empty(&parent_dest);

    let other_zip = temp.path().join("macosx-other.zip");
    write_stored_zip(
        &other_zip,
        [("__MACOSX/Other.app/x", b"evil".as_slice())].as_slice(),
    );
    let other_dest = temp.path().join("macosx-other-extract");
    assert!(extract_update_zip(&other_zip, &other_dest).is_err());
    assert_destination_empty(&other_dest);
}

#[test]
fn system_app_passes_apple_anchor() {
    let signing = SecCodeSigning;
    signing
        .check(
            Path::new("/System/Applications/Calculator.app"),
            Some("anchor apple"),
        )
        .expect("Apple Calculator should satisfy anchor apple");
    assert_eq!(
        signing
            .team_id(Path::new("/System/Applications/Calculator.app"))
            .expect("query Calculator Team ID"),
        None
    );
    let requirement = app_requirement("ABCDE12345").expect("valid Team ID");
    assert!(signing
        .check(
            Path::new("/System/Applications/Calculator.app"),
            Some(&requirement)
        )
        .is_err());
}

#[test]
fn adhoc_app_has_no_team_and_detects_tamper() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), "Adhoc.app");
    adhoc_sign(&app);
    let signing = SecCodeSigning;
    assert_eq!(signing.team_id(&app).expect("query ad-hoc Team ID"), None);
    signing
        .check(&app, None)
        .expect("ad-hoc app should pass integrity check");
    OpenOptions::new()
        .append(true)
        .open(app.join("Contents/MacOS/rs-host-agent"))
        .expect("open signed agent")
        .write_all(b"x")
        .expect("tamper with signed agent");
    assert!(signing.check(&app, None).is_err());
}

#[test]
fn adhoc_app_rejects_added_resource_after_signing() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), "AdhocResource.app");
    adhoc_sign(&app);
    let resources = app.join("Contents/Resources");
    fs::create_dir_all(&resources).expect("create resources directory after signing");
    fs::write(resources.join("added-after-signing.txt"), b"tampered")
        .expect("add resource after signing");
    assert!(SecCodeSigning.check(&app, None).is_err());
}

#[test]
fn unsigned_app() {
    let temp = TempDir::new();
    let app = unsigned_fake_app(temp.path(), "Unsigned.app");
    let signing = SecCodeSigning;
    assert_eq!(signing.team_id(&app).expect("query unsigned Team ID"), None);
    assert!(signing.check(&app, Some("anchor apple generic")).is_err());
}

#[test]
fn current_team_of_test_binary() {
    assert_eq!(
        SecCodeSigning
            .current_team_id()
            .expect("query current Team ID"),
        None
    );
}

#[test]
fn verifier_refuses_without_current_team() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME);
    let zip = temp.path().join("update.zip");
    zip_keep_parent(&app, &zip);
    let (signing, requirements) = FakeSigning::new(Some("ABCDE12345"), Ok(()));
    let verifier = MacCodeVerifier::with_signing(None, signing);
    let error = verifier
        .verify_platform(&zip)
        .expect_err("unsigned current app accepted");
    assert!(error.contains("refusing update"));
    assert_no_verify_directories(temp.path());
    assert!(requirements.lock().expect("lock requirements").is_empty());
}

#[test]
fn verifier_accepts_matching_team() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME);
    let zip = temp.path().join("update.zip");
    zip_keep_parent(&app, &zip);
    let (signing, requirements) = FakeSigning::new(Some("ABCDE12345"), Ok(()));
    let verifier = MacCodeVerifier::with_signing(Some("ABCDE12345".to_owned()), signing);
    verifier
        .verify_platform(&zip)
        .expect("matching fake signature should pass");
    assert_eq!(
        *requirements.lock().expect("lock requirements"),
        vec![Some(app_requirement("ABCDE12345").expect("valid Team ID"))]
    );
    assert_no_verify_directories(temp.path());
}

#[test]
fn verifier_rejects_mismatch_and_check_failure() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME);
    let zip = temp.path().join("update.zip");
    zip_keep_parent(&app, &zip);

    let (signing, _) = FakeSigning::new(Some("ZZZZZ99999"), Ok(()));
    let verifier = MacCodeVerifier::with_signing(Some("ABCDE12345".to_owned()), signing);
    let error = verifier
        .verify_platform(&zip)
        .expect_err("mismatched Team ID accepted");
    assert!(error.contains("mismatch"));
    assert_no_verify_directories(temp.path());

    let (signing, _) = FakeSigning::new(Some("ABCDE12345"), Err("bad".to_owned()));
    let verifier = MacCodeVerifier::with_signing(Some("ABCDE12345".to_owned()), signing);
    let error = verifier
        .verify_platform(&zip)
        .expect_err("failed code-signing check accepted");
    assert!(error.contains("bad"));
    assert_no_verify_directories(temp.path());
}

#[test]
fn verifier_rejects_real_adhoc_package() {
    let temp = TempDir::new();
    let app = fake_app(temp.path(), APP_BUNDLE_NAME);
    adhoc_sign(&app);
    let zip = temp.path().join("adhoc.zip");
    zip_keep_parent(&app, &zip);
    let verifier = MacCodeVerifier::with_signing(Some("ABCDE12345".to_owned()), SecCodeSigning);
    let error = verifier
        .verify_platform(&zip)
        .expect_err("ad-hoc package accepted");
    assert!(error.contains("not signed with a Team ID"));
    assert_no_verify_directories(temp.path());
}
