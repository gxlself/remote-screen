//! App-side authentication and pairing outcomes (IT-02, IT-03, IT-13, IT-15).

use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use host_fake::harness::*;
use rsp_client::session::{SessionEvent, SessionStateKind};
use rsp_core::{
    errors::ErrorCode,
    hid::{InputMsg, Mods},
    pairing::MAX_CODE_FAILURES,
    pb,
    trust::Permission,
};

const CASE_TIMEOUT: Duration = Duration::from_secs(9);

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn new_device_confirmation_distinguishes_local_and_unseen_pairings() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        for unseen in [false, true] {
            let host = TestHost::start().await;
            let app = app_identity();
            if unseen {
                host.pair_unseen(&app, Permission::Control);
            } else {
                host.pair(&app, Permission::Control);
            }
            let (session, mut events) = open(direct_params(&host, app))
                .await
                .expect("open paired session");
            let report = wait_frames(&mut events, 1, Duration::from_secs(3)).await;
            assert_eq!(report.frames, 1, "unseen={unseen}: {report:?}");
            assert!(report.errors.is_empty(), "unseen={unseen}: {report:?}");
            assert_eq!(
                report.states.contains(&SessionStateKind::PendingApproval),
                unseen,
                "unseen={unseen}: {report:?}"
            );
            session.close();
            host.stop().await;
        }
    })
    .await
    .expect("local and unseen pairing cases complete within 9 seconds");
}

fn status(result: &pb::PairResult) -> pb::pair_result::Status {
    pb::pair_result::Status::try_from(result.status).unwrap_or(pb::pair_result::Status::Unspecified)
}

fn statuses(results: &[pb::PairResult]) -> Vec<pb::pair_result::Status> {
    results.iter().map(status).collect()
}

