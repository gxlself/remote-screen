use async_trait::async_trait;
use host_core::{
    auth::ApprovalReason,
    ipc,
    session::{
        handlers::activity,
        handshake::{ConnectionApprovalRequest, ConnectionApprover},
    },
};
use rsp_core::{hid::InputMsg, pb, trust::Permission};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};
mod wire_support;
use wire_support::{client_hello, write_record, Dev, WireClient, WireHost, WireOptions};

async fn hello(client: &mut WireClient, intent: pb::Intent) -> pb::AuthResult {
    client
        .hello_with(pb::ClientHello {
            intent: intent as i32,
            ..client_hello()
        })
        .await
        .1
}

async fn local_activity(client: &mut WireClient, expect_active: bool) -> bool {
    client
        .next_matching(|message| match message {
            pb::envelope::Msg::LocalActivity(activity) if activity.active == expect_active => {
                Some(activity.active)
            }
            _ => None,
        })
        .await
}

async fn session_paused(client: &mut WireClient, expect_paused: bool) -> pb::SessionState {
    client
        .next_matching(|message| match message {
            pb::envelope::Msg::SessionState(state) if state.input_paused_local == expect_paused => {
                Some(state.clone())
            }
            _ => None,
        })
        .await
}

async fn inject_input(input: &mut quinn::SendStream, message: InputMsg) {
    write_record(input, &message.encode().expect("input message encodes")).await;
}

#[tokio::test]
async fn local_input_pauses_then_resumes_remote_input() {
    let host = WireHost::start().await;
    let mut controller = host.connect().await;
    let auth = hello(&mut controller, pb::Intent::Control).await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    let mut input = controller.open_typed(0x01).await;
    let first = InputMsg::MouseMove { x: 0.1, y: 0.2 };
    inject_input(&mut input, first.clone()).await;
    host.wait_until(|| host.platform.injected_msgs() == [first.clone()])
        .await;

    let releases = host.platform.release_count();
    assert_eq!(host.platform.local_input(), 1);
    assert!(local_activity(&mut controller, true).await);
    assert!(
        session_paused(&mut controller, true)
            .await
            .input_paused_local
    );
    assert!(host.events.events().iter().any(|event| {
        matches!(event, ipc::Event::SessionsChanged(value)
            if value["capsule"]["input_paused_local"] == true)
    }));
    host.wait_until(|| host.platform.release_count() == releases + 1)
        .await;

    let blocked = InputMsg::MouseMove { x: 0.3, y: 0.4 };
    inject_input(&mut input, blocked).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(
        host.platform.injected_msgs().as_slice(),
        std::slice::from_ref(&first)
    );

    tokio::time::sleep(Duration::from_millis(3_300)).await;
    assert!(!local_activity(&mut controller, false).await);
    assert!(
        !session_paused(&mut controller, false)
            .await
            .input_paused_local
    );
    let resumed = InputMsg::MouseMove { x: 0.5, y: 0.6 };
    inject_input(&mut input, resumed.clone()).await;
    host.wait_until(|| host.platform.injected_msgs() == [first.clone(), resumed.clone()])
        .await;
}

#[tokio::test]
async fn viewers_receive_pause_and_new_admissions_get_a_snapshot() {
    let mut options = WireOptions::default();
    options.config.security.confirm_new_devices = false;
    let host = WireHost::start_with(options).await;
    let mut controller = host.connect().await;
    assert_eq!(
        hello(&mut controller, pb::Intent::Control).await.status,
        pb::auth_result::Status::Ok as i32
    );
    host.pair(Dev::B, Permission::ViewOnly);
    let mut viewer = host.connect_as(Dev::B, 0).await;
    assert_eq!(
        hello(&mut viewer, pb::Intent::View).await.status,
        pb::auth_result::Status::Ok as i32
    );
    session_paused(&mut viewer, false).await;

    host.platform.local_input();
    assert!(local_activity(&mut viewer, true).await);

    host.pair(Dev::C, Permission::ViewOnly);
    let mut newcomer = host.connect_as(Dev::C, 0).await;
    assert_eq!(
        hello(&mut newcomer, pb::Intent::View).await.status,
        pb::auth_result::Status::Ok as i32
    );
    assert!(local_activity(&mut newcomer, true).await);
}

