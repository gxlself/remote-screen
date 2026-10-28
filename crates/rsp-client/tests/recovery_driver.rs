//! Acceptance tests for the composed path driver.

use std::time::Duration;

use rsp_client::recovery::driver::{DriverConfig, PathAction, PathDriver};
use rsp_client::recovery::failover::{
    ConnId, FailoverAction, FrameVerdict, GiveUp, InputVerdict, TicketSlot, TOKEN_LEN,
};
use rsp_client::recovery::liveness::PathSig;
use rsp_client::recovery::upgrade::UpgradeAction;
use rsp_core::pb;
use rsp_transport::{dial::PathKind, TransportKind};
use tokio::time::Instant;

const FP: [u8; 32] = [0x42; 32];
const PRIMARY: ConnId = ConnId(1);
const BACKUP: ConnId = ConnId(2);

fn ms(value: u64) -> Duration {
    Duration::from_millis(value)
}

fn primary_sig() -> PathSig {
    PathSig::new(&PathKind::Tailnet, TransportKind::Quic)
}

fn backup_sig() -> PathSig {
    PathSig::new(&PathKind::Manual, TransportKind::Quic)
}

fn driver(t0: Instant, backup: bool) -> PathDriver {
    PathDriver::new(
        DriverConfig::default(),
        FP,
        primary_sig(),
        backup.then(backup_sig),
        t0,
    )
}

fn tickets() -> TicketSlot {
    let mut tickets = TicketSlot::new();
    assert!(tickets.store(&[1; TOKEN_LEN], FP));
    tickets
}

fn ok(token: u8, resumed: bool) -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::Ok as i32,
        session_token: vec![token; TOKEN_LEN],
        resumed,
        ..Default::default()
    }
}

fn denied(code: u32) -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::Denied as i32,
        error: Some(pb::ErrorInfo {
            code,
            ..Default::default()
        }),
        ..Default::default()
    }
}

fn dead_with_backup(
    driver: &mut PathDriver,
    tickets: &mut TicketSlot,
    t0: Instant,
) -> Vec<PathAction> {
    driver.on_streaming(t0);
    assert!(driver
        .poll(tickets, t0 + ms(500))
        .contains(&PathAction::SendPing));
    driver.on_ping_sent(1, t0 + ms(500));
    assert!(driver
        .poll(tickets, t0 + ms(750))
        .contains(&PathAction::SendPing));
    driver.on_ping_sent(2, t0 + ms(750));
    driver.poll(tickets, t0 + ms(1500))
}

#[test]
fn silent_primary_with_backup_switches_at_1500ms() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    assert_eq!(
        dead_with_backup(&mut driver, &mut tickets, t0),
        vec![
            PathAction::Failover(FailoverAction::EnterReconnecting {
                reason: rsp_client::recovery::failover::ReconnectReason::PathLost,
            }),
            PathAction::Failover(FailoverAction::FreezeVideo),
            PathAction::Failover(FailoverAction::ResetReassembler),
            PathAction::Failover(FailoverAction::CloseConn(PRIMARY)),
            PathAction::Failover(FailoverAction::ResumeOn {
                conn: BACKUP,
                token: [1; TOKEN_LEN],
            }),
        ]
    );
}

#[test]
fn inbound_every_400ms_never_dies() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    driver.on_streaming(t0);
    for elapsed in (400..=10_000).step_by(400) {
        let now = t0 + ms(elapsed);
        driver.on_inbound(PRIMARY, now);
        assert!(driver
            .poll(&mut tickets, now)
            .iter()
            .all(|action| !matches!(action, PathAction::Failover(_))));
    }
}

#[test]
fn inbound_on_backup_does_not_keep_primary_alive() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    driver.on_streaming(t0);
    for elapsed in (400..=1200).step_by(400) {
        driver.on_inbound(BACKUP, t0 + ms(elapsed));
    }
    assert!(dead_with_backup(&mut driver, &mut tickets, t0)
        .iter()
        .any(|action| {
            matches!(
                action,
                PathAction::Failover(FailoverAction::EnterReconnecting { .. })
            )
        }));
}

