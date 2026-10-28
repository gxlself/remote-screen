//! RsCore session commands and the single client-to-native/JS event pump.

use std::{collections::BTreeMap, net::SocketAddr, sync::Arc, time::Duration};

use rsp_client::{
    clipboard::ClipboardClientError,
    recovery::{background::BgState, failover::TOKEN_LEN},
    EndReason, PrewarmPool, Session, SessionError, SessionEvent, SessionHandle, SessionParams,
    SessionStateKind,
};
use rsp_core::{
    hid::{InputMsg, Mods, MAX_TEXT_BYTES},
    pb,
};
use rsp_transport::dial::{plan, DefaultDialer};

use super::{
    adapters::{
        driver::DriverInput, is_adapter_id, route as adapter_route, session as adapter_session,
    },
    audio::{self, AudioTable},
    bgwire::{BgClock, BgDriver, BgFlow},
    combos::combo_inputs,
    dto_core::{
        action_kind_to_pb, id_str, quality_to_pb, Codec, CoreEvent, DevicePlatform, DialAttempt,
        GoodbyeReason, HubDisplay, OpenIntent, OpenOpts, Quality, SessionActionKind, SessionCombo,
        SessionPhase, SessionSummary,
    },
    error::CoreError,
    manual_addrs::host_target_with_manual,
    netinfo::{local_net_excluding, read_interfaces},
    runtime::{core, Core},
    sessmap::{dial_from_plan, dial_from_report, SessionMapper},
    settings::{Settings, SettingsState},
    store::{HostSeenRow, Store, StoreError},
    tables::{SessionEntry, SessionTable},
};

pub fn settings_snapshot(core: &Arc<Core>) -> Settings {
    core.state::<SettingsState>()
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .clone()
}

/// Push the current device power state to existing sessions. A session that is
/// still connecting reads it when its handle is published.
pub fn sync_low_power(core: &Arc<Core>) {
    let settings = core.state::<SettingsState>();
    let low_power = settings
        .0
        .lock()
        .unwrap_or_else(|error| error.into_inner())
        .low_power;
    for entry in core.state::<SessionTable>().list() {
        if let Some(handle) = entry.handle() {
            handle.set_low_power(low_power);
        }
    }
}

pub fn emit_phase(core: &Arc<Core>, entry: &SessionEntry, phase: SessionPhase) {
    emit_phase_with_dial(core, entry, phase, None);
}

pub fn emit_phase_with_dial(
    core: &Arc<Core>,
    entry: &SessionEntry,
    phase: SessionPhase,
    dial: Option<Vec<DialAttempt>>,
) {
    entry.update_summary(|summary| summary.phase = phase);
    core.events().emit(&CoreEvent::SessionState {
        session_id: id_str(entry.id),
        host_id: entry.host_id.clone(),
        phase,
        path: None,
        transport: None,
        derp_region: None,
        dial,
        approval_timeout_s: None,
        resumed: None,
        reconnect_reason: None,
    });
}

pub fn finish_open(
    core: &Arc<Core>,
    entry: &SessionEntry,
    code: Option<u32>,
    reason: Option<GoodbyeReason>,
    message: String,
) {
    finish_open_detail(core, entry, code, reason, message, None);
}

pub fn finish_open_detail(
    core: &Arc<Core>,
    entry: &SessionEntry,
    code: Option<u32>,
    reason: Option<GoodbyeReason>,
    message: String,
    detail: Option<BTreeMap<String, String>>,
) {
    core.events().emit(&CoreEvent::SessionError {
        session_id: id_str(entry.id),
        host_id: entry.host_id.clone(),
        code,
        reason,
        message: Some(message.clone()),
        detail,
        retry_after_s: None,
    });
    emit_phase(core, entry, SessionPhase::Ended);
    entry.video.close(&message);
    entry.recorder.closed(&message);
    core.state::<AudioTable>().remove(entry.id);
    core.state::<SessionTable>().remove(entry.id);
}

pub fn open(core: &Arc<Core>, host_id: &str, opts: OpenOpts) -> Result<u64, CoreError> {
    // Reject missing identity before allocating an ID or publishing an event.
    core.local_identity()?;
    let paired = core.store().paired_host(host_id)?;
    let host_name = paired
        .as_ref()
        .map(|host| host.name.as_str())
        .unwrap_or(host_id);
    let intent = match opts.intent {
        OpenIntent::Control => pb::Intent::Control,
        OpenIntent::View => pb::Intent::View,
    };
    // The same plan Session::open dials. run_session reports any setup error.
    let dial = host_params(core, host_id, intent)
        .ok()
        .map(|params| {
            dial_from_plan(&plan(
                params.candidates,
                &params.local_net,
                &params.intranet_cidrs,
            ))
        })
        .filter(|dial| !dial.is_empty());
    let entry = core
        .state::<SessionTable>()
        .insert(host_id, host_name, opts, core.now_us());
    emit_phase_with_dial(core, &entry, SessionPhase::Connecting, dial);
    let id = entry.id;
    let c = core.clone();
    core.spawn(async move { run_session(&c, entry).await });
    Ok(id)
}

