use std::time::Duration;

use host_core::{platform::SessionActionKind, session::handlers::idle};
use rsp_core::{features, pb};

mod wire_support;
use wire_support::{client_hello, WireClient, WireHost, WireOptions};

const WARN_WINDOW: Duration = Duration::from_millis(1500);
const IDLE_TIMEOUT_MIN: u16 = 2;

async fn start() -> WireHost {
    let mut options = WireOptions::default();
    options.config.access.idle_timeout_min = IDLE_TIMEOUT_MIN;
    WireHost::start_with(options).await
}

async fn connect(host: &WireHost, dev: wire_support::Dev) -> (WireClient, pb::AuthResult) {
    let mut client = host.connect_as(dev, 0).await;
    let (hello, auth) = client.hello().await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_ne!(hello.features & features::SESSION_ACTIONS, 0);
    (client, auth)
}

async fn next_idle_warning(client: &mut WireClient) -> pb::IdleWarning {
    client
        .next_matching(|message| match message {
            pb::envelope::Msg::IdleWarning(warning) => Some(*warning),
            _ => None,
        })
        .await
}

async fn expect_no_idle_warning(client: &mut WireClient, within: Duration) {
    client
        .expect_none_matching(within, |message| {
            matches!(message, pb::envelope::Msg::IdleWarning(_))
        })
        .await;
}

async fn expect_no_error(client: &mut WireClient, within: Duration) {
    client
        .expect_none_matching(within, |message| {
            matches!(message, pb::envelope::Msg::Error(_))
        })
        .await;
}

fn lock_screen_count(host: &WireHost) -> usize {
    host.platform
        .session_actions()
        .into_iter()
        .filter(|kind| *kind == SessionActionKind::LockScreen)
        .count()
}

fn lock_on_end(enable: bool) -> pb::SessionAction {
    pb::SessionAction {
        kind: SessionActionKind::LockOnEnd as i32,
        enable,
    }
}

fn session_action(kind: SessionActionKind) -> pb::SessionAction {
    pb::SessionAction {
        kind: kind as i32,
        enable: false,
    }
}

fn controller_key(host: &WireHost) -> host_core::session::manager::ConnKey {
    host.conns()
        .into_iter()
        .find(|info| info.role == host_core::auth::Role::Controller)
        .expect("a controller is connected")
        .key
}

async fn wait_for_close(client: &mut WireClient, reason: pb::goodbye::Reason, code: u64) {
    let goodbye = client
        .next_matching(|message| match message {
            pb::envelope::Msg::Goodbye(goodbye) => Some(goodbye.clone()),
            _ => None,
        })
        .await;
    assert_eq!(goodbye.reason, reason as i32);
    assert_eq!(client.close_code().await, code);
}

#[tokio::test]
async fn controller_warns_sixty_seconds_before_idle_timeout() {
    let host = start().await;
    let (mut controller, _) = connect(&host, wire_support::Dev::A).await;

    host.clock.skip(Duration::from_secs(61));
    let warning = tokio::time::timeout(WARN_WINDOW, next_idle_warning(&mut controller))
        .await
        .expect("idle warning arrives within 1.5 seconds");
    assert!((58..=60).contains(&warning.seconds_left), "{warning:?}");
    assert!(!warning.lock_on_end);

    host.clock.skip(Duration::from_secs(60));
    wait_for_close(&mut controller, pb::goodbye::Reason::IdleTimeout, 5).await;
}

#[tokio::test]
async fn controller_input_restarts_the_warning_window() {
    let host = start().await;
    let (mut controller, _) = connect(&host, wire_support::Dev::A).await;
    let key = controller_key(&host);

    host.clock.skip(Duration::from_secs(61));
    let warning = tokio::time::timeout(WARN_WINDOW, next_idle_warning(&mut controller))
        .await
        .expect("first idle warning arrives");
    assert!((58..=60).contains(&warning.seconds_left), "{warning:?}");

    assert!(host.mgr.post(
        key,
        host_core::session::handlers::FeatureEvent::Idle(idle::Event::ControllerInput)
    ));
    tokio::time::sleep(Duration::from_millis(100)).await;
    // The timer arm polls at most once per second, so leave half a second of
    // margin before the reset deadline while still crossing the old expiry.
    host.clock.skip(Duration::from_secs(58));
    tokio::time::sleep(WARN_WINDOW).await;
    expect_no_idle_warning(&mut controller, Duration::from_millis(100)).await;
    assert_eq!(host.conns().len(), 1);

    host.clock.skip(Duration::from_secs(2));
    tokio::time::timeout(WARN_WINDOW, next_idle_warning(&mut controller))
        .await
        .expect("a fresh warning follows the restarted timer");
}

#[tokio::test]
async fn viewers_have_no_idle_timer() {
    let host = start().await;
    let (mut controller, _) = connect(&host, wire_support::Dev::A).await;
    let (mut viewer, _) = connect(&host, wire_support::Dev::B).await;

    host.clock.skip(Duration::from_secs(200));
    tokio::time::sleep(WARN_WINDOW).await;
    expect_no_idle_warning(&mut viewer, Duration::from_millis(100)).await;
    assert_eq!(host.conns().len(), 1);

    // The controller still times out independently of the viewer.
    wait_for_close(&mut controller, pb::goodbye::Reason::IdleTimeout, 5).await;
}

