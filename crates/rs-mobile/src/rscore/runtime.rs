//! RsCore ownership and uniffi control-plane exports.
//!
//! Export names use `<domain>_<snake_case_method>`. Native IDs are `u64` at
//! the uniffi boundary. Objects cross as JSON serialized from `dto_core`,
//! never hand-built JSON. Async exports use
//! `#[uniffi::export(async_runtime = "tokio")]` and delegate with
//! `let c = core()?; c.clone().run(async move { /* business call */ }).await`.
//! Business functions are ordinary public functions accepting `&Arc<Core>`;
//! export wrappers only obtain the core and encode/decode boundary values.
//! Initialization bootstraps the first runtime and therefore is the exception
//! to obtaining an existing core before dispatch.

use std::any::{Any, TypeId};
use std::collections::HashMap;
use std::future::Future;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::path::PathBuf;
use std::sync::{Arc, Mutex, MutexGuard};
use std::time::{Duration, SystemTime, UNIX_EPOCH};

use rsp_core::ids::id_from_fp;
use rsp_transport::identity::{spki_fingerprint, ExternalSigner, KeySource, LocalIdentity};
use rsp_transport::{system_net, NetDialer};
use tokio::runtime::{Handle, Runtime};

use super::dto_core::CoreConfig;
use super::error::CoreError;
use super::events::EventHub;
use super::logging;
use super::signer::{global_signer, spki_matches};
use super::store::Store;
use crate::uniffi_api::EventSink;

pub struct Timeouts {
    pub hostkey_prompt: Duration,
    pub pairing: Duration,
    pub probe: Duration,
    pub resume_token_ttl: Duration,
}

impl Default for Timeouts {
    fn default() -> Self {
        Self {
            hostkey_prompt: Duration::from_secs(60),
            pairing: Duration::from_secs(120),
            probe: Duration::from_secs(3),
            resume_token_ttl: Duration::from_secs(60),
        }
    }
}

pub struct CoreOptions {
    /// Initial outbound network; `None` uses system networking.
    pub net: Option<Arc<dyn NetDialer>>,
    pub clock: Option<Arc<dyn Fn() -> u64 + Send + Sync>>,
    pub timeouts: Timeouts,
    pub worker_threads: usize,
    pub init_logging: bool,
}

impl Default for CoreOptions {
    fn default() -> Self {
        Self {
            net: None,
            clock: None,
            timeouts: Timeouts::default(),
            worker_threads: 2,
            init_logging: true,
        }
    }
}

#[derive(Default)]
struct IdentityCache {
    identity: Option<Arc<LocalIdentity>>,
}

type ShutdownHook = Box<dyn FnOnce() + Send>;

pub struct Core {
    net: Mutex<Arc<dyn NetDialer>>,
    cfg: Mutex<CoreConfig>,
    data_dir: PathBuf,
    store: Mutex<Store>,
    runtime: Option<Runtime>,
    events: EventHub,
    states: Mutex<HashMap<TypeId, Arc<dyn Any + Send + Sync>>>,
    signer: Mutex<Option<Arc<dyn ExternalSigner>>>,
    cache: Mutex<IdentityCache>,
    shutdown_hooks: Mutex<Option<Vec<ShutdownHook>>>,
    export_lock: Mutex<()>,
    opts: CoreOptions,
}

impl Core {
    pub fn new(cfg: CoreConfig, opts: CoreOptions) -> Result<Arc<Core>, CoreError> {
        if opts.worker_threads == 0 {
            return Err(CoreError::internal("worker_threads must be positive"));
        }
        std::fs::create_dir_all(&cfg.data_dir)?;
        let data_dir = PathBuf::from(&cfg.data_dir);
        let data_dir = if data_dir.is_absolute() {
            data_dir
        } else {
            std::env::current_dir()?.join(data_dir)
        };
        let store = Store::open(&data_dir.join("rs.db"))?;
        // Do fallible work before creating a runtime: even an error path must
        // never drop a newly built runtime on another runtime's worker thread.
        if opts.init_logging {
            logging::init_global(&data_dir.join("logs"), "info").map_err(CoreError::internal)?;
            super::crash_hook::install_panic_hook(data_dir.join("crashes"), &cfg);
            super::crash_native::ingest_at_start(&data_dir.join("crashes"), &cfg);
        }
        let events = EventHub::new();
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(opts.worker_threads)
            .thread_name("rs-core")
            .enable_all()
            .build()?;
        Ok(Arc::new(Core {
            net: Mutex::new(opts.net.clone().unwrap_or_else(system_net)),
            cfg: Mutex::new(cfg),
            data_dir,
            store: Mutex::new(store),
            runtime: Some(runtime),
            events,
            states: Mutex::new(HashMap::new()),
            signer: Mutex::new(None),
            cache: Mutex::new(IdentityCache::default()),
            shutdown_hooks: Mutex::new(Some(Vec::new())),
            export_lock: Mutex::new(()),
            opts,
        }))
    }

