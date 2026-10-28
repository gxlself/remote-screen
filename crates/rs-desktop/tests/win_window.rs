#![cfg(windows)]

//! Run on a logged-in interactive Windows desktop, with --test-threads=1.
//! Each test thread is its UI thread; workers never pump or touch window state.

use std::{
    mem::size_of,
    sync::{mpsc, Arc, Mutex, MutexGuard, Once},
    thread,
    time::{Duration, Instant},
};

use rs_desktop::{
    platform::{
        CursorImage, KeyInput, MenuCommand, PlatformError, PointerCapture, PointerInput,
        ScrollInput, SessionMenu, SessionWindow, StreamInfo, VideoCodec, WindowChange,
        WindowCommand, WindowEvents, WindowSpec,
    },
    win_window::{factory, WinFactory, WinWindow},
};
use windows::Win32::{
    Foundation::{HWND, LPARAM, POINT, RECT, WPARAM},
    Graphics::Gdi::{
        ClientToScreen, GetMonitorInfoW, MonitorFromWindow, MONITORINFO, MONITOR_DEFAULTTONEAREST,
    },
    UI::{
        HiDpi::{SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2},
        Input::KeyboardAndMouse::GetCapture,
        WindowsAndMessaging::*,
    },
};

static DESKTOP: Mutex<()> = Mutex::new(());
static DPI: Once = Once::new();
const TIMEOUT: Duration = Duration::from_secs(5);
const WM_MOUSELEAVE: u32 = 0x02A3; // winuser.h, no UI::Controls APIs needed.

#[derive(Debug, Clone, PartialEq)]
enum Recorded {
    Key(KeyInput),
    Pointer(PointerInput),
    Scroll(ScrollInput),
    Window(WindowChange),
}

#[derive(Default)]
struct Recorder(Mutex<Vec<Recorded>>);

impl WindowEvents for Recorder {
    fn key(&self, ev: KeyInput) {
        self.0.lock().unwrap().push(Recorded::Key(ev));
    }
    fn pointer(&self, ev: PointerInput) {
        self.0.lock().unwrap().push(Recorded::Pointer(ev));
    }
    fn scroll(&self, ev: ScrollInput) {
        self.0.lock().unwrap().push(Recorded::Scroll(ev));
    }
    fn window(&self, ev: WindowChange) {
        self.0.lock().unwrap().push(Recorded::Window(ev));
    }
}

impl Recorder {
    fn has(&self, ev: Recorded) -> bool {
        self.0.lock().unwrap().contains(&ev)
    }
    fn keys(&self) -> Vec<KeyInput> {
        self.0
            .lock()
            .unwrap()
            .iter()
            .filter_map(|ev| match ev {
                Recorded::Key(key) => Some(*key),
                _ => None,
            })
            .collect()
    }
    fn clear(&self) {
        self.0.lock().unwrap().clear();
    }
}

struct Ui {
    factory: Arc<WinFactory>,
    windows: Vec<Arc<WinWindow>>,
    _desktop: MutexGuard<'static, ()>,
}

impl Ui {
    fn new() -> Self {
        let desktop = DESKTOP.lock().unwrap_or_else(|err| err.into_inner());
        DPI.call_once(|| {
            // SAFETY: this test executable has no manifest or windows yet.
            unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) }
                .expect("per-monitor v2 DPI awareness");
        });
        let public_factory = factory().expect("real UI-thread factory");
        assert!(public_factory.screen_pixel_width() > 0);
        // Typed factory shares the same dispatcher and create path as the trait,
        // letting tests inspect WinWindow without changing the frozen trait.
        let factory = Arc::new(WinFactory::new().expect("typed factory"));
        Self {
            factory,
            windows: Vec::new(),
            _desktop: desktop,
        }
    }

    fn create(&mut self, title: &str) -> (Arc<WinWindow>, Arc<Recorder>) {
        let factory = Arc::clone(&self.factory);
        let spec = spec(title);
        let events = Arc::new(Recorder::default());
        let recorder = Arc::clone(&events);
        let (tx, rx) = mpsc::channel();
        let worker = thread::spawn(move || {
            tx.send(factory.create_window(&spec, recorder)).unwrap();
        });
        let mut result = None;
        assert!(pump_until(
            || {
                if let Ok(created) = rx.try_recv() {
                    result = Some(created);
                }
                result.is_some()
            },
            TIMEOUT
        ));
        worker.join().unwrap();
        let window = result.unwrap().unwrap();
        self.windows.push(Arc::clone(&window));
        (window, events)
    }
}

