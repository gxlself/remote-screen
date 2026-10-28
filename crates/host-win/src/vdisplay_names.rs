#![cfg(windows)]

//! QueryDisplayConfig: GDI device name to EDID monitor name.

use std::collections::HashMap;
use std::mem::size_of;

use windows::Win32::Devices::Display::{
    DisplayConfigGetDeviceInfo, GetDisplayConfigBufferSizes, QueryDisplayConfig,
    DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME, DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
    DISPLAYCONFIG_DEVICE_INFO_HEADER, DISPLAYCONFIG_SOURCE_DEVICE_NAME,
    DISPLAYCONFIG_TARGET_DEVICE_NAME, QDC_ONLY_ACTIVE_PATHS,
};
use windows::Win32::Foundation::{ERROR_INSUFFICIENT_BUFFER, ERROR_SUCCESS};

/// Return the active-path mapping from `\\.\DISPLAYn` to the monitor's EDID name.
pub fn edid_names() -> HashMap<String, String> {
    for _attempt in 0..3 {
        let mut path_count = 0;
        let mut mode_count = 0;
        // SAFETY: both counts point to writable locals and the flag is a valid
        // QueryDisplayConfig mode.
        let result = unsafe {
            GetDisplayConfigBufferSizes(QDC_ONLY_ACTIVE_PATHS, &mut path_count, &mut mode_count)
        };
        if result == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if result != ERROR_SUCCESS {
            return HashMap::new();
        }

        let mut paths = vec![
            windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO::default();
            path_count as usize
        ];
        let mut modes = vec![
            windows::Win32::Devices::Display::DISPLAYCONFIG_MODE_INFO::default();
            mode_count as usize
        ];
        // SAFETY: vectors have exactly the capacities reported by the preceding
        // size query and remain alive for the duration of the call.
        let result = unsafe {
            QueryDisplayConfig(
                QDC_ONLY_ACTIVE_PATHS,
                &mut path_count,
                paths.as_mut_ptr(),
                &mut mode_count,
                modes.as_mut_ptr(),
                None,
            )
        };
        if result == ERROR_INSUFFICIENT_BUFFER {
            continue;
        }
        if result != ERROR_SUCCESS {
            return HashMap::new();
        }

        let mut names = HashMap::new();
        for path in paths.into_iter().take(path_count as usize) {
            let Some(source) = source_name(&path) else {
                continue;
            };
            let Some(target) = target_name(&path) else {
                continue;
            };
            names.insert(source, target);
        }
        return names;
    }
    HashMap::new()
}

fn source_name(path: &windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut name = DISPLAYCONFIG_SOURCE_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_SOURCE_NAME,
            size: size_of::<DISPLAYCONFIG_SOURCE_DEVICE_NAME>() as u32,
            adapterId: path.sourceInfo.adapterId,
            id: path.sourceInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: `name` is a correctly sized source-name packet with identifiers
    // copied from the active path.
    let result = unsafe {
        DisplayConfigGetDeviceInfo(&mut name.header as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER)
    };
    (result == ERROR_SUCCESS.0 as i32).then(|| utf16_string(&name.viewGdiDeviceName))
}

fn target_name(path: &windows::Win32::Devices::Display::DISPLAYCONFIG_PATH_INFO) -> Option<String> {
    let mut name = DISPLAYCONFIG_TARGET_DEVICE_NAME {
        header: DISPLAYCONFIG_DEVICE_INFO_HEADER {
            r#type: DISPLAYCONFIG_DEVICE_INFO_GET_TARGET_NAME,
            size: size_of::<DISPLAYCONFIG_TARGET_DEVICE_NAME>() as u32,
            adapterId: path.targetInfo.adapterId,
            id: path.targetInfo.id,
        },
        ..Default::default()
    };
    // SAFETY: `name` is a correctly sized target-name packet with identifiers
    // copied from the active path.
    let result = unsafe {
        DisplayConfigGetDeviceInfo(&mut name.header as *mut DISPLAYCONFIG_DEVICE_INFO_HEADER)
    };
    (result == ERROR_SUCCESS.0 as i32).then(|| utf16_string(&name.monitorFriendlyDeviceName))
}

fn utf16_string(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}
