use host_core::{
    auth::{Role, SessionSnapshot},
    ipc::Event,
    session::{
        handlers::{self, approval, roster},
        handshake::ConnectionApprovalRequest,
    },
};
use rsp_core::{
    clipboard::{encode_stream, ClipboardItem, ClipboardKind},
    errors::ErrorCode,
    features,
    hid::InputMsg,
    pb,
    trust::Permission,
};
use std::{sync::atomic::Ordering, time::Duration};

mod wire_support;
use wire_support::{client_hello, device_id, write_record, Dev, WireClient, WireHost, WireOptions};

const QUIET: Duration = Duration::from_millis(200);

fn files_hello() -> pb::ClientHello {
    pb::ClientHello {
        intent: pb::Intent::Files as i32,
        features: client_hello().features | features::FILE_BROWSE,
        required_features: features::FILE_BROWSE,
        ..client_hello()
    }
}

// The shared hello_with fixture waits for SessionState after OK. FILES has no
// roster entry and deliberately receives no SessionState.
async fn files_auth(client: &mut WireClient, hello: pb::ClientHello) -> pb::AuthResult {
    client.send(hello).await;
    loop {
        match client.next().await {
            pb::envelope::Msg::ServerHello(_) => {}
            pb::envelope::Msg::AuthResult(auth) => return auth,
            other => panic!("unexpected FILES handshake message: {other:?}"),
        }
    }
}

fn assert_ok(auth: &pb::AuthResult, role: pb::Role) {
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, role as i32);
    assert_eq!(auth.session_token.len(), 16);
    assert!(!auth.resumed);
}

async fn ping(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 42 }).await;
    client
        .next_skipping_session_state("Pong", |msg| match msg {
            pb::envelope::Msg::Pong(pong) if pong.t1_us == 42 => Some(()),
            _ => None,
        })
        .await;
}

async fn files(host: &WireHost, device: Dev) -> (WireClient, pb::AuthResult) {
    let mut client = host.connect_as(device, 0).await;
    let auth = files_auth(&mut client, files_hello()).await;
    assert_ok(&auth, pb::Role::Viewer);
    ping(&mut client).await;
    (client, auth)
}

fn assert_only_controller(host: &WireHost, device: Dev) {
    let summaries = host.mgr.session_control().sessions_list();
    assert_eq!(summaries.len(), 1);
    assert_eq!(summaries[0].device_id, device_id(device));
    assert_eq!(summaries[0].role, host_core::ipc::SessionRole::Controller);
    assert_eq!(
        roster::auth_snapshot(&host.mgr, "another-device"),
        SessionSnapshot {
            has_controller: true,
            viewer_count: 0,
        }
    );
    let events = host.events.events();
    let sessions: Vec<_> = events
        .iter()
        .filter_map(|event| match event {
            Event::SessionsChanged(payload) => Some(payload),
            _ => None,
        })
        .collect();
    assert!(!sessions.is_empty());
    for payload in sessions {
        assert_eq!(payload["sessions"].as_array().unwrap().len(), 1);
        assert_eq!(payload["sessions"][0]["device_id"], device_id(device));
        assert_eq!(
            payload["capsule"]["controller"]["device_id"],
            device_id(device)
        );
        assert_eq!(payload["capsule"]["viewer_count"], 0);
    }
}

#[tokio::test]
async fn files_leave_controller_roster_and_capsule_unchanged() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    assert_ok(&a.hello().await.1, pb::Role::Controller);
    let controller = host.conns()[0].clone();
    let (mut b, _) = files(&host, Dev::B).await;
    assert_eq!(host.conns().len(), 2);
    let info = host
        .conns()
        .into_iter()
        .find(|conn| conn.intent == pb::Intent::Files)
        .unwrap();
    assert_eq!(info.role, Role::Viewer);
    assert!(info.can_control);
    assert_eq!(info.session_id.len(), 32);
    assert!(info.session_id.bytes().all(|byte| byte.is_ascii_hexdigit()));
    assert!(roster::with_roster(&host.mgr, |r| r
        .session_of(info.key)
        .is_none()));
    assert_only_controller(&host, Dev::A);
    a.expect_none_matching(QUIET, |msg| {
        matches!(
            msg,
            pb::envelope::Msg::Goodbye(_) | pb::envelope::Msg::ControlRequest(_)
        ) || matches!(msg, pb::envelope::Msg::SessionState(state)
                if state.your_role != pb::Role::Controller as i32)
    })
    .await;
    assert_eq!(
        host.mgr.info(controller.key).unwrap().role,
        Role::Controller
    );
    b.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| host.conns().len() == 1).await;
    assert_only_controller(&host, Dev::A);
    ping(&mut a).await;
}

