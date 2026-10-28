use std::collections::BTreeSet;
use std::sync::Arc;

use async_trait::async_trait;
use host_core::pairing::{
    pairings_digest, PairApproval, PairApprover, PairingCodeState, PairingConfirmOutcome,
    PairingError, PairingManager, PairingReply, PairingSession,
};
use host_core::store::{DevicePlatform, PairedDevice, PairingPath, PathPermissions, Store};
use host_core::whois::WhoIs;
use rsp_core::pairing::AppPairing;
use rsp_core::pb;
use rsp_core::trust::Permission;

struct Allow;

#[async_trait]
impl PairApprover for Allow {
    async fn request(&self, _request: host_core::pairing::PairApprovalRequest) -> PairApproval {
        PairApproval {
            allow: true,
            permission: Permission::Control,
        }
    }
}

struct Deny;

#[async_trait]
impl PairApprover for Deny {
    async fn request(&self, _request: host_core::pairing::PairApprovalRequest) -> PairApproval {
        PairApproval {
            allow: false,
            permission: Permission::ViewOnly,
        }
    }
}

fn store(name: &str) -> Arc<Store> {
    let root = std::env::var_os("TMPDIR")
        .map(std::path::PathBuf::from)
        .unwrap_or_else(|| std::path::PathBuf::from("target"));
    let path = root.join(format!("rsp-pairing-{name}-{}", std::process::id()));
    let _ = std::fs::remove_dir_all(&path);
    Arc::new(Store::open(path).expect("store"))
}

fn make_manager(store: Arc<Store>, host_fp: [u8; 32]) -> PairingManager {
    PairingManager::new(
        store,
        host_fp,
        "host",
        vec!["127.0.0.1:1".parse().expect("address")],
    )
}

fn status(reply: PairingReply) -> pb::pair_result::Status {
    let PairingReply::Result(result) = reply else {
        panic!("expected PairResult");
    };
    pb::pair_result::Status::try_from(result.status).expect("status")
}

#[tokio::test]
async fn app_and_host_pairing_round_trip_persists_permission() {
    let host_fp = [1; 32];
    let device_fp = [2; 32];
    let store = store("roundtrip");
    let manager = PairingManager::new(
        store.clone(),
        host_fp,
        "host",
        vec!["127.0.0.1:1".parse().expect("address")],
    )
    .with_approver(Arc::new(Allow))
    .with_clock(Arc::new(|| 1_000_002));
    let ticket = manager.start(1_000_000).expect("ticket");
    let mut session = manager.session_with_whois(
        device_fp,
        vec![1, 2, 3],
        Some(WhoIs {
            login: "user@example.com".into(),
            stable_id: "n-phone".into(),
            node_name: "phone.hs.example.com".into(),
        }),
        PairingPath::Tailnet,
    );
    let mut app = AppPairing::new(
        ticket.code.clone(),
        device_fp,
        host_fp,
        "phone",
        pb::Platform::Ios,
    )
    .expect("app");
    let start = app.try_start().expect("start");
    let PairingReply::Spake(host_spake) = session
        .on_start_at(1_000_001, start, true)
        .expect("host start")
    else {
        panic!("expected host spake");
    };
    let (app_spake, app_confirm) = app.on_spake(host_spake).expect("app spake");
    assert!(matches!(
        session.on_spake(app_spake).expect("host spake"),
        PairingReply::Continue
    ));
    let host_confirm = session.on_confirm(app_confirm).expect("host confirm");
    assert_eq!(
        app.on_confirm(host_confirm.0)
            .expect("app confirm")
            .device_fp(),
        &device_fp
    );
    let result = session.approve().await.expect("approval");
    assert_eq!(
        pb::pair_result::Status::try_from(result.status),
        Ok(pb::pair_result::Status::Ok)
    );
    let paired = store.get(&device_fp).expect("get").expect("paired");
    assert_eq!(
        paired,
        PairedDevice {
            fp: device_fp,
            device_id: rsp_core::ids::id_from_fp(&device_fp),
            name: "phone".into(),
            cert_der: vec![1, 2, 3],
            permission: Permission::Control,
            paired_at_us: 1_000_002,
            require_confirm: false,
            platform: DevicePlatform::Ios,
            paired_via: PairingPath::Tailnet,
            whois_user: Some("user@example.com".into()),
            whois_node: Some("n-phone".into()),
            whois_node_name: Some("phone.hs.example.com".into()),
            path_permissions: PathPermissions::default(),
        }
    );
}

