//! Windows service assembly. All Win32 operations use host-win's safe adapters.

use std::path::{Path, PathBuf};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use anyhow::{bail, Result};
use host_core::config::{ConfigStore, HostConfig};
use host_core::ipc_listener::StreamServer;
use host_core::listen::{ListenError, ListenPolicy, ListenerUpdater};
use host_core::local_api::{ExportDir, LocalApi};
use host_core::logs::LogGuard;
use host_core::pairing::ExportFileRequest;
use host_core::platform::Nic;
use host_core::update::{
    agent::{self, UpdatePlatform, UpdateWiring, DEFAULT_CHECK_INTERVAL, DEFAULT_INITIAL_DELAY},
    manifest::TrustedKeys,
    remote::CurlDownloader,
};
use host_core::user_dirs::{DirsOwner, UserDirsSource};
use host_win::addr_watch::{watch_addresses, AddrWatch};
use host_win::addr_watch_model::{follow_nics, DEBOUNCE, MAX_DELAY, RESYNC};
use host_win::dpapi::{secure_data_dir, AclSecurer};
use host_win::firewall::{SyncReport, WinFirewall};
use host_win::keyfile::DirSecurer;
use host_win::nics::nics;
use host_win::paths::WinPaths;
use host_win::pipe::UiPipeListener;
use host_win::proxy::PipePlatform;
use host_win::scm::{self, ScmError};
use host_win::supervisor::SupervisorConfig;
use host_win::svc_local::WinServiceLocal;
use host_win::svc_net_model::{
    follow_nic_names, interfaces_patch, listen_policy, load_book, nic_json, save_book, BIND_RETRY,
    NIC_BOOK,
};
use host_win::svc_os_model::{
    deploy_fingerprint, load_marker, needs_export, parse_deploy, save_marker, DeployMarker,
    NoopStatus, ScmEvent, StatusSink, DEPLOY_MARKER, SERVICE_NAME, STOP_WAIT_HINT, TRAY_EXE,
};
use host_win::svc_session::start_supervisor;
use host_win::svc_session_model::{sibling_exe, SESSION_EXE};
use host_win::user_dirs::WinUserDirs;
use tokio::sync::{mpsc, watch, Notify};
use tokio::task::{JoinHandle, JoinSet};
use tracing::{debug, error, info, warn};

use super::{
    bind_guard, owner, Args, ChainedWhoIs, CliSource, HostRuntime, HostServices, LocalApiSource,
    LocalBackend, NetworkFirewallStatusResult, NetworkInfo, OwnerOptions, PairingManager,
    RuntimeOptions, SessionManager, Value,
};

pub(crate) fn run(args: &Args) -> Result<()> {
    if args.foreground {
        let (tx, rx) = mpsc::unbounded_channel();
        std::thread::Builder::new()
            .name("rs-svc-console".into())
            .spawn(move || {
                let result = tokio::runtime::Builder::new_current_thread()
                    .enable_all()
                    .build()
                    .and_then(|runtime| runtime.block_on(tokio::signal::ctrl_c()));
                if let Err(error) = result {
                    eprintln!("cannot wait for service Ctrl-C: {error}");
                }
                let _ = tx.send(ScmEvent::Stop);
            })?;
        let code = serve(args, Arc::new(NoopStatus), rx);
        if code != 0 {
            bail!("service exited with code {code}");
        }
        return Ok(());
    }
    let args = args.clone();
    match scm::run_service(
        SERVICE_NAME,
        Box::new(move |status, events| serve(&args, status, events)),
    ) {
        Ok(()) => Ok(()),
        Err(ScmError::NotUnderScm) => {
            bail!("由 SCM 启动，或用 --service --foreground 在控制台调试")
        }
        Err(error @ ScmError::Api(_)) => Err(error.into()),
    }
}

