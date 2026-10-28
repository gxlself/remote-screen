//! Platform selection and process lifecycle for the Remote Screen host.

use std::net::{IpAddr, Ipv4Addr, SocketAddr};
use std::path::PathBuf;
use std::sync::{Arc, OnceLock};

use anyhow::{bail, Context, Result};
use clap::{Parser, ValueEnum};
use host_core::platform::Platform;
use host_core::{
    auth_config::auth_policy_from_config,
    ipc::*,
    local_api::{ExportDir, LocalApi},
    logs,
    owner::{self, OwnerOptions},
    pairing::{IpcPairApprover, PairApproval, PairingManager},
    session::{
        handshake::IpcConnectionApprover,
        manager::{HostServices, SessionControl, SessionManager},
    },
    update::agent::UpdateService,
    user_dirs::{DirsOwner, EnvDirs, UserDirsSource},
    whois::{ChainedWhoIs, CliSource, LocalApiSource},
};
use host_core::{HostRuntime, RuntimeOptions};
use host_fake::FakePlatform;
use tokio::task::JoinSet;
use tracing::warn;

mod host_net;
mod host_status;

#[cfg(target_os = "linux")]
mod linux;

#[derive(Debug, Default, Clone, Copy, PartialEq, Eq, ValueEnum)]
enum PlatformKind {
    #[cfg(target_os = "macos")]
    #[default]
    Mac,
    #[cfg(target_os = "linux")]
    Linux,
    #[cfg_attr(not(target_os = "macos"), default)]
    Fake,
}

#[cfg(windows)]
mod win_service;

mod bind_guard;
mod devices_ipc;

#[derive(Debug, Clone, Parser)]
#[command(name = "rs-host", about = "Remote Screen host agent")]
struct Args {
    #[arg(long, hide = true)]
    service: bool,
    // Legacy uninstaller hook: accepted, hidden, and exits
    // immediately without touching state. Remove after one release.
    #[arg(long, hide = true, conflicts_with = "service")]
    notify_uninstall: bool,
    /// Run the service in the current console without SCM (manual testing).
    #[arg(long, hide = true, requires = "service")]
    foreground: bool,
    #[arg(long, value_enum, default_value_t = PlatformKind::default())]
    platform: PlatformKind,
    /// Listen on an IP or IP:port; may be repeated. Defaults to a tailnet IP (Mac) or loopback (fake).
    #[arg(long)]
    bind: Vec<String>,
    /// Default port for --bind values without a port.
    #[arg(long, default_value_t = rsp_core::DEFAULT_PORT)]
    port: u16,
    #[arg(long)]
    view_only: bool,
    #[arg(long, default_value = "info")]
    log: String,
    /// Directory for file logs when --log is a single level.
    #[arg(long)]
    log_dir: Option<PathBuf>,
    /// Write the encoded Annex-B stream to this file.
    #[arg(long)]
    dump: Option<PathBuf>,
    #[arg(long)]
    config: Option<PathBuf>,
    #[arg(long)]
    print_pair_code: bool,
    /// Development mode: allow clients without a device certificate (loopback only).
    #[arg(long)]
    dev_allow_unpaired: bool,
    /// Start a six-digit pairing ticket at startup.
    #[arg(long)]
    pair: bool,
    /// Persistent pairing table and fake-platform key directory.
    #[arg(long)]
    state_dir: Option<PathBuf>,
    /// Local JSON-RPC socket (defaults to <state-dir>/agent.sock).
    #[arg(long)]
    ipc_socket: Option<PathBuf>,
    /// Authentication error injection (fake platform only).
    #[arg(long)]
    fail: Option<String>,
}

#[cfg(test)]
fn is_tailnet(ip: Ipv4Addr) -> bool {
    let bytes = ip.octets();
    bytes[0] == 100 && (bytes[1] & 0xc0) == 0x40
}

fn bind_addresses(args: &Args) -> Result<Vec<SocketAddr>> {
    if !args.bind.is_empty() {
        return args
            .bind
            .iter()
            .map(|value| {
                value
                    .parse::<SocketAddr>()
                    .or_else(|_| {
                        value
                            .parse::<IpAddr>()
                            .map(|ip| SocketAddr::new(ip, args.port))
                    })
                    .with_context(|| format!("invalid --bind address: {value}"))
            })
            .collect();
    }
    if args.platform == PlatformKind::Fake {
        return Ok(vec![SocketAddr::from((Ipv4Addr::LOCALHOST, args.port))]);
    }
    Ok(Vec::new())
}

