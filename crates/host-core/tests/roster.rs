use std::collections::HashSet;

use host_core::auth::SessionSnapshot;
use host_core::ipc::SessionRole;
use host_core::session::roster::{
    participant_pb, ConnKey, Handoff, ParticipantInfo, ParticipantView, Roster, RosterConfig,
    RosterError, RosterRole, SessionId, SessionPrefs, LINGER_US, TOKEN_LEN,
};
use rsp_core::pb::{self, Role};

const T0: u64 = 1_000_000;

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

fn admit(
    roster: &mut Roster,
    conn: u64,
    device: &str,
    role: RosterRole,
    now: u64,
) -> (SessionId, [u8; TOKEN_LEN], RosterRole) {
    let admitted = roster
        .admit(ConnKey(conn), info(device), role, now)
        .expect("admit");
    (admitted.session_id, admitted.token, admitted.role)
}

fn assert_invariants(roster: &Roster) {
    let participants = roster.participants();
    let controllers = participants
        .iter()
        .filter(|participant| participant.role == RosterRole::Controller)
        .count();
    assert!(controllers <= 1, "more than one controller");

    let mut devices = HashSet::new();
    for participant in &participants {
        assert!(
            devices.insert(participant.info.device_id.clone()),
            "duplicate device {}",
            participant.info.device_id
        );
    }

    let active_viewers = participants
        .iter()
        .filter(|participant| participant.role == RosterRole::Viewer && !participant.lingering)
        .count();
    assert!(active_viewers <= 3, "too many active viewers");
}

fn pb_participant(view: &ParticipantView) -> pb::Participant {
    participant_pb(view)
}

#[test]
fn controller_then_controller_request_becomes_viewer() {
    let mut roster = roster();
    let (controller, _, role) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    assert_eq!(role, RosterRole::Controller);
    let (_, _, role) = admit(&mut roster, 2, "B", RosterRole::Controller, T0 + 1);
    assert_eq!(role, RosterRole::Viewer);
    assert_eq!(
        roster.controller().map(|view| view.session_id),
        Some(controller)
    );
    assert_eq!(
        roster.auth_snapshot("C"),
        SessionSnapshot {
            has_controller: true,
            viewer_count: 1,
        }
    );
    assert_invariants(&roster);
}

#[test]
fn viewer_capacity_is_enforced_before_mutation_and_clamped() {
    let mut roster = roster();
    admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    admit(&mut roster, 2, "B", RosterRole::Viewer, T0 + 1);
    admit(&mut roster, 3, "C", RosterRole::Viewer, T0 + 2);
    admit(&mut roster, 4, "D", RosterRole::Viewer, T0 + 3);
    let before = roster.participants();
    assert_eq!(
        roster.admit(ConnKey(5), info("E"), RosterRole::Viewer, T0 + 4),
        Err(RosterError::SessionFull)
    );
    assert_eq!(roster.participants(), before);

    let mut single = Roster::new(RosterConfig::default());
    single.set_max_viewers(1);
    admit(&mut single, 20, "A", RosterRole::Controller, T0);
    admit(&mut single, 21, "B", RosterRole::Viewer, T0 + 1);
    assert_eq!(
        single.admit(ConnKey(22), info("C"), RosterRole::Viewer, T0 + 2),
        Err(RosterError::SessionFull)
    );
    assert_invariants(&single);

    let mut fresh = Roster::new(RosterConfig::default());
    fresh.set_max_viewers(9);
    admit(&mut fresh, 10, "A", RosterRole::Controller, T0);
    admit(&mut fresh, 11, "B", RosterRole::Viewer, T0 + 1);
    admit(&mut fresh, 12, "C", RosterRole::Viewer, T0 + 2);
    admit(&mut fresh, 13, "D", RosterRole::Viewer, T0 + 3);
    assert_eq!(
        fresh.admit(ConnKey(14), info("E"), RosterRole::Viewer, T0 + 4),
        Err(RosterError::SessionFull)
    );
    assert_invariants(&fresh);
}

#[test]
fn view_only_device_cannot_control() {
    let mut roster = roster();
    let mut view_only = info("A");
    view_only.can_control = false;
    let admitted = roster
        .admit(ConnKey(1), view_only, RosterRole::Controller, T0)
        .expect("admit");
    assert_eq!(admitted.role, RosterRole::Viewer);
    assert_eq!(
        roster.set_controller(&admitted.session_id),
        Err(RosterError::ViewOnlyDevice)
    );
    assert_invariants(&roster);
}