fn serve(
    args: &Args,
    status: Arc<dyn StatusSink>,
    events: mpsc::UnboundedReceiver<ScmEvent>,
) -> u32 {
    let paths = WinPaths::from_program_data(std::env::var("ProgramData").ok().as_deref());
    if let Err(error) = secure_data_dir(&paths) {
        eprintln!("cannot secure service data directory: {error}");
        return 1;
    }
    let (cfg, config_error) = match host_core::config::load(&paths.config_file) {
        Ok(cfg) => (cfg, None),
        Err(error) => (HostConfig::default(), Some(error)),
    };
    let level = cfg.log.level.as_ref();
    let log_guard = match host_core::logs::init(&paths.logs_dir, level) {
        Ok(guard) => Some(Arc::new(guard)),
        Err(error) => {
            eprintln!("cannot initialize service file logging: {error}");
            let _ = tracing_subscriber::fmt()
                .with_env_filter(level)
                .with_writer(std::io::stderr)
                .try_init();
            None
        }
    };
    if let Some(error) = config_error {
        warn!(%error, "cannot load service config; using defaults");
    }
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        error!(location = ?panic.location(), message = %panic, "service panic");
        original_hook(panic);
    }));
    let exe = match std::env::current_exe() {
        Ok(exe) => exe.to_string_lossy().into_owned(),
        Err(error) => {
            error!(%error, "cannot locate service executable");
            return 1;
        }
    };
    info!(version = env!("CARGO_PKG_VERSION"), pid = std::process::id(), current_exe = %exe,
        data_dir = %paths.data_dir.display(), foreground = args.foreground, "service starting");
    let runtime = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(4)
        .thread_name("rs-svc")
        .enable_all()
        .build()
    {
        Ok(runtime) => runtime,
        Err(error) => {
            error!(%error, "cannot build service runtime");
            return 1;
        }
    };
    let code = runtime.block_on(serve_async(
        args,
        status,
        events,
        paths,
        cfg,
        exe,
        log_guard.clone(),
    ));
    runtime.shutdown_timeout(Duration::from_secs(2));
    code
}

