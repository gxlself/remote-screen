//! Windows clipboard access serialized on a dedicated message thread.

use std::ffi::c_void;
use std::ptr::{self, NonNull};
use std::sync::atomic::{AtomicIsize, AtomicU32, Ordering};
use std::sync::mpsc::{self, Receiver, Sender, SyncSender};
use std::sync::Arc;
use std::thread::{self, JoinHandle};
use std::time::Duration;

use host_core::platform::{ClipboardItem, PlatformError};
use tokio::sync::mpsc as tokio_mpsc;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::{
    GlobalFree, HANDLE, HGLOBAL, HINSTANCE, HWND, LPARAM, LRESULT, WPARAM,
};
use windows::Win32::System::DataExchange::{
    AddClipboardFormatListener, CloseClipboard, EmptyClipboard, GetClipboardData,
    GetClipboardSequenceNumber, IsClipboardFormatAvailable, OpenClipboard,
    RegisterClipboardFormatW, RemoveClipboardFormatListener, SetClipboardData,
};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::System::Memory::{
    GlobalAlloc, GlobalLock, GlobalSize, GlobalUnlock, GMEM_MOVEABLE,
};
use windows::Win32::System::Ole::{CF_DIB, CF_DIBV5, CF_UNICODETEXT};
use windows::Win32::UI::WindowsAndMessaging::{
    CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetMessageW, PostMessageW,
    RegisterClassW, TranslateMessage, UnregisterClassW, HWND_MESSAGE, MSG, WINDOW_EX_STYLE,
    WINDOW_STYLE, WM_APP, WM_CLIPBOARDUPDATE, WNDCLASSW,
};

use host_core::clip_convert::{
    pick_format, read_item, write_formats, ClipFormat, OwnWrites, MAX_PNG_BYTES,
};

const REQUEST_TIMEOUT: Duration = Duration::from_secs(2);
static NEXT_CLASS_ID: AtomicU32 = AtomicU32::new(1);

enum Request {
    Get(SyncSender<Option<ClipboardItem>>),
    Set(ClipboardItem, SyncSender<Result<(), PlatformError>>),
    Watch(tokio_mpsc::Sender<()>),
    Stop,
}

pub struct WinClipboard {
    requests: Sender<Request>,
    hwnd: Arc<AtomicIsize>,
    thread: Option<JoinHandle<()>>,
}

impl WinClipboard {
    pub fn start() -> Result<Self, PlatformError> {
        let (requests, receiver) = mpsc::channel();
        let (ready_tx, ready_rx) = mpsc::sync_channel(1);
        let hwnd = Arc::new(AtomicIsize::new(0));
        let thread_hwnd = Arc::clone(&hwnd);
        let thread = thread::Builder::new()
            .name("rs-clipboard".into())
            .spawn(move || clipboard_thread(receiver, ready_tx, thread_hwnd))
            .map_err(|err| PlatformError::Platform(format!("clipboard thread: {err}")))?;
        let clipboard = Self {
            requests,
            hwnd,
            thread: Some(thread),
        };
        let result = ready_rx.recv_timeout(REQUEST_TIMEOUT);
        // A late startup must fail its acknowledgement and release its window.
        drop(ready_rx);
        match result {
            Ok(Ok(())) => Ok(clipboard),
            Ok(Err(err)) => Err(err),
            Err(_) => Err(PlatformError::Platform("clipboard timeout".into())),
        }
    }

    pub fn get(&self) -> Option<ClipboardItem> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.request(Request::Get(reply_tx)).ok()?;
        reply_rx.recv_timeout(REQUEST_TIMEOUT).ok().flatten()
    }

    pub fn set(&self, item: &ClipboardItem) -> Result<(), PlatformError> {
        let (reply_tx, reply_rx) = mpsc::sync_channel(1);
        self.request(Request::Set(item.clone(), reply_tx))?;
        reply_rx
            .recv_timeout(REQUEST_TIMEOUT)
            .map_err(|_| PlatformError::Platform("clipboard timeout".into()))?
    }

    pub fn watch(&self, tx: tokio_mpsc::Sender<()>) {
        let _ = self.request(Request::Watch(tx));
    }

    fn request(&self, request: Request) -> Result<(), PlatformError> {
        self.requests
            .send(request)
            .map_err(|_| PlatformError::Platform("clipboard thread stopped".into()))?;
        self.wake()
    }

    fn wake(&self) -> Result<(), PlatformError> {
        let raw = self.hwnd.load(Ordering::Acquire);
        if raw == 0 {
            return Err(PlatformError::Platform("clipboard thread stopped".into()));
        }
        // SAFETY: this is a posted notification with no borrowed parameters;
        // the clipboard thread publishes and clears the HWND as it owns it.
        unsafe { PostMessageW(Some(HWND(raw as *mut c_void)), WM_APP, WPARAM(0), LPARAM(0)) }
            .map_err(|err| PlatformError::Platform(format!("clipboard wake: {err}")))
    }
}

