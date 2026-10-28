use std::time::Duration;

use rsp_client::recovery::background::{
    BackgroundTimeline, BgAction, BgState, PAUSE_GRACE, RESUME_KEYFRAME_WAIT, RESUME_TTL,
};
use tokio::time::Instant;

fn background_actions(state: &mut BackgroundTimeline, now: Instant) -> Vec<BgAction> {
    state.set_background(true, now)
}

fn assert_paused(state: &BackgroundTimeline, since: Instant) {
    assert_eq!(state.state(), BgState::Paused { since });
}

fn assert_closed(state: &BackgroundTimeline, deadline: Instant) {
    assert_eq!(state.state(), BgState::Closed { deadline });
}

#[test]
fn it09_twenty_seconds_resumes_the_stream() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();

    assert_eq!(
        background_actions(&mut timeline, t0),
        vec![
            BgAction::SendReleaseAll,
            BgAction::SendPauseStream,
            BgAction::SuspendMonitors,
        ]
    );
    assert_paused(&timeline, t0);
    assert_eq!(timeline.poll(t0 + Duration::from_secs(20)), Vec::new());
    assert_eq!(
        timeline.set_background(false, t0 + Duration::from_secs(20)),
        vec![BgAction::SendResumeStream, BgAction::ResumeMonitors]
    );
    assert_eq!(timeline.state(), BgState::Foreground);

    let resumed_at = t0 + Duration::from_secs(20);
    assert_eq!(
        timeline.next_deadline(),
        Some(resumed_at + RESUME_KEYFRAME_WAIT)
    );
    timeline.on_keyframe(resumed_at + Duration::from_millis(300));
    assert_eq!(timeline.poll(t0 + Duration::from_secs(21)), Vec::new());
    assert_eq!(timeline.state(), BgState::Foreground);
    assert_eq!(timeline.next_deadline(), None);
}

#[test]
fn it09_forty_five_seconds_closes_then_starts_token_resume() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();
    background_actions(&mut timeline, t0);

    assert_eq!(
        timeline.poll(t0 + Duration::from_millis(29_999)),
        Vec::new()
    );
    assert_paused(&timeline, t0);
    assert_eq!(timeline.next_deadline(), Some(t0 + PAUSE_GRACE));

    assert_eq!(
        timeline.poll(t0 + PAUSE_GRACE),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::MarkTicketLost {
                at: t0 + PAUSE_GRACE,
            },
        ]
    );
    assert_closed(&timeline, t0 + PAUSE_GRACE + RESUME_TTL);
    assert_eq!(
        timeline.next_deadline(),
        Some(t0 + PAUSE_GRACE + RESUME_TTL)
    );

    assert_eq!(
        timeline.set_background(false, t0 + Duration::from_secs(45)),
        vec![BgAction::StartResume]
    );
    assert_eq!(timeline.state(), BgState::Foreground);
    assert_eq!(timeline.next_deadline(), None);

    let mut late_poll = BackgroundTimeline::new();
    background_actions(&mut late_poll, t0);
    assert_eq!(
        late_poll.poll(t0 + Duration::from_secs(31)),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::MarkTicketLost {
                at: t0 + PAUSE_GRACE,
            },
        ]
    );
    assert_closed(&late_poll, t0 + PAUSE_GRACE + RESUME_TTL);
}

#[test]
fn it09_one_hundred_twenty_seconds_expires_the_token() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();
    background_actions(&mut timeline, t0);
    timeline.poll(t0 + PAUSE_GRACE);

    assert_eq!(
        timeline.poll(t0 + PAUSE_GRACE + RESUME_TTL),
        vec![BgAction::ExpireTicket]
    );
    assert_eq!(timeline.state(), BgState::Expired);
    assert_eq!(timeline.next_deadline(), None);
    assert_eq!(
        timeline.set_background(false, t0 + Duration::from_secs(120)),
        vec![BgAction::ReturnToDevices]
    );
    assert_eq!(timeline.state(), BgState::Expired);
    assert_eq!(
        timeline.set_background(false, t0 + Duration::from_secs(121)),
        Vec::new()
    );
}

