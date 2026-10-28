use std::collections::HashMap;

use host_core::ipc::Event;
use host_core::session::takeover::{
    handoff_event, Candidate, GrantReason, Seat, Takeover, TakeoverAction, TakeoverPolicy,
    TAKEOVER_TIMEOUT_S, TAKEOVER_TIMEOUT_US,
};
use rsp_core::errors::ErrorCode;
use rsp_core::pb;
use serde_json::json;

const T0: u64 = 1_726_000_000_000_000;

fn candidate(session_id: &str, user: &str, can_control: bool) -> Candidate {
    Candidate {
        session_id: session_id.to_owned(),
        device_id: format!("device-{session_id}"),
        device_name: format!("Device {session_id}"),
        user: user.to_owned(),
        can_control,
        path: pb::PathKind::TailnetDirect,
        since_us: 1_726_000_000_000_000,
    }
}

fn occupied(controller: &Candidate) -> Seat {
    Seat::Occupied {
        controller: controller.clone(),
    }
}

fn lingering(former: &Candidate) -> Seat {
    Seat::Lingering {
        former: former.clone(),
    }
}

fn reject(to: &str, reason: &str) -> TakeoverAction {
    TakeoverAction::Reject {
        to: to.to_owned(),
        error: pb::ErrorInfo {
            code: u32::from(ErrorCode::TakeoverDenied.code()),
            message: "takeover denied".to_owned(),
            retry_after_s: 0,
            detail: HashMap::from([("reason".to_owned(), reason.to_owned())]),
        },
    }
}

fn grant(to: &str, from: Option<&str>, reason: GrantReason) -> TakeoverAction {
    TakeoverAction::Grant {
        to: to.to_owned(),
        from: from.map(str::to_owned),
        reason,
    }
}

fn response(requester_device_id: &str, allow: bool) -> pb::ControlResponse {
    pb::ControlResponse {
        requester_device_id: requester_device_id.to_owned(),
        allow,
    }
}

#[test]
fn same_user_takes_over_immediately() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "alice@", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());

    assert_eq!(
        takeover.on_request(requester, &occupied(&controller), T0),
        vec![grant(
            "requester",
            Some("controller"),
            GrantReason::SameUser
        )]
    );
    assert_eq!(takeover.pending_requester(), None);
    assert_eq!(takeover.next_deadline(), None);
}

#[test]
fn unknown_users_ask_controller_and_open_local_prompt() {
    let controller = candidate("controller", "", true);
    let requester = candidate("requester", "", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());

    assert_eq!(
        takeover.on_request(requester.clone(), &occupied(&controller), T0),
        vec![
            TakeoverAction::SendControlRequest {
                to: "controller".to_owned(),
                msg: pb::ControlRequest {
                    requester: Some(requester.participant()),
                    timeout_s: TAKEOVER_TIMEOUT_S,
                },
            },
            TakeoverAction::OpenLocalPrompt {
                requester: requester.clone(),
            },
        ]
    );
    assert_eq!(takeover.pending_requester(), Some(&requester));
    assert_eq!(takeover.next_deadline(), Some(T0 + TAKEOVER_TIMEOUT_US));
}

#[test]
fn controller_can_allow_or_deny() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "bob@", true);

    let mut allow = Takeover::new(TakeoverPolicy::default());
    allow.on_request(requester.clone(), &occupied(&controller), T0);
    assert_eq!(
        allow.on_controller_response("controller", &response(&requester.device_id, true)),
        vec![
            TakeoverAction::CloseLocalPrompt,
            grant(
                "requester",
                Some("controller"),
                GrantReason::ControllerAllowed
            ),
        ]
    );
    assert_eq!(allow.pending_requester(), None);
    assert_eq!(allow.next_deadline(), None);

    let mut deny = Takeover::new(TakeoverPolicy::default());
    deny.on_request(requester.clone(), &occupied(&controller), T0);
    assert_eq!(
        deny.on_controller_response("controller", &response(&requester.device_id, false)),
        vec![
            TakeoverAction::CloseLocalPrompt,
            reject("requester", "denied"),
        ]
    );
    assert_eq!(deny.pending_requester(), None);
}

#[test]
fn local_user_can_allow_or_deny() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "bob@", true);

    let mut allow = Takeover::new(TakeoverPolicy::default());
    allow.on_request(requester.clone(), &occupied(&controller), T0);
    assert_eq!(
        allow.on_local_response(true, Some("controller")),
        vec![grant(
            "requester",
            Some("controller"),
            GrantReason::LocalAllowed
        )]
    );
    assert_eq!(allow.pending_requester(), None);

    let mut deny = Takeover::new(TakeoverPolicy::default());
    deny.on_request(requester, &occupied(&controller), T0);
    assert_eq!(
        deny.on_local_response(false, Some("controller")),
        vec![reject("requester", "denied")]
    );
    assert_eq!(deny.pending_requester(), None);
}

