//! Local approval wiring, pending recovery, and the SEC-13 input gate.
use async_trait::async_trait;
use host_core::{
    ipc::{Event, Permission as IpcPermission},
    pairing::{PairApproval, PairApprovalRequest, PairApprover, PairingManager},
    session::{
        handlers::{activity, approval, roster},
        handshake::{ConnectionApprovalRequest, ConnectionApprover},
        manager::EndReason,
        takeover::Seat,
    },
    HostRuntime, RuntimeOptions,
};
use rsp_core::{hid::InputMsg, pb, trust::Permission};
use serde_json::{json, Value};
use std::{
    path::PathBuf,
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};

mod wire_support;
use wire_support::{
    client_hello, device_id, identity, write_record, Dev, WireClient, WireHost, WireOptions,
};

fn options(attended: bool, confirm_new: bool) -> WireOptions {
    let mut options = WireOptions::default();
    options.options.auth_policy.attended_mode = attended;
    options.config.security.confirm_new_devices = confirm_new;
    options
}

fn requested(host: &WireHost) -> Vec<Value> {
    host.events
        .events()
        .into_iter()
        .filter_map(|event| match event {
            Event::ApprovalRequested(payload) => Some(payload),
            _ => None,
        })
        .collect()
}

async fn closed(host: &WireHost, request: &Value, outcome: &str) {
    host.wait_until(|| {
        host.events.events().iter().any(|event| {
            matches!(event, Event::ApprovalClosed(payload)
            if payload == &json!({
                "request_id": request["request_id"], "kind": request["kind"],
                "outcome": outcome,
            }))
        })
    })
    .await;
}

async fn pending(host: &WireHost, device: Dev) -> (WireClient, Value) {
    let before = requested(host).len();
    let mut client = host.connect_as(device, 0).await;
    let (_, auth) = client.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::PendingApproval as i32);
    assert_eq!(auth.approval_timeout_s, 30);
    host.wait_until(|| requested(host).len() == before + 1)
        .await;
    let payload = requested(host).pop().unwrap();
    assert_eq!(payload["device_id"], device_id(device));
    assert_eq!(payload["kind"], "connection");
    assert_eq!(payload["platform"], "ios");
    assert_eq!(payload["path"], "lan");
    assert_eq!(payload["timeout_s"], 30);
    assert_eq!(payload["permission_choice"], false);
    assert_eq!(payload["whois_login"], Value::Null);
    assert_eq!(
        payload["device_fp"],
        rsp_core::ids::fp_b64url(&identity(device).fingerprint())
    );
    (client, payload)
}

fn respond(host: &WireHost, payload: &Value, allow: bool) {
    assert!(host.mgr.session_control().approval_respond(
        payload["request_id"].as_str().unwrap(),
        allow,
        None,
    ));
}

async fn auth(client: &mut WireClient, status: pb::auth_result::Status) -> pb::AuthResult {
    let result = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::AuthResult(result) => Some(result.clone()),
            _ => None,
        })
        .await;
    assert_eq!(result.status, status as i32);
    result
}

async fn allow(host: &WireHost, device: Dev) -> WireClient {
    let (mut client, request) = pending(host, device).await;
    respond(host, &request, true);
    auth(&mut client, pb::auth_result::Status::Ok).await;
    closed(host, &request, "allowed").await;
    client
}

async fn ping(client: &mut WireClient) {
    client.send(pb::Ping { t1_us: 42 }).await;
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Pong(pong) if pong.t1_us == 42 => Some(()),
            _ => None,
        })
        .await;
}

#[tokio::test]
async fn connection_allowed_through_local_prompt() {
    let host = WireHost::start_with(options(true, false)).await;
    let (mut client, request) = pending(&host, Dev::A).await;
    assert_eq!(request["reasons"], json!(["attended_mode"]));
    respond(&host, &request, true);
    auth(&mut client, pb::auth_result::Status::Ok).await;
    closed(&host, &request, "allowed").await;
    assert!(host.mgr.session_control().approval_pending().is_empty());
    assert!(!host.mgr.session_control().approval_respond(
        request["request_id"].as_str().unwrap(),
        true,
        None,
    ));
}

