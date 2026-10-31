//! Adapter session task and DriverHost ; monitor selection.

use std::collections::HashMap;
use std::io;
use std::sync::{atomic::Ordering, Arc, Mutex, MutexGuard};
use std::time::Duration;

use async_trait::async_trait;
use rsp_core::hid::InputMsg;
use rsp_transport::net::{NetDialer, NetStream};
use tokio::sync::{mpsc, oneshot};
use zeroize::Zeroizing;

use super::audio::AdapterAudioTable;
use super::driver::{
    AdapterError, Credentials, Dial, DriverHost, DriverInput, DriverParams, PcmSink, ProtocolDriver,
};
use super::dto::{
    AdapterErrorInfo, AdapterEvent, AdapterOpenOpts, AdapterPhase, AdapterProtocol, AdapterScreen,
    AdapterSummary, AdapterTarget, CredField,
};
use super::ffi_raw::RsRect;
use super::rawframe::{RawFrame, RawRegistry};
use super::secrets;
use super::table::{AdapterEntry, AdapterTable};
use super::{rdp::RdpDriver, vnc::VncDriver};
use crate::rscore::error::CoreError;
use crate::rscore::runtime::Core;

const DEFAULT_CONNECT_TIMEOUT: Duration = Duration::from_secs(15);
const DEFAULT_CERT_PROMPT_TIMEOUT: Duration = Duration::from_secs(60);
const CLOSE_GRACE: Duration = Duration::from_millis(100);

fn lock<T>(mutex: &Mutex<T>) -> MutexGuard<'_, T> {
    mutex.lock().unwrap_or_else(|error| error.into_inner())
}

#[derive(Debug, Clone, Copy)]
pub struct SessionTimings {
    pub connect: Duration,
    pub cert_prompt: Duration,
}

impl Default for SessionTimings {
    fn default() -> Self {
        Self {
            connect: DEFAULT_CONNECT_TIMEOUT,
            cert_prompt: DEFAULT_CERT_PROMPT_TIMEOUT,
        }
    }
}

pub const CERT_KV_PREFIX: &str = "adapter.cert.";

/// adapters_select_screen index that shows the whole desktop.
pub const WHOLE_DESKTOP: u8 = 255;

/// The last adapter.screens layout and the shown monitor.
#[derive(Default)]
struct ScreenView {
    screens: Vec<AdapterScreen>,
    selected: Option<usize>,
}

impl ScreenView {
    fn show(&mut self, id: u64, frame: &RawFrame, selected: Option<usize>) {
        self.selected = selected;
        let rect = selected.map(|index| {
            let screen = self.screens[index];
            RsRect {
                x: screen.x,
                y: screen.y,
                w: screen.width,
                h: screen.height,
            }
        });
        if frame.set_view(rect) {
            tracing::info!(
                session_id = id,
                index = ?selected,
                rect = ?rect,
                "adapter screen selected"
            );
        }
    }
}

/// Screen views by adapter session id.
#[derive(Default)]
struct ScreenTable {
    views: Mutex<HashMap<u64, Arc<Mutex<ScreenView>>>>,
}

impl ScreenTable {
    fn create(&self, id: u64) -> Arc<Mutex<ScreenView>> {
        lock(&self.views).entry(id).or_default().clone()
    }

    fn get(&self, id: u64) -> Option<Arc<Mutex<ScreenView>>> {
        lock(&self.views).get(&id).cloned()
    }

    fn remove(&self, id: u64) {
        lock(&self.views).remove(&id);
    }
}

/// TCP dialer used by the built-in protocol drivers.
pub struct NetDial {
    net: Arc<dyn NetDialer>,
    addr: String,
    port: u16,
    timeout: Duration,
}

impl NetDial {
    pub fn new(core: &Arc<Core>, target: &AdapterTarget, timeout: Duration) -> Self {
        Self {
            net: core.net(),
            addr: target.addr.clone(),
            port: target.port_or_default(),
            timeout,
        }
    }
}

