//! Input injection through the portal RemoteDesktop interface.
//!
//! Planning is deliberately kept on the caller's thread. The portal calls are
//! serialized by a small asynchronous worker so an input callback never waits
//! for D-Bus I/O.

use std::{
    collections::HashMap,
    sync::{Arc, Mutex, MutexGuard},
    time::{Duration, Instant},
};

use rsp_core::hid::InputMsg;
use tokio::sync::{mpsc, oneshot};
use zbus::{message::Flags, Message, OwnedObjectPath, OwnedVariant};

use crate::input_plan::{DisplayGeom, LinuxInputOp, LinuxPlanner, ScrollMode, WheelAxis};

// These values mirror the stable names and device bits from portal_model.
const PORTAL_BUS: &str = "org.freedesktop.portal.Desktop";
const PORTAL_PATH: &str = "/org/freedesktop/portal/desktop";
const IFACE_RD: &str = "org.freedesktop.portal.RemoteDesktop";
const KEYBOARD: u32 = 1;
const POINTER: u32 = 2;

type Options = HashMap<String, OwnedVariant>;

/// The display and RemoteDesktop stream receiving an input operation.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct PortalTarget {
    pub display_id: u32,
    pub node_id: u32,
    pub geom: DisplayGeom,
}

/// Counters for the asynchronous portal sender.
#[derive(Debug, Clone, Copy, Default, PartialEq, Eq)]
pub struct PortalInputStats {
    pub sent: u64,
    pub no_device: u64,
    pub queue_full: u64,
    pub send_errors: u64,
}

/// The most recently queued absolute pointer position.
#[derive(Debug, Clone, Copy, PartialEq)]
pub struct LastPointer {
    pub display_id: u32,
    pub x: f64,
    pub y: f64,
    pub at: Instant,
}

#[derive(Debug)]
struct Inner {
    tx: mpsc::Sender<QueueItem>,
    planner: Arc<Mutex<LinuxPlanner>>,
    stats: Arc<Mutex<PortalInputStats>>,
    last: Arc<Mutex<Option<LastPointer>>>,
    last_target: Arc<Mutex<Option<PortalTarget>>>,
    started: Instant,
}

#[derive(Debug)]
enum QueueItem {
    Ops {
        target: Option<PortalTarget>,
        ops: Vec<LinuxInputOp>,
    },
    Wake,
    Barrier(oneshot::Sender<()>),
}

#[derive(Debug)]
struct SenderState {
    planner: Arc<Mutex<LinuxPlanner>>,
    stats: Arc<Mutex<PortalInputStats>>,
    last_target: Arc<Mutex<Option<PortalTarget>>>,
    started: Instant,
}

/// Translate RSP input and send it through portal RemoteDesktop.
#[derive(Clone, Debug)]
pub struct PortalInjector {
    inner: Arc<Inner>,
}

impl PortalInjector {
    /// Maximum number of planned input batches waiting for the sender.
    pub const QUEUE: usize = 512;

    /// Start the portal sender task on `rt`.
    pub fn new(
        conn: zbus::Connection,
        session: String,
        devices: u32,
        rt: &tokio::runtime::Handle,
    ) -> Self {
        let (tx, rx) = mpsc::channel(Self::QUEUE);
        let planner = Arc::new(Mutex::new(LinuxPlanner::new(ScrollMode::Portal)));
        let stats = Arc::new(Mutex::new(PortalInputStats::default()));
        let last = Arc::new(Mutex::new(None));
        let last_target = Arc::new(Mutex::new(None));
        let started = Instant::now();

        let sender_planner = Arc::clone(&planner);
        let sender_stats = Arc::clone(&stats);
        let sender_last_target = Arc::clone(&last_target);
        rt.spawn(send_loop(
            conn,
            session,
            devices,
            rx,
            SenderState {
                planner: sender_planner,
                stats: sender_stats,
                last_target: sender_last_target,
                started,
            },
        ));

        Self {
            inner: Arc::new(Inner {
                tx,
                planner,
                stats,
                last,
                last_target,
                started,
            }),
        }
    }