#[tokio::test]
async fn files_cannot_start_video_or_request_control_and_remain_connected() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let (mut b, _) = files(&host, Dev::B).await;
    b.send(pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        max_width: 640,
        fps: 30,
        bitrate_kbps: 500,
        ..Default::default()
    })
    .await;
    match b.next().await {
        pb::envelope::Msg::Error(error) => {
            assert_eq!(error.code, u32::from(ErrorCode::PolicyDenied.code()));
            assert_eq!(error.message, "files session has no video");
        }
        other => panic!("expected video denial, got {other:?}"),
    }
    ping(&mut b).await;
    assert_eq!(host.platform.active_captures(), 0);
    b.send(pb::RequestControl {}).await;
    ping(&mut b).await;
    a.expect_none_matching(QUIET, |msg| {
        matches!(
            msg,
            pb::envelope::Msg::ControlRequest(_) | pb::envelope::Msg::Goodbye(_)
        )
    })
    .await;
    assert_only_controller(&host, Dev::A);
}

#[tokio::test]
async fn files_cannot_inject_input_write_clipboard_or_run_session_actions() {
    let host = WireHost::start().await;
    let (mut b, _) = files(&host, Dev::B).await;
    let info = host.conns()[0].clone();
    let counters = host.mgr.features().input.counters(info.key).unwrap();
    let mut input = b.open_typed(0x01).await;
    write_record(
        &mut input,
        &InputMsg::MouseMove { x: 0.4, y: 0.6 }.encode().unwrap(),
    )
    .await;
    host.wait_until(|| counters.dropped.load(Ordering::Relaxed) > 0)
        .await;
    let item = ClipboardItem::new(ClipboardKind::Text, b"files must not write".to_vec()).unwrap();
    let mut clipboard = b
        .open_typed(host_core::session::clipboard::CLIPBOARD_STREAM_TYPE)
        .await;
    clipboard
        .write_all(&encode_stream(&item).unwrap())
        .await
        .unwrap();
    clipboard.finish().unwrap();
    b.send(pb::SessionAction {
        kind: pb::session_action::Kind::LockScreen as i32,
        enable: true,
    })
    .await;
    ping(&mut b).await;
    tokio::time::sleep(QUIET).await;
    assert!(host.platform.injected_msgs().is_empty());
    assert!(host.platform.clipboard_writes().is_empty());
    assert!(host.platform.session_actions().is_empty());
    assert!(host.mgr.session_control().sessions_list().is_empty());
}

#[tokio::test]
async fn same_device_files_replaces_only_files_across_listeners() {
    let host = WireHost::start_with(WireOptions {
        listeners: 2,
        ..Default::default()
    })
    .await;
    let mut a = host.connect().await;
    a.hello().await;
    let (mut b, old_auth) = files(&host, Dev::B).await;
    let old = host
        .conns()
        .into_iter()
        .find(|info| info.intent == pb::Intent::Files)
        .unwrap();
    let mut replacement = host.connect_as(Dev::B, 1).await;
    let auth = files_auth(&mut replacement, files_hello()).await;
    assert_ok(&auth, pb::Role::Viewer);
    ping(&mut replacement).await;
    assert_ne!(auth.session_token, old_auth.session_token);
    let reason = b
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Goodbye(goodbye) => Some(goodbye.reason),
            _ => None,
        })
        .await;
    assert_eq!(reason, pb::goodbye::Reason::Replaced as i32);
    assert_eq!(b.close_code().await, 3);
    assert!(host.mgr.info(old.key).is_none());
    assert_eq!(host.conns().len(), 2);
    assert!(host
        .conns()
        .iter()
        .all(|info| info.session_id != old.session_id));
    assert_only_controller(&host, Dev::A);
    ping(&mut a).await;
}

