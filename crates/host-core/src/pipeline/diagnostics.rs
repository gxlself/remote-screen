//! Opt-in process-wide counters for isolated pipeline load tests.
//! No frame contents or peer identifiers are retained.

use std::sync::atomic::{AtomicBool, AtomicU64, Ordering::Relaxed};

static ENABLED: AtomicBool = AtomicBool::new(false);
static COUNTERS: [AtomicU64; 20] = [const { AtomicU64::new(0) }; 20];

#[derive(Clone, Copy)]
pub(crate) enum Counter {
    Starts,
    Requests,
    ForceRequest,
    ForceDeadline,
    ForceEncodeError,
    ForceRefineError,
    EncodeFull,
    Keyframes,
    KeyBytes,
    KeyMaxBytes,
    QueueMaxFrames,
    BacklogMaxBytes,
    Waits,
    Timeouts,
    SourceDrops,
    ChainDrops,
    ForceRepair,
    SendAgeMaxUs,
    SendDurationMaxUs,
    StartAgeMaxUs,
}

pub(crate) fn add(counter: Counter, value: u64) {
    if ENABLED.load(Relaxed) {
        COUNTERS[counter as usize].fetch_add(value, Relaxed);
    }
}

pub(crate) fn max(counter: Counter, value: u64) {
    if ENABLED.load(Relaxed) {
        COUNTERS[counter as usize].fetch_max(value, Relaxed);
    }
}

/// Enable counters before creating an isolated test session. Snapshots are
/// cumulative across pipelines, including rebuilds; concurrent sessions mix.
pub fn enable() {
    ENABLED.store(true, Relaxed);
}

/// Start a measured interval after a test session's warmup has completed.
pub fn reset() {
    for counter in &COUNTERS {
        counter.store(0, Relaxed);
    }
}

/// Named counters suitable for once-per-second diagnostic output.
pub fn snapshot() -> Vec<(&'static str, u64)> {
    [
        "starts",
        "requests",
        "force_request",
        "force_deadline",
        "force_encode_error",
        "force_refine_error",
        "encode_full",
        "keyframes",
        "key_bytes",
        "key_max_bytes",
        "queue_max_frames",
        "backlog_max_bytes",
        "waits",
        "timeouts",
        "source_drops",
        "chain_drops",
        "force_repair",
        "send_age_max_us",
        "send_duration_max_us",
        "start_age_max_us",
    ]
    .into_iter()
    .zip(COUNTERS.iter().map(|counter| counter.load(Relaxed)))
    .collect()
}
