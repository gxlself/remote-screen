//! X11 CLIPBOARD ownership and bounded ICCCM selection transfers.

use std::collections::{HashMap, VecDeque};
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::{mpsc, Arc};
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use host_core::platform::{ClipboardItem, PlatformError};
use rsp_core::pb::clipboard_header::Kind;
use x11rb::connection::{Connection, RequestConnection};
use x11rb::protocol::xfixes::{ConnectionExt as _, SelectionEventMask};
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    GetPropertyReply, PropMode, Property, SelectionNotifyEvent, SelectionRequestEvent, Window,
    WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

use crate::clip_model::{
    data_for, item_from, offered_targets, pick_target, property_chunk, ClipTarget, IncrReader,
    IncrState, IncrWriter, MAX_PNG_BYTES, MAX_TEXT_BYTES,
};

pub const STEP_TIMEOUT: Duration = Duration::from_secs(1);

const CALL_TIMEOUT: Duration = Duration::from_secs(3);
const SEND_TIMEOUT: Duration = Duration::from_secs(10);
const POLL_INTERVAL: Duration = Duration::from_millis(5);
const TARGETS_LIMIT: usize = 64 * 1024;
const PROPERTY_READ_WORDS: u32 = 16 * 1024;
const MAX_TRANSFERS: usize = 16;

type XResult<T> = Result<T, XError>;
type ReadReply = mpsc::Sender<Option<ClipboardItem>>;
type SetReply = mpsc::Sender<Result<(), PlatformError>>;