async fn serve_async(
    args: &Args,
    status: Arc<dyn StatusSink>,
    mut events: mpsc::UnboundedReceiver<ScmEvent>,
    paths: WinPaths,
    cfg: HostConfig,
    exe: String,
    log_guard: Option<Arc<LogGuard>>,
) -> u32 {
    let store = match host_core::store::Store::open(&paths.data_dir) {
        Ok(store) => Arc::new(store),
        Err(error) => {
            error!(%error, "cannot open service store");
            return 1;
        }
    };
    let audit = match host_core::audit::AuditQueue::open(&paths.data_dir) {
        Ok(audit) => audit,
        Err(error) => {
            error!(%error, "cannot open service audit queue");
            return 1;
        }
    };
    let config = ConfigStore::new(paths.config_file.clone(), cfg);
    let platform = PipePlatform::new(WinServiceLocal::new(&paths));
    let session_exe = PathBuf::from(sibling_exe(&exe, SESSION_EXE));
    info!(path = %session_exe.display(), "session executable");
    let supervisor = match start_supervisor(
        Arc::new(platform.clone()),
        session_exe,
        SupervisorConfig::default(),
        config.clone(),
    ) {
        Ok(supervisor) => supervisor,
        Err(error) => {
            error!(%error, "cannot start session supervisor");
            return 1;
        }
    };
    let firewall = Arc::new(WinFirewall::for_current_exe(args.port));
    let backend = LocalBackend::new(store.clone(), paths.data_dir.clone());
    backend.set_network(Arc::new(WinNetworkInfo {
        config: config.clone(),
        firewall: firewall.clone(),
        port: args.port,
    }));
    let win_dirs = Arc::new(WinUserDirs::new(&paths));
    backend.set_user_dirs(win_dirs.clone());
    let export_dir: ExportDir = {
        let dirs = win_dirs.clone();
        Arc::new(move || {
            let current = dirs.current();
            match current.owner {
                DirsOwner::User => Some(current.downloads),
                DirsOwner::NoUser => None,
            }
        })
    };
    // Only administrators editing config.toml may redirect SYSTEM writes.
    const LOCKED: &[&str] = &["files.dest_dir", "files.browse_roots"];
    let api = LocalApi::new(config.clone())
        .with_audit(audit.clone())
        .with_logs(paths.logs_dir.clone(), export_dir)
        .with_forced_mask()
        .with_locked_settings(LOCKED);
    backend.set_api(api);
    let ui = Arc::new(StreamServer::new(backend.clone()));
    let notifier = ui.notifier();
    let mut tasks = JoinSet::new();
    if let Some(log_guard) = log_guard {
        let mut rx = config.subscribe();
        let mut level = rx.borrow().log.level.clone();
        tasks.spawn(async move {
            while rx.changed().await.is_ok() {
                let next = rx.borrow_and_update().log.level.clone();
                if next != level {
                    match log_guard.set_level(next.as_ref()) {
                        Ok(()) => level = next,
                        Err(error) => warn!(%error, "cannot reload service log level"),
                    }
                }
            }
        });
    }
    tasks.spawn(async move {
        loop {
            match UiPipeListener::new() {
                Ok(listener) => {
                    info!("UI pipe ready");
                    if let Err(error) = ui.serve(listener).await {
                        warn!(%error, "UI pipe stopped; retrying");
                    }
                }
                Err(error) => warn!(%error, "cannot open UI pipe; retrying"),
            }
            tokio::time::sleep(Duration::from_secs(1)).await;
        }
    });
    let mgr = SessionManager::new(
        Arc::new(platform.clone()),
        HostServices {
            events: Arc::new(notifier),
            config: config.clone(),
            audit: Some(audit.clone()),
            downloads_dir: WinUserDirs::fallback_path(&paths),
            state_dir: Some(paths.data_dir.clone()),
            clock: Arc::new(host_core::clock::SystemClock),
        },
    );
    backend.set_devices(Arc::new(super::devices_ipc::DevicesApiImpl::new(
        mgr.clone(),
        store.clone(),
    )));
    mgr.features().files.set_user_dirs(win_dirs);
    let _ = backend.control.set(mgr.session_control());
    let (listening, _) = watch::channel(false);
    status.running();
    let (stop_tx, stop_rx) = watch::channel(false);
    let (fatal_tx, mut fatal_rx) = mpsc::unbounded_channel();
    let mut address_watch = None;
    let network = ServiceNetwork {
        config,
        data_dir: paths.data_dir.clone(),
        firewall,
        port: args.port,
        updater: Arc::new(Mutex::new(None)),
        bind_notify: Arc::new(Notify::new()),
    };
    let update_platform = UpdatePlatform {
        installer: Arc::new(host_win::update::installer(&paths)),
        verifier: Arc::new(host_win::update::WinTrustVerifier::new()),
        downloader: Arc::new(CurlDownloader {
            program: std::env::var_os("SystemRoot")
                .filter(|root| !root.is_empty())
                .map(PathBuf::from)
                .unwrap_or_else(|| PathBuf::from(r"C:\Windows"))
                .join("System32")
                .join("curl.exe"),
            ..Default::default()
        }),
    };
    let update_service = agent::attach(
        &mgr,
        UpdateWiring {
            work_dir: paths.update_dir.clone(),
            current_version: env!("CARGO_PKG_VERSION").into(),
            keys: TrustedKeys::builtin(),
            platform: update_platform,
            source: None,
            listening: listening.subscribe(),
            audit: Some(audit.clone()),
            initial_delay: DEFAULT_INITIAL_DELAY,
            check_interval: DEFAULT_CHECK_INTERVAL,
        },
    );
    backend.set_updates(update_service);
    let code = {
        // Keep processing SCM controls during slow tray, COM and bind startup.
        // Dropping this future on Stop cancels startup before shutdown begins.
        let startup = async {
            let tray_exe = PathBuf::from(sibling_exe(&exe, TRAY_EXE));
            match tokio::task::spawn_blocking(move || host_win::tray::ensure_tray(&tray_exe)).await
            {
                Ok(Ok((decision, pid))) => info!(?decision, ?pid, "tray decision"),
                Ok(Err(error)) => warn!(%error, "cannot ensure tray"),
                Err(error) => warn!(%error, "tray worker failed"),
            }
            let nic_tx = network
                .start(&mut tasks, &mut address_watch, stop_rx.clone())
                .await;
            let binding = network.clone();
            let args = args.clone();
            let backend = backend.clone();
            tasks.spawn(async move {
                let mut stop = stop_rx;
                let mut attempts = 0u64;
                loop {
                    attempts = attempts.saturating_add(1);
                    let cfg = binding.config.get();
                    let policy = listen_policy(&cfg, args.port);
                    let options = RuntimeOptions {
                        store_dir: binding.data_dir.clone(),
                        auth_policy: host_core::auth_config::auth_policy_from_config(&cfg),
                        live_auth_policy: true,
                        listen_policy: policy,
                        ..RuntimeOptions::default()
                    };
                    let result = bind_guard::bind_then_signal(
                        || async {
                            HostRuntime::new(platform.clone(), options)
                                .with_manager(mgr.clone())
                                .with_store(store.clone())
                                .with_audit(audit.clone())
                                .bind()
                                .await
                        },
                        &listening,
                        None,
                    )
                    .await;
                    let host = match result {
                        Ok(host) => host,
                        Err(error) => {
                            if attempts == 1 || attempts.is_multiple_of(10) {
                                warn!(attempts, %error, "service bind failed; retrying");
                            }
                            tokio::select! {
                                _ = binding.bind_notify.notified() => {},
                                _ = tokio::time::sleep(BIND_RETRY) => {},
                                _ = stop.changed() => return,
                            }
                            continue;
                        }
                    };
                    *binding
                        .updater
                        .lock()
                        .unwrap_or_else(|error| error.into_inner()) = Some(host.listener.updater());
                    // A change may have been reconciled while bind was in flight.
                    // Wake the coordinator to apply the latest snapshot once more.
                    let _ = nic_tx.send(nics());
                    let _ = backend.pairing.set(host.security.pairing.clone());
                    let _owner = AbortTask(owner::spawn(
                        mgr.clone(),
                        OwnerOptions::new(Arc::new(ChainedWhoIs::new(
                            LocalApiSource::default(),
                            CliSource::default(),
                        ))),
                    ));
                    info!("owner login worker started");
                    let pairing = host.security.pairing.clone();
                    let data_dir = binding.data_dir.clone();
                    match tokio::task::spawn_blocking(move || export_deploy(&data_dir, &pairing))
                        .await
                    {
                        Ok(Ok(())) => {}
                        Ok(Err(error)) => warn!(%error, "Deploy export failed"),
                        Err(error) => warn!(%error, "Deploy export worker failed"),
                    }
                    let result = host.run().await;
                    error!(?result, "host run returned unexpectedly");
                    let _ = fatal_tx.send(2);
                    return;
                }
            });
        };
        tokio::pin!(startup);
        let mut started = false;
        loop {
            tokio::select! {
                _ = &mut startup, if !started => started = true,
                event = events.recv() => {
                    info!(?event, "service control event");
                    match event {
                        Some(ScmEvent::SessionChange { kind, session_id }) => {
                            supervisor.session_change(kind, session_id);
                            debug!(?kind, session_id, "forwarded session change");
                        },
                        Some(ScmEvent::Stop | ScmEvent::Shutdown) | None => break 0,
                    }
                },
                code = fatal_rx.recv() => {
                    error!(code = code.unwrap_or(2), "host worker stopped; shutting down service");
                    break code.unwrap_or(2);
                },
            }
        }
    };
    status.stopping(STOP_WAIT_HINT);
    stop_tx.send_replace(true);
    tasks.abort_all();
    drop(address_watch);
    if tokio::time::timeout(Duration::from_secs(1), async {
        while tasks.join_next().await.is_some() {}
    })
    .await
    .is_err()
    {
        warn!("service tasks did not stop within one second");
    }
    if let Some(svc) = backend.updates.get() {
        svc.shutdown();
    }
    match tokio::time::timeout(
        Duration::from_secs(6),
        tokio::task::spawn_blocking(move || supervisor.shutdown()),
    )
    .await
    {
        Ok(Ok(stats)) => info!(?stats, "session supervisor stopped; DriverStats"),
        Ok(Err(error)) => warn!(%error, "supervisor shutdown worker failed"),
        Err(_) => warn!("session supervisor shutdown timed out"),
    }
    info!(code, "service stopped");
    code
}

