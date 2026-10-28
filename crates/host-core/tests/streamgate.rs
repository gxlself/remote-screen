use host_core::session::roster::{
    ConnKey, ParticipantInfo, Roster, RosterConfig, RosterError, RosterRole, SessionPrefs,
    LINGER_US,
};
use host_core::session::streamgate::{
    auth_denied, auth_ok, capture_needed, resume_reply, GateAction, GateState, ResumeReply,
    StreamGate, MAX_PAUSE_US,
};
use rsp_core::errors::ErrorCode;
use rsp_core::pb;

const T0: u64 = 1_726_000_000_000_000;
const SECOND: u64 = 1_000_000;

fn info(device: &str) -> ParticipantInfo {
    ParticipantInfo {
        device_id: device.to_owned(),
        device_name: format!("{device}-name"),
        user: format!("{device}@"),
        path: pb::PathKind::TailnetDirect,
        can_control: true,
    }
}

fn roster() -> Roster {
    Roster::new(RosterConfig::default())
}

fn start_actions() -> Vec<GateAction> {
    vec![
        GateAction::StartEncoder,
        GateAction::ForceKeyframe,
        GateAction::SendCursorShape,
    ]
}

#[test]
fn pause_fallback_default_is_45_seconds() {
    assert_eq!(MAX_PAUSE_US, 45 * SECOND);
}

fn live_gate() -> StreamGate {
    let mut gate = StreamGate::new(MAX_PAUSE_US);
    assert_eq!(gate.on_start_stream(T0), start_actions());
    gate
}

#[test]
fn start_stream_starts_once_then_requests_a_keyframe() {
    let mut gate = StreamGate::new(MAX_PAUSE_US);
    assert_eq!(gate.state(), GateState::Idle);
    assert!(!gate.wants_frames());
    assert_eq!(gate.next_deadline(), None);

    assert_eq!(gate.on_start_stream(T0), start_actions());
    assert_eq!(gate.state(), GateState::Live);
    assert!(gate.wants_frames());
    assert_eq!(gate.next_deadline(), None);

    assert_eq!(
        gate.on_start_stream(T0 + SECOND),
        vec![GateAction::ForceKeyframe]
    );
    assert_eq!(gate.state(), GateState::Live);
}

#[test]
fn pause_is_idempotent_and_stops_frames() {
    let mut gate = live_gate();
    assert_eq!(gate.on_pause(T0), vec![GateAction::StopEncoder]);
    assert_eq!(gate.state(), GateState::Paused { since_us: T0 });
    assert!(!gate.wants_frames());

    assert_eq!(gate.on_pause(T0 + SECOND), Vec::new());
    assert_eq!(
        gate.state(),
        GateState::Paused { since_us: T0 },
        "a repeated pause must not move the deadline"
    );
}

#[test]
fn resume_restarts_encoder_and_sends_cursor_shape() {
    let mut gate = live_gate();
    assert_eq!(gate.on_pause(T0), vec![GateAction::StopEncoder]);

    assert_eq!(gate.on_resume(T0 + 20 * SECOND), start_actions());
    assert_eq!(gate.state(), GateState::Live);
    assert!(gate.wants_frames());
}

#[test]
fn resume_while_live_forces_a_keyframe_and_idle_resume_is_ignored() {
    let mut live = live_gate();
    assert_eq!(live.on_resume(T0 + SECOND), vec![GateAction::ForceKeyframe]);
    assert_eq!(live.state(), GateState::Live);

    let mut idle = StreamGate::new(MAX_PAUSE_US);
    assert_eq!(idle.on_resume(T0), Vec::new());
    assert_eq!(idle.state(), GateState::Idle);
    assert!(!idle.wants_frames());
}

#[test]
fn pause_before_start_defers_encoder_start_until_resume() {
    let mut gate = StreamGate::new(MAX_PAUSE_US);
    assert_eq!(gate.on_pause(T0), Vec::new());
    assert_eq!(gate.state(), GateState::Paused { since_us: T0 });

    assert_eq!(gate.on_start_stream(T0 + SECOND), Vec::new());
    assert_eq!(gate.state(), GateState::Paused { since_us: T0 });
    assert!(!gate.wants_frames());

    assert_eq!(gate.on_resume(T0 + 2 * SECOND), start_actions());
    assert_eq!(gate.state(), GateState::Live);
    assert!(gate.wants_frames());
}

#[test]
fn pause_without_start_returns_to_idle_on_resume() {
    let mut gate = StreamGate::new(MAX_PAUSE_US);
    assert_eq!(gate.on_pause(T0), Vec::new());
    assert_eq!(gate.on_resume(T0 + SECOND), Vec::new());
    assert_eq!(gate.state(), GateState::Idle);
    assert!(!gate.wants_frames());
}

