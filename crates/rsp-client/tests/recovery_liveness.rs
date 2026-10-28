//! Acceptance tests for the pure liveness and backup state machines.

use std::time::Duration;

use rsp_client::recovery::liveness::{
    is_distinct, BackupAction, BackupKeeper, BackupState, Liveness, MonitorConfig, MonitorPoll,
    PathMonitor, PathSig, ACQUIRE_TIMEOUT, BACKUP_RETRY, DEAD_AFTER, HARD_DEAD_AFTER,
    MAX_OUTSTANDING_PINGS, PROBE_AFTER, PROBE_EVERY,
};
use rsp_transport::{dial::PathKind, TransportKind};
use tokio::time::Instant;

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn monitor(t0: Instant) -> PathMonitor {
    PathMonitor::new(MonitorConfig::default(), t0)
}

fn signal(path: PathKind, transport: TransportKind) -> PathSig {
    PathSig::new(&path, transport)
}

fn tailnet_quic() -> PathSig {
    signal(PathKind::Tailnet, TransportKind::Quic)
}

fn tailnet_tcp() -> PathSig {
    signal(PathKind::Tailnet, TransportKind::Tcp)
}

fn lan_quic() -> PathSig {
    signal(PathKind::Lan, TransportKind::Quic)
}

#[test]
fn continuous_video_never_probes_or_dies() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);
    for elapsed_ms in (0..=10_000).step_by(16).skip(1) {
        let now = t0 + ms(elapsed_ms);
        monitor.on_inbound(now);
        assert_eq!(
            monitor.poll(now),
            MonitorPoll {
                liveness: Liveness::Alive,
                send_probe: false,
            }
        );
    }
}

#[test]
fn static_picture_with_pongs_stays_alive() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);

    for elapsed_ms in (0..=60_000).step_by(10) {
        let now = t0 + ms(elapsed_ms);
        if elapsed_ms > 0 && elapsed_ms % 1000 == 0 {
            monitor.on_ping_sent(elapsed_ms / 1000, now);
        }
        if elapsed_ms > 50 && elapsed_ms % 1000 == 50 {
            monitor.on_pong(elapsed_ms / 1000, now);
        }
        assert!(!matches!(monitor.poll(now).liveness, Liveness::Dead { .. }));
    }
}

#[test]
fn dead_at_one_and_a_half_seconds_after_last_inbound() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);

    assert_eq!(monitor.poll(t0 + ms(499)).liveness, Liveness::Alive);
    let suspect = monitor.poll(t0 + ms(500));
    assert_eq!(suspect.liveness, Liveness::Suspect);
    assert!(suspect.send_probe);

    monitor.on_ping_sent(1, t0 + ms(500));
    assert!(!monitor.poll(t0 + ms(600)).send_probe);
    assert_eq!(monitor.poll(t0 + ms(749)).liveness, Liveness::Suspect);
    assert!(monitor.poll(t0 + ms(750)).send_probe);
    assert_eq!(monitor.poll(t0 + ms(1499)).liveness, Liveness::Suspect);
    assert_eq!(
        monitor.poll(t0 + ms(1500)).liveness,
        Liveness::Dead {
            silent_for: ms(1500)
        }
    );
}

#[test]
fn pong_at_1400ms_restarts_silence_clock() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);
    monitor.poll(t0 + ms(500));
    monitor.on_ping_sent(1, t0 + ms(500));

    monitor.on_pong(1, t0 + ms(1400));
    assert_eq!(monitor.poll(t0 + ms(1400)).liveness, Liveness::Alive);
    assert_eq!(monitor.poll(t0 + ms(1899)).liveness, Liveness::Alive);
    assert_eq!(monitor.poll(t0 + ms(1900)).liveness, Liveness::Suspect);
}

#[test]
fn no_ping_falls_back_to_hard_deadline() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);

    assert_eq!(monitor.poll(t0 + ms(1500)).liveness, Liveness::Suspect);
    assert_eq!(
        monitor.poll(t0 + HARD_DEAD_AFTER).liveness,
        Liveness::Dead {
            silent_for: HARD_DEAD_AFTER
        }
    );
}

#[test]
fn dead_is_latched_until_reset() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);
    monitor.on_ping_sent(1, t0);
    monitor.poll(t0 + ms(1500));

    monitor.on_inbound(t0 + ms(1600));
    assert!(matches!(
        monitor.poll(t0 + ms(1600)).liveness,
        Liveness::Dead { .. }
    ));

    monitor.reset(t0 + ms(2000));
    assert_eq!(monitor.poll(t0 + ms(2000)).liveness, Liveness::Alive);
}

