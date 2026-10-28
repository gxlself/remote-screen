//! Takeover arbitration through real loopback QUIC and the local approval IPC.
use host_core::{
    auth::Role,
    ipc::Event,
    session::{handlers::roster, takeover::Seat},
};
use rsp_core::{pb, trust::Permission};
use serde_json::{json, Value};
use std::time::Duration;
use tokio::time::timeout;

mod wire_support;
use wire_support::{client_hello, device_id, identity, Dev, WireClient, WireHost, WireOptions};

const ALICE: &str = "alice@example.com";
const BOB: &str = "bob@example.com";

fn options(users: Option<Vec<String>>) -> WireOptions {
    let mut options = WireOptions {
        tailnet_users: users,
        ..Default::default()
    };
    options.config.security.confirm_new_devices = false;
    options
}

async fn admit(host: &WireHost, device: Dev, user: Option<&str>, role: pb::Role) -> WireClient {
    let mut client = match user {
        Some(user) => host.connect_user(device, user).await,
        None => host.connect_as(device, 0).await,
    };
    let (_, auth) = client
        .hello_with(pb::ClientHello {
            intent: if role == pb::Role::Viewer {
                pb::Intent::View
            } else {
                pb::Intent::Control
            } as i32,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, role as i32);
    client
}

async fn participants(same_user: bool) -> (WireHost, WireClient, WireClient) {
    let users = if same_user {
        vec![ALICE.to_owned()]
    } else {
        vec![ALICE.to_owned(), BOB.to_owned()]
    };
    let host = WireHost::start_with(options(Some(users))).await;
    let a = admit(&host, Dev::A, Some(ALICE), pb::Role::Controller).await;
    let b = admit(
        &host,
        Dev::B,
        Some(if same_user { ALICE } else { BOB }),
        pb::Role::Viewer,
    )
    .await;
    (host, a, b)
}

fn requested(host: &WireHost) -> Vec<Value> {
    host.events
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ApprovalRequested(payload) if payload["kind"] == "takeover" => Some(payload),
            _ => None,
        })
        .collect()
}

async fn prompt(host: &WireHost, user: Option<&str>) -> Value {
    host.wait_until(|| !requested(host).is_empty()).await;
    let request = requested(host).pop().unwrap();
    assert_eq!(request["kind"], "takeover");
    assert_eq!(request["device_id"], device_id(Dev::B));
    assert_eq!(request["device_name"], client_hello().device_name);
    assert_eq!(request["platform"], "ios");
    assert_eq!(request["whois_login"], json!(user));
    assert_eq!(request["path"], Value::Null);
    assert_eq!(request["reasons"], json!([]));
    assert_eq!(request["timeout_s"], 30);
    assert_eq!(request["permission_choice"], false);
    assert_eq!(
        request["device_fp"],
        rsp_core::ids::fp_b64url(&identity(Dev::B).fingerprint())
    );
    assert_eq!(
        host.mgr.session_control().approval_pending(),
        vec![request.clone()]
    );
    assert_eq!(
        host.mgr
            .features()
            .takeover
            .prompt_id
            .lock()
            .unwrap()
            .as_deref(),
        request["request_id"].as_str()
    );
    request
}

async fn begin(host: &WireHost, a: &mut WireClient, b: &mut WireClient) -> Value {
    b.send(pb::RequestControl {}).await;
    let control = a
        .next_matching(|msg| match msg {
            pb::envelope::Msg::ControlRequest(request) => Some(request.clone()),
            _ => None,
        })
        .await;
    let requester = control.requester.unwrap();
    assert_eq!(requester.device_id, device_id(Dev::B));
    assert_eq!(requester.device_name, client_hello().device_name);
    assert_eq!(requester.user, BOB);
    assert_eq!(control.timeout_s, 30);
    let request = prompt(host, Some(BOB)).await;
    // The wire identifies the requester by device_id; IPC uses its own request_id.
    assert_eq!(request["device_id"], requester.device_id);
    request
}

async fn respond_controller(a: &mut WireClient, allow: bool) {
    a.send(pb::ControlResponse {
        requester_device_id: device_id(Dev::B),
        allow,
    })
    .await;
}

fn respond_local(host: &WireHost, request: &Value, allow: bool) {
    assert!(host.mgr.session_control().approval_respond(
        request["request_id"].as_str().unwrap(),
        allow,
        None,
    ));
}