#[test]
fn pending_request_times_out_exactly_at_deadline() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "bob@", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());
    takeover.on_request(requester.clone(), &occupied(&controller), T0);

    assert_eq!(takeover.poll(T0 + TAKEOVER_TIMEOUT_US - 1), Vec::new());
    assert_eq!(takeover.pending_requester(), Some(&requester));
    assert_eq!(
        takeover.poll(T0 + TAKEOVER_TIMEOUT_US),
        vec![
            TakeoverAction::CloseLocalPrompt,
            reject("requester", "timeout"),
        ]
    );
    assert_eq!(takeover.pending_requester(), None);
    assert_eq!(takeover.next_deadline(), None);
    assert_eq!(
        takeover.on_controller_response("controller", &response(&requester.device_id, true)),
        Vec::new()
    );
}

#[test]
fn view_only_requester_is_rejected_without_pending_request() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "alice@", false);
    let mut takeover = Takeover::new(TakeoverPolicy::default());

    assert_eq!(
        takeover.on_request(requester, &occupied(&controller), T0),
        vec![reject("requester", "view_only")]
    );
    assert_eq!(takeover.pending_requester(), None);
    assert_eq!(takeover.next_deadline(), None);
}

#[test]
fn only_one_pending_request_is_arbitrated() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "bob@", true);
    let other = candidate("other", "carol@", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());
    takeover.on_request(requester.clone(), &occupied(&controller), T0);

    assert_eq!(
        takeover.on_request(requester.clone(), &occupied(&controller), T0 + 1),
        Vec::new()
    );
    assert_eq!(takeover.pending_requester(), Some(&requester));
    assert_eq!(takeover.next_deadline(), Some(T0 + TAKEOVER_TIMEOUT_US));
    assert_eq!(
        takeover.on_request(other, &occupied(&controller), T0 + 1),
        vec![reject("other", "busy")]
    );
    assert_eq!(takeover.pending_requester(), Some(&requester));
}

#[test]
fn responses_from_other_sessions_or_devices_are_ignored() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "bob@", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());
    takeover.on_request(requester.clone(), &occupied(&controller), T0);

    assert_eq!(
        takeover.on_controller_response("other", &response(&requester.device_id, true)),
        Vec::new()
    );
    assert_eq!(takeover.pending_requester(), Some(&requester));
    assert_eq!(
        takeover.on_controller_response("controller", &response("another-device", true)),
        Vec::new()
    );
    assert_eq!(takeover.pending_requester(), Some(&requester));
    assert_eq!(
        takeover.on_controller_response("controller", &response(&requester.device_id, true)),
        vec![
            TakeoverAction::CloseLocalPrompt,
            grant(
                "requester",
                Some("controller"),
                GrantReason::ControllerAllowed
            ),
        ]
    );
}

#[test]
fn lingering_seat_only_opens_the_local_prompt() {
    let former = candidate("former", "alice@", true);
    let requester = candidate("requester", "alice@", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());

    assert_eq!(
        takeover.on_request(requester.clone(), &lingering(&former), T0),
        vec![TakeoverAction::OpenLocalPrompt {
            requester: requester.clone(),
        }]
    );
    assert_eq!(takeover.next_deadline(), Some(T0 + TAKEOVER_TIMEOUT_US));
    assert_eq!(
        takeover.on_local_response(true, None),
        vec![grant("requester", None, GrantReason::LocalAllowed)]
    );
}