#[derive(Debug, thiserror::Error)]
#[error("{message}")]
pub struct SessionSetupError {
    pub code: u32,
    pub message: String,
}

const DEFAULT_MAX_FPS: u32 = 60;
const MAX_MAX_FPS: u32 = 240;

fn start_fps(opts: &OpenOpts) -> u16 {
    opts.max_fps
        .unwrap_or(DEFAULT_MAX_FPS)
        .clamp(1, MAX_MAX_FPS) as u16
}

impl From<CoreError> for SessionSetupError {
    fn from(error: CoreError) -> Self {
        Self {
            code: 199,
            message: error.to_string(),
        }
    }
}

pub fn session_params(
    core: &Arc<Core>,
    entry: &SessionEntry,
) -> Result<SessionParams, SessionSetupError> {
    let intent = match entry.opts.intent {
        OpenIntent::Control => pb::Intent::Control,
        OpenIntent::View => pb::Intent::View,
    };
    let mut params = host_params(core, &entry.host_id, intent)?;
    params.display_id = entry.opts.display_id;
    params.quality = quality_to_pb(
        entry
            .opts
            .quality
            .unwrap_or(settings_snapshot(core).quality_default),
    );
    params.start.fps = start_fps(&entry.opts);
    if let Some(width) = entry.opts.max_width {
        params.start.max_width = width.clamp(640, 7680);
    }
    if let Some(codec) = entry.opts.codec {
        params.start.codec = match codec {
            Codec::Hevc => rsp_core::control::Codec::Hevc,
            Codec::H264 => rsp_core::control::Codec::H264,
        };
    }
    params.prewarm = Some(core.state::<PrewarmPool>());
    params.audio = entry.opts.audio.unwrap_or(false);
    params.audio_sink = Some(
        entry
            .recorder
            .audio_tee(core.state::<AudioTable>().get_or_create(entry.id)?),
    );
    Ok(params)
}

pub fn host_params(
    core: &Arc<Core>,
    host_id: &str,
    intent: pb::Intent,
) -> Result<SessionParams, SessionSetupError> {
    let identity = core.local_identity()?;
    let settings = settings_snapshot(core);
    let local = local_net_excluding(
        &read_interfaces().map_err(CoreError::from)?,
        &settings.vpn_pools(),
        &settings.work_profile_ifaces,
    );
    let paired = core.store().paired_host(host_id).map_err(CoreError::from)?;
    let last_good = core.store().last_good(host_id).map_err(CoreError::from)?;
    let target = host_target_with_manual(
        core,
        host_id,
        paired.as_ref(),
        last_good.as_ref(),
        &local,
        &settings.intranet_cidrs(),
    )
    .map_err(|error| SessionSetupError {
        code: u32::from(error.code().code()),
        message: error.to_string(),
    })?;
    let mut params = SessionParams::from_shared(target.candidates, target.fp, identity);
    params.net = core.net();
    params.intent = intent;
    params.device_name = core
        .store()
        .identity()
        .map_err(CoreError::from)?
        .ok_or_else(|| CoreError::internal("identity not created"))?
        .device_name;
    params.platform = match core.config().platform {
        DevicePlatform::Ios | DevicePlatform::Ipados => pb::Platform::Ios,
        DevicePlatform::Android => pb::Platform::Android,
        DevicePlatform::Macos => pb::Platform::Macos,
        DevicePlatform::Windows => pb::Platform::Windows,
    };
    params.local_net = local;
    params.intranet_cidrs = target.intranet_cidrs;
    params.observations = target.observations;
    Ok(params)
}

fn host_os(os: pb::Platform) -> Option<String> {
    match os {
        pb::Platform::Macos => Some("macos".into()),
        pb::Platform::Windows => Some("windows".into()),
        pb::Platform::Linux => Some("linux".into()),
        _ => None,
    }
}

/// Remember what a paired Host disclosed in its ServerHello.
/// Only accepted sessions carry the OS and display inventory; pairing and
/// probe hellos (`inventory == false`) keep what a session recorded and only
/// refresh the version, the time, and features nothing has recorded yet.
/// A failed write is logged and never affects the connection.
pub fn record_host_seen(core: &Arc<Core>, host_id: &str, hello: &pb::ServerHello, inventory: bool) {
    let written = write_host_seen(&core.store(), host_id, hello, inventory, core.now_us());
    if let Err(error) = written {
        tracing::warn!(%error, host_id, "recording Host metadata failed");
    }
}

