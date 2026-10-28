use async_trait::async_trait;
use host_core::{
    auth::Role,
    ipc::{self, SessionRole},
    session::{
        handlers::roster,
        handshake::{ConnectionApprovalRequest, ConnectionApprover},
        manager::ConnKey,
        roster::{ParticipantInfo, RosterRole, SessionId},
        takeover::Seat,
    },
};
use rsp_core::{features, pb};
use serde_json::json;
use std::{
    sync::{
        atomic::{AtomicUsize, Ordering},
        Arc,
    },
    time::Duration,
};
mod wire_support;
use wire_support::{client_hello, device_id, identity, Dev, WireClient, WireHost, WireOptions};

fn assert_ok(auth: &pb::AuthResult, role: pb::Role) {
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, role as i32);
    assert_eq!(auth.session_token.len(), 16);
}

async fn state(
    client: &mut WireClient,
    mut matches: impl FnMut(&pb::SessionState) -> bool,
) -> pb::SessionState {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::SessionState(state) if matches(state) => Some(state.clone()),
            _ => None,
        })
        .await
}

fn controller_is(state: &pb::SessionState, dev: Dev) -> bool {
    state
        .controller
        .as_ref()
        .is_some_and(|p| p.device_id == device_id(dev))
}

async fn assert_closed(client: &mut WireClient, reason: pb::goodbye::Reason, code: u64) {
    let actual = client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::Goodbye(g) => Some(g.reason),
            _ => None,
        })
        .await;
    assert_eq!(actual, reason as i32);
    assert_eq!(client.close_code().await, code);
}

fn sessions_changed(host: &WireHost, count: usize) {
    assert!(host.events.events().iter().any(|event| {
        matches!(event, ipc::Event::SessionsChanged(value)
            if value["sessions"].as_array().is_some_and(|sessions| sessions.len() == count))
    }));
}

fn session(host: &WireHost, dev: Dev) -> String {
    host.mgr
        .session_control()
        .sessions_list()
        .into_iter()
        .find(|s| s.device_id == device_id(dev))
        .unwrap()
        .session_id
}

fn roster_contains(host: &WireHost, dev: Dev) -> bool {
    roster::with_roster(&host.mgr, |r| {
        !r.sessions_for_device(&device_id(dev)).is_empty()
    })
}

#[tokio::test]
async fn different_devices_watch_and_same_device_replaces_its_controller() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (hello, auth) = a.hello().await;
    assert_ok(&auth, pb::Role::Controller);
    assert_eq!(
        hello.features & roster::FEATURES,
        features::RESUME | features::MULTI_VIEWER
    );
    let initial = state(&mut a, |s| controller_is(s, Dev::A)).await;
    assert_eq!(initial.your_role, pb::Role::Controller as i32);
    assert!(initial.viewers.is_empty());
    assert!(!initial.input_paused_local);
    sessions_changed(&host, 1);
    host.events.take();

    let mut b = host.connect_as(Dev::B, 0).await;
    let (_, auth) = b.hello().await;
    assert_ok(&auth, pb::Role::Viewer);
    for (client, role) in [(&mut a, pb::Role::Controller), (&mut b, pb::Role::Viewer)] {
        let shared = state(client, |s| controller_is(s, Dev::A) && s.viewers.len() == 1).await;
        assert_eq!(shared.your_role, role as i32);
        assert_eq!(shared.viewers[0].device_id, device_id(Dev::B));
    }
    sessions_changed(&host, 2);
    assert_eq!(host.conns().len(), 2);
    let old_session = session(&host, Dev::A);
    host.events.take();

    let mut new_a = host.connect_as(Dev::A, 0).await;
    let (_, auth) = new_a.hello().await;
    assert_ok(&auth, pb::Role::Controller);
    assert!(!auth.resumed);
    assert_closed(&mut a, pb::goodbye::Reason::Replaced, 3).await;
    let replacement = state(&mut new_a, |s| {
        controller_is(s, Dev::A) && s.viewers.len() == 1
    })
    .await;
    state(&mut b, |s| {
        s.controller == replacement.controller && s.viewers.len() == 1
    })
    .await;
    assert_ne!(session(&host, Dev::A), old_session);
    assert_eq!(host.conns().len(), 2);
    sessions_changed(&host, 2);
}