#[test]
fn same_device_replacement_changes_session_id_and_invalidates_old_token() {
    let mut roster = roster();
    let (old_session, old_token, role) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    assert_eq!(role, RosterRole::Controller);
    let admitted = roster
        .admit(ConnKey(2), info("A"), RosterRole::Viewer, T0 + 1)
        .expect("replace");
    assert_eq!(admitted.replaced, Some(ConnKey(1)));
    assert_eq!(admitted.role, RosterRole::Controller);
    assert_ne!(admitted.session_id, old_session);
    assert_eq!(
        roster.resume(ConnKey(3), &old_token, "A", T0 + 2),
        Err(RosterError::InvalidToken)
    );
    assert_eq!(roster.session_of(ConnKey(1)), None);
    assert_eq!(roster.session_of(ConnKey(2)), Some(admitted.session_id));
    assert_invariants(&roster);
}

#[test]
fn same_device_reconnect_does_not_count_itself_against_capacity() {
    let mut roster = roster();
    admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    admit(&mut roster, 2, "B", RosterRole::Viewer, T0 + 1);
    admit(&mut roster, 3, "C", RosterRole::Viewer, T0 + 2);
    admit(&mut roster, 4, "D", RosterRole::Viewer, T0 + 3);
    let admitted = roster
        .admit(ConnKey(5), info("D"), RosterRole::Viewer, T0 + 4)
        .expect("self replacement fits");
    assert_eq!(admitted.replaced, Some(ConnKey(4)));
    assert_eq!(roster.auth_snapshot("D").viewer_count, 2);
    assert_invariants(&roster);
}

#[test]
fn auth_snapshot_excludes_own_device() {
    let mut roster = roster();
    admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    assert_eq!(
        roster.auth_snapshot("A"),
        SessionSnapshot {
            has_controller: false,
            viewer_count: 0,
        }
    );
}

#[test]
fn controller_linger_blocks_controller_slot_until_expiry() {
    let mut roster = roster();
    let (controller, _, _) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    let left = roster.leave(ConnKey(1), true, T0).expect("leave");
    assert!(left.lingering);
    assert!(left.was_controller);
    assert_eq!(
        roster.auth_snapshot("B"),
        SessionSnapshot {
            has_controller: true,
            viewer_count: 0,
        }
    );
    assert_eq!(
        roster.lingering_controller().map(|view| view.session_id),
        Some(controller)
    );
    let (_, _, role) = admit(&mut roster, 2, "B", RosterRole::Controller, T0 + 1);
    assert_eq!(role, RosterRole::Viewer);
    assert!(roster.tick(T0 + LINGER_US - 1).is_empty());
    let expired = roster.tick(T0 + LINGER_US);
    assert_eq!(expired.len(), 1);
    assert!(expired[0].was_controller);
    assert_eq!(expired[0].device_id, "A");
    assert_eq!(roster.last_controller_user().as_deref(), Some("A@"));
    let (_, _, role) = admit(
        &mut roster,
        3,
        "C",
        RosterRole::Controller,
        T0 + LINGER_US + 1,
    );
    assert_eq!(role, RosterRole::Controller);
    assert_eq!(roster.last_controller_user(), None);
    assert_invariants(&roster);
}

#[test]
fn resume_lingering_controller_returns_saved_preferences_and_rotates_token() {
    let mut roster = roster();
    let (session, old_token, _) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    let prefs = SessionPrefs {
        display_id: 7,
        quality: 2,
    };
    roster.set_prefs(&session, prefs).expect("prefs");
    roster.leave(ConnKey(1), true, T0 + 10).expect("leave");

    assert_eq!(
        roster.resume(ConnKey(2), &old_token, "B", T0 + 20),
        Err(RosterError::TokenDeviceMismatch)
    );
    let resumed = roster
        .resume(ConnKey(3), &old_token, "A", T0 + 20)
        .expect("resume");
    assert_eq!(resumed.session_id, session);
    assert_eq!(resumed.role, RosterRole::Controller);
    assert_ne!(resumed.token, old_token);
    assert_eq!(resumed.prefs, prefs);
    assert_eq!(resumed.replaced, None);
    assert_eq!(
        roster.resume(ConnKey(4), &old_token, "A", T0 + 21),
        Err(RosterError::InvalidToken)
    );
    assert_eq!(
        roster.participants()[0].since_us,
        T0,
        "since must be preserved"
    );

    let (dead_session, dead_token, _) = admit(&mut roster, 5, "C", RosterRole::Viewer, T0 + 30);
    roster
        .leave(ConnKey(5), true, T0 + 40)
        .expect("leave viewer");
    assert_eq!(
        roster.resume(ConnKey(6), &dead_token, "C", T0 + 40 + LINGER_US),
        Err(RosterError::InvalidToken)
    );
    assert!(roster
        .participants()
        .iter()
        .all(|view| view.session_id != dead_session));
    assert_invariants(&roster);
}