async fn closed(host: &WireHost, request: &Value, outcome: &str) {
    host.wait_until(|| {
        host.events.events().iter().any(|event| {
            matches!(event, Event::ApprovalClosed(payload) if payload == &json!({
                "request_id": request["request_id"],
                "kind": "takeover",
                "outcome": outcome,
            }))
        })
    })
    .await;
    assert!(host.mgr.session_control().approval_pending().is_empty());
}

async fn role(client: &mut WireClient, role: pb::Role) -> pb::SessionState {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::ControlRequest(_) => panic!("unexpected ControlRequest"),
            pb::envelope::Msg::SessionState(state)
                if state.your_role == role as i32
                    && state
                        .controller
                        .as_ref()
                        .is_some_and(|p| p.device_id == device_id(Dev::B)) =>
            {
                Some(state.clone())
            }
            _ => None,
        })
        .await
}

async fn handoff(host: &WireHost, reason: &str, from: Option<Dev>) {
    host.wait_until(|| {
        host.events.events().iter().any(|event| {
            matches!(event, Event::Handoff(payload)
                if payload["to"]["device_id"] == device_id(Dev::B)
                && payload["reason"] == reason
                && payload["from"]["device_id"] == json!(from.map(device_id)))
        })
    })
    .await;
}

async fn error(client: &mut WireClient, reason: &str) {
    let error = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Error(error) => Some(error.clone()),
            _ => None,
        })
        .await;
    assert_eq!(error.code, 141);
    assert_eq!(error.detail.get("reason").map(String::as_str), Some(reason));
}

fn a_still_controls(host: &WireHost) {
    assert!(
        matches!(roster::seat(&host.mgr), Seat::Occupied { controller }
        if controller.device_id == device_id(Dev::A))
    );
    assert!(host.conns().iter().all(|conn| {
        conn.role
            == if conn.device_id == device_id(Dev::A) {
                Role::Controller
            } else {
                Role::Viewer
            }
    }));
    assert!(!host
        .events
        .events()
        .iter()
        .any(|e| matches!(e, Event::Handoff(_))));
}

#[tokio::test]
async fn empty_seat_grants_control_and_emits_handoff() {
    let host = WireHost::start_with(options(None)).await;
    let mut b = admit(&host, Dev::B, None, pb::Role::Viewer).await;
    assert!(matches!(roster::seat(&host.mgr), Seat::Empty { .. }));
    b.send(pb::RequestControl {}).await;
    role(&mut b, pb::Role::Controller).await;
    handoff(&host, "seat_empty", None).await;
    assert!(requested(&host).is_empty());
}

