use host_core::audit::{AuditPath, HostAuditRecord};
use host_core::{
    auth::Role,
    clock::Clock,
    session::{handlers::roster, takeover::Seat},
};
use rsp_core::pb;
use serde_json::json;
use std::time::Duration;

mod wire_support;
use wire_support::{client_hello, device_id, identity, Dev, WireClient, WireHost, WireOptions};

async fn audited_host() -> WireHost {
    WireHost::start_with(WireOptions {
        audit: true,
        ..Default::default()
    })
    .await
}

fn session_events(host: &WireHost) -> Vec<HostAuditRecord> {
    host.audit_events()
        .into_iter()
        .filter(|record| {
            matches!(
                record.event.as_str(),
                "session_start" | "session_end" | "role_change"
            )
        })
        .collect()
}

fn roster_empty(host: &WireHost) -> bool {
    roster::summaries(&host.mgr).is_empty()
}

async fn wait_ended(host: &WireHost, count: usize) -> Vec<HostAuditRecord> {
    host.wait_until(|| {
        roster_empty(host) && host.conns().is_empty() && session_events(host).len() >= count
    })
    .await;
    let events = session_events(host);
    assert_eq!(events.len(), count, "unexpected session audit: {events:?}");
    events
}

fn assert_ok(auth: &pb::AuthResult, role: pb::Role) {
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, role as i32);
}

fn assert_pair(events: &[HostAuditRecord], duration_s: i64) {
    assert_eq!(events.len(), 2);
    assert_eq!(events[0].event, "session_start");
    assert_eq!(events[0].duration_s, None);
    assert_eq!(events[1].event, "session_end");
    assert_eq!(events[1].duration_s, Some(duration_s));
    assert!(events[0].ts_us.unwrap() < events[1].ts_us.unwrap());
    for event in events {
        assert_eq!(event.device_id.as_deref(), Some(device_id(Dev::A).as_str()));
        assert_eq!(event.path, Some(AuditPath::Lan));
        assert_eq!(event.detail, Some(json!({"role": "controller"})));
        assert_eq!(event.ts, event.ts_us.unwrap() / 1_000_000);
        assert!(event.seq.is_some());
        assert!(event.host_id.is_none());
        assert!(event.server.is_none());
        assert!(event.user.is_none());
    }
}

async fn background(host: &WireHost, client: &mut WireClient) {
    client.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| {
        matches!(roster::seat(&host.mgr), Seat::Lingering { .. }) && host.conns().is_empty()
    })
    .await;
    let events = session_events(host);
    assert_eq!(events.len(), 1);
    assert_eq!(events[0].event, "session_start");
}

#[tokio::test]
async fn ordinary_session_records_one_start_and_end() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    let (_, auth) = a.hello().await;
    assert_ok(&auth, pb::Role::Controller);
    assert_eq!(host.conns()[0].path, pb::PathKind::Lan);
    host.clock.skip(Duration::from_secs(5));
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    let events = wait_ended(&host, 2).await;
    assert_pair(&events, 5);
    // Reading the queue must leave it available to later assertions/upload.
    assert_eq!(session_events(&host).len(), 2);
}

#[tokio::test]
async fn background_resume_keeps_one_session_and_original_start_time() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    let (_, auth) = a.hello().await;
    assert_ok(&auth, pb::Role::Controller);
    let original = host.conns()[0].clone();
    host.clock.skip(Duration::from_secs(3));
    background(&host, &mut a).await;
    host.clock.skip(Duration::from_secs(10));

    let mut resumed = host.connect().await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Controller);
    assert!(auth.resumed);
    assert_eq!(host.conns()[0].session_id, original.session_id);
    assert_eq!(host.conns()[0].since_us, original.since_us);
    assert_eq!(session_events(&host).len(), 1);
    host.clock.skip(Duration::from_secs(2));
    resumed.goodbye(pb::goodbye::Reason::UserClosed).await;
    assert_pair(&wait_ended(&host, 2).await, 15);
}

#[tokio::test]
async fn live_connection_resume_does_not_end_the_replaced_connection_session() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    let (_, auth) = a.hello().await;
    host.clock.skip(Duration::from_secs(1));
    let mut resumed = host.connect().await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Controller);
    assert!(auth.resumed);
    let reason = a
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Goodbye(goodbye) => Some(goodbye.reason),
            _ => None,
        })
        .await;
    assert_eq!(reason, pb::goodbye::Reason::Replaced as i32);
    assert_eq!(a.close_code().await, 3);
    host.wait_until(|| host.conns().len() == 1).await;
    assert_eq!(session_events(&host).len(), 1);
    host.clock.skip(Duration::from_secs(2));
    resumed.goodbye(pb::goodbye::Reason::UserClosed).await;
    assert_pair(&wait_ended(&host, 2).await, 3);
}

#[tokio::test]
async fn linger_expiry_records_end_at_flush_time_with_duration_until_disconnect() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    a.hello().await;
    host.clock.skip(Duration::from_secs(4));
    background(&host, &mut a).await;
    host.clock.skip(Duration::from_secs(59));
    host.mgr.tick_all();
    roster::broadcast_session_state(&host.mgr);
    assert!(matches!(roster::seat(&host.mgr), Seat::Lingering { .. }));
    assert_eq!(session_events(&host).len(), 1);
    host.clock.skip(Duration::from_secs(2));
    let expired_at = host.clock.now_micros();
    let events = wait_ended(&host, 2).await;
    assert_pair(&events, 4);
    assert!(events[1].ts_us.unwrap() >= expired_at);
    assert!(events[1].ts_us.unwrap() - events[0].ts_us.unwrap() >= 65_000_000);
    roster::broadcast_session_state(&host.mgr);
    assert_eq!(session_events(&host).len(), 2);
}

