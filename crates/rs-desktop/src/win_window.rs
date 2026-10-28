//! ：Win32 会话窗口与输入采集。
//!
//! `factory` must run on the tao/UI thread. Only the integer HWND and a command
//! queue cross threads; callbacks and all window-owned resources stay on UI.

use std::{
    cell::{Cell, RefCell},
    collections::VecDeque,
    mem::{size_of, take},
    panic::{catch_unwind, AssertUnwindSafe},
    rc::Rc,
    sync::{
        atomic::{AtomicBool, Ordering},
        mpsc::{self, SyncSender},
        Arc, Mutex, MutexGuard, OnceLock,
    },
    time::{Duration, SystemTime, UNIX_EPOCH},
};

use tracing::error;
use windows::{
    core::{w, PCWSTR},
    Win32::{
        Foundation::{COLORREF, HINSTANCE, HWND, LPARAM, LRESULT, POINT, RECT, SIZE, WPARAM},
        Graphics::Gdi::*,
        System::{LibraryLoader::GetModuleHandleW, Threading::GetCurrentThreadId},
        UI::{
            HiDpi::{
                AdjustWindowRectExForDpi, GetDpiForSystem, GetDpiForWindow, GetSystemMetricsForDpi,
                SystemParametersInfoForDpi,
            },
            Input::{
                Ime::{ImmAssociateContextEx, HIMC},
                KeyboardAndMouse::{
                    GetCapture, GetDoubleClickTime, ReleaseCapture, SetCapture, TrackMouseEvent,
                    TME_LEAVE, TRACKMOUSEEVENT,
                },
            },
            WindowsAndMessaging::*,
        },
    },
};

use crate::{
    platform::{
        CursorImage, FrameSink, PlatformError, PointerCapture, PointerInput, SessionMenu,
        SessionWindow, SessionWindowFactory, VideoCodec, WindowChange, WindowCommand, WindowEvents,
        WindowSpec,
    },
    pointer::{ClickCounter, Viewport},
    win_model::{self, CursorBitmap, KeyFilter, WinMenuItem},
    win_present::WinVideoSink,
    win_present_model::target_rect,
};

const CREATE: u32 = WM_APP + 1;
const COMMANDS: u32 = WM_APP + 2;
// winuser.h; windows-rs puts this constant under UI::Controls, although no
// common-control APIs are used by our native windows.
const WM_MOUSELEAVE: u32 = 0x02A3;
// Tauri's WINDOWS_APP_ICON_RESOURCE_ID (tauri-build / tauri-winres).
const APP_ICON_RESOURCE: usize = 32512;
const GEOMETRY_TIMER: usize = 1;

type CreateResult = Result<Arc<WinWindow>, PlatformError>;

struct CreateReq {
    spec: WindowSpec,
    events: Arc<dyn WindowEvents>,
    reply: SyncSender<CreateResult>,
}

struct DispatchShared {
    hwnd: isize,
    thread: u32,
    // Serializes PostMessage against UI-thread teardown, including draining
    // outstanding boxed CreateReqs before destroying the message-only window.
    alive: Mutex<bool>,
}

struct Dispatcher(Arc<DispatchShared>);

impl Drop for Dispatcher {
    fn drop(&mut self) {
        let mut alive = lock(&self.0.alive);
        *alive = false;
        // SAFETY: TLS is destroyed on the UI thread; the gate prevents any new
        // requests being posted while we reclaim the pending owned boxes.
        unsafe {
            let mut msg = MSG::default();
            while PeekMessageW(&mut msg, Some(hwnd(self.0.hwnd)), CREATE, CREATE, PM_REMOVE)
                .as_bool()
            {
                drop(Box::from_raw(msg.lParam.0 as *mut CreateReq));
            }
            let _ = DestroyWindow(hwnd(self.0.hwnd));
        }
    }
}

#[derive(Default)]
struct Hooks {
    windows: Vec<isize>,
    hook: HHOOK,
}

impl Drop for Hooks {
    fn drop(&mut self) {
        if !self.hook.is_invalid() {
            // SAFETY: this TLS owns the hook installed on this UI thread.
            unsafe {
                let _ = UnhookWindowsHookEx(self.hook);
            }
        }
    }
}

thread_local! {
    static DISPATCHER: RefCell<Option<Dispatcher>> = const { RefCell::new(None) };
    static HOOKS: RefCell<Hooks> = RefCell::new(Hooks::default());
    // ClipCursor is desktop-global: an old window must never release the clip
    // after another one has acquired it, even when messages are interleaved.
    static CLIP_OWNER: Cell<isize> = const { Cell::new(0) };
}

/// Must be called on the UI (Tauri main) thread.
pub fn factory() -> Option<Arc<dyn SessionWindowFactory>> {
    match WinFactory::new() {
        Ok(factory) => Some(Arc::new(factory)),
        Err(err) => {
            error!(error = %err, "failed to create Windows session window factory");
            None
        }
    }
}

pub struct WinFactory {
    dispatcher: Arc<DispatchShared>,
}

// SAFETY: HWND is stored as an integer. Workers only PostMessage under the
// teardown gate; window state is dereferenced exclusively on its UI thread.
unsafe impl Send for WinFactory {}
// SAFETY: the dispatcher gate protects posting; all Win32 window work is on UI.
unsafe impl Sync for WinFactory {}

impl WinFactory {
    /// Typed counterpart of `factory`, also used by the interactive tests.
    /// Must be called on the UI thread; repeated calls share its dispatcher.
    pub fn new() -> Result<Self, PlatformError> {
        register_classes()?;
        DISPATCHER.with(|slot| {
            let mut slot = slot.borrow_mut();
            if let Some(dispatcher) = slot.as_ref() {
                return Ok(Self {
                    dispatcher: Arc::clone(&dispatcher.0),
                });
            }
            // SAFETY: classes are registered, the strings are static, and the
            // message-only window belongs to this calling UI thread.
            let dispatcher = unsafe {
                let instance = GetModuleHandleW(None).map_err(platform_error)?;
                let window = CreateWindowExW(
                    WINDOW_EX_STYLE::default(),
                    w!("RemoteScreenDispatch"),
                    w!(""),
                    WINDOW_STYLE::default(),
                    0,
                    0,
                    0,
                    0,
                    Some(HWND_MESSAGE),
                    None,
                    Some(instance.into()),
                    None,
                )
                .map_err(platform_error)?;
                Arc::new(DispatchShared {
                    hwnd: window.0 as isize,
                    thread: GetCurrentThreadId(),
                    alive: Mutex::new(true),
                })
            };
            *slot = Some(Dispatcher(Arc::clone(&dispatcher)));
            Ok(Self { dispatcher })
        })
    }

    /// Same creation path as the trait, retaining the concrete window for UI
    /// diagnostics. The calling worker can block; the UI thread never does.
    pub fn create_window(&self, spec: &WindowSpec, events: Arc<dyn WindowEvents>) -> CreateResult {
        // SAFETY: querying the calling thread's ID has no preconditions.
        if unsafe { GetCurrentThreadId() } == self.dispatcher.thread {
            return create_window(spec, events);
        }
        let (reply, receiver) = mpsc::sync_channel(1);
        let request = Box::new(CreateReq {
            spec: spec.clone(),
            events,
            reply,
        });
        {
            let alive = lock(&self.dispatcher.alive);
            if !*alive {
                return Err(PlatformError::Failed("UI dispatcher closed".into()));
            }
            let raw = Box::into_raw(request);
            // SAFETY: the dispatcher gate keeps the HWND alive. On successful
            // posting the UI proc owns the box; on error we reclaim it here.
            if let Err(err) = unsafe {
                PostMessageW(
                    Some(hwnd(self.dispatcher.hwnd)),
                    CREATE,
                    WPARAM(0),
                    LPARAM(raw as isize),
                )
            } {
                // SAFETY: a failed PostMessage has not transferred ownership.
                unsafe { drop(Box::from_raw(raw)) };
                return Err(platform_error(err));
            }
        }
        receiver
            .recv_timeout(Duration::from_secs(5))
            .map_err(|err| {
                PlatformError::Failed(match err {
                    mpsc::RecvTimeoutError::Timeout => "timed out creating session window".into(),
                    mpsc::RecvTimeoutError::Disconnected => "UI dispatcher closed".into(),
                })
            })?
    }
}

