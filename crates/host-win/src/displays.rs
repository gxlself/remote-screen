#![cfg(windows)]

//! Thin Win32 display enumeration and change notification layer.

use std::mem::size_of;
use std::thread;
use std::time::Duration;

use host_core::platform::DisplayInfo;
use tokio::sync::mpsc::{error::TrySendError, Sender};
use tracing::warn;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{GetLastError, ERROR_ACCESS_DENIED, LPARAM, RECT};
use windows::Win32::Graphics::Gdi::{
    EnumDisplayDevicesW, EnumDisplayMonitors, EnumDisplaySettingsW, GetMonitorInfoW, DEVMODEW,
    DISPLAY_DEVICEW, ENUM_CURRENT_SETTINGS, HDC, HMONITOR, MONITORINFO, MONITORINFOEXW,
};
use windows::Win32::UI::HiDpi::{
    GetDpiForMonitor, SetProcessDpiAwarenessContext, DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2,
    MDT_EFFECTIVE_DPI,
};
use windows::Win32::UI::WindowsAndMessaging::MONITORINFOF_PRIMARY;

use crate::{vdisplay_names, vdisplay_plan};
use host_core::display_model::{displays_changed, to_display_infos, MonitorRecord, RectPx};

/// Set the process context needed for physical-pixel monitor coordinates.
pub fn ensure_dpi_awareness() {
    // SAFETY: the API only changes this process's DPI awareness context.
    let result =
        unsafe { SetProcessDpiAwarenessContext(DPI_AWARENESS_CONTEXT_PER_MONITOR_AWARE_V2) };
    if result.is_ok() {
        return;
    }

    // SAFETY: GetLastError reads the thread-local status from the immediately
    // preceding DPI-awareness call.
    let error = unsafe { GetLastError() };
    if error != ERROR_ACCESS_DENIED {
        warn!(?error, "failed to set per-monitor-v2 DPI awareness");
    }
}

/// Enumerate the monitors visible to the current Windows session.
pub fn enumerate() -> Vec<MonitorRecord> {
    let mut records: Vec<MonitorRecord> = Vec::new();
    let data = LPARAM((&mut records as *mut Vec<MonitorRecord>) as isize);

    // SAFETY: `data` points to `records`, which remains alive until the native
    // enumeration returns.  The callback does not retain the pointer.
    let result = unsafe { EnumDisplayMonitors(None, None, Some(enum_monitor), data) };
    if !result.as_bool() {
        warn!("EnumDisplayMonitors failed");
    }
    let names = vdisplay_names::edid_names();
    for record in &mut records {
        record.friendly_name = vdisplay_plan::merge_name(
            names.get(&record.device_name).map(String::as_str),
            &record.friendly_name,
        );
    }
    records
}

/// Poll monitor state and report only changes to the supplied async channel.
pub fn watch(tx: Sender<Vec<DisplayInfo>>, interval: Duration) {
    thread::spawn(move || {
        let mut previous = to_display_infos(&enumerate());
        loop {
            if tx.is_closed() {
                break;
            }
            thread::sleep(interval);
            if tx.is_closed() {
                break;
            }

            let current = to_display_infos(&enumerate());
            if displays_changed(&previous, &current) {
                match tx.try_send(current.clone()) {
                    Ok(()) | Err(TrySendError::Full(_)) => {}
                    Err(TrySendError::Closed(_)) => break,
                }
            }
            previous = current;
        }
    });
}

unsafe extern "system" fn enum_monitor(
    monitor: HMONITOR,
    _dc: HDC,
    _clip: *mut RECT,
    data: LPARAM,
) -> BOOL {
    if data.0 == 0 {
        return BOOL(0);
    }

    // SAFETY: `enumerate` passes a valid mutable Vec pointer and keeps it alive
    // for the duration of EnumDisplayMonitors.
    let records = unsafe { &mut *(data.0 as *mut Vec<MonitorRecord>) };
    records.push(read_monitor(monitor));
    BOOL(1)
}

