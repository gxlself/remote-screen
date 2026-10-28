mod support;

use std::fs::{self, File};
use std::path::Path;
use std::sync::Arc;
use std::time::{Duration, SystemTime};

use rs_mobile::rscore::crash::{
    crash_dir, crash_record_js, incident_from_name, is_report_name, list_reports, prune, record_js,
    sanitize, write_report, CrashKind, CrashReport, JS_INPUT_MAX,
};
use rs_mobile::rscore::runtime::take_core;
use serde_json::{json, Value};

use support::{temp_dir, test_core, test_core_with};

fn report(incident_at: u64, kind: CrashKind) -> CrashReport {
    CrashReport {
        v: 1,
        kind,
        incident_at,
        app_version: "test".to_owned(),
        platform: rs_mobile::rscore::dto_core::DevicePlatform::Ios,
        os_version: Some("17.4".to_owned()),
        thread: None,
        location: None,
        name: Some("Test".to_owned()),
        message: "message".to_owned(),
        stack: None,
    }
}

fn valid_js(message: &str) -> String {
    json!({"message": message, "isFatal": true}).to_string()
}

#[test]
fn sanitize_table() {
    let cases = [
        ("plain text", "plain text", 4096),
        ("", "", 4096),
        (
            "see https://hs.example.com/a?token=x and HTTP://10.30.1.2:8080/b.",
            "see <url> and <url>",
            4096,
        ),
        (
            "at f (http://localhost:8081/index.bundle:1:2)",
            "at f (<url>)",
            4096,
        ),
        ("/Users/alice/src/main.rs:10:5", "~/src/main.rs:10:5", 4096),
        (
            "/home/bob/.cargo/x.rs and /Users/",
            "~/.cargo/x.rs and /Users/",
            4096,
        ),
        (
            "Err value: Parse(\"my secret clipboard\")",
            "Err value: Parse(\"<redacted>\")",
            4096,
        ),
        ("key='hunter2', other=1", "key='<redacted>', other=1", 4096),
        ("don't stop, it's fine", "don't stop, it's fine", 4096),
        ("a \"b \\\"c\\\" d\" e", "a \"<redacted>\" e", 4096),
        (
            "say \"unterminated\nnext \"x\"",
            "say \"<redacted>\nnext \"<redacted>\"",
            4096,
        ),
        (
            "connect 192.168.31.20:443 failed",
            "connect 192.168.•.•:443 failed",
            4096,
        ),
        (
            "peer fd7a:115c:a1e0::1 down",
            "peer fd7a:115c:•:•:•:•:•:• down",
            4096,
        ),
        ("bell\u{7}x\r\nnext\tcol", "bellx\nnext\tcol", 4096),
        ("éééééééééé", "éé…", 5),
        ("abcdef", "abcd…", 4),
    ];
    for (input, expected, max) in cases {
        assert_eq!(sanitize(input, max), expected, "input={input:?}");
    }
}

#[test]
fn sanitize_never_panics_and_respects_limit() {
    let mut seed = 0x1234_5678_u32;
    for _ in 0..1000 {
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let length = (seed as usize) % 513;
        let mut bytes = Vec::with_capacity(length);
        for _ in 0..length {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            bytes.push((seed >> 24) as u8);
        }
        let input = String::from_utf8_lossy(&bytes);
        seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
        let max = (seed as usize) % 65;
        let output = sanitize(&input, max);
        assert!(output.len() <= max + 3);
    }
}

#[test]
fn js_report_round_trip() {
    let core = test_core_with(|_, options| {
        options.clock = Some(Arc::new(|| 1_792_000_000_000_000));
    });
    let input = r#"{"name":"TypeError","message":"Cannot read \"secret\" of undefined","stack":"at f (/Users/alice/app.js:1:2)\nat g","isFatal":true,"osVersion":"17.4"}"#;
    assert!(record_js(&core, input));
    let reports = list_reports(&crash_dir(&core));
    assert_eq!(reports.len(), 1);
    let name = reports[0].file_name().unwrap().to_string_lossy();
    assert!(name.starts_with("00001792000000000000-js-"));
    assert!(is_report_name(&name));
    let actual: Value = serde_json::from_slice(&fs::read(&reports[0]).unwrap()).unwrap();
    let expected = json!({
        "v": 1,
        "kind": "jsFatal",
        "incidentAt": 1_792_000_000_000_000_u64,
        "appVersion": "test",
        "platform": "ios",
        "osVersion": "17.4",
        "thread": null,
        "location": null,
        "name": "TypeError",
        "message": "Cannot read \"<redacted>\" of undefined",
        "stack": "at f (~/app.js:1:2)\nat g",
    });
    assert_eq!(actual, expected);
    assert!(!fs::read_dir(crash_dir(&core)).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".tmp-")));
}

#[test]
fn js_report_rejections_do_not_consume_quota() {
    let core = test_core();
    let oversized = json!({
        "message": "x".repeat(JS_INPUT_MAX + 1),
        "isFatal": true,
    })
    .to_string();
    for input in [
        r#"{"message":"x"}"#,
        r#"{"message":"x","isFatal":"true"}"#,
        r#"{"isFatal":true}"#,
        r#"{"message":42,"isFatal":true}"#,
        r#"{"message":"x","isFatal":true,"stack":7}"#,
        "not json",
        "[1,2]",
    ] {
        assert!(!record_js(&core, input));
    }
    assert!(!record_js(&core, &oversized));
    assert!(list_reports(&crash_dir(&core)).is_empty());
    for _ in 0..3 {
        assert!(record_js(&core, &valid_js("x")));
    }
}