    /// Plan and enqueue an input message without waiting for D-Bus.
    pub fn inject(&self, msg: &InputMsg, target: PortalTarget) {
        let now_us = elapsed_us(self.inner.started);
        let ops = {
            let mut planner = lock(&self.inner.planner);
            planner.plan(msg, target.geom, now_us)
        };
        if ops.is_empty() {
            // The target remains useful for a later ReleaseAll even when this
            // particular message produced no operation (for example an
            // unmapped key).
            *lock(&self.inner.last_target) = Some(target);
            return;
        }

        match self.inner.tx.try_send(QueueItem::Ops {
            target: Some(target),
            ops: ops.clone(),
        }) {
            Ok(()) => {
                *lock(&self.inner.last_target) = Some(target);
                let now = Instant::now();
                let mut last = lock(&self.inner.last);
                for op in ops {
                    if let LinuxInputOp::MoveAbs { x, y } = op {
                        *last = Some(LastPointer {
                            display_id: target.display_id,
                            x,
                            y,
                            at: now,
                        });
                    }
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
                lock(&self.inner.stats).queue_full += 1;
            }
        }
    }

    /// Release all keys, buttons, and an active smooth scroll.
    pub fn release_all(&self) {
        let ops = {
            let mut planner = lock(&self.inner.planner);
            planner.release_all()
        };
        if ops.is_empty() {
            return;
        }
        let target = *lock(&self.inner.last_target);
        match self.inner.tx.try_send(QueueItem::Ops { target, ops }) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
                lock(&self.inner.stats).queue_full += 1;
            }
        }
    }

    /// Wake the display compositor with a zero relative motion.
    pub fn wake(&self) {
        match self.inner.tx.try_send(QueueItem::Wake) {
            Ok(()) => {}
            Err(mpsc::error::TrySendError::Full(_)) | Err(mpsc::error::TrySendError::Closed(_)) => {
                lock(&self.inner.stats).queue_full += 1;
            }
        }
    }

    /// Return the most recently queued absolute pointer position.
    pub fn last_pointer(&self) -> Option<LastPointer> {
        *lock(&self.inner.last)
    }

    /// Return sender counters.
    pub fn stats(&self) -> PortalInputStats {
        *lock(&self.inner.stats)
    }

    /// Return planner counters.
    pub fn planner_stats(&self) -> crate::input_plan::LinuxInputStats {
        lock(&self.inner.planner).stats()
    }

    /// Wait until all queue items present at the call have been processed.
    pub async fn flush(&self) {
        let (done, wait) = oneshot::channel();
        if self.inner.tx.send(QueueItem::Barrier(done)).await.is_ok() {
            let _ = wait.await;
        }
    }
}

impl Drop for PortalInjector {
    fn drop(&mut self) {
        // Dropping the final Arc sender closes the channel. The worker owns
        // the receiver and drains all already-enqueued items before exiting.
    }
}

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex
        .lock()
        .unwrap_or_else(std::sync::PoisonError::into_inner)
}

fn elapsed_us(started: Instant) -> u64 {
    started.elapsed().as_micros().min(u128::from(u64::MAX)) as u64
}

