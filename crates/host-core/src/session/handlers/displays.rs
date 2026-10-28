//! Host wire display ids and per-connection display selection.
use super::{stream, stream::StreamTarget, vdisplay, ConnCx, FeatureEvent};
use crate::{
    auth,
    platform::{DisplayInfo, Platform},
    session::{
        displays::{DisplayAction, DisplayDiff, DisplayRegistry, SelectError, StreamDisplay},
        manager::{EndReason, HostServices, SessionManager},
    },
};
use rsp_core::{errors::ErrorCode, features, pb};
use std::sync::{Arc, Mutex};
use tokio::sync::mpsc;

pub struct Host {
    pub registry: Mutex<DisplayRegistry>,
}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            registry: Mutex::new(DisplayRegistry::new()),
        }
    }
}
#[derive(Default)]
pub struct Conn {
    pub sd: StreamDisplay,
}
#[derive(Debug)]
pub enum Event {
    RegistryChanged(DisplayDiff),
}
pub const FEATURES: u64 = features::MULTI_DISPLAY;
pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let initial = mgr.platform().displays();
    mgr.features()
        .displays
        .registry
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .update(initial);

    let (tx, mut rx) = mpsc::channel(8);
    mgr.platform().watch_displays(tx);
    let weak = Arc::downgrade(mgr);
    tokio::spawn(async move {
        while let Some(displays) = rx.recv().await {
            let Some(mgr) = weak.upgrade() else {
                break;
            };
            // Advance virtual-display ownership before taking the registry
            // lock.  The state lock is always acquired before the registry
            // lock throughout the display/vdisplay integration.
            let (events, names) = vdisplay::on_platform_displays(&mgr, &displays);
            let (diff, list) = {
                let mut registry = mgr
                    .features()
                    .displays
                    .registry
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                let marked = registry.set_virtual(names);
                let diff = registry.update(displays);
                let list = (!diff.is_empty() || marked).then(|| registry.list());
                (diff, list)
            };
            if let Some(list) = list {
                mgr.broadcast(list.into());
            }
            if !diff.is_empty() {
                mgr.post_all(|_| {
                    Some(FeatureEvent::Displays(Event::RegistryChanged(diff.clone())))
                });
            }
            vdisplay::dispatch(&mgr, events);
        }
    });
}

/// Refresh the registry from the platform's current display list and the
/// virtual-display annotations owned by the vdisplay manager.
pub fn republish<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let names = vdisplay::virtual_names(mgr);
    let displays = mgr.platform().displays();
    let (diff, list) = {
        let mut registry = mgr
            .features()
            .displays
            .registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let marked = registry.set_virtual(names);
        let diff = registry.update(displays);
        let list = (!diff.is_empty() || marked).then(|| registry.list());
        (diff, list)
    };
    if let Some(list) = list {
        mgr.broadcast(list.into());
    }
    if !diff.is_empty() {
        mgr.post_all(|_| Some(FeatureEvent::Displays(Event::RegistryChanged(diff.clone()))));
    }
}
pub fn on_admitted<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    let Event::RegistryChanged(diff) = ev;
    let action = {
        let registry = cx
            .mgr
            .features()
            .displays
            .registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cx.st.displays.sd.on_registry_change(&registry, &diff)
    };
    match action {
        DisplayAction::Rebuild {
            wire_id, display, ..
        } => stream::rebuild(cx, target(wire_id, display)),
        DisplayAction::Suspend { .. } => stream::stop(cx),
        DisplayAction::None => {}
    }
}
pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn on_closed<P: Platform>(_cx: &mut ConnCx<'_, P>, _reason: EndReason) {}

pub fn hello_displays<P: Platform>(mgr: &Arc<SessionManager<P>>) -> Vec<pb::Display> {
    mgr.features()
        .displays
        .registry
        .lock()
        .unwrap_or_else(|poison| poison.into_inner())
        .displays_pb()
}
pub fn resolve_start<P: Platform>(
    cx: &mut ConnCx<'_, P>,
    requested: u32,
) -> Result<StreamTarget, pb::ErrorInfo> {
    let action = {
        let registry = cx
            .mgr
            .features()
            .displays
            .registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cx.st.displays.sd.start(&registry, requested)
    }
    .map_err(select_error)?;
    match action {
        DisplayAction::Rebuild {
            wire_id, display, ..
        } => Ok(target(wire_id, display)),
        // StreamDisplay::start currently always rebuilds on success.
        DisplayAction::None | DisplayAction::Suspend { .. } => {
            Err(select_error(SelectError::NoDisplays))
        }
    }
}
pub fn on_select_display<P: Platform>(cx: &mut ConnCx<'_, P>, m: pb::SelectDisplay) {
    let action = {
        let registry = cx
            .mgr
            .features()
            .displays
            .registry
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        cx.st.displays.sd.select(&registry, m.display_id)
    };
    match action {
        Ok(DisplayAction::Rebuild {
            wire_id, display, ..
        }) => stream::rebuild(cx, target(wire_id, display)),
        Ok(DisplayAction::None | DisplayAction::Suspend { .. }) => {}
        Err(error) => cx.send(select_error(error)),
    }
}

fn target(wire_id: u32, display: DisplayInfo) -> StreamTarget {
    // Registry ids are always 1..=255. Only the stream layer owns wire epochs.
    StreamTarget {
        display,
        wire_id,
        header_id: wire_id as u8,
    }
}

fn select_error(error: SelectError) -> pb::ErrorInfo {
    let message = error.to_string();
    let (key, value) = match error {
        SelectError::NoDisplays => ("display", "none".to_owned()),
        SelectError::UnknownDisplay(id) => ("display_id", id.to_string()),
    };
    pb::ErrorInfo {
        code: u32::from(ErrorCode::Other.code()),
        message,
        detail: [(key.to_owned(), value)].into(),
        ..Default::default()
    }
}
