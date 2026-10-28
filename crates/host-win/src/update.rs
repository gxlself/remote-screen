use std::ffi::c_void;
use std::os::windows::ffi::OsStrExt;
use std::os::windows::process::CommandExt;
use std::path::Path;
use std::process::{Command, Stdio};

use host_core::update::prepare::PackageVerifier;
use windows::core::{GUID, PCWSTR};
use windows::Win32::Foundation::{
    ERROR_ACCESS_DENIED, HANDLE, HWND, INVALID_HANDLE_VALUE, TRUST_E_NOSIGNATURE,
};
use windows::Win32::Security::Cryptography::{
    CertGetNameStringW, CERT_CONTEXT, CERT_NAME_RDN_TYPE, CERT_STRING_TYPE, CERT_X500_NAME_STR,
};
use windows::Win32::Security::WinTrust::{
    WTHelperGetProvSignerFromChain, WTHelperProvDataFromStateData, WinVerifyTrust,
    CRYPT_PROVIDER_SGNR, WINTRUST_ACTION_GENERIC_VERIFY_V2, WINTRUST_DATA, WINTRUST_DATA_0,
    WINTRUST_FILE_INFO, WTD_CACHE_ONLY_URL_RETRIEVAL, WTD_CHOICE_FILE, WTD_REVOKE_NONE,
    WTD_STATEACTION_CLOSE, WTD_STATEACTION_VERIFY, WTD_UICONTEXT_EXECUTE, WTD_UI_NONE,
};
use windows::Win32::System::Threading::{
    CREATE_BREAKAWAY_FROM_JOB, CREATE_NEW_PROCESS_GROUP, DETACHED_PROCESS,
};

use crate::msi::{msiexec_path, subject_matches, Launch, MsiInstaller};
use crate::paths::WinPaths;

/// Verify a PE/MSI Authenticode signature and return its signer subject.
pub fn signer_subject(path: &Path) -> Result<Option<String>, String> {
    let wide_path: Vec<u16> = path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect();
    let mut file_info = WINTRUST_FILE_INFO {
        cbStruct: std::mem::size_of::<WINTRUST_FILE_INFO>() as u32,
        pcwszFilePath: PCWSTR(wide_path.as_ptr()),
        hFile: HANDLE::default(),
        pgKnownSubject: std::ptr::null_mut(),
    };
    let mut trust_data = WINTRUST_DATA {
        cbStruct: std::mem::size_of::<WINTRUST_DATA>() as u32,
        dwUIChoice: WTD_UI_NONE,
        fdwRevocationChecks: WTD_REVOKE_NONE,
        dwUnionChoice: WTD_CHOICE_FILE,
        Anonymous: WINTRUST_DATA_0 {
            pFile: &mut file_info,
        },
        dwStateAction: WTD_STATEACTION_VERIFY,
        dwProvFlags: WTD_CACHE_ONLY_URL_RETRIEVAL,
        dwUIContext: WTD_UICONTEXT_EXECUTE,
        ..Default::default()
    };
    let mut action = WINTRUST_ACTION_GENERIC_VERIFY_V2;
    let close_guard = WinTrustCloseGuard {
        action: &mut action,
        data: &mut trust_data,
    };

    // SAFETY: The action, trust data, file info, and NUL-terminated path all
    // remain alive until WinVerifyTrust returns and the close guard runs.
    let result = unsafe {
        WinVerifyTrust(
            HWND(INVALID_HANDLE_VALUE.0),
            close_guard.action,
            close_guard.data.cast::<c_void>(),
        )
    };
    let result_code = result as u32;
    if result_code == TRUST_E_NOSIGNATURE.0 as u32 {
        return Ok(None);
    }
    if result_code != 0 {
        return Err(format!("WinVerifyTrust: 0x{result_code:08x}"));
    }

    // SAFETY: A successful WinVerifyTrust call populated hWVTStateData, and
    // the close guard keeps the provider state alive until extraction ends.
    let provider_data = unsafe { WTHelperProvDataFromStateData((*close_guard.data).hWVTStateData) };
    if provider_data.is_null() {
        return Err("WinVerifyTrust: missing provider data".to_owned());
    }
    // SAFETY: provider_data is returned by WinTrust for the live state handle;
    // index zero is the primary signer requested by this verifier.
    let signer = unsafe { WTHelperGetProvSignerFromChain(provider_data, 0, false, 0) };
    if signer.is_null() {
        return Err("WinVerifyTrust: missing signer".to_owned());
    }
    let cert = signer_certificate(signer)?;
    certificate_subject(cert).map(Some)
}

struct WinTrustCloseGuard {
    action: *mut GUID,
    data: *mut WINTRUST_DATA,
}

