#![cfg(feature = "desktop-it")]
#![cfg(target_os = "linux")]

use std::collections::HashMap;
use std::sync::mpsc;
use std::thread::{self, JoinHandle};
use std::time::{Duration, Instant};

use host_core::clip_convert::make_item;
use host_core::platform::ClipboardItem;
use host_linux::clip_model::{ClipTarget, MAX_PNG_BYTES, MAX_TEXT_BYTES};
use host_linux::x11_clip::{X11Clipboard, STEP_TIMEOUT};
use rsp_core::pb::clipboard_header::Kind;
use tokio::time::{sleep, timeout};
use x11rb::connection::Connection;
use x11rb::protocol::xproto::{
    Atom, AtomEnum, ChangeWindowAttributesAux, ConnectionExt as _, CreateWindowAux, EventMask,
    GetPropertyReply, PropMode, Property, SelectionNotifyEvent, SelectionRequestEvent, Window,
    WindowClass, SELECTION_NOTIFY_EVENT,
};
use x11rb::protocol::Event;
use x11rb::rust_connection::RustConnection;
use x11rb::wrapper::ConnectionExt as _;
use x11rb::{COPY_DEPTH_FROM_PARENT, CURRENT_TIME, NONE};

const TEST_TIMEOUT: Duration = Duration::from_secs(10);

x11rb::atom_manager! {
    Atoms: AtomsCookie {
        CLIPBOARD,
        TARGETS,
        TIMESTAMP,
        INCR,
        UTF8_STRING,
        TEXT_UTF8: b"text/plain;charset=utf-8",
        PNG: b"image/png",
        MULTIPLE,
        PROPERTY: b"RS_IT_CLIPBOARD_DATA",
        CLOCK: b"RS_IT_CLIPBOARD_CLOCK",
    }
}

fn assert_layer() -> std::fs::File {
    assert_eq!(
        std::env::var("RS_IT_LAYER").as_deref(),
        Ok("x11"),
        "run under crates/host-linux/tests/desktop/run.sh x11"
    );
    // nextest uses separate processes; all tests share this server's CLIPBOARD.
    let display: String = std::env::var("DISPLAY")
        .unwrap()
        .chars()
        .map(|ch| if ch.is_ascii_alphanumeric() { ch } else { '_' })
        .collect();
    let directory =
        std::env::var_os("TMPDIR").expect("TMPDIR must point to the test scratch space");
    let path = std::path::PathBuf::from(directory).join(format!("rs-v8v-x11clip-{display}.lock"));
    let file = std::fs::OpenOptions::new()
        .create(true)
        .truncate(false)
        .read(true)
        .write(true)
        .open(path)
        .unwrap();
    file.lock().unwrap();
    file
}

fn text(data: &[u8]) -> ClipboardItem {
    make_item(Kind::Text, data.to_vec())
}

struct Received {
    type_: Atom,
    format: u8,
    data: Vec<u8>,
    incremental: bool,
}

struct Client {
    connection: RustConnection,
    window: Window,
    atoms: Atoms,
}

