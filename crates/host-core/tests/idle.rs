use host_core::platform::{PlatformError, SessionActionKind};
use host_core::session::idle::{
    decide_session_action, idle_warning_msg, platform_failure_error, reject_error,
    should_lock_on_end, ActionDecision, ActionReject, EndReason, IdleEvent, IdleTimer, LockOnEnd,
};
use rsp_core::errors::ErrorCode;
use rsp_core::pb;

const MIN: u64 = 60_000_000;

#[test]
fn timer_warns_once_then_expires_once() {
    let t0 = 1_700_000_000_000_000;
    let mut timer = IdleTimer::new(30, t0);

    assert_eq!(timer.deadline(), Some(t0 + 30 * MIN));
    assert_eq!(timer.next_deadline(), Some(t0 + 29 * MIN));
    assert_eq!(timer.poll(t0 + 29 * MIN - 1), None);
    assert_eq!(
        timer.poll(t0 + 29 * MIN),
        Some(IdleEvent::Warn { seconds_left: 60 })
    );
    assert_eq!(timer.poll(t0 + 29 * MIN + 1), None);
    assert_eq!(timer.poll(t0 + 30 * MIN - 1), None);
    assert_eq!(timer.poll(t0 + 30 * MIN), Some(IdleEvent::Expire));
    assert_eq!(timer.poll(t0 + 30 * MIN + 1), None);
    assert_eq!(timer.next_deadline(), None);

    timer.on_input(t0 + 31 * MIN);
    assert_eq!(timer.poll(t0 + 61 * MIN), None);
    assert_eq!(timer.next_deadline(), None);
}

#[test]
fn input_after_warning_restarts_the_timer() {
    let t0 = 2_000_000_000_000_000;
    let mut timer = IdleTimer::new(30, t0);
    let input_at = t0 + 29 * MIN;

    assert_eq!(
        timer.poll(input_at),
        Some(IdleEvent::Warn { seconds_left: 60 })
    );
    timer.on_input(input_at);

    assert_eq!(timer.deadline(), Some(input_at + 30 * MIN));
    assert_eq!(timer.next_deadline(), Some(input_at + 29 * MIN));
    assert_eq!(timer.poll(input_at + 29 * MIN - 1), None);
    assert_eq!(
        timer.poll(input_at + 29 * MIN),
        Some(IdleEvent::Warn { seconds_left: 60 })
    );
}

#[test]
fn warning_seconds_are_rounded_up() {
    let t0 = 3_000_000_000_000_000;
    let mut timer = IdleTimer::new(30, t0);
    let deadline = timer.deadline().expect("enabled timer has a deadline");

    assert_eq!(
        timer.poll(deadline - 30_500_000),
        Some(IdleEvent::Warn { seconds_left: 31 })
    );
}

#[test]
fn zero_timeout_disables_the_timer() {
    let t0 = 4_000_000_000_000_000;
    let mut timer = IdleTimer::new(0, t0);

    assert_eq!(timer.deadline(), None);
    assert_eq!(timer.next_deadline(), None);
    assert_eq!(timer.poll(t0), None);
    assert_eq!(timer.poll(t0 + 10_000 * MIN), None);
    timer.on_input(t0 + 20_000 * MIN);
    assert_eq!(timer.poll(t0 + 30_000 * MIN), None);
    assert_eq!(timer.next_deadline(), None);
}

#[test]
fn short_timeout_warns_immediately() {
    let t0 = 5_000_000_000_000_000;
    let mut timer = IdleTimer::new(1, t0);

    assert_eq!(timer.poll(t0), Some(IdleEvent::Warn { seconds_left: 60 }));
    assert_eq!(timer.poll(t0 + MIN), Some(IdleEvent::Expire));
}

#[test]
fn a_late_poll_expires_without_a_warning() {
    let t0 = 6_000_000_000_000_000;
    let mut timer = IdleTimer::new(30, t0);

    assert_eq!(timer.poll(t0 + 31 * MIN), Some(IdleEvent::Expire));
}

#[test]
fn timeout_updates_preserve_input_and_allow_a_new_warning() {
    let t0 = 7_000_000_000_000_000;
    let mut timer = IdleTimer::new(30, t0);

    assert_eq!(
        timer.poll(t0 + 29 * MIN),
        Some(IdleEvent::Warn { seconds_left: 60 })
    );
    timer.set_timeout_min(60, t0 + 29 * MIN);
    assert_eq!(timer.deadline(), Some(t0 + 60 * MIN));
    assert_eq!(timer.next_deadline(), Some(t0 + 59 * MIN));
    assert_eq!(timer.poll(t0 + 59 * MIN - 1), None);
    assert_eq!(
        timer.poll(t0 + 59 * MIN),
        Some(IdleEvent::Warn { seconds_left: 60 })
    );

    let mut shortened = IdleTimer::new(30, t0);
    shortened.set_timeout_min(10, t0 + 31 * MIN);
    assert_eq!(shortened.deadline(), Some(t0 + 10 * MIN));
    assert_eq!(shortened.poll(t0 + 31 * MIN), Some(IdleEvent::Expire));

    let mut moved_input = IdleTimer::new(30, t0);
    moved_input.on_input(t0 + 5 * MIN);
    moved_input.set_timeout_min(60, t0 + 6 * MIN);
    assert_eq!(moved_input.deadline(), Some(t0 + 65 * MIN));
}