#[tokio::test]
async fn same_device_control_and_files_coexist_in_both_admission_orders() {
    for files_first in [false, true] {
        let host = WireHost::start().await;
        let mut control = host.connect().await;
        let (mut transfer, _) = if files_first {
            let files = files(&host, Dev::A).await;
            assert_ok(&control.hello().await.1, pb::Role::Controller);
            files
        } else {
            assert_ok(&control.hello().await.1, pb::Role::Controller);
            files(&host, Dev::A).await
        };
        ping(&mut control).await;
        ping(&mut transfer).await;
        assert_eq!(host.conns().len(), 2);
        assert!(host
            .conns()
            .iter()
            .all(|info| info.device_id == device_id(Dev::A)));
        assert_only_controller(&host, Dev::A);
    }
}

#[tokio::test]
async fn view_only_device_is_denied_files_on_the_wire() {
    let host = WireHost::start().await;
    host.pair(Dev::B, Permission::ViewOnly);
    let mut b = host.connect_as(Dev::B, 0).await;
    let auth = files_auth(&mut b, files_hello()).await;
    assert_eq!(auth.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(
        auth.error.unwrap().code,
        u32::from(ErrorCode::PolicyDenied.code())
    );
    assert_eq!(b.close_code().await, 2);
    assert!(host.conns().is_empty());
}

#[tokio::test]
async fn files_token_never_resumes_or_enters_the_roster() {
    let host = WireHost::start().await;
    let (old, auth) = files(&host, Dev::B).await;
    let (mut new, auth) = {
        let mut client = host.connect_as(Dev::B, 0).await;
        let auth = files_auth(
            &mut client,
            pb::ClientHello {
                resume_token: auth.session_token,
                ..files_hello()
            },
        )
        .await;
        (client, auth)
    };
    assert_ok(&auth, pb::Role::Viewer);
    ping(&mut new).await;
    assert_eq!(old.close_code().await, 3);
    assert_eq!(host.conns().len(), 1);
    assert!(!host.conns()[0].resumed);
    assert!(host.mgr.session_control().sessions_list().is_empty());
}

#[tokio::test]
async fn files_lifecycle_has_no_session_audit_but_controller_does() {
    let host = WireHost::start_with(WireOptions {
        audit: true,
        ..Default::default()
    })
    .await;
    let mut a = host.connect().await;
    a.hello().await;
    let (mut b, _) = files(&host, Dev::B).await;
    b.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| host.conns().len() == 1).await;
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| host.conns().is_empty() && host.audit_events().len() >= 2)
        .await;
    let events: Vec<_> = host
        .audit_events()
        .into_iter()
        .filter(|event| matches!(event.event.as_str(), "session_start" | "session_end"))
        .collect();
    assert_eq!(events.len(), 2, "unexpected session audits: {events:?}");
    assert_eq!(events[0].event, "session_start");
    assert_eq!(events[1].event, "session_end");
    for event in events {
        assert_eq!(event.device_id.as_deref(), Some(device_id(Dev::A).as_str()));
    }
}

#[tokio::test]
async fn files_requires_file_browse_or_receives_compatibility_rejection() {
    let host = WireHost::start().await;
    let mut client = host.connect_as(Dev::B, 0).await;
    assert_ne!(handlers::host_features() & features::FILE_BROWSE, 0);
    let auth = files_auth(&mut client, files_hello()).await;
    assert_ok(&auth, pb::Role::Viewer);
    ping(&mut client).await;
}

