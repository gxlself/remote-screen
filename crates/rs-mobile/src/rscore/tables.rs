//! Process-local session registries and serialized native video/terminal taps.

use std::{
    cell::RefCell,
    collections::{BTreeMap, VecDeque},
    sync::{
        atomic::{AtomicBool, Ordering},
        Arc, Mutex, MutexGuard,
    },
};

use rsp_client::{CursorEvent, ReceivedFrame, SessionHandle};
use rsp_core::pb;
use tokio::sync::{mpsc, Notify};

use super::dto_core::{id_str, OpenOpts, SessionPhase, SessionSummary, SshState};
use super::recording::slot::RecorderSlot;

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

pub struct SessionEntry {
    pub id: u64,
    pub host_id: String,
    pub started_at_us: u64,
    pub opts: OpenOpts,
    pub handle: Mutex<Option<SessionHandle>>,
    pub video: VideoTap,
    pub recorder: RecorderSlot,
    pub summary: Mutex<SessionSummary>,
    pub cancelled: AtomicBool,
    pub background: AtomicBool,
    pub bg_changed: Notify,
    pub cancel: Notify,
}

impl SessionEntry {
    pub fn summary(&self) -> SessionSummary {
        lock(&self.summary).clone()
    }

    pub fn update_summary(&self, f: impl FnOnce(&mut SessionSummary)) {
        f(&mut lock(&self.summary));
    }

    pub fn handle(&self) -> Option<SessionHandle> {
        lock(&self.handle).clone()
    }

    pub fn set_handle(&self, handle: Option<SessionHandle>) {
        *lock(&self.handle) = handle;
    }

    pub fn cancel(&self) {
        self.cancelled.store(true, Ordering::Release);
        self.cancel.notify_waiters();
    }

    pub fn is_cancelled(&self) -> bool {
        self.cancelled.load(Ordering::Acquire)
    }
}

#[derive(Default)]
pub struct SessionTable {
    entries: Mutex<Entries<SessionEntry>>,
}

// Allocation and insertion share a lock, so lists cannot observe later IDs
// before earlier insertions have completed. Removed IDs are never reused.
struct Entries<T> {
    last_id: u64,
    by_id: BTreeMap<u64, Arc<T>>,
}

impl<T> Default for Entries<T> {
    fn default() -> Self {
        Self {
            last_id: 0,
            by_id: BTreeMap::new(),
        }
    }
}

impl<T> Entries<T> {
    fn insert(&mut self, create: impl FnOnce(u64) -> T) -> Arc<T> {
        // Exhaustion must fail rather than wrap around and reuse an ID.
        self.last_id = self
            .last_id
            .checked_add(1)
            .expect("session ID space exhausted");
        let entry = Arc::new(create(self.last_id));
        self.by_id.insert(self.last_id, entry.clone());
        entry
    }
}

impl SessionTable {
    pub fn insert(
        &self,
        host_id: &str,
        host_name: &str,
        opts: OpenOpts,
        now_us: u64,
    ) -> Arc<SessionEntry> {
        lock(&self.entries).insert(|id| SessionEntry {
            id,
            host_id: host_id.into(),
            started_at_us: now_us,
            opts,
            handle: Mutex::new(None),
            video: VideoTap::default(),
            recorder: RecorderSlot::default(),
            summary: Mutex::new(SessionSummary {
                id: id_str(id),
                host_id: host_id.into(),
                host_name: host_name.into(),
                phase: SessionPhase::Connecting,
                role: None,
                path: None,
                transport: None,
                started_at: now_us,
                display_id: None,
            }),
            cancelled: AtomicBool::new(false),
            background: AtomicBool::new(false),
            bg_changed: Notify::new(),
            cancel: Notify::new(),
        })
    }

    pub fn get(&self, id: u64) -> Option<Arc<SessionEntry>> {
        lock(&self.entries).by_id.get(&id).cloned()
    }

    pub fn remove(&self, id: u64) -> Option<Arc<SessionEntry>> {
        lock(&self.entries).by_id.remove(&id)
    }

    pub fn list(&self) -> Vec<Arc<SessionEntry>> {
        lock(&self.entries).by_id.values().cloned().collect()
    }

    pub fn summaries(&self) -> Vec<SessionSummary> {
        self.list().iter().map(|entry| entry.summary()).collect()
    }