async fn run_platform<P: Platform + Clone>(
    platform: P,
    args: &Args,
    log_dir: Option<PathBuf>,
) -> Result<()> {
    let forced_denial = forced_denial(args)?;
    let state_dir = args
        .state_dir
        .clone()
        .unwrap_or_else(host_core::default_state_dir);
    let config_path = args
        .config
        .clone()
        .unwrap_or_else(|| state_dir.join("config.toml"));
    let config = host_core::config::load(&config_path)
        .with_context(|| format!("load config {}", config_path.display()))?;
    let store = Arc::new(host_core::store::Store::open(&state_dir)?);
    let audit = host_core::audit::AuditQueue::open(&state_dir)?;
    let backend = LocalBackend::new(store.clone(), state_dir.clone());
    let ipc = IpcServer::bind(
        args.ipc_socket
            .clone()
            .unwrap_or_else(|| state_dir.join("agent.sock")),
        backend.clone(),
    )?;
    let mut binds = bind_addresses(args)?
        .into_iter()
        .map(Some)
        .collect::<Vec<_>>();
    if binds.is_empty() {
        binds.push(None);
    }
    if args.dev_allow_unpaired
        && (args.bind.is_empty()
            || binds
                .iter()
                .any(|bind| !bind.is_some_and(|addr| addr.ip().is_loopback())))
    {
        bail!("--dev-allow-unpaired requires an explicit loopback --bind");
    }
    let config_store = host_core::config::ConfigStore::new(config_path, config.clone());
    #[cfg(target_os = "linux")]
    let downloads_dir = linux::downloads_dir();
    #[cfg(not(target_os = "linux"))]
    let downloads_dir = std::env::var_os("HOME")
        .map(PathBuf::from)
        .unwrap_or_else(|| PathBuf::from("."))
        .join("Downloads");
    let mgr = SessionManager::new(
        Arc::new(platform.clone()),
        HostServices {
            events: Arc::new(ipc.notifier()),
            config: config_store.clone(),
            audit: Some(audit.clone()),
            downloads_dir: downloads_dir.clone(),
            state_dir: Some(state_dir.clone()),
            clock: Arc::new(host_core::clock::SystemClock),
        },
    );
    backend.set_devices(Arc::new(devices_ipc::DevicesApiImpl::new(
        mgr.clone(),
        store.clone(),
    )));
    let _ = backend.control.set(mgr.session_control());
    let mut api = LocalApi::new(config_store.clone()).with_audit(audit.clone());
    if let Some(log_dir) = log_dir {
        #[cfg(target_os = "linux")]
        let export_dir: ExportDir = {
            let downloads_dir = downloads_dir.clone();
            Arc::new(move || Some(downloads_dir.clone()))
        };
        #[cfg(not(target_os = "linux"))]
        let export_dir: ExportDir =
            Arc::new(|| std::env::var_os("HOME").map(|home| PathBuf::from(home).join("Downloads")));
        api = api.with_logs(log_dir, export_dir);
    }
    backend.set_api(api);
    backend.set_user_dirs(Arc::new(EnvDirs::new(downloads_dir)));
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let (listening, _) = tokio::sync::watch::channel(false);
    let (bind_signal, _) = tokio::sync::watch::channel(false);
    backend.set_status(Arc::new(platform.clone()));
    backend.set_listening(bind_signal.subscribe());
    backend.set_network(Arc::new(host_net::PlatformNetworkInfo::new(
        platform.clone(),
        config_store.clone(),
        args.port,
    )));
    let permission_platform = platform.clone();
    let permission_watcher = host_status::PermissionWatcher::with_default_interval(
        Arc::new(move || permission_platform.permissions()),
        Arc::new(ipc.notifier()),
    )
    .spawn();
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    let mut update_service: Option<Arc<UpdateService>> = None;
    let mut listeners = JoinSet::new();
    let mut network_tasks = JoinSet::new();
    let mut owner_task = None;

    #[cfg(target_os = "macos")]
    if args.platform == PlatformKind::Mac {
        use host_core::update::{
            agent::{
                self, UpdatePlatform, UpdateWiring, DEFAULT_CHECK_INTERVAL, DEFAULT_INITIAL_DELAY,
            },
            manifest::TrustedKeys,
            remote::CurlDownloader,
        };
        use host_mac::{
            update_install::MacAppInstaller,
            update_verify::{CodeSigning, MacCodeVerifier, SecCodeSigning},
        };

        let installer = MacAppInstaller::for_current_app().and_then(|installer| {
            // The installer currently also accepts unsigned bundles.
            SecCodeSigning
                .current_team_id()?
                .ok_or_else(|| "current app has no signing Team ID".to_owned())?;
            Ok(installer)
        });
        match installer {
            Ok(installer) => {
                let platform = UpdatePlatform {
                    installer: Arc::new(installer),
                    verifier: Arc::new(MacCodeVerifier::default()),
                    downloader: Arc::new(CurlDownloader {
                        program: PathBuf::from("/usr/bin/curl"),
                        ..Default::default()
                    }),
                };
                let svc = agent::attach(
                    &mgr,
                    UpdateWiring {
                        work_dir: state_dir.join("update"),
                        current_version: env!("CARGO_PKG_VERSION").into(),
                        keys: TrustedKeys::builtin(),
                        platform,
                        source: None,
                        listening: listening.subscribe(),
                        audit: Some(audit.clone()),
                        initial_delay: DEFAULT_INITIAL_DELAY,
                        check_interval: DEFAULT_CHECK_INTERVAL,
                    },
                );
                backend.set_updates(svc.clone());
                update_service = Some(svc);
            }
            Err(error) => tracing::info!(
                error_kind = "current_app_unavailable",
                reason = %error,
                "automatic updates disabled: not running from a signed app bundle"
            ),
        }
    }

    #[cfg(target_os = "linux")]
    if args.platform == PlatformKind::Linux {
        use host_core::update::{
            agent::{self, UpdateWiring, DEFAULT_CHECK_INTERVAL, DEFAULT_INITIAL_DELAY},
            manifest::TrustedKeys,
        };

        if let Some(platform) = linux::update_platform() {
            let svc = agent::attach(
                &mgr,
                UpdateWiring {
                    work_dir: state_dir.join("update"),
                    current_version: env!("CARGO_PKG_VERSION").into(),
                    keys: TrustedKeys::builtin(),
                    platform,
                    source: None,
                    listening: listening.subscribe(),
                    audit: Some(audit.clone()),
                    initial_delay: DEFAULT_INITIAL_DELAY,
                    check_interval: DEFAULT_CHECK_INTERVAL,
                },
            );
            backend.set_updates(svc.clone());
            update_service = Some(svc);
        }
    }

    for bind in binds {
        let dynamic_bind = bind.is_none();
        let mut options = RuntimeOptions {
            bind,
            view_only: args.view_only,
            dump: args.dump.clone(),
            dev_allow_unpaired: args.dev_allow_unpaired,
            pair: (args.pair || args.print_pair_code) && backend.pairing.get().is_none(),
            store_dir: state_dir.clone(),
            ..RuntimeOptions::default()
        };
        {
            options.auth_policy = auth_policy_from_config(&config);
            options.auth_policy.forced_denial = forced_denial;
            options.live_auth_policy = true;
            options.listen_policy = host_net::listen_policy(&config, args.port);
        }
        // Leave approvers unset so the agent uses the shared LocalPrompts registry.
        let mut runtime = HostRuntime::new(platform.clone(), options)
            .with_manager(mgr.clone())
            .with_store(store.clone())
            .with_audit(audit.clone());
        if let Some(pairing) = backend.pairing.get() {
            runtime = runtime.with_pairing_manager(pairing.clone());
        }
        #[cfg(any(target_os = "macos", target_os = "linux"))]
        let host = {
            let guard = update_service
                .as_deref()
                .map(|service| service as &dyn bind_guard::OutcomeWaiter);
            bind_guard::bind_then_signal(|| async { runtime.bind().await }, &bind_signal, guard)
                .await?
        };
        #[cfg(not(any(target_os = "macos", target_os = "linux")))]
        let host =
            bind_guard::bind_then_signal(|| async { runtime.bind().await }, &bind_signal, None)
                .await?;
        if owner_task.is_none() {
            owner_task = Some(owner::spawn(
                mgr.clone(),
                OwnerOptions::new(Arc::new(ChainedWhoIs::new(
                    LocalApiSource::default(),
                    CliSource::default(),
                ))),
            ));
        }
        let _ = backend.pairing.set(host.security.pairing.clone());
        if dynamic_bind {
            network_tasks.spawn(host_net::run_listener_updates(
                config_store.clone(),
                platform.clone(),
                args.port,
                Arc::new(host.listener.updater()),
            ));
        }
        listeners.spawn(host.run());
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    listening.send_replace(true);
    let result = tokio::select! {
        signal = tokio::signal::ctrl_c() => signal.context("install Ctrl-C handler"),
        result = ipc.serve() => result.map_err(Into::into),
        result = listeners.join_next() => match result {
            Some(result) => result.context("listener task failed")?.map_err(Into::into),
            None => Ok(()),
        },
    };
    listeners.abort_all();
    while listeners.join_next().await.is_some() {}
    network_tasks.abort_all();
    while network_tasks.join_next().await.is_some() {}
    permission_watcher.abort();
    let _ = permission_watcher.await;
    if let Some(task) = owner_task {
        task.abort();
        let _ = task.await;
    }
    #[cfg(any(target_os = "macos", target_os = "linux"))]
    if let Some(svc) = backend.updates.get() {
        svc.shutdown();
    }
    result
}

fn forced_denial(args: &Args) -> Result<Option<rsp_core::errors::ErrorCode>> {
    let Some(value) = args.fail.as_deref() else {
        return Ok(None);
    };
    match host_core::auth::parse_fail_code(value) {
        Ok(code) => Ok(Some(code)),
        Err(error) => bail!("--fail: {error}"),
    }
}

trait NetworkInfo: Send + Sync + 'static {
    fn list_nics(&self) -> Vec<Value>;
    fn firewall_status(&self) -> NetworkFirewallStatusResult;
    fn it_note(&self) -> String;
}

#[derive(Clone)]
struct LocalBackend {
    network: Arc<OnceLock<Arc<dyn NetworkInfo>>>,
    status: Arc<OnceLock<Arc<dyn host_status::StatusSource>>>,
    listening: Arc<OnceLock<tokio::sync::watch::Receiver<bool>>>,
    api: Arc<OnceLock<LocalApi>>,
    updates: Arc<OnceLock<Arc<UpdateService>>>,
    user_dirs: Arc<OnceLock<Arc<dyn UserDirsSource>>>,
    control: Arc<OnceLock<Arc<dyn SessionControl>>>,
    devices: Arc<OnceLock<Arc<dyn devices_ipc::DevicesApi>>>,
    store: Arc<host_core::store::Store>,
    state_dir: PathBuf,
    pairing: Arc<OnceLock<PairingManager>>,
    connection: Arc<OnceLock<Arc<IpcConnectionApprover>>>,
    pair_approval: Arc<OnceLock<Arc<IpcPairApprover>>>,
}