#[test]
fn suspend_disables_monitoring_and_resume_restarts_clock() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);
    monitor.suspend();

    assert_eq!(
        monitor.poll(t0 + Duration::from_secs(10)),
        MonitorPoll {
            liveness: Liveness::Alive,
            send_probe: false,
        }
    );
    assert_eq!(monitor.next_deadline(), None);
    assert!(monitor.is_suspended());

    let resumed = t0 + Duration::from_secs(10);
    monitor.resume(resumed);
    assert!(!monitor.is_suspended());
    assert_eq!(monitor.poll(resumed + ms(500)).liveness, Liveness::Suspect);
    assert!(monitor.poll(resumed + ms(500)).send_probe);
    monitor.on_ping_sent(1, resumed + ms(500));
    assert_eq!(
        monitor.poll(resumed + ms(1500)).liveness,
        Liveness::Dead {
            silent_for: ms(1500)
        }
    );
}

#[test]
fn pong_clears_matching_and_older_pings() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);
    monitor.on_ping_sent(1, t0);
    monitor.on_ping_sent(2, t0);
    monitor.on_ping_sent(3, t0);
    monitor.on_pong(2, t0 + ms(1));
    assert_eq!(monitor.outstanding_pings(), 1);

    for t1 in 4..=43 {
        monitor.on_ping_sent(t1, t0 + ms(2));
    }
    assert_eq!(monitor.outstanding_pings(), MAX_OUTSTANDING_PINGS);
}

#[test]
fn next_deadline_tracks_probe_and_death() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);
    assert_eq!(monitor.next_deadline(), Some(t0 + PROBE_AFTER));

    monitor.on_ping_sent(0, t0 + ms(200));
    assert_eq!(monitor.next_deadline(), Some(t0 + PROBE_AFTER));

    let t0 = Instant::now();
    let mut monitor = PathMonitor::new(MonitorConfig::default(), t0);
    monitor.on_ping_sent(1, t0 + PROBE_AFTER);
    assert_eq!(
        monitor.next_deadline(),
        Some((t0 + PROBE_AFTER + PROBE_EVERY).min(t0 + DEAD_AFTER))
    );
    monitor.poll(t0 + DEAD_AFTER);
    assert_eq!(monitor.next_deadline(), None);
}

#[test]
fn clock_going_backwards_is_treated_as_zero_elapsed() {
    let t0 = Instant::now();
    let mut monitor = monitor(t0);
    monitor.poll(t0);
    monitor.on_inbound(t0 + Duration::from_secs(1));
    assert_eq!(
        monitor.poll(t0 + ms(500)),
        MonitorPoll {
            liveness: Liveness::Alive,
            send_probe: false,
        }
    );
}

#[test]
fn path_signatures_compare_original_path_types() {
    assert!(is_distinct(&tailnet_quic(), &tailnet_tcp()));
    assert!(!is_distinct(
        &signal(PathKind::Lan, TransportKind::Quic),
        &signal(
            PathKind::LastGood(Box::new(PathKind::Lan)),
            TransportKind::Quic
        )
    ));
    assert!(is_distinct(
        &lan_quic(),
        &signal(PathKind::Intranet, TransportKind::Quic)
    ));

    let cached = PathSig::new(
        &PathKind::LastGood(Box::new(PathKind::Lan)),
        TransportKind::Quic,
    );
    assert_eq!(cached.path, PathKind::Lan);
}

#[test]
fn missing_backup_uses_documented_retry_schedule() {
    let t0 = Instant::now();
    let primary = tailnet_quic();
    let mut keeper = BackupKeeper::new(primary.clone(), None, t0);

    assert_eq!(keeper.poll(t0 + ms(4999)), None);
    assert_eq!(
        keeper.poll(t0 + ms(5000)),
        Some(BackupAction::Acquire {
            exclude: primary.clone()
        })
    );
    assert!(matches!(
        keeper.state(),
        BackupState::Acquiring { failures: 0, .. }
    ));

    let expected_retries = [10, 20, 30, 30];
    let mut failed_at = t0 + ms(5000);
    for retry_secs in expected_retries {
        keeper.on_acquire_failed(failed_at);
        let retry_at = failed_at + Duration::from_secs(retry_secs);
        assert_eq!(keeper.poll(retry_at - ms(1)), None);
        assert_eq!(
            keeper.poll(retry_at),
            Some(BackupAction::Acquire {
                exclude: primary.clone()
            })
        );
        failed_at = retry_at;
    }
}