#[tokio::test]
async fn ipc_disconnect_flushes_a_lingering_session_once() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    a.hello().await;
    let session = host.conns()[0].session_id.clone();
    host.clock.skip(Duration::from_secs(4));
    background(&host, &mut a).await;
    host.clock.skip(Duration::from_secs(10));
    let disconnected_at = host.clock.now_micros();
    assert!(roster::disconnect(&host.mgr, &session));
    let events = wait_ended(&host, 2).await;
    assert_pair(&events, 4);
    assert!(events[1].ts_us.unwrap() >= disconnected_at);
    assert!(!roster::disconnect(&host.mgr, &session));
    roster::broadcast_session_state(&host.mgr);
    assert_eq!(session_events(&host).len(), 2);
}

#[tokio::test]
async fn handoff_audits_both_role_changes_and_ignores_unchanged_roles() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    assert_ok(&a.hello().await.1, pb::Role::Controller);
    let mut b = host.connect_as(Dev::B, 0).await;
    assert_ok(&b.hello().await.1, pb::Role::Viewer);
    let session_b = host
        .conns()
        .into_iter()
        .find(|conn| conn.device_id == device_id(Dev::B))
        .unwrap();
    assert!(roster::handoff(&host.mgr, &session_b.session_id));
    host.wait_until(|| session_events(&host).len() == 4).await;
    let events = session_events(&host);
    for (dev, from, to) in [
        (Dev::A, "controller", "viewer"),
        (Dev::B, "viewer", "controller"),
    ] {
        let changes: Vec<_> = events
            .iter()
            .filter(|event| {
                event.event == "role_change"
                    && event.device_id.as_deref() == Some(device_id(dev).as_str())
            })
            .collect();
        assert_eq!(changes.len(), 1);
        assert_eq!(changes[0].duration_s, None);
        assert_eq!(changes[0].path, Some(AuditPath::Lan));
        assert_eq!(changes[0].detail, Some(json!({"from": from, "to": to})));
    }
    let b_start = events
        .iter()
        .find(|event| {
            event.event == "session_start"
                && event.device_id.as_deref() == Some(device_id(Dev::B).as_str())
        })
        .unwrap();
    assert_eq!(b_start.detail, Some(json!({"role": "viewer"})));

    // These commands share a FIFO: ending B waits for the unchanged role
    // command, so the final event count also checks that it emitted nothing.
    assert!(host.mgr.set_role(session_b.key, Role::Controller));
    assert!(roster::disconnect(&host.mgr, &session_b.session_id));
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    let events = wait_ended(&host, 6).await;
    for (dev, role) in [(Dev::A, "viewer"), (Dev::B, "controller")] {
        let end = events
            .iter()
            .find(|event| {
                event.event == "session_end"
                    && event.device_id.as_deref() == Some(device_id(dev).as_str())
            })
            .unwrap();
        assert_eq!(end.detail, Some(json!({"role": role})));
    }
}

#[tokio::test]
async fn revoked_active_session_records_end_after_roster_removal() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    a.hello().await;
    host.clock.skip(Duration::from_secs(5));
    roster::on_device_revoked(&host.mgr, identity(Dev::A).fingerprint());
    assert_pair(&wait_ended(&host, 2).await, 5);
}

#[tokio::test]
async fn revoked_lingering_session_flushes_end() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    a.hello().await;
    host.clock.skip(Duration::from_secs(2));
    background(&host, &mut a).await;
    host.clock.skip(Duration::from_secs(10));
    roster::on_device_revoked(&host.mgr, identity(Dev::A).fingerprint());
    assert_pair(&wait_ended(&host, 2).await, 2);
}

#[tokio::test]
async fn fresh_same_device_session_flushes_the_previous_lingering_end() {
    let host = audited_host().await;
    let mut a = host.connect().await;
    a.hello().await;
    host.clock.skip(Duration::from_secs(2));
    background(&host, &mut a).await;
    host.clock.skip(Duration::from_secs(10));
    let mut fresh = host.connect().await;
    let (_, auth) = fresh.hello().await;
    assert!(!auth.resumed);
    host.wait_until(|| session_events(&host).len() == 3).await;
    let events = session_events(&host);
    let old_end = events.iter().find(|e| e.event == "session_end").unwrap();
    assert_eq!(old_end.duration_s, Some(2));
    assert!(old_end.ts_us.unwrap() > events[0].ts_us.unwrap() + 10_000_000);
    fresh.goodbye(pb::goodbye::Reason::UserClosed).await;
    let events = wait_ended(&host, 4).await;
    assert_eq!(
        events.iter().filter(|e| e.event == "session_start").count(),
        2
    );
    assert_eq!(
        events.iter().filter(|e| e.event == "session_end").count(),
        2
    );
}

#[tokio::test]
async fn auditing_disabled_preserves_normal_session_lifecycle() {
    let host = WireHost::start().await;
    assert!(host.audit.is_none());
    let mut a = host.connect().await;
    assert_ok(&a.hello().await.1, pb::Role::Controller);
    host.clock.skip(Duration::from_secs(5));
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| roster_empty(&host) && host.conns().is_empty())
        .await;
    assert!(host.audit_events().is_empty());
}
