use host_core::{
    auth::{Role, SessionSnapshot},
    platform::Platform,
    session::{handlers, manager::EndReason},
};
use rsp_core::{features, pb};
use std::time::Duration;
mod wire_support;
use wire_support::{client_hello, Dev, WireHost, WireOptions};

fn start() -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        fps: 30,
        bitrate_kbps: 2000,
        max_width: 640,
        ..Default::default()
    }
}
fn is_goodbye(m: &pb::envelope::Msg) -> bool {
    matches!(m, pb::envelope::Msg::Goodbye(_))
}
fn goodbye(m: &pb::envelope::Msg) -> Option<i32> {
    if let pb::envelope::Msg::Goodbye(g) = m {
        Some(g.reason)
    } else {
        None
    }
}
#[tokio::test]
async fn hello_registers_one_controller_with_feature_registry_and_token() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    let (server, auth) = a.hello().await;
    let expected_features = handlers::host_features()
        | if host.platform.audio_capture_supported() {
            features::AUDIO_OPUS
        } else {
            0
        };
    assert_eq!(server.features, expected_features);
    assert_eq!(
        server.features & features::AUDIO_OPUS != 0,
        host.platform.audio_capture_supported()
    );
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.session_token.len(), 16);
    assert_eq!(auth.role, pb::Role::Controller as i32);
    let conns = host.conns();
    assert_eq!(conns.len(), 1);
    assert_eq!(conns[0].role, Role::Controller);
    assert!(conns[0].session_id.starts_with('s'));
    assert_eq!(conns[0].session_id.len(), 17);
    assert!(conns[0].session_id[1..]
        .chars()
        .all(|c| c.is_ascii_digit() || ('a'..='f').contains(&c)));
    assert_eq!(conns[0].device_id, wire_support::device_id(Dev::A));
    assert!(!conns[0].resumed);
    assert!(!auth.resumed);
    assert!(conns[0].can_control);
    assert!(host.mgr.session_control().approval_pending().is_empty());
}

#[tokio::test]
async fn quic_prewarm_does_not_replace_streaming_controller() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    a.start_stream(start()).await;
    a.recv_datagram(0x10).await;
    let _warm = host.connect_as(Dev::B, 0).await;
    a.expect_none_matching(Duration::from_millis(500), is_goodbye)
        .await;
    a.recv_datagram(0x10).await;
    assert_eq!(host.conns().len(), 1);
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn probe_does_not_replace_streaming_controller() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    a.start_stream(start()).await;
    let mut b = host.connect_as(Dev::B, 0).await;
    b.send(pb::ClientHello {
        intent: pb::Intent::Probe as i32,
        ..client_hello()
    })
    .await;
    match b.next().await {
        pb::envelope::Msg::ServerHello(h) => {
            assert_eq!(h.features, 0);
            assert!(h.displays.is_empty());
            assert!(h.host_name.is_empty());
            assert!(h.peer.is_some());
        }
        other => panic!("{other:?}"),
    }
    assert_eq!(
        b.next_skipping_session_state("Goodbye", goodbye).await,
        pb::goodbye::Reason::UserClosed as i32
    );
    assert_eq!(b.close_code().await, 0);
    a.expect_none_matching(Duration::from_millis(100), is_goodbye)
        .await;
    a.recv_datagram(0x10).await;
    assert_eq!(host.conns().len(), 1);
}