// JoinHandle alone detaches on Drop. Keep owner-login cancellation tied to the
// binding task, including cancellation during Deploy export and unexpected run return.
struct AbortTask(JoinHandle<()>);
impl Drop for AbortTask {
    fn drop(&mut self) {
        self.0.abort();
    }
}

struct WinNetworkInfo {
    config: ConfigStore,
    firewall: Arc<WinFirewall>,
    port: u16,
}
impl NetworkInfo for WinNetworkInfo {
    fn list_nics(&self) -> Vec<Value> {
        nics().iter().map(nic_json).collect()
    }
    fn firewall_status(&self) -> NetworkFirewallStatusResult {
        self.firewall
            .status(&listen_policy(&self.config.get(), self.port), &nics())
    }
    fn it_note(&self) -> String {
        self.firewall
            .it_note(&listen_policy(&self.config.get(), self.port), &nics())
    }
}

#[derive(Clone)]
struct ServiceNetwork {
    config: ConfigStore,
    data_dir: PathBuf,
    firewall: Arc<WinFirewall>,
    port: u16,
    updater: Arc<Mutex<Option<ListenerUpdater>>>,
    bind_notify: Arc<Notify>,
}

impl ServiceNetwork {
    // Called only from blocking workers, serially with firewall synchronization.
    fn reconcile(&self, nics: Vec<Nic>) -> (ListenPolicy, Vec<Nic>) {
        let book_path = self.data_dir.join(NIC_BOOK);
        let book = load_book(&book_path);
        let configured = self.config.get().intranet.interfaces;
        let followed = follow_nic_names(&configured, &nics, &book);
        if !followed.renames.is_empty() {
            match self.config.update(&interfaces_patch(&followed.interfaces)) {
                Ok(_) => {
                    for (old, new) in &followed.renames {
                        info!(old, new, "intranet interface renamed");
                    }
                }
                Err(error) => {
                    warn!(%error, "cannot persist intranet interface rename");
                    // Retain the old aliases so the next pass can retry saving.
                    return (listen_policy(&self.config.get(), self.port), nics);
                }
            }
        }
        for name in configured
            .iter()
            .filter(|name| !nics.iter().any(|nic| &nic.name == *name))
        {
            if !followed.renames.iter().any(|(old, _)| old == name) {
                warn!(
                    interface = name,
                    "missing intranet interface; no unique MAC match for rename"
                );
            }
        }
        if followed.book != book {
            if let Err(error) = save_book(&book_path, &followed.book) {
                warn!(%error, "cannot save NIC aliases");
            }
        }
        (listen_policy(&self.config.get(), self.port), nics)
    }