impl Client {
    fn new() -> Self {
        let (connection, screen) = x11rb::connect(None).unwrap();
        let root = connection.setup().roots[screen].root;
        let window = connection.generate_id().unwrap();
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
            )
            .unwrap()
            .check()
            .unwrap();
        let atoms = Atoms::new(&connection).unwrap().reply().unwrap();
        Self {
            connection,
            window,
            atoms,
        }
    }

    async fn event(&self) -> Event {
        loop {
            if let Some(event) = self.connection.poll_for_event().unwrap() {
                return event;
            }
            sleep(Duration::from_millis(2)).await;
        }
    }

    async fn own(&self) {
        self.connection
            .change_property8(
                PropMode::APPEND,
                self.window,
                self.atoms.CLOCK,
                AtomEnum::INTEGER,
                &[],
            )
            .unwrap()
            .check()
            .unwrap();
        let timestamp = loop {
            if let Event::PropertyNotify(event) = self.event().await {
                if event.window == self.window && event.atom == self.atoms.CLOCK {
                    break event.time;
                }
            }
        };
        self.connection
            .set_selection_owner(self.window, self.atoms.CLIPBOARD, timestamp)
            .unwrap()
            .check()
            .unwrap();
        assert_eq!(
            self.connection
                .get_selection_owner(self.atoms.CLIPBOARD)
                .unwrap()
                .reply()
                .unwrap()
                .owner,
            self.window
        );
    }

    fn property(&self, property: Atom, delete: bool) -> GetPropertyReply {
        self.connection
            .get_property(delete, self.window, property, AtomEnum::ANY, 0, u32::MAX)
            .unwrap()
            .reply()
            .unwrap()
    }

    async fn initial(
        &self,
        target: Atom,
        property: Atom,
        delete: bool,
    ) -> Option<GetPropertyReply> {
        self.connection
            .convert_selection(
                self.window,
                self.atoms.CLIPBOARD,
                target,
                property,
                CURRENT_TIME,
            )
            .unwrap()
            .check()
            .unwrap();
        loop {
            if let Event::SelectionNotify(event) = self.event().await {
                if event.requestor == self.window && event.target == target {
                    return (event.property != NONE).then(|| self.property(event.property, delete));
                }
            }
        }
    }

    async fn fetch(&self, target: Atom) -> Option<Received> {
        let initial = self.initial(target, self.atoms.PROPERTY, true).await?;
        if initial.type_ != self.atoms.INCR {
            return Some(Received {
                type_: initial.type_,
                format: initial.format,
                data: initial.value,
                incremental: false,
            });
        }
        assert_eq!(initial.format, 32);
        let announced = initial.value32().unwrap().next().unwrap() as usize;
        assert!(announced <= MAX_PNG_BYTES);
        let mut data = Vec::new();
        loop {
            if let Event::PropertyNotify(event) = self.event().await {
                if event.window != self.window
                    || event.atom != self.atoms.PROPERTY
                    || event.state != Property::NEW_VALUE
                {
                    continue;
                }
                let chunk = self.property(self.atoms.PROPERTY, true);
                assert_eq!(chunk.type_, target);
                assert_eq!(chunk.format, 8);
                assert_eq!(chunk.bytes_after, 0);
                assert!(data.len() + chunk.value.len() <= MAX_PNG_BYTES);
                if chunk.value.is_empty() {
                    assert_eq!(data.len(), announced);
                    return Some(Received {
                        type_: target,
                        format: 8,
                        data,
                        incremental: true,
                    });
                }
                data.extend_from_slice(&chunk.value);
            }
        }
    }
}

#[derive(Clone, Copy)]
enum Delivery {
    Direct,
    Incr,
    Refuse,
    Silent,
    OversizedHeader,
    Underreported,
    Stall,
}

struct Owner {
    stop: mpsc::Sender<()>,
    thread: Option<JoinHandle<()>>,
}

impl Owner {
    async fn start(target: ClipTarget, data: Vec<u8>, delivery: Delivery) -> Self {
        let client = Client::new();
        client.own().await;
        let (stop, receiver) = mpsc::channel();
        let thread = thread::Builder::new()
            .name("rs-it-clip-owner".into())
            .spawn(move || serve(client, target, data, delivery, receiver))
            .unwrap();
        Self {
            stop,
            thread: Some(thread),
        }
    }

    fn stop(&mut self) {
        let _ = self.stop.send(());
        if let Some(thread) = self.thread.take() {
            let result = thread.join();
            if !thread::panicking() {
                result.unwrap();
            }
        }
    }
}

impl Drop for Owner {
    fn drop(&mut self) {
        self.stop();
    }
}

fn notify(client: &Client, request: SelectionRequestEvent, property: Atom) {
    if let Ok(cookie) = client.connection.send_event(
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
    ) {
        // The requestor may have timed out and destroyed its window.
        cookie.ignore_error();
    }
}