#[tokio::test]
async fn connection_denied_through_local_prompt() {
    let host = WireHost::start_with(options(true, false)).await;
    let (mut client, request) = pending(&host, Dev::A).await;
    respond(&host, &request, false);
    assert_eq!(
        auth(&mut client, pb::auth_result::Status::Denied)
            .await
            .error
            .unwrap()
            .code,
        140
    );
    assert_eq!(client.close_code().await, 2);
    closed(&host, &request, "denied").await;
    assert!(host.mgr.session_control().approval_pending().is_empty());
}

#[tokio::test]
async fn connection_approval_timeout_uses_paused_time() {
    let mut options = options(true, false);
    options.audit = true;
    let host = WireHost::start_with(options).await;
    let (mut client, request) = pending(&host, Dev::A).await;
    // The transport has a 10 second idle timeout. Keep it active while the
    // 30 second approval deadline advances, just as a waiting app does.
    let connection = client.conn.clone();
    let keep_alive = tokio::spawn(async move {
        loop {
            let _ = connection.send_datagram(vec![0xff].into());
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    tokio::time::pause();
    tokio::time::sleep(Duration::from_secs(31)).await;
    tokio::time::resume();
    keep_alive.abort();
    let error = auth(&mut client, pb::auth_result::Status::Denied)
        .await
        .error
        .unwrap();
    assert_eq!(error.code, 140);
    assert_eq!(error.detail["reason"], "timeout");
    assert!(host.audit_events().iter().any(|event| {
        event.event == "auth.denied"
            && event
                .detail
                .as_ref()
                .is_some_and(|detail| detail["code"] == 140 && detail["reason"] == "timeout")
    }));
    closed(&host, &request, "timeout").await;
    assert!(host.mgr.session_control().approval_pending().is_empty());
}

#[tokio::test]
async fn no_ui_listener_denies_immediately() {
    let host = WireHost::start_with(options(true, false)).await;
    host.events.set_listening(false);
    let (mut client, _) = pending(&host, Dev::A).await;
    assert_eq!(
        auth(&mut client, pb::auth_result::Status::Denied)
            .await
            .error
            .unwrap()
            .code,
        140
    );
    assert!(host.mgr.session_control().approval_pending().is_empty());
    assert!(approval::prompts(&host.mgr).unwrap().remote_input_allowed());
}

#[tokio::test]
async fn client_disconnect_cancels_pending_prompt() {
    let host = WireHost::start_with(options(true, false)).await;
    let (client, request) = pending(&host, Dev::A).await;
    client.conn.close(0u32.into(), b"cancel approval");
    closed(&host, &request, "cancelled").await;
    assert!(host.mgr.session_control().approval_pending().is_empty());
}

#[tokio::test]
async fn sec13_other_clients_cannot_inject_while_a_prompt_is_open() {
    let host = WireHost::start_with(options(true, false)).await;
    let mut controller = allow(&host, Dev::A).await;
    let mut input = controller.open_typed(0x01).await;
    let movement = InputMsg::MouseMove { x: 0.2, y: 0.3 };
    write_record(&mut input, &movement.encode().unwrap()).await;
    host.wait_until(|| host.platform.injected_msgs().len() == 1)
        .await;

    let releases = host.platform.release_count();
    let (mut other, request) = pending(&host, Dev::B).await;
    // HW1B releases held input when closing the gate. Wait until it has
    // observed the shared prompt before taking the comparison snapshot.
    host.wait_until(|| host.platform.release_count() > releases)
        .await;
    let before = host.platform.injected_msgs();
    write_record(&mut input, &movement.encode().unwrap()).await;
    // An oversized next record stops this stream only after the preceding
    // mouse record has been consumed. Its STOP_SENDING is a processing barrier.
    input.write_all(&u32::MAX.to_be_bytes()).await.unwrap();
    assert_eq!(
        tokio::time::timeout(Duration::from_secs(3), input.stopped())
            .await
            .unwrap()
            .unwrap(),
        Some(1u32.into())
    );
    assert_eq!(host.platform.injected_msgs(), before);

    // Deny B so A remains controller.
    respond(&host, &request, false);
    auth(&mut other, pb::auth_result::Status::Denied).await;
    closed(&host, &request, "denied").await;
    // The control loop handles gate notifications before answering this ping.
    ping(&mut controller).await;
    let mut input = controller.open_typed(0x01).await;
    write_record(&mut input, &movement.encode().unwrap()).await;
    host.wait_until(|| host.platform.injected_msgs().len() == before.len() + 1)
        .await;
    assert_eq!(host.platform.injected_msgs().last(), Some(&movement));
}

async fn pair_prompt(host: &WireHost, allow: bool) -> PairApproval {
    let approver = approval::pair_approver(&host.mgr).unwrap();
    let task = tokio::spawn(async move {
        approver
            .request(PairApprovalRequest {
                request_id: String::new(),
                device_name: "new phone".into(),
                platform: pb::Platform::Ios,
                whois_login: None,
                device_fp: identity(Dev::Unpaired).fingerprint(),
            })
            .await
    });
    host.wait_until(|| requested(host).iter().any(|r| r["kind"] == "pairing"))
        .await;
    let request = requested(host).pop().unwrap();
    assert_eq!(request["kind"], "pairing");
    assert_eq!(request["permission_choice"], true);
    assert_eq!(
        host.mgr.session_control().approval_pending(),
        vec![request.clone()]
    );
    assert!(host.mgr.session_control().approval_respond(
        request["request_id"].as_str().unwrap(),
        allow,
        Some(IpcPermission::ViewOnly),
    ));
    let result = task.await.unwrap();
    closed(host, &request, if allow { "allowed" } else { "denied" }).await;
    result
}

#[tokio::test]
async fn pairing_uses_the_shared_registry_and_permission_choice() {
    let host = WireHost::start_with(options(true, false)).await;
    assert_eq!(
        pair_prompt(&host, true).await,
        PairApproval {
            allow: true,
            permission: Permission::ViewOnly
        }
    );
    assert!(host.mgr.session_control().approval_pending().is_empty());
}

#[tokio::test]
async fn pending_recovers_exact_payloads_in_opening_order() {
    let host = WireHost::start_with(options(true, false)).await;
    let (mut first, first_request) = pending(&host, Dev::A).await;
    let (second, second_request) = pending(&host, Dev::B).await;
    let control = host.mgr.session_control();
    assert_eq!(
        control.approval_pending(),
        vec![first_request.clone(), second_request.clone()]
    );
    assert_eq!(control.approval_pending(), requested(&host));
    respond(&host, &first_request, true);
    auth(&mut first, pb::auth_result::Status::Ok).await;
    assert_eq!(control.approval_pending(), vec![second_request.clone()]);
    second.conn.close(0u32.into(), b"cancel remaining approval");
    closed(&host, &second_request, "cancelled").await;
    assert!(control.approval_pending().is_empty());
}

fn state_dir() -> PathBuf {
    std::env::temp_dir().join(format!(
        "rs-approval-{}-{}",
        std::process::id(),
        rsp_core::clock::unix_micros()
    ))
}

#[tokio::test]
async fn new_devices_are_confirmed_once_and_history_survives_restart() {
    let directory = state_dir();
    let mut opts = options(false, true);
    opts.state_dir = Some(directory.clone());
    let host = WireHost::start_with(opts).await;
    let mut old = host.connect().await;
    assert_eq!(
        old.hello().await.1.status,
        pb::auth_result::Status::Ok as i32
    );
    assert!(requested(&host).is_empty());
    old.conn.close(0u32.into(), b"old device checked");
    host.wait_until(|| host.conns().is_empty()).await;
    host.pair(Dev::Unpaired, Permission::Control);
    let (mut new, request) = pending(&host, Dev::Unpaired).await;
    assert_eq!(request["reasons"], json!(["device_requires_confirmation"]));
    respond(&host, &request, true);
    auth(&mut new, pb::auth_result::Status::Ok).await;
    // OK precedes on_admitted. Wait for the actual seen update and disk write.
    host.wait_until(|| {
        !approval::confirm_new_device(&host.mgr, &device_id(Dev::Unpaired), Some(u64::MAX))
    })
    .await;
    let state: Value =
        serde_json::from_slice(&std::fs::read(directory.join("approval-state.json")).unwrap())
            .unwrap();
    assert!(state["since_us"].as_u64().unwrap() > 0);
    assert!(state["seen"]
        .as_array()
        .unwrap()
        .contains(&json!(device_id(Dev::Unpaired))));
    new.conn.close(0u32.into(), b"reconnect");
    host.wait_until(|| host.conns().is_empty()).await;
    let mut again = host.connect_as(Dev::Unpaired, 0).await;
    assert_eq!(
        again.hello().await.1.status,
        pb::auth_result::Status::Ok as i32
    );
    assert_eq!(requested(&host).len(), 1);
    drop((old, new, again));
    drop(host);
    tokio::task::yield_now().await;

    let mut opts = options(false, true);
    opts.state_dir = Some(directory.clone());
    let host = WireHost::start_with(opts).await;
    host.pair(Dev::Unpaired, Permission::Control);
    let mut client = host.connect_as(Dev::Unpaired, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::Ok as i32
    );
    assert!(requested(&host).is_empty());
    drop(client);
    drop(host);
    tokio::task::yield_now().await;
    std::fs::remove_dir_all(directory).unwrap();
}

#[tokio::test]
async fn disabling_new_device_confirmation_allows_new_pairs() {
    let host = WireHost::start_with(options(false, false)).await;
    host.pair(Dev::Unpaired, Permission::Control);
    let mut client = host.connect_as(Dev::Unpaired, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::Ok as i32
    );
    assert!(requested(&host).is_empty());
}

#[tokio::test]
async fn locally_approved_pair_does_not_need_first_connection_confirmation() {
    let host = WireHost::start_with(options(false, true)).await;
    assert!(pair_prompt(&host, true).await.allow);
    host.pair(Dev::Unpaired, Permission::Control);
    let mut client = host.connect_as(Dev::Unpaired, 0).await;
    assert_eq!(
        client.hello().await.1.status,
        pb::auth_result::Status::Ok as i32
    );
    assert_eq!(requested(&host).len(), 1);
}

#[tokio::test]
async fn denied_new_device_is_asked_again() {
    let host = WireHost::start_with(options(false, true)).await;
    host.pair(Dev::Unpaired, Permission::Control);
    let (mut client, request) = pending(&host, Dev::Unpaired).await;
    respond(&host, &request, false);
    auth(&mut client, pb::auth_result::Status::Denied).await;
    assert_eq!(client.close_code().await, 2);
    let (_client, request) = pending(&host, Dev::Unpaired).await;
    assert_eq!(request["reasons"], json!(["device_requires_confirmation"]));
    respond(&host, &request, false);
}

#[tokio::test]
async fn new_viewer_joining_existing_controller_requires_confirmation() {
    for permission in [Permission::Control, Permission::ViewOnly] {
        for allowed in [false, true] {
            let host = WireHost::start_with(options(false, true)).await;
            let mut controller = host.connect().await;
            assert_eq!(
                controller.hello().await.1.status,
                pb::auth_result::Status::Ok as i32
            );
            host.pair(Dev::Unpaired, permission);
            let mut viewer = host.connect_as(Dev::Unpaired, 0).await;
            let (_, result) = viewer
                .hello_with(pb::ClientHello {
                    intent: pb::Intent::View as i32,
                    ..client_hello()
                })
                .await;
            assert_eq!(
                result.status,
                pb::auth_result::Status::PendingApproval as i32
            );
            assert_eq!(result.role, pb::Role::Viewer as i32);
            assert_eq!(result.approval_timeout_s, 30);
            host.wait_until(|| requested(&host).len() == 1).await;
            let request = requested(&host).pop().unwrap();
            assert_eq!(request["device_id"], device_id(Dev::Unpaired));
            assert_eq!(request["reasons"], json!(["device_requires_confirmation"]));
            assert_eq!(
                host.mgr.session_control().approval_pending().as_slice(),
                std::slice::from_ref(&request)
            );
            assert_eq!(host.conns().len(), 1);
            assert!(host
                .conns()
                .iter()
                .all(|c| c.device_id == device_id(Dev::A)));

            respond(&host, &request, allowed);
            if allowed {
                let result = auth(&mut viewer, pb::auth_result::Status::Ok).await;
                assert_eq!(result.role, pb::Role::Viewer as i32);
                closed(&host, &request, "allowed").await;
                host.wait_until(|| host.conns().len() == 2).await;
                assert!(host.conns().iter().any(|c| {
                    c.device_id == device_id(Dev::Unpaired)
                        && c.role == host_core::auth::Role::Viewer
                }));
            } else {
                assert_eq!(
                    auth(&mut viewer, pb::auth_result::Status::Denied)
                        .await
                        .error
                        .unwrap()
                        .code,
                    140
                );
                assert_eq!(viewer.close_code().await, 2);
                closed(&host, &request, "denied").await;
                assert_eq!(host.conns().len(), 1);
                assert!(host
                    .conns()
                    .iter()
                    .all(|c| c.device_id == device_id(Dev::A)));
            }
            assert!(
                matches!(roster::seat(&host.mgr), Seat::Occupied { controller }
                if controller.device_id == device_id(Dev::A))
            );
            assert!(host.mgr.session_control().approval_pending().is_empty());
        }
    }
}

#[tokio::test]
async fn shared_viewers_still_obey_explicit_approval_policies() {
    for reason in [
        "attended_mode",
        "intranet_confirmation",
        "device_requires_confirmation",
        "local_activity",
    ] {
        let mut opts = options(reason == "attended_mode", false);
        opts.options.auth_policy.intranet_confirm_each = reason == "intranet_confirmation";
        let host = WireHost::start_with(opts).await;
        let _controller = if matches!(reason, "attended_mode" | "intranet_confirmation") {
            allow(&host, Dev::A).await
        } else {
            let mut controller = host.connect().await;
            assert_eq!(
                controller.hello().await.1.status,
                pb::auth_result::Status::Ok as i32
            );
            controller
        };
        host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Occupied { .. }))
            .await;
        host.pair(
            Dev::B,
            if reason == "local_activity" {
                Permission::Control
            } else {
                Permission::ViewOnly
            },
        );
        if reason == "device_requires_confirmation" {
            let mut device = host
                .store
                .get(&identity(Dev::B).fingerprint())
                .unwrap()
                .unwrap();
            device.require_confirm = true;
            host.store.upsert(device).unwrap();
        }
        if reason == "local_activity" {
            host.platform.local_input();
            host.wait_until(|| activity::active_within_60s(&host.mgr))
                .await;
        }
        let before = requested(&host).len();
        let mut viewer = host.connect_as(Dev::B, 0).await;
        let (_, result) = viewer
            .hello_with(pb::ClientHello {
                intent: pb::Intent::View as i32,
                ..client_hello()
            })
            .await;
        assert_eq!(
            result.status,
            pb::auth_result::Status::PendingApproval as i32
        );
        host.wait_until(|| requested(&host).len() == before + 1)
            .await;
        let request = requested(&host).pop().unwrap();
        assert_eq!(request["reasons"], json!([reason]));
        respond(&host, &request, true);
        assert_eq!(
            auth(&mut viewer, pb::auth_result::Status::Ok).await.role,
            pb::Role::Viewer as i32
        );
    }
}

#[tokio::test]
async fn control_intent_to_shared_session_still_confirms_new_device() {
    let host = WireHost::start_with(options(false, true)).await;
    let mut controller = host.connect().await;
    assert_eq!(
        controller.hello().await.1.status,
        pb::auth_result::Status::Ok as i32
    );
    host.pair(Dev::Unpaired, Permission::Control);
    let (mut client, request) = pending(&host, Dev::Unpaired).await;
    assert_eq!(request["reasons"], json!(["device_requires_confirmation"]));
    respond(&host, &request, false);
    auth(&mut client, pb::auth_result::Status::Denied).await;
}

#[tokio::test]
async fn new_viewer_without_controller_still_requires_confirmation() {
    for lingering in [false, true] {
        let host = WireHost::start_with(options(false, true)).await;
        if lingering {
            let mut controller = host.connect().await;
            assert_eq!(
                controller.hello().await.1.status,
                pb::auth_result::Status::Ok as i32
            );
            controller
                .conn
                .close(0u32.into(), b"leave controller seat reserved");
            host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
                .await;
        }
        host.pair(Dev::Unpaired, Permission::ViewOnly);
        let mut viewer = host.connect_as(Dev::Unpaired, 0).await;
        let (_, result) = viewer
            .hello_with(pb::ClientHello {
                intent: pb::Intent::View as i32,
                ..client_hello()
            })
            .await;
        assert_eq!(
            result.status,
            pb::auth_result::Status::PendingApproval as i32
        );
        host.wait_until(|| requested(&host).len() == 1).await;
        let request = requested(&host).pop().unwrap();
        assert_eq!(request["reasons"], json!(["device_requires_confirmation"]));
        respond(&host, &request, false);
        auth(&mut viewer, pb::auth_result::Status::Denied).await;
    }
}

#[tokio::test]
async fn new_device_cutoff_is_inclusive_and_configuration_is_live() {
    let host = WireHost::start_with(options(false, true)).await;
    let state: Value =
        serde_json::from_slice(&std::fs::read(host.state_dir.join("approval-state.json")).unwrap())
            .unwrap();
    let since = state["since_us"].as_u64().unwrap();
    let id = device_id(Dev::Unpaired);
    assert!(!approval::confirm_new_device(&host.mgr, &id, None));
    assert!(!approval::confirm_new_device(
        &host.mgr,
        &id,
        Some(since - 1)
    ));
    assert!(approval::confirm_new_device(&host.mgr, &id, Some(since)));
    host.config
        .update(&json!({"security": {"confirm_new_devices": false}}))
        .unwrap();
    assert!(!approval::confirm_new_device(&host.mgr, &id, Some(since)));
}

struct ConnectionOverride {
    allow: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl ConnectionApprover for ConnectionOverride {
    async fn request(&self, _: ConnectionApprovalRequest) -> bool {
        self.calls.fetch_add(1, Ordering::Relaxed);
        self.allow
    }
}

#[tokio::test]
async fn explicit_connection_approver_precedes_local_prompts() {
    for allowed in [false, true] {
        let calls = Arc::new(AtomicUsize::new(0));
        let mut opts = options(true, false);
        opts.connection_approver = Some(Arc::new(ConnectionOverride {
            allow: allowed,
            calls: calls.clone(),
        }));
        let host = WireHost::start_with(opts).await;
        host.events.set_listening(false);
        let mut client = host.connect().await;
        assert_eq!(
            client.hello().await.1.status,
            pb::auth_result::Status::PendingApproval as i32
        );
        auth(
            &mut client,
            if allowed {
                pb::auth_result::Status::Ok
            } else {
                pb::auth_result::Status::Denied
            },
        )
        .await;
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(requested(&host).is_empty());
        assert!(host.mgr.session_control().approval_pending().is_empty());
    }
}

struct PairOverride {
    allow: bool,
    calls: Arc<AtomicUsize>,
}

#[async_trait]
impl PairApprover for PairOverride {
    async fn request(&self, _: PairApprovalRequest) -> PairApproval {
        self.calls.fetch_add(1, Ordering::Relaxed);
        PairApproval {
            allow: self.allow,
            permission: Permission::ViewOnly,
        }
    }
}

struct RuntimeTask(tokio::task::JoinHandle<host_core::RuntimeResult<()>>);
impl Drop for RuntimeTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

async fn run_pairing(host: &WireHost, manager: &PairingManager) -> pb::PairResult {
    let ticket = manager.start(rsp_core::clock::unix_micros()).unwrap();
    let mut client = host.connect_as(Dev::Unpaired, 0).await;
    client
        .send(pb::ClientHello {
            intent: pb::Intent::Pair as i32,
            ..client_hello()
        })
        .await;
    client
        .next_matching(|msg| matches!(msg, pb::envelope::Msg::ServerHello(_)).then_some(()))
        .await;
    let mut pairing = rsp_core::pairing::AppPairing::new(
        &ticket.code,
        identity(Dev::Unpaired).fingerprint(),
        host.host_fp,
        "new phone",
        pb::Platform::Ios,
    )
    .unwrap();
    client.send(pairing.start()).await;
    let spake = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::PairSpake(spake) => Some(spake.clone()),
            _ => None,
        })
        .await;
    let (spake, confirm) = pairing.on_spake(spake).unwrap();
    client.send(spake).await;
    client.send(confirm).await;
    let confirm = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::PairConfirm(confirm) => Some(confirm.clone()),
            _ => None,
        })
        .await;
    pairing.on_confirm(confirm).unwrap();
    let waiting = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::PairResult(result) => Some(result.clone()),
            _ => None,
        })
        .await;
    assert_eq!(waiting.status, pb::pair_result::Status::WaitingLocal as i32);
    let result = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::PairResult(result) => Some(result.clone()),
            _ => None,
        })
        .await;
    let allowed = result.status == pb::pair_result::Status::Ok as i32;
    assert_eq!(client.close_code().await, if allowed { 0 } else { 2 });
    result
}