#[test]
fn js_reports_are_capped_per_core() {
    let core = test_core();
    let input = valid_js("x");
    assert_eq!(
        [
            record_js(&core, &input),
            record_js(&core, &input),
            record_js(&core, &input),
            record_js(&core, &input),
        ],
        [true, true, true, false]
    );
    assert_eq!(list_reports(&crash_dir(&core)).len(), 3);
    assert!(record_js(&test_core(), &input));
}

#[test]
fn long_fields_are_truncated() {
    let core = test_core();
    let input = json!({
        "name": "n".repeat(300),
        "message": "a".repeat(5000),
        "stack": "s".repeat(20_000),
        "isFatal": true,
        "osVersion": "9".repeat(100),
    })
    .to_string();
    assert!(record_js(&core, &input));
    let report_path = list_reports(&crash_dir(&core)).into_iter().next().unwrap();
    let value: Value = serde_json::from_slice(&fs::read(report_path).unwrap()).unwrap();
    assert_eq!(value["name"], format!("{}…", "n".repeat(128)));
    assert_eq!(value["message"], format!("{}…", "a".repeat(1024)));
    assert_eq!(value["stack"], format!("{}…", "s".repeat(16_384)));
    assert_eq!(value["osVersion"], format!("{}…", "9".repeat(64)));

    let empty_name = json!({"name":"", "message":"x", "isFatal":true}).to_string();
    assert!(record_js(&core, &empty_name));
    let reports = list_reports(&crash_dir(&core));
    let empty_report = reports
        .iter()
        .find(|path| {
            serde_json::from_slice::<Value>(&fs::read(path).unwrap())
                .unwrap()
                .get("message")
                == Some(&Value::String("x".to_owned()))
        })
        .unwrap();
    let value: Value = serde_json::from_slice(&fs::read(empty_report).unwrap()).unwrap();
    assert_eq!(value["name"], Value::Null);
}

#[test]
fn write_report_keeps_ten_newest() {
    let dir = temp_dir("crash-keep");
    for incident in 1..=12 {
        let kind = if incident % 2 == 0 {
            CrashKind::JsFatal
        } else {
            CrashKind::RustPanic
        };
        write_report(&dir, &report(incident, kind)).unwrap();
    }
    let reports = list_reports(&dir);
    assert_eq!(reports.len(), 10);
    let incidents: Vec<_> = reports
        .iter()
        .map(|path| incident_from_name(&path.file_name().unwrap().to_string_lossy()).unwrap())
        .collect();
    assert_eq!(incidents, (3..=12).collect::<Vec<_>>());
    assert!(!fs::read_dir(dir).unwrap().any(|entry| entry
        .unwrap()
        .file_name()
        .to_string_lossy()
        .starts_with(".tmp-")));
}

#[test]
fn write_report_rejects_oversized() {
    let dir = temp_dir("crash-oversized");
    let mut oversized = report(1, CrashKind::RustPanic);
    oversized.message = "x".repeat(70_000);
    let error = write_report(&dir, &oversized).unwrap_err();
    assert_eq!(error.kind(), std::io::ErrorKind::InvalidData);
    assert_eq!(fs::read_dir(dir).unwrap().count(), 0);
}

#[test]
fn list_reports_ignores_other_entries() {
    let dir = temp_dir("crash-list");
    fs::write(dir.join("abc.json"), b"").unwrap();
    fs::write(dir.join("00000000000000000001-rust-ABCDEF12.json"), b"").unwrap();
    fs::write(dir.join("00000000000000000001-py-0000abcd.json"), b"").unwrap();
    fs::write(dir.join(".tmp-x"), b"").unwrap();
    fs::create_dir(dir.join("00000000000000000002-js-0000abcd.json")).unwrap();
    fs::write(dir.join("00000000000000000003-js-0000abcd.json"), b"").unwrap();
    assert_eq!(
        list_reports(&dir),
        vec![dir.join("00000000000000000003-js-0000abcd.json")]
    );
    assert!(list_reports(Path::new("/definitely/missing/crash-dir")).is_empty());
}

#[test]
fn prune_removes_old_reports_and_stale_temp_files() {
    let dir = temp_dir("crash-prune");
    let now = 10 * 86_400_000_000_u64;
    write_report(
        &dir,
        &report(now - 8 * 86_400_000_000, CrashKind::RustPanic),
    )
    .unwrap();
    write_report(&dir, &report(now - 6 * 86_400_000_000, CrashKind::JsFatal)).unwrap();
    let stale = dir.join(".tmp-a");
    let fresh = dir.join(".tmp-b");
    File::create(&stale)
        .unwrap()
        .set_modified(
            SystemTime::now()
                .checked_sub(Duration::from_secs(120))
                .unwrap(),
        )
        .unwrap();
    File::create(&fresh).unwrap();
    fs::write(dir.join("notes.txt"), b"keep").unwrap();
    assert_eq!(prune(&dir, now), 2);
    assert_eq!(list_reports(&dir).len(), 1);
    assert!(!stale.exists());
    assert!(fresh.exists());
    assert!(dir.join("notes.txt").exists());
}

#[test]
fn export_without_core_returns_false() {
    let _ = take_core();
    assert!(!crash_record_js(valid_js("x")));
}
