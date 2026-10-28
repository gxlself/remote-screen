use std::fs;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::time::Duration;

use rs_mobile::rscore::crash::list_reports;
use rs_mobile::rscore::crash_hook::install_panic_hook;
use rs_mobile::rscore::dto_core::{CoreConfig, DevicePlatform};
use serde_json::Value;

#[test]
fn panic_hook_records_chains_and_caps() {
    static PREV: AtomicUsize = AtomicUsize::new(0);

    let root = std::env::temp_dir().join(format!("rs-crash-hook-{}", std::process::id()));
    let dir = root.join("crashes");
    let other = root.join("other");
    let _ = fs::remove_dir_all(&root);
    let cfg = CoreConfig {
        files_dir: None,
        data_dir: root.to_string_lossy().into_owned(),
        app_version: "9.9.9".into(),
        platform: DevicePlatform::Android,
    };

    std::panic::set_hook(Box::new(|_| {
        PREV.fetch_add(1, Ordering::SeqCst);
    }));
    install_panic_hook(dir.clone(), &cfg);
    install_panic_hook(other.clone(), &cfg);

    let joined = std::thread::Builder::new()
        .name("crash-test".into())
        .spawn(|| panic!("boom at /Users/alice/x.rs with \"secret\""))
        .expect("spawn crash test thread")
        .join();
    assert!(joined.is_err());
    assert_eq!(PREV.load(Ordering::SeqCst), 1);

    let reports = list_reports(&dir);
    assert_eq!(reports.len(), 1);
    assert!(reports[0]
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.contains("-rust-")));
    let first: Value = serde_json::from_slice(&fs::read(&reports[0]).expect("read first report"))
        .expect("parse first report");
    assert_eq!(first["v"], 1);
    assert_eq!(first["kind"], "rustPanic");
    assert_eq!(first["appVersion"], "9.9.9");
    assert_eq!(first["platform"], "android");
    assert_eq!(first["osVersion"], Value::Null);
    assert_eq!(first["name"], Value::Null);
    assert_eq!(first["thread"], "crash-test");
    assert_eq!(first["message"], "boom at ~/x.rs with \"<redacted>\"");
    assert!(first["location"]
        .as_str()
        .is_some_and(|location| location.contains("crash_hook.rs:")));
    assert!(first["stack"]
        .as_str()
        .is_some_and(|stack| !stack.is_empty()));
    assert!(!other.exists());
    assert!(!fs::read_dir(&dir)
        .expect("read crash directory")
        .any(|entry| entry
            .expect("read crash directory entry")
            .file_name()
            .to_string_lossy()
            .starts_with(".tmp-")));

    std::thread::sleep(Duration::from_millis(2));
    let caught = std::panic::catch_unwind(|| std::panic::panic_any(42_u32));
    assert!(caught.is_err());
    assert_eq!(PREV.load(Ordering::SeqCst), 2);
    let reports = list_reports(&dir);
    assert_eq!(reports.len(), 2);
    let second: Value = serde_json::from_slice(
        &fs::read(reports.last().expect("second report")).expect("read second report"),
    )
    .expect("parse second report");
    assert_eq!(second["message"], "<non-string panic payload>");

    fs::remove_dir_all(&dir).expect("remove crash directory");
    fs::write(&dir, b"x").expect("replace crash directory with file");
    let caught = std::panic::catch_unwind(|| panic!("write failure"));
    assert!(caught.is_err());
    assert_eq!(PREV.load(Ordering::SeqCst), 3);
    fs::remove_file(&dir).expect("remove crash path file");

    for message in ["fourth", "fifth", "sixth"] {
        std::thread::sleep(Duration::from_millis(2));
        let caught = std::panic::catch_unwind(|| panic!("{message}"));
        assert!(caught.is_err());
    }
    assert_eq!(PREV.load(Ordering::SeqCst), 6);
    let reports = list_reports(&dir);
    assert_eq!(reports.len(), 2);
    let messages: Vec<_> = reports
        .iter()
        .map(|path| {
            let report: Value =
                serde_json::from_slice(&fs::read(path).expect("read capped report"))
                    .expect("parse capped report");
            report["message"]
                .as_str()
                .expect("report message")
                .to_owned()
        })
        .collect();
    assert_eq!(messages, ["fourth", "fifth"]);

    fs::remove_dir_all(root).expect("clean crash hook test directory");
}
