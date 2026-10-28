//! Idle timeout, session actions and lock-on-end policy.
use super::ConnCx;
use crate::{
    auth,
    platform::{Platform, PlatformError, SessionActionKind},
    session::{
        idle::{
            self, decide_session_action, idle_warning_msg, platform_failure_error, reject_error,
            should_lock_on_end, ActionDecision, ActionReject, IdleEvent, IdleTimer, LockOnEnd,
        },
        manager::{EndReason, HostServices, SessionManager},
    },
};
use rsp_core::{features, pb};
use std::{
    collections::HashMap,
    sync::{Arc, Mutex},
};
use tracing::{debug, warn};

pub struct Host {
    pending_lock: Mutex<HashMap<String, bool>>,
}

impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            pending_lock: Mutex::new(HashMap::new()),
        }
    }
}

#[derive(Default)]
pub struct Conn {
    timer: Option<IdleTimer>,
    lock: Option<LockOnEnd>,
}

#[derive(Debug)]
pub enum Event {
    ControllerInput,
}

pub const FEATURES: u64 = features::SESSION_ACTIONS;

pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}

pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let access = cx.mgr.services().config.get().access;
    cx.st.idle.timer = cx
        .is_controller()
        .then(|| IdleTimer::new(access.idle_timeout_min, cx.now_us()));
    cx.st.idle.lock = Some(LockOnEnd::new(access.lock_on_end));
    if cx.info.resumed {
        pending_lock(cx.mgr).remove(&cx.info.session_id);
    }
}

pub fn on_role_changed<P: Platform>(cx: &mut ConnCx<'_, P>, old: auth::Role) {
    if cx.is_controller() {
        if old != auth::Role::Controller {
            let access = cx.mgr.services().config.get().access;
            cx.st.idle.timer = Some(IdleTimer::new(access.idle_timeout_min, cx.now_us()));
        }
    } else {
        cx.st.idle.timer = None;
    }
}

pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {
        Event::ControllerInput => on_controller_input(cx),
    }
}

pub fn deadline<P: Platform>(cx: &ConnCx<'_, P>) -> Option<u64> {
    cx.st.idle.timer.as_ref().and_then(IdleTimer::next_deadline)
}

pub fn on_tick<P: Platform>(cx: &mut ConnCx<'_, P>, now_us: u64) {
    let access = cx.mgr.services().config.get().access;
    if let Some(timer) = cx.st.idle.timer.as_mut() {
        timer.set_timeout_min(access.idle_timeout_min, now_us);
    }
    if let Some(lock) = cx.st.idle.lock.as_mut() {
        lock.set_config_default(access.lock_on_end);
    }

    let Some(event) = cx
        .st
        .idle
        .timer
        .as_mut()
        .and_then(|timer| timer.poll(now_us))
    else {
        return;
    };
    match event {
        IdleEvent::Warn { seconds_left } => {
            let lock_on_end = cx.st.idle.lock.as_ref().is_some_and(LockOnEnd::enabled);
            cx.send(idle_warning_msg(seconds_left, lock_on_end));
        }
        IdleEvent::Expire => cx.end(EndReason::Idle),
    }
}

pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, reason: EndReason) {
    let was_controller = cx.is_controller();
    let mapped = idle_end_reason(reason);
    let lock_enabled = cx.st.idle.lock.as_ref().is_some_and(LockOnEnd::enabled);
    if reason.lingers() && was_controller {
        pending_lock(cx.mgr).insert(cx.info.session_id.clone(), lock_enabled);
        return;
    }
    if let Some(idle_reason) = mapped {
        if let Some(lock) = cx.st.idle.lock.as_ref() {
            if should_lock_on_end(lock, idle_reason, was_controller) {
                if let Err(error) = cx.platform().session_action(SessionActionKind::LockScreen) {
                    warn!(%error, "failed to lock the host screen when the session ended");
                }
            }
        }
    }
}

pub fn on_session_action<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::SessionAction) {
    match decide_session_action(cx.is_controller(), &m) {
        ActionDecision::Platform(kind) => match cx.platform().session_action(kind) {
            Ok(()) => {}
            Err(PlatformError::Unsupported) => {
                if let Some(error) = reject_error(ActionReject::Unsupported) {
                    cx.send(error);
                }
            }
            Err(error) => cx.send(platform_failure_error(kind, &error)),
        },
        ActionDecision::SetLockOnEnd(enable) => {
            if let Some(lock) = cx.st.idle.lock.as_mut() {
                lock.set_override(enable);
            }
        }
        ActionDecision::Reject(reject) => {
            if let Some(error) = reject_error(reject) {
                cx.send(error);
            } else {
                debug!(
                    session_id = %cx.info.session_id,
                    "ignoring session action from a viewer"
                );
            }
        }
    }
}

pub fn on_controller_input<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let now_us = cx.now_us();
    if let Some(timer) = cx.st.idle.timer.as_mut() {
        timer.on_input(now_us);
    }
}

pub fn on_linger_expired<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    session_id: &str,
    was_controller: bool,
) {
    let enabled = pending_lock(mgr).remove(session_id).unwrap_or(false);
    if enabled && was_controller {
        if let Err(error) = mgr.platform().session_action(SessionActionKind::LockScreen) {
            warn!(%error, session_id, "failed to lock the host screen after linger expiry");
        }
    }
}

fn pending_lock<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
) -> std::sync::MutexGuard<'_, HashMap<String, bool>> {
    mgr.features()
        .idle
        .pending_lock
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn idle_end_reason(reason: EndReason) -> Option<idle::EndReason> {
    match reason {
        EndReason::Normal => Some(idle::EndReason::UserClosed),
        EndReason::Peer(reason) if reason == pb::goodbye::Reason::UserClosed as i32 => {
            Some(idle::EndReason::UserClosed)
        }
        EndReason::Idle => Some(idle::EndReason::IdleTimeout),
        EndReason::Revoked => Some(idle::EndReason::Revoked),
        EndReason::Replaced => Some(idle::EndReason::Replaced),
        EndReason::LocalUser => Some(idle::EndReason::ClosedByLocalUser),
        EndReason::HostShutdown | EndReason::HostUpdating => Some(idle::EndReason::HostShutdown),
        _ => None,
    }
}

#[cfg(test)]
mod tests {
    use super::*;

    #[test]
    fn maps_manager_end_reasons_for_lock_on_end() {
        let cases = [
            (
                EndReason::Peer(pb::goodbye::Reason::UserClosed as i32),
                Some(idle::EndReason::UserClosed),
            ),
            (EndReason::Normal, Some(idle::EndReason::UserClosed)),
            (EndReason::Idle, Some(idle::EndReason::IdleTimeout)),
            (EndReason::Revoked, Some(idle::EndReason::Revoked)),
            (EndReason::Replaced, Some(idle::EndReason::Replaced)),
            (
                EndReason::LocalUser,
                Some(idle::EndReason::ClosedByLocalUser),
            ),
            (EndReason::HostShutdown, Some(idle::EndReason::HostShutdown)),
            (EndReason::HostUpdating, Some(idle::EndReason::HostShutdown)),
            (EndReason::PeerClosed, None),
            (EndReason::Background, None),
            (
                EndReason::Peer(pb::goodbye::Reason::Background as i32),
                None,
            ),
            (EndReason::ConsoleLeft, None),
        ];

        for (manager, expected) in cases {
            assert_eq!(idle_end_reason(manager), expected, "{manager:?}");
        }
    }
}
