use std::time::Duration;

use rsp_client::recovery::failover::{
    ConnId, Failover, FailoverAction, FailoverPhase, FrameVerdict, GiveUp, InputVerdict,
    ReconnectReason, TicketSlot, BACKUP_ATTEMPT_TIMEOUT, KEYFRAME_WAIT, REDIAL_ATTEMPT_TIMEOUT,
    REDIAL_BACKOFF, RESUME_TTL, SWITCH_LIMIT, TOKEN_LEN,
};
use rsp_core::pb;
use tokio::time::Instant;

const FP: [u8; 32] = [0x42; 32];
const OTHER_FP: [u8; 32] = [0x24; 32];
const A: ConnId = ConnId(1);
const B: ConnId = ConnId(2);
const C: ConnId = ConnId(3);
const D: ConnId = ConnId(4);

fn token(value: u8) -> Vec<u8> {
    vec![value; TOKEN_LEN]
}

fn ok(value: u8, resumed: bool) -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::Ok as i32,
        session_token: token(value),
        resumed,
        ..Default::default()
    }
}

fn pending(timeout: u32) -> pb::AuthResult {
    pb::AuthResult {
        status: pb::auth_result::Status::PendingApproval as i32,
        approval_timeout_s: timeout,
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

fn setup(_t0: Instant) -> (Failover, TicketSlot) {
    let mut tickets = TicketSlot::new();
    assert!(tickets.store(&token(7), FP));
    (Failover::new(A, FP), tickets)
}

#[test]
fn ticket_slot_lifetime_and_redacted_debug() {
    let t0 = Instant::now();
    let mut tickets = TicketSlot::new();
    assert!(tickets.store(&token(7), FP));
    assert!(!tickets.store(&[7; TOKEN_LEN - 1], FP));
    assert_eq!(tickets.token_for(&FP, t0), Some([7; TOKEN_LEN]));
    assert_eq!(tickets.token_for(&OTHER_FP, t0), None);
    tickets.mark_lost(t0);
    tickets.mark_lost(t0 + Duration::from_secs(10));
    assert_eq!(
        tickets.token_for(&FP, t0 + RESUME_TTL - Duration::from_millis(1)),
        Some([7; TOKEN_LEN])
    );
    assert_eq!(tickets.token_for(&FP, t0 + RESUME_TTL), None);
    let debug = format!("{tickets:?}");
    assert!(debug.contains("<redacted>"));
    assert!(!debug.contains("07"));
    assert!(tickets.store(&[], FP));
    assert!(!tickets.is_valid(t0));
}

#[test]
fn backup_recovery_freezes_and_waits_for_keyframe() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    assert_eq!(
        failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0),
        vec![
            FailoverAction::EnterReconnecting {
                reason: ReconnectReason::PathLost
            },
            FailoverAction::FreezeVideo,
            FailoverAction::ResetReassembler,
            FailoverAction::CloseConn(A),
            FailoverAction::ResumeOn {
                conn: B,
                token: [7; TOKEN_LEN]
            },
        ]
    );
    assert_eq!(
        failover.on_auth_result(
            B,
            &ok(8, true),
            &mut tickets,
            t0 + Duration::from_millis(300)
        ),
        vec![
            FailoverAction::SendReleaseAll,
            FailoverAction::Recovered {
                elapsed: Duration::from_millis(300),
                resumed: true,
                slow: false,
            },
        ]
    );
    assert_eq!(tickets.token_for(&FP, t0), Some([8; TOKEN_LEN]));
    assert_eq!(failover.frame_verdict(B, false), FrameVerdict::Drop);
    assert_eq!(
        failover.frame_verdict(B, true),
        FrameVerdict::DeliverAndUnfreeze
    );
    assert_eq!(failover.frame_verdict(B, false), FrameVerdict::Deliver);
    assert_eq!(failover.input_verdict(), InputVerdict::Send);
}

#[test]
fn inputs_drop_while_switching_and_recover_after_keyframe() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    for _ in 0..5 {
        assert_eq!(failover.input_verdict(), InputVerdict::Drop);
    }
    assert_eq!(failover.dropped_inputs(), 5);
    failover.on_auth_result(B, &ok(8, true), &mut tickets, t0);
    assert_eq!(failover.input_verdict(), InputVerdict::Send);
}

#[test]
fn backup_timeout_falls_back_to_redial() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    assert_eq!(
        failover.poll(&mut tickets, t0 + BACKUP_ATTEMPT_TIMEOUT),
        vec![
            FailoverAction::CloseConn(B),
            FailoverAction::Redial { attempt: 1 }
        ]
    );
    assert_eq!(
        failover.on_connected(C, &tickets, t0 + BACKUP_ATTEMPT_TIMEOUT),
        vec![FailoverAction::ResumeOn {
            conn: C,
            token: [7; TOKEN_LEN]
        }]
    );
    let actions = failover.on_auth_result(
        C,
        &ok(8, true),
        &mut tickets,
        t0 + Duration::from_millis(1300),
    );
    assert!(
        matches!(actions.get(1), Some(FailoverAction::Recovered { elapsed, .. }) if *elapsed == Duration::from_millis(1300))
    );
}

