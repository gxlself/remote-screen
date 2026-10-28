//! Synchronous node operations and a dedicated event callback thread.

use crate::{
    abi::{code_to_io_kind, Api},
    fdsource::TailnetFdSource,
    Code, TailnetConfig, TailnetError, TailnetEvent, TailnetStatus,
};
use rsp_transport::{
    fdnet::{FdNetDialer, FdSource},
    net::NetDialer,
};
use std::{
    ffi::{c_char, CString},
    io,
    net::SocketAddr,
    os::{
        fd::{FromRawFd, OwnedFd},
        unix::ffi::OsStrExt,
    },
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Condvar, Mutex, MutexGuard, OnceLock,
    },
    thread::{self, JoinHandle, ThreadId},
    time::Duration,
};

const MAX_JSON: usize = 8 * 1024 * 1024;
const ERR_CAP: usize = 512;
type Callback = Arc<dyn Fn(TailnetEvent) + Send + Sync>;

#[derive(Default)]
struct Inner {
    events: Option<JoinHandle<()>>,
    event_thread: Option<ThreadId>,
    stopping: bool,
    stop_flag: Arc<AtomicBool>,
    stop_complete: Arc<AtomicBool>,
}

pub struct Tailnet {
    api: &'static Api,
    inner: Mutex<Inner>,
    stopped: Condvar,
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

fn error(code: Code, message: impl Into<String>) -> TailnetError {
    TailnetError {
        code,
        message: message.into(),
    }
}

fn check(rc: i64, buf: &[u8]) -> Result<(), TailnetError> {
    if rc >= 0 {
        return Ok(());
    }
    let end = buf.iter().position(|&byte| byte == 0).unwrap_or(buf.len());
    Err(error(
        Code::from_raw(rc),
        String::from_utf8_lossy(&buf[..end]),
    ))
}

impl Tailnet {
    /// Independent wrapper for test tables. Production code must use `global`.
    /// `api` must implement the C header's buffer and owned-fd contract.
    pub fn new(api: &'static Api) -> Arc<Self> {
        Arc::new(Self {
            api,
            inner: Mutex::new(Inner::default()),
            stopped: Condvar::new(),
        })
    }

    pub fn global(api: &'static Api) -> Arc<Self> {
        static GLOBAL: OnceLock<Arc<Tailnet>> = OnceLock::new();
        let node = GLOBAL.get_or_init(|| Self::new(api));
        assert!(
            std::ptr::eq(node.api, api),
            "Tailnet::global called with a different Api"
        );
        node.clone()
    }

    /// Starts the node and dispatches events on `rs-tailnet-events`.
    /// Do not call `stop` from the callback: forward that request to another thread.
    pub fn start(&self, cfg: &TailnetConfig, on_event: Callback) -> Result<(), TailnetError> {
        let mut inner = lock(&self.inner);
        if inner.stopping
            || inner
                .events
                .as_ref()
                .is_some_and(|thread| !thread.is_finished())
        {
            return Err(error(Code::State, "already started"));
        }
        // Reap a naturally closed event stream before starting another one.
        if let Some(events) = inner.events.take() {
            let _ = events.join();
            inner.event_thread = None;
        }
        for value in [
            Some(cfg.hostname.as_bytes()),
            Some(cfg.control_url.as_bytes()),
            cfg.auth_key.as_deref().map(str::as_bytes),
            Some(cfg.dir.as_os_str().as_bytes()),
            cfg.log_file
                .as_ref()
                .map(|path| path.as_os_str().as_bytes()),
        ]
        .into_iter()
        .flatten()
        {
            if value.contains(&0) {
                return Err(error(Code::Invalid, "config string contains NUL"));
            }
        }
        let mut wire =
            serde_json::to_value(cfg).map_err(|e| error(Code::Invalid, e.to_string()))?;
        // ABI v1's Go Config.Verbose is a level (0..=2); the public Rust
        // configuration is a boolean. Keep its JSON/API contract and translate
        // only at the C boundary, where false/true selects level 0/1.
        wire["verbose"] = serde_json::Value::from(u8::from(cfg.verbose));
        let json = serde_json::to_vec(&wire).map_err(|e| error(Code::Invalid, e.to_string()))?;
        let config = CString::new(json).map_err(|_| error(Code::Invalid, "config contains NUL"))?;
        let mut err = [0u8; ERR_CAP];
        // SAFETY: config is a live C string, err is writable for exactly ERR_CAP bytes.
        let rc = unsafe { (self.api.start)(config.as_ptr(), err.as_mut_ptr().cast(), err.len()) };
        check(i64::from(rc), &err)?;
        inner.stop_flag = Arc::new(AtomicBool::new(false));
        inner.stop_complete = Arc::new(AtomicBool::new(false));
        let stop_flag = inner.stop_flag.clone();
        let stop_complete = inner.stop_complete.clone();
        let api = self.api;
        let events = match thread::Builder::new()
            .name("rs-tailnet-events".into())
            .spawn(move || event_loop(api, on_event, stop_flag, stop_complete))
        {
            Ok(events) => events,
            Err(e) => {
                // SAFETY: stop has no arguments; roll back the successful start.
                unsafe { (api.stop)() };
                return Err(error(Code::Io, format!("start event thread: {e}")));
            }
        };
        inner.event_thread = Some(events.thread().id());
        inner.events = Some(events);
        Ok(())
    }

