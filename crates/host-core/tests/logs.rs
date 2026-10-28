use host_core::logs::{export, mask_line};
use std::fs;
use std::io::Read;
use std::time::{SystemTime, UNIX_EPOCH};

#[test]
fn masks_addresses_and_identity_fields() {
    let line = r#"{"host":"h7v2","user":"alice","peer":"100.64.12.48:51820","v":"0.3.2","ipv6":"2001:db8:abcd::1"}"#;
    let masked = mask_line(line);
    assert!(masked.contains("\u{2022}\u{2022}\u{2022}"));
    assert!(masked.contains("100.64.•.•"));
    assert!(masked.contains("0.3.2"));
    assert!(!masked.contains("12.48"));
}

#[test]
fn export_writes_masked_zip() {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    let root = std::env::temp_dir().join(format!("rs-log-export-{stamp}"));
    fs::create_dir_all(&root).expect("directory");
    fs::write(
        root.join("host.jsonl"),
        r#"{"peer":"10.30.12.48","host":"secret"}
"#,
    )
    .expect("log");
    let output = root.join("logs.zip");
    export(&root, &output, true).expect("export");
    let file = fs::File::open(&output).expect("archive");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    let mut entry = archive.by_name("host.jsonl").expect("entry");
    let mut text = String::new();
    entry.read_to_string(&mut text).expect("read entry");
    assert!(text.contains("10.30.•.•"));
    assert!(text.contains("•••"));
    let _ = fs::remove_dir_all(root);
}
