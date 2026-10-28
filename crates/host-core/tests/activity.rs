use std::sync::Arc;
use std::time::Duration;

use host_core::clock::Clock;
use host_core::ipc::{Event, SessionRole, SessionSummary};
use host_core::session::activity::{
    local_activity_msg, sessions_changed_event, spawn_tracker, ActivityHandle, ActivityPolicy,
    ActivityTransition, GateChange, InputGate, LocalActivitySource, RECENT_WINDOW_US,
    RESUME_AFTER_US,
};
use rsp_core::pb;
use serde_json::Value;
use tokio::sync::{mpsc, watch};
use tokio::time::{advance, Instant};

struct TestClock {
    base_us: u64,
    started: Instant,
}

impl TestClock {
    fn new() -> Self {
        Self {
            base_us: 1_000_000,
            started: Instant::now(),
        }
    }
}

impl Clock for TestClock {
    fn now_micros(&self) -> u64 {
        self.base_us + self.started.elapsed().as_micros() as u64
    }
}

fn harness(
    pause_enabled: bool,
) -> (
    mpsc::Sender<bool>,
    ActivityHandle,
    tokio::task::JoinHandle<()>,
) {
    let (tx, rx) = mpsc::channel(8);
    let clock = Arc::new(TestClock::new());
    let (handle, task) = spawn_tracker(rx, pause_enabled, clock);
    (tx, handle, task)
}

async fn wait_for_paused(rx: &mut watch::Receiver<bool>, expected: bool) {
    tokio::time::timeout(
        Duration::from_secs(1),
        rx.wait_for(|paused| *paused == expected),
    )
    .await
    .expect("timed out waiting for pause state")
    .expect("pause watch closed");
}

fn summary(
    session_id: &str,
    device_id: &str,
    device_name: &str,
    role: SessionRole,
    connected_at_us: u64,
) -> SessionSummary {
    SessionSummary {
        session_id: session_id.into(),
        device_id: device_id.into(),
        device_name: device_name.into(),
        role,
        connected_at_us,
        recording: false,
    }
}

#[test]
fn input_pulses_pause_and_resume_after_three_seconds() {
    let t0 = 10_000_000;
    let mut policy = ActivityPolicy::new(true);

    assert_eq!(policy.on_local_input(t0), Some(ActivityTransition::Paused));
    assert_eq!(policy.on_local_input(t0 + 1_000_000), None);
    assert_eq!(policy.poll(t0 + 3_999_999), None);
    assert_eq!(
        policy.poll(t0 + 4_000_000),
        Some(ActivityTransition::Resumed)
    );
}

#[test]
fn continuous_input_keeps_session_paused() {
    let t0 = 20_000_000;
    let mut policy = ActivityPolicy::new(true);

    assert_eq!(policy.on_local_input(t0), Some(ActivityTransition::Paused));
    for second in 1..10 {
        assert_eq!(policy.on_local_input(t0 + second * 1_000_000), None);
        assert!(policy.is_paused());
    }

    let last = t0 + 9_000_000;
    assert_eq!(policy.poll(last + RESUME_AFTER_US - 1), None);
    assert_eq!(
        policy.poll(last + RESUME_AFTER_US),
        Some(ActivityTransition::Resumed)
    );
}

#[test]
fn disabled_pause_still_tracks_recent_activity() {
    let t0 = 30_000_000;
    let mut policy = ActivityPolicy::new(false);

    assert_eq!(policy.on_local_input(t0), None);
    assert!(!policy.is_paused());
    assert!(policy.active_within(t0 + RECENT_WINDOW_US - 1, RECENT_WINDOW_US));
    assert!(!policy.active_within(t0 + RECENT_WINDOW_US, RECENT_WINDOW_US));

    let never_active = ActivityPolicy::new(false);
    assert!(!never_active.active_within(t0 + 1, RECENT_WINDOW_US));
}