impl Drop for WinTrustCloseGuard {
    fn drop(&mut self) {
        // SAFETY: The guard is created only with pointers to its caller's live
        // action and trust data; Drop runs before either stack value expires.
        unsafe {
            (*self.data).dwStateAction = WTD_STATEACTION_CLOSE;
            let _ = WinVerifyTrust(
                HWND(INVALID_HANDLE_VALUE.0),
                self.action,
                self.data.cast::<c_void>(),
            );
        }
    }
}

fn signer_certificate(signer: *mut CRYPT_PROVIDER_SGNR) -> Result<*const CERT_CONTEXT, String> {
    // SAFETY: signer is a non-null provider pointer returned by WinTrust.
    let (chain_len, cert_chain) = unsafe { ((*signer).csCertChain, (*signer).pasCertChain) };
    if chain_len == 0 {
        return Err("WinVerifyTrust: missing certificate chain".to_owned());
    }
    if cert_chain.is_null() {
        return Err("WinVerifyTrust: missing certificate chain".to_owned());
    }
    // SAFETY: The first chain element is present when WinTrust reports a valid
    // primary signer; its context remains owned by the WinTrust state.
    let cert = unsafe { (*cert_chain).pCert };
    if cert.is_null() {
        return Err("WinVerifyTrust: missing signer certificate".to_owned());
    }
    Ok(cert)
}

fn certificate_subject(cert: *const CERT_CONTEXT) -> Result<String, String> {
    let name_type: CERT_STRING_TYPE = CERT_X500_NAME_STR;
    // SAFETY: cert is a non-null context owned by the live WinTrust state;
    // passing no output buffer asks CryptoAPI for the required UTF-16 length.
    let required = unsafe {
        CertGetNameStringW(
            cert,
            CERT_NAME_RDN_TYPE,
            0,
            Some((&name_type as *const CERT_STRING_TYPE).cast()),
            None,
        )
    };
    if required <= 1 {
        return Err("WinVerifyTrust: certificate subject is empty".to_owned());
    }
    let mut buffer = vec![0u16; required as usize];
    // SAFETY: buffer has exactly the length requested by CryptoAPI and cert is
    // still owned by the WinTrust state.
    let written = unsafe {
        CertGetNameStringW(
            cert,
            CERT_NAME_RDN_TYPE,
            0,
            Some((&name_type as *const CERT_STRING_TYPE).cast()),
            Some(&mut buffer),
        )
    };
    if written == 0 {
        return Err("WinVerifyTrust: could not read certificate subject".to_owned());
    }
    let end = buffer
        .iter()
        .position(|character| *character == 0)
        .unwrap_or(written as usize)
        .min(buffer.len());
    String::from_utf16(&buffer[..end])
        .map_err(|_| "WinVerifyTrust: certificate subject is not UTF-16".to_owned())
}

/// Platform verifier retaining the executable signer observed at startup.
pub struct WinTrustVerifier {
    current: Option<String>,
}

impl WinTrustVerifier {
    pub fn new() -> Self {
        let current = std::env::current_exe()
            .ok()
            .and_then(|path| signer_subject(&path).ok().flatten());
        Self { current }
    }
}

impl Default for WinTrustVerifier {
    fn default() -> Self {
        Self::new()
    }
}

impl PackageVerifier for WinTrustVerifier {
    fn verify_platform(&self, package: &Path) -> Result<(), String> {
        let candidate = signer_subject(package)?;
        subject_matches(self.current.as_deref(), candidate.as_deref())
    }
}

/// Detached process launcher used by the MSI installer.
#[derive(Debug, Clone, Copy, Default)]
pub struct DetachedLauncher;

impl Launch for DetachedLauncher {
    fn launch(&self, exe: &Path, args: &[String]) -> Result<(), String> {
        let detached_flags =
            (DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP | CREATE_BREAKAWAY_FROM_JOB).0;
        match spawn_detached(exe, args, detached_flags) {
            Ok(()) => Ok(()),
            Err(error) if error.raw_os_error() == Some(ERROR_ACCESS_DENIED.0 as i32) => {
                let flags = (DETACHED_PROCESS | CREATE_NEW_PROCESS_GROUP).0;
                spawn_detached(exe, args, flags)
                    .map_err(|retry| format!("launch {}: {retry}", exe.display()))
            }
            Err(error) => Err(format!("launch {}: {error}", exe.display())),
        }
    }
}

fn spawn_detached(exe: &Path, args: &[String], flags: u32) -> std::io::Result<()> {
    let mut command = Command::new(exe);
    command
        .args(args)
        .creation_flags(flags)
        .stdin(Stdio::null())
        .stdout(Stdio::null())
        .stderr(Stdio::null())
        .spawn()
        .map(|_| ())
}

/// Build the Windows installer using paths resolved by the host service.
pub fn installer(paths: &WinPaths) -> MsiInstaller<DetachedLauncher> {
    let system_root = std::env::var("SystemRoot").ok();
    MsiInstaller::new(
        paths.update_dir.clone(),
        paths.logs_dir.clone(),
        msiexec_path(system_root.as_deref()),
        DetachedLauncher,
    )
}