fn map_io_error(error: io::Error) -> AdapterError {
    match error.kind() {
        io::ErrorKind::ConnectionRefused => AdapterError::Refused,
        io::ErrorKind::TimedOut
        | io::ErrorKind::HostUnreachable
        | io::ErrorKind::NetworkUnreachable
        | io::ErrorKind::AddrNotAvailable
        | io::ErrorKind::NotFound => AdapterError::Timeout,
        _ => AdapterError::Io(error.to_string()),
    }
}

#[async_trait]
impl Dial for NetDial {
    async fn dial(&self) -> Result<NetStream, AdapterError> {
        match tokio::time::timeout(self.timeout, self.net.dial_tcp_host(&self.addr, self.port))
            .await
        {
            Ok(Ok(stream)) => Ok(stream),
            Ok(Err(error)) => Err(map_io_error(error)),
            Err(_) => Err(AdapterError::Timeout),
        }
    }
}

pub fn driver_for(protocol: AdapterProtocol) -> Arc<dyn ProtocolDriver> {
    match protocol {
        AdapterProtocol::Vnc => Arc::new(VncDriver),
        AdapterProtocol::Rdp => Arc::new(RdpDriver),
    }
}

fn valid_target(target: &AdapterTarget) -> bool {
    !target.addr.is_empty()
        && target.addr.len() <= 253
        && !target
            .addr
            .chars()
            .any(|character| character.is_whitespace() || character.is_control())
        && target.port != Some(0)
}

#[allow(clippy::too_many_arguments)]
fn emit_state(
    core: &Core,
    entry: &AdapterEntry,
    phase: AdapterPhase,
    cred_fields: Vec<CredField>,
    cred_error: bool,
    width: Option<u32>,
    height: Option<u32>,
    server_name: Option<String>,
    error: Option<AdapterErrorInfo>,
) {
    core.events().emit(&AdapterEvent::State {
        session_id: entry.id.to_string(),
        protocol: entry.protocol(),
        phase,
        cred_fields,
        cred_error,
        width,
        height,
        server_name,
        error,
    });
}

fn emit_connecting(core: &Core, entry: &AdapterEntry) {
    entry.set_phase(AdapterPhase::Connecting);
    emit_state(
        core,
        entry,
        AdapterPhase::Connecting,
        Vec::new(),
        false,
        None,
        None,
        None,
        None,
    );
}

struct SessionHost {
    core: Arc<Core>,
    entry: Arc<AdapterEntry>,
    cert_prompt: Duration,
    frame: Arc<RawFrame>,
    screens: Arc<Mutex<ScreenView>>,
}

impl SessionHost {
    fn clone_credentials(credentials: &Credentials) -> Credentials {
        Credentials {
            user: credentials.user.clone(),
            password: Zeroizing::new(credentials.password.as_str().to_owned()),
            domain: credentials.domain.clone(),
        }
    }

    fn clear_pending(&self) {
        let _ = lock(&self.entry.pending_credentials).take();
    }

    fn set_waiter(&self, waiter: oneshot::Sender<Option<Credentials>>) -> bool {
        let mut slot = lock(&self.entry.credential_waiter);
        if self.entry.closed_by_user.load(Ordering::Acquire) {
            return false;
        }
        *slot = Some(waiter);
        true
    }

    fn emit_credentials(&self, fields: &[CredField], failed_before: bool) {
        self.entry.set_phase(AdapterPhase::NeedsCredentials);
        emit_state(
            &self.core,
            &self.entry,
            AdapterPhase::NeedsCredentials,
            fields.to_vec(),
            failed_before,
            None,
            None,
            None,
            None,
        );
    }
}

#[async_trait]
impl DriverHost for SessionHost {
    fn background(&self) -> tokio::sync::watch::Receiver<bool> {
        self.entry.background.subscribe()
    }

    fn session_id(&self) -> u64 {
        self.entry.id
    }

