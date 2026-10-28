use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_core::auth::{ApprovalReason, PathClass};
use host_core::ipc::{self, Event};
use host_core::pairing::{PairApproval, PairApprovalRequest, PairApprover};
use host_core::session::approval::{
    path_name, reason_name, LocalPrompts, OpenPrompt, PromptAnswer, PromptEvents, PromptKind,
    PromptOutcome, PromptRequest, APPROVAL_TIMEOUT, APPROVAL_TIMEOUT_S,
};
use rsp_core::ids::{fp_b64url, id_from_fp};
use rsp_core::pb;
use rsp_core::trust::Permission as TrustPermission;
use serde_json::{json, Value};

struct FakeEvents {
    accept: AtomicBool,
    events: Mutex<Vec<Event>>,
}

impl FakeEvents {
    fn recording() -> Arc<Self> {
        Arc::new(Self {
            accept: AtomicBool::new(true),
            events: Mutex::new(Vec::new()),
        })
    }

    fn refusing() -> Arc<Self> {
        Arc::new(Self {
            accept: AtomicBool::new(false),
            events: Mutex::new(Vec::new()),
        })
    }

    fn values(&self, requested: bool) -> Vec<Value> {
        self.events
            .lock()
            .expect("events")
            .iter()
            .filter_map(|event| match (requested, event) {
                (true, Event::ApprovalRequested(value)) | (false, Event::ApprovalClosed(value)) => {
                    Some(value.clone())
                }
                _ => None,
            })
            .collect()
    }

    fn opened(&self) -> Value {
        let events = self.values(true);
        assert_eq!(events.len(), 1, "one approvalRequested event");
        events[0].clone()
    }

    fn closed(&self) -> Value {
        let events = self.values(false);
        assert_eq!(events.len(), 1, "one approvalClosed event");
        events[0].clone()
    }
}

impl PromptEvents for FakeEvents {
    fn emit(&self, event: Event) -> bool {
        if !self.accept.load(Ordering::SeqCst) {
            return false;
        }
        self.events.lock().expect("events").push(event);
        true
    }
}

fn request() -> PromptRequest {
    PromptRequest {
        kind: PromptKind::Connection,
        device_name: "Phone".to_owned(),
        device_id: "dev-123".to_owned(),
        platform: pb::Platform::Ios,
        whois_login: Some("alice".to_owned()),
        path: Some("tailnet".to_owned()),
        reasons: vec![reason_name(ApprovalReason::AttendedMode).to_owned()],
        device_fp: None,
        intent: None,
    }
}

fn assert_closed(event: &Value, kind: PromptKind, outcome: &str) {
    assert_eq!(event["kind"], kind.as_str());
    assert_eq!(event["outcome"], outcome);
    assert_eq!(event["request_id"].as_str().expect("request id").len(), 18);
}

#[tokio::test]
async fn connection_intent_metadata_is_optional_and_preserves_legacy_payload() {
    for (intent, name) in [
        (None, None),
        (Some(pb::Intent::Files), Some("files")),
        (Some(pb::Intent::Control), Some("control")),
        (Some(pb::Intent::View), Some("view")),
        (Some(pb::Intent::Unspecified), Some("control")),
    ] {
        let events = FakeEvents::recording();
        let prompts = LocalPrompts::new(events.clone());
        let mut request = request();
        request.intent = intent;
        let open = prompts.open(request);
        let mut expected = json!({
            "request_id": open.request_id,
            "kind": "connection",
            "device_name": "Phone",
            "device_id": "dev-123",
            "platform": "ios",
            "whois_login": "alice",
            "path": "tailnet",
            "reasons": ["attended_mode"],
            "timeout_s": APPROVAL_TIMEOUT_S,
            "permission_choice": false,
            "device_fp": null,
        });
        if let Some(name) = name {
            expected["intent"] = name.into();
        }
        assert_eq!(events.opened(), expected);
        assert_eq!(prompts.pending_requests(), vec![expected]);
        assert!(prompts.respond(&open.request_id, true, None));
        assert!(open.wait().await.allowed());
        assert!(prompts.remote_input_allowed());
    }
}

