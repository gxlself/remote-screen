//! Clipboard stream routing (03 section 8.1).
use super::ConnCx;
use crate::{
    auth,
    config::HostConfig,
    platform::Platform,
    session::{
        clipboard::{
            self, apply_inbound, ClipDirections, ClipboardSync, InboundError, OutgoingClip,
            PlatformClipboard,
        },
        manager::{ConnKey, EndReason, HostServices, SessionManager},
    },
};
use rsp_core::{clipboard::ClipboardKind, features};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::{mpsc, watch};
use tracing::{debug, warn};

pub struct Host {
    sync: Arc<Mutex<ClipboardSync>>,
    controller: watch::Sender<Option<u64>>,
    controller_key: Mutex<Option<ConnKey>>,
}

impl Host {
    pub fn new(services: &HostServices) -> Self {
        let directions = ClipDirections::from(&services.config.get().clipboard);
        Self {
            sync: Arc::new(Mutex::new(ClipboardSync::new(directions))),
            controller: watch::channel(None).0,
            controller_key: Mutex::new(None),
        }
    }
}

#[derive(Default)]
pub struct Conn {}

#[derive(Debug)]
pub enum Event {
    Send(OutgoingClip),
}

pub const FEATURES: u64 = features::CLIPBOARD_TEXT | features::CLIPBOARD_IMAGE;

pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let platform = mgr.platform().clone();
    let (changes_tx, changes_rx) = mpsc::channel::<()>(8);
    platform.watch_clipboard(changes_tx);

    let (out_tx, out_rx) = mpsc::channel::<OutgoingClip>(8);
    let sync = mgr.features().clipboard.sync.clone();
    let controller = mgr.features().clipboard.controller.subscribe();
    let port: Arc<dyn clipboard::ClipboardPort> = Arc::new(PlatformClipboard(platform.clone()));
    tokio::spawn(clipboard::pump_local_changes(
        port, changes_rx, sync, controller, out_tx,
    ));

    let config = mgr.services().config.subscribe();
    tokio::spawn(dispatch(Arc::downgrade(mgr), out_rx, config));
}

async fn dispatch<P: Platform>(
    mgr: Weak<SessionManager<P>>,
    mut out: mpsc::Receiver<OutgoingClip>,
    mut config: watch::Receiver<Arc<HostConfig>>,
) {
    if let Some(mgr) = mgr.upgrade() {
        set_directions(&mgr, &config.borrow());
    }
    loop {
        tokio::select! {
            clip = out.recv() => {
                let Some(clip) = clip else { break; };
                let Some(mgr) = mgr.upgrade() else { break; };
                if let Some(key) = current_controller(&mgr) {
                    mgr.post(key, super::FeatureEvent::Clipboard(Event::Send(clip)));
                }
            }
            changed = config.changed() => {
                if changed.is_err() {
                    break;
                }
                let Some(mgr) = mgr.upgrade() else { break; };
                set_directions(&mgr, &config.borrow_and_update());
            }
        }
    }
}

fn set_directions<P: Platform>(mgr: &Arc<SessionManager<P>>, config: &HostConfig) {
    let directions = ClipDirections::from(&config.clipboard);
    let mut sync = mgr
        .features()
        .clipboard
        .sync
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    sync.set_directions(directions);
}

fn current_controller<P: Platform>(mgr: &SessionManager<P>) -> Option<ConnKey> {
    *mgr.features()
        .clipboard
        .controller_key
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

fn refresh_controller<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    refresh_controller_excluding(mgr, None);
}

fn refresh_controller_excluding<P: Platform>(
    mgr: &Arc<SessionManager<P>>,
    excluded: Option<ConnKey>,
) {
    let host = &mgr.features().clipboard;
    let controller = mgr
        .conns()
        .into_iter()
        .find(|info| info.role == auth::Role::Controller && Some(info.key) != excluded);
    let next = controller
        .as_ref()
        .map(|info| (info.key, info.client_features));

    let mut current = host
        .controller_key
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner());
    let next_key = next.map(|(key, _)| key);
    if *current == next_key {
        return;
    }
    *current = next_key;
    drop(current);

    host.controller
        .send_replace(next.map(|(_, client_features)| client_features));
    host.sync
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
        .on_controller_changed();
}

pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    refresh_controller(cx.mgr);
}

pub fn on_role_changed<P: Platform>(cx: &mut ConnCx<'_, P>, _old: auth::Role) {
    refresh_controller(cx.mgr);
}

pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {
        Event::Send(clip) => {
            if current_controller(cx.mgr) != Some(cx.key) || !cx.is_controller() {
                debug!(
                    kind = %clip.item.kind,
                    len = clip.item.data.len(),
                    "clipboard event skipped for non-controller"
                );
                return;
            }
            let required_feature = match clip.item.kind {
                ClipboardKind::Text => features::CLIPBOARD_TEXT,
                ClipboardKind::Png => features::CLIPBOARD_IMAGE,
            };
            if cx.info.client_features & required_feature == 0 {
                debug!(
                    kind = %clip.item.kind,
                    len = clip.item.data.len(),
                    "clipboard event skipped for unsupported item kind"
                );
                return;
            }

            let kind = clip.item.kind;
            let len = clip.item.data.len();
            let conn = cx.conn.clone();
            cx.spawn(async move {
                let mut send = match conn.open_uni(rsp_transport::StreamKind::Clipboard).await {
                    Ok(send) => send,
                    Err(error) => {
                        debug!(%error, kind = %kind, len, "clipboard stream open failed");
                        return;
                    }
                };
                if let Err(error) = clipboard::write_uni_stream(&mut send, &clip.item).await {
                    debug!(%error, kind = %kind, len, "clipboard stream write failed");
                    let _ = send.reset(1);
                }
            });
        }
    }
}

pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}

pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}

pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    refresh_controller_excluding(cx.mgr, Some(cx.key));
}

pub fn on_stream<P: Platform>(cx: &mut ConnCx<'_, P>, kind: u8, mut recv: rsp_transport::UniRecv) {
    if kind != clipboard::CLIPBOARD_STREAM_TYPE {
        let _ = recv.stop(1);
        return;
    }

    let from_controller = cx.is_controller();
    let sync = cx.mgr.features().clipboard.sync.clone();
    let port = PlatformClipboard(cx.mgr.platform().clone());
    cx.spawn(async move {
        let item = match clipboard::read_uni_stream(&mut recv).await {
            Ok(item) => item,
            Err(error) => {
                let _ = recv.stop(1);
                debug!(%error, "clipboard stream rejected");
                return;
            }
        };
        let kind = item.kind;
        let len = item.data.len();
        match apply_inbound(&port, &sync, from_controller, item) {
            Ok(()) => {}
            Err(InboundError::Rejected(reason)) => {
                debug!(?reason, kind = %kind, len, "clipboard item rejected");
            }
            Err(InboundError::Platform(error)) => {
                warn!(%error, kind = %kind, len, "clipboard platform write failed");
            }
        }
    });
}