#[test]
fn ticket_has_parseable_qr_and_expiry() {
    let store = store("qr");
    let manager = PairingManager::new(
        store,
        [4; 32],
        "host",
        vec!["127.0.0.1:1".parse().expect("address")],
    );
    let ticket = manager.start(10).expect("ticket");
    let qr = rsp_core::qr::PairQr::parse(&ticket.qr_url).expect("qr");
    assert_eq!(qr.code, ticket.code);
    assert_eq!(ticket.expires_at, 300_000_010);
}

#[test]
fn wrong_code_is_bad_code_and_decrements_remaining_attempts() {
    let host_fp = [7; 32];
    let device_fp = [8; 32];
    let store = store("wrong-code");
    let manager = PairingManager::new(
        store,
        host_fp,
        "host",
        vec!["127.0.0.1:1".parse().expect("address")],
    );
    let ticket = manager.start(1_000_000).expect("ticket");
    let mut session = manager.session(device_fp, Vec::new(), None, "lan");
    let mut app =
        AppPairing::new("000000", device_fp, host_fp, "phone", pb::Platform::Ios).expect("app");
    let start = app.try_start().expect("start");
    let PairingReply::Spake(host_spake) = session
        .on_start_at(1_000_001, start, true)
        .expect("host start")
    else {
        panic!("expected host spake");
    };
    let (app_spake, app_confirm) = app.on_spake(host_spake).expect("app spake");
    assert!(matches!(
        session.on_spake(app_spake).expect("host spake"),
        PairingReply::Continue
    ));
    let error = session.on_confirm(app_confirm).expect_err("bad code");
    assert!(matches!(
        error,
        host_core::pairing::PairingError::Protocol(rsp_core::pairing::PairingError::BadCode)
    ));
    let (_, failures) = manager
        .code_state(1_000_002)
        .expect("state")
        .expect("active");
    assert_eq!(failures, 1);
    let _ = ticket;
}

#[test]
fn expiry_and_cancel_disable_pairing() {
    let store = store("expiry");
    let manager = make_manager(store, [9; 32]);
    let ticket = manager.start(1_000_000).expect("ticket");
    assert_eq!(
        manager.code_state(ticket.expires_at).expect("state"),
        Some((host_core::pairing::PairingCodeState::Expired, 0))
    );
    let mut expired = manager.session([10; 32], Vec::new(), None, "lan");
    assert_eq!(
        status(
            expired
                .on_start_at(ticket.expires_at, pb::PairStart::default(), true)
                .expect("result")
        ),
        pb::pair_result::Status::Expired
    );

    let ticket = manager.start(2_000_000).expect("ticket");
    manager.cancel().expect("cancel");
    let mut cancelled = manager.session([11; 32], Vec::new(), None, "lan");
    assert_eq!(
        status(
            cancelled
                .on_start_at(ticket.expires_at - 1, pb::PairStart::default(), true)
                .expect("result")
        ),
        pb::pair_result::Status::Expired
    );
}

