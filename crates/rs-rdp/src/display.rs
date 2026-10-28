//! Display control: dynamic resolution, monitor layout and
//! changing the monitor count at runtime.
//!
//! The display-control DVC is registered for every connection. A server may
//! choose not to open it, or may open it without sending its capabilities; in
//! both cases resize requests are deliberately ignored and the caller keeps
//! scaling the existing framebuffer.

use std::collections::HashMap;
use std::sync::{Mutex, OnceLock};

use ironrdp_connector::{ClientConnector, Config};
use ironrdp_displaycontrol::client::DisplayControlClient;
use ironrdp_displaycontrol::pdu::{
    DeviceScaleFactor, DisplayControlCapabilities, DisplayControlMonitorLayout, MonitorLayoutEntry,
    MonitorOrientation,
};
use ironrdp_dvc::DrdynvcClient;
use ironrdp_pdu::gcc::{ClientMonitorData, Monitor, MonitorFlags};
use ironrdp_session::ActiveStage;
use tracing::{debug, info};

use crate::ext::ExtCtx;
use crate::types::{RdpConfig, RdpError, Rect};

/// The active dimensions are kept by the connection's sink identity because
/// `ActiveStage` intentionally owns only protocol state and does not expose
/// its decoded framebuffer dimensions. `configure` runs once per connection,
/// replacing any stale entry if an allocator reuses an Arc address.
struct DisplayState {
    width: u32,
    height: u32,
    scale_percent: u32,
    monitor_count: u8,
    layout: Vec<Rect>,
    // Activation can deliver the server's layout before the first pump.
    pending_layouts: Option<Vec<Vec<Rect>>>,
    /// The server opened display control on this connection at some point.
    display_control: bool,
    /// A monitor count asked for while a reactivation had the channel closed.
    pending_monitors: Option<u8>,
}

static DISPLAY_STATES: OnceLock<Mutex<HashMap<usize, DisplayState>>> = OnceLock::new();

fn states() -> &'static Mutex<HashMap<usize, DisplayState>> {
    DISPLAY_STATES.get_or_init(|| Mutex::new(HashMap::new()))
}

fn sink_key(ctx: &ExtCtx) -> usize {
    std::sync::Arc::as_ptr(&ctx.sink).cast::<()>() as usize
}

fn with_states<R>(f: impl FnOnce(&mut HashMap<usize, DisplayState>) -> R) -> R {
    let mut guard = match states().lock() {
        Ok(guard) => guard,
        Err(poisoned) => poisoned.into_inner(),
    };
    f(&mut guard)
}

fn adjusted_size(width: u16, height: u16) -> (u32, u32) {
    MonitorLayoutEntry::adjust_display_size(u32::from(width), u32::from(height))
}

fn adjusted_scale(scale_percent: u32) -> u32 {
    MonitorLayoutEntry::adjust_desktop_scale_factor(scale_percent)
}

fn protocol_error(error: impl std::fmt::Display) -> RdpError {
    RdpError::Protocol(format!("display control: {error}"))
}

/// Before `ClientConnector::new`: connector settings this feature needs.
pub fn configure(cfg: &RdpConfig, ctx: &ExtCtx, config: &mut Config) {
    let (width, height) = adjusted_size(cfg.width, cfg.height);
    let monitor_count = cfg.monitors.clamp(1, 4);
    let monitors = (0..monitor_count)
        .map(|index| {
            let left = i32::from(index) * width as i32;
            Monitor {
                left,
                top: 0,
                right: left + width as i32 - 1,
                bottom: height as i32 - 1,
                flags: if index == 0 {
                    MonitorFlags::PRIMARY
                } else {
                    MonitorFlags::empty()
                },
            }
        })
        .collect::<Vec<_>>();
    let layout = monitors
        .iter()
        .map(|monitor| Rect {
            x: monitor.left as u16,
            y: 0,
            w: width as u16,
            h: height as u16,
        })
        .collect::<Vec<_>>();
    config.monitor_layout = if monitor_count > 1 {
        config.desktop_size.width = (width * u32::from(monitor_count)) as u16;
        config.desktop_size.height = height as u16;
        Some(ClientMonitorData { monitors })
    } else {
        None
    };
    info!(monitor_count, ?layout, "RDP monitor layout declared");
    with_states(|states| {
        states.insert(
            sink_key(ctx),
            DisplayState {
                width: width * u32::from(monitor_count),
                height,
                scale_percent: adjusted_scale(cfg.scale_percent),
                monitor_count,
                pending_layouts: Some(vec![layout.clone()]),
                layout,
                display_control: false,
                pending_monitors: None,
            },
        );
    });
    ctx.wake.notify_one();
}

