use std::collections::HashMap;
use std::sync::{Arc, Mutex};

use host_core::platform::{ConsoleState, PlatformError};
use host_core::session::power::{
    console_error, console_transition, console_verdict, in_console, spawn_console_watcher,
    wake_on_stream_start, AwakeGuard, ConsoleChange, ConsoleVerdict, KeepAwake, PowerPort,
    CONSOLE_POLL,
};
use rsp_core::pb;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
enum PowerCall {
    KeepAwake(bool),
    WakeDisplay,
}

#[derive(Debug, Clone, Copy)]
enum WakeReply {
    Ok,
    Unsupported,
    Platform,
}

struct FakePort {
    calls: Mutex<Vec<PowerCall>>,
    console: Mutex<ConsoleState>,
    wake: Mutex<WakeReply>,
}

impl FakePort {
    fn new(console: ConsoleState) -> Self {
        Self {
            calls: Mutex::new(Vec::new()),
            console: Mutex::new(console),
            wake: Mutex::new(WakeReply::Ok),
        }
    }

    fn calls(&self) -> Vec<PowerCall> {
        self.calls.lock().expect("call log lock").clone()
    }

    fn set_console(&self, state: ConsoleState) {
        *self.console.lock().expect("console lock") = state;
    }

    fn set_wake(&self, reply: WakeReply) {
        *self.wake.lock().expect("wake lock") = reply;
    }
}

impl PowerPort for FakePort {
    fn keep_awake(&self, on: bool) {
        self.calls
            .lock()
            .expect("call log lock")
            .push(PowerCall::KeepAwake(on));
    }

    fn wake_display(&self) -> Result<(), PlatformError> {
        self.calls
            .lock()
            .expect("call log lock")
            .push(PowerCall::WakeDisplay);
        match *self.wake.lock().expect("wake lock") {
            WakeReply::Ok => Ok(()),
            WakeReply::Unsupported => Err(PlatformError::Unsupported),
            WakeReply::Platform => Err(PlatformError::Platform("x".to_owned())),
        }
    }

    fn console_state(&self) -> ConsoleState {
        *self.console.lock().expect("console lock")
    }
}

fn fake_port(console: ConsoleState) -> (Arc<FakePort>, Arc<dyn PowerPort>) {
    let fake = Arc::new(FakePort::new(console));
    let port: Arc<dyn PowerPort> = fake.clone();
    (fake, port)
}

#[test]
fn keep_awake_is_reference_counted() {
    fn assert_send<T: Send>() {}
    assert_send::<AwakeGuard>();

    let (fake, port) = fake_port(ConsoleState::LoggedIn);
    let keep_awake = KeepAwake::new(port);
    assert_eq!(keep_awake.active(), 0);

    let first = keep_awake.acquire();
    assert_eq!(keep_awake.active(), 1);
    assert_eq!(fake.calls(), [PowerCall::KeepAwake(true)]);

    let second = keep_awake.acquire();
    assert_eq!(keep_awake.active(), 2);
    assert_eq!(fake.calls(), [PowerCall::KeepAwake(true)]);

    drop(first);
    assert_eq!(keep_awake.active(), 1);
    assert_eq!(fake.calls(), [PowerCall::KeepAwake(true)]);

    drop(second);
    assert_eq!(keep_awake.active(), 0);
    assert_eq!(
        fake.calls(),
        [PowerCall::KeepAwake(true), PowerCall::KeepAwake(false)]
    );
}

#[test]
fn keep_awake_can_be_acquired_again_after_release() {
    let (fake, port) = fake_port(ConsoleState::LoggedIn);
    let keep_awake = KeepAwake::new(port);

    drop(keep_awake.acquire());
    let guard = keep_awake.acquire();

    assert_eq!(
        fake.calls(),
        [
            PowerCall::KeepAwake(true),
            PowerCall::KeepAwake(false),
            PowerCall::KeepAwake(true),
        ]
    );
    assert_eq!(keep_awake.active(), 1);

    drop(guard);
    assert_eq!(keep_awake.active(), 0);
}

#[tokio::test(flavor = "multi_thread", worker_threads = 4)]
async fn concurrent_acquire_and_release_calls_alternate() {
    const TASKS: usize = 8;
    const ROUNDS: usize = 100;

    let (fake, port) = fake_port(ConsoleState::LoggedIn);
    let keep_awake = KeepAwake::new(port);
    let mut tasks = Vec::new();
    for _ in 0..TASKS {
        let keep_awake = Arc::clone(&keep_awake);
        tasks.push(tokio::spawn(async move {
            for _ in 0..ROUNDS {
                let guard = keep_awake.acquire();
                tokio::task::yield_now().await;
                drop(guard);
            }
        }));
    }
    for task in tasks {
        task.await.expect("acquire/release task");
    }

    assert_eq!(keep_awake.active(), 0);
    let calls = fake.calls();
    assert!(!calls.is_empty());
    assert_eq!(calls.len() % 2, 0);
    for (index, call) in calls.iter().enumerate() {
        assert_eq!(*call, PowerCall::KeepAwake(index % 2 == 0));
    }
    assert_eq!(calls.last(), Some(&PowerCall::KeepAwake(false)));
}

