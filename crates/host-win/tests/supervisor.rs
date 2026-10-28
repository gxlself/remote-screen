use std::time::{Duration, Instant};

use host_win::supervisor::{
    ChildState, SessionChangeKind, SupAction, SupEvent, Supervisor, SupervisorConfig, NO_SESSION,
};

fn spawning(now: Instant) -> Supervisor {
    let mut supervisor = Supervisor::new(SupervisorConfig::default());
    assert_eq!(
        supervisor.handle(SupEvent::ConsoleSession(Some(1)), now),
        vec![SupAction::Spawn { session_id: 1 }]
    );
    assert_eq!(supervisor.state(), &ChildState::Spawning { session_id: 1 });
    supervisor
}

fn starting(now: Instant) -> Supervisor {
    let mut supervisor = spawning(now);
    assert!(supervisor
        .handle(
            SupEvent::Spawned {
                session_id: 1,
                pid: 100,
            },
            now,
        )
        .is_empty());
    assert_eq!(
        supervisor.state(),
        &ChildState::Starting {
            session_id: 1,
            pid: 100,
            since: now,
        }
    );
    supervisor
}

fn running(now: Instant) -> Supervisor {
    let mut supervisor = starting(now);
    assert!(supervisor
        .handle(SupEvent::Connected { pid: 100 }, now)
        .is_empty());
    assert_eq!(
        supervisor.state(),
        &ChildState::Running {
            session_id: 1,
            pid: 100,
            since: now,
        }
    );
    supervisor
}

fn spawn_failed(session_id: u32) -> SupEvent {
    SupEvent::SpawnFailed {
        session_id,
        error: "process creation failed".to_owned(),
    }
}

#[test]
fn defaults_and_initial_state() {
    let cfg = SupervisorConfig::default();
    assert_eq!(cfg.connect_timeout, Duration::from_secs(10));
    assert_eq!(cfg.backoff_initial, Duration::from_secs(1));
    assert_eq!(cfg.backoff_max, Duration::from_secs(30));
    assert_eq!(cfg.stable_after, Duration::from_secs(60));
    assert_eq!(NO_SESSION, u32::MAX);

    let supervisor = Supervisor::new(cfg);
    assert_eq!(supervisor.state(), &ChildState::Idle);
    assert_eq!(supervisor.target(), None);
    assert_eq!(supervisor.backoff(), Duration::from_secs(1));
    assert_eq!(supervisor.next_deadline(), None);
}

#[test]
fn startup_uses_spawn_and_handshake_times() {
    let t0 = Instant::now();
    let mut supervisor = spawning(t0);
    assert_eq!(supervisor.target(), Some(1));
    assert_eq!(supervisor.next_deadline(), None);

    let spawned_at = t0 + Duration::from_secs(2);
    assert!(supervisor
        .handle(
            SupEvent::Spawned {
                session_id: 1,
                pid: 100,
            },
            spawned_at,
        )
        .is_empty());
    assert_eq!(
        supervisor.state(),
        &ChildState::Starting {
            session_id: 1,
            pid: 100,
            since: spawned_at,
        }
    );
    assert_eq!(
        supervisor.next_deadline(),
        Some(spawned_at + Duration::from_secs(10))
    );

    let connected_at = t0 + Duration::from_secs(3);
    assert!(supervisor
        .handle(SupEvent::Connected { pid: 100 }, connected_at)
        .is_empty());
    assert_eq!(
        supervisor.state(),
        &ChildState::Running {
            session_id: 1,
            pid: 100,
            since: connected_at,
        }
    );
    assert_eq!(supervisor.next_deadline(), None);
}

#[test]
fn switching_sessions_kills_before_spawning_and_ignores_old_exit() {
    let t0 = Instant::now();
    for mut supervisor in [starting(t0), running(t0)] {
        let now = t0 + Duration::from_secs(1);
        assert_eq!(
            supervisor.handle(SupEvent::ConsoleSession(Some(2)), now),
            vec![
                SupAction::Kill { pid: 100 },
                SupAction::Spawn { session_id: 2 }
            ]
        );
        assert_eq!(supervisor.target(), Some(2));
        assert_eq!(supervisor.state(), &ChildState::Spawning { session_id: 2 });
        assert!(supervisor
            .handle(SupEvent::Exited { pid: 100, code: 0 }, now)
            .is_empty());
        assert_eq!(supervisor.state(), &ChildState::Spawning { session_id: 2 });
        assert_eq!(supervisor.backoff(), Duration::from_secs(1));
    }
}