#[derive(Debug, thiserror::Error)]
enum XError {
    #[error(transparent)]
    Connect(#[from] x11rb::errors::ConnectError),
    #[error(transparent)]
    Connection(#[from] x11rb::errors::ConnectionError),
    #[error(transparent)]
    Reply(#[from] x11rb::errors::ReplyError),
    #[error(transparent)]
    Id(#[from] x11rb::errors::ReplyOrIdError),
    #[error(transparent)]
    Size(#[from] std::num::TryFromIntError),
    #[error("{0}")]
    Protocol(&'static str),
}

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        TIMESTAMP,
        INCR,
        UTF8_STRING,
        TEXT_UTF8: b"text/plain;charset=utf-8",
        PNG: b"image/png",
        TRANSFER: b"RS_CLIPBOARD_TRANSFER",
        CLOCK: b"RS_CLIPBOARD_CLOCK",
    }
}

impl Atoms {
    fn target(&self, target: ClipTarget) -> Atom {
        match target {
            ClipTarget::Utf8String => self.UTF8_STRING,
            ClipTarget::TextPlainUtf8 => self.TEXT_UTF8,
            ClipTarget::String => AtomEnum::STRING.into(),
            ClipTarget::Png => self.PNG,
        }
    }

    fn clip_target(&self, atom: Atom) -> Option<ClipTarget> {
        [
            ClipTarget::Utf8String,
            ClipTarget::TextPlainUtf8,
            ClipTarget::String,
            ClipTarget::Png,
        ]
        .into_iter()
        .find(|&target| self.target(target) == atom)
    }
}

enum Command {
    Get(ReadReply),
    Set(ClipboardItem, SetReply),
    Watch(tokio::sync::mpsc::Sender<()>, mpsc::Sender<()>),
    Shutdown,
}

pub struct X11Clipboard {
    commands: mpsc::Sender<Command>,
    stopping: Arc<AtomicBool>,
    thread: Option<JoinHandle<()>>,
}

impl X11Clipboard {
    pub fn connect(display: Option<&str>) -> Result<Self, PlatformError> {
        let display = display.map(str::to_owned);
        let (commands, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::channel();
        let stopping = Arc::new(AtomicBool::new(false));
        let worker_stop = Arc::clone(&stopping);
        let thread = thread::Builder::new()
            .name("rs-x11-clip".into())
            .spawn(move || match Worker::connect(display.as_deref()) {
                Ok(worker) => {
                    if ready_tx.send(Ok(())).is_ok() && worker.run(receiver, &worker_stop).is_err()
                    {
                        tracing::debug!("X11 clipboard worker stopped");
                    }
                }
                Err(error) => {
                    let _ = ready_tx.send(Err(platform_error(error)));
                }
            })
            .map_err(PlatformError::Io)?;
        let clipboard = Self {
            commands,
            stopping,
            thread: Some(thread),
        };
        ready_rx
            .recv_timeout(CALL_TIMEOUT)
            .map_err(|_| PlatformError::Platform("X11 clipboard connection timed out".into()))??;
        Ok(clipboard)
    }

    pub fn get(&self) -> Option<ClipboardItem> {
        let (tx, rx) = mpsc::channel();
        self.commands.send(Command::Get(tx)).ok()?;
        rx.recv_timeout(CALL_TIMEOUT).ok().flatten()
    }

    pub fn set(&self, item: ClipboardItem) -> Result<(), PlatformError> {
        validate(&item)?;
        let (tx, rx) = mpsc::channel();
        self.commands
            .send(Command::Set(item, tx))
            .map_err(|_| PlatformError::Platform("X11 clipboard worker is stopped".into()))?;
        rx.recv_timeout(CALL_TIMEOUT)
            .map_err(|_| PlatformError::Platform("X11 clipboard write timed out".into()))?
    }

    pub fn watch(&self, tx: tokio::sync::mpsc::Sender<()>) {
        let (ready_tx, ready_rx) = mpsc::channel();
        if self.commands.send(Command::Watch(tx, ready_tx)).is_ok() {
            let _ = ready_rx.recv_timeout(CALL_TIMEOUT);
        }
    }
}

impl Drop for X11Clipboard {
    fn drop(&mut self) {
        self.stopping.store(true, Ordering::Release);
        let _ = self.commands.send(Command::Shutdown);
        if let Some(thread) = self.thread.take() {
            let deadline = Instant::now() + Duration::from_millis(900);
            while !thread.is_finished() && Instant::now() < deadline {
                thread::sleep(POLL_INTERVAL);
            }
            if thread.is_finished() {
                let _ = thread.join();
            }
        }
    }
}

fn platform_error(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Platform(format!("X11 clipboard: {error}"))
}

fn validate(item: &ClipboardItem) -> Result<(), PlatformError> {
    let valid = match item.kind {
        Kind::Text => item.data.len() <= MAX_TEXT_BYTES && std::str::from_utf8(&item.data).is_ok(),
        Kind::Png => item.data.len() <= MAX_PNG_BYTES,
        Kind::Unspecified => false,
    };
    if valid {
        Ok(())
    } else {
        Err(PlatformError::Platform("invalid clipboard item".into()))
    }
}

#[derive(Clone, Copy)]
enum ReadPhase {
    Targets,
    Data(ClipTarget),
}

impl ReadPhase {
    fn target(self, atoms: &Atoms) -> Atom {
        match self {
            Self::Targets => atoms.TARGETS,
            Self::Data(target) => atoms.target(target),
        }
    }

    fn limit(self) -> usize {
        match self {
            Self::Targets => TARGETS_LIMIT,
            Self::Data(ClipTarget::Png) => MAX_PNG_BYTES,
            Self::Data(_) => MAX_TEXT_BYTES,
        }
    }
}

struct Incoming {
    window: Window,
    owner: Window,
    phase: ReadPhase,
    reader: Option<IncrReader>,
    deadline: Instant,
    call_deadline: Instant,
    reply: ReadReply,
}

enum ReadProgress {
    Pending,
    Done(Option<ClipboardItem>),
}

struct Outgoing {
    target: Atom,
    writer: IncrWriter,
    deadline: Instant,
}

struct PendingSet {
    item: ClipboardItem,
    reply: SetReply,
    sequence: u16,
    deadline: Instant,
}

struct Worker {
    connection: RustConnection,
    root: Window,
    window: Window,
    atoms: Atoms,
    chunk: usize,
    item: Option<ClipboardItem>,
    timestamp: u32,
    incoming: HashMap<Window, Incoming>,
    outgoing: HashMap<(Window, Atom), Outgoing>,
    pending_set: Option<PendingSet>,
    set_queue: VecDeque<(ClipboardItem, SetReply)>,
    watchers: Vec<tokio::sync::mpsc::Sender<()>>,
}

impl Worker {
    fn connect(display: Option<&str>) -> XResult<Self> {
        let (connection, screen) = x11rb::connect(display)?;
        let version = connection.xfixes_query_version(5, 0)?.reply()?;
        if version.major_version < 1 {
            return Err(XError::Protocol(
                "XFIXES selection notifications are unavailable",
            ));
        }
        let root = connection.setup().roots[screen].root;
        let window = input_window(&connection, root)?;
        let atoms = Atoms::new(&connection)?.reply()?;
        connection
            .xfixes_select_selection_input(
                window,
                atoms.CLIPBOARD,
                SelectionEventMask::SET_SELECTION_OWNER
                    | SelectionEventMask::SELECTION_WINDOW_DESTROY
                    | SelectionEventMask::SELECTION_CLIENT_CLOSE,
            )?
            .check()?;
        let words = u32::try_from(connection.maximum_request_bytes() / 4).unwrap_or(u32::MAX);
        let chunk = property_chunk(words);
        if chunk == 0 {
            return Err(XError::Protocol("X11 property requests are too small"));
        }
        connection.flush()?;
        Ok(Self {
            connection,
            root,
            window,
            atoms,
            chunk,
            item: None,
            timestamp: CURRENT_TIME,
            incoming: HashMap::new(),
            outgoing: HashMap::new(),
            pending_set: None,
            set_queue: VecDeque::new(),
            watchers: Vec::new(),
        })
    }

    fn run(mut self, commands: mpsc::Receiver<Command>, stopping: &AtomicBool) -> XResult<()> {
        while !stopping.load(Ordering::Acquire) {
            for _ in 0..64 {
                if stopping.load(Ordering::Acquire) {
                    break;
                }
                let Some(event) = self.connection.poll_for_event()? else {
                    break;
                };
                if self.event(event).is_err() {
                    tracing::debug!("X11 clipboard transfer failed");
                }
            }
            match commands.recv_timeout(POLL_INTERVAL) {
                Ok(Command::Shutdown) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
                Ok(command) => self.command(command),
                Err(mpsc::RecvTimeoutError::Timeout) => {}
            }
            self.expire();
            self.begin_set();
            self.connection.flush()?;
        }
        // Destroying the owner window relinquishes only our own selection.
        for (_, read) in self.incoming.drain() {
            let _ = read.reply.send(None);
        }
        self.connection.destroy_window(self.window)?.ignore_error();
        self.connection.flush()?;
        Ok(())
    }

    fn command(&mut self, command: Command) {
        match command {
            Command::Get(reply) => {
                if self.begin_get(reply.clone()).is_err() {
                    let _ = reply.send(None);
                }
            }
            Command::Set(item, reply) => {
                if self.set_queue.len() < MAX_TRANSFERS {
                    self.set_queue.push_back((item, reply));
                } else {
                    let _ = reply.send(Err(platform_error("too many clipboard writes")));
                }
            }
            Command::Watch(tx, ready) => {
                self.watchers.retain(|watcher| !watcher.is_closed());
                self.watchers.push(tx);
                let _ = ready.send(());
            }
            Command::Shutdown => {}
        }
    }

    fn begin_set(&mut self) {
        if self.pending_set.is_some() {
            return;
        }
        let Some((item, reply)) = self.set_queue.pop_front() else {
            return;
        };
        let result = (|| -> XResult<u16> {
            let cookie = self.connection.change_property8(
                PropMode::APPEND,
                self.window,
                self.atoms.CLOCK,
                AtomEnum::INTEGER,
                &[],
            )?;
            let sequence = cookie.sequence_number() as u16;
            cookie.check()?;
            Ok(sequence)
        })();
        match result {
            Ok(sequence) => {
                self.pending_set = Some(PendingSet {
                    item,
                    reply,
                    sequence,
                    deadline: Instant::now() + STEP_TIMEOUT,
                });
            }
            Err(error) => {
                let _ = reply.send(Err(platform_error(error)));
            }
        }
    }

    fn complete_set(&mut self, sequence: u16, timestamp: u32) {
        let Some(pending) = self.pending_set.take() else {
            return;
        };
        if sequence != pending.sequence || timestamp == CURRENT_TIME {
            self.pending_set = Some(pending);
            return;
        }
        if Instant::now() >= pending.deadline {
            let _ = pending
                .reply
                .send(Err(platform_error("clipboard timestamp timed out")));
            return;
        }
        let result = (|| -> XResult<()> {
            self.connection
                .set_selection_owner(self.window, self.atoms.CLIPBOARD, timestamp)?
                .check()?;
            if self.owner()? != self.window {
                return Err(XError::Protocol("clipboard ownership was not acquired"));
            }
            Ok(())
        })();
        if result.is_ok() {
            self.timestamp = timestamp;
            self.item = Some(pending.item);
            let readers: Vec<_> = self.incoming.keys().copied().collect();
            for window in readers {
                self.cancel_read(window);
            }
        }
        let _ = pending.reply.send(result.map_err(platform_error));
    }

    fn owner(&self) -> XResult<Window> {
        Ok(self
            .connection
            .get_selection_owner(self.atoms.CLIPBOARD)?
            .reply()?
            .owner)
    }

    fn begin_get(&mut self, reply: ReadReply) -> XResult<()> {
        let owner = self.owner()?;
        if owner == NONE || owner == self.window {
            let _ = reply.send(if owner == self.window {
                self.item.clone()
            } else {
                None
            });
            return Ok(());
        }
        if self.incoming.len() >= MAX_TRANSFERS {
            let _ = reply.send(None);
            return Ok(());
        }
        // A fresh requestor prevents late replies from a timed-out conversion
        // from overwriting the property of a later get().
        let window = input_window(&self.connection, self.root)?;
        let mut read = Incoming {
            window,
            owner,
            phase: ReadPhase::Targets,
            reader: None,
            deadline: Instant::now() + STEP_TIMEOUT,
            call_deadline: Instant::now() + CALL_TIMEOUT,
            reply,
        };
        if self.convert(&mut read).is_err() {
            self.finish_read(read, None);
        } else {
            self.incoming.insert(window, read);
        }
        Ok(())
    }

    fn convert(&self, read: &mut Incoming) -> XResult<()> {
        self.connection
            .convert_selection(
                read.window,
                self.atoms.CLIPBOARD,
                read.phase.target(&self.atoms),
                self.atoms.TRANSFER,
                CURRENT_TIME,
            )?
            .check()?;
        read.deadline = Instant::now() + STEP_TIMEOUT;
        Ok(())
    }

    fn event(&mut self, event: Event) -> XResult<()> {
        match event {
            Event::XfixesSelectionNotify(event)
                if event.selection == self.atoms.CLIPBOARD && event.window == self.window =>
            {
                if event.owner == self.window || self.owner()? != event.owner {
                    return Ok(());
                }
                self.item = None;
                let readers: Vec<_> = self
                    .incoming
                    .iter()
                    .filter(|(_, read)| read.owner != event.owner)
                    .map(|(&window, _)| window)
                    .collect();
                for window in readers {
                    self.cancel_read(window);
                }
                self.watchers.retain(|tx| match tx.try_send(()) {
                    Ok(()) | Err(tokio::sync::mpsc::error::TrySendError::Full(_)) => true,
                    Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) => false,
                });
            }
            Event::SelectionClear(event) if event.selection == self.atoms.CLIPBOARD => {
                if self.owner()? != self.window {
                    self.item = None;
                }
            }
            Event::SelectionRequest(request) => self.selection_request(request)?,
            Event::SelectionNotify(event) if event.selection == self.atoms.CLIPBOARD => {
                if self
                    .incoming
                    .get(&event.requestor)
                    .is_some_and(|read| read.phase.target(&self.atoms) == event.target)
                {
                    if event.property == self.atoms.TRANSFER {
                        self.receive_read(event.requestor);
                    } else {
                        self.cancel_read(event.requestor);
                    }
                }
            }
            Event::PropertyNotify(event) => {
                if event.state == Property::NEW_VALUE {
                    if event.window == self.window && event.atom == self.atoms.CLOCK {
                        self.complete_set(event.sequence, event.time);
                    } else if event.atom == self.atoms.TRANSFER
                        && self
                            .incoming
                            .get(&event.window)
                            .is_some_and(|read| read.reader.is_some())
                    {
                        self.receive_read(event.window);
                    }
                } else if event.state == Property::DELETE {
                    self.send_next(event.window, event.atom)?;
                }
            }
            Event::DestroyNotify(event) => {
                self.outgoing
                    .retain(|&(window, _), _| window != event.window);
                self.cancel_read(event.window);
            }
            _ => {}
        }
        Ok(())
    }

    fn receive_read(&mut self, window: Window) {
        let Some(mut read) = self.incoming.remove(&window) else {
            return;
        };
        let now = Instant::now();
        if now >= read.deadline || now >= read.call_deadline {
            self.finish_read(read, None);
            return;
        }
        match self.advance_read(&mut read) {
            Ok(ReadProgress::Pending) => {
                self.incoming.insert(window, read);
            }
            Ok(ReadProgress::Done(item)) => self.finish_read(read, item),
            Err(_) => self.finish_read(read, None),
        }
    }

    fn advance_read(&self, read: &mut Incoming) -> XResult<ReadProgress> {
        let Some(property) = self.read_property(read.window, read.phase.limit())? else {
            return Ok(ReadProgress::Done(None));
        };
        if property.type_ == self.atoms.INCR {
            let size = property.value32().and_then(|mut values| values.next());
            if read.reader.is_some()
                || property.value.len() != 4
                || size.is_none_or(|size| u64::from(size) > read.phase.limit() as u64)
            {
                return Ok(ReadProgress::Done(None));
            }
            read.reader = Some(IncrReader::new(read.phase.limit()));
            read.deadline = Instant::now() + STEP_TIMEOUT;
            return Ok(ReadProgress::Pending);
        }
        let (expected_type, expected_format) = match read.phase {
            ReadPhase::Targets => (AtomEnum::ATOM.into(), 32),
            ReadPhase::Data(target) => (self.atoms.target(target), 8),
        };
        if property.type_ != expected_type || property.format != expected_format {
            return Ok(ReadProgress::Done(None));
        }
        let data = if let Some(reader) = &mut read.reader {
            match reader.push(&property.value) {
                IncrState::More => {
                    read.deadline = Instant::now() + STEP_TIMEOUT;
                    return Ok(ReadProgress::Pending);
                }
                IncrState::Done(data) => data,
                IncrState::TooLarge => return Ok(ReadProgress::Done(None)),
            }
        } else {
            property.value
        };
        match read.phase {
            ReadPhase::Targets => {
                if !data.len().is_multiple_of(4) {
                    return Ok(ReadProgress::Done(None));
                }
                let available: Vec<_> = data
                    .chunks_exact(4)
                    .map(|bytes| u32::from_ne_bytes([bytes[0], bytes[1], bytes[2], bytes[3]]))
                    .filter_map(|atom| self.atoms.clip_target(atom))
                    .map(ClipTarget::atom_name)
                    .collect();
                let Some(target) = pick_target(&available) else {
                    return Ok(ReadProgress::Done(None));
                };
                read.phase = ReadPhase::Data(target);
                read.reader = None;
                self.convert(read)?;
                Ok(ReadProgress::Pending)
            }
            ReadPhase::Data(target) => Ok(ReadProgress::Done(if self.owner()? == read.owner {
                item_from(target, data)
            } else {
                None
            })),
        }
    }

    fn read_property(&self, window: Window, limit: usize) -> XResult<Option<GetPropertyReply>> {
        let mut property = self
            .connection
            .get_property(
                true,
                window,
                self.atoms.TRANSFER,
                AtomEnum::ANY,
                0,
                PROPERTY_READ_WORDS,
            )?
            .reply()?;
        if property.type_ == NONE {
            return Ok(None);
        }
        loop {
            if property.value.len() as u64 + u64::from(property.bytes_after) > limit as u64 {
                return Ok(None);
            }
            if property.bytes_after == 0 {
                return Ok(Some(property));
            }
            if property.value.is_empty() || !property.value.len().is_multiple_of(4) {
                return Ok(None);
            }
            let next = self
                .connection
                .get_property(
                    true,
                    window,
                    self.atoms.TRANSFER,
                    AtomEnum::ANY,
                    u32::try_from(property.value.len() / 4)?,
                    PROPERTY_READ_WORDS,
                )?
                .reply()?;
            if next.type_ != property.type_
                || next.format != property.format
                || next.value.is_empty()
            {
                return Ok(None);
            }
            property.value.extend_from_slice(&next.value);
            property.bytes_after = next.bytes_after;
        }
    }

    fn finish_read(&self, read: Incoming, item: Option<ClipboardItem>) {
        if let Ok(cookie) = self.connection.destroy_window(read.window) {
            cookie.ignore_error();
        }
        let _ = read.reply.send(item);
    }

    fn cancel_read(&mut self, window: Window) {
        if let Some(read) = self.incoming.remove(&window) {
            self.finish_read(read, None);
        }
    }

    fn selection_request(&mut self, request: SelectionRequestEvent) -> XResult<()> {
        let property = if request.property == NONE {
            request.target
        } else {
            request.property
        };
        let result = self.offer(request, property);
        let offered = match result {
            Ok(property) => property,
            Err(_) => {
                self.outgoing.remove(&(request.requestor, property));
                self.unselect_requestor(request.requestor);
                NONE
            }
        };
        self.notify(request, offered)
    }

    fn offer(&mut self, request: SelectionRequestEvent, property: Atom) -> XResult<Atom> {
        let Some(item) = &self.item else {
            return Ok(NONE);
        };
        if request.selection != self.atoms.CLIPBOARD
            || request.owner != self.window
            || (request.time != CURRENT_TIME
                && request.time.wrapping_sub(self.timestamp) > i32::MAX as u32)
            || self.outgoing.contains_key(&(request.requestor, property))
        {
            return Ok(NONE);
        }
        if request.target == self.atoms.TARGETS {
            let mut targets = vec![self.atoms.TARGETS, self.atoms.TIMESTAMP];
            targets.extend(
                offered_targets(item)
                    .into_iter()
                    .map(|t| self.atoms.target(t)),
            );
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    AtomEnum::ATOM,
                    &targets,
                )?
                .check()?;
        } else if request.target == self.atoms.TIMESTAMP {
            self.connection
                .change_property32(
                    PropMode::REPLACE,
                    request.requestor,
                    property,
                    AtomEnum::INTEGER,
                    &[self.timestamp],
                )?
                .check()?;
        } else {
            let Some(data) = self
                .atoms
                .clip_target(request.target)
                .and_then(|target| data_for(item, target))
            else {
                return Ok(NONE);
            };
            if data.len() <= self.chunk {
                self.connection
                    .change_property8(
                        PropMode::REPLACE,
                        request.requestor,
                        property,
                        request.target,
                        &data,
                    )?
                    .check()?;
            } else {
                if self.outgoing.len() >= MAX_TRANSFERS {
                    return Ok(NONE);
                }
                self.connection
                    .change_window_attributes(
                        request.requestor,
                        &ChangeWindowAttributesAux::new()
                            .event_mask(EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY),
                    )?
                    .check()?;
                let writer = IncrWriter::new(data, self.chunk);
                self.connection
                    .change_property32(
                        PropMode::REPLACE,
                        request.requestor,
                        property,
                        self.atoms.INCR,
                        &[u32::try_from(writer.total_len())?],
                    )?
                    .check()?;
                self.outgoing.insert(
                    (request.requestor, property),
                    Outgoing {
                        target: request.target,
                        writer,
                        deadline: Instant::now() + SEND_TIMEOUT,
                    },
                );
            }
        }
        Ok(property)
    }

    fn notify(&self, request: SelectionRequestEvent, property: Atom) -> XResult<()> {
        self.connection
            .send_event(
                false,
                request.requestor,
                EventMask::NO_EVENT,
                SelectionNotifyEvent {
                    response_type: SELECTION_NOTIFY_EVENT,
                    sequence: 0,
                    time: request.time,
                    requestor: request.requestor,
                    selection: request.selection,
                    target: request.target,
                    property,
                },
            )?
            .check()?;
        Ok(())
    }

    fn send_next(&mut self, window: Window, property: Atom) -> XResult<()> {
        let Some(mut outgoing) = self.outgoing.remove(&(window, property)) else {
            return Ok(());
        };
        if Instant::now() >= outgoing.deadline {
            self.unselect_requestor(window);
            return Ok(());
        }
        let result = (|| -> XResult<bool> {
            let Some(chunk) = outgoing.writer.next_chunk() else {
                return Ok(false);
            };
            self.connection
                .change_property8(PropMode::REPLACE, window, property, outgoing.target, chunk)?
                .check()?;
            Ok(!chunk.is_empty())
        })();
        if matches!(result, Ok(true)) {
            outgoing.deadline = Instant::now() + SEND_TIMEOUT;
            self.outgoing.insert((window, property), outgoing);
        } else {
            self.unselect_requestor(window);
        }
        result.map(|_| ())
    }

    fn unselect_requestor(&self, window: Window) {
        if window == self.window
            || self.incoming.contains_key(&window)
            || self
                .outgoing
                .keys()
                .any(|&(requestor, _)| requestor == window)
        {
            return;
        }
        if let Ok(cookie) = self.connection.change_window_attributes(
            window,
            &ChangeWindowAttributesAux::new().event_mask(EventMask::NO_EVENT),
        ) {
            cookie.ignore_error();
        }
    }

    fn expire(&mut self) {
        let now = Instant::now();
        let readers: Vec<_> = self
            .incoming
            .iter()
            .filter(|(_, read)| now >= read.deadline || now >= read.call_deadline)
            .map(|(&window, _)| window)
            .collect();
        for window in readers {
            self.cancel_read(window);
        }
        let writers: Vec<_> = self
            .outgoing
            .iter()
            .filter(|(_, write)| now >= write.deadline)
            .map(|(&key, _)| key)
            .collect();
        for key in writers {
            self.outgoing.remove(&key);
            self.unselect_requestor(key.0);
        }
        if self
            .pending_set
            .as_ref()
            .is_some_and(|pending| now >= pending.deadline)
        {
            if let Some(pending) = self.pending_set.take() {
                let _ = pending
                    .reply
                    .send(Err(platform_error("clipboard timestamp timed out")));
            }
        }
    }
}

fn input_window(connection: &RustConnection, root: Window) -> XResult<Window> {
    let window = connection.generate_id()?;
    connection
        .create_window(
            COPY_DEPTH_FROM_PARENT,
            window,
            root,
            0,
            0,
            1,
            1,
            0,
            WindowClass::INPUT_ONLY,
            0,
            &CreateWindowAux::new().event_mask(EventMask::PROPERTY_CHANGE),
        )?
        .check()?;
    Ok(window)
}