impl SessionWindowFactory for WinFactory {
    fn create(
        &self,
        spec: &WindowSpec,
        events: Arc<dyn WindowEvents>,
    ) -> Result<Arc<dyn SessionWindow>, PlatformError> {
        self.create_window(spec, events)
            .map(|window| window as Arc<dyn SessionWindow>)
    }

    fn can_decode(&self, codec: VideoCodec) -> bool {
        crate::win_ffmpeg::can_decode(codec)
    }

    fn screen_pixel_width(&self) -> u32 {
        // SAFETY: monitor queries accept a null foreground HWND and fall back
        // to the primary monitor; no window state is accessed from this thread.
        unsafe {
            monitor_rect(GetForegroundWindow(), MONITOR_DEFAULTTOPRIMARY)
                .map(|rect| (rect.right - rect.left).max(0) as u32)
                .unwrap_or(1920)
        }
    }
}

struct Shared {
    queue: Mutex<VecDeque<WindowCommand>>,
    alive: AtomicBool,
}

pub struct WinWindow {
    hwnd: isize,
    thread: u32,
    shared: Arc<Shared>,
    sink: Arc<WinVideoSink>,
}

// SAFETY: the integer HWND is only used for PostMessage by workers; UI state is
// never exposed across threads. The queue, alive flag, and sink are synchronized.
unsafe impl Send for WinWindow {}
// SAFETY: see Send; debug_state enforces the UI-thread precondition at runtime.
unsafe impl Sync for WinWindow {}

#[derive(Debug, Clone, PartialEq)]
pub struct WinDebugState {
    pub title: String,
    pub fullscreen: bool,
    pub keyboard_capture: bool,
    pub grab_hook_installed: bool,
    pub clip: Option<(i32, i32, i32, i32)>,
    pub overlay: Option<String>,
    pub remote_cursor_visible: bool,
    pub cursor_size: Option<(u32, u32)>,
    pub sysmenu_ids: Vec<u32>,
    pub dpi: u32,
}

impl WinWindow {
    /// Stable integer handle for UI-thread diagnostics / interactive tests.
    pub fn hwnd(&self) -> isize {
        self.hwnd
    }

    /// Only call on the UI thread, while the session window is alive.
    pub fn debug_state(&self) -> WinDebugState {
        // SAFETY: GetCurrentThreadId has no preconditions.
        assert_eq!(
            unsafe { GetCurrentThreadId() },
            self.thread,
            "UI thread only"
        );
        assert!(
            self.shared.alive.load(Ordering::Acquire),
            "window is closed"
        );
        let state = state(hwnd(self.hwnd)).expect("live session HWND owns State");
        // SAFETY: this thread owns the HWND; title storage is a valid buffer.
        let title = unsafe {
            let mut text = vec![0; GetWindowTextLengthW(hwnd(self.hwnd)).max(0) as usize + 1];
            let len = GetWindowTextW(hwnd(self.hwnd), &mut text);
            String::from_utf16_lossy(&text[..len.max(0) as usize])
        };
        let grab_hook_installed = HOOKS.with(|hooks| {
            let hooks = hooks.borrow();
            hooks.windows.contains(&self.hwnd) && !hooks.hook.is_invalid()
        });
        let clip = CLIP_OWNER.with(|owner| {
            if owner.get() != self.hwnd {
                return None;
            }
            let mut rect = RECT::default();
            // SAFETY: valid writable RECT; no ownership is transferred.
            unsafe { GetClipCursor(&mut rect) }.ok()?;
            Some((rect.left, rect.top, rect.right, rect.bottom))
        });
        let overlay = state.overlay_text.borrow().clone();
        let cursor_size = state
            .cursor
            .borrow()
            .as_ref()
            .map(|cursor| (cursor.bitmap.width, cursor.bitmap.height));
        let sysmenu_ids = state.sysmenu_ids.borrow().clone();
        WinDebugState {
            title,
            fullscreen: state.fullscreen.get(),
            keyboard_capture: state.keyboard_capture.get(),
            grab_hook_installed,
            clip,
            overlay,
            // SAFETY: these owned popup handles are only touched on UI.
            remote_cursor_visible: unsafe { IsWindowVisible(state.remote.get()).as_bool() },
            cursor_size,
            sysmenu_ids,
            dpi: state.dpi.get(),
        }
    }
}

impl SessionWindow for WinWindow {
    fn frame_sink(&self) -> Arc<dyn FrameSink> {
        self.sink.clone()
    }

    fn apply(&self, cmd: WindowCommand) {
        let mut queue = lock(&self.shared.queue);
        if !self.shared.alive.load(Ordering::Acquire) {
            return;
        }
        queue.push_back(cmd);
        // SAFETY: this is an asynchronous wakeup with no borrowed parameters.
        // WM_DESTROY takes the same queue lock before marking the HWND dead.
        if let Err(err) =
            unsafe { PostMessageW(Some(hwnd(self.hwnd)), COMMANDS, WPARAM(0), LPARAM(0)) }
        {
            error!(error = %err, "failed to wake Windows session command queue");
        }
    }
}

impl Drop for WinWindow {
    fn drop(&mut self) {
        self.apply(WindowCommand::Close);
    }
}

// The boxed userdata owns an Rc so a reentrant DestroyWindow can drop userdata
// without invalidating the current proc's state. No &mut State spans Win32 calls.
struct State {
    inner: Rc<UiState>,
}

struct UiState {
    hwnd: HWND,
    events: Arc<dyn WindowEvents>,
    shared: Arc<Shared>,
    sink: Arc<WinVideoSink>,
    dpi: Cell<u32>,
    keys: RefCell<KeyFilter>,
    clicks: RefCell<ClickCounter>,
    held: Cell<u8>,
    held_clicks: Cell<[u8; 5]>,
    tracking_leave: Cell<bool>,
    focused: Cell<bool>,
    minimized: Cell<bool>,
    fullscreen: Cell<bool>,
    placement: Cell<WINDOWPLACEMENT>,
    style: Cell<isize>,
    keyboard_capture: Cell<bool>,
    system_grab: Cell<bool>,
    pointer_capture: Cell<PointerCapture>,
    cursor_image: RefCell<Option<CursorImage>>,
    cursor: RefCell<Option<Rc<HostCursor>>>,
    cursor_hidden: Cell<bool>,
    show_remote: Cell<bool>,
    remote_position: Cell<(f32, f32)>,
    remote: Cell<HWND>,
    overlay: Cell<HWND>,
    overlay_text: RefCell<Option<String>>,
    menu: RefCell<SessionMenu>,
    sysmenu_ids: RefCell<Vec<u32>>,
    video_size: Cell<Option<(u32, u32)>>,
}

fn state(window: HWND) -> Option<Rc<UiState>> {
    // SAFETY: only UI-thread entry points call this for our own class. Userdata
    // is null until setup, then Box<State> until WM_NCDESTROY clears it first.
    unsafe {
        let ptr = GetWindowLongPtrW(window, GWLP_USERDATA) as *const State;
        ptr.as_ref().map(|state| Rc::clone(&state.inner))
    }
}

