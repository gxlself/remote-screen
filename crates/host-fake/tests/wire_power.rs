use std::time::Duration;

use host_core::platform::{ConsoleState, SessionActionKind};
use host_core::session::{handlers::power, manager::EndReason};
use rsp_core::{features, pb};

mod wire_support;
use wire_support::{WireClient, WireHost};

fn start() -> pb::StartStream {
    pb::StartStream {
        codec: pb::Codec::Hevc as i32,
        fps: 30,
        bitrate_kbps: 2000,
        max_width: 640,
        ..Default::default()
    }
}

fn is_error_or_goodbye(m: &pb::envelope::Msg) -> bool {
    matches!(
        m,
        pb::envelope::Msg::Error(_) | pb::envelope::Msg::Goodbye(_)
    )
}

async fn denied_console(host: &WireHost, state: ConsoleState, detail: &str) {
    host.platform.set_console_state(state);
    let mut client = host.connect().await;
    let (server, auth) = client.hello().await;
    assert_eq!(server.protocol, features::PROTOCOL_VERSION);
    assert_eq!(auth.status, pb::auth_result::Status::Denied as i32);
    let error = auth.error.expect("console denial carries an error");
    assert_eq!(error.code, 190);
    assert_eq!(
        error.detail.get("console").map(String::as_str),
        Some(detail)
    );
    assert_eq!(client.close_code().await, 2);
    assert!(host.conns().is_empty());
}

async fn admitted(host: &WireHost) -> (WireClient, pb::AuthResult) {
    let mut client = host.connect().await;
    let (_, auth) = client.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    host.wait_until(|| host.platform.keep_awake()).await;
    (client, auth)
}

#[tokio::test]
async fn login_window_denies_before_admission() {
    let host = WireHost::start().await;
    denied_console(&host, ConsoleState::LoginWindow, "login_window").await;
}

#[tokio::test]
async fn other_user_denies_before_admission() {
    let host = WireHost::start().await;
    denied_console(&host, ConsoleState::OtherUser, "other_user").await;
}

#[tokio::test]
async fn locked_and_unknown_consoles_are_allowed() {
    for state in [ConsoleState::Locked, ConsoleState::Unknown] {
        let host = WireHost::start().await;
        host.platform.set_console_state(state);
        let (mut client, auth) = admitted(&host).await;
        assert_eq!(auth.status, pb::auth_result::Status::Ok as i32, "{state:?}");
        client
            .send(pb::Goodbye {
                reason: pb::goodbye::Reason::UserClosed as i32,
                message: String::new(),
            })
            .await;
        assert_eq!(client.close_code().await, 0, "{state:?}");
    }
}

#[tokio::test]
async fn leaving_console_stops_capture_sends_error_then_goodbye() {
    let host = WireHost::start().await;
    let (mut client, _) = admitted(&host).await;
    client.start_stream(start()).await;
    host.wait_until(|| host.platform.active_captures() == 1)
        .await;

    host.platform.set_console_state(ConsoleState::OtherUser);
    power::check_console_now(&host.mgr);

    let error = client
        .next_matching(|m| match m {
            pb::envelope::Msg::Error(e) => Some(e.clone()),
            _ => None,
        })
        .await;
    assert_eq!(error.code, 190);
    assert_eq!(
        error.detail.get("console").map(String::as_str),
        Some("other_user")
    );
    let goodbye = client
        .next_matching(|m| match m {
            pb::envelope::Msg::Goodbye(g) => Some(g.clone()),
            _ => None,
        })
        .await;
    assert_eq!(goodbye.reason, pb::goodbye::Reason::Unspecified as i32);
    assert_eq!(client.close_code().await, 0);
    tokio::time::timeout(
        Duration::from_secs(1),
        host.wait_until(|| host.platform.active_captures() == 0 && host.conns().is_empty()),
    )
    .await
    .expect("capture and roster are cleaned up within one second");
}

#[tokio::test]
async fn polling_detects_leaving_the_console() {
    let host = WireHost::start().await;
    let (mut client, _) = admitted(&host).await;
    client.start_stream(start()).await;
    host.wait_until(|| host.platform.active_captures() == 1)
        .await;

    host.platform.set_console_state(ConsoleState::LoginWindow);
    tokio::time::sleep(Duration::from_millis(2500)).await;
    let error = client
        .next_matching(|m| match m {
            pb::envelope::Msg::Error(e) => Some(e.clone()),
            _ => None,
        })
        .await;
    assert_eq!(error.code, 190);
    assert_eq!(
        error.detail.get("console").map(String::as_str),
        Some("login_window")
    );
}

#[tokio::test]
async fn locking_the_console_does_not_end_the_session() {
    let host = WireHost::start().await;
    let (mut client, _) = admitted(&host).await;
    client.start_stream(start()).await;

    host.platform.set_console_state(ConsoleState::Locked);
    power::check_console_now(&host.mgr);
    client
        .expect_none_matching(Duration::from_millis(500), is_error_or_goodbye)
        .await;
    assert_eq!(host.conns().len(), 1);
    assert_eq!(host.platform.active_captures(), 1);
}

#[tokio::test]
async fn keep_awake_is_reference_counted_per_admitted_connection() {
    let host = WireHost::start().await;
    let (mut client, _) = admitted(&host).await;
    assert!(host.platform.keep_awake());
    assert_eq!(host.platform.keep_awake_calls(), vec![true]);
    client
        .send(pb::Goodbye {
            reason: pb::goodbye::Reason::UserClosed as i32,
            message: String::new(),
        })
        .await;
    assert_eq!(client.close_code().await, 0);
    host.wait_until(|| !host.platform.keep_awake() && host.conns().is_empty())
        .await;

    let (mut second, _) = admitted(&host).await;
    assert!(host.platform.keep_awake());
    second
        .send(pb::Goodbye {
            reason: pb::goodbye::Reason::UserClosed as i32,
            message: String::new(),
        })
        .await;
    assert_eq!(second.close_code().await, 0);
    host.wait_until(|| !host.platform.keep_awake() && host.conns().is_empty())
        .await;
    assert_eq!(
        host.platform.keep_awake_calls(),
        vec![true, false, true, false]
    );
}

#[tokio::test]
async fn starting_a_stream_wakes_the_display() {
    let host = WireHost::start().await;
    let (mut client, _) = admitted(&host).await;
    client.start_stream(start()).await;
    assert!(host
        .platform
        .session_actions()
        .contains(&SessionActionKind::WakeDisplay));
}

#[test]
fn leaving_the_console_uses_the_unspecified_goodbye_reason() {
    assert_eq!(
        EndReason::ConsoleLeft.goodbye(),
        Some(pb::goodbye::Reason::Unspecified)
    );
    assert_eq!(EndReason::ConsoleLeft.close_code(), 0);
}
