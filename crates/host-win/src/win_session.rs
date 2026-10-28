#![cfg(windows)]

//! Windows session-process assembly.
//!
//! Local-activity hooks and the clipboard message window remain on the startup
//! `Default` desktop. Local activity is not detected on a secure desktop;
//! `SessionPlatform` disables clipboard access there. Capture and the input
//! worker follow the input desktop independently.

use std::{
    ffi::OsString,
    path::{Path, PathBuf},
    sync::Arc,
    time::Duration,
};

use host_core::{
    logs,
    platform::{
        AudioCaptureConfig, AudioChunkSink, CaptureConfig, CaptureHandle, ClipboardItem,
        ConsoleState, CursorUpdate, DisplayInfo, FrameSink, HostInfo, PlatformError, Result,
    },
};
use rsp_core::{hid::InputMsg, pb};
use tokio::sync::mpsc;
use tracing::{error, info, warn};

use crate::{
    activity, audio, capture,
    clipboard::WinClipboard,
    console, desktop, displays, encode,
    inject::WinInjector,
    paths::WinPaths,
    pipe,
    power::WinPower,
    session_platform::{EncoderFactory, SessionOs, SessionPlatform},
    session_run::{self, ArgsError, RunExit, CONNECT_TIMEOUT, HANDSHAKE_TIMEOUT},
    wts,
};
use host_core::display_model;

pub const DESKTOP_POLL: Duration = Duration::from_millis(250);
pub const DISPLAY_POLL: Duration = Duration::from_secs(2);
pub const RUNTIME_THREADS: usize = 4;

pub struct WinSessionOs {
    injector: WinInjector,
    clipboard: Option<WinClipboard>,
    power: WinPower,
}

impl WinSessionOs {
    pub fn new() -> Self {
        let clipboard = match WinClipboard::start() {
            Ok(clipboard) => Some(clipboard),
            Err(error) => {
                warn!(%error, "session clipboard unavailable");
                None
            }
        };
        Self {
            injector: WinInjector::new(),
            clipboard,
            power: WinPower::new(),
        }
    }
}

impl Default for WinSessionOs {
    fn default() -> Self {
        Self::new()
    }
}

impl SessionOs for WinSessionOs {
    fn host_info(&self) -> HostInfo {
        HostInfo {
            name: std::env::var("COMPUTERNAME").unwrap_or_default(),
            os: pb::Platform::Windows,
            os_version: String::new(),
            model: String::new(),
            arch: std::env::consts::ARCH.into(),
        }
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        display_model::to_display_infos(&displays::enumerate())
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        displays::watch(tx, DISPLAY_POLL);
    }

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        capture::start(display, cfg, sink)
            .map(|capture| Box::new(capture) as Box<dyn CaptureHandle>)
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        capture::watch_cursor(tx);
    }

    fn audio_capture_supported(&self) -> bool {
        true
    }

    fn capture_audio(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        audio::start_loopback(cfg, sink)
    }

    fn attach_input_desktop(&self) -> Result<String> {
        desktop::attach_thread_to_input_desktop()
    }

    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        self.injector.inject_input(msg, display);
    }

    fn release_all(&self) {
        self.injector.release_all();
    }

    fn lock_workstation(&self) -> Result<()> {
        self.injector.lock_workstation()
    }

    fn wake_display(&self) -> Result<()> {
        self.injector.wake_display()
    }

    fn clipboard_get(&self) -> Option<ClipboardItem> {
        self.clipboard.as_ref().and_then(WinClipboard::get)
    }

    fn clipboard_set(&self, item: &ClipboardItem) -> Result<()> {
        self.clipboard
            .as_ref()
            .ok_or_else(|| PlatformError::Platform("clipboard unavailable".into()))?
            .set(item)
    }

    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        if let Some(clipboard) = &self.clipboard {
            clipboard.watch(tx);
        }
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        activity::watch(tx);
    }

    fn set_keep_awake(&self, on: bool) {
        self.power.set_keep_awake(on);
    }

    fn console_state(&self) -> ConsoleState {
        console::console_state()
    }
}

pub fn encoder_factory() -> Option<EncoderFactory> {
    Some(Arc::new(crate::encode::open_encoder))
}

pub fn session_log_dir(paths: &WinPaths, override_dir: Option<&Path>) -> PathBuf {
    override_dir.map_or_else(|| paths.logs_dir.join("session"), Path::to_path_buf)
}

