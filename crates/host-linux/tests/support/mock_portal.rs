#![allow(dead_code)] // Each integration test imports only the helpers it needs.

use std::{
    collections::{HashMap, HashSet},
    future::poll_fn,
    os::{fd::OwnedFd as StdOwnedFd, unix::fs::DirBuilderExt, unix::net::UnixStream},
    path::PathBuf,
    pin::Pin,
    sync::{
        atomic::{AtomicBool, AtomicU64, Ordering},
        Arc, Mutex,
    },
    time::Duration,
};

use tokio::{net::UnixListener, task::JoinHandle, task::JoinSet};
use zbus::{
    connection::Builder,
    export::futures_core::Stream,
    fdo,
    message::{Header, Type as MessageType},
    object_server::{ResponseDispatchNotifier, SignalEmitter},
    Connection, Guid, Message, MessageStream, OwnedFd, OwnedObjectPath, OwnedVariant, Str, Variant,
};

const DESKTOP: &str = "/org/freedesktop/portal/desktop";
const PORTAL: &str = "org.freedesktop.portal.Desktop";
const RD: &str = "org.freedesktop.portal.RemoteDesktop";
const SC: &str = "org.freedesktop.portal.ScreenCast";
const REQUEST: &str = "org.freedesktop.portal.Request";
const SESSION: &str = "org.freedesktop.portal.Session";
const DBUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

type Options = HashMap<String, OwnedVariant>;

#[derive(Debug, Clone)]
pub struct MockStream {
    pub node_id: u32,
    pub position: Option<(i32, i32)>,
    pub size: Option<(i32, i32)>,
    pub source_type: Option<u32>,
}

#[derive(Debug, Clone, PartialEq)]
pub enum Script {
    Success,
    Cancel,
    Fail(u32),
    NoSessionHandle,
    Never,
    OtherPath,
}

#[derive(Debug, Clone)]
pub struct MockConfig {
    pub register_name: bool,
    pub rd_version: Option<u32>,
    pub rd_devices: u32,
    pub sc_version: u32,
    pub sc_sources: u32,
    pub sc_cursor_modes: u32,
    pub streams: Vec<MockStream>,
    pub start_devices: u32,
    pub restore_token_out: Option<String>,
    pub scripts: Vec<(&'static str, Script)>,
}

impl MockConfig {
    pub fn gnome() -> Self {
        Self {
            register_name: true,
            rd_version: Some(2),
            rd_devices: 7,
            sc_version: 5,
            sc_sources: 7,
            sc_cursor_modes: 7,
            streams: vec![MockStream {
                node_id: 42,
                position: Some((0, 0)),
                size: Some((1920, 1080)),
                source_type: Some(1),
            }],
            start_devices: 3,
            restore_token_out: Some("tok-1".into()),
            scripts: Vec::new(),
        }
    }