async fn send_loop(
    conn: zbus::Connection,
    session: String,
    devices: u32,
    mut rx: mpsc::Receiver<QueueItem>,
    state: SenderState,
) {
    let SenderState {
        planner,
        stats,
        last_target,
        started,
    } = state;
    let session = OwnedObjectPath::try_from(session).ok();
    let mut worker_target = None;

    loop {
        let deadline = lock(&planner).next_deadline();
        if let Some(deadline_us) = deadline {
            let remaining = deadline_us.saturating_sub(elapsed_us(started));
            let sleep = tokio::time::sleep_until(
                tokio::time::Instant::now() + Duration::from_micros(remaining),
            );
            tokio::pin!(sleep);
            tokio::select! {
                item = rx.recv() => {
                    if !process_item(
                        item,
                        &conn,
                        session.as_ref(),
                        devices,
                        &stats,
                        &mut worker_target,
                    ).await {
                        break;
                    }
                }
                () = &mut sleep => {
                    // If the queue is busy, process its already queued work
                    // first so AxisStop cannot overtake the scroll events.
                    while let Ok(item) = rx.try_recv() {
                        if !process_item(
                            Some(item),
                            &conn,
                            session.as_ref(),
                            devices,
                            &stats,
                            &mut worker_target,
                        ).await {
                            return;
                        }
                    }
                    let ops = {
                        let mut planner = lock(&planner);
                        planner.poll(elapsed_us(started))
                    };
                    if !ops.is_empty() {
                        let target = (*lock(&last_target)).or(worker_target);
                        process_ops(
                            &conn,
                            session.as_ref(),
                            devices,
                            target,
                            ops,
                            &stats,
                        )
                        .await;
                    }
                }
            }
        } else {
            let item = rx.recv().await;
            if !process_item(
                item,
                &conn,
                session.as_ref(),
                devices,
                &stats,
                &mut worker_target,
            )
            .await
            {
                break;
            }
        }
    }
}

async fn process_item(
    item: Option<QueueItem>,
    conn: &zbus::Connection,
    session: Option<&OwnedObjectPath>,
    devices: u32,
    stats: &Arc<Mutex<PortalInputStats>>,
    worker_target: &mut Option<PortalTarget>,
) -> bool {
    let Some(item) = item else {
        return false;
    };
    match item {
        QueueItem::Ops { target, ops } => {
            if target.is_some() {
                *worker_target = target;
            }
            process_ops(
                conn,
                session,
                devices,
                target.or(*worker_target),
                ops,
                stats,
            )
            .await;
        }
        QueueItem::Wake => {
            process_wake(conn, session, devices, stats).await;
        }
        QueueItem::Barrier(done) => {
            let _ = done.send(());
        }
    }
    true
}

async fn process_ops(
    conn: &zbus::Connection,
    session: Option<&OwnedObjectPath>,
    devices: u32,
    target: Option<PortalTarget>,
    ops: Vec<LinuxInputOp>,
    stats: &Arc<Mutex<PortalInputStats>>,
) {
    for op in ops {
        let method = method_for(&op);
        let Some(session) = session else {
            record_send_error(stats, method);
            continue;
        };
        let result = match op {
            LinuxInputOp::Key { code, down } if devices & KEYBOARD != 0 => {
                send_call(
                    conn,
                    session,
                    "NotifyKeyboardKeycode",
                    &(session, Options::new(), i32::from(code), u32::from(down)),
                )
                .await
            }
            LinuxInputOp::Keysym { sym, down } if devices & KEYBOARD != 0 => {
                send_call(
                    conn,
                    session,
                    "NotifyKeyboardKeysym",
                    &(session, Options::new(), sym as i32, u32::from(down)),
                )
                .await
            }
            LinuxInputOp::MoveAbs { x, y } if devices & POINTER != 0 && target.is_some() => {
                let Some(target) = target else {
                    continue;
                };
                send_call(
                    conn,
                    session,
                    "NotifyPointerMotionAbsolute",
                    &(session, Options::new(), target.node_id, x, y),
                )
                .await
            }
            LinuxInputOp::Button { code, down } if devices & POINTER != 0 && target.is_some() => {
                send_call(
                    conn,
                    session,
                    "NotifyPointerButton",
                    &(session, Options::new(), i32::from(code), u32::from(down)),
                )
                .await
            }
            LinuxInputOp::Axis { dx, dy } if devices & POINTER != 0 && target.is_some() => {
                send_call(
                    conn,
                    session,
                    "NotifyPointerAxis",
                    &(session, Options::new(), dx, dy),
                )
                .await
            }
            LinuxInputOp::AxisStop if devices & POINTER != 0 && target.is_some() => {
                let mut options = Options::new();
                options.insert("finish".to_owned(), true.into());
                send_call(
                    conn,
                    session,
                    "NotifyPointerAxis",
                    &(session, options, 0.0_f64, 0.0_f64),
                )
                .await
            }
            LinuxInputOp::Wheel { axis, steps } if devices & POINTER != 0 && target.is_some() => {
                let axis = match axis {
                    WheelAxis::Vertical => 0_u32,
                    WheelAxis::Horizontal => 1_u32,
                };
                send_call(
                    conn,
                    session,
                    "NotifyPointerAxisDiscrete",
                    &(session, Options::new(), axis, steps),
                )
                .await
            }
            LinuxInputOp::Key { .. } | LinuxInputOp::Keysym { .. } if devices & KEYBOARD == 0 => {
                record_no_device(stats);
                continue;
            }
            LinuxInputOp::MoveAbs { .. }
            | LinuxInputOp::Button { .. }
            | LinuxInputOp::Axis { .. }
            | LinuxInputOp::AxisStop
            | LinuxInputOp::Wheel { .. }
                if devices & POINTER == 0 && target.is_some() =>
            {
                record_no_device(stats);
                continue;
            }
            // A release_all with no known target deliberately contains only
            // keyboard operations. Ignore any pointer operation without
            // incrementing no_device in case a future planner adds one.
            _ => continue,
        };
        match result {
            Ok(()) => lock(stats).sent += 1,
            Err(_) => record_send_error(stats, method),
        }
    }
}