    pub fn len(&self) -> usize {
        lock(&self.entries).by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}

thread_local! {
    // Track each tap separately: a callback on tap A must not bypass tap B's
    // lock, including when callbacks on different taps are nested.
    static CALLBACKS: RefCell<Vec<usize>> = const { RefCell::new(Vec::new()) };
}

struct CallbackScope;

impl Drop for CallbackScope {
    fn drop(&mut self) {
        CALLBACKS.with(|callbacks| callbacks.borrow_mut().pop());
    }
}

struct CallbackControl<C: ?Sized> {
    attached: AtomicBool,
    closed: AtomicBool,
    detach_requested: AtomicBool,
    replacement: Mutex<Option<Arc<C>>>,
}

impl<C: ?Sized> Default for CallbackControl<C> {
    fn default() -> Self {
        Self {
            attached: AtomicBool::new(false),
            closed: AtomicBool::new(false),
            detach_requested: AtomicBool::new(false),
            replacement: Mutex::new(None),
        }
    }
}

impl<C: ?Sized> CallbackControl<C> {
    fn in_callback(&self) -> bool {
        CALLBACKS.with(|callbacks| callbacks.borrow().contains(&(self as *const Self as usize)))
    }

    fn call(&self, f: impl FnOnce()) {
        CALLBACKS.with(|callbacks| callbacks.borrow_mut().push(self as *const Self as usize));
        let _scope = CallbackScope;
        f();
    }

    fn defer_attach(&self, consumer: Arc<C>) {
        *lock(&self.replacement) = Some(consumer);
        self.detach_requested.store(false, Ordering::Release);
    }

    /// Apply changes requested from the callback before any further callback.
    /// The last reentrant attach/detach wins; replacement replay is deferred
    /// until the current callback returns, preserving callback serialization.
    fn settle(&self, consumer: &mut Option<Arc<C>>) -> bool {
        let replacement = lock(&self.replacement).take();
        if self.detach_requested.swap(false, Ordering::AcqRel) {
            *consumer = None;
            self.attached.store(false, Ordering::Release);
            true
        } else if let Some(replacement) = replacement {
            *consumer = Some(replacement);
            self.attached.store(true, Ordering::Release);
            true
        } else {
            false
        }
    }
}

pub trait VideoConsumer: Send + Sync {
    fn frame(&self, f: &ReceivedFrame);
    fn stream_config(&self, c: &pb::StreamConfig);
    fn cursor(&self, c: &CursorEvent);
    fn cursor_msg(&self, m: &pb::CursorMsg);
    fn clock(&self, rtt_us: u64, offset_us: i64);
    fn closed(&self, reason: &str);
}

#[derive(Default)]
pub struct VideoTap {
    control: CallbackControl<dyn VideoConsumer>,
    state: Mutex<VideoState>,
}

#[derive(Default)]
struct VideoState {
    consumer: Option<Arc<dyn VideoConsumer>>,
    config: Option<pb::StreamConfig>,
    cursor: Option<CursorEvent>,
    shape: Option<pb::CursorMsg>,
    cursor_state: Option<pb::CursorMsg>,
    attach_hook: Option<Box<dyn Fn() + Send + Sync>>,
}

impl VideoTap {
    /// Replay runs synchronously on the caller, which must be a library worker
    /// when adapting this tap to the native C ABI.
    pub fn attach(&self, c: Arc<dyn VideoConsumer>) -> bool {
        if self.control.closed.load(Ordering::Acquire) {
            return false;
        }
        if self.control.in_callback() {
            self.control.defer_attach(c);
            return true;
        }
        let mut state = lock(&self.state);
        if self.control.closed.load(Ordering::Acquire) {
            return false;
        }
        state.consumer = Some(c);
        self.control.attached.store(true, Ordering::Release);
        self.replay(&mut state);
        true
    }

    pub fn detach(&self) {
        if self.control.in_callback() {
            self.control.detach_requested.store(true, Ordering::Release);
            return;
        }
        let mut state = lock(&self.state);
        state.consumer = None;
        self.control.attached.store(false, Ordering::Release);
    }

    pub fn set_attach_hook(&self, f: Box<dyn Fn() + Send + Sync>) {
        lock(&self.state).attach_hook = Some(f);
    }

    fn replay(&self, state: &mut VideoState) {
        'replay: while let Some(consumer) = state.consumer.clone() {
            if let Some(config) = &state.config {
                self.control.call(|| consumer.stream_config(config));
                if self.control.settle(&mut state.consumer) {
                    continue 'replay;
                }
            }
            if let Some(cursor) = &state.cursor {
                self.control.call(|| consumer.cursor(cursor));
                if self.control.settle(&mut state.consumer) {
                    continue 'replay;
                }
            }
            for message in [&state.shape, &state.cursor_state].into_iter().flatten() {
                self.control.call(|| consumer.cursor_msg(message));
                if self.control.settle(&mut state.consumer) {
                    continue 'replay;
                }
            }
            if let Some(hook) = &state.attach_hook {
                self.control.call(hook);
                if self.control.settle(&mut state.consumer) {
                    continue 'replay;
                }
            }
            break;
        }
    }