#[test]
fn suspended_app_without_polls_uses_planned_timestamps() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();
    background_actions(&mut timeline, t0);

    assert_eq!(
        timeline.set_background(false, t0 + Duration::from_secs(45)),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::MarkTicketLost {
                at: t0 + PAUSE_GRACE,
            },
            BgAction::StartResume,
        ]
    );
    assert_eq!(timeline.state(), BgState::Foreground);

    let mut expired = BackgroundTimeline::new();
    background_actions(&mut expired, t0);
    assert_eq!(
        expired.set_background(false, t0 + Duration::from_secs(120)),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::ExpireTicket,
            BgAction::ReturnToDevices,
        ]
    );
    assert_eq!(expired.state(), BgState::Expired);
    assert_eq!(expired.next_deadline(), None);
    assert_eq!(
        expired.set_background(false, t0 + Duration::from_secs(121)),
        Vec::new()
    );

    let mut polled_after_expiry = BackgroundTimeline::new();
    background_actions(&mut polled_after_expiry, t0);
    assert_eq!(
        polled_after_expiry.poll(t0 + Duration::from_secs(95)),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::ExpireTicket,
        ]
    );
    assert_eq!(polled_after_expiry.state(), BgState::Expired);
    assert_eq!(
        polled_after_expiry.set_background(false, t0 + Duration::from_secs(96)),
        vec![BgAction::ReturnToDevices]
    );
}

#[test]
fn connection_loss_while_paused_starts_token_window_at_loss_time() {
    let t0 = Instant::now();
    let lost_at = t0 + Duration::from_secs(12);
    let mut timeline = BackgroundTimeline::new();
    background_actions(&mut timeline, t0);

    assert_eq!(
        timeline.on_connection_lost(lost_at),
        vec![BgAction::MarkTicketLost { at: lost_at }]
    );
    assert_closed(&timeline, lost_at + RESUME_TTL);
    assert_eq!(timeline.next_deadline(), Some(lost_at + RESUME_TTL));
    assert_eq!(
        timeline.set_background(false, t0 + Duration::from_secs(70)),
        vec![BgAction::StartResume]
    );
    assert_eq!(timeline.state(), BgState::Foreground);

    let mut expired = BackgroundTimeline::new();
    background_actions(&mut expired, t0);
    expired.on_connection_lost(lost_at);
    assert_eq!(
        expired.set_background(false, lost_at + RESUME_TTL),
        vec![BgAction::ExpireTicket, BgAction::ReturnToDevices]
    );
    assert_eq!(expired.state(), BgState::Expired);
}

#[test]
fn foreground_connection_loss_is_left_to_recovery() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();

    assert_eq!(timeline.on_connection_lost(t0), Vec::new());
    assert_eq!(timeline.state(), BgState::Foreground);
    assert_eq!(timeline.next_deadline(), None);
}

#[test]
fn configurable_grace_can_hold_background_sessions() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::with_grace(None);
    assert_eq!(timeline.grace(), None);
    assert_eq!(background_actions(&mut timeline, t0).len(), 3);
    assert_eq!(timeline.poll(t0 + Duration::from_secs(600)), Vec::new());
    assert_eq!(timeline.next_deadline(), None);
    let resumed = t0 + Duration::from_secs(600);
    assert_eq!(
        timeline.set_background(false, resumed),
        vec![BgAction::SendResumeStream, BgAction::ResumeMonitors]
    );
    assert_eq!(
        timeline.next_deadline(),
        Some(resumed + RESUME_KEYFRAME_WAIT)
    );

    let mut lost = BackgroundTimeline::with_grace(None);
    background_actions(&mut lost, t0);
    let lost_at = t0 + Duration::from_secs(7);
    assert_eq!(
        lost.on_connection_lost(lost_at),
        vec![BgAction::MarkTicketLost { at: lost_at }]
    );
    assert_closed(&lost, lost_at + RESUME_TTL);
}

#[test]
fn configurable_short_grace_emits_goodbye() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::with_grace(Some(Duration::from_secs(5)));
    background_actions(&mut timeline, t0);
    assert_eq!(
        timeline.poll(t0 + Duration::from_secs(5)),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::MarkTicketLost {
                at: t0 + Duration::from_secs(5)
            }
        ]
    );
}

#[test]
fn changing_grace_restarts_paused_timer() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();
    background_actions(&mut timeline, t0);
    let held_at = t0 + Duration::from_secs(10);
    timeline.set_grace(None, held_at);
    assert_eq!(timeline.poll(t0 + Duration::from_secs(600)), Vec::new());
    let reset_at = t0 + Duration::from_secs(100);
    timeline.set_grace(Some(PAUSE_GRACE), reset_at);
    assert_eq!(timeline.next_deadline(), Some(reset_at + PAUSE_GRACE));
    assert_eq!(
        timeline.poll(reset_at + PAUSE_GRACE),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::MarkTicketLost {
                at: reset_at + PAUSE_GRACE
            }
        ]
    );
}

