//! Synthetic capture, Annex-B-shaped output and recorded input.

use std::collections::HashMap;
use std::path::PathBuf;
use std::sync::atomic::{AtomicBool, AtomicU64, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_core::pipeline::capture::dimensions;
use host_core::platform::*;
use rsp_core::{clock::unix_micros, pb};
use tokio::sync::mpsc;
use tokio::task::JoinHandle;

#[derive(Default)]
struct MemoryKeyStore(Mutex<HashMap<String, Vec<u8>>>);

impl KeyStore for MemoryKeyStore {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        Ok(self
            .0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .get(name)
            .cloned())
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .insert(name.to_owned(), value.to_vec());
        Ok(())
    }

    fn delete(&self, name: &str) -> Result<()> {
        self.0
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .remove(name);
        Ok(())
    }
}

struct DirectoryKeyStore {
    dir: PathBuf,
}

impl KeyStore for DirectoryKeyStore {
    fn load(&self, name: &str) -> Result<Option<Vec<u8>>> {
        match std::fs::read(self.dir.join(name)) {
            Ok(value) => Ok(Some(value)),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(None),
            Err(error) => Err(PlatformError::Io(error)),
        }
    }

    fn store(&self, name: &str, value: &[u8]) -> Result<()> {
        std::fs::create_dir_all(&self.dir).map_err(PlatformError::Io)?;
        #[cfg(unix)]
        {
            use std::os::unix::fs::PermissionsExt;
            std::fs::set_permissions(&self.dir, std::fs::Permissions::from_mode(0o700))?;
        }
        let path = self.dir.join(name);
        let tmp = self.dir.join(format!(".{name}.tmp"));
        let mut options = std::fs::OpenOptions::new();
        options.create(true).truncate(true).write(true);
        #[cfg(unix)]
        {
            use std::os::unix::fs::OpenOptionsExt;
            options.mode(0o600);
        }
        let mut file = options.open(&tmp)?;
        std::io::Write::write_all(&mut file, value)?;
        file.sync_all()?;
        std::fs::rename(tmp, path).map_err(PlatformError::Io)
    }

    fn delete(&self, name: &str) -> Result<()> {
        match std::fs::remove_file(self.dir.join(name)) {
            Ok(()) => Ok(()),
            Err(error) if error.kind() == std::io::ErrorKind::NotFound => Ok(()),
            Err(error) => Err(PlatformError::Io(error)),
        }
    }
}

/// Parameters of a successfully created synthetic encoder.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct EncoderStart {
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_kbps: u32,
    pub codec: pb::Codec,
}

struct State {
    events: Mutex<Vec<InputEvent>>,
    msgs: Mutex<Vec<(InputMsg, u32)>>,
    local_watchers: Mutex<Vec<mpsc::Sender<bool>>>,
    clipboard_watchers: Mutex<Vec<mpsc::Sender<()>>>,
    display_watchers: Mutex<Vec<mpsc::Sender<Vec<DisplayInfo>>>>,
    cursor_watchers: Mutex<Vec<mpsc::Sender<CursorUpdate>>>,
    clipboard: Mutex<Option<ClipboardItem>>,
    clipboard_writes: Mutex<Vec<ClipboardItem>>,
    displays: Mutex<Vec<DisplayInfo>>,
    capture_displays: Mutex<Vec<u32>>,
    console_state: Mutex<ConsoleState>,
    thumbnail_jpeg: Mutex<Option<Vec<u8>>>,
    thumbnail_jpeg_calls: AtomicUsize,
    filevault: Mutex<Option<bool>>,
    keep_awake_calls: Mutex<Vec<bool>>,
    session_actions: Mutex<Vec<SessionActionKind>>,
    fail_session_actions: AtomicBool,
    wol_sends: Mutex<Vec<([u8; 6], String)>>,
    encoder_rates: Mutex<Vec<(u32, u32)>>,
    forced_keyframes: AtomicU64,
    encoders_started: Mutex<Vec<EncoderStart>>,
    hevc_unsupported: AtomicBool,
    capture_paused: AtomicBool,
    releases: AtomicU64,
    captures: AtomicUsize,
    keys: MemoryKeyStore,
    directory_keys: Option<DirectoryKeyStore>,
    nics: Vec<Nic>,
    audio: crate::audio::FakeAudio,
    viewport: crate::viewport::FakeViewport,
    system_auth: Mutex<Option<Arc<dyn host_core::sysauth::SystemAuthenticator>>>,
    vdisplays: Mutex<Option<Arc<dyn host_core::vdisplay::VirtualDisplayBackend>>>,
}