/// Dynamic virtual channels this feature registers.
pub fn dynamic_channels(cfg: &RdpConfig, ctx: &ExtCtx, drdynvc: DrdynvcClient) -> DrdynvcClient {
    let _ = (cfg, ctx);
    // Keep this channel registered even when a server does not advertise
    // display control. ActiveStage::encode_resize treats an unopened or
    // capability-less channel as unavailable.
    drdynvc.with_dynamic_channel(DisplayControlClient::new(|_| Ok(Vec::new())))
}

/// Static virtual channels this feature registers.
pub fn static_channels(
    cfg: &RdpConfig,
    ctx: &ExtCtx,
    connector: ClientConnector,
) -> ClientConnector {
    let _ = (cfg, ctx);
    connector
}

/// After every `ActiveStage::process` batch and every `ctx.wake`: bytes to send.
pub fn pump(stage: &mut ActiveStage, ctx: &ExtCtx) -> Result<Vec<u8>, RdpError> {
    let open = stage.display_control_capabilities().is_some();
    let (layouts, monitors) = with_states(|states| {
        states
            .get_mut(&sink_key(ctx))
            .map_or((None, None), |state| {
                state.display_control |= open;
                let monitors = if open {
                    state.pending_monitors.take()
                } else {
                    None
                };
                (state.pending_layouts.take(), monitors)
            })
    });
    for layout in layouts.into_iter().flatten() {
        ctx.sink.monitors(&layout);
    }
    let Some(count) = monitors else {
        return Ok(Vec::new());
    };
    Ok(set_monitors(stage, ctx, count)?.unwrap_or_else(|| {
        debug!(count, "dropping a monitor count the server cannot take");
        Vec::new()
    }))
}

fn requested_layout(
    monitor_count: u8,
    width: u32,
    height: u32,
    scale: u32,
) -> Result<DisplayControlMonitorLayout, RdpError> {
    if monitor_count == 1 {
        return DisplayControlMonitorLayout::new_single_primary_monitor(
            width,
            height,
            Some(scale),
            None,
        )
        .map_err(protocol_error);
    }
    let orientation = if width < height {
        MonitorOrientation::Portrait
    } else {
        MonitorOrientation::Landscape
    };
    let monitors = (0..monitor_count)
        .map(|index| {
            let entry = if index == 0 {
                MonitorLayoutEntry::new_primary(width, height)
            } else {
                MonitorLayoutEntry::new_secondary(width, height)
            }
            .map_err(protocol_error)?;
            entry
                .with_orientation(orientation)
                .with_device_scale_factor(DeviceScaleFactor::from_desktop_scale_factor(scale))
                .with_position(i32::from(index) * width as i32, 0)
                .and_then(|entry| entry.with_desktop_scale_factor(scale))
                .map_err(protocol_error)
        })
        .collect::<Result<Vec<_>, _>>()?;
    DisplayControlMonitorLayout::new(&monitors).map_err(protocol_error)
}