impl Drop for Ui {
    fn drop(&mut self) {
        for window in &self.windows {
            window.apply(WindowCommand::Close);
        }
        let _ = pump_until(
            || self.windows.iter().all(|window| !is_window(window)),
            TIMEOUT,
        );
    }
}

fn spec(title: &str) -> WindowSpec {
    WindowSpec {
        session_id: 1,
        host_id: "test-host".into(),
        title: title.into(),
        width: 640,
        height: 360,
    }
}

fn pump_until(mut condition: impl FnMut() -> bool, timeout: Duration) -> bool {
    let deadline = Instant::now() + timeout;
    loop {
        if condition() {
            return true;
        }
        if Instant::now() >= deadline {
            return false;
        }
        // SAFETY: this test thread owns the windows and dispatches their queue.
        // Bound a batch so condition/deadline still run if messages keep arriving.
        unsafe {
            let mut msg = MSG::default();
            for _ in 0..512 {
                if !PeekMessageW(&mut msg, None, 0, 0, PM_REMOVE).as_bool() {
                    break;
                }
                let _ = TranslateMessage(&msg);
                DispatchMessageW(&msg);
            }
        }
        thread::sleep(Duration::from_millis(1));
    }
}

fn apply_worker(window: &Arc<WinWindow>, commands: Vec<WindowCommand>) {
    let window = Arc::clone(window);
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        for command in commands {
            window.apply(command);
        }
        tx.send(()).unwrap();
    });
    // Deliberately no UI pump: apply must finish even while the UI is busy.
    rx.recv_timeout(Duration::from_secs(1))
        .expect("apply must be asynchronous");
    worker.join().unwrap();
}

fn handle(window: &WinWindow) -> HWND {
    HWND(window.hwnd() as *mut std::ffi::c_void)
}
fn is_window(window: &WinWindow) -> bool {
    // SAFETY: IsWindow accepts stale / null HWNDs without dereferencing Rust data.
    unsafe { IsWindow(Some(handle(window))).as_bool() }
}
fn send(window: &WinWindow, message: u32, wp: usize, lp: isize) -> isize {
    // SAFETY: test is on the owning UI thread; lp is either a scalar message
    // parameter or a caller-owned struct valid for this synchronous call.
    unsafe { SendMessageW(handle(window), message, Some(WPARAM(wp)), Some(LPARAM(lp))).0 }
}
fn point(x: i16, y: i16) -> isize {
    (u32::from(x as u16) | (u32::from(y as u16) << 16)) as i32 as isize
}
fn rect_tuple(rect: RECT) -> (i32, i32, i32, i32) {
    (rect.left, rect.top, rect.right, rect.bottom)
}
fn window_rect(window: &WinWindow) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: live HWND and valid output storage.
    unsafe { GetWindowRect(handle(window), &mut rect) }.unwrap();
    rect
}
fn client_rect(window: &WinWindow) -> RECT {
    let mut rect = RECT::default();
    // SAFETY: live HWND and valid output storage.
    unsafe { GetClientRect(handle(window), &mut rect) }.unwrap();
    rect
}
fn client_screen_rect(window: &WinWindow) -> RECT {
    let mut rect = client_rect(window);
    let mut origin = POINT::default();
    // SAFETY: valid client coordinate pointer and live HWND.
    assert!(unsafe { ClientToScreen(handle(window), &mut origin) }.as_bool());
    rect.left += origin.x;
    rect.right += origin.x;
    rect.top += origin.y;
    rect.bottom += origin.y;
    rect
}
fn clip_rect() -> RECT {
    let mut rect = RECT::default();
    // SAFETY: valid output storage for the desktop cursor clip rectangle.
    unsafe { GetClipCursor(&mut rect) }.unwrap();
    rect
}
fn virtual_screen() -> RECT {
    // SAFETY: system metrics have no pointer/ownership preconditions.
    unsafe {
        let x = GetSystemMetrics(SM_XVIRTUALSCREEN);
        let y = GetSystemMetrics(SM_YVIRTUALSCREEN);
        RECT {
            left: x,
            top: y,
            right: x + GetSystemMetrics(SM_CXVIRTUALSCREEN),
            bottom: y + GetSystemMetrics(SM_CYVIRTUALSCREEN),
        }
    }
}