#[test]
fn switching_during_spawn_kills_stale_result_and_ignores_stale_failure() {
    let t0 = Instant::now();
    let mut supervisor = spawning(t0);
    assert_eq!(
        supervisor.handle(SupEvent::ConsoleSession(Some(2)), t0),
        vec![SupAction::Spawn { session_id: 2 }]
    );
    assert_eq!(
        supervisor.handle(
            SupEvent::Spawned {
                session_id: 1,
                pid: 101,
            },
            t0,
        ),
        vec![SupAction::Kill { pid: 101 }]
    );
    assert!(supervisor.handle(spawn_failed(1), t0).is_empty());
    assert_eq!(supervisor.state(), &ChildState::Spawning { session_id: 2 });
    assert_eq!(supervisor.backoff(), Duration::from_secs(1));
}

#[test]
fn no_session_kills_a_live_child_and_cancels_deadlines() {
    let t0 = Instant::now();
    for session_id in [None, Some(NO_SESSION)] {
        for mut supervisor in [starting(t0), running(t0)] {
            assert_eq!(
                supervisor.handle(SupEvent::ConsoleSession(session_id), t0),
                vec![SupAction::Kill { pid: 100 }]
            );
            assert_eq!(supervisor.state(), &ChildState::Idle);
            assert_eq!(supervisor.target(), None);
            assert_eq!(supervisor.next_deadline(), None);
            assert!(supervisor
                .handle(SupEvent::Exited { pid: 100, code: 0 }, t0)
                .is_empty());
            assert!(supervisor
                .handle(SupEvent::Tick, t0 + Duration::from_secs(100))
                .is_empty());
            assert_eq!(supervisor.state(), &ChildState::Idle);
        }
    }
}

#[test]
fn no_session_cancels_pending_spawn_and_backoff() {
    let t0 = Instant::now();
    for session_id in [None, Some(NO_SESSION)] {
        let mut backoff = spawning(t0);
        assert!(backoff.handle(spawn_failed(1), t0).is_empty());
        for mut supervisor in [spawning(t0), backoff] {
            assert!(supervisor
                .handle(SupEvent::ConsoleSession(session_id), t0)
                .is_empty());
            assert_eq!(supervisor.target(), None);
            assert_eq!(supervisor.next_deadline(), None);
            assert_eq!(
                supervisor.handle(
                    SupEvent::Spawned {
                        session_id: 1,
                        pid: 101,
                    },
                    t0,
                ),
                vec![SupAction::Kill { pid: 101 }]
            );
            assert!(supervisor.handle(spawn_failed(1), t0).is_empty());
            assert!(supervisor
                .handle(SupEvent::Tick, t0 + Duration::from_secs(100))
                .is_empty());
            assert_eq!(supervisor.state(), &ChildState::Idle);
            assert_eq!(
                supervisor.handle(
                    SupEvent::ConsoleSession(Some(1)),
                    t0 + Duration::from_secs(101),
                ),
                vec![SupAction::Spawn { session_id: 1 }]
            );
            assert_eq!(supervisor.backoff(), Duration::from_secs(1));
        }
    }
}

#[test]
fn repeated_console_poll_preserves_child_and_backoff() {
    let t0 = Instant::now();
    let mut backoff = spawning(t0);
    assert!(backoff.handle(spawn_failed(1), t0).is_empty());
    for mut supervisor in [spawning(t0), starting(t0), running(t0), backoff] {
        let state = supervisor.state().clone();
        let delay = supervisor.backoff();
        assert!(supervisor
            .handle(SupEvent::ConsoleSession(Some(1)), t0)
            .is_empty());
        assert_eq!(supervisor.state(), &state);
        assert_eq!(supervisor.backoff(), delay);
    }
}