#[test]
fn fifth_bad_confirmation_expires_code_and_rate_limit_recovers() {
    let store = store("limits");
    let manager = make_manager(store.clone(), [12; 32]);
    let ticket = manager.start(3_000_000).expect("ticket");
    let wrong_code = if ticket.code == "000000" {
        "111111"
    } else {
        "000000"
    };
    for attempt in 0..5 {
        let device_fp = [20 + attempt as u8; 32];
        let mut session = manager.session(device_fp, Vec::new(), None, "lan");
        let mut app = AppPairing::new(wrong_code, device_fp, [12; 32], "phone", pb::Platform::Ios)
            .expect("app");
        let start = app.try_start().expect("start");
        let PairingReply::Spake(host_spake) = session
            .on_start_at(3_000_001 + attempt, start, true)
            .expect("spake")
        else {
            panic!("spake");
        };
        let (app_spake, app_confirm) = app.on_spake(host_spake).expect("app spake");
        assert!(matches!(
            session.on_spake(app_spake).expect("host spake"),
            PairingReply::Continue
        ));
        let _ = session.on_confirm(app_confirm);
    }
    let mut sixth = manager.session([30; 32], Vec::new(), None, "lan");
    assert_eq!(
        status(
            sixth
                .on_start_at(3_000_010, pb::PairStart::default(), true)
                .expect("result")
        ),
        pb::pair_result::Status::Expired
    );

    let manager = make_manager(Arc::clone(&store), [13; 32]);
    let _ = manager.start(4_000_000).expect("new ticket");
    for attempt in 0..20 {
        let mut session = manager.session([40 + attempt as u8; 32], Vec::new(), None, "lan");
        assert!(matches!(
            session
                .on_start_at(4_000_001 + attempt as u64, pb::PairStart::default(), true)
                .expect("start"),
            PairingReply::Spake(_)
        ));
    }
    let mut locked = manager.session([99; 32], Vec::new(), None, "lan");
    assert_eq!(
        status(
            locked
                .on_start_at(4_000_021, pb::PairStart::default(), true)
                .expect("locked")
        ),
        pb::pair_result::Status::Locked
    );
    manager
        .start(4_000_000 + host_core::pairing::PAIRING_ATTEMPT_WINDOW_US + 1)
        .expect("renew code");
    let mut recovered = manager.session([100; 32], Vec::new(), None, "lan");
    assert!(matches!(
        recovered
            .on_start_at(
                4_000_000 + host_core::pairing::PAIRING_ATTEMPT_WINDOW_US + 1,
                pb::PairStart::default(),
                true
            )
            .expect("recovered"),
        PairingReply::Spake(_)
    ));
}

#[tokio::test]
async fn local_rejection_and_revoked_fingerprint_do_not_persist() {
    let store = store("reject-revoked");
    let manager = make_manager(store.clone(), [50; 32]).with_approver(Arc::new(Deny));
    let ticket = manager.start(5_000_000).expect("ticket");
    let device_fp = [51; 32];
    let mut session = manager.session(device_fp, Vec::new(), None, "lan");
    let mut app =
        AppPairing::new(ticket.code, device_fp, [50; 32], "phone", pb::Platform::Ios).expect("app");
    let start = app.try_start().expect("start");
    let PairingReply::Spake(host_spake) =
        session.on_start_at(5_000_001, start, true).expect("spake")
    else {
        panic!("spake");
    };
    let (app_spake, app_confirm) = app.on_spake(host_spake).expect("app spake");
    let _ = session.on_spake(app_spake);
    let host_confirm = session.on_confirm(app_confirm).expect("confirm");
    let _ = app.on_confirm(host_confirm.0).expect("app confirm");
    assert_eq!(
        pb::pair_result::Status::try_from(session.approve().await.expect("deny").status),
        Ok(pb::pair_result::Status::Denied)
    );
    assert!(store.get(&device_fp).expect("get").is_none());

    let revoked_fp = [52; 32];
    store.revoke(revoked_fp, 1).expect("revoke");
    let manager = make_manager(store.clone(), [53; 32]).with_approver(Arc::new(Allow));
    let ticket = manager.start(6_000_000).expect("ticket");
    let mut session = manager.session(revoked_fp, Vec::new(), None, "lan");
    let mut app = AppPairing::new(
        ticket.code,
        revoked_fp,
        [53; 32],
        "revoked",
        pb::Platform::Ios,
    )
    .expect("app");
    let start = app.try_start().expect("start");
    let PairingReply::Spake(host_spake) =
        session.on_start_at(6_000_001, start, true).expect("spake")
    else {
        panic!("spake");
    };
    let (app_spake, app_confirm) = app.on_spake(host_spake).expect("app spake");
    let _ = session.on_spake(app_spake);
    let host_confirm = session.on_confirm(app_confirm).expect("confirm");
    let _ = app.on_confirm(host_confirm.0).expect("app confirm");
    assert_eq!(
        pb::pair_result::Status::try_from(session.approve().await.expect("revoked").status),
        Ok(pb::pair_result::Status::Denied)
    );
    assert!(store.get(&revoked_fp).expect("get").is_none());
}

#[test]
fn code_generation_has_all_digits() {
    let store = store("distribution");
    let manager = make_manager(store, [60; 32]);
    let mut first_digits = BTreeSet::new();
    for now in 0..10_000_u64 {
        let ticket = manager.start(now + 1).expect("ticket");
        first_digits.insert(ticket.code.as_bytes()[0]);
    }
    assert_eq!(first_digits.len(), 10);
}