#[test]
fn create_from_worker_thread() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("测试");
    assert!(is_window(&window));
    let state = window.debug_state();
    assert_eq!(state.title, "测试");
    let rect = client_rect(&window);
    assert_eq!(
        rect.right,
        (640.0 * f64::from(state.dpi) / 96.0).round() as i32
    );
    assert_eq!(
        rect.bottom,
        (360.0 * f64::from(state.dpi) / 96.0).round() as i32
    );
    assert!(events.has(Recorded::Window(WindowChange::Resized {
        width: f64::from(rect.right) * 96.0 / f64::from(state.dpi),
        height: f64::from(rect.bottom) * 96.0 / f64::from(state.dpi),
        scale: f64::from(state.dpi) / 96.0
    })));
    assert_eq!(
        window.frame_sink().format(),
        rs_desktop::platform::BitstreamFormat::AnnexB
    );
}

#[test]
fn factory_trait_creates_on_ui_and_worker() {
    let _ui = Ui::new();
    let factory = factory().unwrap();
    let events = Arc::new(Recorder::default());
    // UI-thread direct creation must not post then block on its own queue.
    let direct = factory.create(&spec("direct"), events.clone()).unwrap();
    direct.apply(WindowCommand::Close);
    assert!(pump_until(|| Arc::strong_count(&events) == 1, TIMEOUT));
    let copy = events.clone();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        tx.send(factory.create(&spec("trait worker"), copy))
            .unwrap();
    });
    let mut created = None;
    assert!(pump_until(
        || {
            if let Ok(result) = rx.try_recv() {
                created = Some(result);
            }
            created.is_some()
        },
        TIMEOUT
    ));
    worker.join().unwrap();
    created.unwrap().unwrap().apply(WindowCommand::Close);
    assert!(pump_until(|| Arc::strong_count(&events) == 1, TIMEOUT));
}

#[test]
fn apply_is_async_and_ordered() {
    let mut ui = Ui::new();
    let (window, _) = ui.create("初始");
    apply_worker(
        &window,
        vec![
            WindowCommand::SetTitle("一".into()),
            WindowCommand::SetTitle("二".into()),
            WindowCommand::SetOverlayText(Some("正在重连…".into())),
        ],
    );
    assert_eq!(window.debug_state().title, "初始");
    assert!(pump_until(
        || window.debug_state().overlay.as_deref() == Some("正在重连…"),
        TIMEOUT
    ));
    assert_eq!(window.debug_state().title, "二");
    apply_worker(&window, vec![WindowCommand::SetOverlayText(None)]);
    assert!(pump_until(
        || window.debug_state().overlay.is_none(),
        TIMEOUT
    ));
}

fn menu() -> SessionMenu {
    SessionMenu {
        displays: vec![(7, "主显示器".into()), (42, "第二个显示器".into())],
        current_display: Some(7),
        quality: "smooth".into(),
        combos: vec!["altTab".into(), "winKey".into()],
        ctrl_alt_del: true,
        ..Default::default()
    }
}

#[test]
fn sysmenu_from_session_menu() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("menu");
    apply_worker(&window, vec![WindowCommand::SetSessionMenu(menu())]);
    assert!(pump_until(
        || window.debug_state().sysmenu_ids.contains(&0x1410),
        TIMEOUT
    ));
    let ids = window.debug_state().sysmenu_ids;
    for id in [0x1000, 0x1200, 0x1210, 0x1030, 0x1400, 0x1410] {
        assert!(ids.contains(&id));
    }
    send(&window, WM_SYSCOMMAND, 0x1410, 0);
    assert!(events.has(Recorded::Window(WindowChange::Menu(
        MenuCommand::SelectDisplay(42)
    ))));
    // Inspect actual HMENU radio flags, not only the diagnostic projection.
    // SAFETY: system menu belongs to this live UI window and info is writable.
    unsafe {
        let root = GetSystemMenu(handle(&window), false);
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_FTYPE | MIIM_STATE,
            ..Default::default()
        };
        GetMenuItemInfoW(root, 0x1110, false, &mut info).unwrap();
        assert!(info.fType.contains(MFT_RADIOCHECK));
        assert!(info.fState.contains(MFS_CHECKED));
    }
    let mut viewer = menu();
    viewer.viewer = true;
    apply_worker(&window, vec![WindowCommand::SetSessionMenu(viewer)]);
    assert!(pump_until(
        || window.debug_state().sysmenu_ids.contains(&0x1040),
        TIMEOUT
    ));
    // SAFETY: GetMenuItemInfoW searches the menu/submenus by command ID.
    unsafe {
        let root = GetSystemMenu(handle(&window), false);
        let mut info = MENUITEMINFOW {
            cbSize: size_of::<MENUITEMINFOW>() as u32,
            fMask: MIIM_STATE,
            ..Default::default()
        };
        GetMenuItemInfoW(root, 0x1030, false, &mut info).unwrap();
        assert!(info.fState.contains(MFS_DISABLED));
    }
    let mut ids = window.debug_state().sysmenu_ids;
    let count = ids.len();
    ids.sort();
    ids.dedup();
    assert_eq!(ids.len(), count);
}

