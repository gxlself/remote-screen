//! 请求控制的仲裁。
//!
//! This module is deliberately free of clocks, networking, and roster state.
//! Callers provide the current [`Seat`] and the current unix timestamp in
//! microseconds; the state machine returns the actions the session manager
//! should perform.

use std::collections::HashMap;

use rsp_core::errors::ErrorCode;
use rsp_core::pb;
use serde_json::json;

use crate::ipc;

/// How long a takeover request may wait for a controller or local response.
pub const TAKEOVER_TIMEOUT_US: u64 = 30_000_000;

/// The timeout carried in [`pb::ControlRequest`], in seconds.
pub const TAKEOVER_TIMEOUT_S: u32 = 30;

/// A connected participant that can request or hold control.
#[derive(Debug, Clone, PartialEq)]
pub struct Candidate {
    pub session_id: String,
    pub device_id: String,
    pub device_name: String,
    /// WhoIs login name. An empty string means unknown (intranet path).
    pub user: String,
    pub can_control: bool,
    pub path: pb::PathKind,
    pub since_us: u64,
}

impl Candidate {
    /// Converts this candidate to the participant wire type.
    pub fn participant(&self) -> pb::Participant {
        pb::Participant {
            device_id: self.device_id.clone(),
            device_name: self.device_name.clone(),
            user: self.user.clone(),
            path: self.path as i32,
            since_us: self.since_us,
        }
    }
}

/// The current state of the control seat.
#[derive(Debug, Clone, PartialEq)]
pub enum Seat {
    Occupied {
        controller: Candidate,
    },
    /// The former controller disconnected and control is reserved for 60 s.
    Lingering {
        former: Candidate,
    },
    /// No controller is active.
    Empty {
        former_user: Option<String>,
    },
}

/// Local arbitration policy.
#[derive(Debug, Clone, Copy, PartialEq, Eq, Default)]
pub struct TakeoverPolicy {
    /// When set, taking control always requires confirmation on the host.
    pub attended_mode: bool,
}

/// Why a takeover request was granted.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum GrantReason {
    SameUser,
    SeatEmpty,
    ControllerAllowed,
    LocalAllowed,
}

/// An effect that the session manager should apply.
#[derive(Debug, Clone, PartialEq)]
pub enum TakeoverAction {
    /// `to` and `from` are session IDs.
    Grant {
        to: String,
        from: Option<String>,
        reason: GrantReason,
    },
    SendControlRequest {
        to: String,
        msg: pb::ControlRequest,
    },
    OpenLocalPrompt {
        requester: Candidate,
    },
    CloseLocalPrompt,
    Reject {
        to: String,
        error: pb::ErrorInfo,
    },
}

#[derive(Debug, Clone, PartialEq)]
struct Pending {
    requester: Candidate,
    asked_controller: Option<String>,
    prompt_open: bool,
    deadline_us: u64,
}

/// Pure takeover arbitration state machine.
#[derive(Debug, Clone, PartialEq)]
pub struct Takeover {
    policy: TakeoverPolicy,
    pending: Option<Pending>,
}

impl Takeover {
    pub fn new(policy: TakeoverPolicy) -> Self {
        Self {
            policy,
            pending: None,
        }
    }

    pub fn set_policy(&mut self, policy: TakeoverPolicy) {
        self.policy = policy;
    }

    pub fn pending_requester(&self) -> Option<&Candidate> {
        self.pending.as_ref().map(|pending| &pending.requester)
    }

    pub fn next_deadline(&self) -> Option<u64> {
        self.pending.as_ref().map(|pending| pending.deadline_us)
    }

    /// Handles a viewer's `RequestControl` message.
    pub fn on_request(
        &mut self,
        requester: Candidate,
        seat: &Seat,
        now_us: u64,
    ) -> Vec<TakeoverAction> {
        tracing::debug!(session = %requester.session_id, "takeover request");

        if !requester.can_control {
            return vec![reject(&requester.session_id, "view_only")];
        }

        if let Seat::Occupied { controller } = seat {
            if controller.session_id == requester.session_id {
                return Vec::new();
            }
        }

        if let Some(pending) = &self.pending {
            if pending.requester.session_id == requester.session_id {
                return Vec::new();
            }
            return vec![reject(&requester.session_id, "busy")];
        }

        match seat {
            Seat::Occupied { controller } => {
                if same_known_user(&requester.user, &controller.user) {
                    tracing::debug!(
                        session = %requester.session_id,
                        reason = "same_user",
                        "takeover granted"
                    );
                    return vec![TakeoverAction::Grant {
                        to: requester.session_id,
                        from: Some(controller.session_id.clone()),
                        reason: GrantReason::SameUser,
                    }];
                }

                self.begin_pending(
                    requester.clone(),
                    Some(controller.session_id.clone()),
                    now_us,
                );
                let msg = pb::ControlRequest {
                    requester: Some(requester.participant()),
                    timeout_s: TAKEOVER_TIMEOUT_S,
                };
                vec![
                    TakeoverAction::SendControlRequest {
                        to: controller.session_id.clone(),
                        msg,
                    },
                    TakeoverAction::OpenLocalPrompt { requester },
                ]
            }
            Seat::Lingering { .. } => self.open_local_prompt(requester, None, now_us),
            Seat::Empty { former_user } => {
                let directly_available = !self.policy.attended_mode
                    && former_user
                        .as_deref()
                        .is_none_or(|former| same_known_user(former, &requester.user));
                if directly_available {
                    tracing::debug!(
                        session = %requester.session_id,
                        reason = "seat_empty",
                        "takeover granted"
                    );
                    vec![TakeoverAction::Grant {
                        to: requester.session_id,
                        from: None,
                        reason: GrantReason::SeatEmpty,
                    }]
                } else {
                    self.open_local_prompt(requester, None, now_us)
                }
            }
        }
    }