    async fn credentials(&self, fields: &[CredField], failed_before: bool) -> Option<Credentials> {
        if self.entry.closed_by_user.load(Ordering::Acquire) {
            return None;
        }

        let first_call = !self.entry.saved_attempted.swap(true, Ordering::AcqRel);
        if first_call && self.entry.opts.use_saved {
            match secrets::load(&self.entry.store_key) {
                Ok(Some(credentials)) => {
                    self.entry.used_saved.store(true, Ordering::Release);
                    return Some(credentials);
                }
                Ok(None) => {}
                Err(error) => {
                    tracing::warn!(
                        session_id = self.entry.id,
                        "failed to load saved adapter credentials: {error}"
                    );
                }
            }
        }

        self.entry.used_saved.store(false, Ordering::Release);
        self.clear_pending();
        let (sender, receiver) = oneshot::channel();
        if !self.set_waiter(sender) {
            return None;
        }
        self.emit_credentials(fields, failed_before);
        match receiver.await {
            Ok(Some(credentials)) => {
                self.entry.set_phase(AdapterPhase::Connecting);
                Some(credentials)
            }
            Ok(None) | Err(_) => None,
        }
    }

    fn auth_succeeded(&self) {
        if self.entry.used_saved.swap(false, Ordering::AcqRel) {
            self.clear_pending();
            return;
        }
        let Some((credentials, remember)) = lock(&self.entry.pending_credentials).take() else {
            return;
        };
        if !remember || !secrets::can_save() {
            return;
        }
        let core = self.core.clone();
        let key = self.entry.store_key.clone();
        self.core.handle().spawn_blocking(move || {
            if let Err(error) = secrets::save(&core, &key, &credentials) {
                tracing::warn!("failed to save adapter credentials: {error}");
            }
        });
    }

    fn authenticating(&self) {
        self.entry.set_phase(AdapterPhase::Authenticating);
        emit_state(
            &self.core,
            &self.entry,
            AdapterPhase::Authenticating,
            Vec::new(),
            false,
            None,
            None,
            None,
            None,
        );
    }

    fn streaming(&self, width: u32, height: u32, server_name: &str) {
        *lock(&self.entry.width) = Some(width);
        *lock(&self.entry.height) = Some(height);
        self.entry.set_phase(AdapterPhase::Streaming);
        emit_state(
            &self.core,
            &self.entry,
            AdapterPhase::Streaming,
            Vec::new(),
            false,
            Some(width),
            Some(height),
            Some(server_name.to_owned()),
            None,
        );
    }

    fn clipboard(&self, text: &str) {
        self.core.events().emit(&AdapterEvent::Clipboard {
            session_id: self.entry.id.to_string(),
            text: text.to_owned(),
        });
    }

    fn clipboard_other(&self) {
        self.core.events().emit(&AdapterEvent::ClipboardOther {
            session_id: self.entry.id.to_string(),
        });
    }

    fn screens(&self, screens: &[AdapterScreen]) {
        {
            let mut view = lock(&self.screens);
            view.screens = screens.to_vec();
            if let Some(index) = view.selected {
                // Keep the monitor; a smaller layout falls back to the first one.
                let selected = if index < screens.len() {
                    Some(index)
                } else {
                    (!screens.is_empty()).then_some(0)
                };
                view.show(self.entry.id, &self.frame, selected);
            }
        }
        self.core.events().emit(&AdapterEvent::Screens {
            session_id: self.entry.id.to_string(),
            screens: screens.to_vec(),
        });
    }

    fn audio(&self) -> Option<Arc<dyn PcmSink>> {
        let audio: Arc<dyn PcmSink> = self.core.state::<AdapterAudioTable>().get(self.entry.id)?;
        Some(audio)
    }