#[tokio::test(start_paused = true)]
async fn allow_opens_gate_and_closes_once() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let gate = prompts.subscribe_gate();
    assert!(!*gate.borrow());

    let open = prompts.open(request());
    assert_eq!(open.request_id.len(), 18);
    assert!(open.request_id.starts_with("a-"));
    assert_eq!(prompts.open_count(), 1);
    assert!(!prompts.remote_input_allowed());
    assert!(*gate.borrow());

    let opened = events.opened();
    assert_eq!(opened["request_id"], open.request_id);
    assert_eq!(opened["kind"], "connection");
    assert_eq!(opened["device_name"], "Phone");
    assert_eq!(opened["device_id"], "dev-123");
    assert_eq!(opened["platform"], "ios");
    assert_eq!(opened["whois_login"], "alice");
    assert_eq!(opened["path"], "tailnet");
    assert_eq!(opened["reasons"], json!(["attended_mode"]));
    assert_eq!(opened["timeout_s"], APPROVAL_TIMEOUT_S);
    assert_eq!(opened["permission_choice"], false);
    assert!(opened["device_fp"].is_null());

    assert!(prompts.respond(&open.request_id, true, None));
    assert_eq!(
        open.wait().await,
        PromptOutcome::Answered(PromptAnswer {
            allow: true,
            permission: None,
        })
    );
    let closed = events.closed();
    assert_closed(&closed, PromptKind::Connection, "allowed");
    assert_eq!(closed["request_id"], opened["request_id"]);
    assert!(!*gate.borrow());
    assert!(prompts.remote_input_allowed());
    assert_eq!(prompts.open_count(), 0);
}

#[tokio::test(start_paused = true)]
async fn deny_is_answered_and_not_allowed() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let open = prompts.open(request());
    assert!(prompts.respond(&open.request_id, false, None));
    let outcome = open.wait().await;
    assert_eq!(
        outcome,
        PromptOutcome::Answered(PromptAnswer {
            allow: false,
            permission: None,
        })
    );
    assert!(!outcome.allowed());
    assert_closed(&events.closed(), PromptKind::Connection, "denied");
}

#[tokio::test(start_paused = true)]
async fn timeout_is_thirty_seconds_and_late_response_is_rejected() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let started = tokio::time::Instant::now();
    let gate = prompts.subscribe_gate();
    let open = prompts.open(request());
    let request_id = open.request_id.clone();
    assert!(*gate.borrow());

    assert_eq!(open.wait().await, PromptOutcome::TimedOut);
    assert_eq!(started.elapsed(), APPROVAL_TIMEOUT);
    assert_closed(&events.closed(), PromptKind::Connection, "timeout");
    assert!(!*gate.borrow());
    assert_eq!(prompts.open_count(), 0);
    assert!(!prompts.respond(&request_id, true, None));
}

#[tokio::test(start_paused = true)]
async fn deadline_is_measured_from_open() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let open = prompts.open(request());
    tokio::time::advance(Duration::from_secs(20)).await;
    let waiting = tokio::time::Instant::now();
    assert_eq!(open.wait().await, PromptOutcome::TimedOut);
    assert_eq!(waiting.elapsed(), Duration::from_secs(10));
    assert_closed(&events.closed(), PromptKind::Connection, "timeout");
}