#[test]
fn key_messages_reach_events() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("keys");
    apply_worker(&window, vec![WindowCommand::SetKeyboardCapture(true)]);
    assert!(pump_until(
        || window.debug_state().keyboard_capture,
        TIMEOUT
    ));
    send(&window, WM_KEYDOWN, 0x41, 0x001E_0001);
    send(&window, WM_KEYUP, 0x41, 0xC01E_0001u32 as isize);
    assert!(events.has(Recorded::Key(KeyInput::WinKey {
        scan: 0x1E,
        extended: false,
        vk: 0x41,
        down: true,
        repeat: false
    })));
    assert_eq!(send(&window, WM_SYSKEYDOWN, 0x73, 0x203E_0001), 0);
    assert_eq!(send(&window, WM_SYSKEYUP, 0x73, 0xE03E_0001u32 as isize), 0);
    assert!(is_window(&window));
    assert!(events.has(Recorded::Key(KeyInput::WinKey {
        scan: 0x3E,
        extended: false,
        vk: 0x73,
        down: true,
        repeat: false
    })));
    for message in [WM_CHAR, WM_SYSCHAR, WM_DEADCHAR, WM_SYSDEADCHAR] {
        assert_eq!(send(&window, message, 0x41, 0), 0);
    }
    assert_eq!(send(&window, WM_SYSCOMMAND, SC_KEYMENU as usize, 0), 0);
    events.clear();
    send(&window, WM_KEYUP, 0x2C, 0xC137_0001u32 as isize);
    assert_eq!(
        events.keys(),
        vec![
            KeyInput::WinKey {
                scan: 0x37,
                extended: true,
                vk: 0x2C,
                down: true,
                repeat: false
            },
            KeyInput::WinKey {
                scan: 0x37,
                extended: true,
                vk: 0x2C,
                down: false,
                repeat: false
            },
        ]
    );
    apply_worker(&window, vec![WindowCommand::SetKeyboardCapture(false)]);
    assert!(pump_until(
        || !window.debug_state().keyboard_capture,
        TIMEOUT
    ));
    events.clear();
    send(&window, WM_KEYDOWN, 0x42, 0x0030_0001);
    assert!(events.has(Recorded::Key(KeyInput::WinKey {
        scan: 0x30,
        extended: false,
        vk: 0x42,
        down: true,
        repeat: false
    })));
}

#[test]
fn mouse_messages_reach_events() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("mouse");
    let scale = f64::from(window.debug_state().dpi) / 96.0;
    let x = 100.0 / scale;
    let y = 50.0 / scale;
    send(&window, WM_LBUTTONDOWN, 1, point(100, 50));
    // SAFETY: querying this UI thread's capture handle has no preconditions.
    assert_eq!(unsafe { GetCapture() }, handle(&window));
    send(&window, WM_MOUSELEAVE, 0, 0);
    assert!(!events.has(Recorded::Pointer(PointerInput::Leave)));
    send(&window, WM_LBUTTONUP, 0, point(100, 50));
    send(&window, WM_LBUTTONDOWN, 1, point(100, 50));
    send(&window, WM_LBUTTONUP, 0, point(100, 50));
    for clicks in [1, 2] {
        for down in [true, false] {
            assert!(events.has(Recorded::Pointer(PointerInput::Button {
                button: 0,
                down,
                x,
                y,
                clicks
            })));
        }
    }
    for (high, button) in [(1, 3), (2, 4)] {
        assert_eq!(send(&window, WM_XBUTTONDOWN, high << 16, point(100, 50)), 1);
        send(&window, WM_XBUTTONUP, high << 16, point(100, 50));
        assert!(events.has(Recorded::Pointer(PointerInput::Button {
            button,
            down: true,
            x,
            y,
            clicks: 1
        })));
    }
    send(&window, WM_MOUSEWHEEL, 120 << 16, 0);
    send(&window, WM_MOUSEHWHEEL, (0xFF88u32 << 16) as usize, 0);
    assert!(events.has(Recorded::Scroll(ScrollInput::WinWheel {
        delta: 120,
        horizontal: false
    })));
    assert!(events.has(Recorded::Scroll(ScrollInput::WinWheel {
        delta: -120,
        horizontal: true
    })));
    send(&window, WM_MOUSEMOVE, 0, point(-10, 80));
    assert!(events.has(Recorded::Pointer(PointerInput::Move {
        x: -10.0 / scale,
        y: 80.0 / scale,
        dx: 0.0,
        dy: 0.0
    })));
    send(&window, WM_MOUSELEAVE, 0, 0);
    assert!(events.has(Recorded::Pointer(PointerInput::Leave)));
}

