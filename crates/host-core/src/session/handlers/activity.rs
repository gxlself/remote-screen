//! Session feature hooks; extended by the owning feature task.
use super::{roster, ConnCx};
use crate::{
    auth,
    platform::Platform,
    session::{
        activity::{self, local_activity_msg, ActivityHandle},
        manager::{EndReason, HostServices, SessionManager},
    },
};
use std::sync::{Arc, OnceLock};

pub struct Host {
    handle: OnceLock<ActivityHandle>,
}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            handle: OnceLock::new(),
        }
    }
}
#[derive(Default)]
pub struct Conn {}
#[derive(Debug)]
pub enum Event {}
pub const FEATURES: u64 = 0;
pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let (tx, rx) = tokio::sync::mpsc::channel(32);
    mgr.platform().watch_local_activity(tx);
    let (handle, _tracker) = activity::spawn_tracker(
        rx,
        mgr.services().config.get().access.pause_on_local_input,
        mgr.services().clock.clone(),
    );
    let mut paused = handle.subscribe();
    let mut config = mgr.services().config.subscribe();
    let _ = mgr.features().activity.handle.set(handle.clone());
    let weak = Arc::downgrade(mgr);

    tokio::spawn(async move {
        loop {
            tokio::select! {
                changed = paused.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let active = *paused.borrow_and_update();
                    let Some(mgr) = weak.upgrade() else {
                        break;
                    };
                    mgr.broadcast(local_activity_msg(active).into());
                    roster::broadcast_session_state(&mgr);
                }
                changed = config.changed() => {
                    if changed.is_err() {
                        break;
                    }
                    let enabled = config.borrow_and_update().access.pause_on_local_input;
                    handle.set_pause_enabled(enabled);
                }
            }
        }
    });
}
pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    if paused_watch(cx.mgr).is_some_and(|paused| *paused.borrow()) {
        cx.send(local_activity_msg(true));
    }
}
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
pub fn on_closed<P: Platform>(_cx: &mut ConnCx<'_, P>, _reason: EndReason) {}

pub fn active_within_60s<P: Platform>(mgr: &Arc<SessionManager<P>>) -> bool {
    mgr.features()
        .activity
        .handle
        .get()
        .is_some_and(ActivityHandle::active_within_60s)
}
pub fn paused_watch<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
) -> Option<tokio::sync::watch::Receiver<bool>> {
    mgr.features()
        .activity
        .handle
        .get()
        .map(ActivityHandle::subscribe)
}
