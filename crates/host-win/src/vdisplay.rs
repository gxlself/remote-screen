#![cfg(windows)]

//! Windows virtual display backend: IddCx driver IOCTL client and keepalive.

use std::collections::BTreeSet;
use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::sync::{
    atomic::{AtomicBool, Ordering},
    Arc, Mutex, OnceLock,
};
use std::thread;
use std::time::Duration;

use host_core::vdisplay::{VdCaps, VdCreated, VdError, VdMode, VirtualDisplayBackend};
use tracing::{debug, info, warn};
use windows::core::{GUID, PCWSTR};
use windows::Win32::Devices::DeviceAndDriverInstallation::{
    CM_Get_Device_Interface_ListW, CM_Get_Device_Interface_List_SizeW,
    CM_GET_DEVICE_INTERFACE_LIST_PRESENT, CR_SUCCESS,
};
use windows::Win32::Foundation::{GetLastError, GENERIC_READ, GENERIC_WRITE, HANDLE};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, FILE_ATTRIBUTE_NORMAL, FILE_SHARE_READ, FILE_SHARE_WRITE, OPEN_EXISTING,
};
use windows::Win32::System::IO::DeviceIoControl;

use crate::vdisplay_edid::{alloc_id, monitor_name};
use crate::vdisplay_ioctl::{
    decode_add_out, decode_info, decode_remove_all_out, encode_add, encode_remove, map_win32_error,
    version_in, INTERFACE_GUID_PARTS, IOCTL_ADD, IOCTL_GET_INFO, IOCTL_PING, IOCTL_REMOVE,
    IOCTL_REMOVE_ALL,
};

const ERROR_FILE_NOT_FOUND_CODE: u32 = 2;
const ERROR_INVALID_HANDLE_CODE: u32 = 6;
const ERROR_DEVICE_NOT_CONNECTED_CODE: u32 = 1167;
const ADD_OUT_LEN: usize = 8;
const INFO_LEN: usize = 48;
const REMOVE_ALL_OUT_LEN: usize = 4;

/// One process-wide client for the IddCx control device.
pub struct WinIddBackend {
    handle: Mutex<Option<OwnedHandle>>,
    live: Mutex<BTreeSet<u32>>,
    keepalive_started: AtomicBool,
}

static INSTANCE: OnceLock<Arc<WinIddBackend>> = OnceLock::new();

fn instance() -> Arc<WinIddBackend> {
    INSTANCE
        .get_or_init(|| Arc::new(WinIddBackend::new()))
        .clone()
}

/// Return the service-process singleton.  The backend is present even when the
/// driver is unavailable; `caps` then reports `None`.
pub fn backend() -> Option<Arc<dyn VirtualDisplayBackend>> {
    Some(instance())
}

impl WinIddBackend {
    fn new() -> Self {
        Self {
            handle: Mutex::new(None),
            live: Mutex::new(BTreeSet::new()),
            keepalive_started: AtomicBool::new(false),
        }
    }

    fn start_keepalive(&self) {
        if self
            .keepalive_started
            .compare_exchange(false, true, Ordering::AcqRel, Ordering::Acquire)
            .is_err()
        {
            return;
        }
        let backend = instance();
        let result = thread::Builder::new()
            .name("rs-vd-keepalive".to_owned())
            .spawn(move || backend.keepalive_loop());
        if result.is_err() {
            self.keepalive_started.store(false, Ordering::Release);
            warn!("failed to start virtual display keepalive thread");
        }
    }

    fn keepalive_loop(&self) {
        let mut previous: Option<bool> = None;
        loop {
            thread::sleep(Duration::from_secs(1));
            let ok = self.ioctl(IOCTL_PING, &version_in(), &mut []).is_ok();
            if previous.is_some() && previous != Some(ok) {
                if ok {
                    warn!("virtual display keepalive recovered");
                } else {
                    warn!("virtual display keepalive failed");
                }
            }
            previous = Some(ok);
        }
    }

    fn ioctl(&self, code: u32, input: &[u8], output: &mut [u8]) -> Result<usize, u32> {
        let input_len = u32::try_from(input.len()).map_err(|_| 87u32)?;
        let output_len = u32::try_from(output.len()).map_err(|_| 87u32)?;
        let mut handle = self
            .handle
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if handle.is_none() {
            *handle = Some(open_device()?);
        }
        let Some(device) = handle.as_ref() else {
            return Err(ERROR_FILE_NOT_FOUND_CODE);
        };

        let mut returned = 0u32;
        // SAFETY: `device` owns a valid handle; input/output pointers refer to
        // live slices for the synchronous call and their lengths are bounded by
        // u32 as checked above.
        let result = unsafe {
            DeviceIoControl(
                HANDLE(device.as_raw_handle()),
                code,
                (!input.is_empty()).then_some(input.as_ptr().cast()),
                input_len,
                (!output.is_empty()).then_some(output.as_mut_ptr().cast()),
                output_len,
                Some(&mut returned),
                None,
            )
        };
        if result.is_err() {
            let error = unsafe { GetLastError().0 };
            if error == ERROR_INVALID_HANDLE_CODE || error == ERROR_DEVICE_NOT_CONNECTED_CODE {
                *handle = None;
            }
            return Err(error);
        }
        Ok(returned as usize)
    }