#[test]
fn disabling_pause_resumes_and_reenabling_waits_for_input() {
    let t0 = 40_000_000;
    let mut policy = ActivityPolicy::new(true);

    assert_eq!(policy.on_local_input(t0), Some(ActivityTransition::Paused));
    assert_eq!(
        policy.set_pause_enabled(false, t0 + 1),
        Some(ActivityTransition::Resumed)
    );
    assert!(!policy.is_paused());
    assert_eq!(policy.set_pause_enabled(true, t0 + 2), None);
    assert!(!policy.is_paused());
    assert_eq!(
        policy.on_local_input(t0 + 3),
        Some(ActivityTransition::Paused)
    );
}

#[test]
fn next_deadline_tracks_paused_input() {
    let t0 = 50_000_000;
    let mut policy = ActivityPolicy::new(true);

    assert_eq!(policy.next_deadline(), None);
    assert_eq!(policy.on_local_input(t0), Some(ActivityTransition::Paused));
    assert_eq!(policy.next_deadline(), Some(t0 + RESUME_AFTER_US));
    assert_eq!(
        policy.poll(t0 + RESUME_AFTER_US),
        Some(ActivityTransition::Resumed)
    );
    assert_eq!(policy.next_deadline(), None);
}

#[test]
fn clock_rollback_saturates_without_panicking() {
    let t0 = 60_000_000;
    let mut policy = ActivityPolicy::new(true);

    assert_eq!(policy.on_local_input(t0), Some(ActivityTransition::Paused));
    assert_eq!(policy.poll(t0 - 5_000_000), None);
    assert!(policy.active_within(t0 - 5_000_000, RECENT_WINDOW_US));
    assert_eq!(policy.next_deadline(), Some(t0 + RESUME_AFTER_US));
}