    /// Idempotently stops Go and joins the event thread before returning.
    pub fn stop(&self) -> Result<(), TailnetError> {
        let mut inner = lock(&self.inner);
        if inner.event_thread == Some(thread::current().id()) {
            return Err(error(
                Code::State,
                "stop must not run in the event callback",
            ));
        }
        while inner.stopping {
            inner = self
                .stopped
                .wait(inner)
                .unwrap_or_else(|error| error.into_inner());
        }
        inner.stopping = true;
        inner.stop_flag.store(true, Ordering::Release);
        let complete = inner.stop_complete.clone();
        let events = inner.events.take();
        drop(inner);
        // SAFETY: stop has no arguments and is thread-safe under the ABI contract.
        let rc = unsafe { (self.api.stop)() };
        complete.store(true, Ordering::Release);
        let joined = events.map(|events| events.join());
        let mut inner = lock(&self.inner);
        inner.event_thread = None;
        inner.stopping = false;
        self.stopped.notify_all();
        check(i64::from(rc), b"rs_ts_stop failed")?;
        if joined.is_some_and(|result| result.is_err()) {
            return Err(error(Code::Io, "event thread panicked"));
        }
        Ok(())
    }

    fn with_error(
        &self,
        function: unsafe extern "C" fn(*mut c_char, usize) -> i32,
    ) -> Result<(), TailnetError> {
        let mut err = [0u8; ERR_CAP];
        // SAFETY: err is writable for err.len() bytes; the function comes from Api.
        let rc = unsafe { function(err.as_mut_ptr().cast(), err.len()) };
        check(i64::from(rc), &err)
    }

    pub fn login(&self) -> Result<(), TailnetError> {
        self.with_error(self.api.login)
    }
    pub fn logout(&self) -> Result<(), TailnetError> {
        self.with_error(self.api.logout)
    }

    pub fn rebind(&self) -> Result<(), TailnetError> {
        // SAFETY: the ABI's rebind takes no arguments and is thread-safe.
        check(
            i64::from(unsafe { (self.api.rebind)() }),
            b"rs_ts_rebind failed",
        )
    }

    pub fn status(&self) -> Result<TailnetStatus, TailnetError> {
        let mut buf = vec![0u8; 16 * 1024];
        for _ in 0..3 {
            // SAFETY: buf owns buf.len() writable bytes for this synchronous call.
            let length = unsafe { (self.api.status)(buf.as_mut_ptr().cast(), buf.len()) };
            check(length, b"rs_ts_status failed")?;
            if length > MAX_JSON as i64 {
                return Err(error(Code::Io, "status JSON exceeds 8 MiB"));
            }
            let length = length as usize;
            if length > buf.len() {
                buf.resize(length + 1, 0);
                continue;
            }
            return serde_json::from_slice(&buf[..length])
                .map_err(|e| error(Code::Io, format!("status JSON: {e}")));
        }
        Err(error(Code::Io, "status JSON kept growing after 3 attempts"))
    }