#[test]
fn active_resume_replaces_the_old_connection() {
    let mut roster = roster();
    let (session, token, _) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    let resumed = roster
        .resume(ConnKey(2), &token, "A", T0 + 5)
        .expect("resume");
    assert_eq!(resumed.replaced, Some(ConnKey(1)));
    assert_eq!(resumed.session_id, session);
    assert_eq!(roster.session_of(ConnKey(1)), None);
    assert_eq!(roster.session_of(ConnKey(2)), Some(session));
    assert_invariants(&roster);
}

#[test]
fn lingering_viewer_does_not_hold_a_slot() {
    let mut roster = roster();
    admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    let (_, viewer_token, _) = admit(&mut roster, 2, "B", RosterRole::Viewer, T0 + 1);
    admit(&mut roster, 3, "C", RosterRole::Viewer, T0 + 2);
    admit(&mut roster, 4, "D", RosterRole::Viewer, T0 + 3);
    roster.leave(ConnKey(2), true, T0 + 4).expect("leave");
    assert_eq!(roster.auth_snapshot("B").viewer_count, 2);
    admit(&mut roster, 5, "E", RosterRole::Viewer, T0 + 5);
    assert_eq!(
        roster.resume(ConnKey(6), &viewer_token, "B", T0 + 6),
        Err(RosterError::SessionFull)
    );
    assert!(roster
        .participants()
        .iter()
        .any(|view| view.info.device_id == "B" && view.lingering));
    let leaving = roster.sessions_for_device("E")[0].clone();
    assert_eq!(roster.remove(&leaving), Some(ConnKey(5)));
    let resumed = roster
        .resume(ConnKey(7), &viewer_token, "B", T0 + 7)
        .expect("resume after slot frees");
    assert_eq!(resumed.role, RosterRole::Viewer);
    assert_invariants(&roster);
}

#[test]
fn controller_handoff_swaps_roles_and_survives_linger() {
    let mut roster = roster();
    let (controller, _, _) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    let (viewer, viewer_token, _) = admit(&mut roster, 2, "B", RosterRole::Viewer, T0 + 1);
    let handoff = roster.set_controller(&viewer).expect("handoff");
    assert_eq!(
        handoff,
        Handoff {
            from: Some(controller.clone()),
            to: viewer.clone()
        }
    );
    assert_eq!(roster.role_of(&controller), Some(RosterRole::Viewer));
    assert_eq!(roster.role_of(&viewer), Some(RosterRole::Controller));
    let handoff = roster.set_controller(&viewer).expect("already controller");
    assert_eq!(
        handoff,
        Handoff {
            from: None,
            to: viewer.clone()
        }
    );

    let (target, _, _) = admit(&mut roster, 3, "C", RosterRole::Viewer, T0 + 2);
    roster
        .leave(ConnKey(2), true, T0 + 3)
        .expect("linger controller");
    let handoff = roster.set_controller(&target).expect("handoff from linger");
    assert_eq!(handoff.from, Some(viewer.clone()));
    let resumed = roster
        .resume(ConnKey(4), &viewer_token, "B", T0 + 4)
        .expect("resume demoted controller");
    assert_eq!(resumed.role, RosterRole::Viewer);
    assert_eq!(resumed.replaced, None);
    assert_eq!(
        roster.set_controller(&SessionId("s0000000000000000".into())),
        Err(RosterError::UnknownSession)
    );
    assert_invariants(&roster);
}

#[test]
fn set_view_only_demotes_or_rejects_when_full() {
    let mut roster = roster();
    let (controller, _, _) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    roster.set_view_only(&controller).expect("demote");
    assert_eq!(roster.role_of(&controller), Some(RosterRole::Viewer));
    assert_eq!(roster.controller(), None);

    let mut full = Roster::new(RosterConfig::default());
    let (controller, _, _) = admit(&mut full, 2, "B", RosterRole::Controller, T0 + 1);
    admit(&mut full, 3, "C", RosterRole::Viewer, T0 + 2);
    admit(&mut full, 4, "D", RosterRole::Viewer, T0 + 3);
    admit(&mut full, 5, "E", RosterRole::Viewer, T0 + 4);
    let before = full.participants();
    assert_eq!(
        full.set_view_only(&controller),
        Err(RosterError::SessionFull)
    );
    assert_eq!(full.participants(), before);
    assert_invariants(&full);
    assert_invariants(&roster);
}