#[test]
fn close_request_then_close() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("close");
    send(&window, WM_CLOSE, 0, 0);
    assert!(events.has(Recorded::Window(WindowChange::CloseRequested)));
    assert!(is_window(&window));
    apply_worker(
        &window,
        vec![
            WindowCommand::Close,
            WindowCommand::SetTitle("discarded".into()),
        ],
    );
    assert!(pump_until(|| !is_window(&window), TIMEOUT));
    assert_eq!(Arc::strong_count(&events), 1);
    window.apply(WindowCommand::SetTitle("after close".into()));
}

#[test]
fn fullscreen_roundtrip() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("fullscreen");
    let original = window_rect(&window);
    let mut monitor = MONITORINFO {
        cbSize: size_of::<MONITORINFO>() as u32,
        ..Default::default()
    };
    // SAFETY: live HWND, monitor fallback, and correctly sized output struct.
    assert!(unsafe {
        GetMonitorInfoW(
            MonitorFromWindow(handle(&window), MONITOR_DEFAULTTONEAREST),
            &mut monitor,
        )
    }
    .as_bool());
    apply_worker(&window, vec![WindowCommand::SetFullscreen(true)]);
    assert!(pump_until(
        || events.has(Recorded::Window(WindowChange::Fullscreen(true))),
        TIMEOUT
    ));
    assert_eq!(
        rect_tuple(window_rect(&window)),
        rect_tuple(monitor.rcMonitor)
    );
    apply_worker(&window, vec![WindowCommand::SetFullscreen(false)]);
    assert!(pump_until(
        || events.has(Recorded::Window(WindowChange::Fullscreen(false))),
        TIMEOUT
    ));
    assert_eq!(rect_tuple(window_rect(&window)), rect_tuple(original));
}

#[test]
fn grab_hook_lifecycle() {
    let mut ui = Ui::new();
    let (first, _) = ui.create("hook 1");
    let (second, _) = ui.create("hook 2");
    send(&first, WM_SETFOCUS, 0, 0);
    apply_worker(&first, vec![WindowCommand::SetSystemKeyGrab(true)]);
    assert!(pump_until(
        || first.debug_state().grab_hook_installed,
        TIMEOUT
    ));
    apply_worker(&first, vec![WindowCommand::SetSystemKeyGrab(false)]);
    assert!(pump_until(
        || !first.debug_state().grab_hook_installed,
        TIMEOUT
    ));
    apply_worker(&first, vec![WindowCommand::SetSystemKeyGrab(true)]);
    assert!(pump_until(
        || first.debug_state().grab_hook_installed,
        TIMEOUT
    ));
    send(&first, WM_KILLFOCUS, 0, 0);
    assert!(!first.debug_state().grab_hook_installed);
    send(&first, WM_SETFOCUS, 0, 0);
    send(&second, WM_SETFOCUS, 0, 0);
    apply_worker(&second, vec![WindowCommand::SetSystemKeyGrab(true)]);
    assert!(pump_until(
        || first.debug_state().grab_hook_installed && second.debug_state().grab_hook_installed,
        TIMEOUT
    ));
    apply_worker(&first, vec![WindowCommand::SetSystemKeyGrab(false)]);
    assert!(pump_until(
        || !first.debug_state().grab_hook_installed,
        TIMEOUT
    ));
    assert!(second.debug_state().grab_hook_installed);
    second.apply(WindowCommand::Close);
    assert!(pump_until(|| !is_window(&second), TIMEOUT));
}

