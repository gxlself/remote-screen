mod support;

use std::path::PathBuf;
use std::sync::{Arc, Mutex};
use std::time::Duration;

use rs_mobile::rscore::dto_core::{CoreConfig, CoreEvent, DevicePlatform};
use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::{
    core, core_init, core_shutdown, events_subscribe, events_unsubscribe, install_core,
    logs_export, logs_write, take_core,
};
use rs_mobile::rscore::signer::{clear_global_signer, global_signer, register_global_signer};
use rsp_transport::identity::ExternalSigner;
use support::{temp_dir, test_core, with_identity, EventCollector, TestSigner};

// Keep all process-wide core, logging and signer mutations in this single test.
#[test]
fn global_lifecycle_reload_signer_and_log_exports() {
    let rt = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    assert_eq!(
        core().err().unwrap(),
        CoreError::internal("core not initialized")
    );
    assert_eq!(
        events_subscribe(EventCollector::new()),
        Err(CoreError::internal("core not initialized"))
    );
    events_unsubscribe(99);
    logs_write("info".to_owned(), "T".to_owned(), "before init".to_owned());
    assert!(
        matches!(rt.block_on(core_init("{".to_owned())), Err(CoreError::Internal { message }) if message.starts_with("bad json: "))
    );
    let dir = temp_dir("global");
    let mut cfg = CoreConfig {
        files_dir: None,
        data_dir: dir.to_str().unwrap().to_owned(),
        app_version: "1.0".to_owned(),
        platform: DevicePlatform::Ios,
    };
    // Older apps still send the legacy directory URL; it is ignored.
    let with_hub_url = |cfg: &CoreConfig, url: &str| {
        let mut json = serde_json::to_value(cfg).unwrap();
        json["hubUrl"] = url.into();
        json.to_string()
    };
    rt.block_on(core_init(with_hub_url(&cfg, ""))).unwrap();
    let c = core().unwrap();
    let state = c.state::<Mutex<Vec<u32>>>();
    state.lock().unwrap().push(7);
    let first = EventCollector::new();
    assert_eq!(events_subscribe(first.clone()).unwrap(), 1);
    cfg.app_version = "1.1".to_owned();
    rt.block_on(core_init(with_hub_url(&cfg, "https://hs.example.com")))
        .unwrap();
    assert!(Arc::ptr_eq(&c, &core().unwrap()));
    assert!(Arc::ptr_eq(
        &state,
        &core().unwrap().state::<Mutex<Vec<u32>>>()
    ));
    assert_eq!(*state.lock().unwrap(), vec![7]);
    assert_eq!(c.config(), cfg);
    let event = CoreEvent::SessionIdleWarning {
        session_id: "1".to_owned(),
        seconds_left: 3,
        lock_on_end: false,
    };
    c.events().emit(&event);
    assert!(c.events().flush(Duration::from_secs(5)));
    assert!(first.events().is_empty());
    let second = EventCollector::new();
    let id = events_subscribe(second.clone()).unwrap();
    assert_eq!(id, 2);
    c.events().emit(&event);
    assert!(c.events().flush(Duration::from_secs(5)));
    assert_eq!(second.events().len(), 1);
    events_unsubscribe(id);
    events_unsubscribe(99);
    c.events().emit(&event);
    assert!(c.events().flush(Duration::from_secs(5)));
    assert_eq!(second.events().len(), 1);
    let mut other = cfg.clone();
    other.data_dir = temp_dir("other").to_str().unwrap().to_owned();
    assert_eq!(
        rt.block_on(core_init(serde_json::to_string(&other).unwrap())),
        Err(CoreError::internal(
            "already initialized with another dataDir"
        ))
    );

    assert!(global_signer().is_none());
    let signer = with_identity(&c, "Test device");
    c.set_signer(None);
    register_global_signer(signer.clone());
    assert_eq!(global_signer().unwrap().spki_der(), signer.spki_der());
    assert!(c.local_identity().is_ok());
    register_global_signer(TestSigner::new());
    c.identity_changed();
    assert_eq!(
        c.local_identity().err(),
        Some(CoreError::internal("device key changed"))
    );
    clear_global_signer();
    c.identity_changed();
    assert_eq!(
        c.local_identity().err(),
        Some(CoreError::internal("device signer not registered"))
    );

    let exports = dir.join("exports");
    std::fs::create_dir_all(&exports).unwrap();
    std::fs::write(exports.join("rs-logs-old.zip"), b"old").unwrap();
    std::fs::write(exports.join("keep.txt"), b"keep").unwrap();
    std::fs::create_dir(exports.join("rs-logs-directory.zip")).unwrap();
    logs_write("info".to_owned(), "T".to_owned(), "x".to_owned());
    let path = PathBuf::from(rt.block_on(logs_export(true)).unwrap());
    assert!(path.is_absolute());
    assert!(path.is_file());
    assert_eq!(path.parent().unwrap(), exports);
    assert!(zip::ZipArchive::new(std::fs::File::open(&path).unwrap()).is_ok());
    assert!(!exports.join("rs-logs-old.zip").exists());
    assert!(exports.join("keep.txt").is_file());
    assert!(exports.join("rs-logs-directory.zip").is_dir());
    let next = PathBuf::from(rt.block_on(logs_export(false)).unwrap());
    assert!(next.is_file());
    assert_eq!(
        std::fs::read_dir(&exports)
            .unwrap()
            .filter_map(Result::ok)
            .filter(|e| e.file_type().unwrap().is_file()
                && e.path().extension().is_some_and(|ext| ext == "zip"))
            .count(),
        1
    );
    let hooks = Arc::new(Mutex::new(0));
    let hook_count = hooks.clone();
    c.add_shutdown_hook(Box::new(move || *hook_count.lock().unwrap() += 1));
    core_shutdown();
    core_shutdown();
    assert_eq!(*hooks.lock().unwrap(), 1);
    assert_eq!(
        core().err().unwrap(),
        CoreError::internal("core not initialized")
    );
    assert!(rt.block_on(logs_export(true)).is_err());

    let replacement = test_core();
    install_core(replacement.clone());
    assert!(Arc::ptr_eq(&replacement, &core().unwrap()));
    assert!(Arc::ptr_eq(&replacement, &take_core().unwrap()));
    assert!(take_core().is_none());
}