#[tokio::test]
async fn viewer_limit_hot_reload_and_same_device_viewer_replacement() {
    let mut options = WireOptions::default();
    options.config.access.max_viewers = 1;
    let host = WireHost::start_with(options).await;
    let mut a = host.connect().await;
    a.hello().await;
    let mut b = host.connect_as(Dev::B, 0).await;
    assert_ok(&b.hello().await.1, pb::Role::Viewer);
    let mut c = host.connect_as(Dev::C, 0).await;
    let (_, denied) = c.hello().await;
    assert_eq!(denied.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(denied.error.unwrap().code, 142);

    let mut new_b = host.connect_as(Dev::B, 0).await;
    assert_ok(&new_b.hello().await.1, pb::Role::Viewer);
    assert_closed(&mut b, pb::goodbye::Reason::Replaced, 3).await;
    host.config
        .update(&json!({"access": {"max_viewers": 2}}))
        .unwrap();
    host.wait_until(|| {
        roster::with_roster(&host.mgr, |r| {
            r.clone()
                .admit(
                    ConnKey(u64::MAX),
                    ParticipantInfo {
                        device_id: device_id(Dev::C),
                        device_name: "wire-C".into(),
                        user: String::new(),
                        path: pb::PathKind::Lan,
                        can_control: true,
                    },
                    RosterRole::Viewer,
                    host.mgr.now_us(),
                )
                .is_ok()
        })
    })
    .await;
    let mut c = host.connect_as(Dev::C, 0).await;
    assert_ok(&c.hello().await.1, pb::Role::Viewer);
    let mut d = host.connect_as(Dev::D, 0).await;
    assert_eq!(d.hello().await.1.error.unwrap().code, 142);
    assert_eq!(host.mgr.session_control().sessions_list().len(), 3);

    // Without a controller auth rule 8 permits this VIEW request; admission must
    // still enforce the viewer limit and map the roster rejection to 142.
    a.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| !roster_contains(&host, Dev::A)).await;
    let mut e = host.connect_as(Dev::E, 0).await;
    let (_, denied) = e
        .hello_with(pb::ClientHello {
            intent: pb::Intent::View as i32,
            ..client_hello()
        })
        .await;
    assert_eq!(denied.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(denied.error.unwrap().code, 142);
}

#[tokio::test]
async fn default_capacity_is_one_controller_and_three_viewers_across_listeners() {
    let host = WireHost::start_with(WireOptions {
        listeners: 2,
        ..Default::default()
    })
    .await;
    let mut clients = Vec::new();
    for (index, dev) in [Dev::A, Dev::B, Dev::C, Dev::D].into_iter().enumerate() {
        let mut client = host.connect_as(dev, index % 2).await;
        let (_, auth) = client.hello().await;
        assert_ok(
            &auth,
            if index == 0 {
                pb::Role::Controller
            } else {
                pb::Role::Viewer
            },
        );
        clients.push(client);
    }
    let mut e = host.connect_as(Dev::E, 1).await;
    assert_eq!(e.hello().await.1.error.unwrap().code, 142);
    assert_eq!(host.conns().len(), 4);
}

#[derive(Default)]
struct Allow {
    requests: AtomicUsize,
}
#[async_trait]
impl ConnectionApprover for Allow {
    async fn request(&self, _: ConnectionApprovalRequest) -> bool {
        self.requests.fetch_add(1, Ordering::Relaxed);
        true
    }
}

async fn approved(client: &mut WireClient) -> pb::AuthResult {
    client
        .next_matching(|msg| match msg {
            pb::envelope::Msg::AuthResult(auth) => Some(auth.clone()),
            _ => None,
        })
        .await
}

#[tokio::test]
async fn background_resume_rotates_token_and_skips_attended_approval() {
    let approver = Arc::new(Allow::default());
    let mut options = WireOptions {
        connection_approver: Some(approver.clone()),
        ..Default::default()
    };
    options.options.auth_policy.attended_mode = true;
    let host = WireHost::start_with(options).await;
    let mut a = host.connect().await;
    assert_eq!(
        a.hello().await.1.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let first = approved(&mut a).await;
    assert_ok(&first, pb::Role::Controller);
    let original_session = session(&host, Dev::A);
    let mut b = host.connect_as(Dev::B, 0).await;
    assert_eq!(
        b.hello().await.1.status,
        pb::auth_result::Status::PendingApproval as i32
    );
    assert_ok(&approved(&mut b).await, pb::Role::Viewer);
    assert_eq!(approver.requests.load(Ordering::Relaxed), 2);

    a.goodbye(pb::goodbye::Reason::Background).await;
    state(&mut b, |s| s.controller.is_none()).await;
    assert!(matches!(roster::seat(&host.mgr), Seat::Lingering { .. }));
    let mut resumed = host.connect().await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: first.session_token.clone(),
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Controller);
    assert!(auth.resumed);
    assert_ne!(auth.session_token, first.session_token);
    assert_eq!(approver.requests.load(Ordering::Relaxed), 2);
    assert_eq!(session(&host, Dev::A), original_session);
    state(&mut b, |s| controller_is(s, Dev::A)).await;

    let mut replay = host.connect().await;
    assert_eq!(
        replay
            .hello_with(pb::ClientHello {
                resume_token: first.session_token,
                ..client_hello()
            })
            .await
            .1
            .status,
        pb::auth_result::Status::PendingApproval as i32
    );
    let fresh = approved(&mut replay).await;
    assert_ok(&fresh, pb::Role::Controller);
    assert!(!fresh.resumed);
    assert_eq!(approver.requests.load(Ordering::Relaxed), 3);
    assert_closed(&mut resumed, pb::goodbye::Reason::Replaced, 3).await;
}

#[tokio::test]
async fn lingering_controller_expires_after_sixty_seconds() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (_, auth) = a.hello().await;
    a.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    assert!(roster_contains(&host, Dev::A));
    host.clock.skip(Duration::from_secs(59));
    host.mgr.tick_all();
    assert!(roster_contains(&host, Dev::A));
    host.clock.skip(Duration::from_secs(2));
    host.wait_until(|| !roster_contains(&host, Dev::A)).await;
    assert!(host.mgr.session_control().sessions_list().is_empty());
    assert!(matches!(roster::seat(&host.mgr), Seat::Empty { .. }));
    let mut fresh = host.connect().await;
    let (_, auth) = fresh
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Controller);
    assert!(!auth.resumed);
}