#[tokio::test(start_paused = true)]
async fn cancel_closes_once_and_unknown_cancel_is_false() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let gate = prompts.subscribe_gate();
    let open = prompts.open(request());
    let request_id = open.request_id.clone();
    assert!(prompts.cancel(&request_id));
    assert_eq!(open.wait().await, PromptOutcome::Cancelled);
    assert_closed(&events.closed(), PromptKind::Connection, "cancelled");
    assert!(!prompts.cancel(&request_id));
    assert!(!prompts.cancel("a-unknown"));
    assert!(!*gate.borrow());
    assert_eq!(events.values(false).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn no_ui_listener_returns_unavailable_without_opening_gate() {
    let events = FakeEvents::refusing();
    let prompts = LocalPrompts::new(events.clone());
    let gate = prompts.subscribe_gate();
    let open = prompts.open(request());
    assert_eq!(open.wait().await, PromptOutcome::Unavailable);
    assert_eq!(prompts.open_count(), 0);
    assert!(prompts.remote_input_allowed());
    assert!(matches!(gate.has_changed(), Ok(false)));
    assert!(!*gate.borrow());
    assert!(events.values(true).is_empty());
    assert!(events.values(false).is_empty());
}

#[tokio::test(start_paused = true)]
async fn concurrent_prompts_keep_gate_open_until_last_closes() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let gate = prompts.subscribe_gate();
    let first = prompts.open(request());
    let second = prompts.open(request());

    assert_eq!(prompts.open_count(), 2);
    assert!(*gate.borrow());
    assert!(prompts.respond(&first.request_id, true, None));
    assert_eq!(
        first.wait().await,
        PromptOutcome::Answered(PromptAnswer {
            allow: true,
            permission: None,
        })
    );
    assert_eq!(prompts.open_count(), 1);
    assert!(*gate.borrow());
    assert!(!prompts.remote_input_allowed());

    assert!(prompts.respond(&second.request_id, false, None));
    assert_eq!(
        second.wait().await,
        PromptOutcome::Answered(PromptAnswer {
            allow: false,
            permission: None,
        })
    );
    assert_eq!(prompts.open_count(), 0);
    assert!(!*gate.borrow());
    assert_eq!(events.values(false).len(), 2);
}

#[tokio::test(start_paused = true)]
async fn subscribing_after_open_sees_the_current_gate_state() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let open = prompts.open(request());
    let gate = prompts.subscribe_gate();
    assert!(*gate.borrow());
    assert!(!gate.has_changed().expect("gate is open"));

    assert!(prompts.respond(&open.request_id, true, None));
    assert!(open.wait().await.allowed());
    assert!(!*gate.borrow());
}

#[tokio::test(start_paused = true)]
async fn dropping_unwaited_prompt_cancels_it_once() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let gate = prompts.subscribe_gate();
    let open: OpenPrompt = prompts.open(request());
    let request_id = open.request_id.clone();
    assert!(*gate.borrow());
    drop(open);

    assert!(!*gate.borrow());
    assert_eq!(prompts.open_count(), 0);
    assert_closed(&events.closed(), PromptKind::Connection, "cancelled");
    assert!(!prompts.respond(&request_id, true, None));
    assert_eq!(events.values(false).len(), 1);
}

#[tokio::test(start_paused = true)]
async fn second_response_for_same_id_is_rejected() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let open = prompts.open(request());
    assert!(prompts.respond(&open.request_id, true, None));
    assert!(!prompts.respond(&open.request_id, false, None));
    assert_eq!(
        open.wait().await,
        PromptOutcome::Answered(PromptAnswer {
            allow: true,
            permission: None,
        })
    );
    assert_closed(&events.closed(), PromptKind::Connection, "allowed");
}

async fn start_pairing(
    prompts: &LocalPrompts,
    request: PairApprovalRequest,
) -> tokio::task::JoinHandle<PairApproval> {
    let prompts = prompts.clone();
    tokio::spawn(async move { PairApprover::request(&prompts, request).await })
}

fn pairing_request(request_id: &str, device_fp: [u8; 32]) -> PairApprovalRequest {
    PairApprovalRequest {
        request_id: request_id.to_owned(),
        device_name: "Phone".to_owned(),
        platform: pb::Platform::Ios,
        whois_login: Some("alice".to_owned()),
        device_fp,
    }
}

#[tokio::test(start_paused = true)]
async fn pairing_allow_with_unattended_returns_unattended() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let device_fp = [7_u8; 32];
    let task = start_pairing(&prompts, pairing_request("pair-1", device_fp)).await;

    tokio::task::yield_now().await;
    let opened = events.opened();
    assert_eq!(opened["request_id"], "pair-1");
    assert_eq!(opened["kind"], "pairing");
    assert_eq!(opened["permission_choice"], true);
    assert_eq!(opened["device_fp"], fp_b64url(&device_fp));
    assert_eq!(opened["device_id"].as_str().expect("device id").len(), 16);
    assert_eq!(opened["device_id"], id_from_fp(&device_fp));
    assert!(prompts.respond("pair-1", true, Some(ipc::Permission::Unattended)));

    assert_eq!(
        task.await.expect("pair task"),
        PairApproval {
            allow: true,
            permission: TrustPermission::Unattended,
        }
    );
    assert_eq!(events.closed()["outcome"], "allowed");
}

