//! Authentication policy is rebuilt from configuration for every handshake.

use host_core::auth_config::auth_policy_from_config;
use rsp_core::{errors::ErrorCode, pb};
use serde_json::{json, Value};

mod wire_support;
use wire_support::{client_hello, device_id, Dev, WireClient, WireHost, WireOptions};

const ALICE: &str = "alice@example.com";

fn live_options() -> WireOptions {
    WireOptions {
        options: host_core::RuntimeOptions {
            live_auth_policy: true,
            ..host_core::RuntimeOptions::default()
        },
        ..WireOptions::default()
    }
}

async fn auth_status(client: &mut WireClient, intent: pb::Intent) -> pb::AuthResult {
    client
        .hello_with(pb::ClientHello {
            intent: intent as i32,
            ..client_hello()
        })
        .await
        .1
}

fn approval_requests(host: &WireHost) -> Vec<Value> {
    host.events
        .events()
        .into_iter()
        .filter_map(|event| match event {
            host_core::ipc::Event::ApprovalRequested(payload) => Some(payload),
            _ => None,
        })
        .collect()
}

async fn pending_reasons(host: &WireHost, device: Dev, intent: pb::Intent) -> (WireClient, Value) {
    let before = approval_requests(host).len();
    let mut client = host.connect_as(device, 0).await;
    let auth = auth_status(&mut client, intent).await;
    assert_eq!(auth.status, pb::auth_result::Status::PendingApproval as i32);
    assert_eq!(auth.approval_timeout_s, 30);
    host.wait_until(|| approval_requests(host).len() == before + 1)
        .await;
    let request = approval_requests(host).pop().unwrap();
    assert_eq!(request["device_id"], device_id(device));
    assert_eq!(request["kind"], "connection");
    assert_eq!(request["path"], "lan");
    assert_eq!(
        host.mgr.session_control().approval_pending(),
        vec![request.clone()]
    );
    (client, request)
}

async fn ping(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 42 }).await;
    client
        .next_matching(|message| match message {
            pb::envelope::Msg::Pong(pong) if pong.t1_us == 42 => Some(()),
            _ => None,
        })
        .await;
}