fn serve(
    client: Client,
    target: ClipTarget,
    data: Vec<u8>,
    delivery: Delivery,
    stop: mpsc::Receiver<()>,
) {
    let target_atom = client
        .connection
        .intern_atom(false, target.atom_name().as_bytes())
        .unwrap()
        .reply()
        .unwrap()
        .atom;
    // Deliberately implement the peer independently of IncrReader/IncrWriter.
    let mut transfers: HashMap<(Window, Atom), usize> = HashMap::new();
    loop {
        for _ in 0..64 {
            let Some(event) = client.connection.poll_for_event().unwrap() else {
                break;
            };
            match event {
                Event::SelectionRequest(request) => {
                    if matches!(delivery, Delivery::Silent) {
                        continue;
                    }
                    let property = if request.property == NONE {
                        request.target
                    } else {
                        request.property
                    };
                    let accepted = (|| -> Result<bool, Box<dyn std::error::Error>> {
                        if matches!(delivery, Delivery::Refuse) {
                            return Ok(false);
                        }
                        if request.target == client.atoms.TARGETS {
                            client
                                .connection
                                .change_property32(
                                    PropMode::REPLACE,
                                    request.requestor,
                                    property,
                                    AtomEnum::ATOM,
                                    &[client.atoms.TARGETS, target_atom],
                                )?
                                .check()?;
                        } else if request.target != target_atom {
                            return Ok(false);
                        } else if matches!(delivery, Delivery::Direct) {
                            client
                                .connection
                                .change_property8(
                                    PropMode::REPLACE,
                                    request.requestor,
                                    property,
                                    target_atom,
                                    &data,
                                )?
                                .check()?;
                        } else {
                            client
                                .connection
                                .change_window_attributes(
                                    request.requestor,
                                    &ChangeWindowAttributesAux::new().event_mask(
                                        EventMask::PROPERTY_CHANGE | EventMask::STRUCTURE_NOTIFY,
                                    ),
                                )?
                                .check()?;
                            let size = match delivery {
                                Delivery::OversizedHeader => u32::MAX,
                                Delivery::Underreported => 0,
                                _ => u32::try_from(data.len())?,
                            };
                            client
                                .connection
                                .change_property32(
                                    PropMode::REPLACE,
                                    request.requestor,
                                    property,
                                    client.atoms.INCR,
                                    &[size],
                                )?
                                .check()?;
                            if matches!(delivery, Delivery::Incr | Delivery::Underreported) {
                                transfers.insert((request.requestor, property), 0);
                            }
                        }
                        Ok(true)
                    })()
                    .unwrap_or(false);
                    notify(&client, request, if accepted { property } else { NONE });
                }
                Event::PropertyNotify(event) if event.state == Property::DELETE => {
                    let key = (event.window, event.atom);
                    if let Some(offset) = transfers.remove(&key) {
                        let end = (offset + 64 * 1024).min(data.len());
                        let chunk = &data[offset..end];
                        let sent = client
                            .connection
                            .change_property8(
                                PropMode::REPLACE,
                                event.window,
                                event.atom,
                                target_atom,
                                chunk,
                            )
                            .is_ok_and(|cookie| cookie.check().is_ok());
                        if sent && !chunk.is_empty() {
                            transfers.insert(key, end);
                        }
                    }
                }
                Event::DestroyNotify(event) => {
                    transfers.retain(|&(window, _), _| window != event.window);
                }
                _ => {}
            }
        }
        client.connection.flush().unwrap();
        match stop.recv_timeout(Duration::from_millis(2)) {
            Ok(()) | Err(mpsc::RecvTimeoutError::Disconnected) => break,
            Err(mpsc::RecvTimeoutError::Timeout) => {}
        }
    }
}