    async fn start(
        &self,
        tasks: &mut JoinSet<()>,
        address_watch: &mut Option<AddrWatch>,
        mut stop: watch::Receiver<bool>,
    ) -> mpsc::UnboundedSender<Vec<Nic>> {
        let mut config_changes = self.config.subscribe();
        let network = self.clone();
        if let Err(error) = tokio::task::spawn_blocking(move || {
            firewall_report("tailnet", network.firewall.ensure_tailnet());
            let (policy, nics) = network.reconcile(nics());
            firewall_report("intranet", network.firewall.sync_intranet(&policy, &nics));
        })
        .await
        {
            warn!(%error, "initial firewall worker failed");
        }
        let (atx, arx) = mpsc::channel(16);
        *address_watch = match watch_addresses(atx.clone()) {
            Ok(watch) => Some(watch),
            Err(error) => {
                warn!(%error, "address notifications unavailable; using periodic rescan");
                None
            }
        };
        let (nic_tx, mut nic_rx) = mpsc::unbounded_channel();
        // Cover changes between the initial reconciliation and watch baseline.
        let _ = nic_tx.send(nics());
        let watcher_tx = nic_tx.clone();
        tasks.spawn(async move {
            // Even without OS notifications, keep the channel open for RESYNC.
            let _keep_sender = atx;
            follow_nics(
                arx,
                nics,
                move |n| {
                    let _ = watcher_tx.send(n);
                },
                DEBOUNCE,
                MAX_DELAY,
                RESYNC,
            )
            .await;
        });
        let network = self.clone();
        tasks.spawn(async move {
            loop {
                let snapshot = tokio::select! {
                    snapshot = nic_rx.recv() => match snapshot {
                        Some(snapshot) => snapshot,
                        None => return,
                    },
                    changed = config_changes.changed() => {
                        if changed.is_err() { return; }
                        nics()
                    },
                    _ = stop.changed() => return,
                };
                let reconcile = network.clone();
                let (policy, nics) = match tokio::task::spawn_blocking(move || {
                    reconcile.reconcile(snapshot)
                })
                .await
                {
                    Ok(result) => result,
                    Err(error) => {
                        warn!(%error, "network reconciliation worker failed");
                        continue;
                    }
                };
                let updater = network
                    .updater
                    .lock()
                    .unwrap_or_else(|error| error.into_inner())
                    .clone();
                if let Some(updater) = updater {
                    match updater.update(policy.clone(), nics.clone()).await {
                        Ok(()) => {}
                        Err(ListenError::Closed) => debug!("listener closed during network update"),
                    }
                }
                let firewall = network.firewall.clone();
                match tokio::task::spawn_blocking(move || firewall.sync_intranet(&policy, &nics))
                    .await
                {
                    Ok(report) => firewall_report("intranet", report),
                    Err(error) => warn!(%error, "firewall synchronization worker failed"),
                }
                if !network.is_bound() {
                    network.bind_notify.notify_one();
                }
            }
        });
        nic_tx
    }