    /// Snapshot the network for a new connection.
    pub fn net(&self) -> Arc<dyn NetDialer> {
        self.net.lock().unwrap_or_else(|e| e.into_inner()).clone()
    }

    /// Select the network for future connections; established connections keep theirs.
    pub fn set_net(&self, net: Arc<dyn NetDialer>) {
        let previous = std::mem::replace(
            &mut *self.net.lock().unwrap_or_else(|e| e.into_inner()),
            net,
        );
        drop(previous);
    }

    pub fn config(&self) -> CoreConfig {
        self.cfg
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
    }

    pub fn data_dir(&self) -> PathBuf {
        self.data_dir.clone()
    }

    pub fn log_dir(&self) -> PathBuf {
        self.data_dir.join("logs")
    }

    pub fn now_us(&self) -> u64 {
        match &self.opts.clock {
            Some(clock) => clock(),
            None => SystemTime::now()
                .duration_since(UNIX_EPOCH)
                .map(|elapsed| u64::try_from(elapsed.as_micros()).unwrap_or(u64::MAX))
                .unwrap_or(0),
        }
    }

    pub fn timeouts(&self) -> &Timeouts {
        &self.opts.timeouts
    }

    pub fn handle(&self) -> Handle {
        self.runtime
            .as_ref()
            .expect("Core owns its runtime until Drop")
            .handle()
            .clone()
    }

    pub async fn run<T, F>(&self, fut: F) -> Result<T, CoreError>
    where
        F: Future<Output = Result<T, CoreError>> + Send + 'static,
        T: Send + 'static,
    {
        self.handle().spawn(fut).await.map_err(|error| {
            if error.is_panic() {
                CoreError::internal("task panicked")
            } else {
                CoreError::cancelled("task cancelled")
            }
        })?
    }

    pub fn spawn<F>(&self, fut: F) -> tokio::task::JoinHandle<()>
    where
        F: Future<Output = ()> + Send + 'static,
    {
        self.handle().spawn(fut)
    }

    /// For synchronous tests only. Must not be called on a runtime thread.
    pub fn block_on<F: Future>(&self, fut: F) -> F::Output {
        self.handle().block_on(fut)
    }

    pub fn store(&self) -> MutexGuard<'_, Store> {
        self.store.lock().unwrap_or_else(|error| error.into_inner())
    }

    pub fn events(&self) -> &EventHub {
        &self.events
    }

    pub fn state<T: Default + Send + Sync + 'static>(&self) -> Arc<T> {
        let mut states = self
            .states
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        states
            .entry(TypeId::of::<T>())
            .or_insert_with(|| Arc::new(T::default()))
            .clone()
            .downcast::<T>()
            .unwrap_or_else(|_| unreachable!("states are indexed by their own TypeId"))
    }

    pub fn set_signer(&self, s: Option<Arc<dyn ExternalSigner>>) {
        let previous = std::mem::replace(
            &mut *self
                .signer
                .lock()
                .unwrap_or_else(|error| error.into_inner()),
            s,
        );
        drop(previous);
    }

    pub fn signer(&self) -> Option<Arc<dyn ExternalSigner>> {
        self.signer
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .clone()
            .or_else(global_signer)
    }

    pub fn local_identity(&self) -> Result<Arc<LocalIdentity>, CoreError> {
        let mut cache = self.cache.lock().unwrap_or_else(|error| error.into_inner());
        if let Some(identity) = &cache.identity {
            return Ok(identity.clone());
        }
        let stored = self
            .store()
            .identity()?
            .ok_or_else(|| CoreError::internal("identity not created"))?;
        let signer = self
            .signer()
            .ok_or_else(|| CoreError::internal("device signer not registered"))?;
        if !spki_matches(signer.as_ref(), &stored.cert_der) {
            return Err(CoreError::internal("device key changed"));
        }
        let identity = Arc::new(LocalIdentity {
            cert_der: stored.cert_der.into(),
            key: KeySource::External(signer),
        });
        cache.identity = Some(identity.clone());
        Ok(identity)
    }

    pub fn device_id(&self) -> Result<String, CoreError> {
        let stored = self
            .store()
            .identity()?
            .ok_or_else(|| CoreError::internal("identity not created"))?;
        let fp = spki_fingerprint(&stored.cert_der).map_err(CoreError::internal)?;
        Ok(id_from_fp(&fp))
    }

    pub fn identity_changed(&self) {
        let previous =
            std::mem::take(&mut *self.cache.lock().unwrap_or_else(|error| error.into_inner()));
        drop(previous);
    }

    pub fn add_shutdown_hook(&self, f: Box<dyn FnOnce() + Send>) {
        let mut hooks = self
            .shutdown_hooks
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        if let Some(hooks) = hooks.as_mut() {
            hooks.push(f);
        } else {
            drop(hooks);
            if catch_unwind(AssertUnwindSafe(f)).is_err() {
                tracing::error!("shutdown hook panicked");
            }
        }
    }

    pub fn shutdown(&self) {
        let hooks = self
            .shutdown_hooks
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .take();
        for hook in hooks.into_iter().flatten() {
            if catch_unwind(AssertUnwindSafe(hook)).is_err() {
                tracing::error!("shutdown hook panicked");
            }
        }
        self.events.clear();
    }

    pub fn update_config(&self, app_version: String) {
        self.cfg
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .app_version = app_version;
    }
}

