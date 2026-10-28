use std::{collections::HashMap, future::poll_fn, os::fd::OwnedFd, pin::Pin, time::Duration};

use tokio::sync::watch;
use tracing::{debug, warn};
use zbus::{
    export::futures_core::Stream, fdo, message::Type as MessageType, Connection, MatchRule,
    Message, MessageStream, OwnedMatchRule, OwnedObjectPath, OwnedVariant, Proxy, Str,
};

use crate::portal_model::{
    plan, stream_displays, FlowError, FlowState, Method, OptValue, PortalCaps, PortalFlow,
    PortalKind, RVal, RestoreTokenStore, StreamDisplay, StreamInfo, IFACE_RD, IFACE_REQUEST,
    IFACE_SC, IFACE_SESSION, PORTAL_BUS, PORTAL_PATH,
};

const DBUS_BUS: &str = "org.freedesktop.DBus";
const DBUS_PATH: &str = "/org/freedesktop/DBus";

#[derive(Debug, Clone)]
pub struct PortalOptions {
    pub want_input: bool,
    pub state_dir: std::path::PathBuf,
    pub token_prefix: String,
    pub response_timeout: Duration,
}

impl PortalOptions {
    pub fn new(state_dir: std::path::PathBuf, want_input: bool) -> Self {
        Self {
            want_input,
            state_dir,
            token_prefix: format!("rs{}_", std::process::id()),
            response_timeout: Duration::from_secs(120),
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct PortalGrant {
    pub kind: PortalKind,
    pub devices: u32,
    pub cursor_mode: u32,
    pub streams: Vec<StreamInfo>,
    pub displays: Vec<StreamDisplay>,
    pub clipboard_enabled: bool,
}

#[derive(Debug, thiserror::Error)]
pub enum PortalError {
    #[error("xdg-desktop-portal ScreenCast is not available")]
    Unavailable,
    #[error("the user denied screen sharing")]
    Cancelled,
    #[error("portal request failed: {0}")]
    Failed(String),
    #[error("portal did not answer in time")]
    Timeout,
    #[error("portal session was closed")]
    Closed,
    #[error("D-Bus: {0}")]
    DBus(String),
}

pub struct PortalSession {
    conn: Connection,
    session_path: String,
    grant: PortalGrant,
    closed: watch::Receiver<bool>,
    closed_tx: watch::Sender<bool>,
    task: Option<tokio::task::JoinHandle<()>>,
}

impl PortalSession {
    /// Read portal capability properties. Missing interfaces and properties are
    /// represented by `None`/zero and do not fail capability discovery.
    pub async fn caps(conn: &Connection) -> PortalCaps {
        let mut caps = PortalCaps::default();

        if let Ok(proxy) = Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, IFACE_RD).await {
            caps.rd_version = proxy.get_property::<u32>("version").await.ok();
            if caps.rd_version.is_some() {
                caps.rd_devices = proxy
                    .get_property::<u32>("AvailableDeviceTypes")
                    .await
                    .unwrap_or_default();
            }
        }

        if let Ok(proxy) = Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, IFACE_SC).await {
            caps.sc_version = proxy.get_property::<u32>("version").await.ok();
            if caps.sc_version.is_some() {
                caps.sc_sources = proxy
                    .get_property::<u32>("AvailableSourceTypes")
                    .await
                    .unwrap_or_default();
                caps.sc_cursor_modes = proxy
                    .get_property::<u32>("AvailableCursorModes")
                    .await
                    .unwrap_or_default();
            }
        }

        caps
    }

    pub async fn open(conn: &Connection, opts: &PortalOptions) -> Result<Self, PortalError> {
        let caps = Self::caps(conn).await;
        let plan = plan(&caps, opts.want_input).map_err(|_| PortalError::Unavailable)?;
        let sender = conn
            .unique_name()
            .map(|name| name.as_str().to_owned())
            .ok_or_else(|| PortalError::DBus("connection has no unique name".into()))?;
        let store = RestoreTokenStore::new(&opts.state_dir);
        let restore_token = plan.persist.then(|| store.load(plan.kind)).flatten();
        let mut flow = PortalFlow::new(plan.clone(), &sender, &opts.token_prefix, restore_token);

        let start = loop {
            let call = flow
                .next_call()
                .ok_or_else(|| PortalError::Failed("portal flow ended before Start".to_owned()))?;
            let expected_path = call.request_path.clone();
            let mut expected_stream = request_stream(conn, &expected_path).await?;
            // Keep a path-less subscription as a race-free fallback. A portal
            // may emit the response as soon as it has sent the method reply,
            // before a second subscription for a non-predictable handle can be
            // installed.
            let mut fallback_stream = request_fallback_stream(conn).await?;
            let returned_path = call_request(conn, &call).await?;
            let mut returned_stream = if returned_path.as_str() != expected_path {
                Some(request_stream(conn, returned_path.as_str()).await?)
            } else {
                None
            };

            let response = match tokio::time::timeout(
                opts.response_timeout,
                wait_request_response(
                    &mut expected_stream,
                    &mut fallback_stream,
                    returned_stream.as_mut(),
                    &expected_path,
                    (returned_path.as_str() != expected_path).then_some(returned_path.as_str()),
                ),
            )
            .await
            {
                Ok(Some(Ok(message))) => message,
                Ok(Some(Err(error))) => return Err(map_zbus_error(error)),
                Ok(None) => {
                    return Err(PortalError::DBus("portal response stream ended".to_owned()));
                }
                Err(_) => {
                    close_request(conn, returned_path.as_str()).await;
                    return Err(PortalError::Timeout);
                }
            };

            let (code, results) = decode_response(response)?;
            let stream_count = results
                .iter()
                .find_map(|(key, value)| match (key.as_str(), value) {
                    ("streams", RVal::Streams(streams)) => Some(streams.len()),
                    _ => None,
                })
                .unwrap_or(0);
            debug!(
                method = call.method.member(),
                response_code = code,
                streams = stream_count,
                "portal response"
            );
            match flow.on_response(code, &results) {
                Ok(FlowState::Continue) => continue,
                Ok(FlowState::Done(result)) => break result,
                Err(error) => return Err(flow_error(error)),
            }
        };

        if let Some(token) = start.restore_token.as_deref() {
            if let Err(error) = store.save(plan.kind, token) {
                warn!(error = %error, "failed to save portal restore token");
            }
        }

        let grant = PortalGrant {
            kind: start.kind,
            devices: start.devices,
            cursor_mode: plan.cursor_mode,
            streams: start.streams.clone(),
            displays: stream_displays(&start.streams),
            clipboard_enabled: start.clipboard_enabled,
        };
        let session_path = start.session;
        let (closed_tx, closed) = watch::channel(false);
        let task = spawn_closed_watcher(conn, &session_path, closed_tx.clone()).await?;

        Ok(Self {
            conn: conn.clone(),
            session_path,
            grant,
            closed,
            closed_tx,
            task: Some(task),
        })
    }

    pub fn grant(&self) -> &PortalGrant {
        &self.grant
    }

    pub fn connection(&self) -> &Connection {
        &self.conn
    }

    pub fn session_path(&self) -> &str {
        &self.session_path
    }

    pub async fn open_pipewire_remote(&self) -> Result<OwnedFd, PortalError> {
        if *self.closed.borrow() {
            return Err(PortalError::Closed);
        }
        let proxy = Proxy::new(&self.conn, PORTAL_BUS, PORTAL_PATH, IFACE_SC)
            .await
            .map_err(map_zbus_error)?;
        let session = OwnedObjectPath::try_from(self.session_path.as_str())
            .map_err(|error| PortalError::DBus(error.to_string()))?;
        let fd: zbus::OwnedFd = proxy
            .call(
                "OpenPipeWireRemote",
                &(session, HashMap::<String, OwnedVariant>::new()),
            )
            .await
            .map_err(map_zbus_error)?;
        Ok(fd.into())
    }

    pub fn closed(&self) -> watch::Receiver<bool> {
        self.closed.clone()
    }

    pub async fn close(self) -> Result<(), PortalError> {
        let proxy = Proxy::new(
            &self.conn,
            PORTAL_BUS,
            self.session_path.as_str(),
            IFACE_SESSION,
        )
        .await
        .map_err(map_zbus_error)?;
        let result = proxy.call::<_, _, ()>("Close", &()).await;
        match result {
            Ok(()) => {
                let _ = self.closed_tx.send(true);
                Ok(())
            }
            Err(error) if session_already_closed(&error) => {
                let _ = self.closed_tx.send(true);
                Ok(())
            }
            Err(error) => Err(map_zbus_error(error)),
        }
    }
}

impl Drop for PortalSession {
    fn drop(&mut self) {
        if let Some(task) = self.task.take() {
            task.abort();
        }
    }
}

async fn call_request(
    conn: &Connection,
    call: &crate::portal_model::Call,
) -> Result<OwnedObjectPath, PortalError> {
    let options = options_map(&call.options);
    let proxy = Proxy::new(conn, PORTAL_BUS, PORTAL_PATH, call.method.interface())
        .await
        .map_err(map_zbus_error)?;
    match call.method {
        Method::RdCreateSession | Method::ScCreateSession => proxy
            .call(call.method.member(), &(options,))
            .await
            .map_err(map_zbus_error),
        Method::RdStart | Method::ScStart => {
            let session = call_session(call)?;
            proxy
                .call(call.method.member(), &(session, "", options))
                .await
                .map_err(map_zbus_error)
        }
        Method::SelectDevices | Method::SelectSources => {
            let session = call_session(call)?;
            proxy
                .call(call.method.member(), &(session, options))
                .await
                .map_err(map_zbus_error)
        }
    }
}

fn call_session(call: &crate::portal_model::Call) -> Result<OwnedObjectPath, PortalError> {
    let session = call
        .session
        .as_deref()
        .ok_or_else(|| PortalError::DBus("portal call has no session handle".into()))?;
    OwnedObjectPath::try_from(session).map_err(|error| PortalError::DBus(error.to_string()))
}

fn options_map(options: &[(&'static str, OptValue)]) -> HashMap<String, OwnedVariant> {
    options
        .iter()
        .map(|(key, value)| {
            let value = match value {
                OptValue::Str(value) => Str::from(value.as_str()).into(),
                OptValue::U32(value) => (*value).into(),
                OptValue::Bool(value) => (*value).into(),
            };
            ((*key).to_owned(), value)
        })
        .collect()
}

async fn request_stream(conn: &Connection, path: &str) -> Result<MessageStream, PortalError> {
    let rule: OwnedMatchRule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(PORTAL_BUS)
        .interface(IFACE_REQUEST)
        .member("Response")
        .path(path)
        .build()
        .map_err(map_zbus_error)?
        .to_owned()
        .into();
    MessageStream::for_match_rule(rule, conn, None)
        .await
        .map_err(map_zbus_error)
}

async fn request_fallback_stream(conn: &Connection) -> Result<MessageStream, PortalError> {
    let rule: OwnedMatchRule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(PORTAL_BUS)
        .interface(IFACE_REQUEST)
        .member("Response")
        .build()
        .map_err(map_zbus_error)?
        .to_owned()
        .into();
    MessageStream::for_match_rule(rule, conn, None)
        .await
        .map_err(map_zbus_error)
}

async fn wait_request_response(
    expected: &mut MessageStream,
    fallback: &mut MessageStream,
    mut returned: Option<&mut MessageStream>,
    expected_path: &str,
    returned_path: Option<&str>,
) -> Option<zbus::Result<Message>> {
    loop {
        let message = match returned.as_mut() {
            Some(returned) => {
                tokio::select! {
                    message = next_message(expected) => message,
                    message = next_message(fallback) => message,
                    message = next_message(returned) => message,
                }
            }
            None => {
                tokio::select! {
                    message = next_message(expected) => message,
                    message = next_message(fallback) => message,
                }
            }
        };
        match &message {
            Some(Ok(msg)) => {
                let path_matches = msg.header().path().is_some_and(|path| {
                    path.as_str() == expected_path
                        || returned_path.is_some_and(|expected| path.as_str() == expected)
                });
                if path_matches {
                    return message;
                }
            }
            _ => return message,
        }
    }
}

async fn next_message(stream: &mut MessageStream) -> Option<zbus::Result<Message>> {
    poll_fn(|cx| Pin::new(&mut *stream).poll_next(cx)).await
}

fn decode_response(message: Message) -> Result<(u32, Vec<(String, RVal)>), PortalError> {
    let body: (u32, HashMap<String, OwnedVariant>) = message
        .body()
        .deserialize()
        .map_err(|error| PortalError::DBus(error.to_string()))?;
    Ok((
        body.0,
        body.1
            .into_iter()
            .map(|(key, value)| (key.clone(), rval(&key, value)))
            .collect(),
    ))
}

fn rval(key: &str, value: OwnedVariant) -> RVal {
    if key == "streams" {
        return RVal::Streams(parse_streams(value).unwrap_or_default());
    }
    if let Ok(clone) = value.try_clone() {
        if let Ok(string) = String::try_from(clone) {
            return RVal::Str(string);
        }
    }
    if let Ok(clone) = value.try_clone() {
        if let Ok(path) = OwnedObjectPath::try_from(clone) {
            return RVal::Str(path.to_string());
        }
    }
    if let Ok(clone) = value.try_clone() {
        if let Ok(number) = u32::try_from(clone) {
            return RVal::U32(number);
        }
    }
    if let Ok(boolean) = bool::try_from(value) {
        return RVal::Bool(boolean);
    }
    RVal::Other
}

fn parse_streams(value: OwnedVariant) -> Option<Vec<StreamInfo>> {
    let streams: Vec<(u32, HashMap<String, OwnedVariant>)> = value.try_into().ok()?;
    Some(
        streams
            .into_iter()
            .map(|(node_id, mut properties)| StreamInfo {
                node_id,
                id: property_string(properties.remove("id")),
                position: property_pair(properties.remove("position")),
                size: property_pair(properties.remove("size")),
                source_type: property_u32(properties.remove("source_type")),
                mapping_id: property_string(properties.remove("mapping_id")),
            })
            .collect(),
    )
}

fn property_string(value: Option<OwnedVariant>) -> Option<String> {
    value.and_then(|value| String::try_from(value).ok())
}

fn property_u32(value: Option<OwnedVariant>) -> Option<u32> {
    value.and_then(|value| u32::try_from(value).ok())
}

fn property_pair(value: Option<OwnedVariant>) -> Option<(i32, i32)> {
    value.and_then(|value| <(i32, i32)>::try_from(value).ok())
}

async fn close_request(conn: &Connection, path: &str) {
    if let Ok(proxy) = Proxy::new(conn, PORTAL_BUS, path, IFACE_REQUEST).await {
        let _ = proxy.call::<_, _, ()>("Close", &()).await;
    }
}

async fn spawn_closed_watcher(
    conn: &Connection,
    session_path: &str,
    closed_tx: watch::Sender<bool>,
) -> Result<tokio::task::JoinHandle<()>, PortalError> {
    let session_rule: OwnedMatchRule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(PORTAL_BUS)
        .interface(IFACE_SESSION)
        .member("Closed")
        .path(session_path)
        .build()
        .map_err(map_zbus_error)?
        .to_owned()
        .into();
    let owner_rule: OwnedMatchRule = MatchRule::builder()
        .msg_type(MessageType::Signal)
        .sender(DBUS_BUS)
        .path(DBUS_PATH)
        .interface(DBUS_BUS)
        .member("NameOwnerChanged")
        .add_arg(PORTAL_BUS)
        .build()
        .map_err(map_zbus_error)?
        .to_owned()
        .into();
    let session_stream = MessageStream::for_match_rule(session_rule, conn, None)
        .await
        .map_err(map_zbus_error)?;
    let owner_stream = MessageStream::for_match_rule(owner_rule, conn, None)
        .await
        .map_err(map_zbus_error)?;
    let task = tokio::spawn(async move {
        let mut session_stream = session_stream;
        let mut owner_stream = owner_stream;
        loop {
            tokio::select! {
                message = next_message(&mut session_stream) => {
                    if message.is_some() {
                        let _ = closed_tx.send(true);
                    }
                    break;
                }
                message = next_message(&mut owner_stream) => {
                    let closed = message
                        .and_then(Result::ok)
                        .and_then(|message| message.body().deserialize::<(String, String, String)>().ok())
                        .is_some_and(|(name, _old, new)| name == PORTAL_BUS && new.is_empty());
                    if closed {
                        let _ = closed_tx.send(true);
                        break;
                    }
                }
            }
        }
    });
    Ok(task)
}

fn flow_error(error: FlowError) -> PortalError {
    match error {
        FlowError::Cancelled => PortalError::Cancelled,
        FlowError::Failed(code) => PortalError::Failed(format!("code {code}")),
        other => PortalError::Failed(other.to_string()),
    }
}

fn session_already_closed(error: &zbus::Error) -> bool {
    matches!(
        error,
        zbus::Error::FDO(inner)
            if matches!(
                inner.as_ref(),
                fdo::Error::UnknownObject(_)
                    | fdo::Error::NameHasNoOwner(_)
                    | fdo::Error::ServiceUnknown(_)
            )
    )
}

fn map_zbus_error(error: zbus::Error) -> PortalError {
    if matches!(
        &error,
        zbus::Error::FDO(inner)
            if matches!(
                inner.as_ref(),
                fdo::Error::ServiceUnknown(_)
                    | fdo::Error::NameHasNoOwner(_)
                    | fdo::Error::UnknownInterface(_)
                    | fdo::Error::UnknownMethod(_)
            )
    ) {
        PortalError::Unavailable
    } else {
        PortalError::DBus(error.to_string())
    }
}
