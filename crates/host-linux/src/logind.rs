//! logind session discovery, console state and desktop power inhibitors.

use std::{
    collections::HashMap,
    future::poll_fn,
    os::fd::OwnedFd,
    pin::Pin,
    sync::atomic::{AtomicU64, Ordering},
    time::Duration,
};

use host_core::platform::ConsoleState;
use zbus::{
    export::futures_core::Stream,
    message::Type as MessageType,
    proxy::{CacheProperties, MethodFlags},
    Connection, MatchRule, MessageStream, OwnedObjectPath, OwnedVariant, Proxy, Str,
};

use crate::sys_model::{console_state_from, LogindSession};

const LOGIN1: &str = "org.freedesktop.login1";
const MANAGER_PATH: &str = "/org/freedesktop/login1";
const MANAGER: &str = "org.freedesktop.login1.Manager";
const SESSION: &str = "org.freedesktop.login1.Session";
const USER: &str = "org.freedesktop.login1.User";
const SEAT: &str = "org.freedesktop.login1.Seat";
const SEAT0: &str = "/org/freedesktop/login1/seat/seat0";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const INHIBIT: &str = "org.freedesktop.portal.Inhibit";
const REQUEST: &str = "org.freedesktop.portal.Request";
const PORTAL_TIMEOUT: Duration = Duration::from_secs(5);
const CLOSE_TIMEOUT: Duration = Duration::from_secs(1);

type SessionList = Vec<(String, u32, String, String, OwnedObjectPath)>;
type Options = HashMap<String, OwnedVariant>;

#[derive(Debug, thiserror::Error)]
pub enum LogindError {
    #[error("no graphical session for this user")]
    NoSession,
    #[error("D-Bus: {0}")]
    DBus(String),
}

impl From<zbus::Error> for LogindError {
    fn from(error: zbus::Error) -> Self {
        Self::DBus(error.to_string())
    }
}

/// The user's graphical session on the system bus.
pub struct Logind {
    system: Connection,
    session: OwnedObjectPath,
    uid: u32,
}

impl Logind {
    /// Try XDG session ID, PID, User.Display, then the user's graphical seat0 sessions.
    /// The latter two also work from a systemd user service outside a session scope.
    pub async fn connect(
        system: Connection,
        uid: u32,
        pid: u32,
        xdg_session_id: Option<&str>,
    ) -> Result<Self, LogindError> {
        let manager = login_proxy(&system, MANAGER_PATH, MANAGER).await?;
        if let Some(id) = xdg_session_id.filter(|id| !id.trim().is_empty()) {
            if let Ok(path) = manager
                .call::<_, _, OwnedObjectPath>("GetSession", &(id,))
                .await
            {
                if Self::is_own_graphical_session(&system, &path, uid, 1).await {
                    return Ok(Self {
                        system,
                        session: path,
                        uid,
                    });
                }
            }
        }
        if let Ok(path) = manager
            .call::<_, _, OwnedObjectPath>("GetSessionByPID", &(pid,))
            .await
        {
            if Self::is_own_graphical_session(&system, &path, uid, 2).await {
                return Ok(Self {
                    system,
                    session: path,
                    uid,
                });
            }
        }
        if let Ok(path) = manager
            .call::<_, _, OwnedObjectPath>("GetUser", &(uid,))
            .await
        {
            if let Ok(user) = login_proxy(&system, path.as_str(), USER).await {
                if let Ok((_, path)) = user
                    .get_property::<(String, OwnedObjectPath)>("Display")
                    .await
                {
                    if Self::is_own_graphical_session(&system, &path, uid, 3).await {
                        return Ok(Self {
                            system,
                            session: path,
                            uid,
                        });
                    }
                }
            }
        }
        let sessions: SessionList = manager.call("ListSessions", &()).await?;
        for (_, session_uid, _, seat, path) in sessions {
            if session_uid == uid
                && seat == "seat0"
                && Self::is_own_graphical_session(&system, &path, uid, 4).await
            {
                return Ok(Self {
                    system,
                    session: path,
                    uid,
                });
            }
        }
        Err(LogindError::NoSession)
    }

