//! Debounce address notifications and compare only fields used for listening.

use std::net::IpAddr;
use std::time::Duration;

use host_core::platform::Nic;
use tokio::sync::mpsc::Receiver;
use tokio::time::{sleep_until, Instant};

/// Wait for a quiet period after address changes.
pub const DEBOUNCE: Duration = Duration::from_millis(1500);
/// Bound the delay even when notifications keep arriving.
pub const MAX_DELAY: Duration = Duration::from_secs(6);
/// Recheck periodically in case a notification was lost.
pub const RESYNC: Duration = Duration::from_secs(60);

/// Sorted (name, sorted unique addresses, tailnet) tuples.
pub type NicSignature = Vec<(String, Vec<IpAddr>, bool)>;

/// Ignore enumeration order, duplicate addresses, MACs and network metadata.
pub fn listen_signature(nics: &[Nic]) -> NicSignature {
    let mut signature: NicSignature = nics
        .iter()
        .map(|nic| {
            let mut addresses = nic.addresses.clone();
            addresses.sort_unstable();
            addresses.dedup();
            (nic.name.clone(), addresses, nic.tailnet)
        })
        .collect();
    signature.sort_unstable();
    signature
}

/// Read an initial baseline without notifying, then report listening changes.
/// Closing the event channel flushes a pending burst before returning.
pub async fn follow_nics<R, C>(
    mut events: Receiver<()>,
    mut read: R,
    mut on_change: C,
    debounce: Duration,
    max_delay: Duration,
    resync: Duration,
) where
    R: FnMut() -> Vec<Nic> + Send,
    C: FnMut(Vec<Nic>) + Send,
{
    let mut baseline = listen_signature(&read());
    let mut resync_at = Instant::now() + resync;
    let mut next_read = resync_at;
    let mut pending_limit = None;

    loop {
        let closed = tokio::select! {
            // Deadlines take priority so a busy channel cannot starve reads.
            biased;
            _ = sleep_until(next_read) => false,
            event = events.recv() => match event {
                Some(()) => {
                    let now = Instant::now();
                    let limit = *pending_limit.get_or_insert(now + max_delay);
                    next_read = (now + debounce).min(limit).min(resync_at);
                    continue;
                }
                None => {
                    if pending_limit.is_none() {
                        return;
                    }
                    true
                }
            },
        };

        let nics = read();
        let signature = listen_signature(&nics);
        if signature != baseline {
            on_change(nics);
            baseline = signature;
        }
        if closed {
            return;
        }
        resync_at = Instant::now() + resync;
        next_read = resync_at;
        pending_limit = None;
    }
}