#[tokio::test]
async fn files_with_control_token_preserves_controller_and_its_resume_token() {
    let host = WireHost::start().await;
    let mut control = host.connect().await;
    let control_auth = control.hello().await.1;
    let original = host.conns()[0].clone();
    let summaries = host.mgr.session_control().sessions_list();

    let mut transfer = host.connect_as(Dev::A, 0).await;
    let auth = files_auth(
        &mut transfer,
        pb::ClientHello {
            resume_token: control_auth.session_token.clone(),
            ..files_hello()
        },
    )
    .await;
    assert_ok(&auth, pb::Role::Viewer);
    ping(&mut transfer).await;
    ping(&mut control).await;
    assert_eq!(host.conns().len(), 2);
    let controller = host.mgr.info(original.key).unwrap();
    assert_eq!(controller.role, Role::Controller);
    assert_eq!(controller.session_id, original.session_id);
    assert_eq!(host.mgr.session_control().sessions_list(), summaries);
    assert_only_controller(&host, Dev::A);
    let files = host
        .conns()
        .into_iter()
        .find(|info| info.intent == pb::Intent::Files)
        .unwrap();
    assert_eq!(files.role, Role::Viewer);
    assert!(!files.resumed);
    assert_ne!(files.session_id, original.session_id);

    // The FILES attempt must not consume or rotate the controller's token.
    let mut resumed = host.connect().await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: control_auth.session_token,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, pb::Role::Controller as i32);
    assert!(auth.resumed);
    assert_eq!(control.close_code().await, 3);
    assert!(host
        .conns()
        .iter()
        .any(|info| info.session_id == original.session_id));
    ping(&mut transfer).await;
}

#[tokio::test]
async fn files_with_view_token_preserves_viewer_controller_and_resume_token() {
    let host = WireHost::start().await;
    let mut control = host.connect().await;
    control.hello().await;
    let controller = host.conns()[0].clone();
    let mut view = host.connect_as(Dev::B, 0).await;
    let (_, view_auth) = view
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    assert_ok(&view_auth, pb::Role::Viewer);
    let viewer = host
        .conns()
        .into_iter()
        .find(|info| info.intent == pb::Intent::View)
        .unwrap();
    let summaries = host.mgr.session_control().sessions_list();

    let mut transfer = host.connect_as(Dev::B, 0).await;
    let auth = files_auth(
        &mut transfer,
        pb::ClientHello {
            resume_token: view_auth.session_token.clone(),
            ..files_hello()
        },
    )
    .await;
    assert_ok(&auth, pb::Role::Viewer);
    ping(&mut transfer).await;
    ping(&mut view).await;
    ping(&mut control).await;
    assert_eq!(host.conns().len(), 3);
    assert_eq!(
        host.mgr.info(controller.key).unwrap().role,
        Role::Controller
    );
    assert_eq!(
        host.mgr.info(viewer.key).unwrap().session_id,
        viewer.session_id
    );
    assert_eq!(host.mgr.session_control().sessions_list(), summaries);
    assert_eq!(
        roster::auth_snapshot(&host.mgr, "another-device"),
        SessionSnapshot {
            has_controller: true,
            viewer_count: 1,
        }
    );

    let mut resumed = host.connect_as(Dev::B, 0).await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            resume_token: view_auth.session_token,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    assert!(auth.resumed);
    assert_eq!(view.close_code().await, 3);
    assert!(host
        .conns()
        .iter()
        .any(|info| info.session_id == viewer.session_id));
    ping(&mut transfer).await;
    ping(&mut control).await;
}

