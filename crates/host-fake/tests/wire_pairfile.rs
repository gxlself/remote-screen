//! Pairing files through the real Host runtime and rsp-client control path.
use std::fs;
use std::sync::Arc;
use std::time::Duration;

use host_core::pairing::ExportFileRequest;
use host_fake::harness::{app_identity, direct_params, open, Approval, TestHost, TestHostOptions};
use rsp_client::session::{SessionEvent, SessionParams};
use rsp_core::pairfile::{parse, PairFile};
use rsp_core::pb::{self, pair_result::Status};
use rsp_core::trust::Permission;
use rsp_transport::identity::LocalIdentity;

fn export(host: &TestHost, permission: Permission) -> PairFile {
    assert!(host.security.pairing.has_files());
    let exported = host
        .security
        .pairing
        .export_file(
            rsp_core::clock::unix_micros(),
            ExportFileRequest {
                permission,
                label: Some("wire test".into()),
                dir: host.state_dir.clone(),
            },
        )
        .unwrap();
    parse(&fs::read_to_string(exported.path).unwrap()).unwrap()
}

async fn attempt(
    host: &TestHost,
    id: Arc<LocalIdentity>,
    file: &PairFile,
    wrong: bool,
) -> Vec<pb::PairResult> {
    let mut secret = file.secret.clone();
    if wrong {
        secret[0] ^= 1;
    }
    let mut params = direct_params(host, id).with_pairing_file(file.file_id, secret);
    params.intent = pb::Intent::Pair;
    let (session, mut events) = open(params).await.unwrap();
    let results = tokio::time::timeout(Duration::from_secs(5), async {
        let mut results = Vec::new();
        let mut hello_seen = false;
        let mut ended = false;
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::ServerHello(hello) => {
                    hello_seen = true;
                    assert_ne!(hello.features & rsp_core::features::PAIR_FILE, 0);
                    assert!(hello.host_name.is_empty());
                    assert!(hello.os_version.is_empty());
                    assert!(hello.displays.is_empty());
                    assert_eq!(hello.os, pb::Platform::Unspecified as i32);
                }
                SessionEvent::PairResult(result) => results.push(result),
                SessionEvent::Frame(_) => panic!("pairing must not start media"),
                SessionEvent::Error { code, message } => {
                    panic!("unexpected error: {code:?} {message}")
                }
                SessionEvent::Ended { .. } => {
                    ended = true;
                    break;
                }
                _ => {}
            }
        }
        assert!(hello_seen, "ServerHello was not delivered");
        assert!(ended, "pairing connection did not terminate");
        results
    })
    .await
    .expect("pairing completes within 5 seconds");
    session.close();
    results
}

fn assert_result(results: &[pb::PairResult], status: Status, attempts_left: u32) {
    assert_eq!(
        results.len(),
        1,
        "file pairing sends no WAITING_LOCAL: {results:?}"
    );
    assert_eq!(results[0].status, status as i32);
    assert_eq!(results[0].attempts_left, attempts_left);
}

async fn control_with_video(params: SessionParams, role: pb::Role) {
    let (session, mut events) = open(params).await.unwrap();
    tokio::time::timeout(Duration::from_secs(5), async {
        let mut seen_role = None;
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::Role(role) => seen_role = Some(role),
                SessionEvent::Frame(_) => {
                    assert_eq!(seen_role, Some(role));
                    return;
                }
                SessionEvent::Error { code, message } => {
                    panic!("control error: {code:?} {message}")
                }
                SessionEvent::Ended { code, reason, .. } => {
                    panic!("ended before video: {code:?} {reason}")
                }
                _ => {}
            }
        }
        panic!("event stream closed before video");
    })
    .await
    .expect("paired identity receives video within 5 seconds");
    session.close();
}