#[tokio::test(start_paused = true)]
async fn pairing_allow_without_permission_defaults_to_control() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let task = start_pairing(&prompts, pairing_request("pair-2", [8_u8; 32])).await;
    tokio::task::yield_now().await;
    assert!(prompts.respond("pair-2", true, None));
    assert_eq!(
        task.await.expect("pair task"),
        PairApproval {
            allow: true,
            permission: TrustPermission::Control,
        }
    );
}

#[tokio::test(start_paused = true)]
async fn pairing_deny_returns_view_only() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let task = start_pairing(&prompts, pairing_request("pair-3", [9_u8; 32])).await;
    tokio::task::yield_now().await;
    assert!(prompts.respond("pair-3", false, Some(ipc::Permission::Unattended)));
    assert_eq!(
        task.await.expect("pair task"),
        PairApproval {
            allow: false,
            permission: TrustPermission::ViewOnly,
        }
    );
    assert_eq!(events.closed()["outcome"], "denied");
}

#[tokio::test(start_paused = true)]
async fn pairing_timeout_returns_view_only() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let task = start_pairing(&prompts, pairing_request("pair-4", [10_u8; 32])).await;
    tokio::task::yield_now().await;
    tokio::time::advance(APPROVAL_TIMEOUT).await;
    assert_eq!(
        task.await.expect("pair task"),
        PairApproval {
            allow: false,
            permission: TrustPermission::ViewOnly,
        }
    );
    assert_eq!(events.closed()["outcome"], "timeout");
}

#[tokio::test(start_paused = true)]
async fn pairing_generates_id_when_request_id_is_empty() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let task = start_pairing(&prompts, pairing_request("", [11_u8; 32])).await;
    tokio::task::yield_now().await;
    let opened = events.opened();
    let request_id = opened["request_id"].as_str().expect("request id");
    assert!(request_id.starts_with("a-"));
    assert_eq!(request_id.len(), 18);
    assert!(prompts.respond(request_id, true, None));
    assert!(task.await.expect("pair task").allow);
}

#[tokio::test(start_paused = true)]
async fn pairing_generates_a_new_id_when_the_requested_id_is_in_use() {
    let events = FakeEvents::recording();
    let prompts = LocalPrompts::new(events.clone());
    let first = start_pairing(&prompts, pairing_request("shared", [12_u8; 32])).await;
    tokio::task::yield_now().await;
    let second = start_pairing(&prompts, pairing_request("shared", [13_u8; 32])).await;
    tokio::task::yield_now().await;

    let opened = events.values(true);
    assert_eq!(opened.len(), 2);
    assert_eq!(opened[0]["request_id"], "shared");
    let generated = opened[1]["request_id"]
        .as_str()
        .expect("generated request id");
    assert!(generated.starts_with("a-"));
    assert_eq!(generated.len(), 18);
    assert_ne!(generated, "shared");

    assert!(prompts.respond("shared", true, None));
    assert!(prompts.respond(generated, true, Some(ipc::Permission::ViewOnly)));
    let first = first.await.expect("first pair task");
    let second = second.await.expect("second pair task");
    assert_eq!(first.permission, TrustPermission::Control);
    assert_eq!(second.permission, TrustPermission::ViewOnly);
}

#[test]
fn reason_names_are_stable() {
    assert_eq!(reason_name(ApprovalReason::AttendedMode), "attended_mode");
    assert_eq!(reason_name(ApprovalReason::LocalActivity), "local_activity");
    assert_eq!(
        reason_name(ApprovalReason::IntranetConfirmation),
        "intranet_confirmation"
    );
    assert_eq!(reason_name(ApprovalReason::NodeMismatch), "node_mismatch");
    assert_eq!(
        reason_name(ApprovalReason::DeviceRequiresConfirmation),
        "device_requires_confirmation"
    );
}

#[test]
fn path_names_are_stable() {
    assert_eq!(path_name(PathClass::Tailnet), Some("tailnet"));
    assert_eq!(path_name(PathClass::Intranet), Some("intranet"));
    assert_eq!(path_name(PathClass::Lan), Some("lan"));
    assert_eq!(path_name(PathClass::Unknown), None);
}
