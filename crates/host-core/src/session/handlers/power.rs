//! Session feature hooks; extended by the owning feature task.
use super::ConnCx;
use crate::{
    auth,
    platform::Platform,
    session::manager::{EndReason, HostServices, SessionManager},
    session::power::{
        console_transition, console_verdict, wake_on_stream_start, AwakeGuard, ConsoleChange,
        ConsoleVerdict, KeepAwake, PlatformPower, PowerPort, CONSOLE_POLL,
    },
};
use std::sync::{Arc, OnceLock, Weak};
use tokio::sync::Notify;
use tracing::{info, warn};

pub struct Host {
    keep: OnceLock<Arc<KeepAwake>>,
    recheck: Arc<Notify>,
}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            keep: OnceLock::new(),
            recheck: Arc::new(Notify::new()),
        }
    }
}
#[derive(Default)]
pub struct Conn {
    guard: Option<AwakeGuard>,
}
#[derive(Debug)]
pub enum Event {
    ConsoleLeft(rsp_core::pb::ErrorInfo),
}
pub const FEATURES: u64 = 0;
pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let power = Arc::new(PlatformPower(mgr.platform().clone()));
    let keep = KeepAwake::new(power as Arc<dyn PowerPort>);
    let host = &mgr.features().power;
    if host.keep.set(keep).is_err() {
        warn!("power handler started more than once; keeping the first keep-awake owner");
    }

    let previous = mgr.platform().console_state();
    let recheck = Arc::clone(&host.recheck);
    let weak = Arc::downgrade(mgr);
    tokio::spawn(async move {
        watch_console(weak, recheck, previous).await;
    });
}

async fn watch_console<P: Platform>(
    weak: Weak<SessionManager<P>>,
    recheck: Arc<Notify>,
    mut previous: crate::platform::ConsoleState,
) {
    loop {
        tokio::select! {
            () = tokio::time::sleep(CONSOLE_POLL) => {}
            () = recheck.notified() => {}
        }
        let Some(mgr) = weak.upgrade() else {
            break;
        };
        let current = mgr.platform().console_state();
        match console_transition(previous, current) {
            Some(ConsoleChange::Left { error }) => {
                info!(state = ?current, "host left the console; ending remote sessions");
                mgr.post_all(|_| {
                    Some(super::FeatureEvent::Power(Event::ConsoleLeft(
                        error.clone(),
                    )))
                });
            }
            Some(ConsoleChange::Returned) => {
                info!(state = ?current, "host returned to the console");
            }
            None => {}
        }
        previous = current;
    }
}

pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let Some(keep) = cx.mgr.features().power.keep.get() else {
        warn!("keep-awake owner is missing after host start");
        return;
    };
    cx.st.power.guard = Some(keep.acquire());
}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(cx: &mut ConnCx<'_, P>) {
    wake_on_stream_start(&PlatformPower(cx.mgr.platform().clone()));
}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {
        Event::ConsoleLeft(error) => {
            super::stream::stop(cx);
            cx.send(error);
            cx.end(EndReason::ConsoleLeft);
        }
    }
}
pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    cx.st.power.guard = None;
}

pub fn console_check<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Option<rsp_core::pb::ErrorInfo> {
    match console_verdict(mgr.platform().console_state(), None) {
        ConsoleVerdict::Allow => None,
        ConsoleVerdict::Deny(error) => Some(error),
    }
}

/// Re-checks the console state immediately instead of waiting for the next poll.
pub fn check_console_now<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    mgr.features().power.recheck.notify_one();
}