#[test]
fn new_timeline_uses_default_grace() {
    assert_eq!(BackgroundTimeline::new().grace(), Some(PAUSE_GRACE));
}

#[test]
fn resume_keyframe_watchdog_fires_once_at_the_deadline() {
    let t0 = Instant::now();
    let resumed_at = t0 + Duration::from_secs(1);
    let deadline = resumed_at + RESUME_KEYFRAME_WAIT;
    let mut timeline = BackgroundTimeline::new();
    background_actions(&mut timeline, t0);
    assert_eq!(
        timeline.set_background(false, resumed_at),
        vec![BgAction::SendResumeStream, BgAction::ResumeMonitors]
    );

    assert_eq!(
        timeline.poll(deadline - Duration::from_millis(1)),
        Vec::new()
    );
    assert_eq!(timeline.next_deadline(), Some(deadline));
    assert_eq!(timeline.poll(deadline), vec![BgAction::RequestKeyframe]);
    assert_eq!(timeline.next_deadline(), None);
    assert_eq!(
        timeline.poll(deadline + Duration::from_millis(100)),
        Vec::new()
    );
}

#[test]
fn repeated_background_events_are_idempotent() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();

    assert_eq!(timeline.set_background(false, t0), Vec::new());
    background_actions(&mut timeline, t0);
    assert_eq!(
        background_actions(&mut timeline, t0 + Duration::from_secs(1)),
        Vec::new()
    );
    assert_paused(&timeline, t0);
    timeline.poll(t0 + PAUSE_GRACE);
    assert_eq!(
        background_actions(&mut timeline, t0 + Duration::from_secs(31)),
        Vec::new()
    );
    assert_closed(&timeline, t0 + PAUSE_GRACE + RESUME_TTL);
}

#[test]
fn quick_round_trip_starts_a_fresh_pause_window() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();
    background_actions(&mut timeline, t0);
    assert_eq!(
        timeline.set_background(false, t0 + Duration::from_secs(1)),
        vec![BgAction::SendResumeStream, BgAction::ResumeMonitors]
    );
    timeline.on_keyframe(t0 + Duration::from_millis(1_100));

    let second_background = t0 + Duration::from_secs(2);
    assert_eq!(
        background_actions(&mut timeline, second_background),
        vec![
            BgAction::SendReleaseAll,
            BgAction::SendPauseStream,
            BgAction::SuspendMonitors,
        ]
    );
    assert_paused(&timeline, second_background);
    assert_eq!(
        timeline.next_deadline(),
        Some(second_background + PAUSE_GRACE)
    );
    assert_eq!(
        timeline.poll(second_background + PAUSE_GRACE - Duration::from_millis(1)),
        Vec::new()
    );
    assert_eq!(
        timeline.poll(second_background + PAUSE_GRACE),
        vec![
            BgAction::SendGoodbyeBackground,
            BgAction::CloseConnection,
            BgAction::MarkTicketLost {
                at: second_background + PAUSE_GRACE,
            },
        ]
    );
}

#[test]
fn next_deadline_tracks_each_pending_timer() {
    let t0 = Instant::now();
    let mut timeline = BackgroundTimeline::new();
    assert_eq!(timeline.next_deadline(), None);

    background_actions(&mut timeline, t0);
    assert_eq!(timeline.next_deadline(), Some(t0 + PAUSE_GRACE));

    timeline.poll(t0 + PAUSE_GRACE);
    assert_eq!(
        timeline.next_deadline(),
        Some(t0 + PAUSE_GRACE + RESUME_TTL)
    );

    let mut resumed = BackgroundTimeline::new();
    background_actions(&mut resumed, t0);
    let resume_at = t0 + Duration::from_secs(5);
    resumed.set_background(false, resume_at);
    assert_eq!(
        resumed.next_deadline(),
        Some(resume_at + RESUME_KEYFRAME_WAIT)
    );
    resumed.poll(resume_at + RESUME_KEYFRAME_WAIT);
    assert_eq!(resumed.next_deadline(), None);
}