#[tokio::test]
async fn ipc_lists_demotes_and_disconnects_and_emits_changes() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let mut b = host.connect_as(Dev::B, 0).await;
    b.hello().await;
    state(&mut b, |s| s.viewers.len() == 1).await;
    let control = host.mgr.session_control();
    let sessions = control.sessions_list();
    assert_eq!(sessions.len(), 2);
    for session in &sessions {
        let info = host
            .conns()
            .into_iter()
            .find(|c| c.session_id == session.session_id)
            .unwrap();
        assert_eq!(session.connected_at_us, info.since_us);
    }
    assert_eq!(
        sessions
            .iter()
            .find(|s| s.device_id == device_id(Dev::A))
            .unwrap()
            .role,
        SessionRole::Controller
    );
    assert_eq!(
        sessions
            .iter()
            .find(|s| s.device_id == device_id(Dev::B))
            .unwrap()
            .role,
        SessionRole::Viewer
    );
    host.events.take();
    assert!(control.set_view_only(&session(&host, Dev::A)));
    state(&mut a, |s| {
        s.your_role == pb::Role::Viewer as i32 && s.controller.is_none()
    })
    .await;
    host.wait_until(|| host.conns().iter().all(|c| c.role == Role::Viewer))
        .await;
    assert!(control
        .sessions_list()
        .iter()
        .all(|s| s.role == SessionRole::Viewer));
    sessions_changed(&host, 2);
    host.events.take();
    assert!(control.disconnect(&session(&host, Dev::B)));
    assert_closed(&mut b, pb::goodbye::Reason::HostClosedByLocalUser, 0).await;
    host.wait_until(|| control.sessions_list().len() == 1).await;
    sessions_changed(&host, 1);
    assert!(!control.disconnect("unknown"));
    assert!(!control.set_view_only("unknown"));
}

