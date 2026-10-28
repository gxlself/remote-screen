//! Virtual display lifecycle and wire-session integration.

use super::{displays, roster, ConnCx, FeatureEvent};
use crate::{
    auth,
    platform::{DisplayInfo, Platform},
    session::manager::{EndReason, HostServices, SessionManager},
    vdisplay::{
        manager::{Begin, Limits, VdEvent, VdManager, VdStatus},
        mode, VdCaps, VdError, VirtualDisplayBackend,
    },
};
use rsp_core::{features, pb};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use tokio::time::{self, MissedTickBehavior};
use tracing::{debug, info, warn};

pub struct Host {
    backend: OnceLock<Arc<dyn VirtualDisplayBackend>>,
    caps: OnceLock<VdCaps>,
    supported: AtomicBool,
    pub(crate) state: Mutex<VdManager>,
}

impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            backend: OnceLock::new(),
            caps: OnceLock::new(),
            supported: AtomicBool::new(false),
            state: Mutex::new(VdManager::new(Limits::default())),
        }
    }
}

#[derive(Default)]
pub struct Conn {}

#[derive(Debug)]
pub enum Event {
    Ready {
        request_id: u32,
        select: bool,
        platform_id: u32,
    },
    Failed {
        request_id: u32,
        status: VdStatus,
        message: String,
    },
}

/// VIRTUAL_DISPLAY is advertised dynamically only when a backend has caps.
pub const FEATURES: u64 = 0;

/// ServerHello uses this dynamic feature bit.
pub fn advertised<P: Platform>(mgr: &SessionManager<P>) -> u64 {
    if is_supported(mgr) {
        features::VIRTUAL_DISPLAY
    } else {
        0
    }
}

pub fn is_supported<P: Platform>(mgr: &SessionManager<P>) -> bool {
    mgr.features().vdisplay.supported.load(Ordering::Acquire)
}

pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let Some(backend) = mgr.platform().virtual_displays() else {
        return;
    };

    let stale = backend.cleanup_stale();
    if stale > 0 {
        info!(count = stale, "cleaned up stale virtual displays");
    }
    let Some(caps) = backend.caps() else {
        return;
    };

    let host = &mgr.features().vdisplay;
    // Host startup invokes this hook once, so OnceLock cannot already be set.
    let _ = host.backend.set(backend);
    let _ = host.caps.set(caps.clone());
    let limits = Limits {
        total: caps.max_displays.min(2) as usize,
        ..Limits::default()
    };
    *host
        .state
        .lock()
        .unwrap_or_else(|poison| poison.into_inner()) = VdManager::new(limits);
    host.supported.store(true, Ordering::Release);

    let weak = Arc::downgrade(mgr);
    tokio::spawn(async move {
        let mut interval = time::interval(std::time::Duration::from_millis(250));
        interval.set_missed_tick_behavior(MissedTickBehavior::Skip);
        loop {
            interval.tick().await;
            let Some(mgr) = weak.upgrade() else {
                break;
            };
            let now = mgr.now_us();
            let participants = roster::with_roster(&mgr, |roster| roster.participants());
            let events = {
                let mut state = mgr
                    .features()
                    .vdisplay
                    .state
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                state.tick(
                    &|owner| {
                        participants.iter().any(|participant| {
                            participant.session_id.0 == owner
                                && (participant.conn.is_some()
                                    || participant
                                        .linger_deadline_us
                                        .is_some_and(|deadline| deadline > now))
                        })
                    },
                    now,
                )
            };
            dispatch(&mgr, events);
        }
    });
}