#[test]
fn wake_on_stream_start_records_one_wake() {
    let (fake, port) = fake_port(ConsoleState::LoggedIn);

    wake_on_stream_start(port.as_ref());

    assert_eq!(fake.calls(), [PowerCall::WakeDisplay]);
}

#[test]
fn wake_on_stream_start_ignores_wake_failures() {
    let (fake, port) = fake_port(ConsoleState::LoggedIn);

    fake.set_wake(WakeReply::Unsupported);
    wake_on_stream_start(port.as_ref());
    fake.set_wake(WakeReply::Platform);
    wake_on_stream_start(port.as_ref());
}

#[test]
fn console_verdict_allows_logged_in_locked_and_unknown() {
    for state in [
        ConsoleState::LoggedIn,
        ConsoleState::Locked,
        ConsoleState::Unknown,
    ] {
        assert!(in_console(state));
        for screen_sharing in [None, Some(false), Some(true)] {
            assert_eq!(
                console_verdict(state, screen_sharing),
                ConsoleVerdict::Allow
            );
        }
    }
}

#[test]
fn console_verdict_denies_login_window_and_other_user_with_details() {
    assert!(!in_console(ConsoleState::LoginWindow));
    assert!(!in_console(ConsoleState::OtherUser));

    let ConsoleVerdict::Deny(error) = console_verdict(ConsoleState::LoginWindow, None) else {
        panic!("login window must be denied");
    };
    assert_eq!(error.code, 190);
    assert_eq!(error.message, "host console is not available");
    assert_eq!(error.retry_after_s, 0);
    assert_eq!(error.detail.len(), 1);
    assert_eq!(
        error.detail.get("console").map(String::as_str),
        Some("login_window")
    );
    assert!(!error.detail.contains_key("screen_sharing"));

    let ConsoleVerdict::Deny(error) = console_verdict(ConsoleState::OtherUser, Some(true)) else {
        panic!("other user must be denied");
    };
    assert_eq!(
        error.detail.get("console").map(String::as_str),
        Some("other_user")
    );
    assert_eq!(
        error.detail.get("screen_sharing").map(String::as_str),
        Some("on")
    );

    let ConsoleVerdict::Deny(error) = console_verdict(ConsoleState::OtherUser, Some(false)) else {
        panic!("other user must be denied");
    };
    assert_eq!(
        error.detail.get("console").map(String::as_str),
        Some("other_user")
    );
    assert_eq!(
        error.detail.get("screen_sharing").map(String::as_str),
        Some("off")
    );
}

#[test]
fn console_transition_table_covers_all_pairs() {
    let states = [
        ConsoleState::LoggedIn,
        ConsoleState::Locked,
        ConsoleState::Unknown,
        ConsoleState::LoginWindow,
        ConsoleState::OtherUser,
    ];
    let expected = [
        [None, None, None, Some("login_window"), Some("other_user")],
        [None, None, None, Some("login_window"), Some("other_user")],
        [None, None, None, Some("login_window"), Some("other_user")],
        [
            Some("returned"),
            Some("returned"),
            Some("returned"),
            None,
            None,
        ],
        [
            Some("returned"),
            Some("returned"),
            Some("returned"),
            None,
            None,
        ],
    ];

    for (old_index, old) in states.into_iter().enumerate() {
        for (new_index, new) in states.into_iter().enumerate() {
            let actual = console_transition(old, new);
            match expected[old_index][new_index] {
                None => assert_eq!(actual, None, "{old:?} -> {new:?}"),
                Some("returned") => {
                    assert_eq!(actual, Some(ConsoleChange::Returned), "{old:?} -> {new:?}")
                }
                Some(console) => {
                    let expected_error = pb::ErrorInfo {
                        code: 190,
                        message: "host console is not available".to_owned(),
                        retry_after_s: 0,
                        detail: HashMap::from([("console".to_owned(), console.to_owned())]),
                    };
                    assert_eq!(
                        actual,
                        Some(ConsoleChange::Left {
                            error: expected_error
                        }),
                        "{old:?} -> {new:?}"
                    );
                }
            }
        }
    }

    assert_eq!(
        console_error(ConsoleState::LoginWindow, None).detail,
        HashMap::from([("console".to_owned(), "login_window".to_owned())])
    );
}

#[tokio::test(start_paused = true)]
async fn console_watcher_publishes_changes_and_stops_with_receivers() {
    let (fake, port) = fake_port(ConsoleState::LoggedIn);
    let (mut receiver, handle) = spawn_console_watcher(port, CONSOLE_POLL);
    assert_eq!(*receiver.borrow(), ConsoleState::LoggedIn);

    tokio::task::yield_now().await;
    fake.set_console(ConsoleState::LoginWindow);
    tokio::time::advance(CONSOLE_POLL).await;
    receiver
        .changed()
        .await
        .expect("watcher should publish a change");
    assert_eq!(*receiver.borrow_and_update(), ConsoleState::LoginWindow);

    tokio::time::advance(CONSOLE_POLL).await;
    tokio::task::yield_now().await;
    assert!(!receiver.has_changed().expect("watcher sender is alive"));

    drop(receiver);
    tokio::time::advance(CONSOLE_POLL).await;
    handle
        .await
        .expect("watcher task should stop without panicking");
}