impl Drop for WinClipboard {
    fn drop(&mut self) {
        let _ = self.request(Request::Stop);
        if let Some(thread) = self.thread.take() {
            let _ = thread.join();
        }
    }
}

fn clipboard_thread(
    requests: Receiver<Request>,
    ready: SyncSender<Result<(), PlatformError>>,
    hwnd_slot: Arc<AtomicIsize>,
) {
    let window = match ClipboardWindow::new(hwnd_slot) {
        Ok(window) => window,
        Err(err) => {
            let _ = ready.send(Err(err));
            return;
        }
    };
    // SAFETY: w! supplies a static NUL-terminated UTF-16 format name.
    let png_format = unsafe { RegisterClipboardFormatW(w!("PNG")) };
    if png_format == 0 {
        let _ = ready.send(Err(PlatformError::Platform(
            "RegisterClipboardFormatW(PNG) failed".into(),
        )));
        return;
    }
    if ready.send(Ok(())).is_err() {
        return;
    }

    let mut watchers = Vec::new();
    let mut own_writes = OwnWrites::new();
    loop {
        let mut message = MSG::default();
        // SAFETY: message is writable and this thread owns the message queue.
        if unsafe { GetMessageW(&mut message, None, 0, 0) }.0 <= 0 {
            break;
        }
        match message.message {
            WM_APP => {
                if !handle_requests(
                    &requests,
                    window.hwnd,
                    png_format,
                    &mut watchers,
                    &mut own_writes,
                ) {
                    break;
                }
            }
            WM_CLIPBOARDUPDATE => notify_watchers(&mut watchers, &own_writes),
            _ => {
                // SAFETY: GetMessageW initialized this message on its owning thread.
                unsafe {
                    let _ = TranslateMessage(&message);
                    DispatchMessageW(&message);
                }
            }
        }
    }
}

fn handle_requests(
    requests: &Receiver<Request>,
    hwnd: HWND,
    png_format: u32,
    watchers: &mut Vec<tokio_mpsc::Sender<()>>,
    own_writes: &mut OwnWrites,
) -> bool {
    while let Ok(request) = requests.try_recv() {
        match request {
            Request::Get(reply) => {
                let _ = reply.send(read_clipboard(hwnd, png_format));
            }
            Request::Set(item, reply) => {
                let _ = reply.send(write_clipboard(hwnd, png_format, &item, own_writes));
            }
            Request::Watch(tx) => watchers.push(tx),
            Request::Stop => return false,
        }
    }
    true
}

fn notify_watchers(watchers: &mut Vec<tokio_mpsc::Sender<()>>, own_writes: &OwnWrites) {
    // SAFETY: this only reads the clipboard sequence number.
    let sequence = unsafe { GetClipboardSequenceNumber() };
    watchers.retain(|tx| !tx.is_closed());
    if own_writes.is_own(sequence) {
        return;
    }
    watchers.retain(|tx| match tx.try_send(()) {
        Ok(()) | Err(tokio_mpsc::error::TrySendError::Full(_)) => true,
        Err(tokio_mpsc::error::TrySendError::Closed(_)) => false,
    });
}

fn native_format(format: ClipFormat, png_format: u32) -> u32 {
    match format {
        ClipFormat::UnicodeText => u32::from(CF_UNICODETEXT.0),
        ClipFormat::Png => png_format,
        ClipFormat::DibV5 => u32::from(CF_DIBV5.0),
        ClipFormat::Dib => u32::from(CF_DIB.0),
    }
}