fn write_host_seen(
    store: &Store,
    host_id: &str,
    hello: &pb::ServerHello,
    inventory: bool,
    now_us: u64,
) -> Result<(), StoreError> {
    // Holding the store lock keeps a concurrent forget from being undone.
    if store.paired_host(host_id)?.is_none() {
        return Ok(());
    }
    let host_version = (!hello.host_version.is_empty()).then(|| hello.host_version.clone());
    let row = if inventory {
        HostSeenRow {
            host_id: host_id.to_owned(),
            os: host_os(hello.os()),
            os_version: (!hello.os_version.is_empty()).then(|| hello.os_version.clone()),
            host_version,
            features: Some(hello.features),
            displays: hello
                .displays
                .iter()
                .map(|display| HubDisplay {
                    id: i32::try_from(display.id).unwrap_or(i32::MAX),
                    name: display.name.clone(),
                    width_px: display.width_px,
                    height_px: display.height_px,
                    primary: display.primary,
                })
                .collect(),
            seen_at_us: now_us,
        }
    } else {
        let mut row = store.host_seen(host_id)?.unwrap_or_else(|| HostSeenRow {
            host_id: host_id.to_owned(),
            os: None,
            os_version: None,
            host_version: None,
            features: None,
            displays: Vec::new(),
            seen_at_us: now_us,
        });
        row.host_version = host_version.or(row.host_version);
        if row.features.is_none() && hello.features != 0 {
            row.features = Some(hello.features);
        }
        row.seen_at_us = now_us;
        row
    };
    store.upsert_host_seen(&row)
}

/// The client reports displays in ServerHello, then Streaming before Role.
/// Publish the initial UI snapshot as role -> displays -> streaming; mapping
/// and summary updates still consume every client event in its original order.
#[derive(Default)]
pub struct InitialSessionEvents {
    pub role_seen: bool,
    pub displays: Option<CoreEvent>,
    pub streaming: Option<CoreEvent>,
}

impl InitialSessionEvents {
    pub fn emit(&mut self, core: &Arc<Core>, event: CoreEvent) {
        match event {
            event @ CoreEvent::SessionDisplays { .. } if !self.role_seen => {
                self.displays = Some(event);
            }
            event @ CoreEvent::SessionState {
                phase: SessionPhase::Streaming,
                ..
            } if !self.role_seen => {
                self.streaming = Some(event);
            }
            event @ CoreEvent::SessionRole { .. } => {
                self.role_seen = true;
                core.events().emit(&event);
                for event in self
                    .displays
                    .take()
                    .into_iter()
                    .chain(self.streaming.take())
                {
                    core.events().emit(&event);
                }
            }
            event => core.events().emit(&event),
        }
    }
}