    fn is_bound(&self) -> bool {
        self.updater
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .is_some()
    }
}

fn firewall_report(scope: &str, report: SyncReport) {
    info!(
        scope,
        added = report.added.len(),
        removed = report.removed.len(),
        policy_locked = report.policy_locked,
        "firewall synchronization"
    );
    if let Some(error) = report.error {
        warn!(scope, %error, "firewall synchronization incomplete");
    }
}

fn export_deploy(data_dir: &Path, pairing: &PairingManager) -> Result<()> {
    let values = host_win::deploy::read_deploy()?;
    let marker_path = data_dir.join(DEPLOY_MARKER);
    let marker = load_marker(&marker_path);
    if !needs_export(values.as_ref(), marker.as_ref()) {
        info!("Deploy export not requested or already completed");
        return Ok(());
    }
    let Some(values) = values else {
        return Ok(());
    };
    let request = parse_deploy(&values)?;
    std::fs::create_dir_all(&request.dir)?;
    let now = rsp_core::clock::unix_micros();
    let exported = pairing.export_file(
        now,
        ExportFileRequest {
            permission: request.permission,
            label: request.label,
            dir: request.dir.clone(),
        },
    )?;
    let mut path = exported.path;
    let secured = (|| -> Result<()> {
        // Secure the generated file too: a failed rename must not leave a
        // readable one-time secret at its original destination.
        AclSecurer.secure_file(&path)?;
        if let Some(name) = request.file_name {
            let destination = request.dir.join(name);
            if destination != path {
                std::fs::rename(&path, &destination)?;
                path = destination;
            }
        }
        AclSecurer.secure_file(&path)?;
        Ok(())
    })();
    if let Err(error) = secured {
        let _ = pairing.revoke_file(&exported.file_id);
        let _ = std::fs::remove_file(&path);
        return Err(error);
    }
    save_marker(
        &marker_path,
        &DeployMarker {
            fingerprint: deploy_fingerprint(&values),
            file_id: exported.file_id.clone(),
            path: path.to_string_lossy().into_owned(),
            exported_at_us: now,
        },
    )?;
    info!(path = %path.display(), file_id = %exported.file_id,
        expires_at_us = exported.expires_at_us, "Deploy pairing file exported");
    Ok(())
}