async fn process_wake(
    conn: &zbus::Connection,
    session: Option<&OwnedObjectPath>,
    devices: u32,
    stats: &Arc<Mutex<PortalInputStats>>,
) {
    if devices & POINTER == 0 {
        record_no_device(stats);
        return;
    }
    let Some(session) = session else {
        record_send_error(stats, "NotifyPointerMotion");
        return;
    };
    let result = send_call(
        conn,
        session,
        "NotifyPointerMotion",
        &(session, Options::new(), 0.0_f64, 0.0_f64),
    )
    .await;
    match result {
        Ok(()) => lock(stats).sent += 1,
        Err(_) => record_send_error(stats, "NotifyPointerMotion"),
    }
}

fn method_for(op: &LinuxInputOp) -> &'static str {
    match op {
        LinuxInputOp::Key { .. } => "NotifyKeyboardKeycode",
        LinuxInputOp::Keysym { .. } => "NotifyKeyboardKeysym",
        LinuxInputOp::MoveAbs { .. } => "NotifyPointerMotionAbsolute",
        LinuxInputOp::Button { .. } => "NotifyPointerButton",
        LinuxInputOp::Axis { .. } | LinuxInputOp::AxisStop => "NotifyPointerAxis",
        LinuxInputOp::Wheel { .. } => "NotifyPointerAxisDiscrete",
    }
}

fn record_no_device(stats: &Arc<Mutex<PortalInputStats>>) {
    lock(stats).no_device += 1;
}

fn record_send_error(stats: &Arc<Mutex<PortalInputStats>>, method: &str) {
    lock(stats).send_errors += 1;
    tracing::debug!(method, "portal input send failed");
}

async fn send_call<B: serde::ser::Serialize + zbus::DynamicType>(
    conn: &zbus::Connection,
    _session: &OwnedObjectPath,
    member: &str,
    body: &B,
) -> zbus::Result<()> {
    let message = Message::method_call(PORTAL_PATH, member)
        .destination(PORTAL_BUS)
        .interface(IFACE_RD)
        .with_flags(Flags::NO_REPLY_EXPECTED)
        .build(body)?;
    let result = conn.send(&message).await;
    // Notify* stays no-reply; Peer.Ping is a standard transport barrier so
    // spawned zbus service handlers cannot reorder adjacent notifications.
    let _ = conn
        .call_method(
            Some(PORTAL_BUS),
            PORTAL_PATH,
            Some("org.freedesktop.DBus.Peer"),
            "Ping",
            &(),
        )
        .await;
    result
}