#[test]
fn resumed_auth_result_promotes_backup() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    dead_with_backup(&mut driver, &mut tickets, t0);
    let actions = driver.on_auth_result(BACKUP, &ok(2, true), &mut tickets, t0 + ms(100));
    assert!(actions.contains(&PathAction::Failover(FailoverAction::SendReleaseAll)));
    assert!(actions.iter().any(|action| {
        matches!(
            action,
            PathAction::Failover(FailoverAction::Recovered { resumed: true, .. })
        )
    }));
    assert_eq!(driver.primary(), BACKUP);
    assert_eq!(driver.primary_sig(), &backup_sig());
    assert_eq!(driver.backup(), None);
    assert_eq!(tickets.token_for(&FP, t0), Some([2; TOKEN_LEN]));
    assert_eq!(driver.frame_verdict(PRIMARY, true), FrameVerdict::Drop);
    assert_eq!(driver.frame_verdict(BACKUP, false), FrameVerdict::Drop);
    assert_eq!(
        driver.frame_verdict(BACKUP, true),
        FrameVerdict::DeliverAndUnfreeze
    );
    assert_eq!(driver.frame_verdict(BACKUP, false), FrameVerdict::Deliver);
}

#[test]
fn no_backup_and_redial_disabled_is_lost() {
    let t0 = Instant::now();
    let mut driver = driver(t0, false);
    let mut tickets = tickets();
    let actions = dead_with_backup(&mut driver, &mut tickets, t0);
    assert!(
        actions.contains(&PathAction::Failover(FailoverAction::EnterReconnecting {
            reason: rsp_client::recovery::failover::ReconnectReason::PathLost,
        }))
    );
    assert!(actions.contains(&PathAction::Failover(FailoverAction::CloseConn(PRIMARY))));
    assert_eq!(actions.last(), Some(&PathAction::Lost));
    assert!(!actions
        .iter()
        .any(|action| { matches!(action, PathAction::Failover(FailoverAction::Redial { .. })) }));
    assert!(driver.is_lost());
    assert!(driver.poll(&mut tickets, t0 + ms(3000)).is_empty());
}

#[test]
fn redial_enabled_passes_redial_and_resumes_on_new_conn() {
    let t0 = Instant::now();
    let cfg = DriverConfig {
        redial: true,
        ..DriverConfig::default()
    };
    let mut driver = PathDriver::new(cfg, FP, primary_sig(), None, t0);
    let mut tickets = tickets();
    let actions = dead_with_backup(&mut driver, &mut tickets, t0);
    assert!(actions.contains(&PathAction::Failover(FailoverAction::Redial { attempt: 1 })));
    let conn = driver.next_conn_id();
    assert_eq!(conn, ConnId(3));
    assert_eq!(
        driver.on_redial_connected(conn, backup_sig(), &tickets, t0 + ms(100)),
        vec![PathAction::Failover(FailoverAction::ResumeOn {
            conn,
            token: [1; TOKEN_LEN],
        })]
    );
    driver.on_auth_result(conn, &ok(2, true), &mut tickets, t0 + ms(200));
    assert_eq!(driver.primary(), conn);
}

#[test]
fn backup_attempt_timeout_falls_back() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    dead_with_backup(&mut driver, &mut tickets, t0);
    let actions = driver.poll(&mut tickets, t0 + ms(2500));
    assert_eq!(
        actions,
        vec![
            PathAction::Failover(FailoverAction::CloseConn(BACKUP)),
            PathAction::Lost,
        ]
    );
}

#[test]
fn held_backup_lost_then_primary_dies_is_lost() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    driver.on_streaming(t0);
    assert!(driver
        .on_conn_closed(BACKUP, None, &mut tickets, t0)
        .is_empty());
    assert_eq!(driver.backup(), None);
    assert!(dead_with_backup(&mut driver, &mut tickets, t0).contains(&PathAction::Lost));
}

#[test]
fn close_code_on_primary_is_not_a_path_loss() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    driver.on_streaming(t0);
    assert!(driver
        .on_conn_closed(PRIMARY, Some(6), &mut tickets, t0)
        .is_empty());
    assert!(!driver.is_switching());
}