fn create_window(spec: &WindowSpec, events: Arc<dyn WindowEvents>) -> CreateResult {
    // SAFETY: called only on the dispatcher/UI thread with registered classes.
    // The window stays hidden until its userdata and render sink are installed.
    unsafe {
        let instance = GetModuleHandleW(None).map_err(platform_error)?;
        let dpi = GetDpiForSystem().max(96);
        let rect = initial_rect(spec, dpi)?;
        let title = wide(&spec.title);
        let window = CreateWindowExW(
            WINDOW_EX_STYLE::default(),
            w!("RemoteScreenSession"),
            PCWSTR(title.as_ptr()),
            WS_OVERLAPPEDWINDOW,
            CW_USEDEFAULT,
            CW_USEDEFAULT,
            rect.right - rect.left,
            rect.bottom - rect.top,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .map_err(platform_error)?;
        let dpi = GetDpiForWindow(window).max(96);
        // CW_USEDEFAULT may choose a monitor whose DPI differs from the system.
        let setup = (|| {
            let rect = initial_rect(spec, dpi)?;
            SetWindowPos(
                window,
                None,
                0,
                0,
                rect.right - rect.left,
                rect.bottom - rect.top,
                SWP_NOMOVE | SWP_NOZORDER | SWP_NOACTIVATE,
            )
            .map_err(platform_error)?;
            let mut client = RECT::default();
            GetClientRect(window, &mut client).map_err(platform_error)?;
            Ok::<_, PlatformError>(client)
        })();
        let client = match setup {
            Ok(client) => client,
            Err(err) => {
                let _ = DestroyWindow(window);
                return Err(err);
            }
        };
        let _ = ImmAssociateContextEx(window, HIMC::default(), 0);
        let shared = Arc::new(Shared {
            queue: Mutex::new(VecDeque::new()),
            alive: AtomicBool::new(true),
        });
        let sink = WinVideoSink::new(window, client.right as u32, client.bottom as u32);
        let inner = Rc::new(UiState {
            hwnd: window,
            events,
            shared: Arc::clone(&shared),
            sink: Arc::clone(&sink),
            dpi: Cell::new(dpi),
            keys: RefCell::new(KeyFilter::new()),
            clicks: RefCell::new(ClickCounter::new(
                u64::from(GetDoubleClickTime()) * 1000,
                f64::from(GetSystemMetricsForDpi(SM_CXDOUBLECLK, dpi)) * 96.0
                    / f64::from(dpi)
                    / 2.0,
            )),
            held: Cell::new(0),
            held_clicks: Cell::new([0; 5]),
            tracking_leave: Cell::new(false),
            focused: Cell::new(false),
            minimized: Cell::new(false),
            fullscreen: Cell::new(false),
            placement: Cell::new(WINDOWPLACEMENT::default()),
            style: Cell::new(0),
            keyboard_capture: Cell::new(false),
            system_grab: Cell::new(false),
            pointer_capture: Cell::new(PointerCapture::Free),
            cursor_image: RefCell::new(None),
            cursor: RefCell::new(None),
            cursor_hidden: Cell::new(false),
            show_remote: Cell::new(false),
            remote_position: Cell::new((0.5, 0.5)),
            remote: Cell::new(HWND::default()),
            overlay: Cell::new(HWND::default()),
            overlay_text: RefCell::new(None),
            menu: RefCell::new(SessionMenu::default()),
            sysmenu_ids: RefCell::new(Vec::new()),
            video_size: Cell::new(None),
        });
        let boxed = Box::new(State {
            inner: Rc::clone(&inner),
        });
        SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(boxed) as isize);
        // Video size becomes available asynchronously without a window resize.
        // Polling only geometry (not frames) keeps clip / layers in sync then.
        SetTimer(Some(window), GEOMETRY_TIMER, 100, None);
        let result = Arc::new(WinWindow {
            hwnd: window.0 as isize,
            thread: GetCurrentThreadId(),
            shared,
            sink,
        });
        let _ = ShowWindow(window, SW_SHOW);
        let _ = SetForegroundWindow(window);
        inner.resized();
        Ok(result)
    }
}

fn initial_rect(spec: &WindowSpec, dpi: u32) -> Result<RECT, PlatformError> {
    let pixels = |logical: u32| -> Result<i32, PlatformError> {
        i32::try_from((u64::from(logical) * u64::from(dpi) + 48) / 96)
            .ok()
            .filter(|&n| n > 0 && n <= i32::MAX / 2)
            .ok_or_else(|| PlatformError::Failed("invalid session window size".into()))
    };
    let mut rect = RECT {
        right: pixels(spec.width)?,
        bottom: pixels(spec.height)?,
        ..Default::default()
    };
    // SAFETY: valid RECT; no menu and the exact styles used for creation.
    unsafe {
        AdjustWindowRectExForDpi(
            &mut rect,
            WS_OVERLAPPEDWINDOW,
            false,
            WINDOW_EX_STYLE::default(),
            dpi,
        )
    }
    .map_err(platform_error)?;
    Ok(rect)
}

fn register_classes() -> Result<(), PlatformError> {
    static REGISTERED: OnceLock<Result<(), PlatformError>> = OnceLock::new();
    REGISTERED
        .get_or_init(|| {
            // SAFETY: class names and proc pointers are static; registration is
            // process-wide, serialized by OnceLock, with no CS_DBLCLKS.
            unsafe {
                let instance: HINSTANCE = GetModuleHandleW(None).map_err(platform_error)?.into();
                let icon = LoadIconW(Some(instance), PCWSTR(APP_ICON_RESOURCE as *const u16))
                    .or_else(|_| LoadIconW(None, IDI_APPLICATION))
                    .unwrap_or_default();
                for (name, proc, style) in [
                    (
                        w!("RemoteScreenSession"),
                        Some(window_proc as unsafe extern "system" fn(_, _, _, _) -> _),
                        CS_HREDRAW | CS_VREDRAW,
                    ),
                    (
                        w!("RemoteScreenDispatch"),
                        Some(dispatch_proc),
                        WNDCLASS_STYLES(0),
                    ),
                    (
                        w!("RemoteScreenOverlay"),
                        Some(overlay_proc),
                        WNDCLASS_STYLES(0),
                    ),
                ] {
                    let class = WNDCLASSW {
                        style,
                        lpfnWndProc: proc,
                        hInstance: instance,
                        hIcon: icon,
                        lpszClassName: name,
                        // hCursor / hbrBackground intentionally NULL.
                        ..Default::default()
                    };
                    if RegisterClassW(&class) == 0 {
                        return Err(platform_error(windows::core::Error::from_thread()));
                    }
                }
                Ok(())
            }
        })
        .clone()
}

unsafe extern "system" fn dispatch_proc(window: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if msg == CREATE {
            // SAFETY: only create_window posts CREATE with ownership of a box;
            // teardown drains anything not dispatched. This is the sole reader.
            let request = unsafe { Box::from_raw(lp.0 as *mut CreateReq) };
            let result = create_window(&request.spec, request.events);
            if let Err(mpsc::SendError(Ok(window))) = request.reply.send(result) {
                // SAFETY: caller timed out; destroy immediately on the owning
                // UI thread, rather than leave an unobserved session open.
                unsafe {
                    let _ = DestroyWindow(hwnd(window.hwnd));
                }
            }
            return LRESULT(0);
        }
        default_proc(window, msg, wp, lp)
    }));
    result.unwrap_or_else(|_| {
        error!("panic in Windows session dispatcher");
        default_proc(window, msg, wp, lp)
    })
}

unsafe extern "system" fn window_proc(window: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    catch_unwind(AssertUnwindSafe(|| window_message(window, msg, wp, lp))).unwrap_or_else(|_| {
        error!("panic in Windows session window procedure");
        default_proc(window, msg, wp, lp)
    })
}