impl LocalBackend {
    fn new(store: Arc<host_core::store::Store>, state_dir: PathBuf) -> Self {
        Self {
            network: Arc::new(OnceLock::new()),
            status: Arc::new(OnceLock::new()),
            listening: Arc::new(OnceLock::new()),
            api: Arc::new(OnceLock::new()),
            updates: Arc::new(OnceLock::new()),
            user_dirs: Arc::new(OnceLock::new()),
            control: Arc::new(OnceLock::new()),
            devices: Arc::new(OnceLock::new()),
            store,
            state_dir,
            pairing: Arc::new(OnceLock::new()),
            connection: Arc::new(OnceLock::new()),
            pair_approval: Arc::new(OnceLock::new()),
        }
    }
    fn set_api(&self, api: LocalApi) {
        let _ = self.api.set(api);
    }

    // Platform wiring will call this once an Installer/Verifier is available.
    #[allow(dead_code)]
    fn set_updates(&self, svc: Arc<UpdateService>) {
        let _ = self.updates.set(svc);
    }

    fn set_user_dirs(&self, source: Arc<dyn UserDirsSource>) {
        let _ = self.user_dirs.set(source);
    }

    fn set_devices(&self, devices: Arc<dyn devices_ipc::DevicesApi>) {
        let _ = self.devices.set(devices);
    }

    fn api(&self) -> std::result::Result<&LocalApi, IpcError> {
        self.api
            .get()
            .ok_or_else(|| IpcError::business("not_ready", "local API is not ready"))
    }

    fn set_network(&self, info: Arc<dyn NetworkInfo>) {
        let _ = self.network.set(info);
    }

    fn set_status(&self, source: Arc<dyn host_status::StatusSource>) {
        let _ = self.status.set(source);
    }

    fn set_listening(&self, listening: tokio::sync::watch::Receiver<bool>) {
        let _ = self.listening.set(listening);
    }

    async fn network_call<T: Send + 'static>(
        &self,
        method: &'static str,
        call: impl FnOnce(Arc<dyn NetworkInfo>) -> T + Send + 'static,
    ) -> std::result::Result<T, IpcError> {
        let info = self.network.get().cloned().ok_or_else(|| {
            IpcError::business(
                "unsupported",
                format!("{method} is not wired in this agent"),
            )
        })?;
        tokio::task::spawn_blocking(move || call(info))
            .await
            .map_err(|error| IpcError::business("internal", error.to_string()))
    }

    fn pairing(&self) -> std::result::Result<&PairingManager, IpcError> {
        self.pairing
            .get()
            .ok_or_else(|| IpcError::business("not_ready", "listener is not ready"))
    }
}

impl IpcBackend for LocalBackend {
    async fn status_get(&self) -> std::result::Result<StatusGetResult, IpcError> {
        let source = self.status.get().cloned();
        let platform_status = tokio::task::spawn_blocking(move || {
            source.map(|source| {
                let permissions = source.permissions();
                let tailnet_addresses = host_status::tailnet_addresses(&source.nics());
                (
                    host_status::permissions_json(permissions),
                    tailnet_addresses,
                )
            })
        })
        .await
        .map_err(|error| IpcError::business("internal", error.to_string()))?;
        let (permissions, tailnet_addresses) =
            platform_status.unwrap_or_else(|| (serde_json::json!({}), Vec::new()));
        let listening = self.listening.get().is_none_or(|signal| *signal.borrow());
        Ok(StatusGetResult {
            online: true,
            tailnet_addresses,
            permissions,
            listening,
            version: env!("CARGO_PKG_VERSION").into(),
            sessions: self
                .control
                .get()
                .map_or_else(Vec::new, |control| control.sessions_list()),
        })
    }
    async fn pairing_start(&self) -> std::result::Result<PairingStartResult, IpcError> {
        let ticket = self
            .pairing()?
            .start(rsp_core::clock::unix_micros())
            .map_err(|error| IpcError::business("pairing", error.to_string()))?;
        Ok(PairingStartResult {
            code: ticket.code,
            qr_url: ticket.qr_url,
            expires_at: ticket.expires_at,
        })
    }
    async fn pairing_cancel(
        &self,
        _: PairingCancelParams,
    ) -> std::result::Result<PairingCancelResult, IpcError> {
        self.pairing()?
            .cancel()
            .map_err(|error| IpcError::business("pairing", error.to_string()))?;
        Ok(EmptyResult {})
    }
    async fn pairing_export_file(
        &self,
        params: PairingExportFileParams,
    ) -> std::result::Result<PairingExportFileResult, IpcError> {
        let dir = match params.dir {
            Some(dir) => {
                let dir = PathBuf::from(dir);
                if !dir.is_absolute() {
                    return Err(IpcError::business(
                        "invalid_params",
                        "dir must be an absolute path",
                    ));
                }
                if self
                    .user_dirs
                    .get()
                    .is_some_and(|source| !source.user_can_write(&dir))
                {
                    return Err(IpcError::business("forbidden", "无权写入该目录"));
                }
                dir
            }
            None => self
                .user_dirs
                .get()
                .map(|source| source.current())
                .filter(|dirs| dirs.owner == DirsOwner::User)
                .map(|dirs| dirs.downloads)
                .unwrap_or_else(|| host_core::pairing::default_export_dir(&self.state_dir)),
        };
        if params
            .label
            .as_deref()
            .is_some_and(|label| label.chars().count() > 64)
        {
            return Err(IpcError::business(
                "invalid_params",
                "label must be at most 64 characters",
            ));
        }
        let permission = trust_permission(params.permission.unwrap_or(Permission::Control));
        let exported = self
            .pairing()?
            .export_file(
                rsp_core::clock::unix_micros(),
                host_core::pairing::ExportFileRequest {
                    permission,
                    label: params.label,
                    dir,
                },
            )
            .map_err(pairing_file_error)?;
        Ok(PairingExportFileResult {
            file_id: exported.file_id,
            path: exported.path.to_string_lossy().into_owned(),
            file_name: exported.file_name,
            expires_at: exported.expires_at_us,
        })
    }
    async fn pairing_list_files(&self) -> std::result::Result<PairingListFilesResult, IpcError> {
        let files = self
            .pairing()?
            .list_files(rsp_core::clock::unix_micros())
            .map_err(pairing_file_error)?;
        Ok(PairingListFilesResult {
            files: files
                .into_iter()
                .map(|file| PairFileInfo {
                    file_id: file.file_id,
                    label: file.label,
                    permission: ipc_permission(file.permission),
                    created_at: file.created_at_us,
                    expires_at: file.expires_at_us,
                    state: match file.state {
                        host_core::pairfile_store::PairFileState::Active => PairFileState::Active,
                        host_core::pairfile_store::PairFileState::Used => PairFileState::Used,
                        host_core::pairfile_store::PairFileState::Failed => PairFileState::Failed,
                        host_core::pairfile_store::PairFileState::Revoked => PairFileState::Revoked,
                        host_core::pairfile_store::PairFileState::Expired => PairFileState::Expired,
                    },
                    used_by_device_id: file.used_by_device_id,
                    used_at: file.used_at_us,
                })
                .collect(),
        })
    }
    async fn pairing_revoke_file(
        &self,
        params: PairingRevokeFileParams,
    ) -> std::result::Result<EmptyResult, IpcError> {
        self.pairing()?
            .revoke_file(&params.file_id)
            .map_err(pairing_file_error)?;
        Ok(EmptyResult {})
    }
    async fn approval_pending(&self) -> std::result::Result<ApprovalPendingResult, IpcError> {
        Ok(ApprovalPendingResult {
            requests: self
                .control
                .get()
                .map(|c| c.approval_pending())
                .unwrap_or_default(),
        })
    }
    async fn approval_respond(
        &self,
        params: ApprovalRespondParams,
    ) -> std::result::Result<ApprovalRespondResult, IpcError> {
        if self.control.get().is_some_and(|control| {
            control.approval_respond(&params.request_id, params.allow, params.permission)
        }) {
            return Ok(EmptyResult {});
        }
        if self
            .connection
            .get()
            .is_some_and(|approver| approver.respond(&params.request_id, params.allow))
        {
            return Ok(EmptyResult {});
        }
        let permission = match params.permission.unwrap_or(Permission::Control) {
            Permission::ViewOnly => rsp_core::trust::Permission::ViewOnly,
            Permission::Control => rsp_core::trust::Permission::Control,
            Permission::Unattended => rsp_core::trust::Permission::Unattended,
        };
        if self.pair_approval.get().is_some_and(|approver| {
            approver.respond(
                &params.request_id,
                PairApproval {
                    allow: params.allow,
                    permission,
                },
            )
        }) {
            return Ok(EmptyResult {});
        }
        Err(IpcError::business(
            "unknown_request",
            "approval request expired or is unknown",
        ))
    }
    async fn devices_list(&self) -> std::result::Result<DevicesListResult, IpcError> {
        let devices = self
            .store
            .paired()
            .map_err(|error| IpcError::business("store", error.to_string()))?
            .into_iter()
            .map(|device| DeviceSummary {
                device_id: device.device_id,
                name: device.name,
                permission: match device.permission {
                    rsp_core::trust::Permission::ViewOnly => Permission::ViewOnly,
                    rsp_core::trust::Permission::Control => Permission::Control,
                    rsp_core::trust::Permission::Unattended => Permission::Unattended,
                },
                paths: devices_ipc::path_names(&device.path_permissions),
                ..Default::default()
            })
            .collect();
        Ok(DevicesListResult { devices })
    }
    async fn sessions_list(&self) -> std::result::Result<SessionsListResult, IpcError> {
        Ok(SessionsListResult {
            sessions: self
                .control
                .get()
                .map_or_else(Vec::new, |control| control.sessions_list()),
        })
    }
    async fn sessions_disconnect(
        &self,
        params: SessionsDisconnectParams,
    ) -> std::result::Result<SessionsDisconnectResult, IpcError> {
        if self
            .control
            .get()
            .is_some_and(|control| control.disconnect(&params.session_id))
        {
            Ok(EmptyResult {})
        } else {
            Err(IpcError::business("unknown_session", "session is unknown"))
        }
    }
    async fn sessions_set_view_only(
        &self,
        params: SessionsSetViewOnlyParams,
    ) -> std::result::Result<SessionsSetViewOnlyResult, IpcError> {
        if self
            .control
            .get()
            .is_some_and(|control| control.set_view_only(&params.session_id))
        {
            Ok(EmptyResult {})
        } else {
            Err(IpcError::business("unknown_session", "session is unknown"))
        }
    }
    async fn devices_update(
        &self,
        params: DevicesUpdateParams,
    ) -> std::result::Result<DevicesUpdateResult, IpcError> {
        let devices = self.devices.get().ok_or_else(|| {
            IpcError::business("unsupported", "devices_update is not wired in this agent")
        })?;
        devices.update(params).await
    }
    async fn devices_revoke(
        &self,
        params: DevicesRevokeParams,
    ) -> std::result::Result<DevicesRevokeResult, IpcError> {
        let devices = self.devices.get().ok_or_else(|| {
            IpcError::business("unsupported", "devices_revoke is not wired in this agent")
        })?;
        devices.revoke(params).await
    }
    async fn settings_get(&self) -> std::result::Result<Value, IpcError> {
        self.api()?.settings_get().await
    }
    async fn settings_set(&self, settings: Value) -> std::result::Result<Value, IpcError> {
        self.api()?.settings_set(settings).await
    }
    async fn network_list_nics(&self) -> std::result::Result<NetworkListNicsResult, IpcError> {
        self.network_call("network_list_nics", |info| NetworkListNicsResult {
            nics: info.list_nics(),
        })
        .await
    }
    async fn network_firewall_status(
        &self,
    ) -> std::result::Result<NetworkFirewallStatusResult, IpcError> {
        self.network_call("network_firewall_status", |info| info.firewall_status())
            .await
    }
    async fn network_it_note(&self) -> std::result::Result<NetworkItNoteResult, IpcError> {
        self.network_call("network_it_note", |info| NetworkItNoteResult {
            text: info.it_note(),
        })
        .await
    }
    async fn update_status(&self) -> std::result::Result<UpdateStatusResult, IpcError> {
        let svc = self.updates.get().ok_or_else(|| {
            IpcError::business("unsupported", "update_status is not wired in this agent")
        })?;
        Ok(svc.ipc_status())
    }
    async fn update_install_now(&self) -> std::result::Result<UpdateInstallNowResult, IpcError> {
        let svc = self.updates.get().ok_or_else(|| {
            IpcError::business(
                "unsupported",
                "update_install_now is not wired in this agent",
            )
        })?;
        svc.install_now()
            .map_err(|unavailable| IpcError::business(unavailable.code(), unavailable.message()))?;
        Ok(EmptyResult {})
    }
    async fn update_set_channel(
        &self,
        params: UpdateSetChannelParams,
    ) -> std::result::Result<UpdateSetChannelResult, IpcError> {
        self.api()?.update_set_channel(params).await
    }
    async fn logs_export(
        &self,
        params: LogsExportParams,
    ) -> std::result::Result<LogsExportResult, IpcError> {
        self.api()?.logs_export(params).await
    }
    async fn audit_list(
        &self,
        params: AuditListParams,
    ) -> std::result::Result<AuditListResult, IpcError> {
        self.api()?.audit_list(params).await
    }
}

