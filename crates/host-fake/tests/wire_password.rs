//! Account-password pairing through the actual TLS and Host control path.
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use host_core::config::HostConfig;
use host_core::sysauth::{SysAuthError, SystemAccount, SystemAuthenticator};
use host_fake::harness::{
    app_identity, direct_params, open, wait_frames, Approval, TestHost, TestHostOptions,
};
use host_fake::FakePlatform;
use rsp_client::session::SessionEvent;
use rsp_core::pb::{self, pair_result::Status};
use rsp_core::trust::Permission;
use rsp_transport::identity::LocalIdentity;
use serde_json::json;

const PASSWORD: &str = "wire-password-private!";
const WRONG_PASSWORD: &str = "wire-wrong-password-private!";
const UNKNOWN: &str = "wire-private-unknown-account";

struct Auth;
impl SystemAuthenticator for Auth {
    fn lookup(&self, name: &str) -> Result<Option<SystemAccount>, SysAuthError> {
        Ok(
            matches!(name, "alice" | "Alice Liu").then(|| SystemAccount {
                short_name: "alice".into(),
                full_name: "Alice Liu".into(),
                is_admin: true,
                is_console_user: false,
            }),
        )
    }
    fn verify(&self, name: &str, password: &[u8]) -> Result<(), SysAuthError> {
        if name == "alice" && password == PASSWORD.as_bytes() {
            Ok(())
        } else {
            Err(SysAuthError::WrongPassword)
        }
    }
}

async fn attempt(
    host: &TestHost,
    id: Arc<LocalIdentity>,
    account: &str,
    password: &str,
) -> pb::PairResult {
    let mut params =
        direct_params(host, id).with_pairing_password(account, password.as_bytes().to_vec().into());
    params.intent = pb::Intent::Pair;
    let (session, mut events) = open(params).await.unwrap();
    let result = tokio::time::timeout(Duration::from_secs(5), async {
        let mut result = None;
        let mut hello_seen = false;
        let mut ended = false;
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::ServerHello(hello) => {
                    hello_seen = true;
                    assert_ne!(hello.features & rsp_core::features::PAIR_PASSWORD, 0);
                    assert!(hello.host_name.is_empty());
                    assert!(hello.os_version.is_empty());
                    assert!(hello.displays.is_empty());
                    assert_eq!(hello.os, pb::Platform::Unspecified as i32);
                }
                SessionEvent::PairResult(value) => {
                    assert!(
                        result.replace(value).is_none(),
                        "password pairing has one terminal result"
                    );
                }
                SessionEvent::Frame(_) => panic!("pairing connection started video"),
                SessionEvent::Error { code, message } => {
                    panic!("unexpected error {code:?}: {message}")
                }
                SessionEvent::Ended { .. } => {
                    ended = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(hello_seen && ended);
        result.expect("password pairing result")
    })
    .await
    .expect("password pairing completes in five seconds");
    session.close();
    result
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn password_pairs_control_streams_and_failures_are_private_in_audit_and_logs() {
    // Install the real Host JSON logger in this test process and inspect flushed
    // files after stopping all runtimes, including spawn_blocking auth workers.
    let log_dir = std::env::temp_dir().join(format!(
        "wire-pw-logs-{}-{}",
        std::process::id(),
        rsp_core::clock::unix_micros()
    ));
    let guard = host_core::logs::init(&log_dir, "trace").unwrap();
    let platform = FakePlatform::new();
    assert!(host_core::platform::Platform::system_authenticator(&platform).is_none());
    platform.set_system_authenticator(Arc::new(Auth));
    let mut config = HostConfig::default();
    config.security.password_pairing = true;
    // The existing harness accepts config through TestHostOptions, preserving
    // the actual HostServices/ConfigStore path without a test-only runtime hook.
    let host = TestHost::start_with_platform(
        platform.clone(),
        TestHostOptions {
            config: Some(config),
            pairing_approval: Some(Approval::Deny),
            ..Default::default()
        },
    )
    .await;
    let identity = app_identity();
    let result = attempt(&host, identity.clone(), "Alice Liu", PASSWORD).await;
    assert_eq!(result.status, Status::Ok as i32);
    assert_eq!(result.host_id, rsp_core::ids::id_from_fp(&host.fp));
    assert_eq!(result.host_name, "fake-host");
    assert_eq!(
        host.security
            .store
            .get(&identity.fingerprint())
            .unwrap()
            .unwrap()
            .permission,
        Permission::Control
    );

    let (session, mut events) = open(direct_params(&host, identity.clone())).await.unwrap();
    let report = wait_frames(&mut events, 1, Duration::from_secs(5)).await;
    assert_eq!(
        report.frames, 1,
        "paired identity receives video: {report:?}"
    );
    assert!(report.errors.is_empty());
    assert!(report
        .states
        .contains(&rsp_client::session::SessionStateKind::Streaming));
    session.close();
    for account in ["alice", UNKNOWN] {
        let result = attempt(&host, app_identity(), account, WRONG_PASSWORD).await;
        assert_eq!(result.status, Status::BadCode as i32);
        assert!(result.host_id.is_empty());
    }
    let audit = host.security.audit.as_ref().unwrap().batch(100).unwrap();
    let successes: Vec<_> = audit
        .items
        .iter()
        .filter(|item| item.event == "pairing.password")
        .collect();
    assert_eq!(successes.len(), 1);
    assert_eq!(
        successes[0].device_id,
        Some(rsp_core::ids::id_from_fp(&identity.fingerprint()))
    );
    assert_eq!(
        successes[0].detail,
        Some(json!({"result":"ok", "account":"alice"}))
    );
    let denied: Vec<_> = audit
        .items
        .iter()
        .filter(|item| item.event == "pairing.denied")
        .collect();
    assert_eq!(denied.len(), 2);
    for item in denied {
        assert_eq!(
            item.detail,
            Some(json!({"method":"password", "status":Status::BadCode as i32}))
        );
        let json = serde_json::to_string(item).unwrap();
        assert!(!json.contains("alice"));
        assert!(!json.contains(UNKNOWN));
    }
    assert_eq!(host.security.store.paired().unwrap().len(), 1);
    host.stop().await;

    // Default config keeps the method disabled even on a supported platform.
    let host = TestHost::start_with_platform(platform, TestHostOptions::default()).await;
    let result = attempt(&host, app_identity(), UNKNOWN, PASSWORD).await;
    assert_eq!(result.status, Status::MethodDisabled as i32);
    let audit = host.security.audit.as_ref().unwrap().batch(100).unwrap();
    assert_eq!(audit.items.len(), 1);
    assert_eq!(audit.items[0].event, "pairing.denied");
    assert_eq!(
        audit.items[0].detail,
        Some(json!({"method":"password", "status":Status::MethodDisabled as i32}))
    );
    assert!(!serde_json::to_string(&audit.items)
        .unwrap()
        .contains(UNKNOWN));
    assert!(host.security.store.paired().unwrap().is_empty());
    host.stop().await;
    drop(guard);
    let logs = fs::read_dir(&log_dir)
        .unwrap()
        .map(|entry| fs::read_to_string(entry.unwrap().path()).unwrap())
        .collect::<Vec<_>>()
        .join("\n");
    assert!(
        logs.contains("pairing.password"),
        "Host logger did not capture authentication"
    );
    assert!(logs.contains("pairing.denied"));
    for secret in [PASSWORD, WRONG_PASSWORD, UNKNOWN, "Alice Liu"] {
        assert!(!logs.contains(secret), "sensitive value leaked");
    }
    fs::remove_dir_all(log_dir).unwrap();
}