fn read_monitor(monitor: HMONITOR) -> MonitorRecord {
    let mut monitor_info = MONITORINFOEXW::default();
    monitor_info.monitorInfo.cbSize = size_of::<MONITORINFOEXW>() as u32;

    // SAFETY: `monitor_info` is a writable MONITORINFOEXW whose prefix is the
    // MONITORINFO expected by GetMonitorInfoW; the monitor handle came from
    // EnumDisplayMonitors.
    let info_ok =
        unsafe { GetMonitorInfoW(monitor, &mut monitor_info.monitorInfo as *mut MONITORINFO) }
            .as_bool();

    if !info_ok {
        warn!("GetMonitorInfoW failed while enumerating a monitor");
    }

    let (device_name, rect, primary) = if info_ok {
        (
            utf16_string(&monitor_info.szDevice),
            rect_from_win32(monitor_info.monitorInfo.rcMonitor),
            monitor_info.monitorInfo.dwFlags & MONITORINFOF_PRIMARY != 0,
        )
    } else {
        (
            String::new(),
            RectPx {
                left: 0,
                top: 0,
                width: 0,
                height: 0,
            },
            false,
        )
    };

    let dpi = effective_dpi(monitor);
    let (refresh_hz, orientation) = if device_name.is_empty() {
        (0, 0)
    } else {
        current_settings(&monitor_info.szDevice)
    };
    let friendly_name = if device_name.is_empty() {
        String::new()
    } else {
        friendly_name(&monitor_info.szDevice)
    };

    MonitorRecord {
        device_name,
        friendly_name,
        rect,
        primary,
        dpi,
        refresh_hz,
        orientation,
    }
}

fn effective_dpi(monitor: HMONITOR) -> u32 {
    let mut dpi_x = 0;
    let mut dpi_y = 0;
    // SAFETY: the monitor handle is supplied by EnumDisplayMonitors and both
    // output pointers refer to initialized local u32 values.
    let result = unsafe { GetDpiForMonitor(monitor, MDT_EFFECTIVE_DPI, &mut dpi_x, &mut dpi_y) };
    if result.is_ok() {
        dpi_x
    } else {
        0
    }
}

fn current_settings(device_name: &[u16; 32]) -> (u32, u32) {
    let mut settings = DEVMODEW {
        dmSize: size_of::<DEVMODEW>() as u16,
        ..Default::default()
    };
    // SAFETY: `device_name` is a null-terminated fixed-size name returned by
    // GetMonitorInfoW, and `settings` is a writable DEVMODEW with dmSize set.
    let result = unsafe {
        EnumDisplaySettingsW(
            PCWSTR(device_name.as_ptr()),
            ENUM_CURRENT_SETTINGS,
            &mut settings,
        )
    };
    if !result.as_bool() {
        return (0, 0);
    }

    // SAFETY: EnumDisplaySettingsW initialized the DEVMODEW union for the
    // current display mode.
    let orientation = unsafe { settings.Anonymous1.Anonymous2.dmDisplayOrientation.0 };
    (settings.dmDisplayFrequency, orientation)
}

fn friendly_name(device_name: &[u16; 32]) -> String {
    let mut display_device = DISPLAY_DEVICEW {
        cb: size_of::<DISPLAY_DEVICEW>() as u32,
        ..Default::default()
    };
    // SAFETY: `device_name` is a null-terminated fixed-size name and
    // `display_device` is a writable DISPLAY_DEVICEW with cb initialized.
    let result =
        unsafe { EnumDisplayDevicesW(PCWSTR(device_name.as_ptr()), 0, &mut display_device, 0) };
    if result.as_bool() {
        utf16_string(&display_device.DeviceString)
    } else {
        String::new()
    }
}

fn utf16_string(value: &[u16]) -> String {
    let length = match value.iter().position(|character| *character == 0) {
        Some(length) => length,
        None => value.len(),
    };
    String::from_utf16_lossy(&value[..length])
}

fn rect_from_win32(rect: RECT) -> RectPx {
    RectPx {
        left: rect.left,
        top: rect.top,
        width: extent(rect.left, rect.right),
        height: extent(rect.top, rect.bottom),
    }
}

fn extent(start: i32, end: i32) -> u32 {
    let value = i64::from(end) - i64::from(start);
    if value <= 0 {
        0
    } else if value > i64::from(u32::MAX) {
        u32::MAX
    } else {
        value as u32
    }
}