#[tokio::test]
async fn different_user_controller_allows_and_cancels_local_prompt() {
    let (host, mut a, mut b) = participants(false).await;
    let request = begin(&host, &mut a, &mut b).await;
    respond_controller(&mut a, true).await;
    let a_state = role(&mut a, pb::Role::Viewer).await;
    let b_state = role(&mut b, pb::Role::Controller).await;
    assert_eq!(a_state.controller, b_state.controller);
    assert_eq!(a_state.viewers, b_state.viewers);
    assert!(a_state
        .viewers
        .iter()
        .any(|p| p.device_id == device_id(Dev::A)));
    handoff(&host, "controller_allowed", Some(Dev::A)).await;
    closed(&host, &request, "cancelled").await;
    assert!(!host.mgr.session_control().approval_respond(
        request["request_id"].as_str().unwrap(),
        true,
        None,
    ));
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn immediate_controller_response_cannot_leave_a_stale_prompt() {
    for _ in 0..8 {
        let (host, mut a, mut b) = participants(false).await;
        let controller = async {
            a.next_matching(|msg| {
                matches!(msg, pb::envelope::Msg::ControlRequest(_)).then_some(())
            })
            .await;
            respond_controller(&mut a, true).await;
            role(&mut a, pb::Role::Viewer).await;
        };
        let requester = async {
            b.send(pb::RequestControl {}).await;
            role(&mut b, pb::Role::Controller).await;
        };
        tokio::join!(controller, requester);
        handoff(&host, "controller_allowed", Some(Dev::A)).await;
        host.wait_until(|| {
            host.mgr.session_control().approval_pending().is_empty()
                && host
                    .mgr
                    .features()
                    .takeover
                    .prompt_id
                    .lock()
                    .unwrap()
                    .is_none()
        })
        .await;
        for request in requested(&host) {
            closed(&host, &request, "cancelled").await;
        }
    }
}

#[tokio::test]
async fn controller_denies_with_error_141_and_keeps_roles() {
    let (host, mut a, mut b) = participants(false).await;
    let request = begin(&host, &mut a, &mut b).await;
    respond_controller(&mut a, false).await;
    error(&mut b, "denied").await;
    closed(&host, &request, "cancelled").await;
    a_still_controls(&host);
}

#[tokio::test]
async fn local_allow_hands_off_without_controller_response() {
    let (host, mut a, mut b) = participants(false).await;
    let request = begin(&host, &mut a, &mut b).await;
    respond_local(&host, &request, true);
    role(&mut a, pb::Role::Viewer).await;
    role(&mut b, pb::Role::Controller).await;
    handoff(&host, "local_allowed", Some(Dev::A)).await;
    closed(&host, &request, "allowed").await;
    assert!(host
        .mgr
        .features()
        .takeover
        .prompt_id
        .lock()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn timeout_uses_host_clock_and_closes_local_prompt() {
    let (host, mut a, mut b) = participants(false).await;
    let request = begin(&host, &mut a, &mut b).await;
    host.clock.skip(Duration::from_secs(31));
    timeout(Duration::from_millis(1500), error(&mut b, "timeout"))
        .await
        .expect("takeover timeout must observe an offset clock jump within 1.5 s");
    closed(&host, &request, "cancelled").await;
    a_still_controls(&host);
}

#[tokio::test]
async fn same_online_user_hands_off_without_either_prompt() {
    let (host, mut a, mut b) = participants(true).await;
    b.send(pb::RequestControl {}).await;
    timeout(Duration::from_secs(1), role(&mut b, pb::Role::Controller))
        .await
        .unwrap();
    role(&mut a, pb::Role::Viewer).await;
    a.expect_none_matching(Duration::from_millis(100), |msg| {
        matches!(msg, pb::envelope::Msg::ControlRequest(_))
    })
    .await;
    handoff(&host, "same_user", Some(Dev::A)).await;
    assert!(requested(&host).is_empty());
}

#[tokio::test]
async fn requester_disconnect_cancels_prompt_without_handoff() {
    let (host, mut a, mut b) = participants(false).await;
    let request = begin(&host, &mut a, &mut b).await;
    b.conn.close(0u32.into(), b"requester left");
    closed(&host, &request, "cancelled").await;
    host.wait_until(|| host.conns().len() == 1).await;
    a_still_controls(&host);
    assert!(host
        .mgr
        .features()
        .takeover
        .prompt_id
        .lock()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn view_only_device_cannot_request_control() {
    let host = WireHost::start_with(options(None)).await;
    let _a = admit(&host, Dev::A, None, pb::Role::Controller).await;
    let mut device = host
        .store
        .get(&identity(Dev::B).fingerprint())
        .unwrap()
        .unwrap();
    device.permission = Permission::ViewOnly;
    host.store.upsert(device).unwrap();
    let mut b = admit(&host, Dev::B, None, pb::Role::Viewer).await;
    b.send(pb::RequestControl {}).await;
    error(&mut b, "view_only").await;
    a_still_controls(&host);
    assert!(requested(&host).is_empty());
}

#[tokio::test]
async fn same_user_during_linger_needs_local_confirmation() {
    let (host, mut a, mut b) = participants(true).await;
    a.goodbye(pb::goodbye::Reason::Background).await;
    assert_eq!(a.close_code().await, 0);
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    b.send(pb::RequestControl {}).await;
    let request = prompt(&host, Some(ALICE)).await;
    b.expect_none_matching(Duration::from_millis(100), |msg| {
        matches!(msg, pb::envelope::Msg::SessionState(s)
            if s.your_role == pb::Role::Controller as i32)
            || matches!(msg, pb::envelope::Msg::ControlRequest(_))
    })
    .await;
    assert!(host.conns().iter().all(|c| c.role == Role::Viewer));
    assert!(matches!(roster::seat(&host.mgr), Seat::Lingering { .. }));
    respond_local(&host, &request, true);
    role(&mut b, pb::Role::Controller).await;
    handoff(&host, "local_allowed", None).await;
    closed(&host, &request, "allowed").await;
}

#[tokio::test]
async fn same_user_after_linger_expires_takes_control_immediately() {
    let (host, mut a, mut b) = participants(true).await;
    a.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    host.clock.skip(Duration::from_secs(61));
    host.wait_until(|| {
        // IPC lists only active connections, so also wait for the reserved
        // roster entry to expire before requesting the now-empty seat.
        matches!(roster::seat(&host.mgr), Seat::Empty { .. })
            && host
                .mgr
                .session_control()
                .sessions_list()
                .iter()
                .all(|s| s.device_id != device_id(Dev::A))
    })
    .await;
    assert!(
        matches!(roster::seat(&host.mgr), Seat::Empty { former_user }
        if former_user.as_deref() == Some(ALICE))
    );
    b.send(pb::RequestControl {}).await;
    timeout(Duration::from_secs(1), role(&mut b, pb::Role::Controller))
        .await
        .unwrap();
    handoff(&host, "seat_empty", None).await;
    assert!(requested(&host).is_empty());
}

#[tokio::test]
async fn local_deny_keeps_roles_and_a_late_controller_allow_is_ignored() {
    let (host, mut a, mut b) = participants(false).await;
    let request = begin(&host, &mut a, &mut b).await;
    respond_local(&host, &request, false);
    error(&mut b, "denied").await;
    closed(&host, &request, "denied").await;
    respond_controller(&mut a, true).await;
    a.send(pb::Ping { t1_us: 42 }).await;
    a.next_matching(|msg| match msg {
        pb::envelope::Msg::Pong(pong) if pong.t1_us == 42 => Some(()),
        _ => None,
    })
    .await;
    a_still_controls(&host);
    assert!(host
        .mgr
        .features()
        .takeover
        .prompt_id
        .lock()
        .unwrap()
        .is_none());
}

#[tokio::test]
async fn wrong_responder_or_device_cannot_allow_and_other_requesters_are_busy() {
    let (host, mut a, mut b) = participants(false).await;
    let mut c = admit(&host, Dev::C, Some(BOB), pb::Role::Viewer).await;
    let request = begin(&host, &mut a, &mut b).await;
    respond_controller(&mut c, true).await;
    c.send(pb::RequestControl {}).await;
    error(&mut c, "busy").await;
    a.send(pb::ControlResponse {
        requester_device_id: device_id(Dev::C),
        allow: true,
    })
    .await;
    a.send(pb::Ping { t1_us: 43 }).await;
    a.next_matching(|msg| match msg {
        pb::envelope::Msg::Pong(pong) if pong.t1_us == 43 => Some(()),
        _ => None,
    })
    .await;
    a_still_controls(&host);
    assert_eq!(
        host.mgr.session_control().approval_pending(),
        vec![request.clone()]
    );
    respond_controller(&mut a, true).await;
    role(&mut b, pb::Role::Controller).await;
    role(&mut c, pb::Role::Viewer).await;
    closed(&host, &request, "cancelled").await;
}

#[tokio::test]
async fn attended_policy_changes_apply_to_empty_seat_requests() {
    let host = WireHost::start_with(options(None)).await;
    let mut b = admit(&host, Dev::B, None, pb::Role::Viewer).await;
    host.config
        .update(&json!({"access": {"attended_mode": true}}))
        .unwrap();
    // Let the config subscriber process the change before the wire request.
    host.wait_until(|| {
        let mut state = host.mgr.features().takeover.state.lock().unwrap().clone();
        let requester = roster::candidate(&host.mgr, host.conns()[0].key).unwrap();
        !state
            .on_request(
                requester,
                &Seat::Empty { former_user: None },
                host.mgr.now_us(),
            )
            .iter()
            .any(|a| {
                matches!(
                    a,
                    host_core::session::takeover::TakeoverAction::Grant { .. }
                )
            })
    })
    .await;
    b.send(pb::RequestControl {}).await;
    let request = prompt(&host, None).await;
    respond_local(&host, &request, false);
    error(&mut b, "denied").await;
    host.config
        .update(&json!({"access": {"attended_mode": false}}))
        .unwrap();
    host.wait_until(|| {
        let mut state = host.mgr.features().takeover.state.lock().unwrap().clone();
        let requester = roster::candidate(&host.mgr, host.conns()[0].key).unwrap();
        state
            .on_request(
                requester,
                &Seat::Empty { former_user: None },
                host.mgr.now_us(),
            )
            .iter()
            .any(|a| {
                matches!(
                    a,
                    host_core::session::takeover::TakeoverAction::Grant { .. }
                )
            })
    })
    .await;
    b.send(pb::RequestControl {}).await;
    role(&mut b, pb::Role::Controller).await;
    assert_eq!(requested(&host).len(), 1);
}