#[test]
fn crashes_double_delay_and_cap_it_at_thirty_seconds() {
    let t0 = Instant::now();
    let mut supervisor = running(t0);
    let mut now = t0 + Duration::from_secs(5);

    for (pid, seconds) in (100..).zip([1, 2, 4, 8, 16, 30, 30]) {
        assert!(supervisor
            .handle(SupEvent::Exited { pid, code: 1 }, now)
            .is_empty());
        let until = now + Duration::from_secs(seconds);
        assert_eq!(supervisor.state(), &ChildState::Backoff { until });
        assert_eq!(supervisor.next_deadline(), Some(until));
        assert_eq!(
            supervisor.backoff(),
            Duration::from_secs((seconds * 2).min(30))
        );
        assert!(supervisor
            .handle(SupEvent::Tick, until - Duration::from_millis(500))
            .is_empty());
        assert_eq!(supervisor.state(), &ChildState::Backoff { until });
        assert_eq!(
            supervisor.handle(SupEvent::Tick, until),
            vec![SupAction::Spawn { session_id: 1 }]
        );
        assert_eq!(supervisor.state(), &ChildState::Spawning { session_id: 1 });
        assert_eq!(supervisor.next_deadline(), None);
        assert!(supervisor.handle(SupEvent::Tick, until).is_empty());

        now = until;
        let next_pid = pid + 1;
        assert!(supervisor
            .handle(
                SupEvent::Spawned {
                    session_id: 1,
                    pid: next_pid,
                },
                now,
            )
            .is_empty());
        assert!(supervisor
            .handle(SupEvent::Connected { pid: next_pid }, now)
            .is_empty());
    }
}

#[test]
fn stable_runtime_resets_backoff_at_the_exact_threshold() {
    let t0 = Instant::now();
    for (runtime, expected_delay) in [
        (Duration::from_millis(59_999), Duration::from_secs(8)),
        (Duration::from_secs(60), Duration::from_secs(1)),
        (Duration::from_secs(61), Duration::from_secs(1)),
    ] {
        let mut supervisor = spawning(t0);
        let mut now = t0;
        for seconds in [1, 2, 4] {
            assert!(supervisor.handle(spawn_failed(1), now).is_empty());
            now += Duration::from_secs(seconds);
            assert_eq!(
                supervisor.handle(SupEvent::Tick, now),
                vec![SupAction::Spawn { session_id: 1 }]
            );
        }
        assert_eq!(supervisor.backoff(), Duration::from_secs(8));
        assert!(supervisor
            .handle(
                SupEvent::Spawned {
                    session_id: 1,
                    pid: 100,
                },
                now,
            )
            .is_empty());
        // Time spent waiting for Hello must not count toward stable runtime.
        now += Duration::from_secs(9);
        assert!(supervisor
            .handle(SupEvent::Connected { pid: 100 }, now)
            .is_empty());
        now += runtime;
        assert!(supervisor
            .handle(SupEvent::Exited { pid: 100, code: 1 }, now)
            .is_empty());
        assert_eq!(
            supervisor.state(),
            &ChildState::Backoff {
                until: now + expected_delay,
            }
        );
        assert_eq!(supervisor.backoff(), expected_delay * 2);
    }
}

#[test]
fn connection_timeout_kills_once_and_ignores_late_child_events() {
    let t0 = Instant::now();
    let mut supervisor = starting(t0);
    let starting_state = supervisor.state().clone();
    assert!(supervisor
        .handle(SupEvent::Tick, t0 + Duration::from_millis(9_900))
        .is_empty());
    assert_eq!(supervisor.state(), &starting_state);

    let now = t0 + Duration::from_secs(10);
    assert_eq!(
        supervisor.handle(SupEvent::Tick, now),
        vec![SupAction::Kill { pid: 100 }]
    );
    let until = now + Duration::from_secs(1);
    assert_eq!(supervisor.state(), &ChildState::Backoff { until });
    assert_eq!(supervisor.next_deadline(), Some(until));
    assert_eq!(supervisor.backoff(), Duration::from_secs(2));
    for event in [
        SupEvent::Exited { pid: 100, code: 1 },
        SupEvent::Connected { pid: 100 },
        SupEvent::Tick,
    ] {
        assert!(supervisor.handle(event, now).is_empty());
        assert_eq!(supervisor.state(), &ChildState::Backoff { until });
        assert_eq!(supervisor.backoff(), Duration::from_secs(2));
    }
    assert_eq!(
        supervisor.handle(SupEvent::Tick, until),
        vec![SupAction::Spawn { session_id: 1 }]
    );
}

