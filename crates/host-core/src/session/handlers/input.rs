//! Reliable input records and the connection's shared injection gate.
use super::{cursor, displays, idle, ConnCx, FeatureEvent};
use crate::{
    auth,
    platform::{input_from_m0, DisplayInfo, Platform},
    session::manager::{ConnCommand, ConnKey, EndReason, HostServices, SessionManager},
};
use rsp_core::{
    features,
    hid::InputMsg,
    input::InputEvent,
    pointer::{PointerDatagram, PointerFilter},
};
use std::{
    collections::HashMap,
    sync::{
        atomic::{AtomicU64, Ordering},
        Arc, Mutex,
    },
};
use tokio::sync::{mpsc, watch};
use tracing::debug;

/// Per-record allocation limit until the protocol specifies an input limit.
pub const MAX_INPUT_RECORD: usize = 64 * 1024;
const UNCOORDINATED_REPORT_INTERVAL_US: u64 = 1_000_000;

pub struct InputPort<P: Platform> {
    pub platform: Arc<P>,
    pub gate: watch::Receiver<bool>,
    pub display: watch::Receiver<Option<(DisplayInfo, u8)>>,
    pub cmd: mpsc::Sender<ConnCommand>,
    pub dropped: Arc<AtomicU64>,
    pub stats: Arc<InputCounters>,
}
// Deriving Clone would unnecessarily require the platform itself to be Clone.
impl<P: Platform> Clone for InputPort<P> {
    fn clone(&self) -> Self {
        Self {
            platform: self.platform.clone(),
            gate: self.gate.clone(),
            display: self.display.clone(),
            cmd: self.cmd.clone(),
            dropped: self.dropped.clone(),
            stats: self.stats.clone(),
        }
    }
}

#[derive(Debug, Default)]
pub struct InputCounters {
    pub dropped: Arc<AtomicU64>,
    pub stale: AtomicU64,
    pub wrong_display: AtomicU64,
    pub bad: AtomicU64,
    pub injected: AtomicU64,
    reported: AtomicU64,
}
impl InputCounters {
    #[doc(hidden)]
    pub fn reported(&self) -> u64 {
        self.reported.load(Ordering::Relaxed)
    }
}

pub struct Host {
    counters: Mutex<HashMap<ConnKey, Arc<InputCounters>>>,
}
impl Host {
    pub fn new(_s: &HostServices) -> Self {
        Self {
            counters: Mutex::new(HashMap::new()),
        }
    }

    #[doc(hidden)]
    pub fn counters(&self, key: ConnKey) -> Option<Arc<InputCounters>> {
        self.counters
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .get(&key)
            .cloned()
    }

    fn register(&self, key: ConnKey, counters: Arc<InputCounters>) {
        self.counters
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .insert(key, counters);
    }