/// Open a normal session and collect its terminal code, state transitions, and
/// whether any media frame was observed before it ended.
async fn open_until_end(
    host: &TestHost,
    identity: Arc<rsp_transport::identity::LocalIdentity>,
    intent: pb::Intent,
) -> (Option<ErrorCode>, Vec<SessionStateKind>, bool) {
    let mut params = direct_params(host, identity);
    params.intent = intent;
    let (session, mut events) = open(params).await.expect("open session");
    let mut states = Vec::new();
    let mut saw_frame = false;
    let ended = tokio::time::timeout(Duration::from_secs(3), async {
        while let Some(event) = events.recv().await {
            match event {
                SessionEvent::StateChanged(state) => states.push(state),
                SessionEvent::Frame(_) => saw_frame = true,
                SessionEvent::Ended { code, .. } => return code,
                _ => {}
            }
        }
        None
    })
    .await
    .ok()
    .flatten();
    session.close();
    (ended, states, saw_frame)
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn it02_unpaired_client_sees_181() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start().await;
        let (code, _, saw_frame) = open_until_end(&host, app_identity(), pb::Intent::Control).await;
        assert_eq!(code, Some(ErrorCode::NotPaired));
        assert!(!saw_frame, "an unpaired device receives no media");
        host.stop().await;
    })
    .await
    .expect("IT-02 completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn it03_revoked_client_sees_180() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start().await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        host.revoke(app.fingerprint());
        let (code, _, saw_frame) = open_until_end(&host, app, pb::Intent::Control).await;
        assert_eq!(code, Some(ErrorCode::DeviceRevoked));
        assert!(!saw_frame, "revoked devices receive no media");
        host.stop().await;
    })
    .await
    .expect("IT-03 completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn remote_control_disabled_is_132() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start_with(TestHostOptions {
            allow_remote_control: Some(false),
            ..TestHostOptions::default()
        })
        .await;
        let app = app_identity();
        host.pair(&app, Permission::Control);

        let (control_code, _, control_frame) =
            open_until_end(&host, app.clone(), pb::Intent::Control).await;
        assert_eq!(control_code, Some(ErrorCode::RemoteControlDisabled));
        assert!(!control_frame, "a denied controller receives no media");

        // The current Host applies the global switch before role selection, so
        // VIEW is denied by the same 132 rather than admitted as a viewer.
        let (view_code, _, view_frame) = open_until_end(&host, app, pb::Intent::View).await;
        println!("allow_remote_control=false VIEW result: {view_code:?}");
        assert_eq!(view_code, Some(ErrorCode::RemoteControlDisabled));
        assert!(!view_frame, "a denied viewer receives no media");
        host.stop().await;
    })
    .await
    .expect("remote-control-disabled case completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attended_deny_is_140() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start_with(TestHostOptions {
            attended: Some(Approval::Deny),
            ..TestHostOptions::default()
        })
        .await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let (code, states, saw_frame) = open_until_end(&host, app, pb::Intent::Control).await;
        assert_eq!(code, Some(ErrorCode::ApprovalDenied));
        assert!(!saw_frame);
        let pending = states
            .iter()
            .position(|state| *state == SessionStateKind::PendingApproval)
            .expect("PendingApproval is emitted");
        assert!(
            states[pending + 1..].contains(&SessionStateKind::Ended),
            "PendingApproval precedes Ended: {states:?}"
        );
        host.stop().await;
    })
    .await
    .expect("attended-deny case completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn attended_allow_streams() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start_with(TestHostOptions {
            attended: Some(Approval::Allow),
            ..TestHostOptions::default()
        })
        .await;
        let app = app_identity();
        host.pair(&app, Permission::Control);
        let (session, mut events) = open(direct_params(&host, app))
            .await
            .expect("open attended session");
        assert!(
            wait_state(
                &mut events,
                SessionStateKind::PendingApproval,
                Duration::from_secs(2)
            )
            .await,
            "attended connection pauses for approval"
        );
        let report = wait_frames(&mut events, 1, Duration::from_secs(3)).await;
        assert_eq!(report.frames, 1, "{report:?}");
        assert!(report.errors.is_empty(), "{report:?}");
        assert!(
            report.states.contains(&SessionStateKind::Streaming),
            "approval transitions into Streaming: {:?}",
            report.states
        );
        session.close();
        host.stop().await;
    })
    .await
    .expect("attended-allow case completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn view_only_permission_gets_viewer() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let platform = host_fake::FakePlatform::new();
        let host =
            TestHost::start_with_platform(platform.clone(), TestHostOptions::default()).await;
        let app = app_identity();
        host.pair(&app, Permission::ViewOnly);
        let (session, mut events) = open(direct_params(&host, app))
            .await
            .expect("open view-only session");
        let mut role = None;
        let mut session_state = None;
        let mut saw_frame = false;
        tokio::time::timeout(Duration::from_secs(3), async {
            while let Some(event) = events.recv().await {
                match event {
                    SessionEvent::Role(observed) => role = Some(observed),
                    SessionEvent::SessionState(state) => session_state = Some(state),
                    SessionEvent::Frame(_) => {
                        saw_frame = true;
                        break;
                    }
                    SessionEvent::Ended { .. } => break,
                    _ => {}
                }
            }
        })
        .await
        .expect("view-only session becomes ready");
        assert!(saw_frame, "a view-only session receives frames");
        assert_eq!(role, Some(pb::Role::Viewer), "auth role is viewer");
        if let Some(state) = session_state {
            assert_eq!(state.your_role, pb::Role::Viewer as i32);
        }

        session
            .send_input(InputMsg::Key {
                usage: 0x04,
                down: true,
                mods: Mods::NONE,
            })
            .expect("send key");
        tokio::time::sleep(Duration::from_secs(1)).await;
        assert!(
            platform.injected_events().is_empty(),
            "viewer input is not injected: {:?}",
            platform.injected_events()
        );
        session.close();
        host.stop().await;
    })
    .await
    .expect("view-only case completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn it15_five_bad_codes_lock_the_code() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start().await;
        let ticket = host.new_pairing_code();
        let wrong = format!(
            "{:06}",
            (ticket.code.parse::<u32>().unwrap_or(0) + 1) % 1_000_000
        );
        let mut observed = Vec::new();
        for attempt in 1..=MAX_CODE_FAILURES {
            let results = pair_with_code(&host, app_identity(), &wrong).await;
            let result = results.last().expect("pair result");
            let remaining = MAX_CODE_FAILURES - attempt;
            println!(
                "IT-15 attempt {attempt}: status={:?} attempts_left={}",
                status(result),
                result.attempts_left
            );
            observed.push((status(result), result.attempts_left));
            assert_eq!(result.attempts_left, remaining, "{results:?}");
            assert_eq!(
                status(result),
                if remaining == 0 {
                    pb::pair_result::Status::Expired
                } else {
                    pb::pair_result::Status::BadCode
                },
                "{results:?}"
            );
        }
        assert_eq!(
            observed.iter().map(|entry| entry.1).collect::<Vec<_>>(),
            vec![4, 3, 2, 1, 0]
        );

        let correct = pair_with_code(&host, app_identity(), &ticket.code).await;
        println!(
            "IT-15 correct code after exhaustion: {:?}",
            statuses(&correct)
        );
        let last = correct.last().expect("post-exhaustion result");
        assert_ne!(status(last), pb::pair_result::Status::Ok);
        assert_eq!(status(last), pb::pair_result::Status::Expired);
        host.stop().await;
    })
    .await
    .expect("IT-15 completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn expired_code() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let now = Arc::new(AtomicU64::new(1_000_000));
        let clock = Arc::clone(&now);
        let host = TestHost::start_with(TestHostOptions {
            pairing_clock: Some(Arc::new(move || clock.load(Ordering::Relaxed))),
            ..TestHostOptions::default()
        })
        .await;
        let ticket = host.new_pairing_code();
        assert_eq!(
            ticket.expires_at,
            now.load(Ordering::Relaxed) + host_core::pairing::PAIRING_CODE_LIFETIME_US
        );
        now.store(ticket.expires_at + 1_000_000, Ordering::Relaxed);
        let results = pair_with_code(&host, app_identity(), &ticket.code).await;
        println!("expired-code result: {:?}", statuses(&results));
        assert_eq!(
            status(results.last().expect("pair result")),
            pb::pair_result::Status::Expired
        );
        host.stop().await;
    })
    .await
    .expect("expired-code case completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_denied_locally() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start_with(TestHostOptions {
            pairing_approval: Some(Approval::Deny),
            ..TestHostOptions::default()
        })
        .await;
        let app = app_identity();
        let ticket = host.new_pairing_code();
        let results = pair_with_code(&host, app.clone(), &ticket.code).await;
        let observed = statuses(&results);
        println!("local pairing denial: {observed:?}");
        assert!(
            matches!(
                observed.as_slice(),
                [
                    pb::pair_result::Status::WaitingLocal,
                    pb::pair_result::Status::Denied
                ] | [pb::pair_result::Status::Denied]
            ),
            "unexpected local pairing sequence: {observed:?}"
        );
        assert_eq!(
            observed.last(),
            Some(&pb::pair_result::Status::Denied),
            "{observed:?}"
        );

        let (code, _, saw_frame) = open_until_end(&host, app, pb::Intent::Control).await;
        assert_eq!(code, Some(ErrorCode::NotPaired));
        assert!(!saw_frame);
        host.stop().await;
    })
    .await
    .expect("local-pairing-denial case completes within 9 seconds");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn pairing_without_active_code() {
    tokio::time::timeout(CASE_TIMEOUT, async {
        let host = TestHost::start().await;
        let results = pair_with_code(&host, app_identity(), "123456").await;
        let observed = statuses(&results);
        println!("pairing without an active code: {observed:?}");
        let last = results.last().expect("pair result");
        assert_ne!(status(last), pb::pair_result::Status::Ok);
        assert_eq!(status(last), pb::pair_result::Status::Expired);
        host.stop().await;
    })
    .await
    .expect("pairing-without-code case completes within 9 seconds");
}
