use std::fs;
use std::path::PathBuf;
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;

use host_core::audit::{AuditEvent, AuditQueue};
use host_core::config::{load, ConfigStore, HostConfig};
use host_core::ipc::{AuditListParams, LogsExportParams, UpdateChannel, UpdateSetChannelParams};
use host_core::local_api::{locked_change, logs_zip_name, LocalApi};
use serde_json::json;

static NEXT: AtomicU64 = AtomicU64::new(0);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let root = PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../../target/rs-h23a-tests");
        let path = root.join(format!(
            "local-api-{}-{}",
            std::process::id(),
            NEXT.fetch_add(1, Ordering::Relaxed)
        ));
        let _ = fs::remove_dir_all(&path);
        fs::create_dir_all(&path).expect("test directory is created");
        Self(path)
    }

    fn path(&self) -> &std::path::Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

fn config(dir: &TestDir) -> ConfigStore {
    ConfigStore::new(dir.path().join("config.toml"), HostConfig::default())
}

fn event(ts: u64) -> AuditEvent {
    AuditEvent {
        ts,
        user: Some("alice".to_owned()),
        device_id: None,
        host_id: None,
        server: None,
        event: "session_start".to_owned(),
        path: None,
        duration_s: None,
        detail: None,
    }
}

#[tokio::test]
async fn settings_update_and_locked_fields() {
    let dir = TestDir::new();
    let store = config(&dir);
    let api = LocalApi::new(store.clone()).with_locked_settings(&["files.dest_dir"]);
    assert_eq!(
        api.settings_get().await.expect("settings"),
        serde_json::to_value(HostConfig::default()).expect("json")
    );
    let result = api
        .settings_set(json!({"access": {"allow_remote_control": false}}))
        .await
        .expect("settings update");
    assert_eq!(result["access"]["allow_remote_control"], false);
    assert!(
        !load(dir.path().join("config.toml"))
            .expect("config reload")
            .access
            .allow_remote_control
    );

    let error = api
        .settings_set(json!({"files": {"dest_dir": "/tmp/locked"}}))
        .await
        .expect_err("locked field is rejected");
    assert_eq!(error.code, "forbidden");
    assert_eq!(store.get().files.dest_dir, "");
    api.settings_set(json!({"files": {"dest_dir": ""}, "access": {"max_viewers": 2}}))
        .await
        .expect("unchanged locked value is accepted");

    let error = api
        .settings_set(json!({"access": {"max_viewers": 9}}))
        .await
        .expect_err("invalid value is rejected");
    assert_eq!(error.code, "invalid_params");
    assert!(error.message.contains("access.max_viewers"));
}

#[tokio::test]
async fn update_channel_and_audit_pages() {
    let dir = TestDir::new();
    let audit = AuditQueue::open(dir.path()).expect("audit opens");
    let store = config(&dir);
    let api = LocalApi::new(store.clone()).with_audit(audit.clone());
    api.update_set_channel(UpdateSetChannelParams {
        channel: UpdateChannel::Beta,
    })
    .await
    .expect("channel update");
    assert_eq!(
        store.get().update.channel,
        host_core::config::UpdateChannel::Beta
    );
    for ts in 0..60 {
        audit.push(event(ts)).expect("audit push");
    }
    assert_eq!(
        api.audit_list(AuditListParams::default())
            .await
            .expect("page")
            .entries
            .len(),
        50
    );
    assert_eq!(
        api.audit_list(AuditListParams {
            limit: Some(500),
            before_us: None
        })
        .await
        .expect("page")
        .entries
        .len(),
        60
    );
    assert_eq!(
        api.audit_list(AuditListParams {
            limit: Some(0),
            before_us: None
        })
        .await
        .expect("page")
        .entries
        .len(),
        1
    );
    let first = api
        .audit_list(AuditListParams::default())
        .await
        .expect("first page");
    let before = first
        .entries
        .last()
        .and_then(|entry| entry["ts"].as_u64())
        .expect("timestamp");
    let second = api
        .audit_list(AuditListParams {
            limit: Some(50),
            before_us: Some(before),
        })
        .await
        .expect("second page");
    assert!(second
        .entries
        .iter()
        .all(|entry| entry["ts"].as_u64().is_some_and(|ts| ts < before)));
}

#[tokio::test]
async fn logs_export_is_optional_unique_and_masked() {
    let dir = TestDir::new();
    let store = config(&dir);
    let no_logs = LocalApi::new(store.clone());
    assert_eq!(
        no_logs
            .logs_export(LogsExportParams { mask: false })
            .await
            .expect_err("logs are not wired")
            .code,
        "unsupported"
    );

    let logs_dir = dir.path().join("logs");
    let export_dir = dir.path().join("downloads");
    fs::create_dir_all(&logs_dir).expect("logs directory");
    fs::write(logs_dir.join("agent.log"), "peer=192.168.31.7\n").expect("log file");
    let export_for_closure = export_dir.clone();
    let api = LocalApi::new(store)
        .with_logs(logs_dir, Arc::new(move || Some(export_for_closure.clone())))
        .with_forced_mask();
    let first = api
        .logs_export(LogsExportParams { mask: false })
        .await
        .expect("first export");
    let second = api
        .logs_export(LogsExportParams { mask: false })
        .await
        .expect("second export");
    let first_path = PathBuf::from(&first.path);
    let second_path = PathBuf::from(&second.path);
    assert!(first_path.exists());
    assert!(second_path.exists());
    assert!(first_path
        .file_name()
        .and_then(|name| name.to_str())
        .is_some_and(|name| name.ends_with(".zip")));
    assert_ne!(first_path, second_path);

    let file = fs::File::open(first_path).expect("archive");
    let mut archive = zip::ZipArchive::new(file).expect("zip");
    let mut content = String::new();
    std::io::Read::read_to_string(
        &mut archive.by_index(0).expect("archive entry"),
        &mut content,
    )
    .expect("log content");
    assert!(content.contains("192.168.•.•"));
}

#[test]
fn timestamp_and_locked_change_helpers() {
    assert_eq!(logs_zip_name(0), "RemoteHost-logs-19700101-000000.zip");
    assert_eq!(
        logs_zip_name(1_792_728_909_123_456),
        "RemoteHost-logs-20261023-041509.zip"
    );
    let current = json!({"a": {"b": {"c": 1}, "sibling": true}, "array": [1]});
    let locked = vec!["a.b.c".to_owned(), "array".to_owned()];
    assert_eq!(
        locked_change(&current, &json!({"a": {"b": {"c": 2}}}), &locked),
        Some("a.b.c".to_owned())
    );
    assert_eq!(locked_change(&current, &json!({"a": "x"}), &locked), None);
    assert_eq!(
        locked_change(&current, &json!({"a": {"sibling": false}}), &locked),
        None
    );
    assert_eq!(
        locked_change(&current, &json!({"array": [2]}), &locked),
        Some("array".to_owned())
    );
}