#[test]
fn confine_cursor() {
    let mut ui = Ui::new();
    let (window, _) = ui.create("clip");
    send(&window, WM_SETFOCUS, 0, 0);
    apply_worker(
        &window,
        vec![WindowCommand::SetPointerCapture(PointerCapture::Confine)],
    );
    assert!(pump_until(|| window.debug_state().clip.is_some(), TIMEOUT));
    assert_eq!(
        rect_tuple(clip_rect()),
        rect_tuple(client_screen_rect(&window))
    );
    apply_worker(
        &window,
        vec![WindowCommand::SetPointerCapture(PointerCapture::Free)],
    );
    assert!(pump_until(|| window.debug_state().clip.is_none(), TIMEOUT));
    assert_eq!(rect_tuple(clip_rect()), rect_tuple(virtual_screen()));
    apply_worker(
        &window,
        vec![WindowCommand::SetPointerCapture(PointerCapture::Virtual)],
    );
    assert!(pump_until(|| window.debug_state().clip.is_some(), TIMEOUT));
    // Stream dimensions arrive after creation without a WM_SIZE. Confinement
    // must converge to the letterboxed video region when they become known.
    let client = client_screen_rect(&window);
    let height = client.bottom - client.top;
    let width = f64::from(height) * 4.0 / 3.0;
    let left = (f64::from(client.right - client.left) - width) / 2.0;
    let expected = (
        client.left + left.round() as i32,
        client.top,
        client.left + (left + width).round() as i32,
        client.bottom,
    );
    window.frame_sink().configure(&StreamInfo {
        codec: VideoCodec::H264,
        width: 640,
        height: 480,
        display_id: 1,
    });
    assert!(pump_until(|| rect_tuple(clip_rect()) == expected, TIMEOUT));
    send(&window, WM_KILLFOCUS, 0, 0);
    assert_eq!(rect_tuple(clip_rect()), rect_tuple(virtual_screen()));
    send(&window, WM_SETFOCUS, 0, 0);
    assert!(window.debug_state().clip.is_some());
    window.apply(WindowCommand::Close);
    assert!(pump_until(|| !is_window(&window), TIMEOUT));
    assert_eq!(rect_tuple(clip_rect()), rect_tuple(virtual_screen()));
}

fn cursor_image() -> CursorImage {
    let mut png = Vec::new();
    {
        let mut encoder = png::Encoder::new(&mut png, 32, 32);
        encoder.set_color(png::ColorType::Rgba);
        encoder.set_depth(png::BitDepth::Eight);
        let mut writer = encoder.write_header().unwrap();
        writer
            .write_image_data(&[255, 64, 32, 128].repeat(32 * 32))
            .unwrap();
        writer.finish().unwrap();
    }
    CursorImage {
        id: 1,
        width: 32,
        height: 32,
        hot_x: 3,
        hot_y: 4,
        scale: 1.0,
        png,
    }
}

#[test]
fn cursor_image_scaled() {
    let mut ui = Ui::new();
    let (window, _) = ui.create("cursor");
    let side = (32.0 * f64::from(window.debug_state().dpi) / 96.0).round() as u32;
    apply_worker(
        &window,
        vec![
            WindowCommand::SetCursorImage(Some(cursor_image())),
            WindowCommand::MoveVirtualCursor { x: 0.5, y: 0.5 },
            WindowCommand::ShowRemoteCursor(true),
        ],
    );
    assert!(pump_until(
        || window.debug_state().cursor_size == Some((side, side))
            && window.debug_state().remote_cursor_visible,
        TIMEOUT
    ));
    apply_worker(&window, vec![WindowCommand::SetCursorHidden(true)]);
    assert!(pump_until(
        || !window.debug_state().remote_cursor_visible,
        TIMEOUT
    ));
    apply_worker(&window, vec![WindowCommand::SetCursorHidden(false)]);
    assert!(pump_until(
        || window.debug_state().remote_cursor_visible,
        TIMEOUT
    ));
    apply_worker(&window, vec![WindowCommand::SetCursorImage(None)]);
    assert!(pump_until(
        || window.debug_state().cursor_size.is_none()
            && !window.debug_state().remote_cursor_visible,
        TIMEOUT
    ));
}

