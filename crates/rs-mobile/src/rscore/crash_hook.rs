//! Process-wide Rust panic hook writing crash reports.
use std::cell::Cell;
use std::path::PathBuf;
use std::sync::atomic::{AtomicUsize, Ordering};
use std::sync::OnceLock;
use std::time::{SystemTime, UNIX_EPOCH};

use super::crash::{
    self, CrashKind, CrashReport, MESSAGE_MAX, NAME_MAX, REPORT_VERSION, STACK_MAX,
};
use super::dto_core::{CoreConfig, DevicePlatform};

pub const PANICS_PER_PROCESS: usize = 5;
pub const LOCATION_MAX: usize = 512;

struct Target {
    dir: PathBuf,
    app_version: String,
    platform: DevicePlatform,
}

static TARGET: OnceLock<Target> = OnceLock::new();
static PANICS: AtomicUsize = AtomicUsize::new(0);

thread_local! {
    static IN_HOOK: Cell<bool> = const { Cell::new(false) };
}

struct HookGuard;

impl Drop for HookGuard {
    fn drop(&mut self) {
        let _ = IN_HOOK.try_with(|in_hook| in_hook.set(false));
    }
}

/// Installs the panic hook once per process.
///
/// The first directory and configuration win. Caught panics and failed writes
/// also count toward the process limit.
pub fn install_panic_hook(dir: PathBuf, cfg: &CoreConfig) {
    if TARGET.get().is_some() {
        return;
    }
    let target = Target {
        dir,
        app_version: cfg.app_version.clone(),
        platform: cfg.platform,
    };
    if TARGET.set(target).is_err() {
        return;
    }

    let previous = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |info| {
        record(info);
        previous(info);
    }));
}

fn record(info: &std::panic::PanicHookInfo<'_>) {
    match IN_HOOK.try_with(|in_hook| in_hook.replace(true)) {
        Ok(false) => {}
        _ => return,
    }
    let _guard = HookGuard;

    if PANICS.fetch_add(1, Ordering::SeqCst) >= PANICS_PER_PROCESS {
        return;
    }
    let Some(target) = TARGET.get() else {
        return;
    };

    let message = if let Some(text) = info.payload().downcast_ref::<&str>() {
        crash::sanitize(text, MESSAGE_MAX)
    } else if let Some(text) = info.payload().downcast_ref::<String>() {
        crash::sanitize(text, MESSAGE_MAX)
    } else {
        "<non-string panic payload>".to_owned()
    };
    let location = info.location().map(|location| {
        crash::sanitize(
            &format!(
                "{}:{}:{}",
                location.file(),
                location.line(),
                location.column()
            ),
            LOCATION_MAX,
        )
    });
    let current = std::thread::current();
    let thread = crash::sanitize(current.name().unwrap_or("<unnamed>"), NAME_MAX);
    let stack = crash::sanitize(
        &std::backtrace::Backtrace::force_capture().to_string(),
        STACK_MAX,
    );
    let incident_at = match SystemTime::now().duration_since(UNIX_EPOCH) {
        Ok(duration) => duration.as_micros().min(u128::from(u64::MAX)) as u64,
        Err(_) => 0,
    };
    let report = CrashReport {
        v: REPORT_VERSION,
        kind: CrashKind::RustPanic,
        incident_at,
        app_version: target.app_version.clone(),
        platform: target.platform,
        os_version: None,
        thread: Some(thread),
        location,
        name: None,
        message,
        stack: Some(stack),
    };
    let _ = crash::prune(&target.dir, incident_at);
    let _ = crash::write_report(&target.dir, &report);
}
