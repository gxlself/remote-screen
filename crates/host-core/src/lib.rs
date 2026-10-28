//! Platform-independent host runtime and OS boundary.

use std::net::SocketAddr;
use std::path::PathBuf;
use std::sync::Arc;

use platform::{Nic, Platform};
use tokio::task::JoinSet;
use tracing::{info, warn};

pub mod activity_model;
pub mod audit;
pub mod auth;
pub mod auth_config;
pub mod capture_model;
pub mod clip_convert;
pub mod clock;
pub mod config;
pub mod cursor_shape;
pub mod display_model;
pub mod encode_model;
pub mod encoder_pick;
pub mod ffmpeg_abi;
pub mod file_recv;
pub mod file_send;
pub mod identity;
pub mod inject;
pub mod input;
pub mod ipc;
pub mod ipc_listener;
pub mod listen;
pub mod local_api;
pub mod logs;
pub mod nic_model;
pub mod owner;
pub mod pairfile_store;
pub mod pairing;
pub mod pipeline;
pub mod platform;
pub mod pwpair;
pub mod session;
pub mod store;
pub mod sysauth;
pub mod sysquery;
pub mod trust;
pub mod update;
pub mod user_dirs;
pub mod vdisplay;
pub mod whois;
pub mod wol;

pub use platform::PlatformError;
pub use platform::{Error, Result};