impl Default for State {
    fn default() -> Self {
        Self {
            events: Mutex::default(),
            msgs: Mutex::default(),
            local_watchers: Mutex::default(),
            clipboard_watchers: Mutex::default(),
            display_watchers: Mutex::default(),
            cursor_watchers: Mutex::default(),
            clipboard: Mutex::default(),
            clipboard_writes: Mutex::default(),
            displays: Mutex::new(vec![DisplayInfo {
                id: 1,
                name: "Synthetic display".into(),
                width_px: 1920,
                height_px: 1080,
                scale: 1.0,
                x: 0,
                y: 0,
                refresh_hz: 60,
                primary: true,
                rotation: 0,
            }]),
            capture_displays: Mutex::default(),
            console_state: Mutex::new(ConsoleState::LoggedIn),
            thumbnail_jpeg: Mutex::new(Some(vec![0xff, 0xd8, 0xff, 0xd9])),
            thumbnail_jpeg_calls: AtomicUsize::new(0),
            filevault: Mutex::new(None),
            keep_awake_calls: Mutex::default(),
            session_actions: Mutex::default(),
            fail_session_actions: AtomicBool::new(false),
            wol_sends: Mutex::default(),
            encoder_rates: Mutex::default(),
            forced_keyframes: AtomicU64::new(0),
            encoders_started: Mutex::default(),
            hevc_unsupported: AtomicBool::new(false),
            capture_paused: AtomicBool::new(false),
            releases: AtomicU64::new(0),
            captures: AtomicUsize::new(0),
            keys: MemoryKeyStore::default(),
            directory_keys: None,
            nics: Vec::new(),
            audio: crate::audio::FakeAudio::default(),
            viewport: crate::viewport::FakeViewport::default(),
            system_auth: Mutex::default(),
            vdisplays: Mutex::default(),
        }
    }
}

/// Notify every live observer without blocking; full channels stay registered.
fn notify<T: Clone>(watchers: &Mutex<Vec<mpsc::Sender<T>>>, value: T) -> usize {
    let mut sent = 0;
    watchers
        .lock()
        .unwrap_or_else(|p| p.into_inner())
        .retain(|tx| match tx.try_send(value.clone()) {
            Ok(()) => {
                sent += 1;
                true
            }
            Err(mpsc::error::TrySendError::Full(_)) => true,
            Err(mpsc::error::TrySendError::Closed(_)) => false,
        });
    sent
}

/// Clones share input records and counters, so tests can inspect a running host.
#[derive(Clone, Default)]
pub struct FakePlatform {
    state: Arc<State>,
}

impl FakePlatform {
    /// 装上（或拆掉）虚拟显示器后端。必须在 Host 启动前调用：host-core 只在启动时取一次。
    pub fn set_virtual_displays(
        &self,
        backend: Option<Arc<dyn host_core::vdisplay::VirtualDisplayBackend>>,
    ) {
        *self
            .state
            .vdisplays
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = backend;
    }

    pub fn set_system_authenticator(&self, auth: Arc<dyn host_core::sysauth::SystemAuthenticator>) {
        *self
            .state
            .system_auth
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = Some(auth);
    }

    pub fn new() -> Self {
        Self::default()
    }

