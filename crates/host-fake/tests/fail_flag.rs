use std::time::Duration;

use host_fake::harness::{
    app_identity, direct_params, open, pair_with_code, wait_end, wait_frames, TestHost,
    TestHostOptions,
};
use rsp_client::session::EndReason;
use rsp_core::errors::ErrorCode;
use rsp_core::pb;
use rsp_core::trust::Permission;

const CASE_TIMEOUT: Duration = Duration::from_secs(9);

async fn assert_connect_denied(code: ErrorCode) {
    let host = TestHost::start_with(TestHostOptions {
        fail: Some(code),
        ..TestHostOptions::default()
    })
    .await;
    let app = app_identity();
    host.pair(&app, Permission::Control);
    let (session, mut events) = open(direct_params(&host, app)).await.expect("open session");
    let (observed, _, _) = wait_end(&mut events, Duration::from_secs(3))
        .await
        .expect("forced denial ends the session");
    assert_eq!(observed, Some(code), "expected {code:?}");
    session.close();
    host.stop().await;
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn every_configured_code_is_returned_to_connect_clients() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        for code in [
            ErrorCode::SessionFull,
            ErrorCode::HostTooOld,
            ErrorCode::DeviceRevoked,
            ErrorCode::LoginWindow,
        ] {
            assert_connect_denied(code).await;
        }
    })
    .await
    .expect("forced-denial cases complete within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn without_fail_a_paired_client_streams() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start_with(TestHostOptions::default()).await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let (session, mut events) = open(direct_params(&host, app))
            .await
            .expect("open paired session");
        let report = wait_frames(&mut events, 1, Duration::from_secs(3)).await;
        assert!(report.frames >= 1, "{report:?}");
        assert!(report.errors.is_empty(), "{report:?}");
        session.close();
        host.stop().await;
    })
    .await
    .expect("control case completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_intent_is_denied_with_the_configured_code() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start_with(TestHostOptions {
            fail: Some(ErrorCode::SessionFull),
            ..TestHostOptions::default()
        })
        .await;
        let ticket = host.new_pairing_code();
        let results = pair_with_code(&host, app_identity(), &ticket.code).await;
        assert!(
            results.is_empty(),
            "forced denial happens before the pairing state machine: {results:?}"
        );

        let mut params = direct_params(&host, app_identity()).with_pairing_code(ticket.code);
        params.intent = pb::Intent::Pair;
        let (session, mut events) = open(params).await.expect("open pairing session");
        let (code, kind, message) = wait_end(&mut events, Duration::from_secs(3))
            .await
            .expect("forced pairing denial ends the session");
        assert_eq!(code, Some(ErrorCode::SessionFull), "{message}");
        assert_eq!(
            kind,
            EndReason::Error(ErrorCode::SessionFull),
            "pairing denial is reported through AuthResult, not PairResult"
        );
        session.close();
        host.stop().await;
    })
    .await
    .expect("forced pairing denial completes within 9 seconds");
}