#[tokio::test(start_paused = true)]
async fn tracker_pauses_on_true_and_resumes_after_three_seconds() {
    let (tx, handle, task) = harness(true);
    let mut paused = handle.subscribe();
    assert!(!*paused.borrow());

    tx.send(true).await.unwrap();
    wait_for_paused(&mut paused, true).await;
    assert!(LocalActivitySource::input_paused_local(&handle));

    advance(Duration::from_secs(3)).await;
    wait_for_paused(&mut paused, false).await;
    assert!(!handle.is_paused());

    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn tracker_ignores_false_pulses() {
    let (tx, handle, task) = harness(true);
    let mut paused = handle.subscribe();

    tx.send(true).await.unwrap();
    wait_for_paused(&mut paused, true).await;
    tx.send(false).await.unwrap();
    tokio::task::yield_now().await;
    assert!(handle.is_paused());

    advance(Duration::from_micros(RESUME_AFTER_US - 1)).await;
    assert!(handle.is_paused());
    advance(Duration::from_micros(1)).await;
    wait_for_paused(&mut paused, false).await;

    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn tracker_hot_config_disable_resumes_immediately() {
    let (tx, handle, task) = harness(true);
    let mut paused = handle.subscribe();

    tx.send(true).await.unwrap();
    wait_for_paused(&mut paused, true).await;
    handle.set_pause_enabled(false);
    wait_for_paused(&mut paused, false).await;
    assert!(!handle.is_paused());

    handle.set_pause_enabled(true);
    tokio::task::yield_now().await;
    assert!(!handle.is_paused());
    tx.send(true).await.unwrap();
    wait_for_paused(&mut paused, true).await;

    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn late_subscriber_sees_current_pause_state() {
    let (tx, handle, task) = harness(true);

    tx.send(true).await.unwrap();
    tokio::time::timeout(Duration::from_secs(1), async {
        while !handle.is_paused() {
            tokio::task::yield_now().await;
        }
    })
    .await
    .unwrap();
    assert!(*handle.subscribe().borrow());

    drop(tx);
    task.await.unwrap();
}

#[tokio::test(start_paused = true)]
async fn tracker_resumes_and_ends_when_platform_closes_stream() {
    let (tx, handle, task) = harness(true);
    let mut paused = handle.subscribe();

    tx.send(true).await.unwrap();
    wait_for_paused(&mut paused, true).await;
    drop(tx);

    task.await.unwrap();
    assert!(!handle.is_paused());
    assert!(!*paused.borrow());
}

#[tokio::test(start_paused = true)]
async fn recent_activity_uses_strict_sixty_second_window() {
    let (tx, handle, _task) = harness(true);
    let mut paused = handle.subscribe();

    tx.send(true).await.unwrap();
    wait_for_paused(&mut paused, true).await;
    assert!(LocalActivitySource::local_activity_within_60s(&handle));

    advance(Duration::from_secs(60)).await;
    assert!(!handle.active_within_60s());
    assert!(!LocalActivitySource::local_activity_within_60s(&handle));

    drop(tx);
}

#[test]
fn input_gate_tracks_all_closing_conditions() {
    let mut gate = InputGate::default();
    assert!(!gate.allowed());

    assert_eq!(gate.update(true, false, false), GateChange::Opened);
    assert!(gate.allowed());
    assert_eq!(gate.update(true, false, false), GateChange::Unchanged);

    assert_eq!(gate.update(true, true, false), GateChange::Closed);
    assert!(!gate.allowed());
    assert_eq!(gate.update(true, true, false), GateChange::Unchanged);

    assert_eq!(gate.update(false, false, false), GateChange::Unchanged);
    assert!(!gate.allowed());
    assert_eq!(gate.update(true, false, true), GateChange::Unchanged);
    assert!(!gate.allowed());

    assert_eq!(gate.update(true, false, false), GateChange::Opened);
    assert_eq!(gate.update(true, false, true), GateChange::Closed);
    assert_eq!(gate.update(false, false, true), GateChange::Unchanged);
}

#[test]
fn sessions_changed_event_describes_capsule() {
    let sessions = vec![
        summary(
            "session-controller",
            "device-controller",
            "Alice's phone",
            SessionRole::Controller,
            1_234,
        ),
        summary(
            "session-viewer-1",
            "device-viewer-1",
            "Tablet",
            SessionRole::Viewer,
            2_345,
        ),
        summary(
            "session-viewer-2",
            "device-viewer-2",
            "Laptop",
            SessionRole::Viewer,
            3_456,
        ),
    ];

    let Event::SessionsChanged(payload) = sessions_changed_event(&sessions, true) else {
        panic!("expected sessions changed event");
    };
    assert_eq!(
        payload["sessions"],
        serde_json::to_value(&sessions).unwrap()
    );
    let capsule = &payload["capsule"];
    assert_eq!(capsule["visible"], true);
    assert_eq!(capsule["controller"]["session_id"], "session-controller");
    assert_eq!(capsule["controller"]["device_id"], "device-controller");
    assert_eq!(capsule["controller"]["device_name"], "Alice's phone");
    assert_eq!(capsule["controller"]["connected_at_us"], 1_234);
    assert_eq!(capsule["viewer_count"], 2);
    assert_eq!(capsule["input_paused_local"], true);

    let Event::SessionsChanged(empty) = sessions_changed_event(&[], false) else {
        panic!("expected sessions changed event");
    };
    assert!(empty["sessions"].as_array().unwrap().is_empty());
    assert_eq!(empty["capsule"]["visible"], false);
    assert_eq!(empty["capsule"]["controller"], Value::Null);
    assert_eq!(empty["capsule"]["viewer_count"], 0);
    assert_eq!(empty["capsule"]["input_paused_local"], false);
}

#[test]
fn local_activity_message_carries_pause_flag() {
    assert_eq!(local_activity_msg(true), pb::LocalActivity { active: true });
    assert!(!local_activity_msg(false).active);
}