async fn roster_intent_with_files_token_is_fresh(intent: pb::Intent) {
    let host = WireHost::start().await;
    let mut control = host.connect().await;
    control.hello().await;
    let controller = host.conns()[0].clone();
    let (mut transfer, files_auth) = files(&host, Dev::B).await;
    let original_files = host
        .conns()
        .into_iter()
        .find(|info| info.intent == pb::Intent::Files)
        .unwrap();
    let mut client = host.connect_as(Dev::B, 0).await;
    let (_, auth) = client
        .hello_with(pb::ClientHello {
            intent: intent as i32,
            resume_token: files_auth.session_token.clone(),
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Viewer);
    assert_ne!(auth.session_token, files_auth.session_token);
    ping(&mut client).await;
    ping(&mut transfer).await;
    ping(&mut control).await;
    assert_eq!(host.conns().len(), 3);
    assert_eq!(
        host.mgr.info(controller.key).unwrap().role,
        Role::Controller
    );
    assert_eq!(
        host.mgr.info(original_files.key).unwrap().session_id,
        original_files.session_id
    );
    let fresh = host
        .conns()
        .into_iter()
        .find(|info| info.device_id == device_id(Dev::B) && info.intent == intent)
        .unwrap();
    assert!(!fresh.resumed);
    assert_ne!(fresh.session_id, original_files.session_id);
    let summaries = host.mgr.session_control().sessions_list();
    assert_eq!(summaries.len(), 2);
    assert!(summaries
        .iter()
        .any(|summary| summary.session_id == controller.session_id
            && summary.role == host_core::ipc::SessionRole::Controller));
    assert!(!summaries
        .iter()
        .any(|summary| summary.session_id == original_files.session_id));
}

#[tokio::test]
async fn control_with_files_token_is_fresh_and_keeps_existing_connections() {
    roster_intent_with_files_token_is_fresh(pb::Intent::Control).await;
}

#[tokio::test]
async fn view_with_files_token_is_fresh_and_keeps_existing_connections() {
    roster_intent_with_files_token_is_fresh(pb::Intent::View).await;
}

#[tokio::test]
async fn attended_approval_payload_identifies_files_control_and_view_intents() {
    for (intent, name, role) in [
        (pb::Intent::Files, "files", pb::Role::Viewer),
        (pb::Intent::Control, "control", pb::Role::Controller),
        (pb::Intent::View, "view", pb::Role::Viewer),
    ] {
        let mut options = WireOptions::default();
        options.options.auth_policy.attended_mode = true;
        let host = WireHost::start_with(options).await;
        let mut client = host.connect().await;
        let hello = if intent == pb::Intent::Files {
            files_hello()
        } else {
            pb::ClientHello {
                intent: intent as i32,
                ..client_hello()
            }
        };
        client.send(hello).await;
        let pending = client
            .next_matching(|message| match message {
                pb::envelope::Msg::AuthResult(auth) => Some(auth.clone()),
                _ => None,
            })
            .await;
        assert_eq!(
            pending.status,
            pb::auth_result::Status::PendingApproval as i32
        );
        assert_eq!(pending.role, role as i32);
        host.wait_until(|| !host.mgr.session_control().approval_pending().is_empty())
            .await;
        let requests = host.mgr.session_control().approval_pending();
        assert_eq!(requests.len(), 1);
        let payload = &requests[0];
        assert_eq!(payload["intent"], name);
        assert_eq!(payload["reasons"], serde_json::json!(["attended_mode"]));
        assert!(host
            .events
            .events()
            .iter()
            .any(|event| matches!(event, Event::ApprovalRequested(value) if value == payload)));
        assert!(host.mgr.session_control().approval_respond(
            payload["request_id"].as_str().unwrap(),
            true,
            None
        ));
        let auth = client
            .next_matching(|message| match message {
                pb::envelope::Msg::AuthResult(auth) => Some(auth.clone()),
                _ => None,
            })
            .await;
        assert_ok(&auth, role);
        ping(&mut client).await;
    }
}

#[tokio::test]
async fn legacy_connection_approval_without_intent_preserves_payload() {
    let host = WireHost::start().await;
    let approver = approval::connection_approver(&host.mgr).unwrap();
    let request = ConnectionApprovalRequest {
        request_id: "legacy-request".into(),
        device_id: "legacy-device".into(),
        device_name: "Legacy phone".into(),
        device_fp: [1; 32],
        role: Role::Viewer,
        reasons: vec![host_core::auth::ApprovalReason::AttendedMode],
        peer: pb::PeerInfo::default(),
        platform: pb::Platform::Ios,
        path: host_core::auth::PathClass::Lan,
        intent: None,
    };
    let pending = tokio::spawn(async move { approver.request(request).await });
    host.wait_until(|| !host.mgr.session_control().approval_pending().is_empty())
        .await;
    let requests = host.mgr.session_control().approval_pending();
    assert_eq!(requests.len(), 1);
    let payload = &requests[0];
    assert_eq!(
        *payload,
        serde_json::json!({
            "request_id": payload["request_id"],
            "kind": "connection",
            "device_name": "Legacy phone",
            "device_id": "legacy-device",
            "platform": "ios",
            "whois_login": null,
            "path": "lan",
            "reasons": ["attended_mode"],
            "timeout_s": 30,
            "permission_choice": false,
            "device_fp": rsp_core::ids::fp_b64url(&[1; 32]),
        })
    );
    assert!(host
        .events
        .events()
        .iter()
        .any(|event| matches!(event, Event::ApprovalRequested(value) if value == payload)));
    assert!(host.mgr.session_control().approval_respond(
        payload["request_id"].as_str().unwrap(),
        true,
        None
    ));
    assert!(pending.await.unwrap());
}
