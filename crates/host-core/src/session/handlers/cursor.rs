//! Cursor stream and pointer datagram routing (03 sections 2, 4.5, and 6).
use super::{stream, ConnCx};
use crate::{
    auth,
    platform::{CursorUpdate, Platform},
    session::{
        cursor::{ConnCursor, CursorChange, CursorHub, CursorOut},
        manager::{EndReason, HostServices, SessionManager},
    },
};
use rsp_core::features;
use rsp_transport::{DatagramKind, StreamKind};
use std::sync::{Arc, Mutex, Weak};
use tokio::sync::mpsc;
use tracing::debug;

const QUEUE_CAPACITY: usize = 64;

pub struct Host {
    pub hub: Mutex<CursorHub>,
}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            hub: Mutex::new(CursorHub::new()),
        }
    }
}

#[derive(Default)]
pub struct Conn {
    cc: Option<ConnCursor>,
    tx: Option<mpsc::Sender<Vec<u8>>>,
    dropped: u64,
}

#[derive(Debug)]
pub enum Event {
    Change(CursorChange),
    Injected(f32, f32),
}

pub const FEATURES: u64 = features::CURSOR_SHAPE | features::POINTER_DGRAM;

pub fn on_host_start<P: Platform>(mgr: &Arc<SessionManager<P>>) {
    let platform = mgr.platform().clone();
    let mgr: Weak<SessionManager<P>> = Arc::downgrade(mgr);
    let (tx, mut rx) = mpsc::channel::<CursorUpdate>(QUEUE_CAPACITY);
    platform.watch_cursor(tx);
    tokio::spawn(async move {
        while let Some(update) = rx.recv().await {
            let Some(strong) = mgr.upgrade() else {
                break;
            };
            let change = {
                let mut hub = strong
                    .features()
                    .cursor
                    .hub
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                hub.apply(update)
            };
            if let Some(change) = change {
                debug!(?change, "cursor change");
                strong.post_all(|_| Some(super::FeatureEvent::Cursor(Event::Change(change))));
            }
        }
    });
}

pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let cc = ConnCursor::new(cx.info.client_features, cx.is_controller());
    cx.st.cursor.cc = Some(cc);
    if cx.info.client_features & features::CURSOR_SHAPE == 0 {
        return;
    }
    let (tx, mut rx) = mpsc::channel::<Vec<u8>>(QUEUE_CAPACITY);
    cx.st.cursor.tx = Some(tx);
    let conn = cx.conn.clone();
    cx.spawn(async move {
        let mut stream = None;
        while let Some(record) = rx.recv().await {
            let send = match stream.as_mut() {
                Some(send) => send,
                None => match conn.open_uni(StreamKind::Cursor).await {
                    Ok(send) => stream.insert(send),
                    Err(error) => {
                        debug!(%error, "cursor stream open failed");
                        return;
                    }
                },
            };
            // CursorOut includes the QUIC length prefix; TCP adds record framing.
            let result = if conn.kind() == rsp_transport::TransportKind::Tcp {
                send.send_record(&record[4..]).await
            } else {
                send.write_raw(&record).await
            };
            if let Err(error) = result {
                debug!(%error, "cursor stream write failed");
                return;
            }
        }
    });
}

pub fn on_role_changed<P: Platform>(cx: &mut ConnCx<'_, P>, _old: auth::Role) {
    let is_controller = cx.is_controller();
    if let Some(cc) = cx.st.cursor.cc.as_mut() {
        cc.set_controller(is_controller);
    }
}

pub fn on_stream_started<P: Platform>(cx: &mut ConnCx<'_, P>) {
    let Some(target) = stream::current_target(cx) else {
        return;
    };
    let outs = {
        let mut hub = cx
            .mgr
            .features()
            .cursor
            .hub
            .lock()
            .unwrap_or_else(|poison| poison.into_inner());
        let mut outs = Vec::new();
        if let Some(cc) = cx.st.cursor.cc.as_mut() {
            cc.set_stream_display(target.display.id, target.header_id);
            outs = cc.attach(&mut hub);
        }
        outs
    };
    dispatch(cx, outs);
}

pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}

pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {
        Event::Change(change) => {
            let now = cx.now_us();
            let outs = {
                let mut hub = cx
                    .mgr
                    .features()
                    .cursor
                    .hub
                    .lock()
                    .unwrap_or_else(|poison| poison.into_inner());
                cx.st
                    .cursor
                    .cc
                    .as_mut()
                    .map_or_else(Vec::new, |cc| cc.on_change(&change, &mut hub, now))
            };
            dispatch(cx, outs);
        }
        Event::Injected(x, y) => on_injected(cx, x, y),
    }
}

pub fn deadline<P: Platform>(cx: &ConnCx<'_, P>) -> Option<u64> {
    cx.st.cursor.cc.as_ref().and_then(ConnCursor::next_deadline)
}

pub fn on_tick<P: Platform>(cx: &mut ConnCx<'_, P>, now_us: u64) {
    let outs = cx
        .st
        .cursor
        .cc
        .as_mut()
        .map_or_else(Vec::new, |cc| cc.poll(now_us));
    dispatch(cx, outs);
}

pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    if cx.st.cursor.dropped != 0 {
        debug!(dropped = cx.st.cursor.dropped, "cursor records dropped");
    }
    let _ = cx.st.cursor.tx.take();
}

pub fn set_stream_display<P: Platform>(cx: &mut ConnCx<'_, P>, platform_id: u32, wire_id: u8) {
    if let Some(cc) = cx.st.cursor.cc.as_mut() {
        cc.set_stream_display(platform_id, wire_id);
    }
}

pub fn stream_config_cursor<P: Platform>(cx: &ConnCx<'_, P>) -> (f32, f32, bool) {
    let hub = cx
        .mgr
        .features()
        .cursor
        .hub
        .lock()
        .unwrap_or_else(|poison| poison.into_inner());
    cx.st
        .cursor
        .cc
        .as_ref()
        .map_or((0.5, 0.5, false), |cc| cc.stream_config_cursor(&hub))
}

pub fn on_injected<P: Platform>(cx: &mut ConnCx<'_, P>, x: f32, y: f32) {
    let now = cx.now_us();
    if let Some(cc) = cx.st.cursor.cc.as_mut() {
        cc.note_injected(x, y, now);
    }
}

fn dispatch<P: Platform>(cx: &mut ConnCx<'_, P>, outs: Vec<CursorOut>) {
    for out in outs {
        match out {
            CursorOut::Stream(record) => {
                if let Some(tx) = &cx.st.cursor.tx {
                    match tx.try_send(record) {
                        Ok(()) => {}
                        Err(mpsc::error::TrySendError::Full(_)) => {
                            cx.st.cursor.dropped = cx.st.cursor.dropped.saturating_add(1);
                        }
                        Err(mpsc::error::TrySendError::Closed(_)) => {
                            debug!("cursor stream queue closed");
                        }
                    }
                }
            }
            CursorOut::Datagram(datagram) => {
                debug!(?datagram, "cursor datagram");
                if let Err(error) = cx.conn.send_datagram(
                    DatagramKind::Pointer,
                    bytes::Bytes::copy_from_slice(&datagram[1..]),
                ) {
                    debug!(%error, "cursor datagram send failed");
                }
            }
        }
    }
}