pub async fn run_session(core: &Arc<Core>, entry: Arc<SessionEntry>) {
    // Register before reading the sticky flag: notify_waiters retains no permit.
    let cancelled = entry.cancel.notified();
    tokio::pin!(cancelled);
    cancelled.as_mut().enable();
    if entry.is_cancelled() {
        finish_open(
            core,
            &entry,
            None,
            Some(GoodbyeReason::UserClosed),
            "cancelled".into(),
        );
        return;
    }
    let first_params = match session_params(core, &entry) {
        Ok(params) => params,
        Err(error) => {
            finish_open(core, &entry, Some(error.code), None, error.message);
            return;
        }
    };
    let host_fp = first_params.host_fingerprint;
    let initial_quality = first_params.quality;
    let initial_start = first_params.start.clone();
    let mut first_params = Some(first_params);
    let clock = BgClock::new({
        let core = core.clone();
        Arc::new(move || core.now_us())
    });
    let mut driver = BgDriver::new(settings_snapshot(core).background_grace(), host_fp);
    let mut mapper = SessionMapper::new(entry.id, &entry.host_id);
    let mut resume_token: Option<[u8; TOKEN_LEN]> = None;
    let mut first_connection = true;

    'connections: loop {
        if entry.is_cancelled() {
            finish_open(
                core,
                &entry,
                None,
                Some(GoodbyeReason::UserClosed),
                "cancelled".into(),
            );
            return;
        }
        let is_resume = resume_token.is_some();
        let mut params = if first_connection {
            first_params
                .take()
                .unwrap_or_else(|| unreachable!("first session parameters already consumed"))
        } else {
            match session_params(core, &entry) {
                Ok(params) => params,
                Err(error) => {
                    finish_open(
                        core,
                        &entry,
                        Some(if is_resume { 199 } else { error.code }),
                        None,
                        if is_resume {
                            format!("background resume failed: {}", error.message)
                        } else {
                            error.message
                        },
                    );
                    return;
                }
            }
        };
        if let Some(token) = resume_token.take() {
            params.resume_token = Some(token.to_vec());
            params.quality = initial_quality;
            params.start = initial_start.clone();
            params.display_id = entry.summary().display_id.or(entry.opts.display_id);
        }
        let result = tokio::select! {
            biased;
            _ = &mut cancelled => {
                finish_open(core, &entry, None, Some(GoodbyeReason::UserClosed), "cancelled".into());
                return;
            }
            result = Session::open(params) => result,
        };
        let (handle, mut events) = match result {
            Ok(opened) => opened,
            Err(error) => {
                let message = if is_resume {
                    format!("background resume failed: {error}")
                } else {
                    error.to_string()
                };
                let (code, detail) = if is_resume {
                    (199, None)
                } else {
                    match error {
                        SessionError::Dial { code, detail } => (
                            u32::from(code.code()),
                            (!detail.is_empty()).then_some(detail),
                        ),
                        _ => (199, None),
                    }
                };
                finish_open_detail(core, &entry, Some(code), None, message, detail);
                return;
            }
        };
        mapper.set_dial(dial_from_report(&handle.dial_report()));
        first_connection = false;
        {
            let settings = core.state::<SettingsState>();
            let settings = settings.0.lock().unwrap_or_else(|error| error.into_inner());
            entry.set_handle(Some(handle.clone()));
            handle.set_low_power(settings.low_power);
        }
        let keyframe = handle.clone();
        entry
            .video
            .set_attach_hook(Box::new(move || keyframe.request_keyframe()));
        let mut closing = entry.is_cancelled();
        if closing {
            handle.close();
        }
        let mut initial = InitialSessionEvents::default();
        let mut last_background_pause = tokio::time::Instant::now() - Duration::from_secs(2);
        let mut wake_at: Option<tokio::time::Instant> = None;
        let mut drain_old = false;
        let mut resume_opening = is_resume;
        let flow = loop {
            let now = clock.now();
            let next_wake = driver.next_wake(now).map(|deadline| {
                tokio::time::Instant::now() + deadline.saturating_duration_since(now)
            });
            // Keep the earlier wake across events, so frequent statistics do
            // not continually postpone the one-second wall-clock check.
            wake_at = match (wake_at, next_wake) {
                (Some(previous), Some(next)) => Some(previous.min(next)),
                (_, next) => next,
            };
            let event = tokio::select! {
                biased;
                _ = &mut cancelled, if !closing => {
                    closing = true;
                    handle.close();
                    continue;
                }
                _ = entry.bg_changed.notified() => {
                    if handle.state() == SessionStateKind::Streaming {
                        let on = entry.background.load(std::sync::atomic::Ordering::Acquire);
                        let now = clock.now();
                        driver.set_grace(settings_snapshot(core).background_grace(), now);
                        let flow = driver.set_background(on, now, Some(&handle));
                        if !matches!(flow, BgFlow::Continue) {
                            drain_old = true;
                            break flow;
                        }
                    }
                    continue;
                }
                _ = async {
                    if let Some(deadline) = wake_at {
                        tokio::time::sleep_until(deadline).await;
                    } else {
                        std::future::pending::<()>().await;
                    }
                } => {
                    wake_at = None;
                    let now = clock.now();
                    driver.set_grace(settings_snapshot(core).background_grace(), now);
                    let flow = driver.poll(now, Some(&handle));
                    if !matches!(flow, BgFlow::Continue) {
                        drain_old = true;
                        break flow;
                    }
                    continue;
                }
                event = events.recv() => event,
            };
            let Some(event) = event else {
                if !closing && matches!(driver.state(), BgState::Paused { .. }) {
                    break driver.on_connection_lost(clock.now(), Some(&handle));
                }
                finish_open(
                    core,
                    &entry,
                    Some(199),
                    None,
                    "session event stream closed".into(),
                );
                return;
            };
            if !closing && matches!(driver.state(), BgState::Paused { .. }) {
                match &event {
                    SessionEvent::StateChanged(
                        SessionStateKind::Reconnecting | SessionStateKind::Ended,
                    ) => continue,
                    SessionEvent::Ended {
                        kind: EndReason::ConnectionLost | EndReason::Close(_),
                        ..
                    } => {
                        break driver.on_connection_lost(clock.now(), Some(&handle));
                    }
                    _ => {}
                }
            }
            if resume_opening && !closing {
                match &event {
                    SessionEvent::StateChanged(
                        SessionStateKind::Streaming | SessionStateKind::PendingApproval,
                    ) => {
                        resume_opening = false;
                    }
                    SessionEvent::Error { .. }
                    | SessionEvent::ErrorDetail { .. }
                    | SessionEvent::StateChanged(SessionStateKind::Ended) => continue,
                    SessionEvent::Ended { reason, .. } => {
                        finish_open(
                            core,
                            &entry,
                            Some(199),
                            None,
                            format!("background resume failed: {reason}"),
                        );
                        return;
                    }
                    _ => {}
                }
            }
            match &event {
                SessionEvent::ServerHello(hello) => {
                    record_host_seen(core, &entry.host_id, hello, true);
                }
                SessionEvent::Frame(frame) => {
                    entry.video.deliver_frame(frame);
                    entry.recorder.frame(frame);
                    if frame.frame.keyframe {
                        driver.on_keyframe(clock.now());
                    }
                }
                SessionEvent::StreamConfig(config) => {
                    entry.video.deliver_config(config);
                    entry.recorder.stream_config(config);
                    let background_hold = settings_snapshot(core).background_hold;
                    if entry.background.load(std::sync::atomic::Ordering::Acquire)
                        && background_hold
                        && last_background_pause.elapsed() >= Duration::from_secs(2)
                    {
                        handle.pause();
                        last_background_pause = tokio::time::Instant::now();
                        tracing::info!(
                            session_id = entry.id,
                            "host resumed stream while held in background; pausing again"
                        );
                    }
                }
                SessionEvent::StateChanged(SessionStateKind::Streaming) => {
                    mapper.set_auth_resumed(handle.resumed());
                }
                SessionEvent::Cursor(cursor) => {
                    entry.video.deliver_cursor(cursor);
                    entry.recorder.cursor(cursor);
                }
                SessionEvent::CursorMessage(message) => {
                    entry.video.deliver_cursor_msg(message);
                    entry.recorder.cursor_msg(message);
                }
                SessionEvent::ClockSync { rtt_us, offset_us } => {
                    entry.video.deliver_clock(*rtt_us, *offset_us)
                }
                _ => {}
            }
            let mapped = mapper.map(&event);
            entry.update_summary(|summary| mapper.apply_to_summary(summary));
            for event in mapped {
                initial.emit(core, event);
            }
            if matches!(
                &event,
                SessionEvent::StateChanged(SessionStateKind::Streaming)
            ) && entry.background.load(std::sync::atomic::Ordering::Acquire)
            {
                let flow = match driver.state() {
                    BgState::Foreground => {
                        driver.set_grace(settings_snapshot(core).background_grace(), clock.now());
                        driver.set_background(true, clock.now(), Some(&handle))
                    }
                    BgState::Paused { .. } => {
                        handle.pause();
                        BgFlow::Continue
                    }
                    _ => BgFlow::Continue,
                };
                if !matches!(flow, BgFlow::Continue) {
                    break flow;
                }
            }
            if let SessionEvent::Ended { reason, .. } = &event {
                entry.video.close(reason);
                core.state::<AudioTable>().remove(entry.id);
                core.state::<SessionTable>().remove(entry.id);
                return;
            }
        };
        if drain_old {
            let _ = tokio::time::timeout(Duration::from_secs(2), async {
                while let Some(event) = events.recv().await {
                    if matches!(event, SessionEvent::Ended { .. }) {
                        break;
                    }
                }
            })
            .await;
        }
        entry.set_handle(None);
        match flow {
            BgFlow::Resume { token } => {
                let event = mapper.begin_resume();
                entry.update_summary(|summary| mapper.apply_to_summary(summary));
                InitialSessionEvents::default().emit(core, event);
                resume_token = Some(token);
                continue 'connections;
            }
            BgFlow::ReturnToDevices => {
                finish_open(
                    core,
                    &entry,
                    None,
                    Some(GoodbyeReason::Background),
                    "background resume window expired".into(),
                );
                return;
            }
            BgFlow::Closed | BgFlow::Continue => loop {
                if entry.is_cancelled() {
                    finish_open(
                        core,
                        &entry,
                        None,
                        Some(GoodbyeReason::UserClosed),
                        "cancelled".into(),
                    );
                    return;
                }
                let now = clock.now();
                let next_wake = driver.next_wake(now);
                tokio::select! {
                    biased;
                        _ = &mut cancelled => {
                        finish_open(core, &entry, None, Some(GoodbyeReason::UserClosed), "cancelled".into());
                        return;
                    }
                    _ = entry.bg_changed.notified() => {
                        let on = entry.background.load(std::sync::atomic::Ordering::Acquire);
                        let flow = driver.set_background(on, clock.now(), None);
                        match flow {
                            BgFlow::Resume { token } => {
                                let event = mapper.begin_resume();
                                entry.update_summary(|summary| mapper.apply_to_summary(summary));
                                    InitialSessionEvents::default().emit(core, event);
                                resume_token = Some(token);
                                continue 'connections;
                            }
                            BgFlow::ReturnToDevices => {
                                finish_open(core, &entry, None, Some(GoodbyeReason::Background), "background resume window expired".into());
                                return;
                            }
                            BgFlow::Continue | BgFlow::Closed => {}
                        }
                    }
                    _ = async {
                        if let Some(deadline) = next_wake {
                                tokio::time::sleep(deadline.saturating_duration_since(now)).await;
                        } else {
                            std::future::pending::<()>().await;
                        }
                    } => {
                        let flow = driver.poll(clock.now(), None);
                        if matches!(flow, BgFlow::ReturnToDevices) {
                            finish_open(core, &entry, None, Some(GoodbyeReason::Background), "background resume window expired".into());
                            return;
                        }
                    }
                }
            },
        }
    }
}