#[tokio::test]
async fn disabling_pause_suppresses_messages_and_hot_reload_resumes() {
    let mut options = WireOptions::default();
    options.config.access.pause_on_local_input = false;
    let host = WireHost::start_with(options).await;
    let mut controller = host.connect().await;
    assert_eq!(
        hello(&mut controller, pb::Intent::Control).await.status,
        pb::auth_result::Status::Ok as i32
    );
    let mut input = controller.open_typed(0x01).await;
    host.platform.local_input();
    controller
        .expect_none_matching(Duration::from_millis(500), |message| {
            matches!(message, pb::envelope::Msg::LocalActivity(_))
        })
        .await;
    let event = InputMsg::MouseMove { x: 0.2, y: 0.3 };
    inject_input(&mut input, event.clone()).await;
    host.wait_until(|| host.platform.injected_msgs() == [event.clone()])
        .await;

    host.config
        .update(&json!({"access": {"pause_on_local_input": true}}))
        .expect("enable pause");
    // Keep pulsing until the manager has applied the hot update; an earlier
    // pulse only records activity because pause was still disabled.
    host.wait_until(|| {
        host.platform.local_input();
        activity::paused_watch(&host.mgr).is_some_and(|paused| *paused.borrow())
    })
    .await;
    assert!(local_activity(&mut controller, true).await);
    host.config
        .update(&json!({"access": {"pause_on_local_input": false}}))
        .expect("disable pause");
    assert!(!tokio::time::timeout(
        Duration::from_secs(1),
        local_activity(&mut controller, false)
    )
    .await
    .expect("hot reload resumes within one second"));
}

#[derive(Default)]
struct RecordingApprover {
    requests: Mutex<Vec<ConnectionApprovalRequest>>,
    count: AtomicUsize,
}

impl RecordingApprover {
    fn reasons(&self) -> Vec<ApprovalReason> {
        self.requests
            .lock()
            .expect("approval request records are not poisoned")
            .iter()
            .flat_map(|request| request.reasons.clone())
            .collect()
    }
}

#[async_trait]
impl ConnectionApprover for RecordingApprover {
    async fn request(&self, request: ConnectionApprovalRequest) -> bool {
        self.requests
            .lock()
            .expect("approval request records are not poisoned")
            .push(request);
        self.count.fetch_add(1, Ordering::Relaxed);
        true
    }
}

#[tokio::test]
async fn recent_local_activity_requires_approval_within_sixty_seconds() {
    let approver: Arc<RecordingApprover> = Arc::default();
    let host = WireHost::start_with(WireOptions {
        connection_approver: Some(approver.clone()),
        ..Default::default()
    })
    .await;
    host.platform.local_input();
    host.wait_until(|| activity::active_within_60s(&host.mgr))
        .await;

    let mut d = host.connect_as(Dev::D, 0).await;
    assert_eq!(
        hello(&mut d, pb::Intent::Control).await.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    d.next_matching(|message| match message {
        pb::envelope::Msg::AuthResult(auth)
            if auth.status == pb::auth_result::Status::Ok as i32 =>
        {
            Some(())
        }
        _ => None,
    })
    .await;
    host.wait_until(|| approver.count.load(Ordering::Relaxed) == 1)
        .await;
    assert!(approver.reasons().contains(&ApprovalReason::LocalActivity));

    host.clock.skip(Duration::from_secs(61));
    let mut e = host.connect_as(Dev::E, 0).await;
    assert_eq!(
        hello(&mut e, pb::Intent::Control).await.status,
        pb::auth_result::Status::Ok as i32
    );
    assert_eq!(approver.count.load(Ordering::Relaxed), 1);
}