    pub fn wlr() -> Self {
        Self {
            rd_version: None,
            rd_devices: 0,
            sc_version: 4,
            sc_sources: 3,
            sc_cursor_modes: 3,
            streams: vec![MockStream {
                node_id: 43,
                position: None,
                size: Some((1920, 1080)),
                source_type: Some(1),
            }],
            start_devices: 0,
            restore_token_out: Some("wlr-1".into()),
            ..Self::gnome()
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub enum MockVal {
    Str(String),
    Path(String),
    U32(u32),
    I32(i32),
    Bool(bool),
    F64(f64),
    Other(String),
}

impl From<&OwnedVariant> for MockVal {
    fn from(value: &OwnedVariant) -> Self {
        match &**value {
            Variant::Str(value) => Self::Str(value.to_string()),
            Variant::ObjectPath(value) => Self::Path(value.to_string()),
            Variant::U32(value) => Self::U32(*value),
            Variant::I32(value) => Self::I32(*value),
            Variant::Bool(value) => Self::Bool(*value),
            Variant::F64(value) => Self::F64(*value),
            other => Self::Other(format!("{other:?}")),
        }
    }
}

#[derive(Debug, Clone, PartialEq)]
pub struct MockCall {
    pub iface: String,
    pub member: String,
    pub path: String,
    pub session: Option<String>,
    pub options: Vec<(String, MockVal)>,
    pub args: Vec<MockVal>,
}

fn portal_call(
    iface: &str,
    member: &str,
    session: Option<&OwnedObjectPath>,
    options: &Options,
    args: Vec<MockVal>,
) -> MockCall {
    let mut options: Vec<_> = options
        .iter()
        .map(|(key, value)| (key.clone(), MockVal::from(value)))
        .collect();
    options.sort_by(|a, b| a.0.cmp(&b.0));
    MockCall {
        iface: iface.into(),
        member: member.into(),
        path: DESKTOP.into(),
        session: session.map(ToString::to_string),
        options,
        args,
    }
}

#[derive(Default)]
struct State {
    calls: Vec<MockCall>,
    sessions: HashSet<String>,
    pipewire_peers: Vec<UnixStream>,
    requests: u64,
    response_tasks: Vec<JoinHandle<()>>,
}

#[derive(Clone)]
struct Shared {
    cfg: Arc<MockConfig>,
    state: Arc<Mutex<State>>,
}

impl Shared {
    fn record(&self, call: MockCall) {
        self.state.lock().expect("mock state lock").calls.push(call);
    }

    fn check_session(&self, session: &str) -> fdo::Result<()> {
        if self
            .state
            .lock()
            .expect("mock state lock")
            .sessions
            .contains(session)
        {
            Ok(())
        } else {
            Err(fdo::Error::InvalidArgs("unknown portal session".into()))
        }
    }

    async fn request(
        &self,
        conn: &Connection,
        header: &Header<'_>,
        call: MockCall,
        options: Options,
    ) -> fdo::Result<ResponseDispatchNotifier<OwnedObjectPath>> {
        let method = call.member.clone();
        let iface = call.iface.clone();
        let session = call.session.clone();
        self.record(call);
        if let Some(session) = session {
            self.check_session(&session)?;
        }
        let script = self
            .cfg
            .scripts
            .iter()
            .find(|(name, _)| *name == method)
            .map(|(_, script)| script.clone())
            .unwrap_or(Script::Success);
        let sender = header
            .sender()
            .ok_or_else(|| fdo::Error::InvalidArgs("missing sender".into()))?
            .as_str()
            .trim_start_matches(':')
            .replace('.', "_");
        let sequence = {
            let mut state = self.state.lock().expect("mock state lock");
            state.requests += 1;
            state.requests
        };
        let token = match options.get("handle_token") {
            Some(token) => <&str>::try_from(token)
                .map_err(|_| fdo::Error::InvalidArgs("handle_token must be a string".into()))?
                .to_owned(),
            None => format!("request{sequence}"),
        };
        let token = if script == Script::OtherPath {
            format!("other{sequence}")
        } else {
            token
        };
        let path = OwnedObjectPath::try_from(format!("{DESKTOP}/request/{sender}/{token}"))
            .map_err(|_| fdo::Error::InvalidArgs("invalid handle_token".into()))?;
        let code = match script {
            Script::Cancel => 1,
            Script::Fail(code) => code,
            _ => 0,
        };
        let mut results = Options::new();
        if code == 0 && script != Script::Never {
            if method == "CreateSession" {
                let token = options
                    .get("session_handle_token")
                    .and_then(|value| <&str>::try_from(value).ok())
                    .ok_or_else(|| {
                        fdo::Error::InvalidArgs("missing string session_handle_token".into())
                    })?;
                let session =
                    OwnedObjectPath::try_from(format!("{DESKTOP}/session/{sender}/{token}"))
                        .map_err(|_| {
                            fdo::Error::InvalidArgs("invalid session_handle_token".into())
                        })?;
                let inserted = conn
                    .object_server()
                    .at(
                        session.clone(),
                        Session {
                            shared: self.clone(),
                            path: session.to_string(),
                        },
                    )
                    .await?;
                if inserted.is_some() {
                    return Err(fdo::Error::InvalidArgs("session already exists".into()));
                }
                self.state
                    .lock()
                    .expect("mock state lock")
                    .sessions
                    .insert(session.to_string());
                if script != Script::NoSessionHandle {
                    // The portal spec deliberately uses a string (s), not an object path (o).
                    results.insert("session_handle".into(), Str::from(session.as_str()).into());
                }
            } else if method == "Start" {
                let streams: Vec<(u32, Options)> = self
                    .cfg
                    .streams
                    .iter()
                    .map(|stream| {
                        let mut properties = Options::new();
                        if let Some(position) = stream.position {
                            properties.insert("position".into(), owned(position));
                        }
                        if let Some(size) = stream.size {
                            properties.insert("size".into(), owned(size));
                        }
                        if let Some(source_type) = stream.source_type {
                            properties.insert("source_type".into(), source_type.into());
                        }
                        (stream.node_id, properties)
                    })
                    .collect();
                results.insert("streams".into(), owned(streams));
                if iface == RD {
                    results.insert("devices".into(), self.cfg.start_devices.into());
                }
                if let Some(token) = &self.cfg.restore_token_out {
                    results.insert("restore_token".into(), Str::from(token.as_str()).into());
                }
            }
        }
        let active = Arc::new(AtomicBool::new(true));
        let inserted = conn
            .object_server()
            .at(
                path.clone(),
                Request {
                    shared: self.clone(),
                    path: path.to_string(),
                    active: active.clone(),
                },
            )
            .await?;
        if inserted.is_some() {
            return Err(fdo::Error::InvalidArgs("request already exists".into()));
        }
        let (reply, dispatched) = ResponseDispatchNotifier::new(path.clone());
        if script != Script::Never {
            let conn = conn.clone();
            let task = tokio::spawn(async move {
                // Do not race the method reply, including on a multithreaded runtime.
                dispatched.await;
                if active.swap(false, Ordering::SeqCst) {
                    conn.emit_signal(
                        None::<&str>,
                        path.as_str(),
                        REQUEST,
                        "Response",
                        &(code, results),
                    )
                    .await
                    .expect("emit mock portal Response");
                    let _ = conn.object_server().remove::<Request, _>(path).await;
                }
            });
            self.state
                .lock()
                .expect("mock state lock")
                .response_tasks
                .push(task);
        }
        Ok(reply)
    }

    async fn close_session(&self, conn: &Connection, path: &str) -> fdo::Result<()> {
        let removed = self
            .state
            .lock()
            .expect("mock state lock")
            .sessions
            .remove(path);
        if removed {
            conn.emit_signal(None::<&str>, path, SESSION, "Closed", &(Options::new(),))
                .await?;
            conn.object_server().remove::<Session, _>(path).await?;
        }
        Ok(())
    }

    fn notify(&self, call: MockCall) -> fdo::Result<()> {
        let session = call.session.clone();
        self.record(call);
        self.check_session(session.as_deref().unwrap_or_default())
    }
}

fn owned<'a>(value: impl Into<Variant<'a>>) -> OwnedVariant {
    OwnedVariant::try_from(value.into()).expect("own mock portal result")
}

struct Request {
    shared: Shared,
    path: String,
    active: Arc<AtomicBool>,
}

#[zbus::interface(name = "org.freedesktop.portal.Request")]
impl Request {
    #[zbus(signal)]
    async fn response(
        emitter: &SignalEmitter<'_>,
        response: u32,
        results: Options,
    ) -> zbus::Result<()>;

    async fn close(&self, #[zbus(connection)] conn: &Connection) -> fdo::Result<()> {
        self.shared.record(MockCall {
            iface: REQUEST.into(),
            member: "Close".into(),
            path: self.path.clone(),
            session: None,
            options: Vec::new(),
            args: Vec::new(),
        });
        self.active.store(false, Ordering::SeqCst);
        conn.object_server()
            .remove::<Self, _>(self.path.as_str())
            .await?;
        Ok(())
    }
}

struct Session {
    shared: Shared,
    path: String,
}

#[zbus::interface(name = "org.freedesktop.portal.Session")]
impl Session {
    #[zbus(signal)]
    async fn closed(emitter: &SignalEmitter<'_>, details: Options) -> zbus::Result<()>;

    async fn close(&self, #[zbus(connection)] conn: &Connection) -> fdo::Result<()> {
        self.shared.record(MockCall {
            iface: SESSION.into(),
            member: "Close".into(),
            path: self.path.clone(),
            session: Some(self.path.clone()),
            options: Vec::new(),
            args: Vec::new(),
        });
        self.shared.close_session(conn, &self.path).await
    }
}

struct ScreenCast(Shared);

#[zbus::interface(name = "org.freedesktop.portal.ScreenCast")]
impl ScreenCast {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.0.cfg.sc_version
    }

    #[zbus(property)]
    fn available_source_types(&self) -> u32 {
        self.0.cfg.sc_sources
    }

    #[zbus(property)]
    fn available_cursor_modes(&self) -> u32 {
        self.0.cfg.sc_cursor_modes
    }

    async fn create_session(
        &self,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> fdo::Result<ResponseDispatchNotifier<OwnedObjectPath>> {
        self.0
            .request(
                conn,
                &header,
                portal_call(SC, "CreateSession", None, &options, Vec::new()),
                options,
            )
            .await
    }

    async fn select_sources(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> fdo::Result<ResponseDispatchNotifier<OwnedObjectPath>> {
        self.0
            .request(
                conn,
                &header,
                portal_call(SC, "SelectSources", Some(&session_handle), &options, vec![]),
                options,
            )
            .await
    }

    async fn start(
        &self,
        session_handle: OwnedObjectPath,
        parent_window: String,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> fdo::Result<ResponseDispatchNotifier<OwnedObjectPath>> {
        self.0
            .request(
                conn,
                &header,
                portal_call(
                    SC,
                    "Start",
                    Some(&session_handle),
                    &options,
                    vec![MockVal::Str(parent_window)],
                ),
                options,
            )
            .await
    }

    async fn open_pipe_wire_remote(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
    ) -> fdo::Result<OwnedFd> {
        self.0.record(portal_call(
            SC,
            "OpenPipeWireRemote",
            Some(&session_handle),
            &options,
            vec![],
        ));
        self.0.check_session(session_handle.as_str())?;
        let (client, peer) = UnixStream::pair()
            .map_err(|err| fdo::Error::Failed(format!("PipeWire socketpair: {err}")))?;
        self.0
            .state
            .lock()
            .expect("mock state lock")
            .pipewire_peers
            .push(peer);
        Ok(StdOwnedFd::from(client).into())
    }
}

struct RemoteDesktop(Shared);

#[zbus::interface(name = "org.freedesktop.portal.RemoteDesktop")]
impl RemoteDesktop {
    #[zbus(property, name = "version")]
    fn version(&self) -> u32 {
        self.0.cfg.rd_version.unwrap_or_default()
    }

    #[zbus(property)]
    fn available_device_types(&self) -> u32 {
        self.0.cfg.rd_devices
    }

    async fn create_session(
        &self,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> fdo::Result<ResponseDispatchNotifier<OwnedObjectPath>> {
        self.0
            .request(
                conn,
                &header,
                portal_call(RD, "CreateSession", None, &options, vec![]),
                options,
            )
            .await
    }

    async fn select_devices(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> fdo::Result<ResponseDispatchNotifier<OwnedObjectPath>> {
        self.0
            .request(
                conn,
                &header,
                portal_call(RD, "SelectDevices", Some(&session_handle), &options, vec![]),
                options,
            )
            .await
    }

    async fn start(
        &self,
        session_handle: OwnedObjectPath,
        parent_window: String,
        options: Options,
        #[zbus(header)] header: Header<'_>,
        #[zbus(connection)] conn: &Connection,
    ) -> fdo::Result<ResponseDispatchNotifier<OwnedObjectPath>> {
        self.0
            .request(
                conn,
                &header,
                portal_call(
                    RD,
                    "Start",
                    Some(&session_handle),
                    &options,
                    vec![MockVal::Str(parent_window)],
                ),
                options,
            )
            .await
    }

    fn notify_pointer_motion(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        dx: f64,
        dy: f64,
    ) -> fdo::Result<()> {
        self.0.notify(portal_call(
            RD,
            "NotifyPointerMotion",
            Some(&session_handle),
            &options,
            vec![MockVal::F64(dx), MockVal::F64(dy)],
        ))
    }

    fn notify_pointer_motion_absolute(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        stream: u32,
        x: f64,
        y: f64,
    ) -> fdo::Result<()> {
        self.0.notify(portal_call(
            RD,
            "NotifyPointerMotionAbsolute",
            Some(&session_handle),
            &options,
            vec![MockVal::U32(stream), MockVal::F64(x), MockVal::F64(y)],
        ))
    }

    fn notify_pointer_button(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        button: i32,
        state: u32,
    ) -> fdo::Result<()> {
        self.0.notify(portal_call(
            RD,
            "NotifyPointerButton",
            Some(&session_handle),
            &options,
            vec![MockVal::I32(button), MockVal::U32(state)],
        ))
    }

    fn notify_pointer_axis(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        dx: f64,
        dy: f64,
    ) -> fdo::Result<()> {
        self.0.notify(portal_call(
            RD,
            "NotifyPointerAxis",
            Some(&session_handle),
            &options,
            vec![MockVal::F64(dx), MockVal::F64(dy)],
        ))
    }

    fn notify_pointer_axis_discrete(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        axis: u32,
        steps: i32,
    ) -> fdo::Result<()> {
        self.0.notify(portal_call(
            RD,
            "NotifyPointerAxisDiscrete",
            Some(&session_handle),
            &options,
            vec![MockVal::U32(axis), MockVal::I32(steps)],
        ))
    }

    fn notify_keyboard_keycode(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        keycode: i32,
        state: u32,
    ) -> fdo::Result<()> {
        self.0.notify(portal_call(
            RD,
            "NotifyKeyboardKeycode",
            Some(&session_handle),
            &options,
            vec![MockVal::I32(keycode), MockVal::U32(state)],
        ))
    }

    fn notify_keyboard_keysym(
        &self,
        session_handle: OwnedObjectPath,
        options: Options,
        keysym: i32,
        state: u32,
    ) -> fdo::Result<()> {
        self.0.notify(portal_call(
            RD,
            "NotifyKeyboardKeysym",
            Some(&session_handle),
            &options,
            vec![MockVal::I32(keysym), MockVal::U32(state)],
        ))
    }
}

// zbus 6 has no busd dependency. These authenticated p2p links form a small private
// bus for zbus clients, including Hello, well-known names and broadcast signals.
// Client zbus Connections stamp their assigned unique name onto outgoing messages.
#[derive(Default)]
struct Bus {
    peers: HashMap<String, Connection>,
    names: HashMap<String, String>,
}

async fn next_message(stream: &mut MessageStream) -> Option<zbus::Result<Message>> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
}

async fn broadcast(bus: &Mutex<Bus>, message: &Message) -> zbus::Result<()> {
    let peers: Vec<_> = bus
        .lock()
        .expect("mock bus lock")
        .peers
        .values()
        .cloned()
        .collect();
    for peer in peers {
        // A client may disconnect while a broadcast is in flight.
        let _ = peer.send(message).await;
    }
    Ok(())
}

async fn name_changed(bus: &Mutex<Bus>, name: &str, old: &str, new: &str) -> zbus::Result<()> {
    let signal = Message::signal(DBUS_PATH, DBUS, "NameOwnerChanged")
        .sender(DBUS)
        .build(&(name, old, new))?;
    broadcast(bus, &signal).await
}

async fn bus_method(
    bus: &Mutex<Bus>,
    conn: &Connection,
    unique: &str,
    message: &Message,
) -> zbus::Result<()> {
    let header = message.header();
    match header.member().map(|name| name.as_str()) {
        Some("Hello") => {
            conn.reply(&header, &(unique,)).await?;
            name_changed(bus, unique, "", unique).await?;
        }
        Some("AddMatch" | "RemoveMatch") => {
            // Broadcast routing is sufficient here; zbus filters local signal subscriptions.
            let _: String = message.body().deserialize()?;
            conn.reply(&header, &()).await?;
        }
        Some("GetNameOwner" | "NameHasOwner") => {
            let name: String = message.body().deserialize()?;
            let owner = {
                let bus = bus.lock().expect("mock bus lock");
                if name == DBUS || bus.peers.contains_key(&name) {
                    Some(name.clone())
                } else {
                    bus.names.get(&name).cloned()
                }
            };
            if header.member().is_some_and(|name| name == "NameHasOwner") {
                conn.reply(&header, &(owner.is_some(),)).await?;
            } else if let Some(owner) = owner {
                conn.reply(&header, &(owner,)).await?;
            } else {
                conn.reply_dbus_error(&header, fdo::Error::NameHasNoOwner(name))
                    .await?;
            }
        }
        Some("RequestName") => {
            let (name, _flags): (String, u32) = message.body().deserialize()?;
            let code = {
                let mut bus = bus.lock().expect("mock bus lock");
                match bus.names.get(&name) {
                    Some(owner) if owner == unique => 4_u32,
                    Some(_) => 3,
                    None => {
                        bus.names.insert(name.clone(), unique.into());
                        1
                    }
                }
            };
            conn.reply(&header, &(code,)).await?;
            if code == 1 {
                name_changed(bus, &name, "", unique).await?;
                conn.emit_signal(Some(unique), DBUS_PATH, DBUS, "NameAcquired", &(name,))
                    .await?;
            }
        }
        Some("ReleaseName") => {
            let name: String = message.body().deserialize()?;
            let code = {
                let mut bus = bus.lock().expect("mock bus lock");
                match bus.names.get(&name) {
                    Some(owner) if owner == unique => {
                        bus.names.remove(&name);
                        1_u32
                    }
                    Some(_) => 3,
                    None => 2,
                }
            };
            conn.reply(&header, &(code,)).await?;
            if code == 1 {
                name_changed(bus, &name, unique, "").await?;
                conn.emit_signal(Some(unique), DBUS_PATH, DBUS, "NameLost", &(name,))
                    .await?;
            }
        }
        Some("ListNames") => {
            let names: Vec<_> = {
                let bus = bus.lock().expect("mock bus lock");
                bus.peers
                    .keys()
                    .chain(bus.names.keys())
                    .map(String::as_str)
                    .chain(std::iter::once(DBUS))
                    .map(String::from)
                    .collect()
            };
            conn.reply(&header, &(names,)).await?;
        }
        Some("GetId") => {
            conn.reply(&header, &(conn.server_guid().as_str(),)).await?;
        }
        Some("StartServiceByName") => {
            conn.reply_dbus_error(
                &header,
                fdo::Error::ServiceUnknown("mock bus does not activate services".into()),
            )
            .await?;
        }
        _ => {
            conn.reply_dbus_error(
                &header,
                fdo::Error::UnknownMethod("unsupported mock bus method".into()),
            )
            .await?;
        }
    }
    Ok(())
}

async fn bus_peer(
    bus: Arc<Mutex<Bus>>,
    socket: UnixStream,
    guid: zbus::OwnedGuid,
    unique: String,
) -> zbus::Result<()> {
    // build_message_stream installs the receiver before auth can pipeline Hello.
    let mut stream = Builder::unix_stream(socket)
        .server(guid)
        .p2p()
        .unique_name(DBUS)
        .build_message_stream()
        .await?;
    let conn = Connection::from(&stream);
    bus.lock()
        .expect("mock bus lock")
        .peers
        .insert(unique.clone(), conn.clone());
    while let Some(message) = next_message(&mut stream).await {
        let message = match message {
            Ok(message) => message,
            Err(_) => break,
        };
        let header = message.header();
        if header.destination().is_some_and(|name| name == DBUS) {
            bus_method(&bus, &conn, &unique, &message).await?;
        } else if let Some(destination) = header.destination() {
            let target = {
                let bus = bus.lock().expect("mock bus lock");
                let unique = bus.names.get(destination.as_str());
                bus.peers
                    .get(unique.map(String::as_str).unwrap_or(destination.as_str()))
                    .cloned()
            };
            if let Some(target) = target {
                let _ = target.send(&message).await;
            } else if message.message_type() == MessageType::MethodCall {
                conn.reply_dbus_error(&header, fdo::Error::ServiceUnknown(destination.to_string()))
                    .await?;
            }
        } else if message.message_type() == MessageType::Signal {
            broadcast(&bus, &message).await?;
        }
    }
    let names = {
        let mut bus = bus.lock().expect("mock bus lock");
        bus.peers.remove(&unique);
        let names: Vec<_> = bus
            .names
            .iter()
            .filter(|(_, owner)| *owner == &unique)
            .map(|(name, _)| name.clone())
            .collect();
        for name in &names {
            bus.names.remove(name);
        }
        names
    };
    for name in names.iter().chain(std::iter::once(&unique)) {
        name_changed(&bus, name, &unique, "").await?;
    }
    Ok(())
}

struct TempDir(PathBuf);

impl TempDir {
    fn new() -> Self {
        static SEQUENCE: AtomicU64 = AtomicU64::new(0);
        loop {
            let directory = std::env::temp_dir().join(format!(
                "pm{}-{}",
                std::process::id(),
                SEQUENCE.fetch_add(1, Ordering::Relaxed)
            ));
            match std::fs::DirBuilder::new().mode(0o700).create(&directory) {
                Ok(()) => return Self(directory),
                Err(err) if err.kind() == std::io::ErrorKind::AlreadyExists => continue,
                Err(err) => panic!("create mock portal temporary directory: {err}"),
            }
        }
    }
}

impl Drop for TempDir {
    fn drop(&mut self) {
        let _ = std::fs::remove_dir_all(&self.0);
    }
}

pub struct MockPortal {
    directory: TempDir,
    address: String,
    bus_task: JoinHandle<()>,
    server: Connection,
    shared: Shared,
}

impl MockPortal {
    pub async fn start(cfg: MockConfig) -> Self {
        let directory = TempDir::new();
        let socket_path = directory.0.join("bus");
        let listener = UnixListener::bind(&socket_path).expect("bind private mock bus socket");
        let address = format!("unix:path={}", socket_path.display());
        let (bus_socket, service_socket) = UnixStream::pair().expect("mock bus socketpair");
        let guid = zbus::OwnedGuid::from(Guid::generate());
        let bus_task = tokio::spawn(async move {
            let bus = Arc::new(Mutex::new(Bus::default()));
            let mut peers = JoinSet::new();
            peers.spawn(bus_peer(
                bus.clone(),
                bus_socket,
                guid.clone(),
                ":1.1".into(),
            ));
            let mut sequence = 1_u64;
            loop {
                tokio::select! {
                    accepted = listener.accept() => {
                        let (socket, _) = accepted.expect("accept private mock bus client");
                        sequence += 1;
                        peers.spawn(bus_peer(
                            bus.clone(),
                            socket.into_std().expect("convert mock bus socket"),
                            guid.clone(),
                            format!(":1.{sequence}"),
                        ));
                    }
                    finished = peers.join_next(), if !peers.is_empty() => {
                        if let Some(result) = finished {
                            result.expect("mock bus peer task").expect("mock bus peer");
                        }
                    }
                }
            }
        });
        let shared = Shared {
            cfg: Arc::new(cfg),
            state: Arc::new(Mutex::new(State::default())),
        };
        let mut builder =
            Builder::unix_stream(service_socket).serve_at(DESKTOP, ScreenCast(shared.clone()));
        if shared.cfg.rd_version.is_some() {
            builder = builder.serve_at(DESKTOP, RemoteDesktop(shared.clone()));
        }
        if shared.cfg.register_name {
            builder = builder.name(PORTAL);
        }
        // build() waits for Hello and RequestName; callers never race name registration.
        let server = tokio::time::timeout(Duration::from_secs(5), builder.build())
            .await
            .expect("mock portal startup timeout")
            .expect("build mock portal service");
        Self {
            directory,
            address,
            bus_task,
            server,
            shared,
        }
    }

    pub fn address(&self) -> &str {
        &self.address
    }

    pub async fn client(&self) -> Connection {
        tokio::time::timeout(
            Duration::from_secs(5),
            Builder::address(self.address()).build(),
        )
        .await
        .expect("mock portal client connection timeout")
        .expect("connect mock portal client")
    }

    pub fn calls(&self) -> Vec<MockCall> {
        self.shared
            .state
            .lock()
            .expect("mock state lock")
            .calls
            .clone()
    }

    pub fn calls_to(&self, member: &str) -> Vec<MockCall> {
        self.calls()
            .into_iter()
            .filter(|call| call.member == member)
            .collect()
    }

    pub async fn close_session(&self, session_path: &str) {
        self.shared
            .close_session(&self.server, session_path)
            .await
            .expect("close mock portal session");
    }

    pub fn pipewire_fds(&self) -> usize {
        self.shared
            .state
            .lock()
            .expect("mock state lock")
            .pipewire_peers
            .len()
    }
}

impl Drop for MockPortal {
    fn drop(&mut self) {
        self.bus_task.abort();
        for task in self
            .shared
            .state
            .lock()
            .expect("mock state lock")
            .response_tasks
            .drain(..)
        {
            task.abort();
        }
        // TempDir removes the socket even when clients outlive the mock.
    }
}