#[test]
fn session_state_reports_roles_and_sorted_viewers() {
    let mut roster = roster();
    let (controller, _, _) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    let (viewer_b, _, _) = admit(&mut roster, 2, "B", RosterRole::Viewer, T0 + 5);
    let (viewer_c, _, _) = admit(&mut roster, 3, "C", RosterRole::Viewer, T0 + 2);

    let state = roster
        .session_state_for(&controller, true)
        .expect("controller state");
    assert_eq!(state.your_role, Role::Controller as i32);
    assert_eq!(
        state.controller,
        Some(pb_participant(
            &roster.controller().expect("controller view")
        ))
    );
    assert_eq!(
        state
            .viewers
            .iter()
            .map(|participant| participant.device_id.as_str())
            .collect::<Vec<_>>(),
        ["C", "B"]
    );
    assert!(state.input_paused_local);

    let state = roster
        .session_state_for(&viewer_b, false)
        .expect("viewer state");
    assert_eq!(state.your_role, Role::Viewer as i32);
    assert!(!state.input_paused_local);
    let state = roster
        .session_state_for(&viewer_c, false)
        .expect("viewer c state");
    assert_eq!(state.your_role, Role::Viewer as i32);
    assert_eq!(state.viewers.len(), 2);

    roster.leave(ConnKey(1), true, T0 + 10).expect("linger");
    assert_eq!(
        roster
            .session_state_for(&controller, false)
            .unwrap()
            .controller,
        None
    );
    assert_invariants(&roster);
}

#[test]
fn summaries_and_token_lookup_only_cover_active_entries() {
    let mut roster = roster();
    let (controller, controller_token, _) = admit(&mut roster, 1, "A", RosterRole::Controller, T0);
    let (viewer, viewer_token, _) = admit(&mut roster, 2, "B", RosterRole::Viewer, T0 + 1);
    let summaries = roster.summaries();
    assert_eq!(summaries.len(), 2);
    assert_eq!(summaries[0].session_id, controller.0);
    assert_eq!(summaries[0].device_id, "A");
    assert_eq!(summaries[0].device_name, "A-name");
    assert_eq!(summaries[0].role, SessionRole::Controller);
    assert_eq!(summaries[0].connected_at_us, T0);
    assert_eq!(summaries[1].session_id, viewer.0);
    assert_eq!(summaries[1].role, SessionRole::Viewer);

    assert_eq!(roster.by_token(&controller_token), Some(controller.clone()));
    assert_eq!(roster.by_token(&controller_token), Some(controller));
    roster.leave(ConnKey(2), true, T0 + 2).expect("linger");
    assert_eq!(roster.by_token(&viewer_token), None);
    assert_eq!(roster.summaries().len(), 1);
    assert_eq!(roster.by_token(&[0_u8; 15]), None);
    assert_invariants(&roster);
}

