#![cfg(target_os = "macos")]

use std::fs;
use std::io::ErrorKind;
use std::path::PathBuf;
use std::process::Command;
use std::time::{SystemTime, UNIX_EPOCH};

use host_mac::files::{quarantine, quarantine_value};

fn temp_path(label: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock is after epoch")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "remote-host-{label}-{}-{stamp}",
        std::process::id()
    ))
}

#[test]
fn quarantine_value_uses_mac_download_format() {
    assert_eq!(
        quarantine_value("Remote Host", 0x65000000),
        "0081;65000000;Remote Host;"
    );
}

#[test]
fn quarantine_sets_attribute_on_file() {
    let path = temp_path("quarantine");
    fs::write(&path, b"payload").expect("create temporary file");

    quarantine(&path, "Remote Host", 0x65000000).expect("set quarantine attribute");
    let output = Command::new("/usr/bin/xattr")
        .args([
            "-p",
            "com.apple.quarantine",
            path.to_str().expect("temporary path is UTF-8"),
        ])
        .output()
        .expect("run xattr");
    assert!(
        output.status.success(),
        "xattr failed: {}",
        String::from_utf8_lossy(&output.stderr)
    );
    let value = String::from_utf8(output.stdout).expect("xattr output is UTF-8");
    assert!(
        value.starts_with("0081;"),
        "unexpected quarantine value: {value:?}"
    );
    assert!(
        value.contains("Remote Host"),
        "unexpected quarantine value: {value:?}"
    );

    fs::remove_file(path).expect("remove temporary file");
}

#[test]
fn quarantine_reports_missing_path() {
    let path = temp_path("missing");
    let error = quarantine(&path, "Remote Host", 0x65000000).expect_err("missing path fails");
    assert_eq!(error.kind(), ErrorKind::NotFound);
}