fn window_message(window: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    if msg == WM_NCDESTROY {
        // SAFETY: take userdata exactly once and clear before dropping it; an
        // active reentrant proc retains an Rc, so it cannot access freed state.
        unsafe {
            let ptr = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut State;
            if !ptr.is_null() {
                drop(Box::from_raw(ptr));
            }
        }
        return default_proc(window, msg, wp, lp);
    }
    if msg == WM_GETMINMAXINFO {
        // SAFETY: Windows supplies a writable MINMAXINFO for this message.
        unsafe {
            let dpi = GetDpiForWindow(window).max(96);
            let mut rect = RECT {
                right: (320 * dpi / 96) as i32,
                bottom: (200 * dpi / 96) as i32,
                ..Default::default()
            };
            let style = WINDOW_STYLE(GetWindowLongPtrW(window, GWL_STYLE) as u32);
            let _ =
                AdjustWindowRectExForDpi(&mut rect, style, false, WINDOW_EX_STYLE::default(), dpi);
            let info = &mut *(lp.0 as *mut MINMAXINFO);
            info.ptMinTrackSize = POINT {
                x: rect.right - rect.left,
                y: rect.bottom - rect.top,
            };
        }
        return LRESULT(0);
    }
    let Some(s) = state(window) else {
        return default_proc(window, msg, wp, lp);
    };
    if let Some((button, down)) = win_model::button_from_message(msg, wp.0) {
        s.button(button, down, lp);
        return LRESULT(isize::from(
            (WM_XBUTTONDOWN..=WM_XBUTTONDBLCLK).contains(&msg),
        ));
    }
    if let Some(ev) = win_model::wheel_from_message(msg, wp.0) {
        s.events.scroll(ev);
        return LRESULT(0);
    }
    match msg {
        COMMANDS => {
            let commands = take(&mut *lock(&s.shared.queue));
            for command in commands {
                if !s.shared.alive.load(Ordering::Acquire) {
                    break;
                }
                s.command(command);
            }
        }
        WM_KEYDOWN | WM_KEYUP | WM_SYSKEYDOWN | WM_SYSKEYUP => {
            if let Some(key) = win_model::key_from_message(msg, wp.0, lp.0) {
                let fake = if matches!(
                    key,
                    crate::platform::KeyInput::WinKey {
                        scan: 0x1D,
                        extended: false,
                        vk: 0x11,
                        down: true,
                        ..
                    }
                ) {
                    // SAFETY: read the current timestamp before the nonremoving
                    // peek, which can update the last-retrieved message's time.
                    unsafe {
                        let time = GetMessageTime() as u32;
                        let mut next = MSG::default();
                        let peek = PeekMessageW(
                            &mut next,
                            Some(window),
                            WM_KEYFIRST,
                            WM_KEYLAST,
                            PM_NOREMOVE,
                        )
                        .as_bool()
                        .then_some((
                            next.message,
                            next.wParam.0,
                            next.lParam.0,
                            next.time,
                        ));
                        win_model::is_altgr_fake_ctrl(&key, time, peek)
                    }
                } else {
                    false
                };
                s.key(key, fake);
            }
            if !s.keyboard_capture.get() {
                return default_proc(window, msg, wp, lp);
            }
        }
        WM_CHAR | WM_SYSCHAR | WM_DEADCHAR | WM_SYSDEADCHAR => {
            if !s.keyboard_capture.get() {
                return default_proc(window, msg, wp, lp);
            }
        }
        WM_SYSCOMMAND => {
            let id = (wp.0 & 0xFFF0) as u32;
            if id < 0xF000 {
                let command = win_model::sysmenu_command(id, &s.menu.borrow());
                if let Some(command) = command {
                    s.events.window(WindowChange::Menu(command));
                }
            } else if id != SC_KEYMENU || !s.keyboard_capture.get() {
                return default_proc(window, msg, wp, lp);
            }
        }
        WM_MOUSEMOVE => {
            if !s.tracking_leave.get() {
                let mut track = TRACKMOUSEEVENT {
                    cbSize: size_of::<TRACKMOUSEEVENT>() as u32,
                    dwFlags: TME_LEAVE,
                    hwndTrack: window,
                    ..Default::default()
                };
                // SAFETY: track is a valid struct for our live window.
                s.tracking_leave
                    .set(unsafe { TrackMouseEvent(&mut track) }.is_ok());
            }
            let (px, py) = win_model::lparam_point(lp.0);
            let (x, y) = win_model::to_logical(px, py, s.dpi.get());
            s.events.pointer(PointerInput::Move {
                x,
                y,
                dx: 0.0,
                dy: 0.0,
            });
        }
        WM_MOUSELEAVE => {
            s.tracking_leave.set(false);
            if s.held.get() == 0 {
                s.events.pointer(PointerInput::Leave);
            }
        }
        WM_CAPTURECHANGED => {
            s.held.set(0);
            s.held_clicks.set([0; 5]);
        }
        WM_SETCURSOR if lp.0 as u16 as u32 == HTCLIENT => {
            s.refresh_cursor();
            return LRESULT(1);
        }
        WM_SETFOCUS | WM_KILLFOCUS => {
            let focus = msg == WM_SETFOCUS;
            s.focused.set(focus);
            if !focus {
                s.keys.borrow_mut().clear();
                s.release_clip();
            }
            update_hook(window, focus && s.system_grab.get());
            if focus {
                s.update_clip();
            }
            s.events.window(WindowChange::Focus(focus));
        }
        WM_SIZE => {
            let minimized = wp.0 == SIZE_MINIMIZED as usize;
            if s.minimized.replace(minimized) != minimized {
                s.events.window(WindowChange::Minimized(minimized));
            }
            if !minimized {
                s.resized();
            } else {
                s.release_clip();
                s.update_layers();
            }
        }
        WM_MOVE | WM_WINDOWPOSCHANGED => {
            s.update_clip();
            s.update_layers();
            // DefWindowProc emits WM_SIZE / WM_MOVE for WM_WINDOWPOSCHANGED.
            return default_proc(window, msg, wp, lp);
        }
        WM_DPICHANGED => {
            s.dpi.set((wp.0 as u16 as u32).max(96));
            // SAFETY: the system supplies this suggested RECT for WM_DPICHANGED.
            unsafe {
                let rect = *(lp.0 as *const RECT);
                let _ = SetWindowPos(
                    window,
                    None,
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_NOZORDER | SWP_NOACTIVATE,
                );
            }
            s.rebuild_cursor();
            s.rebuild_text();
            s.resized();
        }
        WM_TIMER if wp.0 == GEOMETRY_TIMER => {
            let size = s.sink.video_size();
            if s.video_size.replace(size) != size {
                s.update_clip();
                s.update_layers();
                s.refresh_cursor();
            }
        }
        WM_CLOSE => {
            s.events.window(WindowChange::CloseRequested);
        }
        WM_ERASEBKGND => return LRESULT(1),
        WM_PAINT => {
            // SAFETY: BeginPaint / EndPaint are paired; DXGI supplies the image.
            unsafe {
                let mut paint = PAINTSTRUCT::default();
                BeginPaint(window, &mut paint);
                let _ = EndPaint(window, &paint);
            }
        }
        WM_DESTROY => {
            s.destroy();
        }
        _ => return default_proc(window, msg, wp, lp),
    }
    LRESULT(0)
}

impl UiState {
    fn key(&self, key: crate::platform::KeyInput, fake: bool) {
        // Release the RefCell borrow before invoking application callbacks.
        let events = self.keys.borrow_mut().on_key(key, fake);
        for ev in events {
            self.events.key(ev);
        }
    }

    fn button(&self, button: u8, down: bool, lp: LPARAM) {
        let (px, py) = win_model::lparam_point(lp.0);
        let (x, y) = win_model::to_logical(px, py, self.dpi.get());
        let mut counts = self.held_clicks.get();
        let count;
        if down {
            count = self.clicks.borrow_mut().down(button, x, y, now_us());
            counts[button as usize] = count;
            let was_held = self.held.replace(self.held.get() | (1 << button));
            self.held_clicks.set(counts);
            if was_held == 0 {
                // SAFETY: capture belongs to this UI-thread window, for dragging
                // outside the client area until the last held button is raised.
                unsafe {
                    SetCapture(self.hwnd);
                }
            }
        } else {
            count = counts[button as usize].max(1);
            counts[button as usize] = 0;
            self.held_clicks.set(counts);
            self.held.set(self.held.get() & !(1 << button));
            if self.held.get() == 0 {
                // SAFETY: only release our own capture. This can synchronously
                // reenter WM_CAPTURECHANGED, so no mutable borrow spans it.
                unsafe {
                    if GetCapture() == self.hwnd {
                        let _ = ReleaseCapture();
                    }
                }
            }
        }
        self.events.pointer(PointerInput::Button {
            button,
            down,
            x,
            y,
            clicks: count,
        });
        if !down && self.held.get() == 0 && self.pointer_outside() {
            self.events.pointer(PointerInput::Leave);
        }
    }