#[test]
fn scripted_sequence_preserves_invariants_at_every_step() {
    let mut roster = roster();
    let mut time = T0;
    let mut tokens = Vec::new();

    let (a, a_token, _) = admit(&mut roster, 1, "A", RosterRole::Controller, time);
    tokens.push(a_token);
    assert_invariants(&roster);
    time += 1;
    let (b, b_token, _) = admit(&mut roster, 2, "B", RosterRole::Viewer, time);
    tokens.push(b_token);
    assert_invariants(&roster);
    time += 1;
    roster.leave(ConnKey(1), true, time).expect("linger A");
    assert_invariants(&roster);
    time += 1;
    let (c, c_token, _) = admit(&mut roster, 3, "C", RosterRole::Controller, time);
    assert_eq!(roster.role_of(&c), Some(RosterRole::Viewer));
    tokens.push(c_token);
    assert_invariants(&roster);
    time += 1;
    roster.set_controller(&b).expect("handoff to B");
    assert_eq!(roster.role_of(&b), Some(RosterRole::Controller));
    assert_invariants(&roster);
    time += 1;
    let resumed_a = roster
        .resume(ConnKey(4), &a_token, "A", time)
        .expect("resume A");
    assert_eq!(resumed_a.role, RosterRole::Viewer);
    assert_eq!(resumed_a.replaced, None);
    assert_invariants(&roster);
    time += 1;
    let (d, d_token, _) = admit(&mut roster, 5, "D", RosterRole::Viewer, time);
    tokens.push(d_token);
    assert_invariants(&roster);
    time += 1;
    roster
        .leave(ConnKey(3), true, time)
        .expect("linger viewer C");
    assert_invariants(&roster);
    time += 1;
    let (e, e_token, _) = admit(&mut roster, 6, "E", RosterRole::Viewer, time);
    tokens.push(e_token);
    assert_invariants(&roster);
    time += 1;
    assert_eq!(
        roster.admit(ConnKey(7), info("F"), RosterRole::Viewer, time),
        Err(RosterError::SessionFull)
    );
    assert_invariants(&roster);
    time += 1;
    roster.set_controller(&e).expect("handoff to E");
    assert_eq!(roster.role_of(&b), Some(RosterRole::Viewer));
    assert_invariants(&roster);
    time += 1;
    assert_eq!(
        roster.set_view_only(&e),
        Err(RosterError::SessionFull),
        "E cannot be demoted while all viewer slots are live"
    );
    assert_invariants(&roster);
    time += 1;
    roster.remove(&d).expect("remove D");
    roster.set_view_only(&e).expect("demote E now");
    assert_eq!(roster.role_of(&e), Some(RosterRole::Viewer));
    assert_eq!(roster.controller(), None);
    assert_invariants(&roster);
    time += 1;
    assert_eq!(roster.leave(ConnKey(99), true, time), None);
    assert_invariants(&roster);
    time += 1;
    assert_eq!(
        roster.resume(ConnKey(8), &[1_u8; TOKEN_LEN], "B", time),
        Err(RosterError::InvalidToken)
    );
    assert_invariants(&roster);
    time += 1;
    let resumed_b = roster
        .resume(ConnKey(9), &b_token, "B", time)
        .expect("active resume B");
    assert_eq!(resumed_b.role, RosterRole::Viewer);
    assert_eq!(resumed_b.replaced, Some(ConnKey(2)));
    assert_invariants(&roster);
    time += 1;
    roster.remove(&a).expect("remove A");
    assert_invariants(&roster);
    time += 1;
    assert!(
        roster.remove(&c).is_none(),
        "lingering entries have no conn"
    );
    assert!(roster
        .participants()
        .iter()
        .all(|participant| participant.session_id != c));
    assert_invariants(&roster);
    time += 1;
    let (g, g_token, _) = admit(&mut roster, 10, "G", RosterRole::Controller, time);
    tokens.push(g_token);
    assert_invariants(&roster);
    time += 1;
    roster.leave(ConnKey(10), true, time).expect("linger G");
    assert_invariants(&roster);
    time += 1;
    let expired = roster.tick(time + LINGER_US);
    assert_eq!(expired.len(), 1, "the lingering controller expires");
    assert_eq!(expired[0].session_id, g);
    assert!(expired[0].was_controller);
    assert_eq!(roster.last_controller_user().as_deref(), Some("G@"));
    assert_invariants(&roster);
    time += 1;
    let (h, h_token, _) = admit(&mut roster, 11, "H", RosterRole::Controller, time);
    tokens.push(h_token);
    assert_eq!(roster.last_controller_user(), None);
    assert_invariants(&roster);
    time += 1;
    assert_eq!(
        roster.resume(ConnKey(12), &g_token, "G", time),
        Err(RosterError::InvalidToken)
    );
    assert_invariants(&roster);
    time += 1;
    let (_i, _, _) = admit(&mut roster, 13, "I", RosterRole::Viewer, time);
    assert_invariants(&roster);
    time += 1;
    roster
        .leave(ConnKey(13), false, time)
        .expect("hard leave I");
    assert_invariants(&roster);
    time += 1;
    roster.set_max_viewers(1);
    roster.remove(&b).expect("remove B");
    assert_invariants(&roster);
    time += 1;
    assert_eq!(
        roster.admit(ConnKey(14), info("J"), RosterRole::Viewer, time),
        Err(RosterError::SessionFull)
    );
    assert_invariants(&roster);
    time += 1;
    let resumed_e = roster
        .resume(ConnKey(15), &e_token, "E", time)
        .expect("active resume E");
    assert_eq!(resumed_e.role, RosterRole::Viewer);
    assert_eq!(resumed_e.replaced, Some(ConnKey(6)));
    assert_invariants(&roster);
    time += 1;
    roster.remove(&e).expect("remove E");
    let (j, _, _) = admit(&mut roster, 16, "J", RosterRole::Viewer, time);
    assert_eq!(roster.role_of(&j), Some(RosterRole::Viewer));
    assert_invariants(&roster);
    roster.remove(&j).expect("remove J");
    roster.remove(&h).expect("remove H");
    assert_invariants(&roster);
    assert_eq!(roster.participants(), Vec::new());
    assert_eq!(roster.next_deadline(), None);
    assert_invariants(&roster);
    assert_eq!(roster.sessions_for_device("E"), Vec::new());
    assert!(tokens.iter().all(|token| token.len() == TOKEN_LEN));
}
