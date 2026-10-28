use host_core::config::{apply_patch, load, save, ConfigError, ConfigStore, HostConfig};
use serde_json::json;
use std::fs;
use std::io::{self, Write};
use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::{SystemTime, UNIX_EPOCH};

fn temp_path(name: &str) -> PathBuf {
    let stamp = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("clock")
        .as_nanos();
    std::env::temp_dir().join(format!(
        "rs-host-core-{name}-{}-{stamp}",
        std::process::id()
    ))
}

#[test]
fn defaults_load_and_round_trip() {
    let path = temp_path("config").join("config.toml");
    let config = load(&path).expect("load defaults");
    assert_eq!(config, HostConfig::default());
    assert_eq!(load(&path).expect("load again"), config);
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[derive(Clone)]
struct LogWriter(Arc<Mutex<Vec<u8>>>);

impl Write for LogWriter {
    fn write(&mut self, bytes: &[u8]) -> io::Result<usize> {
        self.0.lock().unwrap().extend_from_slice(bytes);
        Ok(bytes.len())
    }

    fn flush(&mut self) -> io::Result<()> {
        Ok(())
    }
}

impl<'a> tracing_subscriber::fmt::MakeWriter<'a> for LogWriter {
    type Writer = Self;

    fn make_writer(&'a self) -> Self::Writer {
        self.clone()
    }
}

#[test]
fn legacy_legacy_directory_section_is_ignored_with_a_warning() {
    let path = temp_path("legacy-legacy_directory").join("config.toml");
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    let contents = "# Existing installation\n[legacy_directory]\nurl = \"http://studio-mac:7300\"\nthumbnail = false\ncrash_reports = true\n\n[access]\nmax_viewers = 2\n";
    fs::write(&path, contents).expect("write old config");
    let logs = Arc::new(Mutex::new(Vec::new()));
    let subscriber = tracing_subscriber::fmt()
        .with_ansi(false)
        .without_time()
        .with_writer(LogWriter(logs.clone()))
        .finish();
    let config =
        tracing::subscriber::with_default(subscriber, || load(&path)).expect("load old config");

    let mut expected = HostConfig::default();
    expected.access.max_viewers = 2;
    assert_eq!(config, expected);
    let logs = String::from_utf8(logs.lock().unwrap().clone()).expect("utf-8 logs");
    assert!(
        logs.contains("WARN") && logs.contains("unknown configuration field ignored"),
        "{logs}"
    );
    assert!(logs.contains("field=legacy_directory"), "{logs}");
    assert_eq!(fs::read_to_string(&path).expect("read config"), contents);

    save(&path, &config).expect("save");
    let saved = fs::read_to_string(&path).expect("read saved config");
    assert!(!saved.contains("legacy_directory"), "{saved}");
    assert_eq!(load(&path).expect("reload"), expected);
    fs::remove_dir_all(path.parent().expect("parent")).expect("cleanup");
}

#[test]
fn unknown_and_missing_fields_are_compatible() {
    let path = temp_path("unknown").join("config.toml");
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(&path, "[access]\nmax_viewers = 2\nfuture = true\n").expect("write");
    let config = load(&path).expect("load with unknown field");
    assert_eq!(config.access.max_viewers, 2);
    assert!(config.access.pause_on_local_input);
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[test]
fn patch_merges_and_reports_types() {
    let config = HostConfig::default();
    let updated = apply_patch(&config, &json!({"video": {"max_fps": 30}})).expect("patch");
    assert_eq!(updated.video.max_fps, 30);
    assert_eq!(updated.access.max_viewers, config.access.max_viewers);
    let error =
        apply_patch(&config, &json!({"access": {"max_viewers": "three"}})).expect_err("type error");
    assert!(error.to_string().contains("access.max_viewers"));
}

#[test]
fn security_defaults_to_confirming_new_devices() {
    assert!(HostConfig::default().security.confirm_new_devices);
    let path = temp_path("security").join("config.toml");
    fs::create_dir_all(path.parent().expect("parent")).expect("directory");
    fs::write(
        &path,
        "[legacy_directory]\nurl = \"http://hs.example.com\"\n",
    )
    .expect("write");
    assert!(
        load(&path)
            .expect("legacy configuration")
            .security
            .confirm_new_devices
    );
    fs::remove_dir_all(path.parent().expect("parent")).expect("cleanup");
}

#[test]
fn security_patch_checks_boolean_type() {
    let config = HostConfig::default();
    let updated = apply_patch(
        &config,
        &json!({"security": {"confirm_new_devices": false}}),
    )
    .expect("security patch");
    assert!(!updated.security.confirm_new_devices);
    let error = apply_patch(&config, &json!({"security": {"confirm_new_devices": "no"}}))
        .expect_err("boolean type error");
    assert!(matches!(error, ConfigError::Invalid { field, reason }
        if field == "security.confirm_new_devices" && reason.contains("boolean")));
}

#[test]
fn invalid_values_are_rejected() {
    let mut config = HostConfig::default();
    config.access.max_viewers = 0;
    assert!(
        matches!(config.validate(), Err(ConfigError::Invalid { field, .. }) if field == "access.max_viewers")
    );
    config.access.max_viewers = 3;
    config.intranet.allow_sources = vec!["10.0.0.1/33".to_owned()];
    assert!(
        matches!(config.validate(), Err(ConfigError::Invalid { field, .. }) if field.starts_with("intranet.allow_sources"))
    );
}

#[tokio::test]
async fn store_publishes_updates() {
    let path = temp_path("store").join("config.toml");
    let store = ConfigStore::new(&path, HostConfig::default());
    let mut receiver = store.subscribe();
    store
        .update(&json!({"log": {"level": "debug"}}))
        .expect("update");
    receiver.changed().await.expect("watch update");
    assert_eq!(receiver.borrow().log.level.as_ref(), "debug");
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}

#[cfg(unix)]
#[test]
fn save_uses_private_permissions() {
    use std::os::unix::fs::PermissionsExt;
    let path = temp_path("permissions").join("config.toml");
    save(&path, &HostConfig::default()).expect("save");
    assert_eq!(
        fs::metadata(&path).expect("metadata").permissions().mode() & 0o777,
        0o600
    );
    let _ = fs::remove_dir_all(path.parent().expect("parent"));
}