#[test]
fn empty_seat_policy_and_former_user_are_applied() {
    let alice = candidate("alice", "alice@", true);
    let bob = candidate("bob", "bob@", true);

    let mut same_user = Takeover::new(TakeoverPolicy::default());
    assert_eq!(
        same_user.on_request(
            alice.clone(),
            &Seat::Empty {
                former_user: Some("alice@".to_owned()),
            },
            T0,
        ),
        vec![grant("alice", None, GrantReason::SeatEmpty)]
    );

    let mut different_user = Takeover::new(TakeoverPolicy::default());
    assert_eq!(
        different_user.on_request(
            bob.clone(),
            &Seat::Empty {
                former_user: Some("alice@".to_owned()),
            },
            T0,
        ),
        vec![TakeoverAction::OpenLocalPrompt {
            requester: bob.clone(),
        }]
    );

    let mut never_controlled = Takeover::new(TakeoverPolicy::default());
    assert_eq!(
        never_controlled.on_request(bob.clone(), &Seat::Empty { former_user: None }, T0),
        vec![grant("bob", None, GrantReason::SeatEmpty)]
    );

    let mut attended = Takeover::new(TakeoverPolicy::default());
    attended.set_policy(TakeoverPolicy {
        attended_mode: true,
    });
    assert_eq!(
        attended.on_request(alice.clone(), &Seat::Empty { former_user: None }, T0),
        vec![TakeoverAction::OpenLocalPrompt {
            requester: alice.clone(),
        }]
    );

    let mut attended_with_former_user = Takeover::new(TakeoverPolicy {
        attended_mode: true,
    });
    assert_eq!(
        attended_with_former_user.on_request(
            alice,
            &Seat::Empty {
                former_user: Some("alice@".to_owned()),
            },
            T0,
        ),
        vec![TakeoverAction::OpenLocalPrompt {
            requester: candidate("alice", "alice@", true),
        }]
    );

    let mut empty_users = Takeover::new(TakeoverPolicy::default());
    assert_eq!(
        empty_users.on_request(
            candidate("one", "", true),
            &Seat::Empty {
                former_user: Some(String::new()),
            },
            T0,
        ),
        vec![TakeoverAction::OpenLocalPrompt {
            requester: candidate("one", "", true),
        }]
    );
}

#[test]
fn participants_leaving_cancel_or_detach_the_pending_request() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "bob@", true);

    let mut requester_left = Takeover::new(TakeoverPolicy::default());
    requester_left.on_request(requester.clone(), &occupied(&controller), T0);
    assert_eq!(
        requester_left.on_participant_left("requester"),
        vec![TakeoverAction::CloseLocalPrompt]
    );
    assert_eq!(requester_left.pending_requester(), None);
    assert_eq!(requester_left.poll(T0 + TAKEOVER_TIMEOUT_US), Vec::new());

    let mut controller_left = Takeover::new(TakeoverPolicy::default());
    controller_left.on_request(requester.clone(), &occupied(&controller), T0);
    assert_eq!(
        controller_left.on_participant_left("controller"),
        Vec::new()
    );
    assert_eq!(controller_left.pending_requester(), Some(&requester));
    assert_eq!(
        controller_left.on_controller_response("controller", &response(&requester.device_id, true)),
        Vec::new()
    );
    assert_eq!(
        controller_left.on_local_response(true, None),
        vec![grant("requester", None, GrantReason::LocalAllowed)]
    );
}

#[test]
fn current_controller_requesting_control_is_a_noop() {
    let controller = candidate("controller", "alice@", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());

    assert_eq!(
        takeover.on_request(controller.clone(), &occupied(&controller), T0),
        Vec::new()
    );
    assert_eq!(takeover.pending_requester(), None);
}

#[test]
fn handoff_event_has_stable_payload_and_reason_strings() {
    let from = candidate("former", "alice@", true);
    let to = candidate("next", "bob@", true);
    let cases = [
        (GrantReason::SameUser, "same_user"),
        (GrantReason::SeatEmpty, "seat_empty"),
        (GrantReason::ControllerAllowed, "controller_allowed"),
        (GrantReason::LocalAllowed, "local_allowed"),
    ];

    for (reason, text) in cases {
        let event = handoff_event(Some(&from), &to, reason);
        assert!(matches!(event, Event::Handoff(_)));
        assert_eq!(
            event,
            Event::Handoff(json!({
                "from": {
                    "device_id": "device-former",
                    "device_name": "Device former",
                    "user": "alice@",
                },
                "to": {
                    "device_id": "device-next",
                    "device_name": "Device next",
                    "user": "bob@",
                },
                "reason": text,
            }))
        );
    }

    assert_eq!(
        handoff_event(None, &to, GrantReason::SeatEmpty),
        Event::Handoff(json!({
            "from": null,
            "to": {
                "device_id": "device-next",
                "device_name": "Device next",
                "user": "bob@",
            },
            "reason": "seat_empty",
        }))
    );
}

#[test]
fn next_deadline_tracks_the_pending_request() {
    let controller = candidate("controller", "alice@", true);
    let requester = candidate("requester", "bob@", true);
    let mut takeover = Takeover::new(TakeoverPolicy::default());

    assert_eq!(takeover.next_deadline(), None);
    takeover.on_request(requester, &occupied(&controller), T0);
    assert_eq!(takeover.next_deadline(), Some(T0 + TAKEOVER_TIMEOUT_US));
}