#[tokio::test]
async fn session_actions_run_only_for_the_controller() {
    let host = start().await;
    let (mut controller, _) = connect(&host, wire_support::Dev::A).await;
    let (mut viewer, _) = connect(&host, wire_support::Dev::B).await;

    controller
        .send(session_action(SessionActionKind::LockScreen))
        .await;
    host.wait_until(|| lock_screen_count(&host) == 1).await;
    controller
        .send(session_action(SessionActionKind::WakeDisplay))
        .await;
    host.wait_until(|| {
        host.platform
            .session_actions()
            .contains(&SessionActionKind::WakeDisplay)
    })
    .await;
    controller
        .send(session_action(SessionActionKind::CtrlAltDel))
        .await;
    host.wait_until(|| {
        host.platform
            .session_actions()
            .contains(&SessionActionKind::CtrlAltDel)
    })
    .await;

    controller
        .send(pb::SessionAction {
            kind: 99,
            enable: false,
        })
        .await;
    let unsupported = controller
        .next_matching(|message| match message {
            pb::envelope::Msg::Error(error) => Some(error.clone()),
            _ => None,
        })
        .await;
    assert_eq!(unsupported.code, 199);
    assert_eq!(unsupported.message, "unsupported session action");

    viewer
        .send(session_action(SessionActionKind::LockScreen))
        .await;
    tokio::time::sleep(Duration::from_millis(200)).await;
    assert_eq!(lock_screen_count(&host), 1);
    expect_no_error(&mut viewer, Duration::from_millis(100)).await;

    host.platform.fail_session_actions(true);
    controller
        .send(session_action(SessionActionKind::LockScreen))
        .await;
    let error = controller
        .next_matching(|message| match message {
            pb::envelope::Msg::Error(error) => Some(error.clone()),
            _ => None,
        })
        .await;
    assert_eq!(error.code, 199);
    assert_eq!(
        error.detail.get("action").map(String::as_str),
        Some("lock_screen")
    );
}

#[tokio::test]
async fn user_disconnect_locks_only_when_lock_on_end_is_enabled() {
    let host = start().await;
    let (mut controller, _) = connect(&host, wire_support::Dev::A).await;
    controller.send(lock_on_end(true)).await;
    controller.goodbye(pb::goodbye::Reason::UserClosed).await;
    host.wait_until(|| lock_screen_count(&host) == 1).await;
    assert_eq!(controller.close_code().await, 0);

    host.platform.fail_session_actions(false);
    let baseline = lock_screen_count(&host);
    let (mut second, _) = connect(&host, wire_support::Dev::B).await;
    second.goodbye(pb::goodbye::Reason::UserClosed).await;
    assert_eq!(second.close_code().await, 0);
    host.wait_until(|| host.conns().is_empty()).await;
    tokio::time::sleep(Duration::from_millis(100)).await;
    assert_eq!(lock_screen_count(&host), baseline);
}

#[tokio::test]
async fn background_disconnect_defers_lock_until_linger_expires() {
    let host = start().await;
    let (mut controller, _) = connect(&host, wire_support::Dev::A).await;
    controller.send(lock_on_end(true)).await;
    controller.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| host.conns().is_empty()).await;
    assert_eq!(lock_screen_count(&host), 0);

    host.clock.skip(Duration::from_secs(61));
    host.wait_until(|| lock_screen_count(&host) == 1).await;
}

#[tokio::test]
async fn resume_during_linger_cancels_the_pending_lock() {
    let host = start().await;
    let (mut controller, first_auth) = connect(&host, wire_support::Dev::A).await;
    controller.send(lock_on_end(true)).await;
    controller.goodbye(pb::goodbye::Reason::Background).await;
    host.wait_until(|| host.conns().is_empty()).await;

    let mut resumed = host.connect().await;
    let (_, auth) = resumed
        .hello_with(pb::ClientHello {
            resume_token: first_auth.session_token,
            ..client_hello()
        })
        .await;
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert!(auth.resumed);
    host.clock.skip(Duration::from_secs(61));
    tokio::time::sleep(WARN_WINDOW).await;
    assert_eq!(lock_screen_count(&host), 0);
}

#[tokio::test]
async fn warning_lock_on_end_follows_configuration() {
    let mut options = WireOptions::default();
    options.config.access.idle_timeout_min = IDLE_TIMEOUT_MIN;
    options.config.access.lock_on_end = true;
    let host = WireHost::start_with(options).await;
    let (mut controller, _) = connect(&host, wire_support::Dev::A).await;

    host.clock.skip(Duration::from_secs(61));
    let warning = tokio::time::timeout(WARN_WINDOW, next_idle_warning(&mut controller))
        .await
        .expect("idle warning includes the lock-on-end setting");
    assert!((58..=60).contains(&warning.seconds_left), "{warning:?}");
    assert!(warning.lock_on_end);
}
