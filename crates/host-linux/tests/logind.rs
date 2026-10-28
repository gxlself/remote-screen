#![cfg(target_os = "linux")]

use std::{
    collections::HashMap,
    future::Future,
    os::unix::net::UnixStream,
    sync::{Arc, Mutex},
    time::Duration,
};

use host_core::platform::ConsoleState;
use host_linux::{
    logind::{keep_awake, KeepAwakeGuard, Logind, LogindError},
    sys_model::LogindSession,
};
use tokio::{io::AsyncReadExt, net::unix::pipe, sync::Notify};
use zbus::{
    connection::Builder, fdo, message::Header, Connection, Guid, OwnedFd, OwnedObjectPath,
    OwnedVariant,
};

const LOGIN1: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const USER_PATH: &str = "/org/freedesktop/login1/user/_1000";
const SEAT_PATH: &str = "/org/freedesktop/login1/seat/seat0";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const REQUEST: &str = "org.freedesktop.portal.Request";
const UID: u32 = 1000;
const PID: u32 = 1234;
type Options = HashMap<String, OwnedVariant>;
type SessionList = Vec<(String, u32, String, String, OwnedObjectPath)>;

async fn within<T>(future: impl Future<Output = T>) -> T {
    tokio::time::timeout(Duration::from_secs(10), future)
        .await
        .expect("logind mock test timed out")
}

fn object_path(path: impl Into<String>) -> OwnedObjectPath {
    OwnedObjectPath::try_from(path.into()).unwrap()
}

fn session_path(id: &str) -> String {
    format!("{MANAGER_PATH}/session/{id}")
}

#[derive(Debug, Clone)]
struct SessionData {
    id: String,
    uid: u32,
    seat: String,
    class: String,
    kind: String,
    active: bool,
    locked: bool,
    remote: bool,
}

