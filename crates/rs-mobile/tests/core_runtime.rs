mod support;

use std::sync::{Arc, Mutex};
use std::time::Duration;

use rs_mobile::rscore::error::CoreError;
use rs_mobile::rscore::runtime::{Core, CoreOptions, Timeouts};
use rs_mobile::rscore::signer::spki_matches;
use rsp_core::ids::id_from_fp;
use rsp_transport::identity::{spki_fingerprint, ExternalSigner};
use support::{test_core, test_core_with, with_identity, TestSigner};

#[test]
fn paths_state_clock_and_defaults() {
    let c = test_core_with(|_, opts| opts.clock = Some(Arc::new(|| 123_456)));
    assert!(c.data_dir().join("rs.db").is_file());
    assert_eq!(c.config().data_dir, c.data_dir().to_str().unwrap());
    assert_eq!(c.log_dir(), c.data_dir().join("logs"));
    assert_eq!(c.now_us(), 123_456);
    #[derive(Default)]
    struct Foo(Mutex<u32>);
    #[derive(Default)]
    struct Bar(Mutex<u32>);
    let foo = c.state::<Foo>();
    assert!(Arc::ptr_eq(&foo, &c.state::<Foo>()));
    *foo.0.lock().unwrap() = 7;
    assert_eq!(*c.state::<Foo>().0.lock().unwrap(), 7);
    assert_eq!(*c.state::<Bar>().0.lock().unwrap(), 0);
    let opts = CoreOptions::default();
    assert_eq!(opts.worker_threads, 2);
    assert!(opts.init_logging);
    let timeouts = Timeouts::default();
    assert_eq!(timeouts.hostkey_prompt, Duration::from_secs(60));
    assert_eq!(timeouts.pairing, Duration::from_secs(120));
    assert_eq!(timeouts.probe, Duration::from_secs(3));
    assert_eq!(timeouts.resume_token_ttl, Duration::from_secs(60));
    assert_eq!(c.timeouts().probe, timeouts.probe);
}

#[tokio::test]
async fn runs_on_owned_runtime_and_drops_in_another_runtime() {
    let c = test_core();
    assert_eq!(c.run(async { Ok(7) }).await.unwrap(), 7);
    let thread = c
        .run(async {
            tokio::time::sleep(Duration::from_millis(1)).await;
            Ok(std::thread::current().name().unwrap().to_owned())
        })
        .await
        .unwrap();
    assert_eq!(thread, "rs-core");
    assert_eq!(
        c.run::<(), _>(async { panic!("test task panic") }).await,
        Err(CoreError::internal("task panicked"))
    );
    let (send, receive) = tokio::sync::oneshot::channel();
    c.spawn(async move {
        send.send(()).unwrap();
    });
    receive.await.unwrap();
    drop(c);
    drop(test_core());
}

#[test]
fn identity_validation_cache_and_device_id() {
    let c = test_core();
    assert_eq!(
        c.local_identity().err(),
        Some(CoreError::internal("identity not created"))
    );
    assert_eq!(
        c.device_id(),
        Err(CoreError::internal("identity not created"))
    );
    let signer = with_identity(&c, "Test device");
    assert!(signer.sign_count() > 0);
    let cert = c.store().identity().unwrap().unwrap().cert_der;
    let fp = spki_fingerprint(&cert).unwrap();
    let identity = c.local_identity().unwrap();
    assert_eq!(identity.fingerprint(), fp);
    assert!(Arc::ptr_eq(&identity, &c.local_identity().unwrap()));
    assert!(spki_matches(signer.as_ref(), &cert));
    assert!(!spki_matches(signer.as_ref(), b"invalid certificate"));
    let id = c.device_id().unwrap();
    assert_eq!(id, id_from_fp(&fp));
    assert_eq!(id.len(), 16);
    assert!(id
        .bytes()
        .all(|b| b.is_ascii_lowercase() || (b'2'..=b'7').contains(&b)));
    assert_eq!(signer.spki_der(), c.signer().unwrap().spki_der());
    c.set_signer(Some(TestSigner::new()));
    c.identity_changed();
    assert_eq!(
        c.local_identity().err(),
        Some(CoreError::internal("device key changed"))
    );
    c.set_signer(None);
    c.identity_changed();
    // No process-global signer is installed in this test binary.
    assert_eq!(
        c.local_identity().err(),
        Some(CoreError::internal("device signer not registered"))
    );
    c.set_signer(Some(signer));
    c.identity_changed();
    assert!(!Arc::ptr_eq(&identity, &c.local_identity().unwrap()));
}

#[test]
fn config_update_and_shutdown_hooks() {
    let c = test_core();
    c.update_config("1.2.3".to_owned());
    assert_eq!(c.config().app_version, "1.2.3");
    let calls = Arc::new(Mutex::new(Vec::new()));
    for index in 0..2 {
        let calls = calls.clone();
        c.add_shutdown_hook(Box::new(move || calls.lock().unwrap().push(index)));
    }
    c.shutdown();
    c.shutdown();
    assert_eq!(*calls.lock().unwrap(), vec![0, 1]);
}

#[test]
fn errors_match_ts_contract() {
    let bad_json = serde_json::from_str::<serde_json::Value>("{").unwrap_err();
    assert!(
        matches!(CoreError::from(bad_json), CoreError::Internal { message } if message.starts_with("bad json: "))
    );
    assert!(matches!(
        CoreError::from(std::io::Error::other("x")),
        CoreError::Internal { .. }
    ));
}

#[test]
fn store_poisoning_is_recovered_and_invalid_thread_count_is_an_error() {
    let c = test_core();
    let other = c.clone();
    assert!(std::thread::spawn(move || {
        let _store = other.store();
        panic!("poison store");
    })
    .join()
    .is_err());
    assert!(c.store().identity().unwrap().is_none());
    assert!(Core::new(
        c.config(),
        CoreOptions {
            worker_threads: 0,
            init_logging: false,
            ..CoreOptions::default()
        }
    )
    .is_err());
}