#[test]
fn suspend_blocks_death_and_backup_refill() {
    let t0 = Instant::now();
    let mut driver = driver(t0, false);
    let mut tickets = tickets();
    driver.on_streaming(t0);
    driver.suspend();
    assert!(driver
        .poll(&mut tickets, t0 + Duration::from_secs(10))
        .is_empty());
    let resumed = t0 + Duration::from_secs(10);
    driver.resume(resumed);
    assert!(driver
        .poll(&mut tickets, resumed + ms(500))
        .contains(&PathAction::SendPing));
    driver.on_ping_sent(1, resumed + ms(500));
    assert!(driver
        .poll(&mut tickets, resumed + ms(750))
        .contains(&PathAction::SendPing));
    driver.on_ping_sent(2, resumed + ms(750));
    let before_dead = driver.poll(&mut tickets, resumed + ms(1499));
    assert!(!before_dead.iter().any(|action| matches!(
        action,
        PathAction::Failover(FailoverAction::EnterReconnecting { .. }) | PathAction::Lost
    )));
    if before_dead.contains(&PathAction::SendPing) {
        driver.on_ping_sent(3, resumed + ms(1499));
    }
    assert!(driver
        .poll(&mut tickets, resumed + ms(1500))
        .contains(&PathAction::Lost));
}

#[test]
fn denied_resume_gives_up_and_clears_ticket() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    dead_with_backup(&mut driver, &mut tickets, t0);
    assert_eq!(
        driver.on_auth_result(BACKUP, &denied(180), &mut tickets, t0 + ms(100)),
        vec![PathAction::Failover(FailoverAction::GiveUp(
            GiveUp::Denied(180)
        ))]
    );
    assert!(!tickets.is_valid(t0));
}

#[test]
fn input_dropped_only_while_switching() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    driver.on_streaming(t0);
    assert_eq!(driver.input_verdict(), InputVerdict::Send);
    dead_with_backup(&mut driver, &mut tickets, t0);
    assert_eq!(driver.input_verdict(), InputVerdict::Drop);
    assert_eq!(driver.dropped_inputs(), 1);
    driver.on_auth_result(BACKUP, &ok(2, true), &mut tickets, t0 + ms(100));
    assert_eq!(driver.input_verdict(), InputVerdict::Send);
}

#[test]
fn not_distinct_backup_is_closed() {
    let t0 = Instant::now();
    let mut driver = driver(t0, false);
    assert_eq!(
        driver.on_backup_ready(ConnId(3), primary_sig(), t0),
        vec![PathAction::CloseBackup(ConnId(3))]
    );
}

#[test]
fn next_deadline_is_earliest() {
    let t0 = Instant::now();
    let mut driver = driver(t0, true);
    let mut tickets = tickets();
    driver.on_streaming(t0);
    assert_eq!(driver.next_deadline(), Some(t0 + ms(500)));
    dead_with_backup(&mut driver, &mut tickets, t0);
    assert_eq!(driver.next_deadline(), Some(t0 + ms(2500)));
}

fn secs(value: u64) -> Duration {
    Duration::from_secs(value)
}

fn tcp_sig() -> PathSig {
    PathSig::new(&PathKind::Tailnet, TransportKind::Tcp)
}

fn upgrade_driver(t0: Instant, primary: PathSig) -> PathDriver {
    let cfg = DriverConfig {
        redial: true,
        ..DriverConfig::default()
    };
    let mut driver = PathDriver::new(cfg, FP, primary, None, t0);
    driver.on_streaming(t0);
    driver
}

/// Polls a primary that just received data, so liveness never interferes.
fn alive_poll(driver: &mut PathDriver, tickets: &mut TicketSlot, now: Instant) -> Vec<PathAction> {
    driver.on_inbound(driver.primary(), now);
    driver.poll(tickets, now)
}

fn probes(actions: &[PathAction]) -> bool {
    actions.contains(&PathAction::Upgrade(UpgradeAction::ProbeQuic))
}

#[test]
fn tcp_primary_probes_every_30s() {
    let t0 = Instant::now();
    let mut driver = upgrade_driver(t0, tcp_sig());
    let mut tickets = tickets();
    for elapsed in (1..30).map(secs) {
        assert!(!probes(&alive_poll(
            &mut driver,
            &mut tickets,
            t0 + elapsed
        )));
    }
    assert!(!probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(30) - ms(1)
    )));
    assert!(probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(30)
    )));
    assert!(driver.on_probe_failed(t0 + secs(31)).is_empty());
    for elapsed in (32..60).map(secs) {
        assert!(!probes(&alive_poll(
            &mut driver,
            &mut tickets,
            t0 + elapsed
        )));
    }
    assert!(!probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(60) - ms(1)
    )));
    assert!(probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(60)
    )));
}