    pub fn dial_udp(&self, peer: SocketAddr, timeout: Duration) -> io::Result<OwnedFd> {
        self.dial(
            &peer.ip().to_string(),
            peer.port(),
            timeout,
            self.api.dial_udp,
        )
    }

    pub fn dial_tcp(&self, host: &str, port: u16, timeout: Duration) -> io::Result<OwnedFd> {
        self.dial(host, port, timeout, self.api.dial_tcp)
    }

    fn dial(
        &self,
        host: &str,
        port: u16,
        timeout: Duration,
        function: unsafe extern "C" fn(*const c_char, u16, i32, *mut c_char, usize) -> i32,
    ) -> io::Result<OwnedFd> {
        let host = CString::new(host)
            .map_err(|_| io::Error::new(io::ErrorKind::InvalidInput, "host contains NUL"))?;
        let timeout = timeout.as_millis().min(i32::MAX as u128) as i32;
        let mut err = [0u8; ERR_CAP];
        // SAFETY: host is a live C string and err is writable for err.len() bytes.
        let fd = unsafe {
            function(
                host.as_ptr(),
                port,
                timeout,
                err.as_mut_ptr().cast(),
                err.len(),
            )
        };
        check(i64::from(fd), &err)
            .map_err(|e| io::Error::new(code_to_io_kind(e.code), e.message))?;
        // SAFETY: a successful ABI dial transfers a fresh, valid descriptor to Rust.
        Ok(unsafe { OwnedFd::from_raw_fd(fd) })
    }

    pub fn fd_source(self: &Arc<Self>) -> Arc<dyn FdSource> {
        Arc::new(TailnetFdSource {
            tailnet: self.clone(),
            udp_timeout: Duration::from_secs(5),
            tcp_timeout: Duration::from_secs(10),
        })
    }

    /// `FdNetDialer` moves blocking ABI dials onto `spawn_blocking` for callers.
    pub fn net_dialer(self: &Arc<Self>) -> Arc<dyn NetDialer> {
        Arc::new(FdNetDialer::new(self.fd_source()))
    }
}

// No Drop side effects: the global lives forever, and test instances must stop
// explicitly. In particular, dropping the final Arc on an event thread cannot join itself.

fn event_loop(
    api: &'static Api,
    callback: Callback,
    stop: Arc<AtomicBool>,
    complete: Arc<AtomicBool>,
) {
    let mut buf = vec![0u8; 4096];
    loop {
        // Snapshot before polling: a timeout that raced Go's final enqueue must
        // not skip `stopped`. Once stop completed, drain the queue without waiting.
        let stopping = stop.load(Ordering::Acquire) && complete.load(Ordering::Acquire);
        let timeout = if stopping { 0 } else { 1000 };
        // SAFETY: buf is writable for buf.len() bytes and Api remains live forever.
        let length = unsafe { (api.next_event)(buf.as_mut_ptr().cast(), buf.len(), timeout) };
        if length < 0 {
            break;
        }
        if length == 0 {
            if stopping {
                break;
            }
            continue;
        }
        if length > MAX_JSON as i64 {
            tracing::warn!("tailnet event exceeds 8 MiB; ending event stream");
            break;
        }
        let length = length as usize;
        if length > buf.len() {
            buf.resize(length + 1, 0);
            continue;
        }
        match serde_json::from_slice(&buf[..length]) {
            Ok(event) => {
                if catch_unwind(AssertUnwindSafe(|| callback(event))).is_err() {
                    tracing::warn!("tailnet event callback panicked");
                }
            }
            Err(error) => {
                // Do not log raw events or serde's offending values (possibly secrets).
                tracing::warn!(
                    line = error.line(),
                    column = error.column(),
                    "invalid tailnet event JSON"
                );
            }
        }
    }
}
