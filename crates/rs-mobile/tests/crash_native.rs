mod support;

use std::fs::{self, File};
use std::path::{Path, PathBuf};
use std::sync::Arc;
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rs_mobile::rscore::crash::{
    crash_dir, incident_from_name, is_report_name, list_reports, record_js, report_file_name,
    CrashKind, MESSAGE_MAX, NAME_MAX, OS_VERSION_MAX, REPORT_FILE_MAX, STACK_MAX,
};
use rs_mobile::rscore::crash_native::{
    inbox_dir, ingest_at_start, ingest_native, native_report, IngestStats, APP_VERSION_MAX,
    NATIVE_INPUT_MAX,
};
use rs_mobile::rscore::dto_core::{CoreConfig, DevicePlatform};
use serde_json::{json, Value};
use support::{temp_dir, test_core_with};

const T: u64 = 1_792_000_000_000_000;

fn cfg() -> CoreConfig {
    CoreConfig {
        files_dir: None,
        data_dir: "/tmp/x".into(),
        app_version: "test".into(),
        platform: DevicePlatform::Android,
    }
}

fn inbox(dir: &Path) -> PathBuf {
    let path = inbox_dir(dir);
    fs::create_dir_all(&path).unwrap();
    path
}

fn put(dir: &Path, name: &str, body: impl AsRef<[u8]>) {
    fs::write(inbox(dir).join(name), body).unwrap();
}

fn valid(incident_at: u64) -> Vec<u8> {
    serde_json::to_vec(&json!({"v": 1, "incidentAt": incident_at, "message": "m"})).unwrap()
}

fn inbox_names(dir: &Path) -> Vec<String> {
    let mut names: Vec<_> = fs::read_dir(inbox_dir(dir))
        .unwrap()
        .map(|entry| entry.unwrap().file_name().into_string().unwrap())
        .collect();
    names.sort();
    names
}

fn incidents(dir: &Path) -> Vec<u64> {
    list_reports(dir)
        .iter()
        .map(|path| incident_from_name(path.file_name().unwrap().to_str().unwrap()).unwrap())
        .collect()
}

#[test]
fn native_report_round_trip() {
    let input = json!({
        "v": 1,
        "incidentAt": T,
        "appVersion": "0.1.0",
        "osVersion": "17.4",
        "name": "EXC_BAD_ACCESS / SIGSEGV",
        "message": "KERN_INVALID_ADDRESS at 0x10",
        "stack": "0 RemoteScreen 0x1049d2f10 6B2E0C1A-1111-2222-3333-444455556666 +49443",
        "thread": null,
    });
    let report = native_report(&serde_json::to_vec(&input).unwrap(), &cfg()).unwrap();
    assert_eq!(
        serde_json::to_value(report).unwrap(),
        json!({
            "v": 1,
            "kind": "nativeCrash",
            "incidentAt": T,
            "appVersion": "0.1.0",
            "platform": "android",
            "osVersion": "17.4",
            "thread": null,
            "location": null,
            "name": "EXC_BAD_ACCESS / SIGSEGV",
            "message": "KERN_INVALID_ADDRESS at 0x10",
            "stack": "0 RemoteScreen 0x1049d2f10 6B2E0C1A-1111-2222-3333-444455556666 +49443",
        })
    );
}