#[test]
fn redial_backoff_and_deadline_are_bounded_by_ticket() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, None, &mut tickets, t0);
    assert_eq!(failover.next_deadline(), Some(t0 + REDIAL_ATTEMPT_TIMEOUT));
    failover.on_redial_failed(&mut tickets, t0);
    assert_eq!(failover.next_deadline(), Some(t0 + REDIAL_BACKOFF[1]));
    assert_eq!(
        failover.poll(&mut tickets, t0 + REDIAL_BACKOFF[1]),
        vec![FailoverAction::Redial { attempt: 2 }]
    );
    failover.on_redial_failed(&mut tickets, t0 + REDIAL_BACKOFF[1]);
    assert_eq!(
        failover.next_deadline(),
        Some(t0 + REDIAL_BACKOFF[1] + REDIAL_BACKOFF[2])
    );
    assert!(failover
        .next_deadline()
        .is_some_and(|deadline| deadline <= t0 + RESUME_TTL));
}

#[test]
fn expiration_gives_up_and_stops() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, None, &mut tickets, t0);
    assert_eq!(
        failover.poll(&mut tickets, t0 + RESUME_TTL),
        vec![FailoverAction::GiveUp(GiveUp::TicketExpired)]
    );
    assert_eq!(
        failover.phase(),
        &FailoverPhase::Failed(GiveUp::TicketExpired)
    );
    assert!(failover.poll(&mut tickets, t0 + RESUME_TTL).is_empty());
    assert!(failover
        .start(ReconnectReason::PathLost, A, None, &mut tickets, t0)
        .is_empty());
}

#[test]
fn auth_result_outcomes() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    let actions = failover.on_auth_result(B, &ok(8, false), &mut tickets, t0);
    assert!(actions.contains(&FailoverAction::StartNewSession));

    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    assert_eq!(
        failover.on_auth_result(B, &pending(30), &mut tickets, t0),
        vec![FailoverAction::HandToApproval {
            approval_timeout_s: 30
        }]
    );
    assert!(!tickets.is_valid(t0));

    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    assert_eq!(
        failover.on_auth_result(B, &denied(180), &mut tickets, t0),
        vec![FailoverAction::GiveUp(GiveUp::Denied(180))]
    );
    assert!(!tickets.is_valid(t0));
}

#[test]
fn closes_and_stale_events_are_handled() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    assert!(failover
        .on_auth_result(A, &ok(8, true), &mut tickets, t0)
        .is_empty());
    assert_eq!(
        failover.on_conn_closed(A, Some(3), Some(B), &mut tickets, t0),
        Vec::new()
    );
    failover.on_auth_result(B, &ok(8, true), &mut tickets, t0);
    assert_eq!(
        failover.on_conn_closed(A, Some(3), Some(B), &mut tickets, t0),
        Vec::new()
    );
    assert_eq!(
        failover.on_conn_closed(B, Some(0), Some(C), &mut tickets, t0),
        Vec::new()
    );
    assert!(!failover
        .on_conn_closed(B, None, Some(C), &mut tickets, t0)
        .is_empty());

    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    assert_eq!(
        failover.on_conn_closed(B, Some(6), None, &mut tickets, t0),
        vec![FailoverAction::GiveUp(GiveUp::Closed(6))]
    );

    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    assert_eq!(
        failover.on_conn_closed(B, None, None, &mut tickets, t0),
        vec![FailoverAction::Redial { attempt: 1 }]
    );
}

#[test]
fn keyframe_request_is_sent_once_and_adopt_only_streaming() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    failover.on_auth_result(B, &ok(8, true), &mut tickets, t0);
    assert!(failover
        .poll(&mut tickets, t0 + KEYFRAME_WAIT - Duration::from_millis(1))
        .is_empty());
    assert_eq!(
        failover.poll(&mut tickets, t0 + KEYFRAME_WAIT),
        vec![FailoverAction::RequestKeyframe]
    );
    assert!(failover.poll(&mut tickets, t0 + KEYFRAME_WAIT).is_empty());
    assert_eq!(
        failover.frame_verdict(B, true),
        FrameVerdict::DeliverAndUnfreeze
    );
    assert!(failover.adopt(D));
    assert_eq!(failover.current(), D);
    assert_eq!(failover.frame_verdict(B, true), FrameVerdict::Drop);

    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    assert!(!failover.adopt(D));
}

#[test]
fn slow_recovery_is_marked() {
    let t0 = Instant::now();
    let (mut failover, mut tickets) = setup(t0);
    failover.start(ReconnectReason::PathLost, A, Some(B), &mut tickets, t0);
    let actions = failover.on_auth_result(
        B,
        &ok(8, true),
        &mut tickets,
        t0 + SWITCH_LIMIT + Duration::from_millis(500),
    );
    assert!(matches!(
        actions.last(),
        Some(FailoverAction::Recovered { slow: true, .. })
    ));
}