    /// Handles a `ControlResponse` sent by the controller that was asked.
    pub fn on_controller_response(
        &mut self,
        from_session: &str,
        resp: &pb::ControlResponse,
    ) -> Vec<TakeoverAction> {
        let Some(pending) = self.pending.as_ref() else {
            return Vec::new();
        };
        if pending.asked_controller.as_deref() != Some(from_session)
            || pending.requester.device_id != resp.requester_device_id
        {
            return Vec::new();
        }

        let requester_session = pending.requester.session_id.clone();
        let prompt_open = pending.prompt_open;
        self.pending = None;

        tracing::debug!(
            session = %requester_session,
            allow = resp.allow,
            "controller responded to takeover request"
        );

        let mut actions = Vec::with_capacity(2);
        if prompt_open {
            actions.push(TakeoverAction::CloseLocalPrompt);
        }
        if resp.allow {
            actions.push(TakeoverAction::Grant {
                to: requester_session,
                from: Some(from_session.to_owned()),
                reason: GrantReason::ControllerAllowed,
            });
        } else {
            actions.push(reject(&requester_session, "denied"));
        }
        actions
    }

    /// Handles the local confirmation prompt. The UI has already closed it.
    pub fn on_local_response(
        &mut self,
        allow: bool,
        current_controller: Option<&str>,
    ) -> Vec<TakeoverAction> {
        let Some(pending) = self.pending.take() else {
            return Vec::new();
        };

        tracing::debug!(
            session = %pending.requester.session_id,
            allow,
            "local user responded to takeover request"
        );

        if allow {
            vec![TakeoverAction::Grant {
                to: pending.requester.session_id,
                from: current_controller.map(str::to_owned),
                reason: GrantReason::LocalAllowed,
            }]
        } else {
            vec![reject(&pending.requester.session_id, "denied")]
        }
    }

    /// Handles a participant disconnecting.
    pub fn on_participant_left(&mut self, session_id: &str) -> Vec<TakeoverAction> {
        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.requester.session_id == session_id)
        {
            let prompt_open = self
                .pending
                .as_ref()
                .is_some_and(|pending| pending.prompt_open);
            self.pending = None;
            tracing::debug!(session = %session_id, "takeover requester left");
            return if prompt_open {
                vec![TakeoverAction::CloseLocalPrompt]
            } else {
                Vec::new()
            };
        }

        if self
            .pending
            .as_ref()
            .is_some_and(|pending| pending.asked_controller.as_deref() == Some(session_id))
        {
            if let Some(pending) = self.pending.as_mut() {
                pending.asked_controller = None;
            }
            tracing::debug!(session = %session_id, "asked takeover controller left");
        }

        Vec::new()
    }

    /// Rejects a pending request once its deadline is reached.
    pub fn poll(&mut self, now_us: u64) -> Vec<TakeoverAction> {
        let Some(pending) = self.pending.as_ref() else {
            return Vec::new();
        };
        if now_us < pending.deadline_us {
            return Vec::new();
        }

        let prompt_open = pending.prompt_open;
        let requester_session = pending.requester.session_id.clone();
        self.pending = None;

        tracing::debug!(session = %requester_session, "takeover request timed out");

        let mut actions = Vec::with_capacity(2);
        if prompt_open {
            actions.push(TakeoverAction::CloseLocalPrompt);
        }
        actions.push(reject(&requester_session, "timeout"));
        actions
    }

    fn open_local_prompt(
        &mut self,
        requester: Candidate,
        asked_controller: Option<String>,
        now_us: u64,
    ) -> Vec<TakeoverAction> {
        tracing::debug!(session = %requester.session_id, "opening takeover prompt");
        self.begin_pending(requester.clone(), asked_controller, now_us);
        vec![TakeoverAction::OpenLocalPrompt { requester }]
    }

    fn begin_pending(
        &mut self,
        requester: Candidate,
        asked_controller: Option<String>,
        now_us: u64,
    ) {
        self.pending = Some(Pending {
            requester,
            asked_controller,
            prompt_open: true,
            deadline_us: now_us.saturating_add(TAKEOVER_TIMEOUT_US),
        });
    }
}

/// Builds the `event.handoff` IPC payload for a completed handoff.
pub fn handoff_event(from: Option<&Candidate>, to: &Candidate, reason: GrantReason) -> ipc::Event {
    tracing::debug!(
        session = %to.session_id,
        reason = grant_reason_str(reason),
        "handoff event"
    );
    ipc::Event::Handoff(json!({
        "from": from.map(candidate_event_party),
        "to": candidate_event_party(to),
        "reason": grant_reason_str(reason),
    }))
}

fn same_known_user(left: &str, right: &str) -> bool {
    !left.is_empty() && left == right
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

fn candidate_event_party(candidate: &Candidate) -> serde_json::Value {
    json!({
        "device_id": candidate.device_id,
        "device_name": candidate.device_name,
        "user": candidate.user,
    })
}

fn grant_reason_str(reason: GrantReason) -> &'static str {
    match reason {
        GrantReason::SameUser => "same_user",
        GrantReason::SeatEmpty => "seat_empty",
        GrantReason::ControllerAllowed => "controller_allowed",
        GrantReason::LocalAllowed => "local_allowed",
    }
}