#[test]
fn next_deadline_tracks_warning_and_expiry_state() {
    let t0 = 8_000_000_000_000_000;
    let mut timer = IdleTimer::new(30, t0);

    assert_eq!(timer.next_deadline(), Some(t0 + 29 * MIN));
    assert_eq!(
        timer.poll(t0 + 29 * MIN),
        Some(IdleEvent::Warn { seconds_left: 60 })
    );
    assert_eq!(timer.next_deadline(), Some(t0 + 30 * MIN));
    assert_eq!(timer.poll(t0 + 30 * MIN), Some(IdleEvent::Expire));
    assert_eq!(timer.next_deadline(), None);
}

#[test]
fn lock_setting_override_wins_until_cleared() {
    let mut lock = LockOnEnd::new(false);
    assert!(!lock.enabled());

    lock.set_override(true);
    assert!(lock.enabled());

    lock.clear_override();
    assert!(!lock.enabled());

    lock.set_config_default(true);
    assert!(lock.enabled());

    lock.set_override(false);
    assert!(!lock.enabled());

    lock.clear_override();
    assert!(lock.enabled());
}

#[test]
fn lock_on_end_truth_table() {
    let lock = LockOnEnd::new(true);
    let cases = [
        (EndReason::UserClosed, true),
        (EndReason::IdleTimeout, true),
        (EndReason::LingerExpired, true),
        (EndReason::Revoked, true),
        (EndReason::Replaced, false),
        (EndReason::Handoff, false),
        (EndReason::ClosedByLocalUser, false),
        (EndReason::HostShutdown, false),
    ];

    for (reason, expected) in cases {
        assert_eq!(
            should_lock_on_end(&lock, reason, true),
            expected,
            "{reason:?}"
        );
        assert!(!should_lock_on_end(&lock, reason, false), "{reason:?}");
    }

    let disabled = LockOnEnd::new(false);
    for (reason, _) in cases {
        assert!(!should_lock_on_end(&disabled, reason, true), "{reason:?}");
    }
}

#[test]
fn session_actions_are_mapped_by_role_and_kind() {
    let viewer_action = pb::SessionAction {
        kind: 99,
        enable: true,
    };
    assert_eq!(
        decide_session_action(false, &viewer_action),
        ActionDecision::Reject(ActionReject::NotController)
    );
    assert_eq!(reject_error(ActionReject::NotController), None);

    for kind in [
        SessionActionKind::LockScreen,
        SessionActionKind::CtrlAltDel,
        SessionActionKind::WakeDisplay,
    ] {
        let action = pb::SessionAction {
            kind: kind as i32,
            enable: false,
        };
        assert_eq!(
            decide_session_action(true, &action),
            ActionDecision::Platform(kind)
        );
    }

    let lock_on_end = pb::SessionAction {
        kind: SessionActionKind::LockOnEnd as i32,
        enable: true,
    };
    assert_eq!(
        decide_session_action(true, &lock_on_end),
        ActionDecision::SetLockOnEnd(true)
    );

    for kind in [0, 99] {
        let action = pb::SessionAction { kind, enable: true };
        assert_eq!(
            decide_session_action(true, &action),
            ActionDecision::Reject(ActionReject::Unsupported)
        );
        let error =
            reject_error(ActionReject::Unsupported).expect("unsupported action has an error");
        assert_eq!(error.code, u32::from(ErrorCode::Other.code()));
        assert_eq!(error.message, "unsupported session action");
    }
}

#[test]
fn platform_failures_use_protocol_error_codes_and_action_detail() {
    let denied = platform_failure_error(
        SessionActionKind::CtrlAltDel,
        &PlatformError::PermissionDenied("accessibility".to_owned()),
    );
    assert_eq!(denied.code, u32::from(ErrorCode::MissingPermissions.code()));
    assert_eq!(
        denied.detail.get("action").map(String::as_str),
        Some("ctrl_alt_del")
    );

    let unsupported =
        platform_failure_error(SessionActionKind::CtrlAltDel, &PlatformError::Unsupported);
    assert_eq!(unsupported.code, u32::from(ErrorCode::Other.code()));
    assert_eq!(
        unsupported.detail.get("action").map(String::as_str),
        Some("ctrl_alt_del")
    );
}

#[test]
fn idle_warning_message_has_the_protocol_fields() {
    let warning = idle_warning_msg(60, true);
    assert_eq!(
        warning,
        pb::IdleWarning {
            seconds_left: 60,
            lock_on_end: true,
        }
    );
}