impl SessionData {
    fn graphical(id: &str) -> Self {
        Self {
            id: id.into(),
            uid: UID,
            seat: "seat0".into(),
            class: "user".into(),
            kind: "wayland".into(),
            active: true,
            locked: false,
            remote: false,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
enum Lookup {
    Id(String),
    Pid(u32),
    User(u32),
    List,
}

#[derive(Debug, Clone, PartialEq, Eq)]
struct InhibitCall {
    what: String,
    who: String,
    why: String,
    mode: String,
}

#[derive(Default)]
struct LoginState {
    sessions: Vec<SessionData>,
    pid_session: Option<String>,
    user_display: Option<String>,
    seat_active: Option<String>,
    lookups: Vec<Lookup>,
    locks: Vec<String>,
    inhibitors: Vec<InhibitCall>,
    readers: Vec<pipe::Receiver>,
    fail_inhibit: bool,
    fail_properties: bool,
}

type LoginShared = Arc<Mutex<LoginState>>;

#[derive(Debug, zbus::DBusError)]
#[zbus(prefix = "org.freedesktop.login1", impl_display = true)]
enum LoginError {
    #[zbus(name = "NoSuchSession")]
    SessionMissing(String),
    #[zbus(name = "NoSessionForPID")]
    PidOutsideSession(String),
    #[zbus(name = "NoSuchUser")]
    UserMissing(String),
}

struct Manager(LoginShared);

#[zbus::interface(name = "org.freedesktop.login1.Manager")]
impl Manager {
    fn get_session(&self, id: &str) -> Result<OwnedObjectPath, LoginError> {
        let mut state = self.0.lock().unwrap();
        state.lookups.push(Lookup::Id(id.into()));
        state
            .sessions
            .iter()
            .find(|session| session.id == id)
            .map(|session| object_path(session_path(&session.id)))
            .ok_or_else(|| LoginError::SessionMissing(id.into()))
    }

    #[zbus(name = "GetSessionByPID")]
    fn get_session_by_pid(&self, pid: u32) -> Result<OwnedObjectPath, LoginError> {
        let mut state = self.0.lock().unwrap();
        state.lookups.push(Lookup::Pid(pid));
        state
            .pid_session
            .as_deref()
            .map(|id| object_path(session_path(id)))
            .ok_or_else(|| LoginError::PidOutsideSession("outside a session scope".into()))
    }

    fn get_user(&self, uid: u32) -> Result<OwnedObjectPath, LoginError> {
        self.0.lock().unwrap().lookups.push(Lookup::User(uid));
        if uid == UID {
            Ok(object_path(USER_PATH))
        } else {
            Err(LoginError::UserMissing(uid.to_string()))
        }
    }

    fn list_sessions(&self) -> SessionList {
        let mut state = self.0.lock().unwrap();
        state.lookups.push(Lookup::List);
        state
            .sessions
            .iter()
            .map(|session| {
                (
                    session.id.clone(),
                    session.uid,
                    "mock-user".into(),
                    session.seat.clone(),
                    object_path(session_path(&session.id)),
                )
            })
            .collect()
    }

    fn inhibit(&self, what: &str, who: &str, why: &str, mode: &str) -> fdo::Result<OwnedFd> {
        let mut state = self.0.lock().unwrap();
        state.inhibitors.push(InhibitCall {
            what: what.into(),
            who: who.into(),
            why: why.into(),
            mode: mode.into(),
        });
        if state.fail_inhibit {
            return Err(fdo::Error::AccessDenied("inhibitor denied".into()));
        }
        let (writer, reader) = pipe::pipe().unwrap();
        state.readers.push(reader);
        Ok(writer.into_blocking_fd().unwrap().into())
    }
}

struct Session {
    shared: LoginShared,
    id: String,
}

impl Session {
    fn data(&self) -> fdo::Result<SessionData> {
        let state = self.shared.lock().unwrap();
        if state.fail_properties {
            return Err(fdo::Error::Failed("property query failed".into()));
        }
        state
            .sessions
            .iter()
            .find(|session| session.id == self.id)
            .cloned()
            .ok_or_else(|| fdo::Error::UnknownObject(self.id.clone()))
    }
}

#[zbus::interface(name = "org.freedesktop.login1.Session")]
impl Session {
    #[zbus(property)]
    fn id(&self) -> fdo::Result<String> {
        Ok(self.data()?.id)
    }

    #[zbus(property)]
    fn user(&self) -> fdo::Result<(u32, OwnedObjectPath)> {
        let uid = self.data()?.uid;
        Ok((uid, object_path(format!("{MANAGER_PATH}/user/_{uid}"))))
    }

    #[zbus(property)]
    fn class(&self) -> fdo::Result<String> {
        Ok(self.data()?.class)
    }

    #[zbus(property, name = "Type")]
    fn kind(&self) -> fdo::Result<String> {
        Ok(self.data()?.kind)
    }

    #[zbus(property)]
    fn active(&self) -> fdo::Result<bool> {
        Ok(self.data()?.active)
    }

    #[zbus(property)]
    fn locked_hint(&self) -> fdo::Result<bool> {
        Ok(self.data()?.locked)
    }

    #[zbus(property)]
    fn remote(&self) -> fdo::Result<bool> {
        Ok(self.data()?.remote)
    }

    fn lock(&self) {
        self.shared.lock().unwrap().locks.push(self.id.clone());
    }
}

struct User(LoginShared);

#[zbus::interface(name = "org.freedesktop.login1.User")]
impl User {
    #[zbus(property)]
    fn display(&self) -> (String, OwnedObjectPath) {
        match self.0.lock().unwrap().user_display.as_deref() {
            Some(id) => (id.into(), object_path(session_path(id))),
            None => (String::new(), object_path("/")),
        }
    }
}

struct Seat(LoginShared);

#[zbus::interface(name = "org.freedesktop.login1.Seat")]
impl Seat {
    #[zbus(property)]
    fn active_session(&self) -> (String, OwnedObjectPath) {
        match self.0.lock().unwrap().seat_active.as_deref() {
            Some(id) => (id.into(), object_path(session_path(id))),
            None => (String::new(), object_path("/")),
        }
    }
}

struct MockLogind {
    server: Connection,
    client: Connection,
    shared: LoginShared,
}

impl MockLogind {
    async fn start(state: LoginState) -> Self {
        // zbus 6's p2p connections replace busd. Each mock has its own authenticated
        // socketpair and exports the real login1 interfaces without a system daemon.
        let (server, client) = UnixStream::pair().unwrap();
        let shared = Arc::new(Mutex::new(state));
        let mut service = Builder::unix_stream(server)
            .server(Guid::generate())
            .p2p()
            .unique_name(":1.1")
            .name(LOGIN1)
            .serve_at(MANAGER_PATH, Manager(shared.clone()))
            .serve_at(USER_PATH, User(shared.clone()))
            .serve_at(SEAT_PATH, Seat(shared.clone()));
        for session in &shared.lock().unwrap().sessions {
            service = service.serve_at(
                session_path(&session.id),
                Session {
                    shared: shared.clone(),
                    id: session.id.clone(),
                },
            );
        }
        let (server, client) =
            tokio::try_join!(service.build(), Builder::unix_stream(client).p2p().build(),).unwrap();
        client.set_unique_name(":1.2").unwrap();
        Self {
            server,
            client,
            shared,
        }
    }

    async fn own(id: &str) -> Self {
        Self::start(LoginState {
            sessions: vec![SessionData::graphical(id)],
            pid_session: Some(id.into()),
            seat_active: Some(id.into()),
            ..Default::default()
        })
        .await
    }

    async fn connect(&self, xdg: Option<&str>) -> Logind {
        Logind::connect(self.client.clone(), UID, PID, xdg)
            .await
            .unwrap()
    }

    fn lookups(&self) -> Vec<Lookup> {
        self.shared.lock().unwrap().lookups.clone()
    }

    fn change(&self, id: &str, change: impl FnOnce(&mut SessionData)) {
        let mut state = self.shared.lock().unwrap();
        change(
            state
                .sessions
                .iter_mut()
                .find(|session| session.id == id)
                .unwrap(),
        );
    }
}

#[derive(Clone, Default)]
struct PortalConfig {
    code: Option<u32>,
    absent: bool,
    alternate_path: bool,
    hang_method: bool,
}

#[derive(Debug, Clone)]
struct PortalCall {
    window: String,
    flags: u32,
    reason: String,
    token: String,
    path: String,
}

#[derive(Default)]
struct PortalState {
    calls: Vec<PortalCall>,
    closed: Vec<String>,
}

#[derive(Clone, Default)]
struct PortalShared {
    state: Arc<Mutex<PortalState>>,
    changed: Arc<Notify>,
}

struct PortalInhibit {
    config: PortalConfig,
    shared: PortalShared,
}

#[zbus::interface(name = "org.freedesktop.portal.Inhibit")]
impl PortalInhibit {
    async fn inhibit(
        &self,
        window: String,
        flags: u32,
        options: Options,
        #[zbus(connection)] conn: &Connection,
        #[zbus(header)] header: Header<'_>,
    ) -> fdo::Result<OwnedObjectPath> {
        let token = <&str>::try_from(&options["handle_token"])
            .unwrap()
            .to_owned();
        let reason = <&str>::try_from(&options["reason"]).unwrap().to_owned();
        let sender = header
            .sender()
            .unwrap()
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let path = object_path(format!(
            "{DESKTOP}/request/{sender}/{}",
            if self.config.alternate_path {
                "other"
            } else {
                &token
            }
        ));
        conn.object_server()
            .at(
                path.clone(),
                PortalRequest {
                    path: path.to_string(),
                    shared: self.shared.clone(),
                },
            )
            .await?;
        self.shared.state.lock().unwrap().calls.push(PortalCall {
            window,
            flags,
            reason,
            token,
            path: path.to_string(),
        });
        self.shared.changed.notify_one();
        if self.config.hang_method {
            return std::future::pending().await;
        }
        if let Some(code) = self.config.code {
            // Deliberately emit before replying, to exercise the subscription race.
            conn.emit_signal(
                None::<&str>,
                path.as_str(),
                REQUEST,
                "Response",
                &(code, Options::new()),
            )
            .await?;
        }
        Ok(path)
    }
}

struct PortalRequest {
    path: String,
    shared: PortalShared,
}

#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl PortalRequest {
    fn close(&self) {
        self.shared
            .state
            .lock()
            .unwrap()
            .closed
            .push(self.path.clone());
        self.shared.changed.notify_one();
    }
}

struct MockPortal {
    _server: Connection,
    client: Connection,
    shared: PortalShared,
}

impl MockPortal {
    async fn start(config: PortalConfig) -> Self {
        let (server, client) = UnixStream::pair().unwrap();
        let shared = PortalShared::default();
        let mut service = Builder::unix_stream(server)
            .server(Guid::generate())
            .p2p()
            .unique_name(":1.1");
        if !config.absent {
            service = service.name(PORTAL).serve_at(
                DESKTOP,
                PortalInhibit {
                    config,
                    shared: shared.clone(),
                },
            );
        } else {
            // Start an object server so the unavailable portal returns UnknownObject.
            service = service.serve_at(
                "/unused",
                PortalRequest {
                    path: "/unused".into(),
                    shared: shared.clone(),
                },
            );
        }
        let (server, client) =
            tokio::try_join!(service.build(), Builder::unix_stream(client).p2p().build(),).unwrap();
        // The SASL p2p client has no bus-assigned unique name. Set one explicitly
        // via bus-impl so portal request paths use the same naming as a real bus.
        client.set_unique_name(":1.2").unwrap();
        Self {
            _server: server,
            client,
            shared,
        }
    }

    fn calls(&self) -> Vec<PortalCall> {
        self.shared.state.lock().unwrap().calls.clone()
    }

    async fn closed(&self, expected: usize) -> Vec<String> {
        loop {
            let changed = self.shared.changed.notified();
            let closed = self.shared.state.lock().unwrap().closed.clone();
            if closed.len() >= expected {
                return closed;
            }
            changed.await;
        }
    }
}

async fn assert_logind_guard(mock: &MockLogind, guard: KeepAwakeGuard, reason: &str) {
    assert_eq!(guard.kind(), "logind");
    let mut reader = {
        let mut state = mock.shared.lock().unwrap();
        assert_eq!(
            state.inhibitors,
            vec![InhibitCall {
                what: "idle:sleep".into(),
                who: "Remote Host".into(),
                why: reason.into(),
                mode: "block".into(),
            }]
        );
        state.readers.pop().unwrap()
    };
    let mut buffer = [0];
    // The descriptor is live for the entire guard lifetime.
    assert!(
        tokio::time::timeout(Duration::from_millis(20), reader.read(&mut buffer))
            .await
            .is_err()
    );
    drop(guard);
    assert_eq!(
        reader.read(&mut buffer).await.unwrap(),
        0,
        "inhibitor fd must close"
    );
}

#[tokio::test]
async fn find_by_xdg_session_id() {
    within(async {
        let mock = MockLogind::start(LoginState {
            sessions: vec![SessionData::graphical("c2"), SessionData::graphical("c3")],
            pid_session: Some("c3".into()),
            user_display: Some("c3".into()),
            ..Default::default()
        })
        .await;
        let logind = mock.connect(Some("c2")).await;
        assert_eq!(logind.session_path(), session_path("c2"));
        assert_eq!(mock.lookups(), vec![Lookup::Id("c2".into())]);
    })
    .await;
}

#[tokio::test]
async fn find_by_pid_then_user_display() {
    within(async {
        let mock = MockLogind::start(LoginState {
            sessions: vec![SessionData::graphical("c3")],
            user_display: Some("c3".into()),
            ..Default::default()
        })
        .await;
        let logind = mock.connect(None).await;
        assert_eq!(logind.session_path(), session_path("c3"));
        assert_eq!(mock.lookups(), vec![Lookup::Pid(PID), Lookup::User(UID)]);
    })
    .await;
}

#[tokio::test]
async fn find_by_pid() {
    within(async {
        let mock = MockLogind::own("c2").await;
        let logind = mock.connect(Some(" ")).await;
        assert_eq!(logind.session_path(), session_path("c2"));
        assert_eq!(mock.lookups(), vec![Lookup::Pid(PID)]);
    })
    .await;
}

#[tokio::test]
async fn find_by_list() {
    within(async {
        let mut tty = SessionData::graphical("c1");
        tty.kind = "tty".into();
        let mut other = SessionData::graphical("c2");
        other.uid = 1001;
        let mut other_seat = SessionData::graphical("c3");
        other_seat.seat = "seat1".into();
        let mock = MockLogind::start(LoginState {
            sessions: vec![
                tty,
                other,
                other_seat,
                SessionData::graphical("c4"),
                SessionData::graphical("c5"),
            ],
            ..Default::default()
        })
        .await;
        let logind = mock.connect(Some("stale")).await;
        assert_eq!(logind.session_path(), session_path("c4"));
        assert_eq!(
            mock.lookups(),
            vec![
                Lookup::Id("stale".into()),
                Lookup::Pid(PID),
                Lookup::User(UID),
                Lookup::List
            ]
        );
    })
    .await;
}

#[tokio::test]
async fn discovery_rejects_foreign_and_tty_candidates() {
    within(async {
        let mut other = SessionData::graphical("c1");
        other.uid = 1001;
        let mut tty = SessionData::graphical("c2");
        tty.kind = "tty".into();
        let mut x11 = SessionData::graphical("c3");
        x11.kind = "x11".into();
        let mock = MockLogind::start(LoginState {
            sessions: vec![other, tty, x11],
            pid_session: Some("c2".into()),
            user_display: Some("c3".into()),
            ..Default::default()
        })
        .await;
        assert_eq!(
            mock.connect(Some("c1")).await.session_path(),
            session_path("c3")
        );
        assert_eq!(
            mock.lookups(),
            vec![Lookup::Id("c1".into()), Lookup::Pid(PID), Lookup::User(UID)]
        );
    })
    .await;
}

#[tokio::test]
async fn no_session() {
    within(async {
        let mock = MockLogind::start(LoginState::default()).await;
        assert!(matches!(
            Logind::connect(mock.client.clone(), UID, PID, Some("")).await,
            Err(LogindError::NoSession)
        ));
    })
    .await;
}

#[tokio::test]
async fn console_states() {
    within(async {
        let mut greeter = SessionData::graphical("g1");
        greeter.uid = 120;
        greeter.class = "greeter".into();
        let mut other = SessionData::graphical("c3");
        other.uid = 1001;
        let mock = MockLogind::start(LoginState {
            sessions: vec![SessionData::graphical("c2"), greeter, other],
            pid_session: Some("c2".into()),
            seat_active: Some("c2".into()),
            ..Default::default()
        })
        .await;
        let logind = mock.connect(None).await;
        assert_eq!(
            logind.own_session().await.unwrap(),
            LogindSession {
                id: "c2".into(),
                uid: UID,
                class: "user".into(),
                active: true,
                locked_hint: false,
            }
        );
        assert_eq!(logind.console_state().await, ConsoleState::LoggedIn);
        mock.change("c2", |session| session.locked = true);
        assert_eq!(logind.console_state().await, ConsoleState::Locked);
        mock.change("c2", |session| {
            session.locked = false;
            session.active = false;
        });
        mock.shared.lock().unwrap().seat_active = Some("g1".into());
        assert_eq!(
            logind.seat_active().await.unwrap().unwrap().class,
            "greeter"
        );
        assert_eq!(logind.console_state().await, ConsoleState::LoginWindow);
        mock.shared.lock().unwrap().seat_active = Some("c3".into());
        assert_eq!(logind.seat_active().await.unwrap().unwrap().uid, 1001);
        assert_eq!(logind.console_state().await, ConsoleState::OtherUser);
        mock.shared.lock().unwrap().seat_active = None;
        assert_eq!(logind.seat_active().await.unwrap(), None);
        assert_eq!(logind.console_state().await, ConsoleState::LoginWindow);
        mock.server.clone().close().await.unwrap();
        assert_eq!(logind.console_state().await, ConsoleState::Unknown);
    })
    .await;
}

#[tokio::test]
async fn missing_seat_and_query_errors() {
    within(async {
        let mock = MockLogind::own("c2").await;
        let logind = mock.connect(None).await;
        mock.server
            .object_server()
            .remove::<Seat, _>(SEAT_PATH)
            .await
            .unwrap();
        assert_eq!(logind.seat_active().await.unwrap(), None);
        assert_eq!(logind.console_state().await, ConsoleState::LoggedIn);
        mock.server
            .object_server()
            .at(SEAT_PATH, Seat(mock.shared.clone()))
            .await
            .unwrap();
        mock.shared.lock().unwrap().seat_active = Some("missing".into());
        assert_eq!(logind.console_state().await, ConsoleState::Unknown);
        mock.shared.lock().unwrap().seat_active = Some("c2".into());
        mock.shared.lock().unwrap().fail_properties = true;
        assert_eq!(logind.console_state().await, ConsoleState::Unknown);
    })
    .await;
}

#[tokio::test]
async fn lock_calls_session() {
    within(async {
        let mock = MockLogind::own("c2").await;
        mock.connect(None).await.lock().await.unwrap();
        assert_eq!(mock.shared.lock().unwrap().locks, vec!["c2"]);
    })
    .await;
}

#[tokio::test]
async fn session_type() {
    within(async {
        let mock = MockLogind::own("c2").await;
        let logind = mock.connect(None).await;
        assert_eq!(logind.session_type().await.unwrap(), "wayland");
        mock.change("c2", |session| session.kind = "x11".into());
        assert_eq!(logind.session_type().await.unwrap(), "x11");
    })
    .await;
}

#[tokio::test]
async fn keep_awake_portal() {
    within(async {
        let mock = MockLogind::start(LoginState::default()).await;
        let portal = MockPortal::start(PortalConfig {
            code: Some(0),
            ..Default::default()
        })
        .await;
        let guard = keep_awake(Some(&portal.client), &mock.client, "Remote session")
            .await
            .unwrap();
        assert_eq!(guard.kind(), "portal");
        let calls = portal.calls();
        assert_eq!(calls.len(), 1);
        assert_eq!(calls[0].window, "");
        assert_eq!(calls[0].flags, 12);
        assert_eq!(calls[0].reason, "Remote session");
        assert!(!calls[0].token.is_empty());
        assert!(mock.shared.lock().unwrap().inhibitors.is_empty());
        assert!(portal.shared.state.lock().unwrap().closed.is_empty());
        drop(guard);
        assert_eq!(portal.closed(1).await, vec![calls[0].path.clone()]);
    })
    .await;
}

#[tokio::test]
async fn keep_awake_fallback() {
    within(async {
        for config in [
            PortalConfig {
                absent: true,
                ..Default::default()
            },
            PortalConfig {
                code: Some(2),
                ..Default::default()
            },
        ] {
            let mock = MockLogind::start(LoginState::default()).await;
            let portal = MockPortal::start(config.clone()).await;
            let guard = keep_awake(Some(&portal.client), &mock.client, "Remote session")
                .await
                .unwrap();
            assert_logind_guard(&mock, guard, "Remote session").await;
            if !config.absent {
                assert_eq!(portal.closed(1).await, vec![portal.calls()[0].path.clone()]);
            }
        }
    })
    .await;
}

#[tokio::test]
async fn keep_awake_without_session_bus() {
    within(async {
        let mock = MockLogind::start(LoginState::default()).await;
        let guard = keep_awake(None, &mock.client, "No desktop portal")
            .await
            .unwrap();
        assert_logind_guard(&mock, guard, "No desktop portal").await;
    })
    .await;
}

#[tokio::test]
async fn keep_awake_timeout_closes_pending_request() {
    // Check both a missing Response and a method that creates a request but never replies.
    for hang_method in [false, true] {
        within(async {
            let mock = MockLogind::start(LoginState::default()).await;
            let portal = MockPortal::start(PortalConfig {
                hang_method,
                ..Default::default()
            })
            .await;
            let system = mock.client.clone();
            let session = portal.client.clone();
            let pending =
                tokio::spawn(async move { keep_awake(Some(&session), &system, "Timeout").await });
            loop {
                let changed = portal.shared.changed.notified();
                if !portal.calls().is_empty() {
                    break;
                }
                changed.await;
            }
            tokio::time::pause();
            tokio::time::advance(Duration::from_secs(5)).await;
            tokio::time::resume();
            let guard = pending.await.unwrap().unwrap();
            assert_logind_guard(&mock, guard, "Timeout").await;
            assert_eq!(portal.closed(1).await, vec![portal.calls()[0].path.clone()]);
        })
        .await;
    }
}

#[tokio::test]
async fn portal_uses_returned_path_and_unique_tokens() {
    within(async {
        let mock = MockLogind::start(LoginState::default()).await;
        let portal = MockPortal::start(PortalConfig {
            code: Some(0),
            alternate_path: true,
            ..Default::default()
        })
        .await;
        let first = keep_awake(Some(&portal.client), &mock.client, "First")
            .await
            .unwrap();
        drop(first);
        assert_eq!(portal.closed(1).await, vec![portal.calls()[0].path.clone()]);
        // The usual predicted path must differ between simultaneous guards.
        let portal = MockPortal::start(PortalConfig {
            code: Some(0),
            ..Default::default()
        })
        .await;
        let (first, second) = tokio::join!(
            keep_awake(Some(&portal.client), &mock.client, "First"),
            keep_awake(Some(&portal.client), &mock.client, "Second"),
        );
        let (first, second) = (first.unwrap(), second.unwrap());
        assert_eq!(first.kind(), "portal");
        assert_eq!(second.kind(), "portal");
        let calls = portal.calls();
        assert_ne!(calls[0].token, calls[1].token);
        assert_ne!(calls[0].path, calls[1].path);
        drop((first, second));
        assert_eq!(portal.closed(2).await.len(), 2);
    })
    .await;
}

#[tokio::test]
async fn portal_guard_drops_without_current_runtime() {
    within(async {
        let mock = MockLogind::start(LoginState::default()).await;
        let portal = MockPortal::start(PortalConfig {
            code: Some(0),
            ..Default::default()
        })
        .await;
        let guard = keep_awake(Some(&portal.client), &mock.client, "Plain thread")
            .await
            .unwrap();
        let (sent, finished) = tokio::sync::oneshot::channel();
        std::thread::spawn(move || {
            drop(guard);
            sent.send(()).unwrap();
        });
        finished.await.unwrap();
        assert_eq!(portal.closed(1).await, vec![portal.calls()[0].path.clone()]);
    })
    .await;
}

#[tokio::test]
async fn keep_awake_reports_logind_failure() {
    within(async {
        let mock = MockLogind::start(LoginState {
            fail_inhibit: true,
            ..Default::default()
        })
        .await;
        assert!(matches!(
            keep_awake(None, &mock.client, "Denied").await,
            Err(LogindError::DBus(_))
        ));
    })
    .await;
}