    async fn is_own_graphical_session(
        system: &Connection,
        path: &OwnedObjectPath,
        uid: u32,
        method: u8,
    ) -> bool {
        if path.as_str() == "/" {
            return false;
        }
        let candidate = async {
            let session = login_proxy(system, path.as_str(), SESSION).await?;
            let (owner, _): (u32, OwnedObjectPath) = session.get_property("User").await?;
            let kind: String = session.get_property("Type").await?;
            if owner != uid || !matches!(kind.as_str(), "wayland" | "x11") {
                return Ok::<_, zbus::Error>(None);
            }
            session.get_property::<String>("Id").await.map(Some)
        }
        .await;
        if let Ok(Some(id)) = candidate {
            tracing::info!(session = %id, method, "found logind graphical session");
            true
        } else {
            false
        }
    }

    pub fn session_path(&self) -> &str {
        self.session.as_str()
    }

    pub async fn own_session(&self) -> Result<LogindSession, LogindError> {
        let session = read_session(&self.system, self.session.as_str()).await?;
        if session.uid != self.uid {
            return Err(LogindError::DBus("session owner changed".into()));
        }
        Ok(session)
    }

    /// Return seat0's active session, or None when seat0 has no active session or is absent.
    pub async fn seat_active(&self) -> Result<Option<LogindSession>, LogindError> {
        let seat = login_proxy(&self.system, SEAT0, SEAT).await?;
        let active = seat
            .get_property::<(String, OwnedObjectPath)>("ActiveSession")
            .await;
        let (_, path) = match active {
            Ok(active) => active,
            Err(error) if missing_seat(&error) => return Ok(None),
            Err(error) => return Err(error.into()),
        };
        if path.as_str() == "/" {
            return Ok(None);
        }
        Ok(Some(read_session(&self.system, path.as_str()).await?))
    }

    /// Always read both snapshots; a failed query must not look like a logged-in console.
    pub async fn console_state(&self) -> ConsoleState {
        let (own, seat) = tokio::join!(self.own_session(), self.seat_active());
        match (own, seat) {
            (Ok(own), Ok(seat)) => console_state_from(Some(&own), seat.as_ref()),
            _ => {
                tracing::debug!("could not query logind console state");
                ConsoleState::Unknown
            }
        }
    }

    pub async fn lock(&self) -> Result<(), LogindError> {
        login_proxy(&self.system, self.session.as_str(), SESSION)
            .await?
            .call::<_, _, ()>("Lock", &())
            .await?;
        Ok(())
    }

    /// The current logind session Type, e.g. "wayland", "x11" or "tty".
    pub async fn session_type(&self) -> Result<String, LogindError> {
        Ok(login_proxy(&self.system, self.session.as_str(), SESSION)
            .await?
            .get_property("Type")
            .await?)
    }
}

async fn login_proxy<'a>(
    system: &Connection,
    path: &'a str,
    interface: &'static str,
) -> zbus::Result<Proxy<'a>> {
    // Console state is polled. Bypass caches so a daemon loss or a user switch is visible
    // even when no PropertiesChanged signal was delivered.
    zbus::proxy::Builder::new(system)
        .destination(LOGIN1)
        .path(path)
        .interface(interface)
        .cache_properties(CacheProperties::No)
        .build()
        .await
}

async fn read_session(system: &Connection, path: &str) -> Result<LogindSession, LogindError> {
    let session = login_proxy(system, path, SESSION).await?;
    let (id, (uid, _), class, active, locked_hint) = tokio::try_join!(
        session.get_property::<String>("Id"),
        session.get_property::<(u32, OwnedObjectPath)>("User"),
        session.get_property::<String>("Class"),
        session.get_property::<bool>("Active"),
        session.get_property::<bool>("LockedHint"),
    )?;
    Ok(LogindSession {
        id,
        uid,
        class,
        active,
        locked_hint,
    })
}

fn missing_seat(error: &zbus::Error) -> bool {
    match error {
        zbus::Error::FDO(error) => matches!(
            **error,
            zbus::fdo::Error::UnknownObject(_) | zbus::fdo::Error::UnknownInterface(_)
        ),
        zbus::Error::MethodError(name, _, _) => {
            name.as_str() == "org.freedesktop.login1.NoSuchSeat"
        }
        _ => false,
    }
}

/// The resource whose lifetime keeps the desktop awake.
pub enum KeepAwake {
    Portal { request: String },
    Logind(OwnedFd),
}

/// Releasing the guard closes the portal request or the logind inhibitor fd.
pub struct KeepAwakeGuard {
    inhibitor: KeepAwake,
    session_bus: Option<Connection>,
}

impl KeepAwakeGuard {
    pub fn kind(&self) -> &'static str {
        match self.inhibitor {
            KeepAwake::Portal { .. } => "portal",
            KeepAwake::Logind(_) => "logind",
        }
    }
}