    pub fn with_state_dir(path: impl Into<PathBuf>) -> Self {
        Self {
            state: Arc::new(State {
                directory_keys: Some(DirectoryKeyStore { dir: path.into() }),
                ..State::default()
            }),
        }
    }

    pub fn with_nics(nics: Vec<Nic>) -> Self {
        Self {
            state: Arc::new(State {
                nics,
                ..State::default()
            }),
        }
    }

    pub fn with_displays(displays: Vec<DisplayInfo>) -> Self {
        Self {
            state: Arc::new(State {
                displays: Mutex::new(displays),
                ..State::default()
            }),
        }
    }

    pub fn injected_msgs(&self) -> Vec<InputMsg> {
        self.state
            .msgs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .iter()
            .map(|(msg, _)| msg.clone())
            .collect()
    }

    pub fn injected_with_display(&self) -> Vec<(InputMsg, u32)> {
        self.state
            .msgs
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn clear_injected(&self) {
        let mut msgs = self.state.msgs.lock().unwrap_or_else(|p| p.into_inner());
        let mut events = self.state.events.lock().unwrap_or_else(|p| p.into_inner());
        msgs.clear();
        events.clear();
    }

    pub fn local_input(&self) -> usize {
        notify(&self.state.local_watchers, true)
    }

    pub fn set_clipboard(&self, item: Option<ClipboardItem>) {
        let mut clipboard = self
            .state
            .clipboard
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *clipboard = item;
        notify(&self.state.clipboard_watchers, ());
    }

    pub fn clipboard_writes(&self) -> Vec<ClipboardItem> {
        self.state
            .clipboard_writes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn set_displays(&self, displays: Vec<DisplayInfo>) {
        let mut current = self
            .state
            .displays
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        *current = displays;
        notify(&self.state.display_watchers, current.clone());
    }

    pub fn capture_displays(&self) -> Vec<u32> {
        self.state
            .capture_displays
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn push_cursor(&self, update: CursorUpdate) -> usize {
        notify(&self.state.cursor_watchers, update)
    }

    pub fn set_console_state(&self, state: ConsoleState) {
        *self
            .state
            .console_state
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = state;
    }

    /// Configure the bytes returned by the synthetic primary-display snapshot.
    pub fn set_thumbnail_jpeg(&self, image: Option<Vec<u8>>) {
        *self
            .state
            .thumbnail_jpeg
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = image;
    }

    pub fn thumbnail_jpeg_calls(&self) -> usize {
        self.state.thumbnail_jpeg_calls.load(Ordering::Relaxed)
    }

    pub fn set_filevault(&self, enabled: Option<bool>) {
        *self
            .state
            .filevault
            .lock()
            .unwrap_or_else(|p| p.into_inner()) = enabled;
    }

    pub fn keep_awake(&self) -> bool {
        self.state
            .keep_awake_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .last()
            .copied()
            .unwrap_or(false)
    }

    pub fn keep_awake_calls(&self) -> Vec<bool> {
        self.state
            .keep_awake_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn session_actions(&self) -> Vec<SessionActionKind> {
        self.state
            .session_actions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn fail_session_actions(&self, fail: bool) {
        self.state
            .fail_session_actions
            .store(fail, Ordering::Relaxed);
    }

    pub fn wol_sends(&self) -> Vec<([u8; 6], String)> {
        self.state
            .wol_sends
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn encoder_rates(&self) -> Vec<(u32, u32)> {
        self.state
            .encoder_rates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn forced_keyframes(&self) -> u64 {
        self.state.forced_keyframes.load(Ordering::Relaxed)
    }

    pub fn encoders_started(&self) -> Vec<(u32, u32, u32, u32)> {
        self.encoders_started_with_codecs()
            .into_iter()
            .map(|start| (start.width, start.height, start.fps, start.bitrate_kbps))
            .collect()
    }

    /// Encoder starts including negotiated codecs; keeps the legacy hook intact.
    pub fn encoders_started_with_codecs(&self) -> Vec<EncoderStart> {
        self.state
            .encoders_started
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn set_hevc_unsupported(&self, unsupported: bool) {
        self.state
            .hevc_unsupported
            .store(unsupported, Ordering::Relaxed);
    }

    /// Simulate an unchanged screen by suppressing capture callbacks.
    pub fn set_capture_paused(&self, paused: bool) {
        self.state.capture_paused.store(paused, Ordering::Relaxed);
    }

    pub fn injected_events(&self) -> Vec<InputEvent> {
        self.state
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn release_count(&self) -> u64 {
        self.state.releases.load(Ordering::Relaxed)
    }

    pub fn active_captures(&self) -> usize {
        self.state.captures.load(Ordering::Relaxed)
    }

    pub fn audio(&self) -> crate::audio::FakeAudio {
        self.state.audio.clone()
    }

    pub fn viewport(&self) -> crate::viewport::FakeViewport {
        self.state.viewport.clone()
    }
}

/// Native metadata carried by CapturedFrame; no pixel allocation is needed.
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct SyntheticFrame {
    pub frame_number: u64,
    pub capture_ts_us: u64,
}

struct FakeCapture {
    task: JoinHandle<()>,
    state: Arc<State>,
}

impl CaptureHandle for FakeCapture {}

impl Drop for FakeCapture {
    fn drop(&mut self) {
        self.task.abort();
        self.state.captures.fetch_sub(1, Ordering::Relaxed);
    }
}

impl Platform for FakePlatform {
    fn virtual_displays(&self) -> Option<Arc<dyn host_core::vdisplay::VirtualDisplayBackend>> {
        self.state
            .vdisplays
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn system_authenticator(&self) -> Option<Arc<dyn host_core::sysauth::SystemAuthenticator>> {
        self.state
            .system_auth
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn info(&self) -> HostInfo {
        HostInfo {
            name: "fake-host".into(),
            os: pb::Platform::Linux,
            os_version: std::env::consts::OS.into(),
            model: "synthetic".into(),
            arch: std::env::consts::ARCH.into(),
        }
    }

    fn displays(&self) -> Vec<DisplayInfo> {
        self.state
            .displays
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn watch_displays(&self, tx: mpsc::Sender<Vec<DisplayInfo>>) {
        let displays = self
            .state
            .displays
            .lock()
            .unwrap_or_else(|p| p.into_inner());
        let _ = tx.try_send(displays.clone());
        self.state
            .display_watchers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(tx);
    }

    fn capture(
        &self,
        display: u32,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let display = self
            .displays()
            .into_iter()
            .find(|d| d.id == display)
            .ok_or_else(|| PlatformError::Platform("unknown display".into()))?;
        if cfg.fps == 0 || cfg.fps > 120 {
            return Err(PlatformError::Platform("capture fps must be 1..120".into()));
        }
        let (width, height) = dimensions(&display, cfg.max_width, cfg.max_height)?;
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| PlatformError::Platform(error.to_string()))?;
        self.state.captures.fetch_add(1, Ordering::Relaxed);
        self.state
            .capture_displays
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(display.id);
        let state = Arc::clone(&self.state);
        let task = runtime.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(cfg.fps)));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut frame_number = 0u64;
            loop {
                tick.tick().await;
                if state.capture_paused.load(Ordering::Relaxed) {
                    continue;
                }
                let capture_ts_us = unix_micros();
                let frame = CapturedFrame {
                    display_id: display.id,
                    width,
                    height,
                    capture_ts_us,
                    native: Arc::new(SyntheticFrame {
                        frame_number,
                        capture_ts_us,
                    }),
                };
                if let Err(mpsc::error::TrySendError::Closed(_)) = sink.try_send(frame) {
                    return;
                }
                frame_number = frame_number.wrapping_add(1);
            }
        });
        Ok(Box::new(FakeCapture {
            task,
            state: Arc::clone(&self.state),
        }))
    }

    fn audio_capture_supported(&self) -> bool {
        crate::audio::supported(&self.state.audio)
    }

    fn capture_audio(
        &self,
        cfg: AudioCaptureConfig,
        sink: AudioChunkSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        crate::audio::start(&self.state.audio, cfg, sink)
    }

    fn capture_region_supported(&self) -> bool {
        self.state.viewport.capture_region_supported()
    }

    fn capture_region(
        &self,
        display: u32,
        rect: PixelRect,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        let info = self
            .displays()
            .into_iter()
            .find(|d| d.id == display)
            .ok_or_else(|| PlatformError::Platform("unknown display".into()))?;
        self.state.viewport.capture_region(&info, rect, cfg, sink)
    }

    fn encoder(&self, cfg: EncoderConfig) -> Result<Box<dyn Encoder>> {
        if !matches!(cfg.codec, pb::Codec::Hevc | pb::Codec::H264)
            || (cfg.codec == pb::Codec::Hevc && self.state.hevc_unsupported.load(Ordering::Relaxed))
        {
            return Err(PlatformError::Unsupported);
        }
        if cfg.fps == 0 || cfg.fps > 120 || cfg.bitrate_kbps == 0 || cfg.width < 2 || cfg.height < 2
        {
            return Err(PlatformError::Platform(
                "invalid encoder configuration".into(),
            ));
        }
        self.state
            .encoders_started
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(EncoderStart {
                width: cfg.width,
                height: cfg.height,
                fps: cfg.fps,
                bitrate_kbps: cfg.bitrate_kbps,
                codec: cfg.codec,
            });
        Ok(Box::new(FakeEncoder {
            state: Arc::clone(&self.state),
            cfg,
            force: true,
            quality_boost: false,
            last_key_us: 0,
        }))
    }

    fn inject(&self, ev: InputEvent, _display: &DisplayInfo) {
        self.state
            .events
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(ev);
    }

    fn inject_input(&self, msg: &InputMsg, display: &DisplayInfo) {
        let mut msgs = self.state.msgs.lock().unwrap_or_else(|p| p.into_inner());
        msgs.push((msg.clone(), display.id));
        if let Some(ev) = input_to_m0(msg) {
            self.inject(ev, display);
        }
        if matches!(msg, InputMsg::ReleaseAll) {
            self.release_all();
        }
    }

    fn release_all(&self) {
        self.state.releases.fetch_add(1, Ordering::Relaxed);
    }

    fn session_action(&self, kind: SessionActionKind) -> Result<()> {
        self.state
            .session_actions
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(kind);
        if self.state.fail_session_actions.load(Ordering::Relaxed) {
            Err(PlatformError::Platform("fake failure".into()))
        } else {
            Ok(())
        }
    }

    fn watch_cursor(&self, tx: mpsc::Sender<CursorUpdate>) {
        let _ = tx.try_send(CursorUpdate::Position {
            display_id: 1,
            seq: 0,
            x: 0.5,
            y: 0.5,
            visible: true,
        });
        self.state
            .cursor_watchers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(tx);
    }

    fn watch_local_activity(&self, tx: mpsc::Sender<bool>) {
        self.state
            .local_watchers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(tx);
    }
    fn clipboard_get(&self) -> Option<ClipboardItem> {
        self.state
            .clipboard
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }
    fn clipboard_set(&self, item: ClipboardItem) -> Result<()> {
        self.state
            .clipboard_writes
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(item.clone());
        self.set_clipboard(Some(item));
        Ok(())
    }
    fn watch_clipboard(&self, tx: mpsc::Sender<()>) {
        self.state
            .clipboard_watchers
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(tx);
    }
    fn power_keep_awake(&self, on: bool) {
        self.state
            .keep_awake_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(on);
    }
    fn console_state(&self) -> ConsoleState {
        *self
            .state
            .console_state
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }
    fn thumbnail_jpeg(&self, _max_width: u32, _max_bytes: usize) -> Option<Vec<u8>> {
        self.state
            .thumbnail_jpeg_calls
            .fetch_add(1, Ordering::Relaxed);
        self.state
            .thumbnail_jpeg
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    fn filevault_enabled(&self) -> Option<bool> {
        *self
            .state
            .filevault
            .lock()
            .unwrap_or_else(|p| p.into_inner())
    }
    fn permissions(&self) -> Permissions {
        Permissions {
            screen_recording: true,
            accessibility: true,
        }
    }
    fn request_permission(&self, _p: PermissionKind) {}
    fn nics(&self) -> Vec<Nic> {
        self.state.nics.clone()
    }
    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        self.state
            .wol_sends
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((mac, via.name.clone()));
        Ok(())
    }
    fn keystore(&self) -> &dyn KeyStore {
        self.state
            .directory_keys
            .as_ref()
            .map(|keys| keys as &dyn KeyStore)
            .unwrap_or(&self.state.keys)
    }
}

struct FakeEncoder {
    state: Arc<State>,
    cfg: EncoderConfig,
    force: bool,
    quality_boost: bool,
    last_key_us: u64,
}

impl Encoder for FakeEncoder {
    fn encode(&mut self, frame: CapturedFrame, opts: EncodeOpts) -> Result<()> {
        let metadata = frame
            .native
            .downcast_ref::<SyntheticFrame>()
            .ok_or_else(|| PlatformError::Platform("not a synthetic frame".into()))?;
        let keyframe = self.force
            || opts.force_keyframe
            || frame.capture_ts_us.saturating_sub(self.last_key_us) >= 2_000_000;
        // A keyframe is four delta frames; compensate across a two-second GOP.
        let base = (u64::from(self.cfg.bitrate_kbps) * 1000 / 8 * 2
            / (u64::from(self.cfg.fps) * 2 + 3)) as usize;
        let size = base
            .saturating_mul(if keyframe { 4 } else { 1 })
            .clamp(22, 4 << 20);
        let mut data = Vec::with_capacity(size);
        if self.cfg.codec == pb::Codec::Hevc {
            data.extend_from_slice(&[0, 0, 0, 1, if keyframe { 19 << 1 } else { 1 << 1 }, 1]);
        } else {
            data.extend_from_slice(&[0, 0, 0, 1, if keyframe { 0x65 } else { 0x41 }]);
        }
        data.extend_from_slice(&metadata.frame_number.to_be_bytes());
        data.extend_from_slice(&metadata.capture_ts_us.to_be_bytes());
        data.resize(
            size,
            if self.quality_boost || opts.quality_boost {
                0x66
            } else {
                0x55
            },
        );
        let output = EncodedFrame {
            data,
            keyframe,
            still_refine: self.quality_boost || opts.quality_boost,
            capture_ts_us: frame.capture_ts_us,
            encoded_ts_us: unix_micros(),
        };
        match self.cfg.sink.try_send(output) {
            Ok(()) => {
                self.force = false;
                if keyframe {
                    self.last_key_us = frame.capture_ts_us;
                }
            }
            Err(mpsc::error::TrySendError::Full(_)) => self.force = true,
            Err(mpsc::error::TrySendError::Closed(_)) => {
                return Err(PlatformError::Platform("encoder sink closed".into()));
            }
        }
        Ok(())
    }

    fn set_rate(&mut self, kbps: u32, fps: u32) {
        self.state
            .encoder_rates
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push((kbps, fps));
        self.cfg.bitrate_kbps = kbps.clamp(1, 150_000);
        self.cfg.fps = fps.clamp(1, 120);
    }

    fn force_keyframe(&mut self) {
        self.state.forced_keyframes.fetch_add(1, Ordering::Relaxed);
        self.force = true;
    }
    fn set_quality_boost(&mut self, on: bool) {
        self.quality_boost = on;
    }

    fn roi_supported(&self) -> bool {
        self.state.viewport.roi_supported()
    }

    fn set_roi(&mut self, regions: &[RoiRegion]) -> Result<()> {
        self.state.viewport.set_roi(regions)
    }
}