pub fn session_entry(core: &Arc<Core>, id: u64) -> Result<Arc<SessionEntry>, CoreError> {
    core.state::<SessionTable>()
        .get(id)
        .ok_or_else(|| CoreError::internal(format!("unknown session {id}")))
}

pub fn with_handle(
    core: &Arc<Core>,
    id: u64,
    command: impl FnOnce(&SessionHandle) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    let entry = session_entry(core, id)?;
    with_entry_handle(&entry, command)
}

pub fn with_entry_handle(
    entry: &SessionEntry,
    command: impl FnOnce(&SessionHandle) -> Result<(), CoreError>,
) -> Result<(), CoreError> {
    match entry.handle() {
        Some(handle) => command(&handle),
        None => {
            tracing::debug!(
                session_id = entry.id,
                "ignoring command while session is connecting"
            );
            Ok(())
        }
    }
}

pub fn close(core: &Arc<Core>, id: u64) {
    if is_adapter_id(id) {
        adapter_route::close(core, id);
        return;
    }
    if let Some(entry) = core.state::<SessionTable>().get(id) {
        entry.cancel();
        core.state::<AudioTable>().remove(id);
        if let Some(handle) = entry.handle() {
            handle.close();
        }
    }
}

pub fn list(core: &Arc<Core>) -> Vec<SessionSummary> {
    core.state::<SessionTable>().summaries()
}