#[derive(Debug, thiserror::Error)]
pub enum RuntimeError {
    #[error(transparent)]
    Platform(#[from] PlatformError),
    #[error(transparent)]
    Io(#[from] std::io::Error),
    #[error(transparent)]
    Certificate(#[from] rcgen::Error),
    #[error(transparent)]
    Tls(#[from] rustls::Error),
    #[error(transparent)]
    TlsConfig(#[from] rsp_transport::tls::TlsError),
    #[error(transparent)]
    QuicCrypto(#[from] quinn::crypto::rustls::NoInitialCipherSuite),
    #[error(transparent)]
    Random(#[from] getrandom::Error),
    #[error(transparent)]
    Connection(#[from] quinn::ConnectionError),
    #[error(transparent)]
    Transport(#[from] rsp_transport::ConnError),
    #[error(transparent)]
    Listen(#[from] listen::ListenError),
    #[error(transparent)]
    Write(#[from] quinn::WriteError),
    #[error(transparent)]
    Read(#[from] quinn::ReadError),
    #[error(transparent)]
    Control(#[from] rsp_core::control::ControlError),
    #[error(transparent)]
    Identity(#[from] identity::IdentityError),
    #[error(transparent)]
    Store(#[from] store::StoreError),
    #[error(transparent)]
    Pairing(#[from] pairing::PairingError),
    #[error(transparent)]
    Decode(#[from] rsp_core::wire::DecodeError),
    #[error("no display is available")]
    NoDisplay,
    #[error("invalid runtime options: {0}")]
    InvalidOptions(&'static str),
}

pub type RuntimeResult<T> = std::result::Result<T, RuntimeError>;

#[derive(Debug, Clone)]
pub struct RuntimeOptions {
    /// None discovers tailnet NICs and the configured intranet NICs.
    pub bind: Option<SocketAddr>,
    pub view_only: bool,
    pub dump: Option<PathBuf>,
    pub listen_policy: listen::ListenPolicy,
    pub dev_allow_unpaired: bool,
    pub pair: bool,
    pub store_dir: PathBuf,
    pub auth_policy: auth::AuthPolicy,
    pub live_auth_policy: bool,
    /// Current sessions used when evaluating the authentication policy.
    pub session_snapshot: auth::SessionSnapshot,
    /// H6 supplies activity within the preceding 60 seconds.
    pub local_activity_within_60s: bool,
}

impl Default for RuntimeOptions {
    fn default() -> Self {
        Self {
            bind: None,
            view_only: false,
            dump: None,
            listen_policy: listen::ListenPolicy::default(),
            dev_allow_unpaired: false,
            pair: false,
            store_dir: default_state_dir(),
            auth_policy: auth::AuthPolicy::default(),
            live_auth_policy: false,
            session_snapshot: auth::SessionSnapshot::default(),
            local_activity_within_60s: false,
        }
    }
}

pub fn default_state_dir() -> PathBuf {
    let home = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."));
    if cfg!(target_os = "macos") {
        home.join("Library/Application Support/Remote Host")
    } else {
        std::env::var_os("XDG_STATE_HOME")
            .map(PathBuf::from)
            .unwrap_or_else(|| home.join(".local/state"))
            .join("remote-host")
    }
}

pub struct HostRuntime<P: Platform> {
    platform: Arc<P>,
    options: RuntimeOptions,
    store: Option<Arc<store::Store>>,
    connection_approver: Option<Arc<dyn session::handshake::ConnectionApprover>>,
    pairing_approver: Option<Arc<dyn pairing::PairApprover>>,
    pairing: Option<pairing::PairingManager>,
    audit: Option<audit::AuditQueue>,
    manager: Option<Arc<session::manager::SessionManager<P>>>,
    services: Option<session::manager::HostServices>,
}

/// IPC and tests can inspect ephemeral addresses and share the pairing ticket
/// before starting the accept loop.
pub struct BoundHost<P: Platform> {
    runtime: HostRuntime<P>,
    pub listener: listen::HostListener,
    pub manager: Arc<session::manager::SessionManager<P>>,
    pub security: Arc<session::handshake::HandshakeContext>,
}

impl<P: Platform> HostRuntime<P> {
    pub fn new(platform: P, options: RuntimeOptions) -> Self {
        Self {
            platform: Arc::new(platform),
            options,
            store: None,
            connection_approver: None,
            pairing_approver: None,
            pairing: None,
            audit: None,
            manager: None,
            services: None,
        }
    }
    pub fn with_manager(mut self, mgr: Arc<session::manager::SessionManager<P>>) -> Self {
        self.manager = Some(mgr);
        self
    }
    pub fn with_services(mut self, services: session::manager::HostServices) -> Self {
        self.services = Some(services);
        self
    }
    fn manager(&self) -> Arc<session::manager::SessionManager<P>> {
        let mgr = self.manager.clone().unwrap_or_else(|| {
            session::manager::SessionManager::new(
                self.platform.clone(),
                self.services.clone().unwrap_or_default(),
            )
        });
        mgr.start();
        mgr
    }
    pub fn with_store(mut self, store: Arc<store::Store>) -> Self {
        self.store = Some(store);
        self
    }
    pub fn with_audit(mut self, audit: audit::AuditQueue) -> Self {
        self.audit = Some(audit);
        self
    }
    pub fn with_connection_approver(
        mut self,
        approver: Arc<dyn session::handshake::ConnectionApprover>,
    ) -> Self {
        self.connection_approver = Some(approver);
        self
    }
    pub fn with_pairing_approver(mut self, approver: Arc<dyn pairing::PairApprover>) -> Self {
        self.pairing_approver = Some(approver);
        self
    }
    pub fn with_pairing_manager(mut self, pairing: pairing::PairingManager) -> Self {
        self.pairing = Some(pairing);
        self
    }

    fn validate(&self) -> RuntimeResult<()> {
        if self.options.dev_allow_unpaired
            && !self
                .options
                .bind
                .is_some_and(|addr| addr.ip().is_loopback())
        {
            return Err(RuntimeError::InvalidOptions(
                "--dev-allow-unpaired requires an explicit loopback --bind",
            ));
        }
        Ok(())
    }

    fn security(
        &self,
        identity: Arc<identity::HostIdentity>,
        addresses: Vec<SocketAddr>,
        mgr: &Arc<session::manager::SessionManager<P>>,
    ) -> RuntimeResult<Arc<session::handshake::HandshakeContext>> {
        let store = match &self.store {
            Some(store) => store.clone(),
            None => Arc::new(store::Store::open(&self.options.store_dir)?),
        };
        let mut pairing = self.pairing.clone().unwrap_or_else(|| {
            pairing::PairingManager::new(
                store.clone(),
                identity.fp(),
                self.platform.info().name,
                addresses,
            )
        });
        if !pairing.has_events() {
            pairing = pairing.with_events(mgr.services().events.clone());
        }
        pairing = pairing.with_password_lock(Arc::new(pwpair::PasswordLock::open(
            &self.options.store_dir,
            mgr.now_us(),
        )));
        if !pairing.has_files() {
            match pairfile_store::PairFileStore::open(&self.options.store_dir) {
                Ok(files) => pairing = pairing.with_files(Arc::new(files)),
                Err(error) => tracing::warn!(%error, "pairing files unavailable"),
            }
        }
        if let Some(approver) = self
            .pairing_approver
            .clone()
            .or_else(|| session::handlers::approval::pair_approver(mgr))
        {
            pairing = pairing.with_approver(approver);
        }
        if self.options.pair {
            let ticket = pairing.start(mgr.now_us())?;
            println!("pairing code {}\n{}", ticket.code, ticket.qr_url);
        }
        Ok(Arc::new(session::handshake::HandshakeContext {
            identity,
            store,
            pairing,
            approver: self.connection_approver.clone(),
            audit: self.audit.clone().or_else(|| mgr.services().audit.clone()),
            whois: Arc::new(whois::ChainedWhoIs::new(
                whois::LocalApiSource::default(),
                whois::CliSource::default(),
            )),
        }))
    }

    pub async fn bind(self) -> RuntimeResult<BoundHost<P>> {
        self.validate()?;
        let manager = self.manager();
        let identity = Arc::new(identity::HostIdentity::load_or_create(
            self.platform.keystore(),
            manager.now_us(),
        )?);
        let mut policy = self.options.listen_policy.clone();
        let nics = if let Some(bind) = self.options.bind {
            policy.port = bind.port();
            policy.intranet_nics.push("explicit-bind".into());
            if bind.ip().is_loopback() {
                let cidr = if bind.is_ipv4() {
                    "127.0.0.0/8"
                } else {
                    "::1/128"
                }
                .parse()
                .map_err(|_| RuntimeError::InvalidOptions("invalid loopback CIDR"))?;
                policy.allowed_cidrs = vec![cidr];
                policy.lan_cidrs = policy.allowed_cidrs.clone();
            }
            vec![Nic {
                name: "explicit-bind".into(),
                addresses: vec![bind.ip()],
                networks: vec![],
                mac: None,
                tailnet: false,
            }]
        } else {
            self.platform.nics()
        };
        let listener =
            listen::HostListener::start(policy, nics, Arc::new(self.tls(&identity)?)).await?;
        let addresses = listener
            .bound()
            .into_iter()
            .filter_map(|(addr, _, transport)| {
                (transport == rsp_transport::TransportKind::Quic).then_some(addr)
            })
            .collect::<Vec<_>>();
        if addresses.is_empty() {
            return Err(RuntimeError::InvalidOptions(
                "no QUIC listener bound; configure a tailnet NIC or --bind",
            ));
        }
        for address in &addresses {
            info!("listening on {address} (udp/quic, alpn rsp/1)");
        }
        let security = self.security(identity, addresses, &manager)?;
        Ok(BoundHost {
            runtime: self,
            listener,
            manager,
            security,
        })
    }

    fn tls(&self, identity: &identity::HostIdentity) -> RuntimeResult<rustls::ServerConfig> {
        if self.options.dev_allow_unpaired {
            warn!(
                "WARNING: --dev-allow-unpaired permits unpaired control; LOOPBACK DEVELOPMENT ONLY"
            );
        }
        // Keep the certificate pin log consumed by pre-P2 iOS clients.
        let cert_fp = ring::digest::digest(&ring::digest::SHA256, identity.cert_der());
        info!("certificate sha256 {}", hex(cert_fp.as_ref()));
        info!("host spki sha256 {}", hex(&identity.fp()));
        Ok(if self.options.dev_allow_unpaired {
            rsp_transport::tls::server_config_with_client_auth(
                &identity.local_identity(),
                rsp_transport::tls::ClientAuth::Optional,
            )?
        } else {
            rsp_transport::tls::server_config(&identity.local_identity())?
        })
    }

    pub async fn run(self) -> RuntimeResult<()> {
        self.bind().await?.run().await
    }

    /// Compatibility endpoint, configured with the same identity and strict
    /// TLS before the first accept. This supplied Quinn endpoint accepts only
    /// QUIC; new callers should use bind()/BoundHost for both transports.
    pub async fn serve(self, endpoint: quinn::Endpoint) -> RuntimeResult<()> {
        self.validate()?;
        let manager = self.manager();
        let identity = Arc::new(identity::HostIdentity::load_or_create(
            self.platform.keystore(),
            manager.now_us(),
        )?);
        let crypto = quinn::crypto::rustls::QuicServerConfig::try_from(self.tls(&identity)?)?;
        let mut config = quinn::ServerConfig::with_crypto(Arc::new(crypto));
        let mut transport = quinn::TransportConfig::default();
        // Match the capacity used by Conn's datagram backlog accounting.
        transport.datagram_send_buffer_size(rsp_transport::DATAGRAM_SEND_BUFFER_BYTES);
        config.transport_config(Arc::new(transport));
        endpoint.set_server_config(Some(config));
        let local = endpoint.local_addr()?;
        let security = self.security(identity, vec![local], &manager)?;
        let mut sessions = JoinSet::new();
        while let Some(incoming) = endpoint.accept().await {
            let path = if local.ip().is_loopback() {
                listen::PathClass::Lan
            } else {
                let Some(path) = listen::admit(
                    local,
                    incoming.remote_address(),
                    &self.options.listen_policy,
                ) else {
                    incoming.ignore();
                    continue;
                };
                path
            };
            match incoming.await {
                Ok(conn) => self.spawn_session(
                    conn.into(),
                    None,
                    local,
                    path,
                    security.clone(),
                    &mut sessions,
                    manager.clone(),
                ),
                Err(error) => warn!(%error, "TLS handshake failed"),
            }
        }
        sessions.abort_all();
        while sessions.join_next().await.is_some() {}
        Ok(())
    }

    #[allow(clippy::too_many_arguments)]
    fn spawn_session(
        &self,
        conn: rsp_transport::Conn,
        tcp_tokens: Option<rsp_transport::tcp::TcpTokenRegistry>,
        local: SocketAddr,
        path: listen::PathClass,
        security: Arc<session::handshake::HandshakeContext>,
        sessions: &mut JoinSet<()>,
        manager: Arc<session::manager::SessionManager<P>>,
    ) {
        let options = self.options.clone();
        sessions.spawn(async move {
            let remote = conn.remote_addr();
            let context = session::handshake::SessionContext {
                security,
                tcp_tokens,
                local,
                path,
            };
            if let Err(error) =
                session::conn::run_with_context(conn, manager, options, context).await
            {
                info!(%remote, %error, "session ended");
            }
        });
    }
}

impl<P: Platform> BoundHost<P> {
    pub async fn run(self) -> RuntimeResult<()> {
        let mut sessions = JoinSet::new();
        loop {
            let accepted = tokio::select! {
                accepted = self.listener.accept() => accepted?,
                _ = sessions.join_next(), if !sessions.is_empty() => continue,
            };
            self.runtime.spawn_session(
                accepted.conn,
                accepted.tcp_tokens,
                accepted.local,
                accepted.path,
                self.security.clone(),
                &mut sessions,
                self.manager.clone(),
            );
        }
    }
}

fn hex(bytes: &[u8]) -> String {
    bytes.iter().map(|byte| format!("{byte:02x}")).collect()
}

pub mod file_browse;
