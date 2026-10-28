//! Wire arbitration and host-side confirmation for requests to take control.
use super::{approval, roster, ConnCx};
use crate::{
    auth,
    platform::Platform,
    session::{
        approval::{PromptKind, PromptOutcome, PromptRequest},
        manager::{EndReason, HostServices, SessionManager},
        roster::SessionId,
        takeover::{self, Candidate, Seat, Takeover, TakeoverAction, TakeoverPolicy},
    },
};
use rsp_core::pb;
use std::sync::{Arc, Mutex};

pub struct Host {
    pub state: Mutex<Takeover>,
    pub prompt_id: Mutex<Option<String>>,
}
impl Host {
    pub fn new(s: &HostServices) -> Self {
        Self {
            state: Mutex::new(Takeover::new(TakeoverPolicy {
                attended_mode: s.config.get().access.attended_mode,
            })),
            prompt_id: Mutex::new(None),
        }
    }
}

fn candidate<P: Platform>(mgr: &Arc<SessionManager<P>>, session: &str) -> Option<Candidate> {
    let conn = roster::with_roster(mgr, |r| r.conn_of(&SessionId(session.to_owned())))?;
    roster::candidate(mgr, conn)
}

fn current_controller<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Option<String> {
    match roster::seat(mgr) {
        Seat::Occupied { controller } => Some(controller.session_id),
        Seat::Lingering { .. } | Seat::Empty { .. } => None,
    }
}

fn close_prompt<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let id = mgr
        .features()
        .takeover
        .prompt_id
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .take();
    if let (Some(id), Some(prompts)) = (id, approval::prompts(mgr)) {
        prompts.cancel(&id);
    }
}

fn apply<P: Platform>(mgr: &Arc<SessionManager<P>>, actions: Vec<TakeoverAction>) {
    for action in actions {
        match action {
            TakeoverAction::Grant { to, from, reason } => {
                let from = from.as_deref().and_then(|id| candidate(mgr, id));
                let to_c = candidate(mgr, &to);
                if let Some(to_c) = to_c {
                    if roster::handoff(mgr, &to) {
                        mgr.emit(takeover::handoff_event(from.as_ref(), &to_c, reason));
                    }
                }
                close_prompt(mgr);
            }
            TakeoverAction::SendControlRequest { to, msg } => {
                if let Some(conn) = roster::with_roster(mgr, |r| r.conn_of(&SessionId(to))) {
                    mgr.send(conn, msg);
                }
            }
            TakeoverAction::OpenLocalPrompt { requester } => {
                let Some(prompts) = approval::prompts(mgr) else {
                    let controller = current_controller(mgr);
                    let actions = mgr
                        .features()
                        .takeover
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .on_local_response(false, controller.as_deref());
                    apply(mgr, actions);
                    continue;
                };
                let info = roster::with_roster(mgr, |r| {
                    r.conn_of(&SessionId(requester.session_id.clone()))
                })
                .and_then(|conn| mgr.info(conn));
                let Some(info) = info else {
                    let actions = mgr
                        .features()
                        .takeover
                        .state
                        .lock()
                        .unwrap_or_else(|p| p.into_inner())
                        .on_participant_left(&requester.session_id);
                    apply(mgr, actions);
                    continue;
                };
                let host = &mgr.features().takeover;
                let mut id = host.prompt_id.lock().unwrap_or_else(|p| p.into_inner());
                // The controller may answer immediately on another connection,
                // or the requester may leave before these effects are applied.
                let still_pending = host
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner())
                    .pending_requester()
                    .is_some_and(|pending| pending.session_id == requester.session_id);
                if !still_pending {
                    continue;
                }
                let requester_session = requester.session_id;
                let prompt = prompts.open(PromptRequest {
                    kind: PromptKind::Takeover,
                    device_name: requester.device_name,
                    device_id: requester.device_id,
                    platform: info.platform,
                    whois_login: (!requester.user.is_empty()).then_some(requester.user),
                    path: None,
                    reasons: vec![],
                    device_fp: Some(info.device_fp),
                    intent: None,
                });
                let request_id = prompt.request_id.clone();
                *id = Some(request_id.clone());
                drop(id);
                let weak = Arc::downgrade(mgr);
                tokio::spawn(async move {
                    let PromptOutcome::Answered(answer) = prompt.wait().await else {
                        return;
                    };
                    let Some(mgr) = weak.upgrade() else {
                        return;
                    };
                    let controller = current_controller(&mgr);
                    let actions = {
                        let host = &mgr.features().takeover;
                        let mut id = host.prompt_id.lock().unwrap_or_else(|p| p.into_inner());
                        // A cancelled prompt's queued answer cannot resolve a later request.
                        if id.as_deref() != Some(&request_id) {
                            return;
                        }
                        let mut state = host.state.lock().unwrap_or_else(|p| p.into_inner());
                        *id = None;
                        if state
                            .pending_requester()
                            .is_none_or(|pending| pending.session_id != requester_session)
                        {
                            return;
                        }
                        state.on_local_response(answer.allow, controller.as_deref())
                    };
                    apply(&mgr, actions);
                });
            }
            TakeoverAction::CloseLocalPrompt => close_prompt(mgr),
            TakeoverAction::Reject { to, error } => {
                if let Some(conn) = roster::with_roster(mgr, |r| r.conn_of(&SessionId(to))) {
                    mgr.send(conn, error);
                }
            }
        }
    }
}

#[derive(Default)]
pub struct Conn {}
#[derive(Debug)]
pub enum Event {}
pub const FEATURES: u64 = 0;
pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let weak = Arc::downgrade(mgr);
    let mut config = mgr.services().config.subscribe();
    tokio::spawn(async move {
        loop {
            let Some(mgr) = weak.upgrade() else {
                break;
            };
            let now = mgr.now_us();
            let (actions, deadline) = {
                let mut state = mgr
                    .features()
                    .takeover
                    .state
                    .lock()
                    .unwrap_or_else(|p| p.into_inner());
                state.set_policy(TakeoverPolicy {
                    attended_mode: config.borrow_and_update().access.attended_mode,
                });
                (state.poll(now), state.next_deadline())
            };
            apply(&mgr, actions);
            let delay = super::timer_delay(deadline, now);
            drop(mgr);
            tokio::select! {
                () = tokio::time::sleep(delay) => {},
                changed = config.changed() => {
                    if changed.is_err() {
                        break;
                    }
                },
            }
        }
    });
}
pub fn on_admitted<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(_cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {}
}
pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    let actions = cx
        .mgr
        .features()
        .takeover
        .state
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .on_participant_left(&cx.info.session_id);
    apply(cx.mgr, actions);
}

pub fn on_request_control<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if cx.info.intent == pb::Intent::Files {
        tracing::debug!("ignoring control request from files session");
        return;
    }
    let Some(requester) = roster::candidate(cx.mgr, cx.key) else {
        return;
    };
    let seat = roster::seat(cx.mgr);
    let actions = cx
        .mgr
        .features()
        .takeover
        .state
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .on_request(requester, &seat, cx.now_us());
    apply(cx.mgr, actions);
}
pub fn on_control_response<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::ControlResponse) {
    let actions = cx
        .mgr
        .features()
        .takeover
        .state
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .on_controller_response(&cx.info.session_id, &m);
    apply(cx.mgr, actions);
}