#[test]
fn spawn_failure_schedules_retry_and_doubles_next_delay() {
    let t0 = Instant::now();
    let mut supervisor = spawning(t0);
    let failed_at = t0 + Duration::from_secs(2);
    assert!(supervisor.handle(spawn_failed(1), failed_at).is_empty());
    let until = failed_at + Duration::from_secs(1);
    assert_eq!(supervisor.state(), &ChildState::Backoff { until });
    assert_eq!(supervisor.next_deadline(), Some(until));
    assert_eq!(supervisor.backoff(), Duration::from_secs(2));
    assert!(supervisor.handle(spawn_failed(1), failed_at).is_empty());
    assert_eq!(supervisor.state(), &ChildState::Backoff { until });
    assert_eq!(supervisor.backoff(), Duration::from_secs(2));
    assert_eq!(
        supervisor.handle(SupEvent::Tick, until + Duration::from_millis(1)),
        vec![SupAction::Spawn { session_id: 1 }]
    );
}

#[test]
fn exit_before_handshake_uses_backoff() {
    let t0 = Instant::now();
    let mut supervisor = starting(t0);
    assert!(supervisor
        .handle(SupEvent::Exited { pid: 100, code: 1 }, t0)
        .is_empty());
    assert_eq!(
        supervisor.state(),
        &ChildState::Backoff {
            until: t0 + Duration::from_secs(1),
        }
    );
    assert_eq!(supervisor.backoff(), Duration::from_secs(2));
}

#[test]
fn switching_sessions_bypasses_and_resets_backoff() {
    let t0 = Instant::now();
    let mut supervisor = spawning(t0);
    assert!(supervisor.handle(spawn_failed(1), t0).is_empty());
    assert_eq!(supervisor.backoff(), Duration::from_secs(2));
    assert_eq!(
        supervisor.handle(SupEvent::ConsoleSession(Some(2)), t0),
        vec![SupAction::Spawn { session_id: 2 }]
    );
    assert_eq!(supervisor.target(), Some(2));
    assert_eq!(supervisor.state(), &ChildState::Spawning { session_id: 2 });
    assert_eq!(supervisor.backoff(), Duration::from_secs(1));
    assert_eq!(supervisor.next_deadline(), None);
}

#[test]
fn logon_and_logoff_rebuild_current_live_session() {
    let t0 = Instant::now();
    for kind in [SessionChangeKind::Logon, SessionChangeKind::Logoff] {
        for mut supervisor in [starting(t0), running(t0)] {
            assert_eq!(
                supervisor.handle(
                    SupEvent::SessionChange {
                        kind,
                        session_id: 1,
                    },
                    t0,
                ),
                vec![
                    SupAction::Kill { pid: 100 },
                    SupAction::Spawn { session_id: 1 }
                ]
            );
            assert_eq!(supervisor.state(), &ChildState::Spawning { session_id: 1 });
            assert_eq!(supervisor.target(), Some(1));
            assert_eq!(supervisor.backoff(), Duration::from_secs(1));
        }
    }
}

#[test]
fn session_rebuild_resets_delay_after_an_earlier_failure() {
    let t0 = Instant::now();
    for (event, session_id) in [
        (SupEvent::ConsoleSession(Some(2)), 2),
        (
            SupEvent::SessionChange {
                kind: SessionChangeKind::Logon,
                session_id: 1,
            },
            1,
        ),
        (
            SupEvent::SessionChange {
                kind: SessionChangeKind::Logoff,
                session_id: 1,
            },
            1,
        ),
    ] {
        let mut supervisor = spawning(t0);
        assert!(supervisor.handle(spawn_failed(1), t0).is_empty());
        let now = t0 + Duration::from_secs(1);
        assert_eq!(
            supervisor.handle(SupEvent::Tick, now),
            vec![SupAction::Spawn { session_id: 1 }]
        );
        assert!(supervisor
            .handle(
                SupEvent::Spawned {
                    session_id: 1,
                    pid: 100,
                },
                now,
            )
            .is_empty());
        assert!(supervisor
            .handle(SupEvent::Connected { pid: 100 }, now)
            .is_empty());
        assert_eq!(supervisor.backoff(), Duration::from_secs(2));
        assert_eq!(
            supervisor.handle(event, now),
            vec![
                SupAction::Kill { pid: 100 },
                SupAction::Spawn { session_id }
            ]
        );
        assert_eq!(supervisor.state(), &ChildState::Spawning { session_id });
        assert_eq!(supervisor.backoff(), Duration::from_secs(1));
    }
}