fn confirm_pairing(session: &mut PairingSession, app: &mut AppPairing, now: u64) {
    let PairingReply::Spake(host_spake) = session
        .on_start_at(now, app.try_start().expect("app start"), true)
        .expect("host start")
    else {
        panic!("expected SPAKE2");
    };
    let (app_spake, app_confirm) = app.on_spake(host_spake).expect("app SPAKE2");
    assert_eq!(session.on_spake(app_spake).unwrap(), PairingReply::Continue);
    let (host_confirm, waiting) = session.on_confirm(app_confirm).expect("host confirmation");
    app.on_confirm(host_confirm).expect("app confirmation");
    assert_eq!(waiting.status, pb::pair_result::Status::WaitingLocal as i32);
    assert_eq!(
        app.on_result(waiting),
        rsp_core::pairing::Outcome::WaitingLocal
    );
}

#[tokio::test]
async fn mismatched_host_identity_fails_both_confirmations_and_exhausts_code() {
    let store = store("mitm");
    let actual_host_fp = [61; 32];
    let app_observed_host_fp = [62; 32];
    let manager = make_manager(store.clone(), actual_host_fp)
        .with_approver(Arc::new(Allow))
        .with_clock(Arc::new(|| 7_000_002));
    let ticket = manager.start(7_000_000).unwrap();
    let before = store.paired().unwrap();
    for attempt in 1..=rsp_core::pairing::MAX_CODE_FAILURES {
        let device_fp = [63; 32];
        let mut session = manager.session(device_fp, vec![1, 2, 3], None, "tailnet");
        let mut app = AppPairing::new(
            &ticket.code,
            device_fp,
            app_observed_host_fp,
            "phone",
            pb::Platform::Ios,
        )
        .unwrap();
        let PairingReply::Spake(host_spake) = session
            .on_start_at(7_000_001, app.try_start().unwrap(), true)
            .unwrap()
        else {
            panic!("expected SPAKE2");
        };
        let (app_spake, app_confirm) = app.on_spake(host_spake).unwrap();
        assert_eq!(session.on_spake(app_spake).unwrap(), PairingReply::Continue);
        let PairingConfirmOutcome::Result(result) =
            session.on_confirm_outcome(app_confirm.clone()).unwrap()
        else {
            panic!("mismatched identity must fail confirmation");
        };
        let remaining = rsp_core::pairing::MAX_CODE_FAILURES - attempt;
        assert_eq!(result.attempts_left, remaining);
        assert_eq!(
            result.status,
            if remaining == 0 {
                pb::pair_result::Status::Expired as i32
            } else {
                pb::pair_result::Status::BadCode as i32
            }
        );
        // The host emits no confirmation after rejection. Reflecting the
        // app's MAC as a forged host confirmation must also fail at the app.
        assert_eq!(
            app.on_confirm(app_confirm),
            Err(rsp_core::pairing::PairingError::ManInTheMiddle)
        );
        assert!(!app.is_verified());
        assert!(matches!(
            session.approve().await,
            Err(PairingError::Protocol(_))
        ));
        assert_eq!(store.paired().unwrap(), before);
        assert_eq!(
            manager.code_state(7_000_002).unwrap(),
            Some((
                if remaining == 0 {
                    PairingCodeState::Failed
                } else {
                    PairingCodeState::Active
                },
                attempt,
            ))
        );
    }
    let mut retry = manager.session([64; 32], Vec::new(), None, "tailnet");
    assert_eq!(
        status(
            retry
                .on_start_at(7_000_003, pb::PairStart::default(), true)
                .unwrap()
        ),
        pb::pair_result::Status::Expired
    );
}

struct NoResponse;

#[async_trait]
impl PairApprover for NoResponse {
    async fn request(&self, _request: host_core::pairing::PairApprovalRequest) -> PairApproval {
        std::future::pending().await
    }
}