fn pairing_file_error(error: host_core::pairing::PairingError) -> IpcError {
    use host_core::pairfile_store::PairFileError;
    use host_core::pairing::PairingError;

    match error {
        PairingError::PairFile(PairFileError::NotFound) => {
            IpcError::business("not_found", "pairing file not found")
        }
        PairingError::PairFile(PairFileError::TooMany) => {
            IpcError::business("too_many_files", "too many active pairing files")
        }
        error @ (PairingError::InvalidPath
        | PairingError::PairFile(PairFileError::InvalidLabel)) => {
            IpcError::business("invalid_params", error.to_string())
        }
        error @ (PairingError::Export(_) | PairingError::PairFile(PairFileError::Io(_))) => {
            IpcError::business("io", error.to_string())
        }
        PairingError::FilesUnavailable => {
            IpcError::business("unsupported", "pairing files are unavailable on this host")
        }
        other => IpcError::business("pairing", other.to_string()),
    }
}

fn trust_permission(permission: Permission) -> rsp_core::trust::Permission {
    match permission {
        Permission::ViewOnly => rsp_core::trust::Permission::ViewOnly,
        Permission::Control => rsp_core::trust::Permission::Control,
        Permission::Unattended => rsp_core::trust::Permission::Unattended,
    }
}

fn ipc_permission(permission: rsp_core::trust::Permission) -> Permission {
    match permission {
        rsp_core::trust::Permission::ViewOnly => Permission::ViewOnly,
        rsp_core::trust::Permission::Control => Permission::Control,
        rsp_core::trust::Permission::Unattended => Permission::Unattended,
    }
}

fn service_entry(args: &Args) -> Option<Result<()>> {
    if !args.service {
        return None;
    }
    #[cfg(windows)]
    {
        Some(win_service::run(args))
    }
    #[cfg(not(windows))]
    {
        Some(Err(anyhow::anyhow!(
            "--service is only available on Windows"
        )))
    }
}

fn is_simple_log_level(filter: &str) -> bool {
    matches!(filter, "trace" | "debug" | "info" | "warn" | "error")
}

fn file_log_dir(args: &Args) -> Option<PathBuf> {
    if !is_simple_log_level(&args.log) {
        return None;
    }
    if let Some(dir) = &args.log_dir {
        return Some(dir.clone());
    }
    #[cfg(target_os = "macos")]
    if let Some(home) = std::env::var_os("HOME") {
        return Some(PathBuf::from(home).join("Library/Logs/Remote Host"));
    }
    Some(
        args.state_dir
            .clone()
            .unwrap_or_else(host_core::default_state_dir)
            .join("logs"),
    )
}