pub fn parse_enum<T: serde::de::DeserializeOwned>(value: &str) -> Result<T, CoreError> {
    serde_json::from_value(serde_json::Value::String(value.to_owned())).map_err(CoreError::internal)
}

pub fn set_quality(core: &Arc<Core>, id: u64, quality: &str) -> Result<(), CoreError> {
    let entry = session_entry(core, id)?;
    let quality: Quality = parse_enum(quality)?;
    with_entry_handle(&entry, |handle| {
        handle.set_quality(quality_to_pb(quality));
        Ok(())
    })
}

pub fn select_display(core: &Arc<Core>, id: u64, display_id: u32) -> Result<(), CoreError> {
    with_handle(core, id, |handle| {
        handle.select_display(display_id);
        Ok(())
    })
}

pub fn request_control(core: &Arc<Core>, id: u64) -> Result<(), CoreError> {
    with_handle(core, id, |handle| {
        handle.request_control();
        Ok(())
    })
}

pub fn respond_control(
    core: &Arc<Core>,
    id: u64,
    requester_device_id: &str,
    allow: bool,
) -> Result<(), CoreError> {
    with_handle(core, id, |handle| {
        handle.respond_control(requester_device_id, allow);
        Ok(())
    })
}

pub fn action(
    core: &Arc<Core>,
    id: u64,
    kind: &str,
    enable: Option<bool>,
) -> Result<(), CoreError> {
    if is_adapter_id(id) {
        return adapter_action(core, id, parse_enum(kind)?);
    }
    let entry = session_entry(core, id)?;
    let kind: SessionActionKind = parse_enum(kind)?;
    with_entry_handle(&entry, |handle| {
        handle.action(action_kind_to_pb(kind), enable.unwrap_or(true));
        Ok(())
    })
}

/// Screen Sharing / Remote Desktop: only Ctrl+Alt+Del has an equivalent, so
/// the keyboard bar and the toolbar reach the same adapter input.
fn adapter_action(core: &Arc<Core>, id: u64, kind: SessionActionKind) -> Result<(), CoreError> {
    match kind {
        SessionActionKind::CtrlAltDel => {
            adapter_session::send_input(core, id, DriverInput::CtrlAltDel)
        }
        SessionActionKind::Lock | SessionActionKind::LockOnEnd => Err(CoreError::unsupported(
            "adapter sessions have no lock action",
        )),
    }
}

pub fn key(core: &Arc<Core>, id: u64, usage: u16, down: bool, mods: u8) -> Result<(), CoreError> {
    if is_adapter_id(id) {
        return adapter_route::key(core, id, usage, down, mods);
    }
    with_handle(core, id, |handle| {
        handle
            .send_input(InputMsg::Key {
                usage,
                down,
                mods: Mods::from_bits(mods),
            })
            .map_err(CoreError::internal)
    })
}