    fn deliver(
        &self,
        cache: impl FnOnce(&mut VideoState),
        callback: impl FnOnce(&dyn VideoConsumer),
    ) -> bool {
        let mut state = lock(&self.state);
        if self.control.closed.load(Ordering::Acquire) {
            return false;
        }
        cache(&mut state);
        let Some(consumer) = state.consumer.clone() else {
            return false;
        };
        self.control.call(|| callback(consumer.as_ref()));
        if self.control.settle(&mut state.consumer) {
            self.replay(&mut state);
        }
        true
    }

    pub fn deliver_frame(&self, f: &ReceivedFrame) -> bool {
        self.deliver(|_| {}, |consumer| consumer.frame(f))
    }

    pub fn deliver_config(&self, c: &pb::StreamConfig) {
        self.deliver(|state| state.config = Some(*c), |out| out.stream_config(c));
    }

    pub fn deliver_cursor(&self, c: &CursorEvent) {
        self.deliver(|state| state.cursor = Some(c.clone()), |out| out.cursor(c));
    }

    pub fn deliver_cursor_msg(&self, m: &pb::CursorMsg) {
        self.deliver(
            |state| match &m.msg {
                Some(pb::cursor_msg::Msg::Shape(_)) => {
                    state.shape = Some(m.clone());
                    // Only Use/Hidden events after the newest shape are replayed.
                    state.cursor_state = None;
                }
                Some(pb::cursor_msg::Msg::Use(_) | pb::cursor_msg::Msg::Hidden(_)) => {
                    state.cursor_state = Some(m.clone());
                }
                None => {}
            },
            |out| out.cursor_msg(m),
        );
    }

    pub fn deliver_clock(&self, rtt_us: u64, offset_us: i64) {
        self.deliver(|_| {}, |out| out.clock(rtt_us, offset_us));
    }

    pub fn close(&self, reason: &str) {
        let mut state = lock(&self.state);
        if self.control.closed.swap(true, Ordering::AcqRel) {
            return;
        }
        self.control.attached.store(false, Ordering::Release);
        if let Some(consumer) = state.consumer.take() {
            self.control.call(|| consumer.closed(reason));
            self.control.settle(&mut state.consumer);
        }
        state.config = None;
        state.cursor = None;
        state.shape = None;
        state.cursor_state = None;
        state.attach_hook = None;
    }

    pub fn is_attached(&self) -> bool {
        self.control.attached.load(Ordering::Acquire)
    }