impl Drop for Core {
    fn drop(&mut self) {
        self.shutdown();
        if let Some(runtime) = self.runtime.take() {
            runtime.shutdown_background();
        }
    }
}

static CORE: Mutex<Option<Arc<Core>>> = Mutex::new(None);

pub fn core() -> Result<Arc<Core>, CoreError> {
    CORE.lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
        .ok_or_else(|| CoreError::internal("core not initialized"))
}

pub fn install_core(c: Arc<Core>) {
    let previous = CORE
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .replace(c);
    drop(previous);
}

pub fn take_core() -> Option<Arc<Core>> {
    CORE.lock()
        .unwrap_or_else(|error| error.into_inner())
        .take()
}

/// Register transfer drivers for every source before transfer controls are used.
pub fn register_transfer_drivers(core: &Arc<Core>) {
    super::api_files::register_driver(core);
    super::api_sftp::register_driver(core);
}

/// Initialize once; JS reload clears subscriptions and updates the app version,
/// retaining typed extension state and sessions. A different dataDir is rejected.
pub fn init_core(cfg: CoreConfig) -> Result<(), CoreError> {
    let mut slot = CORE.lock().unwrap_or_else(|error| error.into_inner());
    if let Some(c) = slot.as_ref() {
        if c.config().data_dir != cfg.data_dir {
            return Err(CoreError::internal(
                "already initialized with another dataDir",
            ));
        }
        let c = c.clone();
        // Clearing may drop foreign sinks; do not retain the global lock while
        // executing their destructors.
        drop(slot);
        c.events().clear();
        c.update_config(cfg.app_version);
        register_transfer_drivers(&c);
    } else {
        let c = Core::new(cfg, CoreOptions::default())?;
        *slot = Some(c.clone());
        drop(slot);
        register_transfer_drivers(&c);
    }
    Ok(())
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn core_init(cfg_json: String) -> Result<(), CoreError> {
    init_core(serde_json::from_str(&cfg_json)?)
}

#[uniffi::export]
pub fn core_shutdown() {
    if let Some(c) = take_core() {
        c.shutdown();
    }
}

#[uniffi::export]
pub fn events_subscribe(sink: Arc<dyn EventSink>) -> Result<u64, CoreError> {
    Ok(core()?.events().subscribe(sink))
}

#[uniffi::export]
pub fn events_unsubscribe(id: u64) {
    if let Ok(c) = core() {
        c.events().unsubscribe(id);
    }
}

#[uniffi::export]
pub fn logs_write(level: String, tag: String, msg: String) {
    logging::write_js(&level, &tag, &msg);
}

/// Exports one archive, pruning only old regular `rs-logs-*.zip` files.
/// The lock serializes exports so concurrent requests cannot delete a new zip.
pub fn export_core_logs(c: &Arc<Core>, mask: bool) -> Result<String, CoreError> {
    let _lock = c
        .export_lock
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    let exports = c.data_dir().join("exports");
    std::fs::create_dir_all(&exports)?;
    for entry in std::fs::read_dir(&exports)? {
        let entry = entry?;
        if entry.file_type()?.is_file()
            && entry
                .file_name()
                .to_str()
                .is_some_and(|name| name.starts_with("rs-logs-") && name.ends_with(".zip"))
        {
            std::fs::remove_file(entry.path())?;
        }
    }
    let zip = logging::export_logs(&c.log_dir(), &exports, mask, c.now_us())
        .map_err(CoreError::internal)?;
    Ok(zip.to_string_lossy().into_owned())
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn logs_export(mask: bool) -> Result<String, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { export_core_logs(&c, mask) })
        .await
}