#[test]
fn native_report_minimal_fields() {
    let report = native_report(br#"{"v":1,"incidentAt":5,"message":""}"#, &cfg()).unwrap();
    assert_eq!(report.app_version, "test");
    assert_eq!(report.message, "");
    assert_eq!(report.os_version, None);
    assert_eq!(report.name, None);
    assert_eq!(report.stack, None);
    assert_eq!(report.thread, None);

    let report = native_report(
        br#"{"v":1,"incidentAt":5,"message":"m","appVersion":""}"#,
        &cfg(),
    )
    .unwrap();
    assert_eq!(report.app_version, "test");
}

#[test]
fn native_report_scrubs() {
    let input = json!({
        "v": 1,
        "incidentAt": T,
        "message": "open \"/Users/alice/secret.txt\" failed at https://hs.example.com/x from 192.168.31.20",
        "stack": "at /home/bob/app.so",
    });
    let report = native_report(&serde_json::to_vec(&input).unwrap(), &cfg()).unwrap();
    assert_eq!(
        report.message,
        "open \"<redacted>\" failed at <url> from 192.168.\u{2022}.\u{2022}"
    );
    assert_eq!(report.stack.as_deref(), Some("at ~/app.so"));
}

#[test]
fn native_report_rejects() {
    for input in [
        "not json",
        "[1]",
        r#"{"incidentAt":5,"message":"m"}"#,
        r#"{"v":2,"incidentAt":5,"message":"m"}"#,
        r#"{"v":"1","incidentAt":5,"message":"m"}"#,
        r#"{"v":1,"incidentAt":0,"message":"m"}"#,
        r#"{"v":1,"incidentAt":-5,"message":"m"}"#,
        r#"{"v":1,"incidentAt":5}"#,
        r#"{"v":1,"incidentAt":5,"message":3}"#,
        r#"{"v":1,"incidentAt":5,"message":"m","stack":[]}"#,
        r#"{"v":1,"incidentAt":5,"message":"m","osVersion":17}"#,
        r#"{"v":1.0,"incidentAt":5,"message":"m"}"#,
        r#"{"v":true,"incidentAt":5,"message":"m"}"#,
        r#"{"v":1,"message":"m"}"#,
        r#"{"v":1,"incidentAt":"5","message":"m"}"#,
        r#"{"v":1,"incidentAt":5.5,"message":"m"}"#,
        r#"{"v":1,"incidentAt":5,"message":"m","appVersion":true}"#,
        r#"{"v":1,"incidentAt":5,"message":"m","name":{}}"#,
        r#"{"v":1,"incidentAt":5,"message":"m","thread":0}"#,
    ] {
        assert!(native_report(input.as_bytes(), &cfg()).is_none(), "{input}");
    }
    assert!(native_report(&[0xff], &cfg()).is_none());
}

#[test]
fn native_report_optional_fields_are_sanitized_and_bounded() {
    for empty in [Value::Null, json!(""), json!("\u{7}")] {
        let input = json!({
            "v": 1, "incidentAt": T, "message": "\u{7}",
            "appVersion": empty, "osVersion": empty, "name": empty,
            "thread": empty, "stack": empty, "ignored": [1, 2],
        });
        let report = native_report(&serde_json::to_vec(&input).unwrap(), &cfg()).unwrap();
        assert_eq!(report.app_version, "test");
        assert_eq!(report.message, "");
        assert_eq!(report.os_version, None);
        assert_eq!(report.name, None);
        assert_eq!(report.thread, None);
        assert_eq!(report.stack, None);
    }

    let input = json!({
        "v": 1, "incidentAt": T, "message": "m".repeat(MESSAGE_MAX + 1),
        "appVersion": "a".repeat(APP_VERSION_MAX + 1),
        "osVersion": "o".repeat(OS_VERSION_MAX + 1),
        "name": "n".repeat(NAME_MAX + 1), "thread": "t".repeat(NAME_MAX + 1),
        "stack": "s".repeat(STACK_MAX + 1),
    });
    let report = native_report(&serde_json::to_vec(&input).unwrap(), &cfg()).unwrap();
    assert_eq!(
        report.app_version,
        format!("{}\u{2026}", "a".repeat(APP_VERSION_MAX))
    );
    assert_eq!(
        report.message,
        format!("{}\u{2026}", "m".repeat(MESSAGE_MAX))
    );
    assert_eq!(
        report.os_version,
        Some(format!("{}\u{2026}", "o".repeat(OS_VERSION_MAX)))
    );
    assert_eq!(
        report.name,
        Some(format!("{}\u{2026}", "n".repeat(NAME_MAX)))
    );
    assert_eq!(
        report.thread,
        Some(format!("{}\u{2026}", "t".repeat(NAME_MAX)))
    );
    assert_eq!(
        report.stack,
        Some(format!("{}\u{2026}", "s".repeat(STACK_MAX)))
    );
}

#[test]
fn ingest_moves_valid_and_drops_bad() {
    let dir = temp_dir("u11j-ingest");
    put(&dir, "a.json", valid(T - 2_000_000));
    put(&dir, "b.json", valid(T - 1_000_000));
    put(&dir, "c.json", "not json");
    put(&dir, "d.json", valid(T - 8 * 86_400_000_000));
    let padding = NATIVE_INPUT_MAX as usize + 1 - valid(T).len();
    let oversized = serde_json::to_vec(&json!({
        "v": 1, "incidentAt": T, "message": "m".repeat(padding + 1),
    }))
    .unwrap();
    assert_eq!(oversized.len() as u64, NATIVE_INPUT_MAX + 1);
    put(&dir, "e.json", oversized);
    put(&dir, ".tmp-x", "fresh");
    let old = File::create(inbox(&dir).join(".tmp-old")).unwrap();
    old.set_modified(
        SystemTime::now()
            .checked_sub(Duration::from_secs(120))
            .unwrap(),
    )
    .unwrap();
    put(&dir, "notes.txt", "keep");
    fs::create_dir(inbox(&dir).join("sub.json")).unwrap();

    assert_eq!(
        ingest_native(&cfg(), &dir, T),
        IngestStats {
            ingested: 2,
            dropped: 3
        }
    );
    let reports = list_reports(&dir);
    assert_eq!(reports.len(), 2);
    for report in reports {
        let name = report.file_name().unwrap().to_str().unwrap();
        assert!(name.contains("-native-"));
        assert!(is_report_name(name));
    }
    assert_eq!(incidents(&dir), [T - 2_000_000, T - 1_000_000]);
    assert_eq!(inbox_names(&dir), [".tmp-x", "notes.txt", "sub.json"]);
}

#[test]
fn ingest_limits_batch_and_handles_missing_dir() {
    let dir = temp_dir("u11j-batch");
    assert_eq!(ingest_native(&cfg(), &dir, T), IngestStats::default());
    assert!(!inbox_dir(&dir).exists());

    for i in 0..25 {
        put(
            &dir,
            &format!("n{i:02}.json"),
            valid(T - 100_000_000 + i * 1_000_000),
        );
    }
    assert_eq!(
        ingest_native(&cfg(), &dir, T),
        IngestStats {
            ingested: 20,
            dropped: 0
        }
    );
    assert_eq!(
        inbox_names(&dir),
        (20..25)
            .map(|i| format!("n{i:02}.json"))
            .collect::<Vec<_>>()
    );
    assert_eq!(
        incidents(&dir),
        (10..20)
            .map(|i| T - 100_000_000 + i * 1_000_000)
            .collect::<Vec<_>>()
    );

    assert_eq!(
        ingest_native(&cfg(), &dir, T),
        IngestStats {
            ingested: 5,
            dropped: 0
        }
    );
    assert!(inbox_names(&dir).is_empty());
    assert_eq!(
        incidents(&dir),
        (15..25)
            .map(|i| T - 100_000_000 + i * 1_000_000)
            .collect::<Vec<_>>()
    );
}

#[test]
fn ingest_keeps_failed_writes_and_continues() {
    let dir = temp_dir("u11j-retry");
    let mut config = cfg();
    config.app_version = "v".repeat(REPORT_FILE_MAX as usize + 1);
    put(&dir, "a.json", valid(T - 2));
    put(
        &dir,
        "b.json",
        serde_json::to_vec(&json!({
            "v": 1, "incidentAt": T - 1, "message": "m", "appVersion": "0.1.0",
        }))
        .unwrap(),
    );

    assert_eq!(
        ingest_native(&config, &dir, T),
        IngestStats {
            ingested: 1,
            dropped: 0
        }
    );
    assert_eq!(inbox_names(&dir), ["a.json"]);
    assert_eq!(incidents(&dir), [T - 1]);
    assert_eq!(
        ingest_native(&cfg(), &dir, T),
        IngestStats {
            ingested: 1,
            dropped: 0
        }
    );
    assert!(inbox_names(&dir).is_empty());
    assert_eq!(incidents(&dir), [T - 2, T - 1]);
}

#[cfg(unix)]
#[test]
fn ingest_ignores_symlinks_and_accepts_non_utf8_names() {
    use std::ffi::OsString;
    use std::os::unix::ffi::OsStringExt;
    use std::os::unix::fs::symlink;

    let dir = temp_dir("u11j-file-types");
    let path = inbox(&dir);
    let external = dir.join("external.json");
    fs::write(&external, valid(T)).unwrap();
    symlink(&external, path.join("link.json")).unwrap();
    symlink(&external, path.join(".old-link")).unwrap();
    fs::create_dir(path.join(".old-dir")).unwrap();
    fs::write(
        path.join(OsString::from_vec(b"\xff.json".to_vec())),
        valid(T + 1),
    )
    .unwrap();

    assert_eq!(
        ingest_native(&cfg(), &dir, T),
        IngestStats {
            ingested: 1,
            dropped: 0
        }
    );
    assert_eq!(incidents(&dir), [T + 1]);
    assert!(external.exists());
    assert!(path
        .join("link.json")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(path
        .join(".old-link")
        .symlink_metadata()
        .unwrap()
        .file_type()
        .is_symlink());
    assert!(path.join(".old-dir").is_dir());
}

#[test]
fn js_error_has_its_own_quota() {
    let core = test_core_with(|_, options| options.clock = Some(Arc::new(|| T)));
    let input = r#"{"name":null,"message":"x","stack":null,"isFatal":false,"osVersion":null}"#;
    assert_eq!(
        [
            record_js(&core, input),
            record_js(&core, input),
            record_js(&core, input),
            record_js(&core, input)
        ],
        [true, true, true, false]
    );
    assert!(record_js(&core, &input.replace("false", "true")));

    let reports = list_reports(&crash_dir(&core));
    assert_eq!(reports.len(), 4);
    let mut errors = 0;
    let mut fatals = 0;
    for path in reports {
        let name = path.file_name().unwrap().to_str().unwrap();
        if name.contains("-jserr-") {
            errors += 1;
            let report: Value = serde_json::from_slice(&fs::read(path).unwrap()).unwrap();
            assert_eq!(report["kind"], "jsError");
        } else if name.contains("-js-") {
            fatals += 1;
        }
    }
    assert_eq!((errors, fatals), (3, 1));
}

#[test]
fn report_names_accept_new_tags() {
    for name in [
        "00000000000000000001-native-0000abcd.json",
        "00000000000000000001-jserr-0000abcd.json",
        "00000000000000000001-rust-0000abcd.json",
        "00000000000000000001-js-0000abcd.json",
    ] {
        assert!(is_report_name(name), "{name}");
    }
    for name in [
        "00000000000000000001-nat-0000abcd.json",
        "00000000000000000001-JSERR-0000abcd.json",
        "00000000000000000001-native-0000abcd.json.bak",
        "00000000000000000001-native-0000abc.json",
        "00000000000000000001-native-0000ABCd.json",
        "0000000000000000001-native-0000abcd.json",
        "000000000000000000001-native-0000abcd.json",
        "x0000000000000000001-native-0000abcd.json",
        "00000000000000000001native-0000abcd.json",
        "00000000000000000001-native--0000abcd.json",
        "00000000000000000001-native-0000abcd.json\n",
        "",
        "native",
        "\u{2022}",
    ] {
        assert!(!is_report_name(name), "{name}");
    }
    let mut report = native_report(&valid(T), &cfg()).unwrap();
    for (kind, tag) in [
        (CrashKind::JsError, "-jserr-"),
        (CrashKind::NativeCrash, "-native-"),
    ] {
        report.kind = kind;
        let name = report_file_name(&report);
        assert!(name.contains(tag));
        assert!(is_report_name(&name));
    }
}

#[test]
fn ingest_at_start_uses_wall_clock() {
    let dir = temp_dir("u11j-start");
    let now_us = u64::try_from(
        SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .unwrap()
            .as_micros(),
    )
    .unwrap();
    put(&dir, "recent.json", valid(now_us - 1_000_000));

    ingest_at_start(&dir, &cfg());
    let reports = list_reports(&dir);
    assert_eq!(reports.len(), 1);
    assert!(reports[0]
        .file_name()
        .unwrap()
        .to_str()
        .unwrap()
        .contains("-native-"));
    assert!(inbox_names(&dir).is_empty());
}
