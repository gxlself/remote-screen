//! UniFFI control-plane placeholder API.
//!
//! The S2 spike proved the Rust → React Native path with these small calls.
//! They live in `rs-mobile` so the eventual RsCore API and the existing C ABI
//! are emitted by the same static library.

use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{Arc, Mutex, OnceLock};
use std::time::{Duration, Instant};

use tokio::runtime::Runtime;
use tokio::task::JoinHandle;
use tokio::time::sleep;

/// A callback implemented by the React Native generated module.
#[uniffi::export(with_foreign)]
pub trait EventSink: Send + Sync {
    fn on_event(&self, seq: u64, payload: String);
}

type EventTask = (Arc<AtomicBool>, JoinHandle<()>);
static EVENTS: OnceLock<Mutex<Option<EventTask>>> = OnceLock::new();
static RUNTIME: OnceLock<Result<Runtime, String>> = OnceLock::new();

fn events_slot() -> &'static Mutex<Option<EventTask>> {
    EVENTS.get_or_init(|| Mutex::new(None))
}

#[derive(Debug, thiserror::Error, uniffi::Error)]
pub enum SpikeError {
    #[error("event runtime unavailable: {message}")]
    Runtime { message: String },
    #[error("event task state was poisoned")]
    EventState,
}

/// A synchronous call used to prove the TurboModule path is live.
#[uniffi::export]
pub fn ping() -> String {
    "pong".to_owned()
}

/// An asynchronous call used to prove the Tokio future path is live.
#[uniffi::export(async_runtime = "tokio")]
pub async fn slow(ms: u32) -> String {
    let started = Instant::now();
    sleep(Duration::from_millis(u64::from(ms))).await;
    format!("slept_ms={}", started.elapsed().as_millis())
}

/// Start a five-hundred millisecond event ticker. Calling it again replaces
/// the previous ticker so a JS reload cannot create duplicate event streams.
#[uniffi::export]
pub fn start_events(sink: Arc<dyn EventSink>) -> Result<(), SpikeError> {
    stop_events();

    let runtime = RUNTIME
        .get_or_init(|| {
            tokio::runtime::Builder::new_multi_thread()
                .worker_threads(1)
                .thread_name("rs-events")
                .enable_all()
                .build()
                .map_err(|error| error.to_string())
        })
        .as_ref()
        .map_err(|message| SpikeError::Runtime {
            message: message.clone(),
        })?;
    let mut slot = events_slot().lock().map_err(|_| SpikeError::EventState)?;
    let stopped = Arc::new(AtomicBool::new(false));
    let stop_for_task = Arc::clone(&stopped);
    let task = runtime.spawn(async move {
        let mut seq = 0_u64;
        while !stop_for_task.load(Ordering::Acquire) {
            sink.on_event(seq, format!("event-{seq}"));
            seq = seq.saturating_add(1);
            sleep(Duration::from_millis(500)).await;
        }
    });
    *slot = Some((stopped, task));
    Ok(())
}

/// Stop future ticks. A callback already queued on the JS thread may still be
/// delivered; no thread is joined here, so JS can safely stop its own sink.
#[uniffi::export]
pub fn stop_events() {
    let previous = events_slot().lock().ok().and_then(|mut slot| slot.take());
    if let Some((stopped, task)) = previous {
        stopped.store(true, Ordering::Release);
        task.abort();
    }
}

// File browser and transfer exports.

pub use crate::rscore::files_upload::files_upload;

// Keep the facade's Rust API while the implementation owns the UniFFI exports.
pub use crate::rscore::api_sftp::{sftp_download, sftp_list, sftp_upload};

#[uniffi::export]
pub fn transfers_list() -> Result<String, crate::rscore::error::CoreError> {
    let core = crate::rscore::runtime::core()?;
    Ok(serde_json::to_string(&crate::rscore::xfer::list(&core))?)
}

#[uniffi::export]
pub fn transfers_pause(transfer_id: String) {
    let result = crate::rscore::runtime::core()
        .and_then(|core| crate::rscore::xfer::pause(&core, &transfer_id));
    if let Err(error) = result {
        tracing::warn!(%error, "transfers.pause failed");
    }
}

#[uniffi::export]
pub async fn transfers_resume(transfer_id: String) -> Result<(), crate::rscore::error::CoreError> {
    let core = crate::rscore::runtime::core()?;
    core.clone()
        .run(async move { crate::rscore::xfer::resume(&core, &transfer_id).await })
        .await
}

#[uniffi::export]
pub fn transfers_cancel(transfer_id: String) {
    let result = crate::rscore::runtime::core()
        .and_then(|core| crate::rscore::xfer::cancel(&core, &transfer_id));
    if let Err(error) = result {
        tracing::warn!(%error, "transfers.cancel failed");
    }
}

#[uniffi::export]
pub fn transfers_remove(transfer_id: String) {
    let result = crate::rscore::runtime::core()
        .and_then(|core| crate::rscore::xfer::remove(&core, &transfer_id));
    if let Err(error) = result {
        tracing::warn!(%error, "transfers.remove failed");
    }
}

// 实现见 rscore::api_files
pub use crate::rscore::api_files::{files_download, files_list};