    pub fn is_closed(&self) -> bool {
        self.control.closed.load(Ordering::Acquire)
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub enum TermInput {
    Bytes(Vec<u8>),
    Resize { cols: u16, rows: u16 },
}

pub trait TermOutput: Send + Sync {
    fn output(&self, bytes: &[u8]);
    fn closed(&self);
}

/// Maximum detached terminal backlog in bytes. Overflow discards oldest bytes.
pub const TERM_BUFFER_CAPACITY: usize = 64 * 1024;

pub struct TermTap {
    control: CallbackControl<dyn TermOutput>,
    state: Mutex<TermState>,
    input: Mutex<TermInputState>,
}

#[derive(Default)]
struct TermState {
    consumer: Option<Arc<dyn TermOutput>>,
    buffer: VecDeque<Vec<u8>>,
    buffered_bytes: usize,
}

struct TermInputState {
    sender: Option<mpsc::UnboundedSender<TermInput>>,
    last_size: Option<(u16, u16)>,
}

impl TermTap {
    pub fn new() -> (TermTap, mpsc::UnboundedReceiver<TermInput>) {
        let (sender, receiver) = mpsc::unbounded_channel();
        (
            Self {
                control: CallbackControl::default(),
                state: Mutex::new(TermState::default()),
                input: Mutex::new(TermInputState {
                    sender: Some(sender),
                    last_size: None,
                }),
            },
            receiver,
        )
    }

    pub fn attach(&self, out: Arc<dyn TermOutput>) -> bool {
        if self.control.closed.load(Ordering::Acquire) {
            return false;
        }
        if self.control.in_callback() {
            self.control.defer_attach(out);
            return true;
        }
        let mut state = lock(&self.state);
        if self.control.closed.load(Ordering::Acquire) {
            return false;
        }
        state.consumer = Some(out);
        self.control.attached.store(true, Ordering::Release);
        self.flush(&mut state);
        true
    }

    pub fn detach(&self) {
        if self.control.in_callback() {
            self.control.detach_requested.store(true, Ordering::Release);
            return;
        }
        let mut state = lock(&self.state);
        state.consumer = None;
        self.control.attached.store(false, Ordering::Release);
    }

    fn flush(&self, state: &mut TermState) {
        while let Some(consumer) = state.consumer.clone() {
            let Some(bytes) = state.buffer.pop_front() else {
                break;
            };
            state.buffered_bytes -= bytes.len();
            self.control.call(|| consumer.output(&bytes));
            self.control.settle(&mut state.consumer);
        }
    }

    pub fn push_output(&self, bytes: &[u8]) {
        let mut state = lock(&self.state);
        if self.control.closed.load(Ordering::Acquire) || bytes.is_empty() {
            return;
        }
        if let Some(consumer) = state.consumer.clone() {
            self.control.call(|| consumer.output(bytes));
            if self.control.settle(&mut state.consumer) {
                self.flush(&mut state);
            }
            return;
        }
        let bytes = &bytes[bytes.len().saturating_sub(TERM_BUFFER_CAPACITY)..];
        let mut excess = (state.buffered_bytes + bytes.len()).saturating_sub(TERM_BUFFER_CAPACITY);
        while excess > 0 {
            let Some(front) = state.buffer.front_mut() else {
                break;
            };
            let removed = excess.min(front.len());
            if removed == front.len() {
                state.buffer.pop_front();
            } else {
                front.drain(..removed);
            }
            state.buffered_bytes -= removed;
            excess -= removed;
        }
        state.buffered_bytes += bytes.len();
        state.buffer.push_back(bytes.to_vec());
    }

    pub fn input(&self, bytes: &[u8]) -> bool {
        let input = lock(&self.input);
        !self.control.closed.load(Ordering::Acquire)
            && input
                .sender
                .as_ref()
                .is_some_and(|sender| sender.send(TermInput::Bytes(bytes.to_vec())).is_ok())
    }

    pub fn resize(&self, cols: u16, rows: u16) -> bool {
        if cols == 0 || rows == 0 {
            return false;
        }
        let mut input = lock(&self.input);
        let sent = !self.control.closed.load(Ordering::Acquire)
            && input
                .sender
                .as_ref()
                .is_some_and(|sender| sender.send(TermInput::Resize { cols, rows }).is_ok());
        if sent {
            input.last_size = Some((cols, rows));
        }
        sent
    }

    pub fn last_size(&self) -> Option<(u16, u16)> {
        lock(&self.input).last_size
    }

    pub fn close(&self) {
        let mut state = lock(&self.state);
        {
            let mut input = lock(&self.input);
            if self.control.closed.swap(true, Ordering::AcqRel) {
                return;
            }
            input.sender = None;
        }
        self.control.attached.store(false, Ordering::Release);
        state.buffer.clear();
        state.buffered_bytes = 0;
        if let Some(consumer) = state.consumer.take() {
            self.control.call(|| consumer.closed());
            self.control.settle(&mut state.consumer);
        }
    }
}

pub struct SshEntry {
    pub id: u64,
    pub server_id: String,
    pub term: TermTap,
    pub state: Mutex<SshState>,
    pub cancel: Notify,
    pub cancelled: AtomicBool,
}

#[derive(Default)]
pub struct SshTable {
    entries: Mutex<Entries<SshEntry>>,
}

impl SshTable {
    pub fn insert(&self, server_id: &str, term: TermTap) -> Arc<SshEntry> {
        lock(&self.entries).insert(|id| SshEntry {
            id,
            server_id: server_id.into(),
            term,
            state: Mutex::new(SshState::Connecting),
            cancel: Notify::new(),
            cancelled: AtomicBool::new(false),
        })
    }

    pub fn get(&self, id: u64) -> Option<Arc<SshEntry>> {
        lock(&self.entries).by_id.get(&id).cloned()
    }

    pub fn remove(&self, id: u64) -> Option<Arc<SshEntry>> {
        lock(&self.entries).by_id.remove(&id)
    }

    pub fn list(&self) -> Vec<Arc<SshEntry>> {
        lock(&self.entries).by_id.values().cloned().collect()
    }

    pub fn len(&self) -> usize {
        lock(&self.entries).by_id.len()
    }

    pub fn is_empty(&self) -> bool {
        self.len() == 0
    }
}
