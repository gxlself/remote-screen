use std::time::Duration;

use rsp_client::recovery::upgrade::{
    UpgradeAction, UpgradeState, Upgrader, PROBE_TIMEOUT, RESUME_TIMEOUT, UPGRADE_EVERY,
};
use rsp_core::pb;
use rsp_transport::TransportKind;
use tokio::time::Instant;

fn ok(resumed: bool) -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::Ok as i32,
        session_token: vec![0x5a; 16],
        resumed,
        ..Default::default()
    }
}

fn denied() -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::Denied as i32,
        ..Default::default()
    }
}

fn pending() -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::PendingApproval as i32,
        approval_timeout_s: 30,
        ..Default::default()
    }
}

fn unknown() -> pb::AuthResult {
    pb::AuthResult {
        status: 99,
        ..Default::default()
    }
}

fn started(t0: Instant) -> Upgrader {
    let mut upgrader = Upgrader::new();
    assert!(upgrader.on_primary(TransportKind::Tcp, t0).is_empty());
    upgrader
}

fn probing(t0: Instant) -> Upgrader {
    let mut upgrader = started(t0);
    assert_eq!(
        upgrader.poll(t0 + UPGRADE_EVERY),
        vec![UpgradeAction::ProbeQuic]
    );
    upgrader
}

fn resuming(t0: Instant) -> Upgrader {
    let mut upgrader = probing(t0);
    assert_eq!(
        upgrader.on_probe_ok(t0 + UPGRADE_EVERY + Duration::from_millis(200)),
        vec![UpgradeAction::ResumeOnQuic]
    );
    upgrader
}

#[test]
fn tcp_primary_waits_thirty_seconds_before_probing() {
    let t0 = Instant::now();
    let mut upgrader = Upgrader::new();

    assert_eq!(upgrader.state(), UpgradeState::Inactive);
    assert!(upgrader.on_primary(TransportKind::Tcp, t0).is_empty());
    assert_eq!(
        upgrader.state(),
        UpgradeState::Waiting {
            next: t0 + UPGRADE_EVERY
        }
    );
    assert!(upgrader.poll(t0 + Duration::from_millis(29_999)).is_empty());
    assert_eq!(
        upgrader.poll(t0 + UPGRADE_EVERY),
        vec![UpgradeAction::ProbeQuic]
    );
    assert_eq!(upgrader.attempts(), 1);
    assert_eq!(
        upgrader.state(),
        UpgradeState::Probing {
            started: t0 + UPGRADE_EVERY
        }
    );
}

#[test]
fn successful_resume_commits_and_becomes_inactive() {
    let t0 = Instant::now();
    let mut upgrader = probing(t0);
    let probe_ok_at = t0 + UPGRADE_EVERY + Duration::from_millis(200);

    assert_eq!(
        upgrader.on_probe_ok(probe_ok_at),
        vec![UpgradeAction::ResumeOnQuic]
    );
    assert_eq!(
        upgrader.state(),
        UpgradeState::Resuming {
            started: t0 + UPGRADE_EVERY,
            since: probe_ok_at,
        }
    );
    assert!(upgrader.expects_replacement());

    let auth_at = probe_ok_at + Duration::from_millis(50);
    assert_eq!(
        upgrader.on_auth_result(&ok(true), auth_at),
        vec![UpgradeAction::Commit]
    );
    assert_eq!(upgrader.state(), UpgradeState::Inactive);
    assert!(!upgrader.expects_replacement());
    assert!(upgrader
        .poll(auth_at + Duration::from_secs(3_600))
        .is_empty());
}

#[test]
fn retry_cadence_uses_attempt_start_not_failure_time() {
    let t0 = Instant::now();
    let mut upgrader = probing(t0);

    assert!(upgrader
        .on_probe_failed(t0 + UPGRADE_EVERY + Duration::from_secs(1))
        .is_empty());
    assert_eq!(
        upgrader.state(),
        UpgradeState::Waiting {
            next: t0 + 2 * UPGRADE_EVERY
        }
    );
    assert!(upgrader
        .poll(t0 + 2 * UPGRADE_EVERY - Duration::from_millis(1))
        .is_empty());
    assert_eq!(
        upgrader.poll(t0 + 2 * UPGRADE_EVERY),
        vec![UpgradeAction::ProbeQuic]
    );
}

