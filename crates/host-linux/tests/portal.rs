#![cfg(target_os = "linux")]

#[path = "support/mock_portal.rs"]
mod mock_portal;

use std::{
    path::PathBuf,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use host_linux::{
    portal::{PortalError, PortalOptions, PortalSession},
    portal_model::{PortalKind, RestoreTokenStore, StreamDisplay},
};
use mock_portal::{MockConfig, MockPortal, MockVal, Script};

static STATE_SEQUENCE: AtomicU64 = AtomicU64::new(0);

fn state_dir() -> PathBuf {
    let path = std::env::temp_dir().join(format!(
        "remote-screen-portal-{}-{}",
        std::process::id(),
        STATE_SEQUENCE.fetch_add(1, Ordering::Relaxed)
    ));
    std::fs::create_dir_all(&path).expect("create portal state directory");
    path
}

fn options(state_dir: PathBuf, want_input: bool) -> PortalOptions {
    PortalOptions {
        response_timeout: Duration::from_secs(2),
        ..PortalOptions::new(state_dir, want_input)
    }
}

#[tokio::test]
async fn gnome_open() {
    let mock = MockPortal::start(MockConfig::gnome()).await;
    let client = mock.client().await;
    let dir = state_dir();
    let session = PortalSession::open(&client, &options(dir.clone(), true))
        .await
        .expect("open gnome portal");
    assert_eq!(session.grant().kind, PortalKind::RemoteDesktop);
    assert_eq!(session.grant().devices, 3);
    assert_eq!(session.grant().cursor_mode, 4);
    assert_eq!(
        session.grant().displays,
        vec![StreamDisplay {
            id: 1,
            node_id: 42,
            x: 0,
            y: 0,
            width: 1920,
            height: 1080,
            primary: true,
        }]
    );
    assert_eq!(
        mock.calls()
            .iter()
            .map(|call| call.member.as_str())
            .collect::<Vec<_>>(),
        ["CreateSession", "SelectDevices", "SelectSources", "Start"]
    );
    assert_eq!(
        RestoreTokenStore::new(&dir).load(PortalKind::RemoteDesktop),
        Some("tok-1".into())
    );
}

#[tokio::test]
async fn restore_token_reused() {
    let dir = state_dir();
    RestoreTokenStore::new(&dir)
        .save(PortalKind::RemoteDesktop, "old")
        .expect("seed restore token");
    let mock = MockPortal::start(MockConfig::gnome()).await;
    let client = mock.client().await;
    let session = PortalSession::open(&client, &options(dir.clone(), true))
        .await
        .expect("open with restore token");
    drop(session);
    let select = mock.calls_to("SelectDevices");
    assert_eq!(
        select[0]
            .options
            .iter()
            .find(|(key, _)| key == "restore_token")
            .map(|(_, value)| value),
        Some(&MockVal::Str("old".into()))
    );
    assert_eq!(
        select[0]
            .options
            .iter()
            .find(|(key, _)| key == "persist_mode")
            .map(|(_, value)| value),
        Some(&MockVal::U32(2))
    );
    assert_eq!(
        RestoreTokenStore::new(&dir).load(PortalKind::RemoteDesktop),
        Some("tok-1".into())
    );
}

#[tokio::test]
async fn cancel_is_cancelled() {
    let mut config = MockConfig::gnome();
    config.scripts = vec![("Start", Script::Cancel)];
    let mock = MockPortal::start(config).await;
    let client = mock.client().await;
    let dir = state_dir();
    let result = PortalSession::open(&client, &options(dir.clone(), true)).await;
    assert!(matches!(result, Err(PortalError::Cancelled)));
    assert_eq!(
        RestoreTokenStore::new(&dir).load(PortalKind::RemoteDesktop),
        None
    );
}

#[tokio::test]
async fn fail_code() {
    let mut config = MockConfig::gnome();
    config.scripts = vec![("SelectSources", Script::Fail(2))];
    let mock = MockPortal::start(config).await;
    let client = mock.client().await;
    let result = PortalSession::open(&client, &options(state_dir(), true)).await;
    assert!(matches!(result, Err(PortalError::Failed(message)) if message.contains("code 2")));
}

#[tokio::test]
async fn wlr_screencast_only() {
    let mock = MockPortal::start(MockConfig::wlr()).await;
    let client = mock.client().await;
    let session = PortalSession::open(&client, &options(state_dir(), true))
        .await
        .expect("open screencast portal");
    assert_eq!(session.grant().kind, PortalKind::ScreenCast);
    assert_eq!(session.grant().devices, 0);
    assert_eq!(session.grant().cursor_mode, 2);
    assert_eq!(session.grant().displays[0].x, 0);
    let select = mock.calls_to("SelectSources");
    assert!(select[0]
        .options
        .iter()
        .any(|(key, value)| key == "persist_mode" && value == &MockVal::U32(2)));
}

#[tokio::test]
async fn want_input_false() {
    let mock = MockPortal::start(MockConfig::gnome()).await;
    let client = mock.client().await;
    let session = PortalSession::open(&client, &options(state_dir(), false))
        .await
        .expect("open screencast flow");
    assert_eq!(session.grant().kind, PortalKind::ScreenCast);
    assert!(mock
        .calls_to("CreateSession")
        .first()
        .is_some_and(|call| call.iface == "org.freedesktop.portal.ScreenCast"));
}

#[tokio::test]
async fn no_portal() {
    let mut config = MockConfig::gnome();
    config.register_name = false;
    let mock = MockPortal::start(config).await;
    let client = mock.client().await;
    assert_eq!(PortalSession::caps(&client).await, Default::default());
    assert!(matches!(
        PortalSession::open(&client, &options(state_dir(), true)).await,
        Err(PortalError::Unavailable)
    ));
}

#[tokio::test]
async fn timeout_closes_request() {
    let mut config = MockConfig::gnome();
    config.scripts = vec![("Start", Script::Never)];
    let mock = MockPortal::start(config).await;
    let client = mock.client().await;
    let mut opts = options(state_dir(), true);
    opts.response_timeout = Duration::from_millis(300);
    assert!(matches!(
        PortalSession::open(&client, &opts).await,
        Err(PortalError::Timeout)
    ));
    assert_eq!(mock.calls_to("Close").len(), 1);
    assert_eq!(
        mock.calls_to("Close")[0].iface,
        "org.freedesktop.portal.Request"
    );
}

#[tokio::test]
async fn other_handle_path() {
    let mut config = MockConfig::gnome();
    config.scripts = vec![("SelectSources", Script::OtherPath)];
    let mock = MockPortal::start(config).await;
    let client = mock.client().await;
    let session = PortalSession::open(&client, &options(state_dir(), true)).await;
    assert!(session.is_ok());
}

#[tokio::test]
async fn pipewire_remote_and_close() {
    let mock = MockPortal::start(MockConfig::gnome()).await;
    let client = mock.client().await;
    let session = PortalSession::open(&client, &options(state_dir(), true))
        .await
        .expect("open portal");
    let first = session
        .open_pipewire_remote()
        .await
        .expect("first PipeWire fd");
    let second = session
        .open_pipewire_remote()
        .await
        .expect("second PipeWire fd");
    assert_ne!(
        std::os::fd::AsRawFd::as_raw_fd(&first),
        std::os::fd::AsRawFd::as_raw_fd(&second)
    );
    assert_eq!(mock.pipewire_fds(), 2);
    session.close().await.expect("close portal session");
    assert_eq!(mock.calls_to("Close").len(), 1);
    assert_eq!(
        mock.calls_to("Close")[0].iface,
        "org.freedesktop.portal.Session"
    );
}

#[tokio::test]
async fn closed_signal() {
    let mock = MockPortal::start(MockConfig::gnome()).await;
    let client = mock.client().await;
    let session = PortalSession::open(&client, &options(state_dir(), true))
        .await
        .expect("open portal");
    let mut closed = session.closed();
    mock.close_session(session.session_path()).await;
    tokio::time::timeout(Duration::from_secs(5), closed.changed())
        .await
        .expect("closed signal timeout")
        .expect("closed watcher ended");
    assert!(*closed.borrow());
}

#[tokio::test]
async fn missing_session_handle() {
    let mut config = MockConfig::gnome();
    config.scripts = vec![("CreateSession", Script::NoSessionHandle)];
    let mock = MockPortal::start(config).await;
    let client = mock.client().await;
    assert!(matches!(
        PortalSession::open(&client, &options(state_dir(), true)).await,
        Err(PortalError::Failed(message)) if message.contains("session handle")
    ));
}
