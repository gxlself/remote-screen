mod support;

use std::sync::{mpsc, Arc, Mutex, Weak};
use std::time::Duration;

use rs_mobile::rscore::dto_core::CoreEvent;
use rs_mobile::rscore::events::EventHub;
use rs_mobile::uniffi_api::EventSink;
use serde::{Serialize, Serializer};
use support::EventCollector;

const WAIT: Duration = Duration::from_secs(5);

fn event(seconds_left: u32) -> CoreEvent {
    CoreEvent::SessionIdleWarning {
        session_id: "1".to_owned(),
        seconds_left,
        lock_on_end: false,
    }
}

#[test]
fn ordered_fanout_unsubscribe_clear_and_sequences() {
    let hub = EventHub::new();
    let first = EventCollector::new();
    let second = EventCollector::new();
    let id1 = hub.subscribe(first.clone());
    assert_eq!(id1, 1);
    assert_eq!(hub.subscribe(second.clone()), 2);
    for seconds in [3, 2, 1] {
        hub.emit(&event(seconds));
    }
    assert!(hub.flush(WAIT));
    let received = first.events();
    assert_eq!(received, second.events());
    assert_eq!(
        received.iter().map(|(seq, _)| *seq).collect::<Vec<_>>(),
        vec![1, 2, 3]
    );
    for ((_, payload), seconds) in received.iter().zip([3, 2, 1]) {
        assert_eq!(*payload, serde_json::to_value(event(seconds)).unwrap());
    }
    assert_eq!(first.of_type("session.idleWarning").len(), 3);
    assert!(first.wait_for(|events| events.len() == 3, WAIT));
    assert!(hub.unsubscribe(id1));
    assert!(!hub.unsubscribe(99));
    hub.emit(&event(0));
    assert!(hub.flush(WAIT));
    assert_eq!(first.events().len(), 3);
    assert_eq!(second.events().last().unwrap().0, 4);
    hub.clear();
    hub.emit(&event(9));
    assert!(hub.flush(WAIT));
    assert_eq!(hub.last_seq(), 5);
    assert_eq!(first.events().len(), 3);
    assert_eq!(second.events().len(), 4);
    assert_eq!(hub.subscribe(first), 3);
}

struct PanicSink;
impl EventSink for PanicSink {
    fn on_event(&self, _: u64, _: String) {
        panic!("test sink panic");
    }
}

#[test]
fn sink_panics_do_not_interrupt_delivery() {
    let hub = EventHub::new();
    hub.subscribe(Arc::new(PanicSink));
    let collected = EventCollector::new();
    hub.subscribe(collected.clone());
    for n in 0..3 {
        hub.emit(&event(n));
    }
    assert!(hub.flush(WAIT));
    hub.emit(&event(3));
    assert!(hub.flush(WAIT));
    assert_eq!(
        collected
            .events()
            .iter()
            .map(|(seq, _)| *seq)
            .collect::<Vec<_>>(),
        vec![1, 2, 3, 4]
    );
}

#[test]
fn thousand_events_and_concurrent_producers_have_no_gaps() {
    let hub = Arc::new(EventHub::new());
    let collected = EventCollector::new();
    hub.subscribe(collected.clone());
    for n in 0..1000 {
        hub.emit(&event(n));
    }
    assert!(hub.flush(WAIT));
    assert_eq!(
        collected
            .events()
            .iter()
            .map(|(seq, _)| *seq)
            .collect::<Vec<_>>(),
        (1..=1000).collect::<Vec<_>>()
    );
    let producers: Vec<_> = (0..4)
        .map(|_| {
            let hub = hub.clone();
            std::thread::spawn(move || {
                for n in 0..250 {
                    hub.emit(&event(n));
                }
            })
        })
        .collect();
    for producer in producers {
        producer.join().unwrap();
    }
    assert!(hub.flush(WAIT));
    assert_eq!(
        collected
            .events()
            .iter()
            .map(|(seq, _)| *seq)
            .collect::<Vec<_>>(),
        (1..=2000).collect::<Vec<_>>()
    );
}

struct BadJson;
impl Serialize for BadJson {
    fn serialize<S: Serializer>(&self, _: S) -> Result<S::Ok, S::Error> {
        Err(serde::ser::Error::custom("test serialization failure"))
    }
}

#[test]
fn bad_json_is_discarded_without_consuming_a_sequence() {
    let hub = EventHub::new();
    let collected = EventCollector::new();
    hub.subscribe(collected.clone());
    hub.emit(&BadJson);
    hub.emit(&event(0));
    assert!(hub.flush(WAIT));
    assert_eq!(hub.last_seq(), 1);
    assert_eq!(collected.events().len(), 1);
}

struct ClearSink(Weak<EventHub>);
impl EventSink for ClearSink {
    fn on_event(&self, _: u64, _: String) {
        self.0.upgrade().unwrap().clear();
    }
}

#[test]
fn callback_can_clear_subscriptions_and_subscription_order_is_respected() {
    let hub = Arc::new(EventHub::new());
    hub.subscribe(Arc::new(ClearSink(Arc::downgrade(&hub))));
    let later = EventCollector::new();
    hub.subscribe(later.clone());
    hub.emit(&event(0));
    assert!(hub.flush(WAIT));
    assert!(later.events().is_empty());
}

struct BlockingSink {
    started: mpsc::SyncSender<()>,
    release: Mutex<mpsc::Receiver<()>>,
    exited: mpsc::SyncSender<()>,
}
impl EventSink for BlockingSink {
    fn on_event(&self, _: u64, _: String) {
        self.started.send(()).unwrap();
        self.release.lock().unwrap().recv_timeout(WAIT).unwrap();
        self.exited.send(()).unwrap();
    }
}

#[test]
fn flush_times_out_and_drop_does_not_join_a_blocked_callback() {
    let hub = EventHub::new();
    let (started_tx, started_rx) = mpsc::sync_channel(1);
    let (release_tx, release_rx) = mpsc::sync_channel(1);
    let (exited_tx, exited_rx) = mpsc::sync_channel(1);
    hub.subscribe(Arc::new(BlockingSink {
        started: started_tx,
        release: Mutex::new(release_rx),
        exited: exited_tx,
    }));
    hub.emit(&event(0));
    started_rx.recv_timeout(WAIT).unwrap();
    assert!(!hub.flush(Duration::from_millis(10)));
    let (dropped_tx, dropped_rx) = mpsc::sync_channel(1);
    let thread = std::thread::spawn(move || {
        drop(hub);
        dropped_tx.send(()).unwrap();
    });
    let dropped = dropped_rx.recv_timeout(Duration::from_secs(1)).is_ok();
    release_tx.send(()).unwrap();
    thread.join().unwrap();
    exited_rx.recv_timeout(WAIT).unwrap();
    assert!(dropped, "drop must not wait for a foreign callback");
}