    fn unregister(&self, key: ConnKey) {
        self.counters
            .lock()
            .unwrap_or_else(|poison| poison.into_inner())
            .remove(&key);
    }
}
pub struct Conn {
    pub gate_tx: watch::Sender<bool>,
    pub display_tx: watch::Sender<Option<(DisplayInfo, u8)>>,
    pub gate_open: bool,
    pub dropped: Arc<AtomicU64>,
    pub stats: Arc<InputCounters>,
}
impl Default for Conn {
    fn default() -> Self {
        let stats = Arc::new(InputCounters::default());
        Self {
            gate_tx: watch::channel(false).0,
            display_tx: watch::channel(None).0,
            gate_open: false,
            dropped: stats.dropped.clone(),
            stats,
        }
    }
}
#[derive(Debug)]
pub enum Event {
    Injected { pointer: Option<(f32, f32)> },
}
pub const FEATURES: u64 = features::KEY_HID | features::TEXT_INPUT;
pub fn on_host_start<P: Platform>(_mgr: &Arc<SessionManager<P>>) {}
pub fn on_admitted<P: Platform>(cx: &mut ConnCx<'_, P>) {
    cx.mgr
        .features()
        .input
        .register(cx.key, cx.st.input.stats.clone());
    let native = cx.platform().displays();
    let hello = displays::hello_displays(cx.mgr);
    if let Some(display) = native.iter().find(|d| d.primary).or_else(|| native.first()) {
        if let Some(wire) = hello.iter().find(|d| d.primary).or_else(|| hello.first()) {
            let _ = cx
                .st
                .input
                .display_tx
                .send(Some((display.clone(), wire.id as u8)));
        }
    }
}
pub fn on_role_changed<P: Platform>(_cx: &mut ConnCx<'_, P>, _old: auth::Role) {}
pub fn on_stream_started<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_stream_stopped<P: Platform>(_cx: &mut ConnCx<'_, P>) {}
pub fn on_event<P: Platform>(cx: &mut ConnCx<'_, P>, ev: Event) {
    match ev {
        Event::Injected { pointer } => {
            if !cx.is_controller() {
                return;
            }
            idle::on_controller_input(cx);
            if let Some((x, y)) = pointer {
                cursor::on_injected(cx, x, y);
            }
            cx.st.input.stats.reported.fetch_add(1, Ordering::Relaxed);
        }
    }
}
pub fn deadline<P: Platform>(_cx: &ConnCx<'_, P>) -> Option<u64> {
    None
}
pub fn on_tick<P: Platform>(_cx: &mut ConnCx<'_, P>, _now_us: u64) {}
pub fn on_closed<P: Platform>(cx: &mut ConnCx<'_, P>, _reason: EndReason) {
    let stats = &cx.st.input.stats;
    debug!(
        dropped = stats.dropped.load(Ordering::Relaxed),
        stale = stats.stale.load(Ordering::Relaxed),
        wrong_display = stats.wrong_display.load(Ordering::Relaxed),
        bad = stats.bad.load(Ordering::Relaxed),
        injected = stats.injected.load(Ordering::Relaxed),
        reported = stats.reported(),
        "input injection stats"
    );
    cx.mgr.features().input.unregister(cx.key);
    if cx.st.input.gate_open {
        cx.st.input.gate_open = false;
        let _ = cx.st.input.gate_tx.send(false);
    }
    cx.platform().release_all();
}

/// Consume one stream. Legacy streams have already lost their first zero
/// length byte to the dispatcher, so restore it for the first record.
/// Authorization is sampled at the first byte and again after each record.
pub async fn read_stream<P: Platform>(
    port: InputPort<P>,
    mut recv: rsp_transport::UniRecv,
    legacy: bool,
) {
    let mut restore_zero = legacy;
    let mut last_uncoordinated_report_us = None;
    loop {
        let mut header = [0; 4];
        if restore_zero {
            restore_zero = false;
        } else if !matches!(recv.read_raw(&mut header[..1]).await, Ok(Some(1))) {
            return;
        }
        let started_open = *port.gate.borrow();
        if read_exact(&mut recv, &mut header[1..]).await.is_err() {
            return;
        }
        let len = u32::from_be_bytes(header) as usize;
        if len > MAX_INPUT_RECORD {
            port.stats.bad.fetch_add(1, Ordering::Relaxed);
            let _ = recv.stop(1);
            return;
        }
        let mut payload = vec![0; len];
        if read_exact(&mut recv, &mut payload).await.is_err() {
            return;
        }
        // Keep the read lock through injection: closing the gate must publish
        // false before release_all, and no earlier record may inject after it.
        let gate = port.gate.borrow();
        let msg = if legacy {
            InputEvent::decode(&payload).map(input_from_m0)
        } else {
            InputMsg::decode(&payload)
        };
        let Ok(msg) = msg else {
            port.stats.bad.fetch_add(1, Ordering::Relaxed);
            let _ = recv.stop(1);
            return;
        };
        if !started_open || !*gate {
            port.stats.dropped.fetch_add(1, Ordering::Relaxed);
            continue;
        }
        if let Some((display, _)) = port.display.borrow().as_ref() {
            port.platform.inject_input(&msg, display);
            port.stats.injected.fetch_add(1, Ordering::Relaxed);
            let pointer = match msg {
                InputMsg::MouseMove { x, y } | InputMsg::MouseButton { x, y, .. } => Some((x, y)),
                _ => None,
            };
            let now_us = rsp_core::clock::unix_micros();
            if pointer.is_some()
                || last_uncoordinated_report_us.is_none_or(|last| {
                    now_us.saturating_sub(last) >= UNCOORDINATED_REPORT_INTERVAL_US
                })
            {
                report_injected(&port, pointer);
                if pointer.is_none() {
                    last_uncoordinated_report_us = Some(now_us);
                }
            }
        }
    }
}

/// Read exactly the requested raw bytes; preserve first-byte gate sampling.
async fn read_exact(
    recv: &mut rsp_transport::UniRecv,
    mut bytes: &mut [u8],
) -> Result<(), rsp_transport::ConnError> {
    while !bytes.is_empty() {
        let count = recv
            .read_raw(bytes)
            .await?
            .ok_or_else(|| std::io::Error::from(std::io::ErrorKind::UnexpectedEof))?;
        bytes = &mut bytes[count..];
    }
    Ok(())
}

pub fn on_pointer_datagram<P: Platform>(
    port: &InputPort<P>,
    filter: &mut PointerFilter,
    payload: &[u8],
) {
    let Ok(datagram) = PointerDatagram::decode(payload) else {
        port.stats.bad.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if !filter.accept(&datagram) {
        port.stats.stale.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let gate = port.gate.borrow();
    if !*gate {
        port.stats.dropped.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let display = port.display.borrow();
    let Some((display, header_id)) = display.as_ref() else {
        port.stats.wrong_display.fetch_add(1, Ordering::Relaxed);
        return;
    };
    if *header_id != datagram.display {
        port.stats.wrong_display.fetch_add(1, Ordering::Relaxed);
        return;
    }
    let msg = InputMsg::MouseMove {
        x: datagram.x,
        y: datagram.y,
    };
    port.platform.inject_input(&msg, display);
    port.stats.injected.fetch_add(1, Ordering::Relaxed);
    report_injected(port, Some((datagram.x, datagram.y)));
}

fn report_injected<P: Platform>(port: &InputPort<P>, pointer: Option<(f32, f32)>) {
    let _ = port
        .cmd
        .try_send(ConnCommand::Feature(FeatureEvent::Input(Event::Injected {
            pointer,
        })));
}

pub fn on_gate_change<P: Platform>(_cx: &mut ConnCx<'_, P>, _open: bool) {}