#[test]
fn quic_primary_never_probes() {
    let t0 = Instant::now();
    let mut driver = upgrade_driver(t0, primary_sig());
    let mut tickets = tickets();
    for elapsed in (1..=150).map(secs) {
        let actions = alive_poll(&mut driver, &mut tickets, t0 + elapsed);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, PathAction::Upgrade(_))),
            "{actions:?}"
        );
    }
    assert!(!driver.expects_replacement());
}

#[test]
fn suspend_blocks_probe_and_drops_in_flight() {
    let t0 = Instant::now();
    let mut driver = upgrade_driver(t0, tcp_sig());
    let mut tickets = tickets();
    assert!(probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(30)
    )));
    assert_eq!(
        driver.suspend(),
        vec![PathAction::Upgrade(UpgradeAction::DropProbe)]
    );
    for elapsed in (31..=45).map(secs) {
        assert!(!probes(&driver.poll(&mut tickets, t0 + elapsed)));
    }
    driver.resume(t0 + secs(45));
    for elapsed in (46..60).map(secs) {
        assert!(!probes(&alive_poll(
            &mut driver,
            &mut tickets,
            t0 + elapsed
        )));
    }
    assert!(!probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(60) - ms(1)
    )));
    assert!(probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(60)
    )));
}

#[test]
fn commit_switches_primary_and_stops_upgrading() {
    let t0 = Instant::now();
    let mut driver = upgrade_driver(t0, tcp_sig());
    let mut tickets = tickets();
    assert!(probes(&alive_poll(
        &mut driver,
        &mut tickets,
        t0 + secs(30)
    )));
    let candidate = driver.next_conn_id();
    let quic = PathSig::new(&PathKind::Manual, TransportKind::Quic);
    assert_eq!(
        driver.on_probe_ok(candidate, quic.clone(), t0 + secs(30) + ms(100)),
        vec![PathAction::Upgrade(UpgradeAction::ResumeOnQuic)]
    );
    assert!(driver.expects_replacement());
    let actions = driver.on_upgrade_auth_result(
        candidate,
        &ok(2, true),
        &mut tickets,
        t0 + secs(30) + ms(200),
    );
    assert!(actions.contains(&PathAction::Upgrade(UpgradeAction::Commit)));
    assert_eq!(driver.primary(), candidate);
    assert_eq!(driver.primary_sig(), &quic);
    assert_eq!(tickets.token_for(&FP, t0), Some([2; TOKEN_LEN]));
    assert!(!driver.expects_replacement());
    assert_eq!(driver.frame_verdict(PRIMARY, true), FrameVerdict::Drop);
    assert_eq!(
        driver.frame_verdict(candidate, false),
        FrameVerdict::Deliver
    );
    for elapsed in (31..=150).map(secs) {
        let actions = alive_poll(&mut driver, &mut tickets, t0 + elapsed);
        assert!(
            !actions
                .iter()
                .any(|action| matches!(action, PathAction::Upgrade(_))),
            "{actions:?}"
        );
    }
}

#[test]
fn switching_blocks_upgrade() {
    let t0 = Instant::now();
    let mut driver = upgrade_driver(t0, tcp_sig());
    let mut tickets = tickets();
    let silent_at = t0 + secs(28);
    assert!(!probes(&alive_poll(&mut driver, &mut tickets, silent_at)));
    let mut now = silent_at;
    let mut t1_us = 0;
    while !driver.is_switching() {
        now += ms(250);
        assert!(now <= silent_at + secs(3), "TCP primary never died");
        let actions = driver.poll(&mut tickets, now);
        assert!(!probes(&actions));
        if actions.contains(&PathAction::SendPing) {
            t1_us += 1;
            driver.on_ping_sent(t1_us, now);
        }
    }
    assert!(now < t0 + secs(30));
    while now < t0 + secs(36) {
        now += ms(250);
        let actions = driver.poll(&mut tickets, now);
        assert!(!probes(&actions), "{actions:?}");
        assert!(driver.is_switching());
    }
    assert!(!driver.expects_replacement());
}