/// `RdpCommand::Resize`: request the new size for every monitor; bytes to send
/// (empty while the server has not opened display control).
pub fn resize(
    stage: &mut ActiveStage,
    ctx: &ExtCtx,
    width: u16,
    height: u16,
    scale_percent: u32,
) -> Result<Vec<u8>, RdpError> {
    let Some(capabilities) = stage.display_control_capabilities() else {
        debug!("ignoring resize: display control capabilities are unavailable");
        return Ok(Vec::new());
    };

    let (requested_width, requested_height) = adjusted_size(width, height);
    let requested_scale = adjusted_scale(scale_percent);
    let key = sink_key(ctx);
    let monitor_count =
        with_states(|states| states.get(&key).map_or(1, |state| state.monitor_count));
    send_layout(
        stage,
        &capabilities,
        key,
        monitor_count,
        requested_width,
        requested_height,
        requested_scale,
    )
}

/// `RdpCommand::Monitors`: `count` monitors side by side, each the size of the
/// current primary; bytes to send. None = the server cannot change the count
/// at runtime (no display control, or fewer monitors than asked for), so the
/// caller connects again with the new count.
pub fn set_monitors(
    stage: &mut ActiveStage,
    ctx: &ExtCtx,
    count: u8,
) -> Result<Option<Vec<u8>>, RdpError> {
    let count = count.clamp(1, 4);
    let key = sink_key(ctx);
    let Some((current, width, height, scale)) = with_states(|states| {
        states.get(&key).map(|state| {
            let (width, height) = state.layout.first().map_or(
                (state.width / u32::from(state.monitor_count), state.height),
                |primary| (u32::from(primary.w), u32::from(primary.h)),
            );
            (state.monitor_count, width, height, state.scale_percent)
        })
    }) else {
        return Ok(Some(Vec::new()));
    };
    if current == count {
        with_states(|states| {
            if let Some(state) = states.get_mut(&key) {
                state.pending_monitors = None;
            }
        });
        return Ok(Some(Vec::new()));
    }
    let (width, height) = MonitorLayoutEntry::adjust_display_size(width, height);
    let Some(capabilities) = stage.display_control_capabilities() else {
        // After a reactivation the server reopens the channel; wait for it.
        let waiting = with_states(|states| {
            states.get_mut(&key).is_some_and(|state| {
                if state.display_control {
                    state.pending_monitors = Some(count);
                }
                state.display_control
            })
        });
        if waiting {
            debug!(count, "monitor count waits for display control to reopen");
            return Ok(Some(Vec::new()));
        }
        info!(count, "RDP monitor count needs a new connection");
        return Ok(None);
    };
    if capabilities.max_num_monitors() < u32::from(count) {
        info!(
            count,
            "RDP server takes fewer monitors at runtime; reconnecting"
        );
        return Ok(None);
    }
    if requested_layout(count, width, height, scale)?
        .fit_to_capabilities(&capabilities)
        .is_err()
    {
        info!(
            count,
            "RDP monitor layout exceeds the server limits; reconnecting"
        );
        return Ok(None);
    }
    with_states(|states| {
        if let Some(state) = states.get_mut(&key) {
            state.monitor_count = count;
            state.pending_monitors = None;
        }
    });
    send_layout(stage, &capabilities, key, count, width, height, scale).map(Some)
}