#[test]
fn pause_fallback_resumes_streaming_at_the_deadline() {
    let mut gate = live_gate();
    assert_eq!(gate.on_pause(T0), vec![GateAction::StopEncoder]);
    assert_eq!(gate.next_deadline(), Some(T0 + MAX_PAUSE_US));

    assert_eq!(gate.poll(T0 + MAX_PAUSE_US - 1), Vec::new());
    assert_eq!(gate.next_deadline(), Some(T0 + MAX_PAUSE_US));
    assert_eq!(gate.poll(T0 + MAX_PAUSE_US), start_actions());
    assert_eq!(gate.state(), GateState::Live);
    assert!(gate.wants_frames());
    assert_eq!(gate.next_deadline(), None);
    assert_eq!(gate.poll(T0 + MAX_PAUSE_US + SECOND), Vec::new());
}

#[test]
fn pause_fallback_without_start_returns_to_idle() {
    let mut gate = StreamGate::new(MAX_PAUSE_US);
    assert_eq!(gate.on_pause(T0), Vec::new());
    assert_eq!(gate.poll(T0 + MAX_PAUSE_US), Vec::new());
    assert_eq!(gate.state(), GateState::Idle);
    assert!(!gate.wants_frames());
    assert_eq!(gate.next_deadline(), None);
}

#[test]
fn zero_pause_limit_disables_the_fallback() {
    let mut gate = StreamGate::new(0);
    assert_eq!(gate.on_start_stream(T0), start_actions());
    assert_eq!(gate.on_pause(T0 + SECOND), vec![GateAction::StopEncoder]);

    assert_eq!(gate.next_deadline(), None);
    assert_eq!(gate.poll(T0 + 10 * 60 * SECOND), Vec::new());
    assert_eq!(gate.poll(u64::MAX), Vec::new());
}

#[test]
fn clock_going_backwards_is_treated_as_zero_elapsed() {
    let mut gate = live_gate();
    assert_eq!(gate.on_pause(T0), vec![GateAction::StopEncoder]);
    assert_eq!(gate.poll(T0 - SECOND), Vec::new());
    assert_eq!(gate.next_deadline(), Some(T0 + MAX_PAUSE_US));
}

#[test]
fn capture_is_needed_while_any_connection_is_live() {
    let live = live_gate();

    let mut paused = live_gate();
    assert_eq!(paused.on_pause(T0), vec![GateAction::StopEncoder]);

    let idle = StreamGate::new(MAX_PAUSE_US);
    assert!(capture_needed([&live, &paused, &idle]));
    assert!(!capture_needed([&paused, &idle]));
    assert!(!capture_needed(std::iter::empty()));
}

#[test]
fn resumed_gate_is_live_and_starts_the_stream() {
    let (gate, actions) = StreamGate::resumed(MAX_PAUSE_US);
    assert_eq!(actions, start_actions());
    assert_eq!(gate.state(), GateState::Live);
    assert!(gate.wants_frames());
    assert_eq!(gate.next_deadline(), None);
}

#[test]
fn lingering_controller_resume_returns_a_fresh_authenticated_session() {
    let mut roster = roster();
    let admitted = roster
        .admit(ConnKey(1), info("device-a"), RosterRole::Controller, T0)
        .expect("admit controller");
    let old_token = admitted.token;
    let prefs = SessionPrefs {
        display_id: 7,
        quality: 2,
    };
    roster
        .set_prefs(&admitted.session_id, prefs)
        .expect("save session preferences");
    roster
        .leave(ConnKey(1), true, T0 + SECOND)
        .expect("linger controller");

    let reply = resume_reply(roster.resume(ConnKey(2), &old_token, "device-a", T0 + 2 * SECOND));
    let ResumeReply::Resumed {
        auth,
        prefs: resumed_prefs,
        replaced,
        release_all,
    } = reply
    else {
        panic!("expected a successful resume");
    };

    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert!(auth.resumed);
    assert_eq!(auth.role, pb::Role::Controller as i32);
    assert_eq!(auth.session_token.len(), 16);
    assert_ne!(auth.session_token, old_token);
    assert_eq!(resumed_prefs, prefs);
    assert_eq!(replaced, None);
    assert!(!release_all);
}

#[test]
fn active_controller_resume_replaces_connection_and_releases_input() {
    let mut roster = roster();
    let admitted = roster
        .admit(ConnKey(1), info("device-a"), RosterRole::Controller, T0)
        .expect("admit controller");

    let reply = resume_reply(roster.resume(ConnKey(2), &admitted.token, "device-a", T0 + SECOND));
    let ResumeReply::Resumed {
        auth,
        replaced,
        release_all,
        ..
    } = reply
    else {
        panic!("expected an active resume");
    };

    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert!(auth.resumed);
    assert_eq!(replaced, Some(ConnKey(1)));
    assert!(release_all, "a replaced controller may still hold keys");
}