#[test]
fn probe_timeout_drops_probe_and_keeps_attempt_start_cadence() {
    let t0 = Instant::now();
    let attempt_at = t0 + UPGRADE_EVERY;
    let mut upgrader = probing(t0);

    assert!(upgrader
        .poll(attempt_at + PROBE_TIMEOUT - Duration::from_millis(1))
        .is_empty());
    assert_eq!(
        upgrader.poll(attempt_at + PROBE_TIMEOUT),
        vec![UpgradeAction::DropProbe]
    );
    assert_eq!(
        upgrader.state(),
        UpgradeState::Waiting {
            next: t0 + 2 * UPGRADE_EVERY
        }
    );
    assert!(upgrader
        .poll(t0 + 2 * UPGRADE_EVERY - Duration::from_millis(1))
        .is_empty());
    assert_eq!(
        upgrader.poll(t0 + 2 * UPGRADE_EVERY),
        vec![UpgradeAction::ProbeQuic]
    );
}

#[test]
fn resume_timeout_drops_probe_and_waits_for_next_cadence() {
    let t0 = Instant::now();
    let resume_at = t0 + UPGRADE_EVERY + Duration::from_millis(200);
    let mut upgrader = resuming(t0);

    assert!(upgrader
        .poll(resume_at + RESUME_TIMEOUT - Duration::from_millis(1))
        .is_empty());
    assert_eq!(
        upgrader.poll(resume_at + RESUME_TIMEOUT),
        vec![UpgradeAction::DropProbe]
    );
    assert_eq!(
        upgrader.state(),
        UpgradeState::Waiting {
            next: t0 + 2 * UPGRADE_EVERY
        }
    );
}

#[test]
fn non_resumed_auth_result_commits_a_new_session() {
    let t0 = Instant::now();
    let mut upgrader = resuming(t0);
    let auth_at = t0 + UPGRADE_EVERY + Duration::from_millis(300);

    assert_eq!(
        upgrader.on_auth_result(&ok(false), auth_at),
        vec![UpgradeAction::CommitNewSession]
    );
    assert_eq!(upgrader.state(), UpgradeState::Inactive);
    assert!(!upgrader.expects_replacement());
}

#[test]
fn rejected_auth_results_drop_the_probe() {
    let t0 = Instant::now();
    let auth_at = t0 + UPGRADE_EVERY + Duration::from_millis(300);

    for result in [denied(), pending(), unknown()] {
        let mut upgrader = resuming(t0);
        assert_eq!(
            upgrader.on_auth_result(&result, auth_at),
            vec![UpgradeAction::DropProbe]
        );
        assert_eq!(
            upgrader.state(),
            UpgradeState::Waiting {
                next: t0 + 2 * UPGRADE_EVERY
            }
        );
        assert!(!upgrader.expects_replacement());
    }
}

#[test]
fn blocking_pauses_due_probes_and_unblocking_probes_immediately() {
    let t0 = Instant::now();
    let mut upgrader = started(t0);

    assert!(upgrader
        .set_blocked(true, t0 + Duration::from_secs(10))
        .is_empty());
    assert_eq!(
        upgrader.state(),
        UpgradeState::Waiting {
            next: t0 + UPGRADE_EVERY
        }
    );
    assert_eq!(upgrader.next_deadline(), None);
    assert!(upgrader.poll(t0 + Duration::from_secs(40)).is_empty());

    assert!(upgrader
        .set_blocked(false, t0 + Duration::from_secs(45))
        .is_empty());
    assert_eq!(
        upgrader.state(),
        UpgradeState::Waiting {
            next: t0 + Duration::from_secs(45)
        }
    );
    assert_eq!(
        upgrader.poll(t0 + Duration::from_secs(45)),
        vec![UpgradeAction::ProbeQuic]
    );
}