    fn map_decode_error(error: impl std::fmt::Display) -> VdError {
        VdError::Failed(error.to_string())
    }
}

impl VirtualDisplayBackend for WinIddBackend {
    fn caps(&self) -> Option<VdCaps> {
        let mut output = [0u8; INFO_LEN];
        let bytes = match self.ioctl(IOCTL_GET_INFO, &[], &mut output) {
            Ok(bytes) => bytes,
            Err(error) => {
                debug!(error_code = error, "virtual display GET_INFO failed");
                return None;
            }
        };
        let info = match decode_info(&output[..bytes.min(output.len())]) {
            Ok(info) => info,
            Err(error) => {
                debug!(%error, "virtual display GET_INFO response was invalid");
                return None;
            }
        };
        self.start_keepalive();
        Some(crate::vdisplay_plan::caps_from_info(&info))
    }

    fn create(&self, mode: VdMode, _label: &str) -> Result<VdCreated, VdError> {
        let mut live = self
            .live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        let used: Vec<u32> = live.iter().copied().collect();
        let id = alloc_id(&used).ok_or(VdError::Limit)?;
        let add = crate::vdisplay_plan::build_add(id, &mode)?;
        let input = encode_add(&add).map_err(Self::map_decode_error)?;
        let mut output = [0u8; ADD_OUT_LEN];
        let bytes = self
            .ioctl(IOCTL_ADD, &input, &mut output)
            .map_err(map_win32_error)?;
        decode_add_out(&output[..bytes.min(output.len())]).map_err(Self::map_decode_error)?;
        live.insert(id);
        info!(
            event = "vdisplay.win.created",
            monitor_id = id,
            width = mode.width_px,
            height = mode.height_px
        );
        Ok(VdCreated {
            token: u64::from(id),
            platform_id: None,
            match_name: Some(monitor_name(id)),
            mode,
        })
    }

    fn destroy(&self, token: u64) -> Result<(), VdError> {
        let id = u32::try_from(token)
            .map_err(|_| VdError::Failed("virtual display token out of range".to_owned()))?;
        let input = encode_remove(id);
        match self.ioctl(IOCTL_REMOVE, &input, &mut []) {
            Ok(_) => {
                self.live
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                Ok(())
            }
            Err(error) if crate::vdisplay_ioctl::is_not_found(error) => {
                self.live
                    .lock()
                    .unwrap_or_else(|poisoned| poisoned.into_inner())
                    .remove(&id);
                Ok(())
            }
            Err(error) => Err(map_win32_error(error)),
        }
    }

    fn cleanup_stale(&self) -> usize {
        let mut output = [0u8; REMOVE_ALL_OUT_LEN];
        let removed = self
            .ioctl(IOCTL_REMOVE_ALL, &[], &mut output)
            .ok()
            .and_then(|bytes| decode_remove_all_out(&output[..bytes.min(output.len())]).ok())
            .unwrap_or(0);
        self.live
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner())
            .clear();
        removed as usize
    }
}

fn open_device() -> Result<OwnedHandle, u32> {
    let guid = GUID::from_values(
        INTERFACE_GUID_PARTS.0,
        INTERFACE_GUID_PARTS.1,
        INTERFACE_GUID_PARTS.2,
        INTERFACE_GUID_PARTS.3,
    );
    let mut length = 0u32;
    // SAFETY: the GUID and output length pointer are valid for this call.
    let result = unsafe {
        CM_Get_Device_Interface_List_SizeW(
            &mut length,
            &guid,
            None,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if result != CR_SUCCESS {
        return Err(result.0);
    }
    if length == 0 {
        return Err(ERROR_FILE_NOT_FOUND_CODE);
    }

    let mut buffer = vec![0u16; length as usize];
    // SAFETY: `buffer` has the exact size returned by the preceding query.
    let result = unsafe {
        CM_Get_Device_Interface_ListW(
            &guid,
            None,
            &mut buffer,
            CM_GET_DEVICE_INTERFACE_LIST_PRESENT,
        )
    };
    if result != CR_SUCCESS {
        return Err(result.0);
    }
    let end = buffer
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(0);
    if end == 0 {
        return Err(ERROR_FILE_NOT_FOUND_CODE);
    }
    let path_string = String::from_utf16_lossy(&buffer[..end]);
    let path = PCWSTR(buffer.as_ptr());
    debug!(device_path = %path_string, "opening virtual display device");
    // SAFETY: `path` points to the NUL-terminated path returned by ConfigMgr.
    let handle = match unsafe {
        CreateFileW(
            path,
            GENERIC_READ.0 | GENERIC_WRITE.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE,
            None,
            OPEN_EXISTING,
            FILE_ATTRIBUTE_NORMAL,
            None,
        )
    } {
        Ok(handle) => handle,
        Err(_) => return Err(unsafe { GetLastError().0 }),
    };
    // SAFETY: CreateFileW returned a valid uniquely owned handle.
    Ok(unsafe { OwnedHandle::from_raw_handle(handle.0) })
}
