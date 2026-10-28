use std::sync::{mpsc, Arc};
use std::time::{Duration, Instant};

use rs_mobile::uniffi_api::{ping, slow, start_events, stop_events, EventSink};

struct Sink(mpsc::Sender<(u64, String)>);

impl EventSink for Sink {
    fn on_event(&self, seq: u64, payload: String) {
        let _ = self.0.send((seq, payload));
    }
}

#[test]
fn exported_api_timing_and_event_restart_lifecycle() {
    assert_eq!(ping(), "pong");
    let runtime = tokio::runtime::Builder::new_current_thread()
        .enable_all()
        .build()
        .unwrap();
    let started = Instant::now();
    let elapsed = runtime.block_on(slow(30));
    assert!(elapsed.starts_with("slept_ms="));
    assert!(started.elapsed() >= Duration::from_millis(25));

    let (send, receive) = mpsc::channel();
    start_events(Arc::new(Sink(send))).unwrap();
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(1)).unwrap(),
        (0, "event-0".into())
    );
    assert_eq!(
        receive.recv_timeout(Duration::from_secs(1)).unwrap(),
        (1, "event-1".into())
    );

    // Replacement must release the previous sink and restart at sequence zero.
    let (send_new, receive_new) = mpsc::channel();
    start_events(Arc::new(Sink(send_new))).unwrap();
    assert_eq!(
        receive_new.recv_timeout(Duration::from_secs(1)).unwrap().0,
        0
    );
    assert!(receive.recv_timeout(Duration::from_millis(600)).is_err());
    stop_events();
    // Drain a tick that raced the stop call; subsequent ticks must cease.
    while receive_new.try_recv().is_ok() {}
    assert!(receive_new
        .recv_timeout(Duration::from_millis(600))
        .is_err());
}