#[test]
fn ready_backup_must_be_distinct() {
    let t0 = Instant::now();
    let mut keeper = BackupKeeper::new(tailnet_quic(), None, t0);
    assert!(!keeper.on_backup_ready(tailnet_quic(), t0));
    assert!(!keeper.has_backup());

    assert!(keeper.on_backup_ready(tailnet_tcp(), t0 + ms(1000)));
    assert!(keeper.has_backup());
    assert!(matches!(
        keeper.state(),
        BackupState::Held(sig) if *sig == tailnet_tcp()
    ));

    keeper.on_backup_lost(t0 + ms(2000));
    assert!(matches!(
        keeper.state(),
        BackupState::Missing { failures: 0, .. }
    ));
    assert_eq!(
        keeper.poll(t0 + ms(2000)),
        Some(BackupAction::Acquire {
            exclude: tailnet_quic()
        })
    );
    assert!(matches!(
        keeper.state(),
        BackupState::Acquiring { failures: 0, .. }
    ));

    assert!(!keeper.on_backup_ready(tailnet_quic(), t0 + ms(7000)));
    assert!(matches!(
        keeper.state(),
        BackupState::Missing { failures: 1, .. }
    ));
}

#[test]
fn backup_loss_acquires_immediately_and_failover_consumes_it() {
    let t0 = Instant::now();
    let primary = tailnet_quic();
    let mut keeper = BackupKeeper::new(primary.clone(), Some(lan_quic()), t0);
    keeper.on_backup_lost(t0 + ms(100));
    assert_eq!(
        keeper.poll(t0 + ms(100)),
        Some(BackupAction::Acquire {
            exclude: primary.clone()
        })
    );
    keeper.on_acquire_failed(t0 + ms(100));

    let mut keeper = BackupKeeper::new(primary.clone(), Some(lan_quic()), t0);
    assert_eq!(keeper.take_for_failover(t0 + ms(100)), Some(lan_quic()));
    assert!(matches!(
        keeper.state(),
        BackupState::Missing { next_try, failures: 0 } if *next_try == t0 + ms(5100)
    ));
    assert_eq!(
        keeper.poll(t0 + ms(5100)),
        Some(BackupAction::Acquire { exclude: primary })
    );
    assert_eq!(keeper.take_for_failover(t0 + ms(6000)), None);
}

#[test]
fn changing_primary_drops_colliding_backup() {
    let t0 = Instant::now();
    let mut keeper = BackupKeeper::new(tailnet_quic(), Some(lan_quic()), t0);
    assert!(keeper.set_primary(lan_quic(), t0 + ms(100)));
    assert!(matches!(
        keeper.state(),
        BackupState::Missing { next_try, failures: 0 } if *next_try == t0 + ms(100)
    ));

    let mut keeper = BackupKeeper::new(tailnet_quic(), Some(lan_quic()), t0);
    assert!(!keeper.set_primary(
        signal(PathKind::Intranet, TransportKind::Quic),
        t0 + ms(100)
    ));
    assert!(keeper.has_backup());
}

#[test]
fn acquire_timeout_and_suspension() {
    let t0 = Instant::now();
    let mut keeper = BackupKeeper::new(tailnet_quic(), None, t0);
    keeper.poll(t0 + BACKUP_RETRY[0]);
    assert_eq!(
        keeper.poll(t0 + BACKUP_RETRY[0] + ACQUIRE_TIMEOUT - ms(1)),
        None
    );
    assert!(matches!(
        keeper.state(),
        BackupState::Acquiring { failures: 0, .. }
    ));
    keeper.poll(t0 + BACKUP_RETRY[0] + ACQUIRE_TIMEOUT);
    assert!(matches!(
        keeper.state(),
        BackupState::Missing { failures: 1, .. }
    ));
    assert_eq!(
        keeper.next_deadline(),
        Some(t0 + ms(5000) + ms(10000) + ms(10000))
    );

    keeper.suspend();
    assert_eq!(keeper.poll(t0 + Duration::from_secs(100)), None);
    assert_eq!(keeper.next_deadline(), None);

    keeper.resume(t0 + Duration::from_secs(101));
    assert_eq!(
        keeper.poll(t0 + Duration::from_secs(101)),
        Some(BackupAction::Acquire {
            exclude: tailnet_quic()
        })
    );
}

#[test]
fn held_backup_survives_suspend_and_resume() {
    let t0 = Instant::now();
    let mut keeper = BackupKeeper::new(tailnet_quic(), Some(lan_quic()), t0);
    keeper.suspend();
    assert!(matches!(keeper.state(), BackupState::Suspended));
    assert_eq!(keeper.poll(t0 + Duration::from_secs(10)), None);

    keeper.resume(t0 + Duration::from_secs(10));
    assert!(keeper.has_backup());
    assert!(matches!(keeper.state(), BackupState::Held(sig) if *sig == lan_quic()));
}
