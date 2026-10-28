//! Windows low-level keyboard/mouse activity watcher.

use std::cell::OnceCell;
use std::sync::atomic::{AtomicBool, Ordering};
use std::sync::mpsc::{self, SyncSender};
use std::sync::{Mutex, MutexGuard, OnceLock};
use std::thread;
use std::time::Instant;

use tokio::sync::mpsc::{error::TrySendError, Sender};
use tracing::warn;
use windows::core::{Error, Owned};
use windows::Win32::Foundation::{LPARAM, LRESULT, WPARAM};
use windows::Win32::System::LibraryLoader::GetModuleHandleW;
use windows::Win32::UI::WindowsAndMessaging::{
    CallNextHookEx, GetMessageW, KillTimer, SetTimer, SetWindowsHookExW, HHOOK, KBDLLHOOKSTRUCT,
    MSG, MSLLHOOKSTRUCT, WH_KEYBOARD_LL, WH_MOUSE_LL, WM_TIMER,
};

use host_core::activity_model::{keyboard_is_ours, mouse_is_ours, ActivityState};

static WATCHERS: OnceLock<Mutex<Vec<Sender<bool>>>> = OnceLock::new();
// Read and written only while holding WATCHERS, including during teardown.
static RUNNING: AtomicBool = AtomicBool::new(false);

thread_local! {
    // Low-level hooks run on their installing thread. Initialize the channel
    // before installing hooks, so callbacks only perform a lookup and try_send.
    static EVENTS: OnceCell<SyncSender<()>> = const { OnceCell::new() };
}

fn watchers() -> MutexGuard<'static, Vec<Sender<bool>>> {
    WATCHERS
        .get_or_init(|| Mutex::new(Vec::new()))
        .lock()
        .unwrap_or_else(|poisoned| poisoned.into_inner())
}

/// Subscribe to local input, sharing one hook thread with all subscribers.
///
/// The hook thread must be on the input desktop. The session process owns desktop selection
/// and switching; hooks need to be rebuilt when that input desktop changes.
pub fn watch(tx: Sender<bool>) {
    let mut senders = watchers();
    senders.retain(|sender| !sender.is_closed());
    if tx.is_closed() {
        return;
    }
    senders.push(tx);
    if RUNNING.load(Ordering::Relaxed) {
        return;
    }

    RUNNING.store(true, Ordering::Relaxed);
    if let Err(error) = thread::Builder::new()
        .name("rs-local-activity".to_owned())
        .spawn(run)
    {
        RUNNING.store(false, Ordering::Relaxed);
        senders.clear();
        warn!(%error, "failed to start the Windows local activity watcher");
    }
}

unsafe extern "system" fn keyboard_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: Windows supplies a valid KBDLLHOOKSTRUCT for nonnegative
        // low-level keyboard hook codes, valid until this callback returns.
        let event = unsafe { &*(lparam.0 as *const KBDLLHOOKSTRUCT) };
        if !keyboard_is_ours(event.flags.0, event.dwExtraInfo) {
            notify_event();
        }
    }
    // SAFETY: forward every event with the original arguments; the optional
    // hook handle is ignored by CallNextHookEx for low-level hooks.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

unsafe extern "system" fn mouse_hook(code: i32, wparam: WPARAM, lparam: LPARAM) -> LRESULT {
    if code >= 0 {
        // SAFETY: Windows supplies a valid MSLLHOOKSTRUCT for nonnegative
        // low-level mouse hook codes, valid until this callback returns.
        let event = unsafe { &*(lparam.0 as *const MSLLHOOKSTRUCT) };
        if !mouse_is_ours(event.flags, event.dwExtraInfo) {
            notify_event();
        }
    }
    // SAFETY: forward every event with the original arguments, retaining
    // none of the hook data.
    unsafe { CallNextHookEx(None, code, wparam, lparam) }
}

fn notify_event() {
    EVENTS.with(|events| {
        if let Some(sender) = events.get() {
            let _ = sender.try_send(());
        }
    });
}