#[tokio::test(start_paused = true)]
async fn local_approval_timeout_denies_after_thirty_seconds_and_consumes_code() {
    let store = store("approval-timeout");
    let manager = make_manager(store.clone(), [65; 32]).with_approver(Arc::new(NoResponse));
    let ticket = manager.start(8_000_000).unwrap();
    let mut session = manager.session([66; 32], Vec::new(), None, "lan");
    let mut app =
        AppPairing::new(&ticket.code, [66; 32], [65; 32], "phone", pb::Platform::Ios).unwrap();
    confirm_pairing(&mut session, &mut app, 8_000_001);
    let approval = tokio::spawn(async move { session.approve().await });
    tokio::task::yield_now().await;
    tokio::time::advance(std::time::Duration::from_secs(29)).await;
    tokio::task::yield_now().await;
    assert!(
        !approval.is_finished(),
        "approval must remain pending for 30 seconds"
    );
    tokio::time::advance(std::time::Duration::from_secs(1)).await;
    let result = approval.await.unwrap().unwrap();
    assert_eq!(result.status, pb::pair_result::Status::Denied as i32);
    assert_eq!(app.on_result(result), rsp_core::pairing::Outcome::Denied);
    assert!(store.paired().unwrap().is_empty());
    assert_eq!(
        manager.code_state(8_000_002).unwrap(),
        Some((PairingCodeState::Used, 0))
    );
    let mut retry = manager.session([67; 32], Vec::new(), None, "lan");
    assert_eq!(
        status(
            retry
                .on_start_at(8_000_003, pb::PairStart::default(), true)
                .unwrap()
        ),
        pb::pair_result::Status::Expired
    );
}

#[tokio::test]
async fn successful_pairing_code_cannot_be_reused() {
    let store = store("code-reuse");
    let manager = make_manager(store.clone(), [68; 32]).with_approver(Arc::new(Allow));
    let ticket = manager.start(9_000_000).unwrap();
    let mut session = manager.session([69; 32], Vec::new(), None, "intranet");
    let mut app = AppPairing::new(
        &ticket.code,
        [69; 32],
        [68; 32],
        "phone",
        pb::Platform::Android,
    )
    .unwrap();
    confirm_pairing(&mut session, &mut app, 9_000_001);
    assert_eq!(
        session.approve().await.unwrap().status,
        pb::pair_result::Status::Ok as i32
    );
    let before = store.paired().unwrap();
    assert_eq!(before.len(), 1);
    assert_eq!(before[0].platform, DevicePlatform::Android);
    assert_eq!(before[0].paired_via, PairingPath::Intranet);
    assert_eq!(
        manager.code_state(9_000_002).unwrap(),
        Some((PairingCodeState::Used, 0))
    );
    for device_fp in [[69; 32], [70; 32]] {
        let mut retry = manager.session(device_fp, Vec::new(), None, "intranet");
        let mut app = AppPairing::new(
            &ticket.code,
            device_fp,
            [68; 32],
            "phone",
            pb::Platform::Android,
        )
        .unwrap();
        assert_eq!(
            status(
                retry
                    .on_start_at(9_000_003, app.try_start().unwrap(), true)
                    .unwrap()
            ),
            pb::pair_result::Status::Expired
        );
        assert_eq!(store.paired().unwrap(), before);
    }
}

#[test]
fn pairings_digest_lists_each_paired_fingerprint() {
    let store = store("digest-ids");
    let fp: [u8; 32] = std::array::from_fn(|i| i as u8);
    store
        .upsert(PairedDevice {
            fp,
            device_id: "legacy-id-must-not-be-trusted".into(),
            name: "phone".into(),
            cert_der: Vec::new(),
            permission: Permission::Control,
            paired_at_us: 42,
            require_confirm: false,
            platform: DevicePlatform::Unknown,
            paired_via: PairingPath::Tailnet,
            whois_user: None,
            whois_node: None,
            whois_node_name: None,
            path_permissions: PathPermissions::default(),
        })
        .unwrap();
    let digest = pairings_digest(&store).unwrap();
    assert_eq!(digest.devices.len(), 1);
    assert_eq!(digest.devices[0].fp, rsp_core::ids::fp_b64url(&fp));
    let mut same_prefix = store.get(&fp).unwrap().unwrap();
    same_prefix.fp[31] ^= 1;
    let other = same_prefix.fp;
    store.upsert(same_prefix).unwrap();
    let fps = |store: &Store| {
        pairings_digest(store)
            .unwrap()
            .devices
            .into_iter()
            .map(|device| device.fp)
            .collect::<Vec<_>>()
    };
    assert_eq!(
        fps(&store),
        vec![
            rsp_core::ids::fp_b64url(&other),
            rsp_core::ids::fp_b64url(&fp)
        ]
    );
    store.revoke(fp, 1).unwrap();
    assert_eq!(fps(&store), vec![rsp_core::ids::fp_b64url(&other)]);
}