pub fn text(core: &Arc<Core>, id: u64, text: &str) -> Result<(), CoreError> {
    if is_adapter_id(id) {
        return adapter_route::text(core, id, text);
    }
    with_handle(core, id, |handle| {
        let mut remaining = text;
        while !remaining.is_empty() {
            let mut end = remaining.len().min(MAX_TEXT_BYTES);
            while !remaining.is_char_boundary(end) {
                end -= 1;
            }
            handle
                .send_input(InputMsg::Text(remaining[..end].to_owned()))
                .map_err(CoreError::internal)?;
            remaining = &remaining[end..];
        }
        Ok(())
    })
}

pub fn combo(core: &Arc<Core>, id: u64, combo: &str) -> Result<(), CoreError> {
    if is_adapter_id(id) {
        return adapter_route::combo(core, id, combo);
    }
    let entry = session_entry(core, id)?;
    let combo: SessionCombo = parse_enum(combo)?;
    with_entry_handle(&entry, |handle| {
        for input in combo_inputs(combo) {
            handle.send_input(input).map_err(CoreError::internal)?;
        }
        Ok(())
    })
}

pub fn clipboard_send(core: &Arc<Core>, id: u64, text: &str) -> Result<(), CoreError> {
    if is_adapter_id(id) {
        return adapter_route::clipboard_send(core, id, text);
    }
    let entry = session_entry(core, id)?;
    if text.is_empty() {
        return Err(CoreError::unsupported("clipboard text must not be empty"));
    }
    let handle = entry
        .handle()
        .ok_or_else(|| CoreError::network("clipboard.offline: connection unavailable"))?;
    handle
        .send_clipboard_text(text)
        .map_err(map_clipboard_error)?;
    tracing::debug!(
        session_id = entry.id,
        direction = "toHost",
        len = text.len(),
        "clipboard text sent"
    );
    core.events().emit(&CoreEvent::SessionClipboard {
        session_id: id_str(entry.id),
        direction: super::dto_core::ClipboardDirection::ToHost,
        text: text.to_owned(),
        at: core.now_us(),
    });
    Ok(())
}

fn map_clipboard_error(error: ClipboardClientError) -> CoreError {
    match error {
        ClipboardClientError::NotController => {
            CoreError::unsupported("clipboard requires the controller role")
        }
        ClipboardClientError::Disabled => CoreError::unsupported("clipboard is disabled"),
        ClipboardClientError::FeatureUnsupported(kind) => {
            CoreError::unsupported(format!("clipboard {kind} is unsupported by the Host"))
        }
        ClipboardClientError::Codec(error) => {
            CoreError::unsupported(format!("clipboard text is invalid: {error}"))
        }
        ClipboardClientError::Transport(error) => {
            CoreError::network(format!("clipboard.offline: {error}"))
        }
    }
}

pub fn background(core: &Arc<Core>, id: u64, in_background: bool) -> Result<(), CoreError> {
    if is_adapter_id(id) {
        return super::adapters::session::background(core, id, in_background);
    }
    let entry = session_entry(core, id)?;
    entry
        .background
        .store(in_background, std::sync::atomic::Ordering::Release);
    entry.recorder.paused(in_background);
    entry.bg_changed.notify_one();
    Ok(())
}