pub fn on_create<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::CreateVirtualDisplay) {
    if !is_supported(cx.mgr) {
        send_result(
            cx,
            m.request_id,
            VdStatus::Unsupported,
            "virtual displays unsupported",
        );
        return;
    }
    if !cx.is_controller() {
        send_result(
            cx,
            m.request_id,
            VdStatus::NotAllowed,
            "only the controller can create virtual displays",
        );
        return;
    }

    let Some(caps) = cx.mgr.features().vdisplay.caps.get().cloned() else {
        send_result(
            cx,
            m.request_id,
            VdStatus::Unsupported,
            "virtual displays unsupported",
        );
        return;
    };
    let request = mode::VdRequest {
        width_px: m.width_px,
        height_px: m.height_px,
        scale: m.scale,
        refresh_hz: m.refresh_hz,
    };
    let mode = match mode::choose(&request, &caps) {
        Ok(mode) => mode,
        Err(error) => {
            send_result(cx, m.request_id, VdStatus::BadRequest, error.to_string());
            return;
        }
    };

    let name = cx.info.device_name.trim();
    let name: String = name.chars().take(32).collect();
    let label = if name.is_empty() {
        "虚拟屏".to_owned()
    } else {
        format!("虚拟屏（{name}）")
    };
    let begin = {
        let mut state = cx
            .mgr
            .features()
            .vdisplay
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.begin(&cx.info.session_id, &label, cx.key, m.request_id, m.select)
    };

    match begin {
        Begin::Reject { status, message } => send_result(cx, m.request_id, status, message),
        Begin::Existing { platform_id } => {
            let Some(display) = registry_display(cx.mgr, platform_id) else {
                send_result(
                    cx,
                    m.request_id,
                    VdStatus::Failed,
                    "virtual display not listed yet",
                );
                return;
            };
            cx.send(pb::VirtualDisplayResult {
                request_id: m.request_id,
                status: status_pb(VdStatus::Ok),
                display: Some(display),
                ..Default::default()
            });
            if m.select {
                if let Some(wire) = registry_wire_id(cx.mgr, platform_id) {
                    displays::on_select_display(cx, pb::SelectDisplay { display_id: wire });
                }
            }
        }
        Begin::Create(ticket) => {
            let weak = Arc::downgrade(cx.mgr);
            let Some(backend) = cx.mgr.features().vdisplay.backend.get().cloned() else {
                send_result(
                    cx,
                    m.request_id,
                    VdStatus::Unsupported,
                    "virtual displays unsupported",
                );
                return;
            };
            tokio::spawn(async move {
                let create =
                    tokio::task::spawn_blocking(move || backend.create(mode, &label)).await;
                let result = match create {
                    Ok(result) => result,
                    Err(_) => Err(VdError::Failed("create task failed".to_owned())),
                };
                let Some(mgr) = weak.upgrade() else {
                    return;
                };
                let current = mgr.platform().displays();
                let events = {
                    let mut state = mgr
                        .features()
                        .vdisplay
                        .state
                        .lock()
                        .unwrap_or_else(|poison| poison.into_inner());
                    state.created(ticket, result, &current, mgr.now_us())
                };
                if events
                    .iter()
                    .any(|event| matches!(event, VdEvent::Ready { .. }))
                {
                    displays::republish(&mgr);
                }
                dispatch(&mgr, events);
            });
        }
    }
}

pub fn on_release<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::ReleaseVirtualDisplay) {
    let Some(platform_id) = registry_platform_id(cx.mgr, m.display_id) else {
        debug!(wire_id = m.display_id, "virtual display release ignored");
        return;
    };
    let result = {
        let mut state = cx
            .mgr
            .features()
            .vdisplay
            .state
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        state.release(&cx.info.session_id, platform_id)
    };
    match result {
        Ok(events) => {
            dispatch(cx.mgr, events);
            displays::republish(cx.mgr);
        }
        Err(status) => {
            debug!(wire_id = m.display_id, status = ?status, "virtual display release ignored");
        }
    }
}

pub fn on_admitted<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {
        Event::Ready {
            request_id,
            select,
            platform_id,
        } => {
            let Some((wire_id, pb_display)) = registry_entry(cx.mgr, platform_id) else {
                send_result(
                    cx,
                    request_id,
                    VdStatus::Failed,
                    "virtual display not listed yet",
                );
                return;
            };
            info!(
                event = "vdisplay.created",
                wire_id,
                width = pb_display.width_px,
                height = pb_display.height_px
            );
            cx.send(pb::VirtualDisplayResult {
                request_id,
                status: status_pb(VdStatus::Ok),
                display: Some(pb_display),
                ..Default::default()
            });
            if select {
                displays::on_select_display(
                    cx,
                    pb::SelectDisplay {
                        display_id: wire_id,
                    },
                );
            }
        }
        Event::Failed {
            request_id,
            status,
            message,
        } => send_result(cx, request_id, status, message),
    }
}

pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn on_closed<P: Platform>(_cx: &mut ConnCx<'_, P>, _reason: EndReason) {}

/// Advance lifecycle state from a platform display update before the registry
/// lock is taken by the display handler.
pub(crate) fn on_platform_displays<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    displays: &[DisplayInfo],
) -> (Vec<VdEvent>, Vec<(u32, String)>) {
    if !is_supported(mgr) {
        return (Vec::new(), Vec::new());
    }
    let mut state = mgr
        .features()
        .vdisplay
        .state
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let events = state.on_displays(displays, mgr.now_us());
    let names = state.virtual_names();
    (events, names)
}

/// Dispatch manager lifecycle events onto connection queues or the backend.
pub(crate) fn dispatch<P: Platform>(mgr: &Arc<SessionManager<P>>, events: Vec<VdEvent>) {
    if !is_supported(mgr) {
        return;
    }
    let backend = mgr.features().vdisplay.backend.get().cloned();
    let mut has_destroy = false;
    for event in events {
        match event {
            VdEvent::Ready {
                conn,
                request_id,
                select,
                platform_id,
            } => {
                let posted = mgr.post(
                    conn,
                    FeatureEvent::Vdisplay(Event::Ready {
                        request_id,
                        select,
                        platform_id,
                    }),
                );
                if !posted {
                    debug!("virtual display ready event dropped: connection closed");
                }
            }
            VdEvent::Failed {
                conn,
                request_id,
                status,
                message,
            } => {
                let posted = mgr.post(
                    conn,
                    FeatureEvent::Vdisplay(Event::Failed {
                        request_id,
                        status,
                        message,
                    }),
                );
                if !posted {
                    debug!("virtual display failure event dropped: connection closed");
                }
            }
            VdEvent::Destroy { token } => {
                has_destroy = true;
                let Some(backend) = backend.clone() else {
                    continue;
                };
                let mgr = Arc::clone(mgr);
                tokio::spawn(async move {
                    let result = tokio::task::spawn_blocking(move || backend.destroy(token)).await;
                    match result {
                        Ok(Ok(())) => info!(event = "vdisplay.destroyed", token),
                        Ok(Err(_)) | Err(_) => warn!(token, "virtual display destroy failed"),
                    }
                    // A backend may remove the platform display as part of
                    // destroy; republish after it completes to remove it from
                    // the wire list even when no watch callback is delivered.
                    displays::republish(&mgr);
                });
            }
        }
    }
    if has_destroy {
        displays::republish(mgr);
    }
}

pub(crate) fn virtual_names<P: Platform>(mgr: &SessionManager<P>) -> Vec<(u32, String)> {
    if !is_supported(mgr) {
        return Vec::new();
    }
    mgr.features()
        .vdisplay
        .state
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .virtual_names()
}

fn send_result<P: Platform>(
    cx: &mut ConnCx<'_, P>,
    request_id: u32,
    status: VdStatus,
    message: impl Into<String>,
) {
    cx.send(pb::VirtualDisplayResult {
        request_id,
        status: status_pb(status),
        message: message.into(),
        ..Default::default()
    });
}

fn status_pb(status: VdStatus) -> i32 {
    use pb::virtual_display_result::Status;
    match status {
        VdStatus::Ok => Status::Ok as i32,
        VdStatus::Unsupported => Status::Unsupported as i32,
        VdStatus::NotAllowed => Status::NotAllowed as i32,
        VdStatus::Limit => Status::Limit as i32,
        VdStatus::BadRequest => Status::BadRequest as i32,
        VdStatus::Failed => Status::Failed as i32,
    }
}

fn registry_wire_id<P: Platform>(mgr: &SessionManager<P>, platform_id: u32) -> Option<u32> {
    mgr.features()
        .displays
        .registry
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .wire_id(platform_id)
        .map(u32::from)
}

fn registry_platform_id<P: Platform>(mgr: &SessionManager<P>, wire_id: u32) -> Option<u32> {
    mgr.features()
        .displays
        .registry
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .platform_id(wire_id)
}

fn registry_display<P: Platform>(mgr: &SessionManager<P>, platform_id: u32) -> Option<pb::Display> {
    registry_entry(mgr, platform_id).map(|(_, display)| display)
}

fn registry_entry<P: Platform>(
    mgr: &SessionManager<P>,
    platform_id: u32,
) -> Option<(u32, pb::Display)> {
    let registry = mgr
        .features()
        .displays
        .registry
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    let wire_id = u32::from(registry.wire_id(platform_id)?);
    let display = registry
        .displays_pb()
        .into_iter()
        .find(|display| display.id == wire_id)?;
    Some((wire_id, display))
}