#[tokio::test]
async fn pairing_and_denied_handshakes_do_not_replace_controller() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    a.start_stream(start()).await;
    let key = host.conns()[0].key;

    let mut denied = host.connect_as(Dev::Unpaired, 0).await;
    let (_, auth) = denied.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(auth.error.unwrap().code, 181);
    assert_eq!(denied.close_code().await, 2);

    let mut pairing = host.connect_as(Dev::Unpaired, 0).await;
    pairing
        .send(pb::ClientHello {
            intent: pb::Intent::Pair as i32,
            ..client_hello()
        })
        .await;
    assert!(matches!(
        pairing.next().await,
        pb::envelope::Msg::ServerHello(_)
    ));
    a.expect_none_matching(Duration::from_millis(100), is_goodbye)
        .await;
    a.recv_datagram(0x10).await;
    assert_eq!(host.conns().len(), 1);
    assert_eq!(host.conns()[0].key, key);
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn pending_approval_does_not_replace_controller() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    a.start_stream(start()).await;
    let key = host.conns()[0].key;
    let mut device = host
        .store
        .get(&wire_support::identity(Dev::B).fingerprint())
        .unwrap()
        .unwrap();
    device.require_confirm = true;
    host.store.upsert(device).unwrap();

    let mut pending = host.connect_as(Dev::B, 0).await;
    let (_, auth) = pending.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::PendingApproval as i32);
    a.expect_none_matching(Duration::from_millis(100), is_goodbye)
        .await;
    a.recv_datagram(0x10).await;
    assert_eq!(host.conns().len(), 1);
    assert_eq!(host.conns()[0].key, key);
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn m0_replaces_only_at_admission_and_stops_capture_before_ok() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    a.start_stream(start()).await;
    a.recv_datagram(0x10).await;
    let mut b = host.connect_as(Dev::A, 0).await;
    let replacing = async {
        let (_, auth) = b.hello().await;
        assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
        assert_eq!(host.platform.active_captures(), 0);
        b.start_stream(start()).await;
        b.recv_datagram(0x10).await;
        assert_eq!(
            a.next_skipping_session_state("Goodbye", goodbye).await,
            pb::goodbye::Reason::Replaced as i32
        );
        assert_eq!(a.close_code().await, 3);
    };
    let monitor = async {
        loop {
            assert!(host.platform.active_captures() <= 1);
            tokio::time::sleep(Duration::from_millis(1)).await;
        }
    };
    tokio::select! { () = replacing => {}, () = monitor => unreachable!() }
    assert_eq!(host.platform.active_captures(), 1);
    assert_eq!(host.conns().len(), 1);
}

#[tokio::test]
async fn all_listeners_share_one_admission_registry() {
    let host = WireHost::start_with(WireOptions {
        listeners: 2,
        ..Default::default()
    })
    .await;
    let mut a = host.connect_as(Dev::A, 0).await;
    a.hello().await;
    let mut b = host.connect_as(Dev::A, 1).await;
    let (_, auth) = b.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(
        a.next_skipping_session_state("Goodbye", goodbye).await,
        pb::goodbye::Reason::Replaced as i32
    );
    assert_eq!(a.close_code().await, 3);
    assert_eq!(host.conns().len(), 1);
    assert_eq!(host.conns()[0].device_id, wire_support::device_id(Dev::A));
}

#[tokio::test]
async fn session_control_lists_changes_role_and_disconnects() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let control = host.mgr.session_control();
    let summaries = control.sessions_list();
    assert_eq!(summaries.len(), 1);
    let id = &summaries[0].session_id;
    assert_eq!(summaries[0].connected_at_us, host.conns()[0].since_us);
    assert!(control.set_view_only(id));
    host.wait_until(|| host.conns()[0].role == Role::Viewer)
        .await;
    assert!(control.disconnect(id));
    assert_eq!(
        a.next_skipping_session_state("Goodbye", goodbye).await,
        pb::goodbye::Reason::HostClosedByLocalUser as i32
    );
    assert_eq!(a.close_code().await, 0);
    assert!(control.sessions_list().is_empty());
    assert!(!control.disconnect("unknown"));
    assert!(!control.set_view_only("unknown"));
}

#[tokio::test]
async fn send_broadcast_and_tick_reach_the_connection_channel() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    let key = host.conns()[0].key;
    assert!(host.mgr.send(key, pb::LocalActivity { active: true }));
    assert!(
        a.next_skipping_session_state("LocalActivity(true)", |message| match message {
            pb::envelope::Msg::LocalActivity(activity) => Some(activity.active),
            _ => None,
        })
        .await
    );
    assert_eq!(
        host.mgr
            .broadcast(pb::LocalActivity { active: false }.into()),
        1
    );
    assert!(
        !a.next_skipping_session_state("LocalActivity(false)", |message| match message {
            pb::envelope::Msg::LocalActivity(activity) => Some(activity.active),
            _ => None,
        })
        .await
    );
    assert_eq!(host.mgr.tick_all(), 1);
}