#[test]
fn blocking_while_probing_drops_the_probe_and_preserves_cadence() {
    let t0 = Instant::now();
    let attempt_at = t0 + UPGRADE_EVERY;
    let mut upgrader = probing(t0);

    assert_eq!(
        upgrader.set_blocked(true, attempt_at + Duration::from_secs(1)),
        vec![UpgradeAction::DropProbe]
    );
    assert_eq!(
        upgrader.state(),
        UpgradeState::Waiting {
            next: t0 + 2 * UPGRADE_EVERY
        }
    );
    assert!(upgrader
        .poll(t0 + 2 * UPGRADE_EVERY + Duration::from_secs(1))
        .is_empty());
}

#[test]
fn quic_primary_discards_resume_and_late_callbacks() {
    let t0 = Instant::now();
    let mut upgrader = resuming(t0);
    let switch_at = t0 + UPGRADE_EVERY + Duration::from_millis(300);

    assert_eq!(
        upgrader.on_primary(TransportKind::Quic, switch_at),
        vec![UpgradeAction::DropProbe]
    );
    assert_eq!(upgrader.state(), UpgradeState::Inactive);
    assert!(!upgrader.expects_replacement());
    assert_eq!(
        upgrader.on_probe_ok(switch_at + Duration::from_millis(10)),
        vec![UpgradeAction::DropProbe]
    );
    assert_eq!(upgrader.state(), UpgradeState::Inactive);
    assert!(upgrader
        .on_auth_result(&ok(true), switch_at + Duration::from_millis(20))
        .is_empty());
    assert_eq!(upgrader.state(), UpgradeState::Inactive);
}

#[test]
fn five_failed_attempts_are_exactly_thirty_seconds_apart() {
    let t0 = Instant::now();
    let mut upgrader = started(t0);

    for attempt in 1..=5 {
        let attempt_at = t0 + attempt * UPGRADE_EVERY;
        assert!(upgrader
            .poll(attempt_at - Duration::from_millis(1))
            .is_empty());
        assert_eq!(upgrader.poll(attempt_at), vec![UpgradeAction::ProbeQuic]);
        assert_eq!(upgrader.attempts(), attempt);
        assert!(upgrader
            .on_probe_failed(attempt_at + Duration::from_secs(1))
            .is_empty());
        assert_eq!(
            upgrader.state(),
            UpgradeState::Waiting {
                next: t0 + (attempt + 1) * UPGRADE_EVERY
            }
        );
    }

    assert_eq!(upgrader.attempts(), 5);
}

#[test]
fn next_deadline_reflects_each_state() {
    let t0 = Instant::now();
    let mut upgrader = Upgrader::new();
    assert_eq!(upgrader.next_deadline(), None);

    assert!(upgrader.on_primary(TransportKind::Tcp, t0).is_empty());
    assert_eq!(upgrader.next_deadline(), Some(t0 + UPGRADE_EVERY));

    assert_eq!(
        upgrader.poll(t0 + UPGRADE_EVERY),
        vec![UpgradeAction::ProbeQuic]
    );
    assert_eq!(
        upgrader.next_deadline(),
        Some(t0 + UPGRADE_EVERY + PROBE_TIMEOUT)
    );

    let probe_ok_at = t0 + UPGRADE_EVERY + Duration::from_millis(200);
    assert_eq!(
        upgrader.on_probe_ok(probe_ok_at),
        vec![UpgradeAction::ResumeOnQuic]
    );
    assert_eq!(upgrader.next_deadline(), Some(probe_ok_at + RESUME_TIMEOUT));

    assert_eq!(
        upgrader.on_auth_result(&ok(true), probe_ok_at + Duration::from_millis(100)),
        vec![UpgradeAction::Commit]
    );
    assert_eq!(upgrader.next_deadline(), None);
}