#[test]
fn logon_and_logoff_without_live_child_preserve_pending_work() {
    let t0 = Instant::now();
    let mut backoff = spawning(t0);
    assert!(backoff.handle(spawn_failed(1), t0).is_empty());
    for mut supervisor in [
        Supervisor::new(SupervisorConfig::default()),
        spawning(t0),
        backoff,
    ] {
        let state = supervisor.state().clone();
        let delay = supervisor.backoff();
        for kind in [SessionChangeKind::Logon, SessionChangeKind::Logoff] {
            assert!(supervisor
                .handle(
                    SupEvent::SessionChange {
                        kind,
                        session_id: 1,
                    },
                    t0,
                )
                .is_empty());
            assert_eq!(supervisor.state(), &state);
            assert_eq!(supervisor.backoff(), delay);
        }
    }
}

#[test]
fn custom_configuration_controls_deadlines_cap_and_stability() {
    let t0 = Instant::now();
    let cfg = SupervisorConfig {
        connect_timeout: Duration::from_millis(250),
        backoff_initial: Duration::from_millis(100),
        backoff_max: Duration::from_millis(350),
        stable_after: Duration::from_millis(500),
    };
    let mut supervisor = Supervisor::new(cfg);
    assert_eq!(
        supervisor.handle(SupEvent::ConsoleSession(Some(1)), t0),
        vec![SupAction::Spawn { session_id: 1 }]
    );
    assert!(supervisor
        .handle(
            SupEvent::Spawned {
                session_id: 1,
                pid: 100,
            },
            t0,
        )
        .is_empty());
    let mut now = t0 + cfg.connect_timeout;
    assert_eq!(supervisor.next_deadline(), Some(now));
    assert_eq!(
        supervisor.handle(SupEvent::Tick, now),
        vec![SupAction::Kill { pid: 100 }]
    );
    assert_eq!(supervisor.backoff(), Duration::from_millis(200));
    for delay in [100, 200, 350] {
        now += Duration::from_millis(delay);
        assert_eq!(supervisor.next_deadline(), Some(now));
        assert_eq!(
            supervisor.handle(SupEvent::Tick, now),
            vec![SupAction::Spawn { session_id: 1 }]
        );
        if delay != 350 {
            assert!(supervisor.handle(spawn_failed(1), now).is_empty());
            assert_eq!(supervisor.backoff(), cfg.backoff_max);
        }
    }
    assert!(supervisor
        .handle(
            SupEvent::Spawned {
                session_id: 1,
                pid: 101,
            },
            now,
        )
        .is_empty());
    assert!(supervisor
        .handle(SupEvent::Connected { pid: 101 }, now)
        .is_empty());
    now += cfg.stable_after;
    assert!(supervisor
        .handle(SupEvent::Exited { pid: 101, code: 1 }, now)
        .is_empty());
    assert_eq!(supervisor.next_deadline(), Some(now + cfg.backoff_initial));
    assert_eq!(supervisor.backoff(), Duration::from_millis(200));
}

#[test]
fn unrelated_session_notifications_preserve_live_child() {
    let t0 = Instant::now();
    let mut supervisor = running(t0);
    let state = supervisor.state().clone();
    for kind in [
        SessionChangeKind::ConsoleDisconnect,
        SessionChangeKind::RemoteConnect,
        SessionChangeKind::RemoteDisconnect,
        SessionChangeKind::Lock,
        SessionChangeKind::Unlock,
        SessionChangeKind::Other(0),
        SessionChangeKind::Other(9),
    ] {
        for session_id in [1, 2] {
            assert!(supervisor
                .handle(SupEvent::SessionChange { kind, session_id }, t0)
                .is_empty());
            assert_eq!(supervisor.state(), &state);
            assert_eq!(supervisor.target(), Some(1));
        }
    }
    for kind in [SessionChangeKind::Logon, SessionChangeKind::Logoff] {
        assert!(supervisor
            .handle(
                SupEvent::SessionChange {
                    kind,
                    session_id: 2,
                },
                t0,
            )
            .is_empty());
        assert_eq!(supervisor.state(), &state);
    }
}

#[test]
fn console_connect_selects_a_new_session_and_normalizes_no_session() {
    let t0 = Instant::now();
    for (session_id, target, actions, state) in [
        (
            3,
            Some(3),
            vec![
                SupAction::Kill { pid: 100 },
                SupAction::Spawn { session_id: 3 },
            ],
            ChildState::Spawning { session_id: 3 },
        ),
        (
            NO_SESSION,
            None,
            vec![SupAction::Kill { pid: 100 }],
            ChildState::Idle,
        ),
    ] {
        let mut supervisor = running(t0);
        assert_eq!(
            supervisor.handle(
                SupEvent::SessionChange {
                    kind: SessionChangeKind::ConsoleConnect,
                    session_id,
                },
                t0,
            ),
            actions
        );
        assert_eq!(supervisor.state(), &state);
        assert_eq!(supervisor.target(), target);
    }
}