    async fn confirm_cert(&self, fp_hex: &str) -> bool {
        if self.entry.closed_by_user.load(Ordering::Acquire) {
            return false;
        }
        let key = format!("{CERT_KV_PREFIX}{}", self.entry.store_key);
        let old = match self.core.store().kv_get(&key) {
            Ok(old) => old,
            Err(error) => {
                tracing::warn!(
                    session_id = self.entry.id,
                    "failed to read adapter certificate pin: {error}"
                );
                return true;
            }
        };
        match old {
            None => {
                if let Err(error) = self.core.store().kv_set(&key, fp_hex) {
                    tracing::warn!(
                        session_id = self.entry.id,
                        "failed to write adapter certificate pin: {error}"
                    );
                }
                true
            }
            Some(old_fp) if old_fp == fp_hex => true,
            Some(old_fp) => {
                let (sender, receiver) = oneshot::channel();
                {
                    let mut slot = lock(&self.entry.cert_waiter);
                    if self.entry.closed_by_user.load(Ordering::Acquire) {
                        return false;
                    }
                    *slot = Some(sender);
                }
                self.core.events().emit(&AdapterEvent::CertPrompt {
                    session_id: self.entry.id.to_string(),
                    fp: colon_fp(fp_hex),
                    old_fp: Some(colon_fp(&old_fp)),
                });
                let accepted = matches!(
                    tokio::time::timeout(self.cert_prompt, receiver).await,
                    Ok(Ok(true))
                );
                // The slot may have been taken by trust_cert or close. If the
                // timeout won, remove the stale sender before another prompt.
                lock(&self.entry.cert_waiter).take();
                if accepted {
                    if let Err(error) = self.core.store().kv_set(&key, fp_hex) {
                        tracing::warn!(
                            session_id = self.entry.id,
                            "failed to update adapter certificate pin: {error}"
                        );
                    }
                }
                accepted
            }
        }
    }
}

pub async fn open_with(
    core: &Arc<Core>,
    target: AdapterTarget,
    opts: AdapterOpenOpts,
    driver: Arc<dyn ProtocolDriver>,
    dial: Arc<dyn Dial>,
) -> Result<u64, CoreError> {
    open_with_timings(core, target, opts, driver, dial, SessionTimings::default()).await
}