fn read_clipboard(hwnd: HWND, png_format: u32) -> Option<ClipboardItem> {
    let (format, data) = {
        let _clipboard = ClipboardGuard::open(hwnd)?;
        let formats: Vec<_> = [
            ClipFormat::UnicodeText,
            ClipFormat::Png,
            ClipFormat::DibV5,
            ClipFormat::Dib,
        ]
        .into_iter()
        .filter(|&format| {
            // SAFETY: querying format availability requires no pointers.
            unsafe { IsClipboardFormatAvailable(native_format(format, png_format)) }.is_ok()
        })
        .collect();
        let format = pick_format(&formats)?;
        // SAFETY: the guard keeps the clipboard open while the handle is copied.
        let handle = unsafe { GetClipboardData(native_format(format, png_format)) }.ok()?;
        let limit = (format == ClipFormat::Png).then_some(MAX_PNG_BYTES);
        (format, read_global(handle, limit)?)
    };
    read_item(format, data)
}

fn write_clipboard(
    hwnd: HWND,
    png_format: u32,
    item: &ClipboardItem,
    own_writes: &mut OwnWrites,
) -> Result<(), PlatformError> {
    let allocations = write_formats(item)?
        .into_iter()
        .map(|(format, data)| Ok((native_format(format, png_format), OwnedGlobal::new(&data)?)))
        .collect::<Result<Vec<_>, PlatformError>>()?;
    let clipboard = ClipboardGuard::open(hwnd)
        .ok_or_else(|| PlatformError::Platform("clipboard busy".into()))?;
    // SAFETY: the guard owns the open clipboard on this thread.
    unsafe { EmptyClipboard() }
        .map_err(|err| PlatformError::Platform(format!("EmptyClipboard: {err}")))?;
    let result = allocations
        .into_iter()
        .try_for_each(|(format, memory)| memory.publish(format));
    drop(clipboard);
    // Suppress our notification even if only some representations were written.
    // SAFETY: the clipboard has been closed before reading its sequence number.
    own_writes.note_written(unsafe { GetClipboardSequenceNumber() });
    result
}

struct ClipboardGuard;

impl ClipboardGuard {
    fn open(hwnd: HWND) -> Option<Self> {
        for _ in 0..10 {
            // SAFETY: hwnd is the clipboard thread's live owner window.
            if unsafe { OpenClipboard(Some(hwnd)) }.is_ok() {
                return Some(Self);
            }
            thread::sleep(Duration::from_millis(10));
        }
        None
    }
}

impl Drop for ClipboardGuard {
    fn drop(&mut self) {
        // SAFETY: this guard matches a successful OpenClipboard on this thread.
        let _ = unsafe { CloseClipboard() };
    }
}

fn read_global(handle: HANDLE, max_size: Option<usize>) -> Option<Vec<u8>> {
    let hglobal = HGLOBAL(handle.0);
    // SAFETY: the open clipboard keeps the global memory handle alive.
    let size = unsafe { GlobalSize(hglobal) };
    if size == 0 || size > isize::MAX as usize || max_size.is_some_and(|max| size > max) {
        return None;
    }
    let lock = GlobalLockGuard::new(hglobal)?;
    // SAFETY: GlobalSize bounds this read and the guard keeps the allocation locked.
    Some(unsafe { std::slice::from_raw_parts(lock.pointer.as_ptr(), size) }.to_vec())
}

struct GlobalLockGuard {
    handle: HGLOBAL,
    pointer: NonNull<u8>,
}

impl GlobalLockGuard {
    fn new(handle: HGLOBAL) -> Option<Self> {
        // SAFETY: callers hold a live global memory allocation for this guard's lifetime.
        let pointer = NonNull::new(unsafe { GlobalLock(handle) }.cast::<u8>())?;
        Some(Self { handle, pointer })
    }
}

impl Drop for GlobalLockGuard {
    fn drop(&mut self) {
        // SAFETY: this guard owns the matching successful GlobalLock call.
        let _ = unsafe { GlobalUnlock(self.handle) };
    }
}

struct OwnedGlobal {
    handle: HGLOBAL,
    transferred: bool,
}

impl OwnedGlobal {
    fn new(bytes: &[u8]) -> Result<Self, PlatformError> {
        // SAFETY: GMEM_MOVEABLE is required for SetClipboardData ownership transfer.
        let handle = unsafe { GlobalAlloc(GMEM_MOVEABLE, bytes.len()) }
            .map_err(|err| PlatformError::Platform(format!("GlobalAlloc: {err}")))?;
        let memory = Self {
            handle,
            transferred: false,
        };
        let lock = GlobalLockGuard::new(handle)
            .ok_or_else(|| PlatformError::Platform("GlobalLock failed".into()))?;
        // SAFETY: this fresh allocation holds bytes.len() writable bytes; the source
        // slice cannot overlap it. The lock is dropped before the memory is returned.
        unsafe { ptr::copy_nonoverlapping(bytes.as_ptr(), lock.pointer.as_ptr(), bytes.len()) };
        drop(lock);
        Ok(memory)
    }