#[test]
fn three_windows_three_threads() {
    let mut ui = Ui::new();
    let (tx, rx) = mpsc::channel();
    let mut events = Vec::new();
    let mut workers = Vec::new();
    for i in 0..3 {
        let recorder = Arc::new(Recorder::default());
        events.push(recorder.clone());
        let factory = ui.factory.clone();
        let tx = tx.clone();
        workers.push(thread::spawn(move || {
            tx.send((
                i,
                factory.create_window(&spec(&format!("window {i}")), recorder),
            ))
            .unwrap();
        }));
    }
    drop(tx);
    let mut created = Vec::new();
    assert!(pump_until(
        || {
            while let Ok(result) = rx.try_recv() {
                created.push(result);
            }
            created.len() == 3
        },
        TIMEOUT
    ));
    for worker in workers {
        worker.join().unwrap();
    }
    for (i, window) in created {
        let window = window.unwrap();
        assert_eq!(window.debug_state().title, format!("window {i}"));
        assert!(is_window(&window));
        ui.windows.push(window);
    }
    for window in &ui.windows {
        window.apply(WindowCommand::Close);
    }
    assert!(pump_until(
        || ui.windows.iter().all(|window| !is_window(window)),
        TIMEOUT
    ));
    for recorder in events {
        assert_eq!(Arc::strong_count(&recorder), 1);
    }
}

#[test]
fn timed_out_create_destroys_late_window() {
    let ui = Ui::new();
    let factory = ui.factory.clone();
    let events = Arc::new(Recorder::default());
    let copy = events.clone();
    let (tx, rx) = mpsc::channel();
    let worker = thread::spawn(move || {
        tx.send(factory.create_window(&spec("late create"), copy))
            .unwrap();
    });
    // Do not pump until the caller times out; its request still owns events.
    let result = rx.recv_timeout(Duration::from_secs(8)).unwrap();
    assert!(
        matches!(result, Err(PlatformError::Failed(ref reason)) if reason == "timed out creating session window")
    );
    worker.join().unwrap();
    assert_eq!(Arc::strong_count(&events), 2);
    assert!(pump_until(|| Arc::strong_count(&events) == 1, TIMEOUT));
    assert!(events.0.lock().unwrap().iter().any(|event| matches!(
        event,
        Recorded::Window(WindowChange::Resized { width, height, scale })
            if *width > 0.0 && *height > 0.0 && *scale >= 1.0
    )));
}

#[test]
fn dropping_last_window_handle_posts_close() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("drop");
    let raw = window.hwnd();
    ui.windows.clear();
    thread::spawn(move || drop(window)).join().unwrap();
    assert!(pump_until(
        || {
            // SAFETY: IsWindow accepts an HWND whose native window may be gone.
            !unsafe { IsWindow(Some(HWND(raw as *mut std::ffi::c_void))) }.as_bool()
        },
        TIMEOUT
    ));
    assert_eq!(Arc::strong_count(&events), 1);
}

fn owned_overlays(window: &WinWindow) -> Vec<HWND> {
    unsafe extern "system" fn collect(window: HWND, param: LPARAM) -> windows::core::BOOL {
        // SAFETY: EnumThreadWindows invokes this synchronously with the mutable
        // tuple below, on its owning UI thread. We retain only integer handles.
        unsafe {
            let (owner, windows) = &mut *(param.0 as *mut (HWND, Vec<HWND>));
            if GetWindow(window, GW_OWNER).ok() == Some(*owner) {
                windows.push(window);
            }
        }
        true.into()
    }
    let mut payload = (handle(window), Vec::new());
    // SAFETY: payload outlives synchronous enumeration of this UI thread.
    unsafe {
        let _ = EnumThreadWindows(
            windows::Win32::System::Threading::GetCurrentThreadId(),
            Some(collect),
            LPARAM((&mut payload as *mut (HWND, Vec<HWND>)) as isize),
        );
    }
    payload.1
}

