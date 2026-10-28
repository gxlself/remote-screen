//! Ordered JSON events on one callback thread. The queue is intentionally unbounded.

use std::collections::BTreeMap;
use std::panic::{catch_unwind, AssertUnwindSafe};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc, Mutex};
use std::time::Duration;

use crate::uniffi_api::EventSink;

enum Message {
    Event(u64, String),
    Flush(mpsc::SyncSender<()>),
}

#[derive(Default)]
struct Subscribers {
    next_id: u64,
    sinks: BTreeMap<u64, Arc<dyn EventSink>>,
}

struct Dispatch {
    subscribers: Mutex<Subscribers>,
    stopped: AtomicBool,
}

struct Queue {
    seq: u64,
    sender: mpsc::Sender<Message>,
}

pub struct EventHub {
    dispatch: Arc<Dispatch>,
    queue: Mutex<Queue>,
}

impl Default for EventHub {
    fn default() -> Self {
        Self::new()
    }
}

impl EventHub {
    pub fn new() -> EventHub {
        let (sender, receiver) = mpsc::channel();
        let dispatch = Arc::new(Dispatch {
            subscribers: Mutex::new(Subscribers::default()),
            stopped: AtomicBool::new(false),
        });
        let worker = dispatch.clone();
        std::thread::Builder::new()
            .name("rs-events".to_owned())
            .spawn(move || {
                while let Ok(message) = receiver.recv() {
                    if worker.stopped.load(Ordering::Acquire) {
                        break;
                    }
                    match message {
                        Message::Event(seq, payload) => {
                            let sinks: Vec<_> = worker
                                .subscribers
                                .lock()
                                .unwrap_or_else(|error| error.into_inner())
                                .sinks
                                .iter()
                                .map(|(&id, sink)| (id, sink.clone()))
                                .collect();
                            for (id, sink) in sinks {
                                if worker.stopped.load(Ordering::Acquire) {
                                    break;
                                }
                                // A callback may clear/unsubscribe other callbacks. Never
                                // hold the subscriber lock across foreign code.
                                let active = worker
                                    .subscribers
                                    .lock()
                                    .unwrap_or_else(|error| error.into_inner())
                                    .sinks
                                    .contains_key(&id);
                                if active
                                    && catch_unwind(AssertUnwindSafe(|| {
                                        sink.on_event(seq, payload.clone());
                                    }))
                                    .is_err()
                                {
                                    tracing::error!(subscription_id = id, "event sink panicked");
                                }
                            }
                        }
                        Message::Flush(done) => {
                            let _ = done.send(());
                        }
                    }
                }
            })
            .expect("rs-events dispatcher thread must be available");
        EventHub {
            dispatch,
            queue: Mutex::new(Queue { seq: 0, sender }),
        }
    }

    pub fn subscribe(&self, sink: Arc<dyn EventSink>) -> u64 {
        let mut subscribers = self
            .dispatch
            .subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner());
        subscribers.next_id += 1;
        let id = subscribers.next_id;
        subscribers.sinks.insert(id, sink);
        id
    }

    /// An already executing callback may finish after this returns.
    pub fn unsubscribe(&self, id: u64) -> bool {
        let removed = self
            .dispatch
            .subscribers
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sinks
            .remove(&id);
        removed.is_some()
    }

    /// Clears subscriptions, including deliveries still waiting in the queue.
    pub fn clear(&self) {
        let removed = std::mem::take(
            &mut self
                .dispatch
                .subscribers
                .lock()
                .unwrap_or_else(|error| error.into_inner())
                .sinks,
        );
        drop(removed);
    }

    pub fn emit<T: serde::Serialize>(&self, event: &T) {
        let payload = match serde_json::to_string(event) {
            Ok(payload) => payload,
            Err(error) => {
                tracing::error!(%error, "event serialization failed");
                return;
            }
        };
        // Allocate the sequence and enqueue under the same lock so concurrent
        // producers cannot put a later sequence ahead of an earlier one.
        let mut queue = self.queue.lock().unwrap_or_else(|error| error.into_inner());
        queue.seq += 1;
        let _ = queue.sender.send(Message::Event(queue.seq, payload));
    }

    /// Wait for events queued before this call. Do not call from an event callback.
    pub fn flush(&self, timeout: Duration) -> bool {
        let (done, receiver) = mpsc::sync_channel(1);
        let sent = self
            .queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .sender
            .send(Message::Flush(done));
        sent.is_ok() && receiver.recv_timeout(timeout).is_ok()
    }

    pub fn last_seq(&self) -> u64 {
        self.queue
            .lock()
            .unwrap_or_else(|error| error.into_inner())
            .seq
    }
}

impl Drop for EventHub {
    fn drop(&mut self) {
        self.dispatch.stopped.store(true, Ordering::Release);
        // Dropping queue closes the channel. Detaching avoids joining a callback
        // that might itself be releasing the last owner of this hub.
    }
}