    fn command(&self, command: WindowCommand) {
        match command {
            WindowCommand::SetTitle(title) => {
                let text = wide(&title);
                // SAFETY: UTF-16 storage is valid until SetWindowTextW returns.
                unsafe {
                    let _ = SetWindowTextW(self.hwnd, PCWSTR(text.as_ptr()));
                }
            }
            WindowCommand::SetFullscreen(fullscreen) => self.set_fullscreen(fullscreen),
            WindowCommand::SetKeyboardCapture(capture) => {
                self.keyboard_capture.set(capture);
                if !capture {
                    self.keys.borrow_mut().clear();
                }
            }
            WindowCommand::SetSystemKeyGrab(grab) => {
                self.system_grab.set(grab);
                update_hook(self.hwnd, grab && self.focused.get());
            }
            WindowCommand::SetPointerCapture(capture) => {
                self.pointer_capture.set(capture);
                self.update_clip();
            }
            WindowCommand::SetCursorImage(image) => {
                *self.cursor_image.borrow_mut() = image;
                self.rebuild_cursor();
            }
            WindowCommand::SetCursorHidden(hidden) => {
                self.cursor_hidden.set(hidden);
                self.refresh_cursor();
                self.update_layers();
            }
            WindowCommand::ShowRemoteCursor(show) => {
                self.show_remote.set(show);
                self.update_layers();
            }
            WindowCommand::MoveVirtualCursor { x, y } => {
                if x.is_finite() && y.is_finite() {
                    self.remote_position
                        .set((x.clamp(0.0, 1.0), y.clamp(0.0, 1.0)));
                    self.update_layers();
                }
            }
            WindowCommand::SetOverlayText(text) => {
                *self.overlay_text.borrow_mut() = text;
                self.rebuild_text();
            }
            WindowCommand::SetSessionMenu(menu) => self.set_menu(menu),
            WindowCommand::Focus => {
                // SAFETY: restore and activate our own window on its UI thread.
                unsafe {
                    if IsIconic(self.hwnd).as_bool() {
                        let _ = ShowWindow(self.hwnd, SW_RESTORE);
                    }
                    let _ = SetForegroundWindow(self.hwnd);
                }
            }
            WindowCommand::Close => {
                // SAFETY: only UI executes queued Close; state is kept alive by
                // the proc's Rc until after WM_NCDESTROY drops boxed userdata.
                unsafe {
                    let _ = DestroyWindow(self.hwnd);
                }
            }
        }
    }

    fn set_fullscreen(&self, fullscreen: bool) {
        if self.fullscreen.get() == fullscreen {
            return;
        }
        // SAFETY: live UI-thread HWND, initialized placement size, and monitor
        // rectangle. No interior borrow spans these reentrant window operations.
        unsafe {
            if fullscreen {
                let Some(rect) = monitor_rect(self.hwnd, MONITOR_DEFAULTTONEAREST) else {
                    return;
                };
                let mut placement = WINDOWPLACEMENT {
                    length: size_of::<WINDOWPLACEMENT>() as u32,
                    ..Default::default()
                };
                if GetWindowPlacement(self.hwnd, &mut placement).is_err() {
                    return;
                }
                self.placement.set(placement);
                let style = GetWindowLongPtrW(self.hwnd, GWL_STYLE);
                self.style.set(style);
                self.fullscreen.set(true);
                SetWindowLongPtrW(
                    self.hwnd,
                    GWL_STYLE,
                    (style & !(WS_OVERLAPPEDWINDOW.0 as isize)) | WS_POPUP.0 as isize,
                );
                let _ = SetWindowPos(
                    self.hwnd,
                    Some(HWND_TOP),
                    rect.left,
                    rect.top,
                    rect.right - rect.left,
                    rect.bottom - rect.top,
                    SWP_FRAMECHANGED | SWP_NOOWNERZORDER,
                );
            } else {
                self.fullscreen.set(false);
                SetWindowLongPtrW(self.hwnd, GWL_STYLE, self.style.get());
                let _ = SetWindowPlacement(self.hwnd, &self.placement.get());
                let _ = SetWindowPos(
                    self.hwnd,
                    None,
                    0,
                    0,
                    0,
                    0,
                    SWP_FRAMECHANGED | SWP_NOMOVE | SWP_NOSIZE | SWP_NOZORDER | SWP_NOOWNERZORDER,
                );
            }
        }
        self.update_clip();
        self.update_layers();
        self.events.window(WindowChange::Fullscreen(fullscreen));
    }

    fn resized(&self) {
        let Some(rect) = self.client_rect() else {
            return;
        };
        if rect.right > 0 && rect.bottom > 0 {
            self.sink.resize(rect.right as u32, rect.bottom as u32);
            let (width, height) = win_model::to_logical(rect.right, rect.bottom, self.dpi.get());
            self.events.window(WindowChange::Resized {
                width,
                height,
                scale: f64::from(self.dpi.get()) / 96.0,
            });
        }
        self.update_clip();
        self.update_layers();
    }

    fn client_rect(&self) -> Option<RECT> {
        let mut rect = RECT::default();
        // SAFETY: UI owns the HWND and rect is writable.
        unsafe { GetClientRect(self.hwnd, &mut rect) }.ok()?;
        Some(rect)
    }

    fn client_origin(&self) -> Option<POINT> {
        let mut origin = POINT::default();
        // SAFETY: UI owns the HWND and origin is writable.
        unsafe { ClientToScreen(self.hwnd, &mut origin) }
            .as_bool()
            .then_some(origin)
    }

    fn viewport(&self) -> Viewport {
        let rect = self.client_rect().unwrap_or_default();
        let (video_w, video_h) = self.sink.video_size().unwrap_or((0, 0));
        Viewport {
            view_w: f64::from(rect.right),
            view_h: f64::from(rect.bottom),
            video_w,
            video_h,
        }
    }

    fn content_rect(&self) -> RECT {
        let vp = self.viewport();
        let target = target_rect(vp.view_w as u32, vp.view_h as u32, vp.video_w, vp.video_h);
        RECT {
            left: target.left as i32,
            top: target.top as i32,
            right: target.right as i32,
            bottom: target.bottom as i32,
        }
    }

    fn update_clip(&self) {
        if self.pointer_capture.get() == PointerCapture::Free
            || !self.focused.get()
            || self.minimized.get()
        {
            self.release_clip();
            return;
        }
        let Some(origin) = self.client_origin() else {
            return;
        };
        let mut rect = self.content_rect();
        if rect.right <= rect.left || rect.bottom <= rect.top {
            self.release_clip();
            return;
        }
        rect.left += origin.x;
        rect.right += origin.x;
        rect.top += origin.y;
        rect.bottom += origin.y;
        // SAFETY: valid screen-space RECT. The owner is changed only after a
        // successful clip, so a losing window cannot undo another window's clip.
        if unsafe { ClipCursor(Some(&rect)) }.is_ok() {
            CLIP_OWNER.with(|owner| owner.set(self.hwnd.0 as isize));
        }
    }

    fn release_clip(&self) {
        CLIP_OWNER.with(|owner| {
            if owner.get() == self.hwnd.0 as isize {
                // SAFETY: only this window's clip is being released.
                unsafe {
                    let _ = ClipCursor(None);
                }
                owner.set(0);
            }
        });
    }

    fn pointer_outside(&self) -> bool {
        self.pointer_client()
            .is_none_or(|point| self.client_rect().is_none_or(|rect| !contains(rect, point)))
    }