fn init_stderr_logging(filter: &str) -> Result<()> {
    tracing_subscriber::fmt()
        .with_env_filter(
            tracing_subscriber::EnvFilter::try_new(filter).context("invalid --log filter")?,
        )
        .with_writer(std::io::stderr)
        .try_init()
        .map_err(|error| anyhow::anyhow!(error.to_string()))
}

fn main() -> Result<()> {
    let args = Args::parse();
    if let Some(result) = service_entry(&args) {
        return result;
    }
    if args.notify_uninstall {
        return Ok(());
    }
    let log_dir = file_log_dir(&args);
    let _log_guard = if let Some(dir) = &log_dir {
        Some(logs::init(dir, &args.log).context("initialize file logging")?)
    } else {
        init_stderr_logging(&args.log)?;
        None
    };
    tokio::runtime::Builder::new_multi_thread()
        .enable_all()
        .build()?
        .block_on(run_cli(args, log_dir))
}

async fn run_cli(args: Args, log_dir: Option<PathBuf>) -> Result<()> {
    if args.fail.is_some() && args.platform != PlatformKind::Fake {
        bail!("--fail is only available with --platform fake");
    }
    if let Some(code) = forced_denial(&args)? {
        warn!(
            "--fail active: every authentication returns {}",
            code.as_str()
        );
    }
    match args.platform {
        PlatformKind::Fake => {
            run_platform(
                FakePlatform::with_state_dir(
                    args.state_dir
                        .clone()
                        .unwrap_or_else(host_core::default_state_dir)
                        .join("keys"),
                ),
                &args,
                log_dir,
            )
            .await
        }
        #[cfg(target_os = "macos")]
        PlatformKind::Mac => {
            if !host_mac::screen_capture_allowed(true) {
                warn!("screen recording permission is required; grant it in System Settings and restart");
            }
            if !args.view_only && !host_mac::accessibility_trusted(true) {
                warn!("accessibility permission is required for input injection");
            }
            run_platform(host_mac::MacPlatform::new(), &args, log_dir).await
        }
        #[cfg(target_os = "linux")]
        PlatformKind::Linux => {
            let state_dir = args
                .state_dir
                .clone()
                .unwrap_or_else(host_core::default_state_dir);
            run_platform(
                linux::platform(state_dir, args.view_only).await?,
                &args,
                log_dir,
            )
            .await
        }
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    fn test_backend(name: &str) -> (PathBuf, LocalBackend) {
        let dir = std::env::temp_dir().join(format!(
            "h23c-{name}-{}-{}",
            std::process::id(),
            rsp_core::clock::unix_micros()
        ));
        let backend = LocalBackend::new(
            Arc::new(host_core::store::Store::open(&dir).unwrap()),
            dir.clone(),
        );
        (dir, backend)
    }

    #[test]
    fn simple_log_levels_exclude_env_filters_and_targets() {
        for level in ["trace", "debug", "info", "warn", "error"] {
            assert!(is_simple_log_level(level), "{level}");
        }
        for filter in ["host_core=debug", "info,hyper=warn", "host_core", "off", ""] {
            assert!(!is_simple_log_level(filter), "{filter}");
        }
    }

    #[tokio::test]
    async fn devices_backend_delegates_update_to_store() {
        use host_core::store::{DevicePlatform, PairedDevice, PairingPath, PathPermissions};

        let dir = PathBuf::from(format!(
            "target/agent-devices-delegation-{}",
            std::process::id()
        ));
        let _ = std::fs::remove_dir_all(&dir);
        let store = Arc::new(host_core::store::Store::open(&dir).expect("store"));
        let identity = host_fake::harness::app_identity();
        let fp = identity.fingerprint();
        let device_id = rsp_core::ids::id_from_fp(&fp);
        store
            .upsert(PairedDevice {
                fp,
                device_id: device_id.clone(),
                name: "test device".into(),
                cert_der: identity.cert_der.as_ref().to_vec(),
                permission: rsp_core::trust::Permission::Control,
                paired_at_us: 1,
                require_confirm: false,
                platform: DevicePlatform::Ios,
                paired_via: PairingPath::Lan,
                whois_user: None,
                whois_node: None,
                whois_node_name: None,
                path_permissions: PathPermissions::default(),
            })
            .expect("pair device");
        let backend = LocalBackend::new(store.clone(), dir.clone());
        let manager = SessionManager::new(Arc::new(FakePlatform::new()), HostServices::default());
        backend.set_devices(Arc::new(devices_ipc::DevicesApiImpl::new(
            manager,
            store.clone(),
        )));
        backend
            .devices_update(DevicesUpdateParams {
                device_id,
                permission: Permission::ViewOnly,
                paths: vec!["lan".into()],
            })
            .await
            .expect("delegated update");
        let updated = store.get(&fp).expect("get device").expect("paired device");
        assert_eq!(updated.permission, rsp_core::trust::Permission::ViewOnly);
        assert_eq!(
            updated.path_permissions.tailnet,
            host_core::store::PathPermission::Deny
        );
        assert_eq!(
            updated.path_permissions.intranet,
            host_core::store::PathPermission::Deny
        );
        assert_eq!(
            updated.path_permissions.lan,
            host_core::store::PathPermission::Allow
        );
        let _ = std::fs::remove_dir_all(dir);
    }

    #[test]
    fn file_log_directory_honors_override_and_complex_filters() {
        let mut args = Args::try_parse_from(["rs-host", "--log-dir", "custom-logs"]).unwrap();
        assert_eq!(file_log_dir(&args), Some(PathBuf::from("custom-logs")));
        args.log = "info,hyper=warn".into();
        assert_eq!(file_log_dir(&args), None);
        args.log_dir = None;
        assert_eq!(file_log_dir(&args), None);

        args.log = "info".into();
        args.state_dir = Some(PathBuf::from("state"));
        #[cfg(target_os = "macos")]
        let expected = std::env::var_os("HOME")
            .map(|home| PathBuf::from(home).join("Library/Logs/Remote Host"))
            .unwrap_or_else(|| PathBuf::from("state/logs"));
        #[cfg(not(target_os = "macos"))]
        let expected = PathBuf::from("state/logs");
        assert_eq!(file_log_dir(&args), Some(expected));
    }

    #[tokio::test]
    async fn local_api_is_not_ready_until_injected() {
        let (dir, backend) = test_backend("not-ready");
        assert_eq!(backend.api().err().unwrap().code, "not_ready");
        assert_eq!(backend.settings_get().await.unwrap_err().code, "not_ready");
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn local_api_settings_share_updates_and_persist() {
        let (dir, backend) = test_backend("settings");
        let config_path = dir.join("config.toml");
        let config_store = host_core::config::ConfigStore::new(
            &config_path,
            host_core::config::HostConfig::default(),
        );
        let receiver = config_store.subscribe();
        let clone = backend.clone();
        backend.set_api(LocalApi::new(config_store.clone()));
        assert_eq!(
            clone.settings_get().await.unwrap(),
            serde_json::to_value(config_store.get()).unwrap()
        );
        let updated = clone
            .settings_set(serde_json::json!({"access": {"allow_remote_control": false}}))
            .await
            .unwrap();
        assert_eq!(updated["access"]["allow_remote_control"], false);
        assert_eq!(backend.settings_get().await.unwrap(), updated);
        assert!(receiver.has_changed().unwrap());
        assert!(!receiver.borrow().access.allow_remote_control);
        assert!(!config_store.get().access.allow_remote_control);
        assert!(
            !host_core::config::load(&config_path)
                .unwrap()
                .access
                .allow_remote_control
        );
        clone
            .update_set_channel(UpdateSetChannelParams {
                channel: UpdateChannel::Beta,
            })
            .await
            .unwrap();
        assert_eq!(
            backend.settings_get().await.unwrap()["update"]["channel"],
            "beta"
        );
        assert_eq!(
            host_core::config::load(&config_path)
                .unwrap()
                .update
                .channel,
            host_core::config::UpdateChannel::Beta
        );
        // A stderr-only LocalApi has no log files to export.
        assert_eq!(
            clone
                .logs_export(LogsExportParams { mask: false })
                .await
                .unwrap_err()
                .code,
            "unsupported"
        );
        assert!(clone
            .audit_list(AuditListParams::default())
            .await
            .unwrap()
            .entries
            .is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn update_methods_remain_unsupported_without_updater() {
        let (dir, backend) = test_backend("updates");
        for (method, error) in [
            ("update_status", backend.update_status().await.unwrap_err()),
            (
                "update_install_now",
                backend.update_install_now().await.unwrap_err(),
            ),
        ] {
            assert_eq!(error.code, "unsupported");
            assert_eq!(
                error.message,
                format!("{method} is not wired in this agent")
            );
        }
        std::fs::remove_dir_all(dir).unwrap();
    }

    struct FakeUserDirs {
        dirs: host_core::user_dirs::UserDirs,
        writable: bool,
    }

    impl UserDirsSource for FakeUserDirs {
        fn current(&self) -> host_core::user_dirs::UserDirs {
            self.dirs.clone()
        }

        fn user_can_write(&self, _: &std::path::Path) -> bool {
            self.writable
        }
    }

    fn pairing_file_backend(name: &str) -> (PathBuf, LocalBackend) {
        let (dir, backend) = test_backend(name);
        let files = host_core::pairfile_store::PairFileStore::open(&dir).unwrap();
        assert!(backend
            .pairing
            .set(
                PairingManager::new(
                    backend.store.clone(),
                    [1; 32],
                    "test host",
                    vec!["127.0.0.1:0".parse().unwrap()],
                )
                .with_files(Arc::new(files)),
            )
            .is_ok());
        (dir, backend)
    }

    #[tokio::test]
    async fn pairing_export_defaults_to_user_downloads() {
        let (dir, backend) = pairing_file_backend("downloads");
        let downloads = dir.join("d");
        std::fs::create_dir_all(&downloads).unwrap();
        let clone = backend.clone();
        backend.set_user_dirs(Arc::new(FakeUserDirs {
            dirs: host_core::user_dirs::UserDirs {
                downloads: downloads.clone(),
                home: Some(dir.clone()),
                owner: DirsOwner::User,
            },
            writable: true,
        }));
        let exported = clone
            .pairing_export_file(PairingExportFileParams::default())
            .await
            .unwrap();
        let path = PathBuf::from(exported.path);
        assert_eq!(path.parent(), Some(downloads.as_path()));
        assert!(path.is_file());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn pairing_export_rejects_unwritable_explicit_directory() {
        let (dir, backend) = pairing_file_backend("forbidden");
        let downloads = dir.join("d");
        std::fs::create_dir_all(&downloads).unwrap();
        backend.set_user_dirs(Arc::new(FakeUserDirs {
            dirs: host_core::user_dirs::UserDirs {
                downloads: downloads.clone(),
                home: Some(dir.clone()),
                owner: DirsOwner::User,
            },
            writable: false,
        }));
        let error = backend
            .pairing_export_file(PairingExportFileParams {
                dir: Some(downloads.to_string_lossy().into_owned()),
                ..Default::default()
            })
            .await
            .unwrap_err();
        assert_eq!(error.code, "forbidden");
        assert_eq!(error.message, "无权写入该目录");
        assert!(std::fs::read_dir(downloads).unwrap().next().is_none());
        assert!(backend.pairing_list_files().await.unwrap().files.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    // The Mac legacy fallback uses the real HOME; keep test exports in temporary storage.
    #[cfg(not(target_os = "macos"))]
    #[tokio::test]
    async fn pairing_export_keeps_legacy_fallback_without_user_directories() {
        for no_user in [false, true] {
            let (dir, backend) = pairing_file_backend("fallback");
            if no_user {
                backend.set_user_dirs(Arc::new(FakeUserDirs {
                    dirs: host_core::user_dirs::UserDirs {
                        downloads: dir.join("unused"),
                        home: None,
                        owner: DirsOwner::NoUser,
                    },
                    writable: false,
                }));
            }
            let exported = backend
                .pairing_export_file(PairingExportFileParams::default())
                .await
                .unwrap();
            let path = PathBuf::from(exported.path);
            assert_eq!(
                path.parent(),
                Some(host_core::pairing::default_export_dir(&dir).as_path())
            );
            assert!(path.is_file());
            std::fs::remove_dir_all(dir).unwrap();
        }
    }

    #[test]
    fn service_flags_require_service_and_leave_cli_alone() {
        assert!(Args::try_parse_from(["rs-host", "--foreground"]).is_err());
        let args = Args::try_parse_from(["rs-host", "--service"]).unwrap();
        assert!(args.service);
        assert!(!args.foreground);
        let args = Args::try_parse_from(["rs-host", "--service", "--foreground"]).unwrap();
        assert!(args.service && args.foreground);
        assert!(service_entry(&Args::try_parse_from(["rs-host"]).unwrap()).is_none());
    }

    #[cfg(not(windows))]
    #[test]
    fn service_entry_rejects_non_windows_before_starting_runtime() {
        let args = Args::try_parse_from(["rs-host", "--service"]).unwrap();
        assert!(service_entry(&args)
            .unwrap()
            .unwrap_err()
            .to_string()
            .contains("Windows"));
    }

    #[test]
    fn auth_policy_maps_every_config_field() {
        for enabled in [false, true] {
            let mut cfg = host_core::config::HostConfig::default();
            cfg.access.allowed_users = vec!["owner@example.com".into()];
            cfg.access.max_viewers = 3;
            cfg.access.allow_remote_control = enabled;
            cfg.access.attended_mode = !enabled;
            cfg.intranet.confirm_each = enabled;
            cfg.intranet.allow_pairing = !enabled;
            let policy = auth_policy_from_config(&cfg);
            assert_eq!(policy.allowed_users, cfg.access.allowed_users);
            assert_eq!(policy.max_viewers, 3);
            assert_eq!(policy.allow_remote_control, enabled);
            assert_eq!(policy.attended_mode, !enabled);
            assert_eq!(policy.intranet_confirm_each, enabled);
            assert_eq!(policy.allow_intranet_pairing, !enabled);
            assert_eq!(policy.forced_denial, None);
        }
    }

    #[tokio::test]
    async fn status_get_uses_platform_status_and_bind_signal() {
        let (dir, backend) = test_backend("status-platform");
        let platform = FakePlatform::with_nics(vec![host_core::platform::Nic {
            name: "utun0".into(),
            addresses: vec!["100.64.0.7".parse().unwrap()],
            networks: vec![],
            mac: None,
            tailnet: true,
        }]);
        let (bind_signal, _) = tokio::sync::watch::channel(false);
        backend.set_status(Arc::new(platform));
        backend.set_listening(bind_signal.subscribe());

        let before = backend.status_get().await.unwrap();
        assert_eq!(
            before.permissions,
            serde_json::json!({
                "screen_recording": true,
                "accessibility": true,
            })
        );
        assert_eq!(before.tailnet_addresses, vec!["100.64.0.7"]);
        assert!(!before.listening);

        bind_signal.send(true).unwrap();
        assert!(backend.status_get().await.unwrap().listening);
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn status_get_without_platform_source_uses_empty_permissions() {
        let (dir, backend) = test_backend("status-empty");
        let status = backend.status_get().await.unwrap();
        assert_eq!(status.permissions, serde_json::json!({}));
        assert!(status.tailnet_addresses.is_empty());
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[tokio::test]
    async fn platform_network_info_is_available_to_ipc() {
        let (dir, backend) = test_backend("network-platform");
        let config = host_core::config::ConfigStore::new(
            dir.join("config.toml"),
            host_core::config::HostConfig::default(),
        );
        let nic = host_core::platform::Nic {
            name: "en0".into(),
            addresses: vec!["10.30.12.8".parse().unwrap()],
            networks: vec!["10.30.0.0/16".into()],
            mac: None,
            tailnet: false,
        };
        backend.set_network(Arc::new(host_net::PlatformNetworkInfo::new(
            FakePlatform::with_nics(vec![nic]),
            config,
            7400,
        )));
        assert_eq!(
            backend.network_list_nics().await.unwrap().nics,
            vec![serde_json::json!({
                "name": "en0",
                "addresses": ["10.30.12.8"],
                "networks": ["10.30.0.0/16"],
                "mac": null,
                "tailnet": false,
            })]
        );
        assert_eq!(
            backend.network_firewall_status().await.unwrap(),
            NetworkFirewallStatusResult {
                enabled: false,
                active: false,
                detail: serde_json::json!({"platform": "not_applicable"}),
            }
        );
        assert!(backend
            .network_it_note()
            .await
            .unwrap()
            .text
            .contains("7400"));
        std::fs::remove_dir_all(dir).unwrap();
    }

    struct FakeNetwork;

    impl NetworkInfo for FakeNetwork {
        fn list_nics(&self) -> Vec<Value> {
            vec![serde_json::json!({"name": "test-nic"})]
        }
        fn firewall_status(&self) -> NetworkFirewallStatusResult {
            NetworkFirewallStatusResult {
                enabled: true,
                active: false,
                detail: serde_json::json!({"source": "test"}),
            }
        }
        fn it_note(&self) -> String {
            "test IT note".into()
        }
    }

    #[tokio::test]
    async fn network_methods_are_unsupported_until_injected_and_shared_by_clones() {
        let dir = std::env::temp_dir().join(format!("network{}", rsp_core::clock::unix_micros()));
        let backend = LocalBackend::new(
            Arc::new(host_core::store::Store::open(&dir).unwrap()),
            dir.clone(),
        );
        for (method, error) in [
            (
                "network_list_nics",
                backend.network_list_nics().await.unwrap_err(),
            ),
            (
                "network_firewall_status",
                backend.network_firewall_status().await.unwrap_err(),
            ),
            (
                "network_it_note",
                backend.network_it_note().await.unwrap_err(),
            ),
        ] {
            assert_eq!(error.code, "unsupported");
            assert_eq!(
                error.message,
                format!("{method} is not wired in this agent")
            );
        }
        let clone = backend.clone();
        backend.set_network(Arc::new(FakeNetwork));
        assert_eq!(
            clone.network_list_nics().await.unwrap().nics,
            FakeNetwork.list_nics()
        );
        let status = clone.network_firewall_status().await.unwrap();
        assert!(status.enabled);
        assert!(!status.active);
        assert_eq!(status.detail, FakeNetwork.firewall_status().detail);
        assert_eq!(
            clone.network_it_note().await.unwrap().text,
            FakeNetwork.it_note()
        );
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[test]
    fn accepts_all_development_flags_and_m0_bind_syntax() {
        let args = Args::try_parse_from([
            "rs-host",
            "--platform",
            "fake",
            "--bind",
            "127.0.0.1:0",
            "--bind",
            "127.0.0.1",
            "--port",
            "0",
            "--view-only",
            "--log",
            "debug",
            "--dump",
            "stream.h265",
            "--config",
            "config.toml",
            "--print-pair-code",
            "--fail",
            "RS-E101",
        ])
        .unwrap();
        assert_eq!(
            bind_addresses(&args).unwrap(),
            vec!["127.0.0.1:0".parse().unwrap(); 2]
        );
        assert!(args.view_only && args.print_pair_code);
        assert_eq!(args.fail.as_deref(), Some("RS-E101"));
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn linux_platform_flag() {
        let args = Args::try_parse_from(["rs-host", "--platform", "linux"]).unwrap();
        assert_eq!(args.platform, PlatformKind::Linux);
        assert!(bind_addresses(&args).unwrap().is_empty());

        let args = Args::try_parse_from(["rs-host"]).unwrap();
        assert_eq!(args.platform, PlatformKind::Fake);
    }

    #[cfg(target_os = "linux")]
    #[test]
    fn download_dir_parsing() {
        let home = PathBuf::from("/home/tester");
        assert_eq!(
            linux::parse_download_dir("XDG_DOWNLOAD_DIR=\"$HOME/下载\"", &home),
            Some(home.join("下载"))
        );
        assert_eq!(
            linux::parse_download_dir("XDG_DOWNLOAD_DIR=\"/mnt/downloads\"", &home),
            Some(PathBuf::from("/mnt/downloads"))
        );
        assert_eq!(
            linux::parse_download_dir("XDG_DOWNLOAD_DIR=\"$HOME/\"", &home),
            None
        );
        assert_eq!(
            linux::parse_download_dir("XDG_DOWNLOAD_DIR=\"$HOME\"", &home),
            None
        );
        assert_eq!(
            linux::parse_download_dir(
                "# XDG_DOWNLOAD_DIR=\"/ignored\"\nXDG_MUSIC_DIR=\"$HOME/Music\"",
                &home
            ),
            None
        );
        assert_eq!(
            linux::parse_download_dir("XDG_DOWNLOAD_DIR=downloads", &home),
            None
        );
        assert_eq!(
            linux::parse_download_dir(
                "# XDG_DOWNLOAD_DIR=\"/ignored\"\nXDG_MUSIC_DIR=\"$HOME/Music\"\n  XDG_DOWNLOAD_DIR = \"$HOME/Download files\" # comment",
                &home
            ),
            Some(home.join("Download files"))
        );
        assert_eq!(
            linux::parse_download_dir("XDG_DOWNLOAD_DIR=\"$HOME//Downloads\"", &home),
            Some(home.join("Downloads"))
        );
        assert_eq!(
            linux::parse_download_dir(r#"XDG_DOWNLOAD_DIR="$HOME/Down\"loads" # "ignored""#, &home),
            Some(home.join("Down\"loads"))
        );
        assert_eq!(
            linux::parse_download_dir("XDG_DOWNLOAD_DIR=\"$XDG_DATA_HOME/downloads\"", &home),
            None
        );
    }

    #[test]
    fn fail_flag_accepts_numeric_codes_and_rejects_unknown_codes() {
        let args =
            Args::try_parse_from(["rs-host", "--platform", "fake", "--fail", "180"]).unwrap();
        assert_eq!(
            forced_denial(&args).unwrap(),
            Some(rsp_core::errors::ErrorCode::DeviceRevoked)
        );

        let args =
            Args::try_parse_from(["rs-host", "--platform", "fake", "--fail", "bogus"]).unwrap();
        let error = forced_denial(&args).unwrap_err().to_string();
        assert!(error.starts_with("--fail:"), "{error}");
        assert!(error.contains("bogus"), "{error}");
    }

    #[test]
    fn tailnet_range() {
        assert!(is_tailnet(Ipv4Addr::new(100, 64, 0, 6)));
        assert!(is_tailnet(Ipv4Addr::new(100, 127, 255, 255)));
        assert!(!is_tailnet(Ipv4Addr::new(100, 128, 0, 1)));
        assert!(!is_tailnet(Ipv4Addr::new(100, 63, 0, 1)));
        assert!(!is_tailnet(Ipv4Addr::new(192, 168, 31, 2)));
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ipc_pairing_start_and_both_approval_routes() {
        use host_core::pairing::{PairApprovalRequest, PairApprover};
        use host_core::session::handshake::{ConnectionApprovalRequest, ConnectionApprover};
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};
        let dir = std::env::temp_dir().join(format!("ipc{}", rsp_core::clock::unix_micros()));
        let store = Arc::new(host_core::store::Store::open(&dir).unwrap());
        let backend = LocalBackend::new(store.clone(), dir.clone());
        let mgr = SessionManager::new(Arc::new(FakePlatform::new()), HostServices::default());
        mgr.start();
        assert!(backend.control.set(mgr.session_control()).is_ok());
        assert!(backend.sessions_list().await.unwrap().sessions.is_empty());
        assert_eq!(
            backend
                .sessions_disconnect(SessionsDisconnectParams {
                    session_id: "unknown".into()
                })
                .await
                .unwrap_err()
                .code,
            "unknown_session"
        );
        let ipc = IpcServer::bind(dir.join("s"), backend.clone()).unwrap();
        let connection = Arc::new(IpcConnectionApprover::new(ipc.notifier()));
        let pairing = Arc::new(IpcPairApprover::new(ipc.notifier()));
        assert!(backend.connection.set(connection.clone()).is_ok());
        assert!(backend.pair_approval.set(pairing.clone()).is_ok());
        assert!(backend
            .pairing
            .set(PairingManager::new(store, [1; 32], "test host", vec![]))
            .is_ok());
        let server = tokio::spawn(async move { ipc.serve().await });
        let stream = tokio::net::UnixStream::connect(dir.join("s"))
            .await
            .unwrap();
        let (reader, mut writer) = stream.into_split();
        let mut lines = BufReader::new(reader).lines();
        writer
            .write_all(b"{\"jsonrpc\":\"2.0\",\"id\":1,\"method\":\"pairing.start\"}\n")
            .await
            .unwrap();
        let response: Value = lines.next_line().await.unwrap().unwrap().parse().unwrap();
        assert_eq!(response["result"]["code"].as_str().unwrap().len(), 6);
        assert!(response["result"]["qr_url"]
            .as_str()
            .unwrap()
            .starts_with("rsp://pair?"));
        let pending_connection = tokio::spawn(async move {
            connection
                .request(ConnectionApprovalRequest {
                    request_id: "test-connection".into(),
                    device_id: "test-device".into(),
                    device_name: "test phone".into(),
                    device_fp: [2; 32],
                    role: host_core::auth::Role::Controller,
                    reasons: vec![host_core::auth::ApprovalReason::AttendedMode],
                    peer: rsp_core::pb::PeerInfo::default(),
                    platform: rsp_core::pb::Platform::Ios,
                    path: host_core::auth::PathClass::Lan,
                    intent: None,
                })
                .await
        });
        let event: Value = lines.next_line().await.unwrap().unwrap().parse().unwrap();
        assert_eq!(event["method"], "event.approvalRequested");
        assert_eq!(event["params"]["kind"], "connection");
        assert_eq!(event["params"]["timeout_s"], 30);
        assert!(event["params"].get("intent").is_none());
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":2,\"method\":\"approval.respond\",\"params\":{\"request_id\":\"test-connection\",\"allow\":true}}\n").await.unwrap();
        let response: Value = lines.next_line().await.unwrap().unwrap().parse().unwrap();
        assert!(response.get("result").is_some());
        assert!(pending_connection.await.unwrap());
        let pending_pair = tokio::spawn(async move {
            pairing
                .request(PairApprovalRequest {
                    request_id: "test-pair".into(),
                    device_name: "test phone".into(),
                    platform: rsp_core::pb::Platform::Ios,
                    whois_login: Some("alice@example.com".into()),
                    device_fp: [2; 32],
                })
                .await
        });
        let event: Value = lines.next_line().await.unwrap().unwrap().parse().unwrap();
        assert_eq!(event["params"]["kind"], "pairing");
        assert_eq!(event["params"]["whois_login"], "alice@example.com");
        writer.write_all(b"{\"jsonrpc\":\"2.0\",\"id\":3,\"method\":\"approval.respond\",\"params\":{\"request_id\":\"test-pair\",\"allow\":true,\"permission\":\"view_only\"}}\n").await.unwrap();
        let response: Value = lines.next_line().await.unwrap().unwrap().parse().unwrap();
        assert!(response.get("result").is_some());
        let result = pending_pair.await.unwrap();
        assert!(result.allow);
        assert_eq!(result.permission, rsp_core::trust::Permission::ViewOnly);
        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(dir).unwrap();
    }

    #[cfg(unix)]
    #[tokio::test]
    async fn ipc_pairing_file_lifecycle() {
        use std::os::unix::fs::PermissionsExt;
        use tokio::io::{AsyncBufReadExt, AsyncWriteExt, BufReader};

        async fn request(stream: &mut tokio::net::UnixStream, value: &str) -> Value {
            stream.write_all(value.as_bytes()).await.unwrap();
            stream.write_all(b"\n").await.unwrap();
            let mut reader = BufReader::new(stream);
            let mut line = String::new();
            reader.read_line(&mut line).await.unwrap();
            line.trim().parse().unwrap()
        }

        let dir = std::env::temp_dir().join(format!("ipcfiles{}", rsp_core::clock::unix_micros()));
        let exports = dir.join("exports");
        std::fs::create_dir_all(&exports).unwrap();
        let store = Arc::new(host_core::store::Store::open(&dir).unwrap());
        let backend = LocalBackend::new(store.clone(), dir.clone());
        let files = host_core::pairfile_store::PairFileStore::open(&dir).unwrap();
        assert!(backend
            .pairing
            .set(
                PairingManager::new(
                    store,
                    [1; 32],
                    "test host",
                    vec!["127.0.0.1:33100".parse().unwrap()],
                )
                .with_files(Arc::new(files)),
            )
            .is_ok());
        let ipc = IpcServer::bind(dir.join("s"), backend).unwrap();
        let server = tokio::spawn(async move { ipc.serve().await });
        let mut stream = tokio::net::UnixStream::connect(dir.join("s"))
            .await
            .unwrap();

        let response = request(
            &mut stream,
            &format!(
                r#"{{"jsonrpc":"2.0","id":1,"method":"pairing.exportFile","params":{{"permission":"control","label":"IT","dir":"{}"}}}}"#,
                exports.display()
            ),
        )
        .await;
        let result = &response["result"];
        let file_id = result["file_id"].as_str().unwrap().to_owned();
        assert_eq!(file_id.len(), 22);
        let path = PathBuf::from(result["path"].as_str().unwrap());
        assert_eq!(path.parent(), Some(exports.as_path()));
        let metadata = std::fs::metadata(&path).unwrap();
        assert_eq!(metadata.permissions().mode() & 0o777, 0o600);
        let text = std::fs::read_to_string(&path).unwrap();
        let pairfile = rsp_core::pairfile::parse(&text).expect("exported pair file parses");
        assert_eq!(pairfile.name, "test host");
        assert_eq!(
            pairfile.expires_at_s,
            result["expires_at"].as_u64().unwrap() / 1_000_000
        );

        let response = request(
            &mut stream,
            r#"{"jsonrpc":"2.0","id":2,"method":"pairing.listFiles","params":null}"#,
        )
        .await;
        let listed = response["result"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["file_id"] == file_id)
            .expect("exported file is listed");
        assert_eq!(listed["state"], "active");
        assert_eq!(listed["permission"], "control");

        let response = request(
            &mut stream,
            &format!(
                r#"{{"jsonrpc":"2.0","id":3,"method":"pairing.revokeFile","params":{{"file_id":"{file_id}"}}}}"#
            ),
        )
        .await;
        assert_eq!(response["result"], Value::Object(Default::default()));
        let response = request(
            &mut stream,
            r#"{"jsonrpc":"2.0","id":4,"method":"pairing.listFiles","params":null}"#,
        )
        .await;
        let listed = response["result"]["files"]
            .as_array()
            .unwrap()
            .iter()
            .find(|file| file["file_id"] == file_id)
            .expect("revoked file remains listed");
        assert_eq!(listed["state"], "revoked");

        let response = request(
            &mut stream,
            r#"{"jsonrpc":"2.0","id":5,"method":"pairing.revokeFile","params":{"file_id":"AAECAwQFBgcICQoLDA0ODw"}}"#,
        )
        .await;
        assert_eq!(response["error"]["data"]["code"], "not_found");

        let response = request(
            &mut stream,
            r#"{"jsonrpc":"2.0","id":6,"method":"pairing.exportFile","params":{"dir":"relative"}}"#,
        )
        .await;
        assert_eq!(response["error"]["data"]["code"], "invalid_params");

        server.abort();
        let _ = server.await;
        std::fs::remove_dir_all(dir).unwrap();
    }
}