#[tokio::test]
async fn live_remote_control_switch_applies_to_control_and_view() {
    let host = WireHost::start_with(live_options()).await;
    let mut controller = host.connect().await;
    let initial = auth_status(&mut controller, pb::Intent::Control).await;
    assert_eq!(initial.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(initial.role, pb::Role::Controller as i32);

    host.config
        .update(&json!({"access": {"allow_remote_control": false}}))
        .unwrap();
    for intent in [pb::Intent::Control, pb::Intent::View] {
        let mut client = host.connect_as(Dev::B, 0).await;
        let auth = auth_status(&mut client, intent).await;
        assert_eq!(auth.status, pb::auth_result::Status::Denied as i32);
        assert_eq!(auth.error.unwrap().code, 132);
    }
    // Configuration changes apply to new handshakes; the admitted session stays active.
    ping(&mut controller).await;

    host.config
        .update(&json!({"access": {"allow_remote_control": true}}))
        .unwrap();
    for (device, intent) in [(Dev::C, pb::Intent::Control), (Dev::D, pb::Intent::View)] {
        let mut client = host.connect_as(device, 0).await;
        let auth = auth_status(&mut client, intent).await;
        assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    }
}

#[tokio::test]
async fn live_allowed_users_change_applies_to_tailnet_handshakes() {
    let mut options = live_options();
    options.tailnet_users = Some(vec![ALICE.into()]);
    options.config.access.allowed_users = vec![ALICE.into()];
    let host = WireHost::start_with(options).await;

    let mut allowed = host.connect_user(Dev::A, ALICE).await;
    let auth = auth_status(&mut allowed, pb::Intent::Control).await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, pb::Role::Controller as i32);

    host.config
        .update(&json!({"access": {"allowed_users": []}}))
        .unwrap();
    let mut removed = host.connect_user(Dev::B, ALICE).await;
    let auth = auth_status(&mut removed, pb::Intent::Control).await;
    assert_eq!(auth.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(auth.error.unwrap().code, 130);
    ping(&mut allowed).await;

    host.config
        .update(&json!({"access": {"allowed_users": [ALICE]}}))
        .unwrap();
    let mut restored = host.connect_user(Dev::C, ALICE).await;
    let auth = auth_status(&mut restored, pb::Intent::Control).await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
}

#[tokio::test]
async fn live_attended_mode_change_requires_approval() {
    let host = WireHost::start_with(live_options()).await;
    let mut controller = host.connect().await;
    assert_eq!(
        auth_status(&mut controller, pb::Intent::Control)
            .await
            .status,
        pb::auth_result::Status::Ok as i32
    );
    host.config
        .update(&json!({"access": {"attended_mode": true}}))
        .unwrap();

    let (_pending, request) = pending_reasons(&host, Dev::B, pb::Intent::View).await;
    assert!(request["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "attended_mode"));
}

#[tokio::test]
async fn live_intranet_confirmation_change_requires_approval() {
    let host = WireHost::start_with(live_options()).await;
    let mut controller = host.connect().await;
    assert_eq!(
        auth_status(&mut controller, pb::Intent::Control)
            .await
            .status,
        pb::auth_result::Status::Ok as i32
    );
    host.config
        .update(&json!({"intranet": {"confirm_each": true}}))
        .unwrap();

    let (_pending, request) = pending_reasons(&host, Dev::B, pb::Intent::View).await;
    assert!(request["reasons"]
        .as_array()
        .unwrap()
        .iter()
        .any(|reason| reason == "intranet_confirmation"));
}

#[tokio::test]
async fn disabled_live_policy_keeps_startup_policy_for_other_fields() {
    let mut options = WireOptions::default();
    options.config.access.attended_mode = false;
    options.options.auth_policy.attended_mode = false;
    options.options.live_auth_policy = false;
    let host = WireHost::start_with(options).await;
    assert!(!host_core::RuntimeOptions::default().live_auth_policy);
    let mut controller = host.connect().await;
    let auth = auth_status(&mut controller, pb::Intent::Control).await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, pb::Role::Controller as i32);
    host.config
        .update(&json!({
            "access": {
                "allow_remote_control": false,
                "attended_mode": true
            },
            "intranet": {"confirm_each": true}
        }))
        .unwrap();

    let mut client = host.connect_as(Dev::B, 0).await;
    let auth = auth_status(&mut client, pb::Intent::View).await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    let mut client = host.connect_as(Dev::C, 0).await;
    let auth = auth_status(&mut client, pb::Intent::Control).await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert!(approval_requests(&host).is_empty());
}

#[tokio::test]
async fn forced_denial_survives_live_policy_rebuild() {
    let mut options = live_options();
    options.options.auth_policy.forced_denial = Some(ErrorCode::PolicyDenied);
    let host = WireHost::start_with(options).await;
    host.config
        .update(&json!({"access": {"allow_remote_control": false}}))
        .unwrap();
    let mut client = host.connect().await;
    let auth = auth_status(&mut client, pb::Intent::Control).await;
    assert_eq!(auth.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(auth.error.unwrap().code, 130);
}

#[test]
fn policy_mapping_used_by_runtime_matches_current_configuration() {
    let mut config = host_core::config::HostConfig::default();
    config.access.allowed_users = vec![ALICE.into()];
    config.access.max_viewers = 2;
    config.access.allow_remote_control = false;
    config.access.attended_mode = true;
    config.intranet.confirm_each = true;
    config.intranet.allow_pairing = true;
    let policy = auth_policy_from_config(&config);
    assert_eq!(policy.allowed_users, vec![ALICE]);
    assert_eq!(policy.max_viewers, 2);
    assert!(!policy.allow_remote_control);
    assert!(policy.attended_mode);
    assert!(policy.intranet_confirm_each);
    assert!(policy.allow_intranet_pairing);
    assert_eq!(policy.forced_denial, None);
}