    fn pointer_client(&self) -> Option<POINT> {
        let mut point = POINT::default();
        // SAFETY: writable POINT and our live UI-thread HWND.
        unsafe {
            GetCursorPos(&mut point).ok()?;
            if !ScreenToClient(self.hwnd, &mut point).as_bool() {
                return None;
            }
        }
        Some(point)
    }

    fn refresh_cursor(&self) {
        let Some(point) = self.pointer_client() else {
            return;
        };
        if self.client_rect().is_none_or(|rect| !contains(rect, point)) {
            return;
        }
        let Some(origin) = self.client_origin() else {
            return;
        };
        // A background session can receive cursor images while another window
        // overlaps it. Client coordinates alone cannot identify the window
        // whose cursor is actually being shown.
        // SAFETY: WindowFromPoint takes a screen-space point and returns a
        // borrowed HWND; our session and its overlays are owned by this UI.
        let target = unsafe {
            WindowFromPoint(POINT {
                x: origin.x + point.x,
                y: origin.y + point.y,
            })
        };
        if target != self.hwnd && target != self.remote.get() && target != self.overlay.get() {
            return;
        }
        let inside_video = contains(self.content_rect(), point);
        // SAFETY: cursor handle is either a borrowed shared arrow or an owned
        // HostCursor kept alive on UI. A null handle intentionally hides it.
        unsafe {
            if inside_video && self.cursor_hidden.get() {
                SetCursor(None);
            } else {
                let cursor = self.cursor.borrow().as_ref().map(|cursor| cursor.handle);
                let cursor = if inside_video {
                    cursor.unwrap_or_else(arrow)
                } else {
                    arrow()
                };
                SetCursor(Some(cursor));
            }
        }
    }

    fn rebuild_cursor(&self) {
        let image = self.cursor_image.borrow().clone();
        let new = image
            .as_ref()
            .map(|image| HostCursor::new(image, self.dpi.get()));
        let new = match new {
            Some(Ok(cursor)) => Some(Rc::new(cursor)),
            Some(Err(err)) => {
                error!(error = %err, "failed to create host cursor");
                None
            }
            None => None,
        };
        let old = self.cursor.replace(new);
        // Stop using the old cursor before destroying its handle.
        self.refresh_cursor();
        drop(old);
        self.update_layers();
    }

    fn popup(&self) -> Result<HWND, PlatformError> {
        // SAFETY: registered class; this session is the owner, not a child
        // parent. Layered transparent popups never activate or enter taskbar.
        unsafe {
            CreateWindowExW(
                WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW,
                w!("RemoteScreenOverlay"),
                w!(""),
                WS_POPUP,
                0,
                0,
                1,
                1,
                Some(self.hwnd),
                None,
                Some(GetModuleHandleW(None).map_err(platform_error)?.into()),
                None,
            )
            .map_err(platform_error)
        }
    }

    fn rebuild_text(&self) {
        let text = self.overlay_text.borrow().clone();
        if let Some(text) = text {
            let window = if self.overlay.get().is_invalid() {
                match self.popup() {
                    Ok(window) => {
                        self.overlay.set(window);
                        window
                    }
                    Err(err) => {
                        error!(error = %err, "failed to create status overlay");
                        return;
                    }
                }
            } else {
                self.overlay.get()
            };
            let text = Box::new(TextState::new(text, self.dpi.get()));
            // SAFETY: only this UI thread reads/replaces the overlay's boxed
            // TextState, and no borrow spans repaint / SetWindowPos.
            unsafe {
                let old = SetWindowLongPtrW(window, GWLP_USERDATA, Box::into_raw(text) as isize)
                    as *mut TextState;
                if !old.is_null() {
                    drop(Box::from_raw(old));
                }
                let _ = SetLayeredWindowAttributes(window, COLORREF(0), 200, LWA_ALPHA);
                let _ = InvalidateRect(Some(window), None, true);
            }
        }
        self.update_layers();
    }

    fn update_layers(&self) {
        let Some(origin) = self.client_origin() else {
            return;
        };
        let vp = self.viewport();
        let visible = !self.minimized.get() && vp.view_w > 0.0 && vp.view_h > 0.0;
        let cursor = self.cursor.borrow().clone();
        if let Some(cursor) =
            cursor.filter(|_| visible && self.show_remote.get() && !self.cursor_hidden.get())
        {
            let window = if self.remote.get().is_invalid() {
                match self.popup() {
                    Ok(window) => {
                        self.remote.set(window);
                        window
                    }
                    Err(err) => {
                        error!(error = %err, "failed to create remote cursor overlay");
                        return;
                    }
                }
            } else {
                self.remote.get()
            };
            let (nx, ny) = self.remote_position.get();
            let (x, y) = win_model::remote_cursor_origin(
                &vp,
                nx,
                ny,
                cursor.bitmap.hot_x,
                cursor.bitmap.hot_y,
            );
            let point = POINT {
                x: origin.x + x,
                y: origin.y + y,
            };
            cursor.present(window, point);
            // SAFETY: moving / showing an owned no-activate popup on UI.
            unsafe {
                let _ = SetWindowPos(
                    window,
                    None,
                    point.x,
                    point.y,
                    0,
                    0,
                    SWP_NOSIZE | SWP_NOACTIVATE | SWP_NOZORDER,
                );
                let _ = ShowWindow(window, SW_SHOWNOACTIVATE);
            }
        } else {
            // SAFETY: hiding our own popup (null handles are harmless).
            unsafe {
                let _ = ShowWindow(self.remote.get(), SW_HIDE);
            }
        }
        let overlay = self.overlay.get();
        if !overlay.is_invalid() {
            let show = visible && self.overlay_text.borrow().is_some();
            // SAFETY: overlay userdata is a live TextState owned by that HWND.
            unsafe {
                if show {
                    let text = GetWindowLongPtrW(overlay, GWLP_USERDATA) as *const TextState;
                    if let Some(text) = text.as_ref() {
                        let margin = (12 * self.dpi.get() / 96) as i32;
                        let width = text.size.cx.min((vp.view_w as i32 - margin * 2).max(1));
                        let _ = SetWindowPos(
                            overlay,
                            None,
                            origin.x + (vp.view_w as i32 - width) / 2,
                            origin.y + margin,
                            width,
                            text.size.cy,
                            SWP_NOACTIVATE | SWP_NOZORDER,
                        );
                    }
                }
                let _ = ShowWindow(overlay, if show { SW_SHOWNOACTIVATE } else { SW_HIDE });
            }
        }
    }

    fn set_menu(&self, menu: SessionMenu) {
        let items = win_model::session_sysmenu(&menu);
        *self.menu.borrow_mut() = menu;
        let mut ids = Vec::new();
        // SAFETY: system menu belongs to this window; reset releases previous
        // appended submenus. append_items transfers popup ownership on success.
        unsafe {
            GetSystemMenu(self.hwnd, true);
            let root = GetSystemMenu(self.hwnd, false);
            if let Err(err) = append_items(root, &items, &mut ids) {
                error!(error = %err, "failed to build session system menu");
            }
            let _ = DrawMenuBar(self.hwnd);
        }
        *self.sysmenu_ids.borrow_mut() = ids;
    }

    fn destroy(&self) {
        {
            let mut queue = lock(&self.shared.queue);
            self.shared.alive.store(false, Ordering::Release);
            queue.clear();
        }
        self.sink.shutdown();
        update_hook(self.hwnd, false);
        self.release_clip();
        self.keys.borrow_mut().clear();
        // SAFETY: all resources are owned by this window on the UI thread.
        // shutdown never joins the decoder; WM_NCDESTROY releases event refs.
        unsafe {
            let _ = KillTimer(Some(self.hwnd), GEOMETRY_TIMER);
            if GetCapture() == self.hwnd {
                let _ = ReleaseCapture();
            }
            for layer in [
                self.remote.replace(HWND::default()),
                self.overlay.replace(HWND::default()),
            ] {
                if !layer.is_invalid() {
                    let _ = DestroyWindow(layer);
                }
            }
            if let Some(cursor) = self.cursor.borrow().as_ref() {
                if GetCursor() == cursor.handle {
                    SetCursor(Some(arrow()));
                }
            }
        }
        self.cursor.borrow_mut().take();
    }
}