/// Run the session process and return its process exit code.
pub fn main_with_args(args: impl IntoIterator<Item = OsString>) -> i32 {
    let args = match session_run::parse_args(args) {
        Ok(args) => args,
        Err(ArgsError::Help) => {
            println!("{}", session_run::usage());
            return 0;
        }
        Err(error) => {
            eprintln!("{error}\n{}", session_run::usage());
            return RunExit::BadArgs.code();
        }
    };

    let program_data = std::env::var("ProgramData").ok();
    let paths = WinPaths::from_program_data(program_data.as_deref());
    let log_dir = session_log_dir(&paths, args.log_dir.as_deref());
    let _log_guard = match std::fs::create_dir_all(&log_dir)
        .map_err(logs::LogError::from)
        .and_then(|()| logs::init(&log_dir, args.log_level.as_deref().unwrap_or("info")))
    {
        Ok(guard) => Some(guard),
        Err(error) => {
            eprintln!("session file logging unavailable: {error}");
            None
        }
    };
    let original_hook = std::panic::take_hook();
    std::panic::set_hook(Box::new(move |panic| {
        error!(panic = %panic, "session process panicked");
        original_hook(panic);
    }));

    // Must precede every window, DXGI object and hook in this process.
    displays::ensure_dpi_awareness();

    let pid = std::process::id();
    let process_session = wts::process_session(pid);
    info!(
        pid,
        session = args.session_id,
        ?process_session,
        pipe = %args.pipe,
        version = env!("CARGO_PKG_VERSION"),
        "starting session process"
    );
    if process_session != Some(args.session_id) {
        warn!(
            ?process_session,
            session = args.session_id,
            "process session differs from --session"
        );
    }

    let rt = match tokio::runtime::Builder::new_multi_thread()
        .worker_threads(RUNTIME_THREADS)
        .thread_name("rs-session")
        .enable_all()
        .build()
    {
        Ok(rt) => rt,
        Err(error) => {
            error!(%error, "failed to create session runtime");
            return RunExit::PipeError.code();
        }
    };

    let prefs = encode::EncodePrefs {
        encoder: args
            .encoder
            .clone()
            .unwrap_or_else(|| encode::EncodePrefs::from_env().encoder),
    };
    info!(encoder = ?prefs.encoder, "configured video encoder preference");
    let _ = encode::configure(prefs);
    // `warm_up` only schedules the probe on a detached worker thread, so it
    // cannot delay the named-pipe connection or consume CONNECT_TIMEOUT.
    encode::warm_up();

    // Clipboard and input workers are std threads, created outside block_on.
    let platform = match SessionPlatform::new(Arc::new(WinSessionOs::new()), encoder_factory()) {
        Ok(platform) => Arc::new(platform),
        Err(error) => {
            error!(%error, "failed to create session platform");
            rt.shutdown_timeout(Duration::from_secs(2));
            return RunExit::PipeError.code();
        }
    };
    let (dtx, drx) = mpsc::unbounded_channel();
    let watch = desktop::spawn_desktop_watch(DESKTOP_POLL, {
        let platform = Arc::clone(&platform);
        move |name: String| {
            platform.set_desktop(Some(name.clone()));
            let _ = dtx.send(name);
        }
    });

    let exit = rt.block_on(async {
        match pipe::connect_client(&args.pipe, CONNECT_TIMEOUT).await {
            Err(error) => {
                error!(%error, "failed to connect session pipe");
                RunExit::ConnectFailed
            }
            Ok(client) => {
                let end = session_run::run_session(
                    client,
                    Arc::clone(&platform),
                    drx,
                    args.session_id,
                    pid,
                    HANDSHAKE_TIMEOUT,
                )
                .await;
                info!(exit = ?end.exit, frames_in = end.frames_in, frames_out = end.frames_out, error = ?end.error, "session ServeEnd");
                end.exit
            }
        }
    });

    let stats = platform.stats();
    info!(
        dropped_inputs = stats.dropped_inputs,
        desktop_attaches = stats.desktop_attaches,
        desktop_attach_failures = stats.desktop_attach_failures,
        input_timeouts = stats.input_timeouts,
        "session stats"
    );
    drop(watch);
    drop(platform);
    rt.shutdown_timeout(Duration::from_secs(2));
    exit.code()
}