struct Hooks {
    _keyboard: Owned<HHOOK>,
    _mouse: Owned<HHOOK>,
    timer: Timer,
}

impl Hooks {
    fn install() -> windows::core::Result<Self> {
        // SAFETY: None requests the current process's module, which remains
        // loaded while the hook functions are installed.
        let instance = unsafe { GetModuleHandleW(None)? }.into();
        // SAFETY: callback has the Win32 ABI, lives for the process lifetime,
        // and the installing thread pumps messages until the hooks are dropped.
        let keyboard =
            unsafe { SetWindowsHookExW(WH_KEYBOARD_LL, Some(keyboard_hook), Some(instance), 0)? };
        // SAFETY: keyboard is uniquely owned and Owned unhooks it on drop,
        // including if installing the second hook or timer fails.
        let keyboard = unsafe { Owned::new(keyboard) };
        // SAFETY: callback and module outlive the hook; this same thread pumps
        // the message queue and owns the returned hook handle.
        let mouse = unsafe { SetWindowsHookExW(WH_MOUSE_LL, Some(mouse_hook), Some(instance), 0)? };
        // SAFETY: mouse is uniquely owned and must be unhooked on drop.
        let mouse = unsafe { Owned::new(mouse) };
        // SAFETY: a windowless timer posts WM_TIMER to this thread's queue;
        // its lifetime is owned by Timer on this same thread.
        let timer = unsafe { SetTimer(None, 0, 250, None) };
        if timer == 0 {
            return Err(Error::from_thread());
        }

        Ok(Self {
            _keyboard: keyboard,
            _mouse: mouse,
            timer: Timer(timer),
        })
    }
}

struct Timer(usize);

impl Drop for Timer {
    fn drop(&mut self) {
        // SAFETY: this windowless timer belongs to this thread and is killed
        // before the hook thread exits.
        let _ = unsafe { KillTimer(None, self.0) };
    }
}

fn run() {
    let (event_tx, event_rx) = mpsc::sync_channel(128);
    EVENTS.with(|events| {
        let _ = events.set(event_tx);
    });
    let hooks = match Hooks::install() {
        Ok(hooks) => hooks,
        Err(error) => {
            let mut senders = watchers();
            senders.clear();
            RUNNING.store(false, Ordering::Relaxed);
            warn!(%error, "failed to install Windows local activity hooks");
            return;
        }
    };

    let origin = Instant::now();
    let mut state = ActivityState::default();
    let mut message = MSG::default();
    loop {
        // SAFETY: message is a valid output buffer; None receives this
        // thread's messages, including hook callbacks and the timer.
        let result = unsafe { GetMessageW(&mut message, None, 0, 0) }.0;
        if result <= 0 {
            let error = (result < 0).then(Error::from_thread);
            // Hold the watcher lock through teardown, so a concurrent watch
            // cannot attach to a worker which is about to exit.
            let mut senders = watchers();
            drop(hooks);
            senders.clear();
            RUNNING.store(false, Ordering::Relaxed);
            if let Some(error) = error {
                warn!(%error, "Windows local activity message loop failed");
            }
            return;
        }
        if message.message != WM_TIMER || message.wParam.0 != hooks.timer.0 {
            continue;
        }

        let now = origin.elapsed();
        let mut senders = watchers();
        for _ in event_rx.try_iter() {
            if let Some(active) = state.on_event(now) {
                publish(&mut senders, active);
            }
        }
        if let Some(active) = state.on_tick(now) {
            publish(&mut senders, active);
        }
        senders.retain(|sender| !sender.is_closed());
        if senders.is_empty() {
            // Keep the lock until both hooks and the timer are gone. The next
            // subscriber then sees RUNNING=false and creates a fresh thread.
            drop(hooks);
            RUNNING.store(false, Ordering::Relaxed);
            return;
        }
    }
}

fn publish(senders: &mut Vec<Sender<bool>>, active: bool) {
    senders.retain(|sender| !matches!(sender.try_send(active), Err(TrySendError::Closed(_))));
}