fn update_hook(window: HWND, register: bool) {
    HOOKS.with(|slot| {
        let mut hooks = slot.borrow_mut();
        let id = window.0 as isize;
        if register {
            if !hooks.windows.contains(&id) {
                hooks.windows.push(id);
            }
        } else {
            hooks.windows.retain(|&window| window != id);
        }
        // SAFETY: hook lifetime is owned by this UI TLS; no callback is invoked
        // by these registration functions. The module remains loaded for life.
        unsafe {
            if hooks.windows.is_empty() {
                if !hooks.hook.is_invalid() {
                    let _ = UnhookWindowsHookEx(hooks.hook);
                    hooks.hook = HHOOK::default();
                }
            } else if hooks.hook.is_invalid() {
                let result = GetModuleHandleW(None).and_then(|module| {
                    SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(module.into()), 0)
                });
                match result {
                    Ok(hook) => hooks.hook = hook,
                    Err(err) => error!(error = %err, "failed to install session keyboard hook"),
                }
            }
        }
    });
}

unsafe extern "system" fn keyboard_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    let result = catch_unwind(AssertUnwindSafe(|| {
        if code != HC_ACTION as i32 {
            return None;
        }
        // SAFETY: HC_ACTION supplies a KBDLLHOOKSTRUCT valid for this callback.
        let key = unsafe { &*(lp.0 as *const KBDLLHOOKSTRUCT) };
        if key.flags.0 & win_model::LLKHF_INJECTED != 0 {
            return None;
        }
        // SAFETY: querying the foreground window has no preconditions.
        let foreground = unsafe { GetForegroundWindow() };
        let registered =
            HOOKS.with(|slot| slot.borrow().windows.contains(&(foreground.0 as isize)));
        if !registered {
            return None;
        }
        let s = state(foreground)?;
        if !s.keyboard_capture.get() {
            return None;
        }
        if let Some(ev) =
            win_model::key_from_ll_hook(wp.0 as u32, key.vkCode, key.scanCode, key.flags.0)
        {
            s.key(ev, win_model::ll_is_fake_ctrl(key.scanCode));
        }
        // All physical keys go through the hook while grabbed, so there is no
        // duplicate WM_KEY delivery. Ctrl+Alt+Del and Win+L are secure sequences
        // Windows does not let us intercept; remote Ctrl+Alt+Del uses the menu.
        Some(LRESULT(1))
    }));
    match result {
        Ok(Some(result)) => result,
        Err(_) => {
            error!("panic in Windows session keyboard hook");
            next_hook(code, wp, lp)
        }
        _ => next_hook(code, wp, lp),
    }
}

fn next_hook(code: i32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: forwards the original callback parameters; hhk is ignored by API.
    unsafe { CallNextHookEx(None, code, wp, lp) }
}

struct Bitmap(HBITMAP);
impl Drop for Bitmap {
    fn drop(&mut self) {
        // SAFETY: owned bitmap is not selected in a DC when destroyed.
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

struct HostCursor {
    handle: HCURSOR,
    bitmap: CursorBitmap,
    color: Bitmap,
}

impl HostCursor {
    fn new(image: &CursorImage, dpi: u32) -> Result<Self, PlatformError> {
        let bitmap = win_model::cursor_bitmap(image, dpi as f32 / 96.0)?;
        let pixels = win_model::premultiplied(&bitmap);
        let color = dib(bitmap.width, bitmap.height, &pixels)?;
        // SAFETY: mask storage covers word-aligned monochrome scanlines; both
        // bitmaps remain live through CreateIconIndirect, which copies them.
        let handle = unsafe {
            let mask_bits =
                vec![0u8; ((bitmap.width as usize).div_ceil(16) * 2) * bitmap.height as usize];
            let mask = Bitmap(CreateBitmap(
                bitmap.width as i32,
                bitmap.height as i32,
                1,
                1,
                Some(mask_bits.as_ptr().cast()),
            ));
            if mask.0.is_invalid() {
                return Err(platform_error(windows::core::Error::from_thread()));
            }
            let icon = CreateIconIndirect(&ICONINFO {
                fIcon: false.into(),
                xHotspot: bitmap.hot_x,
                yHotspot: bitmap.hot_y,
                hbmMask: mask.0,
                hbmColor: color.0,
            })
            .map_err(platform_error)?;
            HCURSOR(icon.0)
        };
        Ok(Self {
            handle,
            bitmap,
            color,
        })
    }

    fn present(&self, window: HWND, point: POINT) {
        // SAFETY: color bitmap is owned and remains live through the call. The
        // memory DC is restored before deletion; UpdateLayeredWindow copies it.
        unsafe {
            let dc = CreateCompatibleDC(None);
            if dc.is_invalid() {
                return;
            }
            let previous = SelectObject(dc, self.color.0.into());
            let size = SIZE {
                cx: self.bitmap.width as i32,
                cy: self.bitmap.height as i32,
            };
            let source = POINT::default();
            let blend = BLENDFUNCTION {
                BlendOp: AC_SRC_OVER as u8,
                BlendFlags: 0,
                SourceConstantAlpha: 255,
                AlphaFormat: AC_SRC_ALPHA as u8,
            };
            let _ = UpdateLayeredWindow(
                window,
                None,
                Some(&point),
                Some(&size),
                Some(dc),
                Some(&source),
                COLORREF(0),
                Some(&blend),
                ULW_ALPHA,
            );
            SelectObject(dc, previous);
            let _ = DeleteDC(dc);
        }
    }
}

impl Drop for HostCursor {
    fn drop(&mut self) {
        // SAFETY: uniquely owned cursor created by CreateIconIndirect(fIcon=0).
        // An edge crossing may not have delivered another WM_SETCURSOR yet;
        // stop using the replaced cursor before destroying its handle.
        unsafe {
            if GetCursor() == self.handle {
                SetCursor(Some(arrow()));
            }
            let _ = DestroyCursor(self.handle);
        }
    }
}

fn dib(width: u32, height: u32, pixels: &[u8]) -> Result<Bitmap, PlatformError> {
    let info = BITMAPINFO {
        bmiHeader: BITMAPINFOHEADER {
            biSize: size_of::<BITMAPINFOHEADER>() as u32,
            biWidth: width as i32,
            biHeight: -(height as i32),
            biPlanes: 1,
            biBitCount: 32,
            biCompression: BI_RGB.0,
            ..Default::default()
        },
        ..Default::default()
    };
    let mut bits = std::ptr::null_mut();
    // SAFETY: dimensions were bounded by cursor_bitmap (<=256²), top-down BGRA
    // needs exactly width*height*4 bytes. GDI supplies writable DIB storage.
    unsafe {
        let bitmap = Bitmap(
            CreateDIBSection(None, &info, DIB_RGB_COLORS, &mut bits, None, 0)
                .map_err(platform_error)?,
        );
        if bits.is_null() || pixels.len() != width as usize * height as usize * 4 {
            return Err(PlatformError::Failed("invalid cursor DIB".into()));
        }
        std::ptr::copy_nonoverlapping(pixels.as_ptr(), bits.cast(), pixels.len());
        Ok(bitmap)
    }
}

struct Font(HFONT);
impl Drop for Font {
    fn drop(&mut self) {
        // SAFETY: this owned font is no longer selected in any DC.
        unsafe {
            let _ = DeleteObject(self.0.into());
        }
    }
}

struct TextState {
    text: Vec<u16>,
    font: Option<Font>,
    padding: i32,
    size: SIZE,
}

impl TextState {
    fn new(text: String, dpi: u32) -> Self {
        let padding = (12 * dpi / 96) as i32;
        // SAFETY: correctly sized NONCLIENTMETRICS / LOGFONT; all DC selections
        // are restored, and only the owned font is deleted by Font::drop.
        unsafe {
            let mut metrics = NONCLIENTMETRICSW {
                cbSize: size_of::<NONCLIENTMETRICSW>() as u32,
                ..Default::default()
            };
            let mut logfont = LOGFONTW {
                lfHeight: -(14 * dpi as i32 / 96),
                ..Default::default()
            };
            if SystemParametersInfoForDpi(
                SPI_GETNONCLIENTMETRICS.0,
                metrics.cbSize,
                Some((&mut metrics as *mut NONCLIENTMETRICSW).cast()),
                0,
                dpi,
            )
            .is_ok()
            {
                logfont = metrics.lfMessageFont;
            }
            let font = CreateFontIndirectW(&logfont);
            let font = (!font.is_invalid()).then_some(Font(font));
            let mut text: Vec<u16> = text.encode_utf16().collect();
            let dc = GetDC(None);
            let object = font
                .as_ref()
                .map(|font| font.0.into())
                .unwrap_or_else(|| GetStockObject(DEFAULT_GUI_FONT));
            let old = SelectObject(dc, object);
            let mut rect = RECT::default();
            DrawTextW(
                dc,
                &mut text,
                &mut rect,
                DT_CALCRECT | DT_SINGLELINE | DT_NOPREFIX,
            );
            SelectObject(dc, old);
            ReleaseDC(None, dc);
            Self {
                text,
                font,
                padding,
                size: SIZE {
                    cx: (rect.right - rect.left + padding * 2).max(1),
                    cy: (rect.bottom - rect.top + padding * 2).max(1),
                },
            }
        }
    }

    fn paint(&self, window: HWND) {
        // SAFETY: valid live text state, BeginPaint/EndPaint pair, temporary
        // font selection restored before returning to the message loop.
        unsafe {
            let mut paint = PAINTSTRUCT::default();
            let dc = BeginPaint(window, &mut paint);
            let mut rect = RECT::default();
            let _ = GetClientRect(window, &mut rect);
            FillRect(dc, &rect, HBRUSH(GetStockObject(BLACK_BRUSH).0));
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0x00FF_FFFF));
            let font = self
                .font
                .as_ref()
                .map(|font| font.0.into())
                .unwrap_or_else(|| GetStockObject(DEFAULT_GUI_FONT));
            let old = SelectObject(dc, font);
            rect.left += self.padding;
            rect.right -= self.padding;
            rect.top += self.padding;
            rect.bottom -= self.padding;
            let mut text = self.text.clone();
            DrawTextW(
                dc,
                &mut text,
                &mut rect,
                DT_SINGLELINE | DT_VCENTER | DT_CENTER | DT_NOPREFIX | DT_END_ELLIPSIS,
            );
            SelectObject(dc, old);
            let _ = EndPaint(window, &paint);
        }
    }
}

