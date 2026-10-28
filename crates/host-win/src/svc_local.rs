#![cfg(windows)]

//! Operations retained in the SYSTEM service, independent of a session pipe.

use std::sync::Arc;

use host_core::platform::{ConsoleState, HostInfo, KeyStore, Nic, Result};
use host_core::sysauth::SystemAuthenticator;
use rsp_core::pb;
use windows::core::{w, PCWSTR};
use windows::Win32::Foundation::ERROR_SUCCESS;
use windows::Win32::System::Registry::{
    RegGetValueW, HKEY_LOCAL_MACHINE, RRF_RT_REG_DWORD, RRF_RT_REG_SZ,
};

use crate::dpapi::{self, AclSecurer, DpapiProtector};
use crate::keyfile::FileKeyStore;
use crate::paths::WinPaths;
use crate::proxy::ServiceLocal;
use crate::svc_session_model::os_version_string;
use crate::sysauth::WinAuthenticator;

#[derive(Clone)]
pub struct WinServiceLocal {
    keystore: Arc<FileKeyStore<DpapiProtector, AclSecurer>>,
    info: Arc<HostInfo>,
}

impl WinServiceLocal {
    pub fn new(paths: &WinPaths) -> Self {
        Self {
            keystore: Arc::new(dpapi::machine_keystore(paths)),
            info: Arc::new(read_host_info()),
        }
    }
}

impl ServiceLocal for WinServiceLocal {
    fn info(&self) -> HostInfo {
        (*self.info).clone()
    }

    fn console_state(&self) -> ConsoleState {
        crate::console::console_state()
    }

    fn nics(&self) -> Vec<Nic> {
        crate::nics::nics()
    }

    fn send_wol(&self, mac: [u8; 6], via: &Nic) -> Result<()> {
        crate::wol::send_wol(mac, via)
    }

    fn keystore(&self) -> &dyn KeyStore {
        &*self.keystore
    }

    fn ctrl_alt_del(&self) -> Result<()> {
        crate::inject::send_sas()
    }

    fn system_authenticator(&self) -> Option<Arc<dyn SystemAuthenticator>> {
        Some(Arc::new(WinAuthenticator))
    }

    fn virtual_displays(&self) -> Option<Arc<dyn host_core::vdisplay::VirtualDisplayBackend>> {
        crate::vdisplay::backend()
    }
}

pub fn read_host_info() -> HostInfo {
    let key = w!("SOFTWARE\\Microsoft\\Windows NT\\CurrentVersion");
    HostInfo {
        name: std::env::var("COMPUTERNAME").unwrap_or_default(),
        os: pb::Platform::Windows,
        os_version: os_version_string(
            registry_dword(key, w!("CurrentMajorVersionNumber")),
            registry_dword(key, w!("CurrentMinorVersionNumber")),
            registry_string(key, w!("CurrentBuild")).as_deref(),
            registry_dword(key, w!("UBR")),
            registry_string(key, w!("DisplayVersion")).as_deref(),
        ),
        model: registry_string(
            w!("HARDWARE\\DESCRIPTION\\System\\BIOS"),
            w!("SystemProductName"),
        )
        .unwrap_or_default(),
        arch: std::env::consts::ARCH.to_owned(),
    }
}

// Only called with static, NUL-terminated UTF-16 keys/names from w! above.
fn registry_dword(key: PCWSTR, name: PCWSTR) -> Option<u32> {
    let mut value = 0_u32;
    let mut size = size_of::<u32>() as u32;
    // SAFETY: key/name are static UTF-16 strings; value is a writable DWORD
    // whose exact byte capacity is supplied to this synchronous call.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_DWORD,
            None,
            Some((&mut value as *mut u32).cast()),
            Some(&mut size),
        )
    };
    (status == ERROR_SUCCESS && size == size_of::<u32>() as u32).then_some(value)
}

fn registry_string(key: PCWSTR, name: PCWSTR) -> Option<String> {
    let mut size = 0_u32;
    // SAFETY: key/name are static UTF-16 strings. No data buffer is supplied;
    // size is a writable byte-count output for the size query.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            None,
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || size == 0 || !size.is_multiple_of(2) {
        return None;
    }
    let mut value = vec![0_u16; size as usize / 2];
    // SAFETY: value owns size bytes of aligned, writable storage; key/name
    // remain live. RegGetValueW reports failure if the value grew meanwhile.
    let status = unsafe {
        RegGetValueW(
            HKEY_LOCAL_MACHINE,
            key,
            name,
            RRF_RT_REG_SZ,
            None,
            Some(value.as_mut_ptr().cast()),
            Some(&mut size),
        )
    };
    if status != ERROR_SUCCESS || !size.is_multiple_of(2) {
        return None;
    }
    let value = value.get(..size as usize / 2)?;
    let end = value.iter().position(|ch| *ch == 0).unwrap_or(value.len());
    String::from_utf16(&value[..end]).ok()
}