pub async fn open_with_timings(
    core: &Arc<Core>,
    target: AdapterTarget,
    opts: AdapterOpenOpts,
    driver: Arc<dyn ProtocolDriver>,
    dial: Arc<dyn Dial>,
    timings: SessionTimings,
) -> Result<u64, CoreError> {
    if !valid_target(&target) {
        return Err(CoreError::internal("invalid adapter target"));
    }

    let table = core.state::<AdapterTable>();
    let id = table.alloc_id();
    let store_key = target.store_key();
    let (input_sender, input_receiver) = mpsc::channel(256);
    let (cancel_sender, cancel_receiver) = oneshot::channel();
    let entry = Arc::new(AdapterEntry::new(
        id,
        target.clone(),
        store_key,
        opts.clone(),
        input_sender,
        cancel_sender,
    ));
    let frame = RawFrame::new();
    core.state::<RawRegistry>().insert(id, frame.clone());
    core.state::<AdapterAudioTable>().get_or_create(id);
    let screens = core.state::<ScreenTable>().create(id);

    if let Some(previous) = table.insert(entry.clone()) {
        previous.request_close();
    }
    emit_connecting(core, &entry);

    let addr_kind = if target.addr.parse::<std::net::IpAddr>().is_ok() {
        "ip"
    } else {
        "hostname"
    };
    tracing::info!(
        id,
        protocol = target.protocol.as_str(),
        addr_kind,
        "adapter session opened"
    );

    let host: Arc<dyn DriverHost> = Arc::new(SessionHost {
        core: core.clone(),
        entry: entry.clone(),
        cert_prompt: timings.cert_prompt,
        frame: frame.clone(),
        screens,
    });
    let params = DriverParams {
        dial,
        server_name: target.addr.clone(),
        view_width: opts.view_width,
        view_height: opts.view_height,
        scale: opts.scale,
        monitors: opts.monitors.clamp(1, 4),
    };
    let task_core = core.clone();
    let task_table = table.clone();
    let task_raw = task_core.state::<RawRegistry>();
    let task_entry = entry.clone();
    core.spawn(async move {
        let run = driver.run(params, host, frame.clone(), input_receiver);
        tokio::pin!(run);
        let result = tokio::select! {
            result = &mut run => result,
            _ = cancel_receiver => {
                // Closing a credential or certificate prompt first wakes the
                // driver with None/false. Give it a short chance to unwind so
                // callers observe that cancellation result before forcing a
                // genuinely stuck protocol task to end.
                match tokio::time::timeout(CLOSE_GRACE, &mut run).await {
                    Ok(result) => result,
                    Err(_) => Err(AdapterError::Cancelled),
                }
            },
        };
        let clean = task_entry.closed_by_user.load(Ordering::Acquire) || result.is_ok();
        let error = (!clean)
            .then(|| {
                result.as_ref().err().map(|error| AdapterErrorInfo {
                    code: error.code().to_owned(),
                    rs_code: error.rs_code(),
                    message: error.to_string(),
                })
            })
            .flatten();
        task_entry.set_phase(AdapterPhase::Ended);
        emit_state(
            &task_core,
            &task_entry,
            AdapterPhase::Ended,
            Vec::new(),
            false,
            *lock(&task_entry.width),
            *lock(&task_entry.height),
            None,
            error.clone(),
        );
        let reason = error
            .as_ref()
            .map_or_else(|| "closed".to_owned(), |error| error.message.clone());
        frame.close(&reason);
        task_table.remove(task_entry.id);
        task_raw.remove(task_entry.id);
        task_core.state::<AdapterAudioTable>().remove(task_entry.id);
        task_core.state::<ScreenTable>().remove(task_entry.id);
        let addr_kind = if task_entry.target.addr.parse::<std::net::IpAddr>().is_ok() {
            "ip"
        } else {
            "hostname"
        };
        // The code is a fixed identifier; the message may carry addresses.
        let by_user = task_entry.closed_by_user.load(Ordering::Acquire);
        let code = error.as_ref().map_or("none", |error| error.code.as_str());
        tracing::info!(
            id = task_entry.id,
            protocol = task_entry.protocol().as_str(),
            addr_kind,
            by_user,
            code,
            "adapter session closed"
        );
    });
    Ok(id)
}

pub fn close(core: &Core, id: u64) -> bool {
    let Some(entry) = core.state::<AdapterTable>().get(id) else {
        return false;
    };
    entry.request_close();
    true
}

pub fn background(core: &Core, id: u64, in_background: bool) -> Result<(), CoreError> {
    let entry = core
        .state::<AdapterTable>()
        .get(id)
        .ok_or_else(|| CoreError::internal(format!("unknown session {id}")))?;
    entry.background.send_if_modified(|paused| {
        if *paused == in_background {
            return false;
        }
        *paused = in_background;
        true
    });
    Ok(())
}

pub fn submit_credentials(
    core: &Core,
    id: u64,
    creds: Credentials,
    remember: bool,
) -> Result<(), CoreError> {
    let Some(entry) = core.state::<AdapterTable>().get(id) else {
        return Err(CoreError::internal("unknown adapter session"));
    };
    let waiter = lock(&entry.credential_waiter)
        .take()
        .ok_or_else(|| CoreError::internal("not waiting for credentials"))?;
    let pending = SessionHost::clone_credentials(&creds);
    *lock(&entry.pending_credentials) = Some((pending, remember));
    if waiter.send(Some(creds)).is_err() {
        lock(&entry.pending_credentials).take();
        return Err(CoreError::internal("not waiting for credentials"));
    }
    Ok(())
}

pub fn cancel_credentials(core: &Core, id: u64) {
    let Some(entry) = core.state::<AdapterTable>().get(id) else {
        return;
    };
    if let Some(waiter) = lock(&entry.credential_waiter).take() {
        let _ = waiter.send(None);
    }
    lock(&entry.pending_credentials).take();
}

pub fn trust_cert(core: &Core, id: u64, accept: bool) {
    let Some(entry) = core.state::<AdapterTable>().get(id) else {
        return;
    };
    let waiter = lock(&entry.cert_waiter).take();
    if let Some(waiter) = waiter {
        let _ = waiter.send(accept);
    }
}