    fn publish(mut self, format: u32) -> Result<(), PlatformError> {
        // SAFETY: memory is unlocked and the clipboard is open on this thread.
        unsafe { SetClipboardData(format, Some(HANDLE(self.handle.0))) }
            .map_err(|err| PlatformError::Platform(format!("SetClipboardData: {err}")))?;
        self.transferred = true;
        Ok(())
    }
}

impl Drop for OwnedGlobal {
    fn drop(&mut self) {
        if !self.transferred {
            // SAFETY: Windows has not taken ownership and all locks are released.
            let _ = unsafe { GlobalFree(Some(self.handle)) };
        }
    }
}

struct WindowClass {
    name: Vec<u16>,
    instance: HINSTANCE,
}

impl WindowClass {
    fn new() -> Result<Self, PlatformError> {
        // SAFETY: a null name requests the current process module without loading it.
        let instance = unsafe { GetModuleHandleW(None) }
            .map_err(|err| PlatformError::Platform(format!("GetModuleHandleW: {err}")))?
            .into();
        let id = NEXT_CLASS_ID.fetch_add(1, Ordering::Relaxed);
        let name: Vec<u16> = format!("RemoteScreenClipboard{id}")
            .encode_utf16()
            .chain(Some(0))
            .collect();
        let class = WNDCLASSW {
            lpfnWndProc: Some(window_proc),
            hInstance: instance,
            lpszClassName: PCWSTR(name.as_ptr()),
            ..Default::default()
        };
        // SAFETY: the class uses a static callback and a valid terminated name.
        if unsafe { RegisterClassW(&class) } == 0 {
            return Err(PlatformError::Platform("RegisterClassW failed".into()));
        }
        Ok(Self { name, instance })
    }
}

impl Drop for WindowClass {
    fn drop(&mut self) {
        // SAFETY: our unique class has no remaining windows when this guard drops.
        let _ = unsafe { UnregisterClassW(PCWSTR(self.name.as_ptr()), Some(self.instance)) };
    }
}

struct ClipboardWindow {
    hwnd: HWND,
    listening: bool,
    hwnd_slot: Arc<AtomicIsize>,
    _class: WindowClass,
}

impl ClipboardWindow {
    fn new(hwnd_slot: Arc<AtomicIsize>) -> Result<Self, PlatformError> {
        let class = WindowClass::new()?;
        // SAFETY: the class is registered and HWND_MESSAGE creates a window
        // owned by this thread, without any borrowed creation parameters.
        let hwnd = unsafe {
            CreateWindowExW(
                WINDOW_EX_STYLE(0),
                PCWSTR(class.name.as_ptr()),
                PCWSTR::null(),
                WINDOW_STYLE(0),
                0,
                0,
                0,
                0,
                Some(HWND_MESSAGE),
                None,
                Some(class.instance),
                None,
            )
        }
        .map_err(|err| PlatformError::Platform(format!("CreateWindowExW: {err}")))?;
        let mut window = Self {
            hwnd,
            listening: false,
            hwnd_slot,
            _class: class,
        };
        // SAFETY: the message-only window is live and owned by this thread.
        unsafe { AddClipboardFormatListener(hwnd) }
            .map_err(|err| PlatformError::Platform(format!("AddClipboardFormatListener: {err}")))?;
        window.listening = true;
        window.hwnd_slot.store(hwnd.0 as isize, Ordering::Release);
        Ok(window)
    }
}

impl Drop for ClipboardWindow {
    fn drop(&mut self) {
        self.hwnd_slot.store(0, Ordering::Release);
        if self.listening {
            // SAFETY: this thread registered the listener on its own live window.
            let _ = unsafe { RemoveClipboardFormatListener(self.hwnd) };
        }
        // SAFETY: this thread created and still owns the message-only window.
        let _ = unsafe { DestroyWindow(self.hwnd) };
    }
}

unsafe extern "system" fn window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: user32 supplies the callback parameters; queued clipboard and
    // request messages are handled by the outer loop before dispatch.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}