// PairingApproval::Deny proves file pairing never asks the configured approver.
#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn file_pairs_then_same_identity_receives_video_with_exported_role() {
    for (permission, role) in [
        (Permission::ViewOnly, pb::Role::Viewer),
        (Permission::Unattended, pb::Role::Controller),
    ] {
        let host = TestHost::start_with(TestHostOptions {
            pairing_approval: Some(Approval::Deny),
            ..Default::default()
        })
        .await;
        let file = export(&host, permission);
        let identity = app_identity();
        assert_result(
            &attempt(&host, identity.clone(), &file, false).await,
            Status::Ok,
            0,
        );
        assert_eq!(
            host.security
                .store
                .get(&identity.fingerprint())
                .unwrap()
                .unwrap()
                .permission,
            permission
        );
        control_with_video(direct_params(&host, identity.clone()), role).await;
        assert_result(
            &attempt(&host, app_identity(), &file, false).await,
            Status::Expired,
            0,
        );
        assert_eq!(host.security.store.paired().unwrap().len(), 1);
        let audit = host.security.audit.as_ref().unwrap().batch(100).unwrap();
        let successes: Vec<_> = audit
            .items
            .iter()
            .filter(|item| item.event == "pairing.file")
            .collect();
        assert_eq!(successes.len(), 1);
        assert_eq!(
            successes[0].device_id,
            Some(rsp_core::ids::id_from_fp(&identity.fingerprint()))
        );
        let file_id = host
            .security
            .pairing
            .list_files(rsp_core::clock::unix_micros())
            .unwrap()[0]
            .file_id
            .clone();
        assert_eq!(
            successes[0].detail,
            Some(serde_json::json!({"file_id": file_id}))
        );
        let failures: Vec<_> = audit
            .items
            .iter()
            .filter(|item| item.event == "pairing.denied")
            .collect();
        assert_eq!(failures.len(), 1);
        assert_eq!(
            failures[0].detail,
            Some(serde_json::json!({"method":"file", "status":Status::Expired as i32}))
        );
        // Extract the base64url secret without adding a test-only dependency.
        let uri = file.to_uri();
        let secret = uri.split("&s=").nth(1).unwrap().split('&').next().unwrap();
        let json = serde_json::to_string(&audit.items).unwrap();
        assert!(!json.contains(secret));
        assert!(!json.contains("rsp://pairfile"));
        host.stop().await;
    }
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn wrong_secret_exhausts_file_and_every_failure_is_audited() {
    let host = TestHost::start().await;
    let file = export(&host, Permission::Control);
    for attempt_no in 1..=5 {
        assert_result(
            &attempt(&host, app_identity(), &file, true).await,
            if attempt_no == 5 {
                Status::Expired
            } else {
                Status::BadCode
            },
            5 - attempt_no,
        );
        assert!(host.security.store.paired().unwrap().is_empty());
    }
    assert_result(
        &attempt(&host, app_identity(), &file, false).await,
        Status::Expired,
        0,
    );
    let audit = host.security.audit.as_ref().unwrap().batch(100).unwrap();
    assert_eq!(audit.items.len(), 6);
    for item in &audit.items {
        assert_eq!(item.event, "pairing.denied");
        assert_eq!(item.detail.as_ref().unwrap()["method"], "file");
    }
    let uri = file.to_uri();
    let secret = uri.split("&s=").nth(1).unwrap().split('&').next().unwrap();
    assert!(!serde_json::to_string(&audit.items)
        .unwrap()
        .contains(secret));
    host.stop().await;
}

#[tokio::test]
async fn corrupt_file_store_does_not_prevent_host_starting() {
    use host_core::{HostRuntime, RuntimeOptions};
    use host_fake::FakePlatform;
    let path = std::env::temp_dir().join(format!("h20e-corrupt-{}", std::process::id()));
    fs::create_dir_all(&path).unwrap();
    fs::write(path.join("pairfiles.json"), b"corrupt").unwrap();
    let host = HostRuntime::new(
        FakePlatform::new(),
        RuntimeOptions {
            bind: Some("127.0.0.1:0".parse().unwrap()),
            store_dir: path.clone(),
            ..Default::default()
        },
    )
    .bind()
    .await
    .expect("corrupt pairing files must not fail Host startup");
    assert!(!host.security.pairing.has_files());
    let mut session = host.security.pairing.session([13; 32], vec![], None, "lan");
    let host_core::pairing::PairingReply::Result(result) = session
        .on_start(
            pb::PairStart {
                pair_file_id: vec![1; 16],
                ..Default::default()
            },
            true,
        )
        .unwrap()
    else {
        panic!("expected expiry")
    };
    assert_eq!(result.status, Status::Expired as i32);
    drop(host);
    fs::remove_dir_all(path).unwrap();
}