/// Prefer a desktop portal inhibitor; fall back to logind if it fails within five seconds.
pub async fn keep_awake(
    session_bus: Option<&Connection>,
    system: &Connection,
    reason: &str,
) -> Result<KeepAwakeGuard, LogindError> {
    if let Some(session) = session_bus {
        if let Ok(Ok(guard)) =
            tokio::time::timeout(PORTAL_TIMEOUT, portal_inhibit(session, reason)).await
        {
            return Ok(guard);
        }
        tracing::debug!("portal inhibitor unavailable; trying logind");
    }
    let fd: zbus::OwnedFd = login_proxy(system, MANAGER_PATH, MANAGER)
        .await?
        .call("Inhibit", &("idle:sleep", "Remote Host", reason, "block"))
        .await?;
    Ok(KeepAwakeGuard {
        inhibitor: KeepAwake::Logind(fd.into()),
        session_bus: None,
    })
}

async fn portal_inhibit(session: &Connection, reason: &str) -> zbus::Result<KeepAwakeGuard> {
    static TOKEN: AtomicU64 = AtomicU64::new(0);
    let token = format!(
        "rs_inhibit_{}_{}",
        std::process::id(),
        TOKEN.fetch_add(1, Ordering::Relaxed)
    );
    let options = Options::from([
        ("handle_token".into(), Str::from(token.as_str()).into()),
        ("reason".into(), Str::from(reason).into()),
    ]);
    // Subscribe before Inhibit: Response may arrive before the method reply. Keep the
    // subscription independent of the returned path for portals choosing a different handle.
    let rule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(PORTAL)
        .interface(REQUEST)
        .member("Response")
        .build()?;
    let mut responses = MessageStream::for_match_rule(rule, session, None).await?;

    // Also clean up a request if Inhibit itself times out after the portal created it.
    let mut pending = session.unique_name().map(|sender| KeepAwakeGuard {
        inhibitor: KeepAwake::Portal {
            request: format!(
                "{DESKTOP}/request/{}/{token}",
                sender.as_str().trim_start_matches(':').replace('.', "_")
            ),
        },
        session_bus: Some(session.clone()),
    });
    let reply = session
        .call_method(
            Some(PORTAL),
            DESKTOP,
            Some(INHIBIT),
            "Inhibit",
            &("", 12_u32, options),
        )
        .await?;
    let path: OwnedObjectPath = reply.body().deserialize()?;
    let mut guard = pending.take().unwrap_or_else(|| KeepAwakeGuard {
        inhibitor: KeepAwake::Portal {
            request: path.to_string(),
        },
        session_bus: Some(session.clone()),
    });
    guard.inhibitor = KeepAwake::Portal {
        request: path.to_string(),
    };
    while let Some(message) = poll_fn(|cx| Pin::new(&mut responses).poll_next(cx)).await {
        let message = message?;
        let header = message.header();
        if header.path().map(|path| path.as_str()) != Some(path.as_str())
            || header.sender() != reply.header().sender()
        {
            continue;
        }
        let (code, _): (u32, Options) = message.body().deserialize()?;
        return if code == 0 {
            Ok(guard)
        } else {
            Err(zbus::Error::Failure("portal inhibitor was rejected".into()))
        };
    }
    Err(zbus::Error::Failure("portal response stream ended".into()))
}

impl Drop for KeepAwakeGuard {
    fn drop(&mut self) {
        let (KeepAwake::Portal { request }, Some(session)) =
            (&self.inhibitor, self.session_bus.take())
        else {
            // OwnedFd releases logind's inhibitor when the fields are dropped.
            return;
        };
        let request = request.clone();
        let close = async move {
            let _ = tokio::time::timeout(CLOSE_TIMEOUT, async {
                let proxy = Proxy::new(&session, PORTAL, request, REQUEST).await?;
                proxy
                    .call_with_flags::<_, _, ()>("Close", MethodFlags::NO_REPLY_EXPECTED, &())
                    .await?;
                Ok::<_, zbus::Error>(())
            })
            .await;
        };
        if let Ok(runtime) = tokio::runtime::Handle::try_current() {
            runtime.spawn(close);
        } else if let Ok(runtime) = tokio::runtime::Builder::new_current_thread()
            .enable_all()
            .build()
        {
            // Destructors can also run on a plain caller thread. Only send Close; a
            // reply would depend on the connection's original runtime still running.
            runtime.block_on(close);
        }
    }
}