pub fn send_input(core: &Core, id: u64, input: DriverInput) -> Result<(), CoreError> {
    let Some(entry) = core.state::<AdapterTable>().get(id) else {
        return Err(CoreError::internal("unknown adapter session"));
    };
    if entry.closed_by_user.load(Ordering::Acquire) {
        return Err(CoreError::internal("unknown adapter session"));
    }
    if *entry.background.borrow() && !matches!(&input, DriverInput::Clipboard(_)) {
        return Ok(());
    }
    let sender = lock(&entry.input).clone();
    let Some(sender) = sender else {
        return Err(CoreError::internal("unknown adapter session"));
    };
    let input = match input {
        DriverInput::Hid(msg) => DriverInput::Hid(to_desktop(core, id, msg)),
        input => input,
    };
    match sender.try_send(input) {
        Ok(()) => Ok(()),
        Err(mpsc::error::TrySendError::Full(_)) => {
            tracing::warn!(session_id = id, "adapter input queue full; dropping input");
            Ok(())
        }
        Err(mpsc::error::TrySendError::Closed(_)) => {
            Err(CoreError::internal("unknown adapter session"))
        }
    }
}

/// Pointer coordinates arrive normalized to the shown monitor; drivers take
/// them normalized to the whole desktop.
fn to_desktop(core: &Core, id: u64, mut msg: InputMsg) -> InputMsg {
    if let InputMsg::MouseMove { x, y }
    | InputMsg::MouseButton { x, y, .. }
    | InputMsg::Touch { x, y, .. } = &mut msg
    {
        let view = core
            .state::<RawRegistry>()
            .get(id)
            .and_then(|frame| frame.view());
        if let Some((view, width, height)) = view {
            *x = to_desktop_axis(*x, view.x, view.w, width);
            *y = to_desktop_axis(*y, view.y, view.h, height);
        }
    }
    msg
}

fn to_desktop_axis(value: f32, origin: u32, side: u32, desktop: u32) -> f32 {
    let value = f64::from(value.clamp(0.0, 1.0));
    ((f64::from(origin) + value * f64::from(side)) / f64::from(desktop)).clamp(0.0, 1.0) as f32
}

/// Show monitor `index` of the last adapter.screens layout, or the whole
/// desktop with WHOLE_DESKTOP. Without a layout (VNC) there is one monitor and
/// 0 / WHOLE_DESKTOP do nothing. A session that is gone is ignored, like close.
pub fn select_screen(core: &Core, id: u64, index: u8) -> Result<(), CoreError> {
    let (Some(view), Some(frame)) = (
        core.state::<ScreenTable>().get(id),
        core.state::<RawRegistry>().get(id),
    ) else {
        return Ok(());
    };
    let mut view = lock(&view);
    let selected = match index {
        WHOLE_DESKTOP => None,
        0 if view.screens.is_empty() => return Ok(()),
        index if usize::from(index) < view.screens.len() => Some(usize::from(index)),
        _ => return Err(CoreError::internal("screen index out of range")),
    };
    view.show(id, &frame, selected);
    Ok(())
}

pub fn entry(core: &Core, id: u64) -> Option<Arc<AdapterEntry>> {
    core.state::<AdapterTable>().get(id)
}

pub fn list(core: &Core) -> Vec<AdapterSummary> {
    core.state::<AdapterTable>().list()
}

/// Format a 64-character SHA-256 hex fingerprint as colon-separated pairs.
pub fn colon_fp(hex: &str) -> String {
    if hex.len() != 64 || !hex.is_ascii() {
        return hex.to_owned();
    }
    hex.as_bytes()
        .chunks_exact(2)
        .map(|pair| {
            // SAFETY: `hex` is ASCII and each pair consists of two ASCII bytes.
            unsafe { std::str::from_utf8_unchecked(pair) }
        })
        .collect::<Vec<_>>()
        .join(":")
}