fn png_4x4() -> Vec<u8> {
    let mut data = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut data, 4, 4);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer.write_image_data(&[0x7f; 4 * 4 * 4]).unwrap();
    }
    data
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn set_then_other_app_reads() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        let data = "hello \u{4e16}\u{754c}".as_bytes();
        clipboard.set(text(data)).unwrap();
        let client = Client::new();
        let targets = client.fetch(client.atoms.TARGETS).await.unwrap();
        assert_eq!(targets.type_, u32::from(AtomEnum::ATOM));
        assert_eq!(targets.format, 32);
        let atoms: Vec<_> = targets
            .data
            .chunks_exact(4)
            .map(|b| u32::from_ne_bytes([b[0], b[1], b[2], b[3]]))
            .collect();
        for atom in [
            client.atoms.UTF8_STRING,
            client.atoms.TEXT_UTF8,
            AtomEnum::STRING.into(),
            client.atoms.TIMESTAMP,
        ] {
            assert!(atoms.contains(&atom));
        }
        assert!(!atoms.contains(&client.atoms.MULTIPLE));
        let received = client.fetch(client.atoms.UTF8_STRING).await.unwrap();
        assert_eq!(received.data, data);
        assert!(!received.incremental);
        assert_eq!(
            client.fetch(client.atoms.TEXT_UTF8).await.unwrap().data,
            data
        );
        let timestamp = client.fetch(client.atoms.TIMESTAMP).await.unwrap();
        assert_eq!(timestamp.type_, u32::from(AtomEnum::INTEGER));
        assert_eq!(timestamp.format, 32);
        assert_eq!(timestamp.data.len(), 4);
        assert!(timestamp.data.iter().any(|&byte| byte != 0));
        assert!(client.fetch(client.atoms.MULTIPLE).await.is_none());
        assert_eq!(
            client
                .initial(client.atoms.UTF8_STRING, NONE, true)
                .await
                .unwrap()
                .value,
            data
        );
        assert_eq!(clipboard.get().unwrap().data, data);
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn other_app_owns_then_get() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        clipboard.watch(tx);
        let _owner =
            Owner::start(ClipTarget::Utf8String, b"copied".to_vec(), Delivery::Direct).await;
        assert_eq!(rx.recv().await, Some(()));
        let item = clipboard.get().unwrap();
        assert_eq!(item.kind, Kind::Text);
        assert_eq!(item.data, b"copied");
        assert_eq!(item.sha256, text(b"copied").sha256);
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn incr_both_ways() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        let data = vec![b'x'; 600_000];
        clipboard.set(text(&data)).unwrap();
        let client = Client::new();
        let received = client.fetch(client.atoms.UTF8_STRING).await.unwrap();
        assert!(received.incremental);
        assert!(received.data == data);
        let _owner = Owner::start(ClipTarget::Utf8String, data.clone(), Delivery::Incr).await;
        let item = clipboard.get().unwrap();
        assert_eq!(item.kind, Kind::Text);
        assert!(item.data == data);
        assert_eq!(item.sha256, text(&data).sha256);
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn png_roundtrip() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        let data = png_4x4();
        clipboard.set(make_item(Kind::Png, data.clone())).unwrap();
        let client = Client::new();
        assert_eq!(client.fetch(client.atoms.PNG).await.unwrap().data, data);
        assert!(client.fetch(client.atoms.UTF8_STRING).await.is_none());
        let _owner = Owner::start(ClipTarget::Png, data.clone(), Delivery::Direct).await;
        let item = clipboard.get().unwrap();
        assert_eq!(item.kind, Kind::Png);
        assert_eq!(item.data, data);
        assert_eq!(item.sha256, make_item(Kind::Png, data).sha256);
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn own_set_no_watch() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        clipboard.watch(tx);
        clipboard.set(text(b"ours")).unwrap();
        clipboard.set(text(b"ours again")).unwrap();
        let _owner = Owner::start(
            ClipTarget::Utf8String,
            b"external".to_vec(),
            Delivery::Direct,
        )
        .await;
        assert_eq!(rx.recv().await, Some(()));
        assert_eq!(clipboard.get().unwrap().data, b"external");
        assert_eq!(
            rx.try_recv(),
            Err(tokio::sync::mpsc::error::TryRecvError::Empty)
        );
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn owner_gone() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        let (tx, mut rx) = tokio::sync::mpsc::channel(8);
        clipboard.watch(tx);
        let mut owner = Owner::start(
            ClipTarget::Utf8String,
            b"temporary".to_vec(),
            Delivery::Direct,
        )
        .await;
        assert_eq!(rx.recv().await, Some(()));
        owner.stop();
        assert!(clipboard.get().is_none());
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn rejects_bad_items() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        clipboard.set(text(b"valid")).unwrap();
        assert!(clipboard.set(text(&[0xff])).is_err());
        assert!(clipboard
            .set(text(&vec![b'x'; MAX_TEXT_BYTES + 1]))
            .is_err());
        assert!(clipboard
            .set(make_item(Kind::Png, vec![0; MAX_PNG_BYTES + 1]))
            .is_err());
        assert!(clipboard.set(make_item(Kind::Unspecified, vec![])).is_err());
        assert_eq!(clipboard.get().unwrap().data, b"valid");
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn text_fallbacks_and_segmented_property() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        {
            let _owner = Owner::start(
                ClipTarget::String,
                vec![0xe9, b'\r', b'\n', b'x', b'\r', 0],
                Delivery::Direct,
            )
            .await;
            assert_eq!(clipboard.get().unwrap().data, "\u{e9}\nx\n".as_bytes());
        }
        let data = vec![b'y'; 200_000];
        let _owner = Owner::start(ClipTarget::TextPlainUtf8, data.clone(), Delivery::Direct).await;
        assert!(clipboard.get().unwrap().data == data);
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn read_refusal_timeout_and_recovery() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        for delivery in [
            Delivery::Refuse,
            Delivery::Silent,
            Delivery::OversizedHeader,
            Delivery::Stall,
        ] {
            let _owner = Owner::start(ClipTarget::Utf8String, b"ignored".to_vec(), delivery).await;
            let start = Instant::now();
            assert!(clipboard.get().is_none());
            assert!(start.elapsed() < Duration::from_secs(3));
        }
        for (target, data) in [
            (ClipTarget::Utf8String, vec![0xff]),
            (ClipTarget::Png, b"invalid PNG".to_vec()),
        ] {
            let _owner = Owner::start(target, data, Delivery::Direct).await;
            assert!(clipboard.get().is_none());
        }
        {
            let _owner = Owner::start(
                ClipTarget::Utf8String,
                vec![b'x'; MAX_TEXT_BYTES + 1],
                Delivery::Underreported,
            )
            .await;
            assert!(clipboard.get().is_none());
        }
        assert_eq!(STEP_TIMEOUT, Duration::from_secs(1));
        let _owner = Owner::start(
            ClipTarget::Utf8String,
            b"recovered".to_vec(),
            Delivery::Direct,
        )
        .await;
        assert_eq!(clipboard.get().unwrap().data, b"recovered");
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn independent_readers_and_requestor_gone() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        let data = vec![b'z'; 600_000];
        clipboard.set(text(&data)).unwrap();
        {
            let abandoned = Client::new();
            let header = abandoned
                .initial(abandoned.atoms.UTF8_STRING, abandoned.atoms.PROPERTY, false)
                .await
                .unwrap();
            assert_eq!(header.type_, abandoned.atoms.INCR);
        }
        let a = Client::new();
        let b = Client::new();
        let (a, b) = tokio::join!(a.fetch(a.atoms.UTF8_STRING), b.fetch(b.atoms.UTF8_STRING));
        assert!(a.unwrap().data == data);
        assert!(b.unwrap().data == data);
    })
    .await
    .expect("X11 clipboard test timed out");
}

#[tokio::test(flavor = "multi_thread", worker_threads = 2)]
async fn drop_releases_selection_with_stalled_reader() {
    let _guard = assert_layer();
    timeout(TEST_TIMEOUT, async {
        let clipboard = X11Clipboard::connect(None).unwrap();
        clipboard.set(text(&vec![b'x'; 600_000])).unwrap();
        let client = Client::new();
        let header = client
            .initial(client.atoms.UTF8_STRING, client.atoms.PROPERTY, false)
            .await
            .unwrap();
        assert_eq!(header.type_, client.atoms.INCR);
        let start = Instant::now();
        drop(clipboard);
        assert!(start.elapsed() < Duration::from_secs(1));
        assert_eq!(
            client
                .connection
                .get_selection_owner(client.atoms.CLIPBOARD)
                .unwrap()
                .reply()
                .unwrap()
                .owner,
            NONE
        );
    })
    .await
    .expect("X11 clipboard test timed out");
}