pub fn prewarm(core: &Arc<Core>, host_ids: Vec<String>) {
    use super::api_diag::{attempt_ms, transport_dto};
    use super::dto_core::{net_path_from_dial, ProbeOutcome};
    use rsp_transport::dial::AttemptResult;

    if !settings_snapshot(core).prewarm_enabled {
        return;
    }
    let Ok(identity) = core.local_identity() else {
        return;
    };
    for host_id in host_ids {
        let c = core.clone();
        let identity = identity.clone();
        core.spawn(async move {
            let warm = async {
                let settings = settings_snapshot(&c);
                let local = local_net_excluding(
                    &read_interfaces()?,
                    &settings.vpn_pools(),
                    &settings.work_profile_ifaces,
                );
                let Some(paired) = c.store().paired_host(&host_id)? else {
                    return Ok(());
                };
                let last_good = c.store().last_good(&host_id)?;
                let target = host_target_with_manual(
                    &c,
                    &host_id,
                    Some(&paired),
                    last_good.as_ref(),
                    &local,
                    &settings.intranet_cidrs(),
                )
                .map_err(CoreError::internal)?;
                let bind: SocketAddr =
                    if target.candidates.iter().all(|candidate| {
                        candidate.addr.ip().is_loopback() && candidate.addr.is_ipv4()
                    }) {
                        ([127, 0, 0, 1], 0).into()
                    } else if target
                        .candidates
                        .iter()
                        .any(|candidate| candidate.addr.is_ipv6())
                    {
                        ([0u16; 8], 0).into()
                    } else {
                        ([0, 0, 0, 0], 0).into()
                    };
                let net = c.net();
                let endpoint = net.shared_quic_endpoint(bind)?;
                let dialer = Arc::new(
                    DefaultDialer::with_net(net, endpoint, &identity)
                        .map_err(CoreError::internal)?,
                );
                let started = std::time::Instant::now();
                let report = c
                    .state::<PrewarmPool>()
                    .warm_report(
                        &plan(target.candidates, &local, &target.intranet_cidrs),
                        dialer,
                        target.fp,
                    )
                    .await
                    .map_err(CoreError::internal)?;
                let attempt = report.attempts.iter().find(|log| {
                    log.result == AttemptResult::Connected
                        && log.transport == Some(report.transport)
                });
                // A fixed non-numeric id never collides with diag.probe ids.
                // Failures stay silent: a cold prewarm does not mean the Host
                // is unreachable.
                c.events().emit(&CoreEvent::ProbeResult {
                    probe_id: "prewarm".to_owned(),
                    host_id: Some(host_id.clone()),
                    path: net_path_from_dial(&report.path.kind),
                    addr: report.path.addr.ip().to_string(),
                    port: report.path.addr.port(),
                    transport: transport_dto(report.transport),
                    outcome: ProbeOutcome::Ok,
                    ms: Some(attempt_ms(attempt, started.elapsed())),
                    reason: None,
                    last: true,
                });
                Ok::<(), CoreError>(())
            }
            .await;
            if let Err(error) = warm {
                tracing::debug!(%error, "session prewarm failed");
            }
        });
    }
}

#[uniffi::export(async_runtime = "tokio")]
pub async fn sessions_open(host_id: String, opts_json: String) -> Result<u64, CoreError> {
    let c = core()?;
    c.clone()
        .run(async move { open(&c, &host_id, serde_json::from_str(&opts_json)?) })
        .await
}

#[uniffi::export]
pub fn sessions_close(id: u64) -> Result<(), CoreError> {
    close(&core()?, id);
    Ok(())
}
#[uniffi::export]
pub fn sessions_list() -> Result<String, CoreError> {
    Ok(serde_json::to_string(&list(&core()?))?)
}
#[uniffi::export]
pub fn sessions_set_quality(id: u64, quality: String) -> Result<(), CoreError> {
    set_quality(&core()?, id, &quality)
}
#[uniffi::export]
pub fn sessions_select_display(id: u64, display_id: u32) -> Result<(), CoreError> {
    select_display(&core()?, id, display_id)
}
#[uniffi::export]
pub fn sessions_create_virtual_display(id: u64, opts_json: String) -> Result<u32, CoreError> {
    super::vdisplay::create(&core()?, id, &opts_json)
}
#[uniffi::export]
pub fn sessions_release_virtual_display(id: u64, display_id: u32) -> Result<(), CoreError> {
    super::vdisplay::release(&core()?, id, display_id)
}

#[uniffi::export]
pub fn sessions_request_control(id: u64) -> Result<(), CoreError> {
    request_control(&core()?, id)
}
#[uniffi::export]
pub fn sessions_respond_control(
    id: u64,
    requester_device_id: String,
    allow: bool,
) -> Result<(), CoreError> {
    respond_control(&core()?, id, &requester_device_id, allow)
}
#[uniffi::export]
pub fn sessions_action(id: u64, kind: String, enable: Option<bool>) -> Result<(), CoreError> {
    action(&core()?, id, &kind, enable)
}
#[uniffi::export]
pub fn sessions_key(id: u64, usage: u16, down: bool, mods: u8) -> Result<(), CoreError> {
    key(&core()?, id, usage, down, mods)
}
#[uniffi::export]
pub fn sessions_text(id: u64, text: String) -> Result<(), CoreError> {
    self::text(&core()?, id, &text)
}
#[uniffi::export]
pub fn sessions_combo(id: u64, combo: String) -> Result<(), CoreError> {
    self::combo(&core()?, id, &combo)
}
#[uniffi::export]
pub fn sessions_clipboard_send(id: u64, text: String) -> Result<(), CoreError> {
    clipboard_send(&core()?, id, &text)
}
#[uniffi::export]
pub fn sessions_background(id: u64, in_background: bool) -> Result<(), CoreError> {
    background(&core()?, id, in_background)
}
#[uniffi::export]
pub fn sessions_prewarm(host_ids: Vec<String>) -> Result<(), CoreError> {
    prewarm(&core()?, host_ids);
    Ok(())
}

/// 会话中开关远端声音。声音接线由 AU2E 实现。
#[uniffi::export]
pub fn sessions_set_audio(id: u64, enabled: bool) -> Result<(), CoreError> {
    audio::set_audio(&core()?, id, enabled)
}