#[test]
fn unknown_pids_and_unexpected_failures_are_ignored() {
    let t0 = Instant::now();
    let mut backoff = spawning(t0);
    assert!(backoff.handle(spawn_failed(1), t0).is_empty());
    for mut supervisor in [
        Supervisor::new(SupervisorConfig::default()),
        spawning(t0),
        starting(t0),
        running(t0),
        backoff,
    ] {
        let state = supervisor.state().clone();
        let delay = supervisor.backoff();
        for event in [
            SupEvent::Connected { pid: 999 },
            SupEvent::Exited { pid: 999, code: 1 },
            spawn_failed(2),
        ] {
            assert!(supervisor.handle(event, t0).is_empty());
            assert_eq!(supervisor.state(), &state);
            assert_eq!(supervisor.backoff(), delay);
        }
    }
}

#[test]
fn repeated_handshake_and_ticks_do_not_change_running_since() {
    let t0 = Instant::now();
    let mut supervisor = running(t0);
    let state = supervisor.state().clone();
    for event in [SupEvent::Connected { pid: 100 }, SupEvent::Tick] {
        assert!(supervisor
            .handle(event, t0 + Duration::from_secs(120))
            .is_empty());
        assert_eq!(supervisor.state(), &state);
        assert_eq!(supervisor.next_deadline(), None);
    }
}

#[test]
fn unexpected_spawn_result_is_killed_without_replacing_child() {
    let t0 = Instant::now();
    for mut supervisor in [starting(t0), running(t0)] {
        let state = supervisor.state().clone();
        assert_eq!(
            supervisor.handle(
                SupEvent::Spawned {
                    session_id: 1,
                    pid: 101,
                },
                t0,
            ),
            vec![SupAction::Kill { pid: 101 }]
        );
        assert!(supervisor.handle(spawn_failed(1), t0).is_empty());
        assert_eq!(supervisor.state(), &state);
    }
}

#[test]
fn shutdown_is_terminal_from_every_state() {
    let t0 = Instant::now();
    let mut backoff = spawning(t0);
    assert!(backoff.handle(spawn_failed(1), t0).is_empty());
    for (mut supervisor, actions) in [
        (Supervisor::new(SupervisorConfig::default()), vec![]),
        (spawning(t0), vec![]),
        (backoff, vec![]),
        (starting(t0), vec![SupAction::Kill { pid: 100 }]),
        (running(t0), vec![SupAction::Kill { pid: 100 }]),
    ] {
        assert_eq!(supervisor.handle(SupEvent::Shutdown, t0), actions);
        assert_eq!(supervisor.state(), &ChildState::Stopped);
        assert_eq!(supervisor.next_deadline(), None);
        for event in [
            SupEvent::ConsoleSession(Some(5)),
            SupEvent::ConsoleSession(None),
            SupEvent::SessionChange {
                kind: SessionChangeKind::ConsoleConnect,
                session_id: 5,
            },
            SupEvent::Spawned {
                session_id: 1,
                pid: 101,
            },
            spawn_failed(1),
            SupEvent::Connected { pid: 100 },
            SupEvent::Exited { pid: 100, code: 0 },
            SupEvent::Tick,
            SupEvent::Shutdown,
        ] {
            assert!(supervisor
                .handle(event, t0 + Duration::from_secs(100))
                .is_empty());
            assert_eq!(supervisor.state(), &ChildState::Stopped);
            assert_eq!(supervisor.next_deadline(), None);
        }
    }
}

#[test]
fn wparam_maps_all_documented_values_and_preserves_unknown_values() {
    for (w, kind) in [
        (1, SessionChangeKind::ConsoleConnect),
        (2, SessionChangeKind::ConsoleDisconnect),
        (3, SessionChangeKind::RemoteConnect),
        (4, SessionChangeKind::RemoteDisconnect),
        (5, SessionChangeKind::Logon),
        (6, SessionChangeKind::Logoff),
        (7, SessionChangeKind::Lock),
        (8, SessionChangeKind::Unlock),
        (9, SessionChangeKind::Other(9)),
        (0, SessionChangeKind::Other(0)),
        (u32::MAX, SessionChangeKind::Other(u32::MAX)),
    ] {
        assert_eq!(SessionChangeKind::from_wparam(w), kind);
    }
}