unsafe extern "system" fn overlay_proc(window: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    catch_unwind(AssertUnwindSafe(|| {
        match msg {
            WM_NCHITTEST => LRESULT(HTTRANSPARENT as isize),
            WM_MOUSEACTIVATE => LRESULT(MA_NOACTIVATE as isize),
            WM_ERASEBKGND => LRESULT(1),
            WM_PAINT => {
                // SAFETY: status overlays own Box<TextState> in userdata; the
                // remote cursor overlay has null userdata and is painted by ULW.
                unsafe {
                    let text = GetWindowLongPtrW(window, GWLP_USERDATA) as *const TextState;
                    if let Some(text) = text.as_ref() {
                        text.paint(window);
                    } else {
                        let mut paint = PAINTSTRUCT::default();
                        BeginPaint(window, &mut paint);
                        let _ = EndPaint(window, &paint);
                    }
                }
                LRESULT(0)
            }
            WM_NCDESTROY => {
                // SAFETY: clear userdata first, then reclaim its owned box once.
                unsafe {
                    let ptr = SetWindowLongPtrW(window, GWLP_USERDATA, 0) as *mut TextState;
                    if !ptr.is_null() {
                        drop(Box::from_raw(ptr));
                    }
                }
                default_proc(window, msg, wp, lp)
            }
            _ => default_proc(window, msg, wp, lp),
        }
    }))
    .unwrap_or_else(|_| {
        error!("panic in Windows session overlay procedure");
        default_proc(window, msg, wp, lp)
    })
}

unsafe fn append_items(
    menu: HMENU,
    items: &[WinMenuItem],
    ids: &mut Vec<u32>,
) -> windows::core::Result<()> {
    // SAFETY: menu is a live owned system menu/submenu. UTF-16 buffers live
    // through each call; AppendMenu copies text and takes ownership of popups.
    unsafe {
        for item in items {
            match item {
                WinMenuItem::Separator => AppendMenuW(menu, MF_SEPARATOR, 0, None)?,
                WinMenuItem::Command {
                    id,
                    text,
                    checked,
                    radio,
                    enabled,
                } => {
                    let text = wide(text);
                    let mut flags = MF_STRING;
                    if *checked {
                        flags |= MF_CHECKED;
                    }
                    if !*enabled {
                        flags |= MF_GRAYED;
                    }
                    AppendMenuW(menu, flags, *id as usize, PCWSTR(text.as_ptr()))?;
                    ids.push(*id);
                    if *radio {
                        let info = MENUITEMINFOW {
                            cbSize: size_of::<MENUITEMINFOW>() as u32,
                            fMask: MIIM_FTYPE,
                            fType: MFT_RADIOCHECK,
                            ..Default::default()
                        };
                        SetMenuItemInfoW(menu, *id, false, &info)?;
                    }
                }
                WinMenuItem::Submenu {
                    text,
                    items,
                    enabled,
                } => {
                    let popup = CreatePopupMenu()?;
                    if let Err(err) = append_items(popup, items, ids) {
                        let _ = DestroyMenu(popup);
                        return Err(err);
                    }
                    let text = wide(text);
                    let flags = MF_POPUP | if *enabled { MF_ENABLED } else { MF_GRAYED };
                    if let Err(err) =
                        AppendMenuW(menu, flags, popup.0 as usize, PCWSTR(text.as_ptr()))
                    {
                        let _ = DestroyMenu(popup);
                        return Err(err);
                    }
                }
            }
        }
        Ok(())
    }
}

unsafe fn monitor_rect(window: HWND, flags: MONITOR_FROM_FLAGS) -> Option<RECT> {
    // SAFETY: MONITORINFO has the required size and is writable. The monitor is
    // queried using the documented fallback for invalid / absent windows.
    unsafe {
        let monitor = MonitorFromWindow(window, flags);
        let mut info = MONITORINFO {
            cbSize: size_of::<MONITORINFO>() as u32,
            ..Default::default()
        };
        GetMonitorInfoW(monitor, &mut info)
            .as_bool()
            .then_some(info.rcMonitor)
    }
}

fn default_proc(window: HWND, msg: u32, wp: WPARAM, lp: LPARAM) -> LRESULT {
    // SAFETY: forwards the original Win32 message parameters unchanged.
    unsafe { DefWindowProcW(window, msg, wp, lp) }
}

fn arrow() -> HCURSOR {
    // SAFETY: IDC_ARROW is a shared system cursor; do not DestroyCursor it.
    unsafe { LoadCursorW(None, IDC_ARROW) }.unwrap_or_default()
}

fn hwnd(value: isize) -> HWND {
    HWND(value as *mut std::ffi::c_void)
}
fn wide(text: &str) -> Vec<u16> {
    text.encode_utf16().chain(Some(0)).collect()
}
fn contains(rect: RECT, point: POINT) -> bool {
    point.x >= rect.left && point.x < rect.right && point.y >= rect.top && point.y < rect.bottom
}
fn platform_error(err: windows::core::Error) -> PlatformError {
    PlatformError::Failed(err.to_string())
}
fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|err| err.into_inner())
}
fn now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .unwrap_or_default()
        .as_micros()
        .min(u128::from(u64::MAX)) as u64
}
