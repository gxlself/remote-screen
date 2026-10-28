//! App SSH client: device signing, pinned host keys, terminals and one-hop ProxyJump.
//!
//! Terminal bytes stay in Rust and can feed the App's `rs_term_*` bridge directly.
//! No process, sshd, tmux installation or platform key export is required locally.

pub mod auth;
pub mod install;
pub mod jump;
pub mod known_hosts;
pub mod pty;
pub mod reconnect;
pub mod sftp;
pub mod sftp_transfer;
pub mod tmux;

use std::{
    borrow::Cow,
    sync::{Arc, Mutex},
    time::Duration,
};

use russh::{client, keys::PublicKeyOrCertificate, Channel, ChannelMsg};
use thiserror::Error;
use tokio::{
    io::{AsyncRead, AsyncWrite},
    sync::{broadcast, watch},
};
use url::Url;

pub use auth::{public_key_openssh, Credential, Prompter};
pub use known_hosts::{HostKeyEntry, HostKeyStore, MemoryHostKeyStore};
pub use pty::{PtyOpts, Shell};

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct SshTarget {
    pub host: String,
    pub port: u16,
    pub user: String,
    pub jump: Option<Box<SshTarget>>,
}

#[derive(Clone, Debug, Error, PartialEq, Eq)]
pub enum SshError {
    #[error("authentication failed (tried: {tried:?})")]
    AuthFailed { tried: Vec<&'static str> },
    #[error("SSH host key changed: expected {expected}, got {got}")]
    HostKeyChanged { expected: String, got: String },
    #[error("SSH host key was rejected")]
    HostKeyRejected,
    #[error("unsupported SSH feature: {0}")]
    Unsupported(&'static str),
    #[error("invalid SSH key")]
    InvalidKey,
    #[error("external SSH signing failed")]
    SigningFailed,
    #[error("SSH operation timed out")]
    Timeout,
    #[error("SSH authentication cancelled")]
    Cancelled,
    #[error("invalid SSH options: {0}")]
    InvalidOptions(&'static str),
    #[error("SSH channel request rejected")]
    RequestRejected,
    #[error("SSH connection closed")]
    Closed,
    #[error("SSH transport: {0}")]
    Transport(String),
}

impl From<russh::Error> for SshError {
    fn from(error: russh::Error) -> Self {
        match error {
            russh::Error::KeepaliveTimeout
            | russh::Error::ConnectionTimeout
            | russh::Error::InactivityTimeout => Self::Timeout,
            russh::Error::NoCommonAlgo {
                kind: russh::AlgorithmKind::Key,
                ref theirs,
                ..
            } if !theirs.is_empty() && theirs.iter().all(|a| a.contains("rsa")) => {
                Self::Unsupported("RSA host keys are not supported")
            }
            russh::Error::Disconnect
            | russh::Error::HUP
            | russh::Error::SendError
            | russh::Error::RecvError => Self::Closed,
            russh::Error::RequestDenied => Self::RequestRejected,
            other => Self::Transport(other.to_string()),
        }
    }
}

impl From<russh::SendError> for SshError {
    fn from(_: russh::SendError) -> Self {
        Self::Closed
    }
}

#[derive(Debug)]
pub struct SshOptions {
    /// Network for the first hop (the target or its jump host).
    pub net: Arc<dyn rsp_transport::NetDialer>,
    pub connect_timeout: Duration,
    /// Includes all authentication methods, UI prompts and Tailscale browser approval.
    pub auth_timeout: Duration,
    pub request_timeout: Duration,
    pub keepalive_interval: Duration,
    pub keepalive_max: usize,
    /// Number of output chunks; a full queue backpressures the SSH connection.
    pub output_buffer: usize,
    /// Exact additional HTTPS hostnames, e.g. `hs.example.com` (no wildcard).
    pub headscale_domains: Vec<String>,
    /// When absent, the target's credentials are also used for the jump host.
    pub jump_credentials: Option<Vec<Credential>>,
}

impl Default for SshOptions {
    fn default() -> Self {
        Self {
            net: rsp_transport::system_net(),
            connect_timeout: Duration::from_secs(30),
            auth_timeout: Duration::from_secs(300),
            request_timeout: Duration::from_secs(30),
            keepalive_interval: Duration::from_secs(15),
            keepalive_max: 3,
            output_buffer: 16,
            headscale_domains: Vec::new(),
            jump_credentials: None,
        }
    }
}

impl SshOptions {
    fn validate(&self) -> Result<(), SshError> {
        if self.connect_timeout.is_zero()
            || self.auth_timeout.is_zero()
            || self.request_timeout.is_zero()
            || self.keepalive_interval.is_zero()
            || self.keepalive_max == 0
            || self.output_buffer == 0
        {
            return Err(SshError::InvalidOptions(
                "timeouts, keepalive count and output buffer must be positive",
            ));
        }
        for domain in &self.headscale_domains {
            let url = Url::parse(&format!("https://{domain}/")).map_err(|_| {
                SshError::InvalidOptions("Headscale domains must be bare DNS hostnames")
            })?;
            let valid_labels = domain.len() <= 253
                && domain.split('.').all(|label| {
                    !label.is_empty()
                        && label.len() <= 63
                        && label
                            .as_bytes()
                            .first()
                            .is_some_and(u8::is_ascii_alphanumeric)
                        && label
                            .as_bytes()
                            .last()
                            .is_some_and(u8::is_ascii_alphanumeric)
                        && label
                            .bytes()
                            .all(|c| c.is_ascii_alphanumeric() || c == b'-')
                });
            if !valid_labels
                || !matches!(url.host(), Some(url::Host::Domain(_)))
                || !url
                    .host_str()
                    .is_some_and(|host| host.eq_ignore_ascii_case(domain))
                || url.path() != "/"
                || url.port().is_some()
                || !url.username().is_empty()
                || url.password().is_some()
                || url.query().is_some()
                || url.fragment().is_some()
            {
                return Err(SshError::InvalidOptions(
                    "Headscale domains must be bare DNS hostnames",
                ));
            }
        }
        Ok(())
    }
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshState {
    Connecting,
    Connected,
    Closed,
    Failed(SshError),
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub enum SshEvent {
    StateChanged(SshState),
    TmuxUnavailable,
}

struct Shared {
    state: watch::Sender<SshState>,
    events: broadcast::Sender<SshEvent>,
    server_ssh_id: Mutex<Option<String>>,
}

impl Shared {
    fn set_state(&self, state: SshState) {
        // Preserve the cause of a disconnect, including a failure on the jump leg.
        let changed = self.state.send_if_modified(|current| {
            if matches!(current, SshState::Closed | SshState::Failed(_)) || *current == state {
                return false;
            }
            *current = state.clone();
            true
        });
        if changed {
            let _ = self.events.send(SshEvent::StateChanged(state));
        }
    }

    fn check(&self) -> Result<(), SshError> {
        match &*self.state.borrow() {
            SshState::Connected | SshState::Connecting => Ok(()),
            SshState::Closed => Err(SshError::Closed),
            SshState::Failed(error) => Err(error.clone()),
        }
    }
}

pub(crate) struct ClientHandler {
    host: String,
    port: u16,
    record_ssh_id: bool,
    store: Arc<dyn HostKeyStore>,
    prompter: Arc<dyn Prompter>,
    shared: Arc<Shared>,
    options: Arc<SshOptions>,
}

impl client::Handler for ClientHandler {
    type Error = SshError;

    async fn kex_done(
        &mut self,
        _shared_secret: Option<&[u8]>,
        _names: &russh::Names,
        session: &mut client::Session,
    ) -> Result<(), SshError> {
        if !self.record_ssh_id {
            return Ok(());
        }
        let ssh_id = String::from_utf8_lossy(session.remote_sshid())
            .trim_end_matches(['\r', '\n'])
            .to_owned();
        let mut recorded = match self.shared.server_ssh_id.lock() {
            Ok(recorded) => recorded,
            Err(poisoned) => poisoned.into_inner(),
        };
        *recorded = Some(ssh_id);
        Ok(())
    }

    async fn check_server_key(
        &mut self,
        presented: &PublicKeyOrCertificate,
    ) -> Result<bool, SshError> {
        let key = presented.public_key();
        if key.algorithm().is_rsa() {
            return Err(SshError::Unsupported("RSA host keys are not supported"));
        }
        if presented.certificate().is_some() {
            return Err(SshError::Unsupported(
                "SSH host certificates require a CA policy",
            ));
        }
        let entry = HostKeyEntry {
            algorithm: key.algorithm().to_string(),
            key: key.to_bytes().map_err(|_| SshError::InvalidKey)?,
            fingerprint: key.fingerprint(ssh_key::HashAlg::Sha256).to_string(),
        };
        if let Some(expected) = self.store.lookup(&self.host, self.port) {
            return compare_host_key(&expected, &entry);
        }
        if !tokio::time::timeout(
            self.options.connect_timeout,
            self.prompter
                .confirm_new_host_key(&self.host, &entry.fingerprint, &entry.algorithm),
        )
        .await
        .map_err(|_| SshError::Timeout)?
        {
            return Err(SshError::HostKeyRejected);
        }
        // A prompt may be outstanding while another connection saves a pin.
        if let Some(expected) = self.store.lookup(&self.host, self.port) {
            return compare_host_key(&expected, &entry);
        }
        self.store.save(&self.host, self.port, entry);
        Ok(true)
    }

    async fn auth_banner(&mut self, banner: &str, _: &mut client::Session) -> Result<(), SshError> {
        for (offset, _) in banner.match_indices("https://") {
            // A URL embedded in another URL's query/path is not a separate link.
            if banner[..offset]
                .chars()
                .next_back()
                .is_some_and(|c| !c.is_whitespace() && !"<>\"'([{".contains(c))
            {
                continue;
            }
            let candidate = banner[offset..]
                .split(|c: char| c.is_whitespace() || "<>\"'()[]{}".contains(c))
                .next()
                .unwrap_or_default()
                .trim_end_matches(['.', ',', ';']);
            let allowed = Url::parse(candidate).ok().is_some_and(|url| {
                url.scheme() == "https"
                    && url.username().is_empty()
                    && url.password().is_none()
                    && url.host_str().is_some_and(|host| {
                        host == "login.tailscale.com"
                            || self
                                .options
                                .headscale_domains
                                .iter()
                                .any(|domain| host.eq_ignore_ascii_case(domain))
                    })
            });
            if allowed {
                self.prompter.auth_url(candidate);
            } else {
                // Never log the banner or URL: an approval URL can contain a token.
                tracing::debug!("ignored untrusted SSH authentication URL");
            }
        }
        Ok(())
    }

    async fn disconnected(
        &mut self,
        reason: client::DisconnectReason<SshError>,
    ) -> Result<(), SshError> {
        match reason {
            client::DisconnectReason::Error(error) => {
                self.shared.set_state(SshState::Failed(error.clone()));
                Err(error)
            }
            client::DisconnectReason::ReceivedDisconnect(_) => {
                self.shared.set_state(SshState::Closed);
                Ok(())
            }
        }
    }
}

fn compare_host_key(expected: &HostKeyEntry, got: &HostKeyEntry) -> Result<bool, SshError> {
    if expected.algorithm == got.algorithm && expected.key == got.key {
        return Ok(true);
    }
    Err(SshError::HostKeyChanged {
        expected: expected.fingerprint.clone(),
        got: got.fingerprint.clone(),
    })
}

struct Connection {
    handle: client::Handle<ClientHandler>,
    jump: Option<client::Handle<ClientHandler>>,
    shared: Arc<Shared>,
    options: Arc<SshOptions>,
}

/// An authenticated SSH transport. Shells hold the transport alive until closed/dropped.
pub struct SshSession {
    connection: Arc<Connection>,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ServerInfo {
    /// Server identification string without the trailing CRLF.
    pub ssh_id: String,
    /// Whether the server identifies itself as Tailscale SSH.
    pub tailscale: bool,
}

#[derive(Clone, Debug, PartialEq, Eq)]
pub struct ExecOutput {
    /// `None` when the remote closes without sending an exit status.
    pub status: Option<u32>,
    pub stdout: Vec<u8>,
    pub stderr: Vec<u8>,
}

impl SshSession {
    pub async fn connect(
        target: SshTarget,
        creds: Vec<Credential>,
        store: Arc<dyn HostKeyStore>,
        prompter: Arc<dyn Prompter>,
        mut opts: SshOptions,
    ) -> Result<Self, SshError> {
        jump::validate(&target)?;
        opts.validate()?;
        let jump_credentials = opts.jump_credentials.take();
        let options = Arc::new(opts);
        let (state, _) = watch::channel(SshState::Connecting);
        let (events, _) = broadcast::channel(32);
        let shared = Arc::new(Shared {
            state,
            events,
            server_ssh_id: Mutex::new(None),
        });
        let handler = |target: &SshTarget, record_ssh_id: bool| ClientHandler {
            host: target.host.clone(),
            port: target.port,
            record_ssh_id,
            store: store.clone(),
            prompter: prompter.clone(),
            shared: shared.clone(),
            options: options.clone(),
        };
        let mut jump_handle = None;
        let result = async {
            let handle = if let Some(jump) = target.jump.as_deref() {
                let stream = tokio::time::timeout(
                    options.connect_timeout,
                    options.net.dial_tcp_host(&jump.host, jump.port),
                )
                .await
                .map_err(|_| SshError::Timeout)?
                .map_err(|e| SshError::Transport(e.to_string()))?;
                let jump_connection = connect_leg(
                    stream,
                    handler(jump, false),
                    jump_credentials.as_deref().unwrap_or(&creds),
                    &jump.user,
                )
                .await?;
                jump_handle = Some(jump_connection);
                let jump_connection = jump_handle.as_ref().ok_or(SshError::Closed)?;
                let channel = tokio::time::timeout(
                    options.connect_timeout,
                    jump_connection.channel_open_direct_tcpip(
                        &target.host,
                        u32::from(target.port),
                        "127.0.0.1",
                        0,
                    ),
                )
                .await
                .map_err(|_| SshError::Timeout)??;
                connect_leg(
                    channel.into_stream(),
                    handler(&target, true),
                    &creds,
                    &target.user,
                )
                .await?
            } else {
                let stream = tokio::time::timeout(
                    options.connect_timeout,
                    options.net.dial_tcp_host(&target.host, target.port),
                )
                .await
                .map_err(|_| SshError::Timeout)?
                .map_err(|e| SshError::Transport(e.to_string()))?;
                connect_leg(stream, handler(&target, true), &creds, &target.user).await?
            };
            Ok::<_, SshError>(handle)
        }
        .await;
        match result {
            Ok(handle) => {
                shared.check()?;
                shared.set_state(SshState::Connected);
                Ok(Self {
                    connection: Arc::new(Connection {
                        handle,
                        jump: jump_handle,
                        shared,
                        options,
                    }),
                })
            }
            Err(error) => {
                if let Some(jump) = jump_handle {
                    disconnect(&jump).await;
                }
                Err(error)
            }
        }
    }

    pub fn state(&self) -> SshState {
        self.connection.shared.state.borrow().clone()
    }
    pub fn subscribe_state(&self) -> watch::Receiver<SshState> {
        self.connection.shared.state.subscribe()
    }
    pub fn events(&self) -> broadcast::Receiver<SshEvent> {
        self.connection.shared.events.subscribe()
    }

    pub fn server_info(&self) -> ServerInfo {
        let ssh_id = match self.connection.shared.server_ssh_id.lock() {
            Ok(recorded) => recorded.clone(),
            Err(poisoned) => poisoned.into_inner().clone(),
        }
        .unwrap_or_default();
        let tailscale = ssh_id.starts_with("SSH-2.0-Tailscale");
        ServerInfo { ssh_id, tailscale }
    }

    /// Resolves with `Timeout` for keepalive failure, including on the jump leg.
    pub async fn closed(&self) -> Result<(), SshError> {
        let mut state = self.subscribe_state();
        loop {
            match state.borrow_and_update().clone() {
                SshState::Closed => return Ok(()),
                SshState::Failed(error) => return Err(error),
                _ => {}
            }
            state.changed().await.map_err(|_| SshError::Closed)?;
        }
    }

    pub async fn open_shell(&self, opts: PtyOpts) -> Result<Shell, SshError> {
        pty::open(self.connection.clone(), opts).await
    }

    pub async fn exec(&self, cmd: &str) -> Result<ExecOutput, SshError> {
        self.exec_inner(cmd, None).await
    }

    pub async fn exec_with_stdin(&self, cmd: &str, stdin: &[u8]) -> Result<ExecOutput, SshError> {
        self.exec_inner(cmd, Some(stdin)).await
    }

    async fn exec_inner(&self, cmd: &str, stdin: Option<&[u8]>) -> Result<ExecOutput, SshError> {
        self.connection.shared.check()?;
        let mut channel = tokio::time::timeout(
            self.connection.options.request_timeout,
            self.connection.handle.channel_open_session(),
        )
        .await
        .map_err(|_| SshError::Timeout)??;
        let result = async {
            channel.exec(true, cmd.as_bytes()).await?;
            let mut pending = std::collections::VecDeque::new();
            tokio::time::timeout(
                self.connection.options.request_timeout,
                pty::request_success(&mut channel, &mut pending),
            )
            .await
            .map_err(|_| SshError::Timeout)??;
            if let Some(stdin) = stdin {
                const CHUNK: usize = 16 * 1024;
                for chunk in stdin.chunks(CHUNK) {
                    tokio::time::timeout(
                        self.connection.options.request_timeout,
                        channel.data(chunk),
                    )
                    .await
                    .map_err(|_| SshError::Timeout)??;
                }
                tokio::time::timeout(self.connection.options.request_timeout, channel.eof())
                    .await
                    .map_err(|_| SshError::Timeout)??;
            }
            let mut out = ExecOutput {
                status: None,
                stdout: Vec::new(),
                stderr: Vec::new(),
            };
            let mut state = self.subscribe_state();
            loop {
                let msg = if let Some(msg) = pending.pop_front() {
                    Some(msg)
                } else {
                    tokio::select! {
                        msg = channel.wait() => msg,
                        _ = state.changed() => { self.connection.shared.check()?; continue; }
                    }
                };
                match msg {
                    Some(ChannelMsg::Data { data }) => out.stdout.extend_from_slice(&data),
                    Some(ChannelMsg::ExtendedData { data, ext: 1 }) => {
                        out.stderr.extend_from_slice(&data)
                    }
                    Some(ChannelMsg::ExitStatus { exit_status }) => out.status = Some(exit_status),
                    Some(ChannelMsg::Close) | None => break,
                    _ => {}
                }
            }
            self.connection.shared.check()?;
            Ok(out)
        }
        .await;
        let _ = channel.close().await;
        result
    }

    pub async fn close(self) {
        self.connection.shared.set_state(SshState::Closed);
        disconnect(&self.connection.handle).await;
        if let Some(jump) = &self.connection.jump {
            disconnect(jump).await;
        }
    }
}

async fn connect_leg<S: AsyncRead + AsyncWrite + Unpin + Send + 'static>(
    stream: S,
    handler: ClientHandler,
    creds: &[Credential],
    user: &str,
) -> Result<client::Handle<ClientHandler>, SshError> {
    let options = handler.options.clone();
    let prompter = handler.prompter.clone();
    let mut config = client::Config {
        keepalive_interval: Some(options.keepalive_interval),
        keepalive_max: options.keepalive_max,
        channel_buffer_size: options.output_buffer,
        nodelay: true,
        ..Default::default()
    };
    config.preferred.key = Cow::Owned(
        config
            .preferred
            .key
            .iter()
            .filter(|key| !matches!(key, ssh_key::Algorithm::Rsa { .. }))
            .cloned()
            .collect(),
    );
    let mut handle = tokio::time::timeout(
        options.connect_timeout,
        client::connect_stream(Arc::new(config), stream, handler),
    )
    .await
    .map_err(|_| SshError::Timeout)??;
    let auth = tokio::time::timeout(
        options.auth_timeout,
        auth::authenticate(&mut handle, user, creds, prompter.as_ref()),
    )
    .await
    .map_err(|_| SshError::Timeout)
    .and_then(|r| r);
    if let Err(error) = auth {
        disconnect(&handle).await;
        return Err(error);
    }
    Ok(handle)
}

async fn disconnect(handle: &client::Handle<ClientHandler>) {
    let _ = tokio::time::timeout(
        Duration::from_secs(1),
        handle.disconnect(russh::Disconnect::ByApplication, "", ""),
    )
    .await;
}

type ClientChannel = Channel<client::Msg>;