#[test]
fn active_viewer_resume_does_not_release_input() {
    let mut roster = roster();
    roster
        .admit(ConnKey(1), info("controller"), RosterRole::Controller, T0)
        .expect("admit controller");
    let admitted = roster
        .admit(ConnKey(2), info("viewer"), RosterRole::Viewer, T0 + SECOND)
        .expect("admit viewer");

    let reply = resume_reply(roster.resume(ConnKey(3), &admitted.token, "viewer", T0 + 2 * SECOND));
    let ResumeReply::Resumed {
        auth,
        replaced,
        release_all,
        ..
    } = reply
    else {
        panic!("expected an active viewer resume");
    };

    assert_eq!(auth.role, pb::Role::Viewer as i32);
    assert_eq!(replaced, Some(ConnKey(2)));
    assert!(!release_all, "viewers never inject input");
}

#[test]
fn replay_and_expired_tokens_fall_back_to_a_fresh_session() {
    let mut roster = roster();
    let admitted = roster
        .admit(ConnKey(1), info("device-a"), RosterRole::Controller, T0)
        .expect("admit controller");
    let token = admitted.token;
    roster
        .leave(ConnKey(1), true, T0 + SECOND)
        .expect("linger controller");

    let first = roster
        .resume(ConnKey(2), &token, "device-a", T0 + 2 * SECOND)
        .expect("first resume");
    roster
        .leave(ConnKey(2), true, T0 + 3 * SECOND)
        .expect("linger again");
    assert_eq!(
        resume_reply(roster.resume(ConnKey(3), &token, "device-a", T0 + 4 * SECOND)),
        ResumeReply::FreshSession {
            reason: "invalid_token"
        },
        "SEC-12: a resume token is one-time"
    );

    assert_eq!(
        resume_reply(roster.resume(
            ConnKey(4),
            &first.token,
            "device-a",
            T0 + 3 * SECOND + LINGER_US
        )),
        ResumeReply::FreshSession {
            reason: "invalid_token"
        },
        "the rotated token expires after the 60 s linger window"
    );
}

#[test]
fn token_from_another_device_falls_back_to_fresh_session() {
    let mut roster = roster();
    let admitted = roster
        .admit(ConnKey(1), info("device-a"), RosterRole::Controller, T0)
        .expect("admit controller");

    assert_eq!(
        resume_reply(roster.resume(ConnKey(2), &admitted.token, "device-b", T0 + SECOND)),
        ResumeReply::FreshSession {
            reason: "device_mismatch"
        }
    );
}

#[test]
fn full_viewer_roster_is_denied_with_142() {
    let mut roster = roster();
    roster
        .admit(ConnKey(1), info("controller"), RosterRole::Controller, T0)
        .expect("admit controller");
    let lingering = roster
        .admit(
            ConnKey(2),
            info("viewer-a"),
            RosterRole::Viewer,
            T0 + SECOND,
        )
        .expect("admit viewer a");
    roster
        .admit(
            ConnKey(3),
            info("viewer-b"),
            RosterRole::Viewer,
            T0 + 2 * SECOND,
        )
        .expect("admit viewer b");
    roster
        .admit(
            ConnKey(4),
            info("viewer-c"),
            RosterRole::Viewer,
            T0 + 3 * SECOND,
        )
        .expect("admit viewer c");
    roster
        .leave(ConnKey(2), true, T0 + 4 * SECOND)
        .expect("linger viewer a");
    roster
        .admit(
            ConnKey(5),
            info("filler"),
            RosterRole::Viewer,
            T0 + 5 * SECOND,
        )
        .expect("fill the missing viewer slot");

    let reply =
        resume_reply(roster.resume(ConnKey(6), &lingering.token, "viewer-a", T0 + 6 * SECOND));
    let ResumeReply::Deny { auth } = reply else {
        panic!("expected a denied recovery");
    };

    assert_eq!(auth.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(
        auth.error.as_ref().map(|error| error.code),
        Some(u32::from(ErrorCode::SessionFull.code()))
    );
    assert_eq!(auth.error.as_ref().map(|error| error.code), Some(142));
}

#[test]
fn auth_result_constructors_set_protocol_fields() {
    let token = [7_u8; 16];
    let auth = auth_ok(RosterRole::Viewer, &token, false);
    assert_eq!(auth.status, pb::auth_result::Status::Ok as i32);
    assert_eq!(auth.role, pb::Role::Viewer as i32);
    assert_eq!(auth.session_token, token.to_vec());
    assert!(!auth.resumed);
    assert_eq!(auth.approval_timeout_s, 0);
    assert_eq!(auth.error, None);

    let denied = auth_denied(ErrorCode::Other);
    assert_eq!(denied.status, pb::auth_result::Status::Denied as i32);
    assert_eq!(denied.error.as_ref().map(|error| error.code), Some(199));
    assert!(!denied.resumed);
    assert!(denied.session_token.is_empty());
}

#[test]
fn random_failure_is_denied_with_199_and_unknown_errors_are_fresh() {
    let ResumeReply::Deny { auth } = resume_reply(Err(RosterError::Random)) else {
        panic!("random failures must deny the resume");
    };
    assert_eq!(auth.error.as_ref().map(|error| error.code), Some(199));

    assert_eq!(
        resume_reply(Err(RosterError::UnknownSession)),
        ResumeReply::FreshSession { reason: "other" }
    );
}