#[test]
fn dpi_minimize_and_overlay_lifecycle() {
    let mut ui = Ui::new();
    let (window, events) = ui.create("layers and DPI");
    apply_worker(
        &window,
        vec![
            WindowCommand::SetCursorImage(Some(cursor_image())),
            WindowCommand::ShowRemoteCursor(true),
            WindowCommand::SetOverlayText(Some("连接中断，正在重连…".into())),
        ],
    );
    assert!(pump_until(
        || window.debug_state().remote_cursor_visible && window.debug_state().overlay.is_some(),
        TIMEOUT
    ));
    let layers = owned_overlays(&window);
    assert_eq!(layers.len(), 2);
    for layer in &layers {
        // SAFETY: this thread owns both no-activate overlay windows.
        unsafe {
            let styles = WINDOW_EX_STYLE(GetWindowLongPtrW(*layer, GWL_EXSTYLE) as u32);
            assert!(styles
                .contains(WS_EX_LAYERED | WS_EX_TRANSPARENT | WS_EX_NOACTIVATE | WS_EX_TOOLWINDOW));
            assert_eq!(
                SendMessageW(*layer, WM_NCHITTEST, None, None).0,
                HTTRANSPARENT as isize
            );
        }
    }
    send(&window, WM_SIZE, SIZE_MINIMIZED as usize, 0);
    assert!(events.has(Recorded::Window(WindowChange::Minimized(true))));
    assert!(!window.debug_state().remote_cursor_visible);
    for layer in &layers {
        // SAFETY: querying visibility of a live UI-thread popup.
        assert!(!unsafe { IsWindowVisible(*layer) }.as_bool());
    }
    send(&window, WM_SIZE, SIZE_RESTORED as usize, 0);
    assert!(events.has(Recorded::Window(WindowChange::Minimized(false))));
    assert!(window.debug_state().remote_cursor_visible);
    let suggested = window_rect(&window);
    send(
        &window,
        WM_DPICHANGED,
        192 | (192 << 16),
        (&suggested as *const RECT) as isize,
    );
    assert_eq!(window.debug_state().dpi, 192);
    assert_eq!(window.debug_state().cursor_size, Some((64, 64)));
    assert!(window.debug_state().remote_cursor_visible);
    window.apply(WindowCommand::Close);
    assert!(pump_until(|| !is_window(&window), TIMEOUT));
    for layer in layers {
        // SAFETY: IsWindow accepts handles after their owner was destroyed.
        assert!(!unsafe { IsWindow(Some(layer)) }.as_bool());
    }
    assert_eq!(Arc::strong_count(&events), 1);
}

#[test]
fn background_cursor_does_not_replace_front_window_cursor() {
    struct RestorePointer(POINT);
    impl Drop for RestorePointer {
        fn drop(&mut self) {
            // SAFETY: restore the desktop pointer changed by this test.
            unsafe {
                let _ = SetCursorPos(self.0.x, self.0.y);
            }
        }
    }
    let mut ui = Ui::new();
    let (back, _) = ui.create("background cursor");
    let (front, _) = ui.create("foreground cursor");
    let rect = window_rect(&back);
    let mut old = POINT::default();
    // SAFETY: live windows owned by this UI thread and a writable POINT.
    unsafe {
        GetCursorPos(&mut old).unwrap();
        SetWindowPos(
            handle(&front),
            Some(HWND_TOP),
            rect.left,
            rect.top,
            rect.right - rect.left,
            rect.bottom - rect.top,
            SET_WINDOW_POS_FLAGS::default(),
        )
        .unwrap();
    }
    let _restore = RestorePointer(old);
    apply_worker(
        &front,
        vec![WindowCommand::SetCursorImage(Some(cursor_image()))],
    );
    assert!(pump_until(
        || front.debug_state().cursor_size.is_some(),
        TIMEOUT
    ));
    let client = client_screen_rect(&front);
    // SAFETY: position the pointer inside the two overlapping client areas.
    unsafe {
        SetCursorPos(
            (client.left + client.right) / 2,
            (client.top + client.bottom) / 2,
        )
    }
    .unwrap();
    let _ = pump_until(|| false, Duration::from_millis(20));
    send(
        &front,
        WM_SETCURSOR,
        front.hwnd() as usize,
        HTCLIENT as isize,
    );
    // SAFETY: these are borrowed cursor handles; the test does not destroy them.
    let current = unsafe {
        let current = GetCursor();
        assert_ne!(current, LoadCursorW(None, IDC_ARROW).unwrap());
        current
    };
    apply_worker(
        &back,
        vec![WindowCommand::SetCursorImage(Some(cursor_image()))],
    );
    assert!(pump_until(
        || back.debug_state().cursor_size.is_some(),
        TIMEOUT
    ));
    // SAFETY: query the cursor currently shown by this UI thread.
    assert_eq!(unsafe { GetCursor() }, current);
}