#[tokio::test]
async fn revocation_ends_live_connections_and_invalidates_lingering_tokens() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (_, auth_a) = a.hello().await;
    let mut b = host.connect_as(Dev::B, 0).await;
    b.hello().await;
    roster::on_device_revoked(&host.mgr, identity(Dev::B).fingerprint());
    assert!(!roster_contains(&host, Dev::B));
    assert_closed(&mut b, pb::goodbye::Reason::Revoked, 6).await;
    assert!(!roster_contains(&host, Dev::B));
    assert_eq!(host.mgr.session_control().sessions_list().len(), 1);

    a.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    roster::on_device_revoked(&host.mgr, identity(Dev::A).fingerprint());
    assert!(!roster_contains(&host, Dev::A));
    let mut fresh = host.connect().await;
    let (_, auth) = fresh
        .hello_with(pb::ClientHello {
            resume_token: auth_a.session_token,
            ..client_hello()
        })
        .await;
    assert!(!auth.resumed);
}

#[tokio::test]
async fn ipc_disconnect_removes_reserved_seat_and_token() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (_, auth) = a.hello().await;
    let id = session(&host, Dev::A);
    a.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    assert!(host.mgr.session_control().disconnect(&id));
    assert!(!roster_contains(&host, Dev::A));
    assert!(!host.mgr.session_control().disconnect(&id));
    let mut fresh = host.connect().await;
    let (_, auth) = fresh
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert!(!auth.resumed);
}

#[tokio::test]
async fn takeover_helpers_handoff_and_active_resume_preserve_controller() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (_, auth_a) = a.hello().await;
    let mut b = host.connect_as(Dev::B, 0).await;
    let (_, auth_b) = b.hello().await;
    let b_key = host
        .conns()
        .iter()
        .find(|c| c.device_id == device_id(Dev::B))
        .unwrap()
        .key;
    let candidate = roster::candidate(&host.mgr, b_key).unwrap();
    assert_eq!(candidate.session_id, session(&host, Dev::B));
    assert!(candidate.can_control);
    assert!(roster::candidate(&host.mgr, ConnKey(u64::MAX)).is_none());
    assert!(!roster::handoff(&host.mgr, "unknown"));
    assert!(roster::handoff(&host.mgr, &candidate.session_id));
    state(&mut a, |s| {
        s.your_role == pb::Role::Viewer as i32 && controller_is(s, Dev::B)
    })
    .await;
    state(&mut b, |s| s.your_role == pb::Role::Controller as i32).await;
    assert!(
        matches!(roster::seat(&host.mgr), Seat::Occupied { controller } if controller.device_id == device_id(Dev::B))
    );
    host.wait_until(|| {
        host.conns()
            .iter()
            .find(|c| c.key == b_key)
            .is_some_and(|c| c.role == Role::Controller)
    })
    .await;

    let stream = b
        .start_stream(pb::StartStream {
            codec: pb::Codec::Hevc as i32,
            quality: pb::Quality::Clear as i32,
            fps: 30,
            max_width: 640,
            ..Default::default()
        })
        .await;
    let prefs = roster::with_roster(&host.mgr, |r| {
        r.clone()
            .resume(
                ConnKey(u64::MAX),
                &auth_b.session_token,
                &device_id(Dev::B),
                host.mgr.now_us(),
            )
            .unwrap()
            .prefs
    });
    assert_eq!(prefs.display_id, stream.display_id);
    assert_eq!(prefs.quality, pb::Quality::Clear as i32);

    let mut resumed = host.connect_as(Dev::B, 0).await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth_b.session_token,
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Controller);
    assert!(auth.resumed);
    assert_closed(&mut b, pb::goodbye::Reason::Replaced, 3).await;
    assert_eq!(session(&host, Dev::B), candidate.session_id);

    let mut wrong_device = host.connect_as(Dev::C, 0).await;
    let (_, auth) = wrong_device
        .hello_with(pb::ClientHello {
            resume_token: auth_a.session_token,
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Viewer);
    assert!(!auth.resumed);
    assert_eq!(host.conns().len(), 3);
    assert_eq!(
        roster::with_roster(&host.mgr, |r| r.role_of(&SessionId(candidate.session_id))),
        Some(RosterRole::Controller)
    );
}

#[tokio::test]
async fn unexpected_disconnect_keeps_seat_for_resume() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (_, auth) = a.hello().await;
    a.conn.close(0u32.into(), b"network disconnected");
    host.wait_until(|| matches!(roster::seat(&host.mgr), Seat::Lingering { .. }))
        .await;
    let mut resumed = host.connect().await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: auth.session_token,
            ..client_hello()
        })
        .await;
    assert_ok(&auth, pb::Role::Controller);
    assert!(auth.resumed);
}