fn send_layout(
    stage: &mut ActiveStage,
    capabilities: &DisplayControlCapabilities,
    key: usize,
    monitor_count: u8,
    requested_width: u32,
    requested_height: u32,
    requested_scale: u32,
) -> Result<Vec<u8>, RdpError> {
    let requested_layout = requested_layout(
        monitor_count,
        requested_width,
        requested_height,
        requested_scale,
    )?;
    let fitted_layout = requested_layout
        .fit_to_capabilities(capabilities)
        .map_err(protocol_error)?;
    let (effective_width, effective_height) = fitted_layout.desktop_size();
    let effective_layout = fitted_layout
        .monitors()
        .iter()
        .map(|monitor| {
            let (x, y) = monitor.position().unwrap_or_default();
            let (w, h) = monitor.dimensions();
            Rect {
                x: x as u16,
                y: y as u16,
                w: w as u16,
                h: h as u16,
            }
        })
        .collect::<Vec<_>>();

    let unchanged = with_states(|states| {
        states.get(&key).is_some_and(|current| {
            current.width == effective_width
                && current.height == effective_height
                && current.scale_percent == requested_scale
                && current.layout == effective_layout
        })
    });
    if unchanged {
        return Ok(Vec::new());
    }

    // Let ActiveStage perform the actual DVC framing. It repeats the
    // capability fitting above, so the encoded layout is the same one whose
    // dimensions we recorded for the no-op check.
    let encoded = if monitor_count == 1 {
        stage
            .encode_resize(
                requested_width,
                requested_height,
                Some(requested_scale),
                None,
            )
            .ok_or_else(|| protocol_error("display control became unavailable"))?
            .map_err(protocol_error)?
    } else {
        let batch = stage
            .prepare_monitor_layout(&requested_layout)
            .ok_or_else(|| protocol_error("display control became unavailable"))?
            .map_err(protocol_error)?;
        stage
            .encode_dvc_messages(batch.into_messages())
            .map_err(protocol_error)?
    };

    with_states(|states| {
        if let Some(state) = states.get_mut(&key) {
            state.width = effective_width;
            state.height = effective_height;
            state.scale_percent = requested_scale;
            state.layout = effective_layout;
        }
    });
    info!(
        width = effective_width,
        height = effective_height,
        scale_percent = requested_scale,
        monitor_count,
        "RDP display layout sent"
    );
    Ok(encoded)
}

/// `ActiveStageOutput::MonitorLayout` from the server.
pub fn server_layout(ctx: &ExtCtx, monitors: &[Monitor]) {
    info!(?monitors, "RDP monitor layout received");
    let Some((layout, width, height)) = normalized_layout(monitors) else {
        debug!("ignoring invalid or unrepresentable RDP monitor layout");
        return;
    };
    let queued = with_states(|states| {
        if let Some(state) = states.get_mut(&sink_key(ctx)) {
            state.width = width;
            state.height = height;
            state.layout.clone_from(&layout);
            if let Some(pending) = &mut state.pending_layouts {
                pending.push(layout.clone());
                return true;
            }
        }
        false
    });
    if !queued {
        ctx.sink.monitors(&layout);
    }
}

fn normalized_layout(monitors: &[Monitor]) -> Option<(Vec<Rect>, u32, u32)> {
    let (left, top, right, bottom) =
        monitors
            .iter()
            .fold(None::<(i32, i32, i32, i32)>, |bounds, monitor| {
                Some(match bounds {
                    None => (monitor.left, monitor.top, monitor.right, monitor.bottom),
                    Some((left, top, right, bottom)) => (
                        left.min(monitor.left),
                        top.min(monitor.top),
                        right.max(monitor.right),
                        bottom.max(monitor.bottom),
                    ),
                })
            })?;
    let width = u16::try_from(i64::from(right) - i64::from(left) + 1).ok()?;
    let height = u16::try_from(i64::from(bottom) - i64::from(top) + 1).ok()?;
    let mut ordered = monitors.iter().collect::<Vec<_>>();
    ordered.sort_by_key(|monitor| !monitor.flags.contains(MonitorFlags::PRIMARY));
    let layout = ordered
        .into_iter()
        .map(|monitor| {
            let rect = Rect {
                x: u16::try_from(i64::from(monitor.left) - i64::from(left)).ok()?,
                y: u16::try_from(i64::from(monitor.top) - i64::from(top)).ok()?,
                w: u16::try_from(i64::from(monitor.right) - i64::from(monitor.left) + 1).ok()?,
                h: u16::try_from(i64::from(monitor.bottom) - i64::from(monitor.top) + 1).ok()?,
            };
            (rect.w > 0 && rect.h > 0).then_some(rect)
        })
        .collect::<Option<Vec<_>>>()?;
    Some((layout, u32::from(width), u32::from(height)))
}