#[test]
fn end_reason_wire_mapping() {
    use pb::goodbye::Reason as G;
    use EndReason as E;
    let rows = [
        (E::Normal, None, 0, false),
        (E::Peer(G::UserClosed as i32), None, 0, false),
        (E::Peer(G::Background as i32), None, 0, true),
        (E::PeerClosed, None, 0, true),
        (E::LocalUser, Some(G::HostClosedByLocalUser), 0, false),
        (E::Idle, Some(G::IdleTimeout), 5, false),
        (E::Replaced, Some(G::Replaced), 3, false),
        (E::Revoked, Some(G::Revoked), 6, false),
        (E::Background, Some(G::Background), 0, true),
        (E::HostShutdown, Some(G::HostShutdown), 4, false),
        (E::HostUpdating, Some(G::HostUpdating), 4, false),
        (E::PermissionLost, Some(G::PermissionLost), 0, false),
        (E::ConsoleLeft, Some(G::Unspecified), 0, false),
        (E::AuthDenied, None, 2, false),
        (E::Protocol, None, 1, false),
        (E::ProtocolImmediate, None, 1, false),
    ];
    for (reason, goodbye, code, lingers) in rows {
        assert_eq!(reason.goodbye(), goodbye, "{reason:?}");
        assert_eq!(reason.close_code(), code, "{reason:?}");
        assert_eq!(reason.lingers(), lingers, "{reason:?}");
        assert_eq!(reason.waits_for_drain(), reason != E::ProtocolImmediate);
    }
}

#[tokio::test]
async fn end_acknowledges_only_after_unregistration_and_revoked_goodbye() {
    let host = WireHost::start().await;
    let mut a = host.connect().await;
    a.hello().await;
    a.start_stream(start()).await;
    let key = host.conns()[0].key;
    assert!(host.mgr.end(key, EndReason::Revoked).await);
    assert!(host.mgr.info(key).is_none());
    assert_eq!(host.platform.active_captures(), 0);
    assert_eq!(
        a.next_skipping_session_state("Goodbye", goodbye).await,
        pb::goodbye::Reason::Revoked as i32
    );
    assert_eq!(a.close_code().await, 6);
    assert!(!host.mgr.end(key, EndReason::Revoked).await);
    assert_eq!(host.mgr.tick_all(), 0);
}

#[tokio::test]
async fn viewer_limit_reads_live_config() {
    let mut o = WireOptions::default();
    o.options.session_snapshot = SessionSnapshot {
        has_controller: true,
        viewer_count: 1,
    };
    o.config.access.max_viewers = 1;
    let host = WireHost::start_with(o).await;
    let mut a = host.connect().await;
    let (_, denied) = a.hello().await;
    assert_eq!(denied.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(denied.error.unwrap().code, 142);
    assert!(host.conns().is_empty());
    host.config
        .update(&serde_json::json!({"access": {"max_viewers": 2}}))
        .unwrap();
    let mut b = host.connect_as(Dev::B, 0).await;
    let (_, auth) = b.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    assert!(host.conns()[0].can_control);
}

#[tokio::test]
async fn empty_allowed_users_requires_known_local_owner() {
    let host = WireHost::start_with(WireOptions {
        tailnet_users: Some(vec![]),
        ..Default::default()
    })
    .await;
    let mut a = host.connect_user(Dev::A, "alice@example.com").await;
    let (_, denied) = a.hello().await;
    assert_eq!(denied.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(denied.error.unwrap().code, 130);
    host.mgr.set_owner_login(Some("alice@example.com".into()));
    let mut a = host.connect_user(Dev::A, "alice@example.com").await;
    let (_, auth) = a.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    let mut b = host.connect_user(Dev::B, "bob@example.com").await;
    let (_, denied) = b.hello().await;
    assert_eq!(denied.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(denied.error.unwrap().code, 130);
    assert_eq!(host.conns()[0].user, "alice@example.com");
}

#[tokio::test]
async fn cancelling_listener_removes_its_connection_from_shared_registry() {
    let host = WireHost::start().await;
    let mut client = host.connect().await;
    client.hello().await;
    client.start_stream(start()).await;
    let mgr = host.mgr.clone();
    let platform = host.platform.clone();
    drop(host);
    tokio::time::timeout(Duration::from_secs(3), async {
        while !mgr.conns().is_empty() || platform.active_captures() != 0 {
            tokio::time::sleep(Duration::from_millis(5)).await;
        }
    })
    .await
    .expect("cancelled connection is cleaned up");
}