#[tokio::test]
async fn every_successful_local_pair_is_seen_including_explicit_approvers() {
    for allowed in [false, true] {
        let mut host = WireHost::start_with(options(false, true)).await;
        host.events.set_listening(false);
        let calls = Arc::new(AtomicUsize::new(0));
        let bound = HostRuntime::new(
            host.platform.clone(),
            RuntimeOptions {
                bind: Some("127.0.0.1:0".parse().unwrap()),
                store_dir: host.state_dir.join("store"),
                ..Default::default()
            },
        )
        .with_manager(host.mgr.clone())
        .with_store(host.store.clone())
        .with_pairing_approver(Arc::new(PairOverride {
            allow: allowed,
            calls: calls.clone(),
        }))
        .bind()
        .await
        .unwrap();
        host.addrs[0] = bound
            .listener
            .bound()
            .into_iter()
            .find(|(_, _, ty)| *ty == rsp_transport::TransportKind::Quic)
            .unwrap()
            .0;
        host.host_fp = bound.security.identity.fp();
        let pairing = bound.security.pairing.clone();
        let _runtime = RuntimeTask(tokio::spawn(bound.run()));
        let result = run_pairing(&host, &pairing).await;
        assert_eq!(
            result.status,
            if allowed {
                pb::pair_result::Status::Ok as i32
            } else {
                pb::pair_result::Status::Denied as i32
            }
        );
        assert_eq!(calls.load(Ordering::Relaxed), 1);
        assert!(requested(&host).is_empty());
        assert_eq!(
            approval::confirm_new_device(&host.mgr, &device_id(Dev::Unpaired), Some(u64::MAX)),
            !allowed
        );
        let stored = host
            .store
            .get(&identity(Dev::Unpaired).fingerprint())
            .unwrap();
        assert_eq!(stored.is_some(), allowed);
        if allowed {
            assert_eq!(stored.unwrap().permission, Permission::ViewOnly);
            let mut client = host.connect_as(Dev::Unpaired, 0).await;
            assert_eq!(
                client.hello().await.1.status,
                pb::auth_result::Status::Ok as i32
            );
            assert!(requested(&host).is_empty());
        }
    }
}

#[tokio::test]
async fn graceful_close_and_replacement_keep_their_end_reasons() {
    for replaced in [false, true] {
        let host = WireHost::start_with(options(false, false)).await;
        let mut client = host.connect().await;
        assert_eq!(
            client.hello().await.1.status,
            pb::auth_result::Status::Ok as i32
        );
        if replaced {
            let mgr = host.mgr.clone();
            let key = host.conns()[0].key;
            let ending = tokio::spawn(async move { mgr.end(key, EndReason::Replaced).await });
            let reason = client
                .next_matching(|msg| match msg {
                    pb::envelope::Msg::Goodbye(goodbye) => Some(goodbye.reason),
                    _ => None,
                })
                .await;
            assert_eq!(reason, pb::goodbye::Reason::Replaced as i32);
            assert_eq!(client.close_code().await, 3);
            assert!(ending.await.unwrap());
        } else {
            client.goodbye(pb::goodbye::Reason::UserClosed).await;
            assert_eq!(client.close_code().await, 0);
        }
        host.wait_until(|| host.conns().is_empty()).await;
        // PeerClosed creates a linger seat; these two explicit ends must not.
        assert!(matches!(roster::seat(&host.mgr), Seat::Empty { .. }));
    }
}
