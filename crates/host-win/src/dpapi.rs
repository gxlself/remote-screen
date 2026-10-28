use std::fs;
use std::os::windows::ffi::OsStrExt;
use std::path::Path;

use host_core::platform::{PlatformError, Result};
use zeroize::{Zeroize, Zeroizing};

use crate::keyfile::{DirSecurer, FileKeyStore, Protector, DIR_SDDL, ENTROPY, FILE_SDDL};
use crate::paths::WinPaths;
use windows::core::{BOOL, PCWSTR};
use windows::Win32::Foundation::{LocalFree, HLOCAL};
use windows::Win32::Security::Authorization::{
    ConvertStringSecurityDescriptorToSecurityDescriptorW, SetNamedSecurityInfoW, SDDL_REVISION_1,
    SE_FILE_OBJECT,
};
use windows::Win32::Security::Cryptography::{
    CryptProtectData, CryptUnprotectData, CRYPTPROTECT_LOCAL_MACHINE, CRYPTPROTECT_UI_FORBIDDEN,
    CRYPT_INTEGER_BLOB,
};
use windows::Win32::Security::{
    GetSecurityDescriptorDacl, ACL, DACL_SECURITY_INFORMATION, PROTECTED_DACL_SECURITY_INFORMATION,
    PSECURITY_DESCRIPTOR,
};

/// Windows DPAPI protector configured for machine-wide decryption.
#[derive(Debug, Clone, Copy, Default)]
pub struct DpapiProtector;

impl Protector for DpapiProtector {
    fn protect(&self, plain: &[u8]) -> Result<Vec<u8>> {
        let input = data_blob(plain)?;
        let entropy = data_blob(ENTROPY)?;
        let mut output = LocalBlob::default();
        // SAFETY: input and entropy point to immutable buffers alive for the
        // duration of the call; output is a valid writable API result slot.
        unsafe {
            CryptProtectData(
                &input,
                PCWSTR(std::ptr::null()),
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_LOCAL_MACHINE | CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.blob,
            )
        }
        .map_err(|error| PlatformError::Platform(format!("dpapi: {error}")))?;
        Ok(output.as_bytes()?.to_vec())
    }

    fn unprotect(&self, blob: &[u8]) -> Result<Zeroizing<Vec<u8>>> {
        let input = data_blob(blob)?;
        let entropy = data_blob(ENTROPY)?;
        let mut output = LocalBlob::default();
        // SAFETY: input and entropy point to immutable buffers alive for the
        // duration of the call; output is a valid writable API result slot.
        unsafe {
            CryptUnprotectData(
                &input,
                None,
                Some(&entropy),
                None,
                None,
                CRYPTPROTECT_UI_FORBIDDEN,
                &mut output.blob,
            )
        }
        .map_err(|error| PlatformError::Platform(format!("dpapi: {error}")))?;
        let mut plain = Zeroizing::new(Vec::new());
        plain.extend_from_slice(output.as_bytes()?);
        Ok(plain)
    }
}

fn data_blob(bytes: &[u8]) -> Result<CRYPT_INTEGER_BLOB> {
    let cb_data = u32::try_from(bytes.len())
        .map_err(|_| PlatformError::Platform("dpapi: input too large".to_owned()))?;
    Ok(CRYPT_INTEGER_BLOB {
        cbData: cb_data,
        pbData: bytes.as_ptr() as *mut u8,
    })
}

#[derive(Default)]
struct LocalBlob {
    blob: CRYPT_INTEGER_BLOB,
}

impl LocalBlob {
    fn as_bytes(&self) -> Result<&[u8]> {
        if self.blob.cbData == 0 {
            return Ok(&[]);
        }
        if self.blob.pbData.is_null() {
            return Err(PlatformError::Platform("dpapi: invalid output".to_owned()));
        }
        // SAFETY: the pointer and length were returned together by DPAPI and
        // remain owned by this wrapper. A DWORD length fits isize on Win64;
        // the nonempty buffer was checked for null above.
        Ok(unsafe { std::slice::from_raw_parts(self.blob.pbData, self.blob.cbData as usize) })
    }
}

impl Drop for LocalBlob {
    fn drop(&mut self) {
        if self.blob.pbData.is_null() {
            return;
        }
        // SAFETY: DPAPI allocated this buffer with LocalAlloc and it is still
        // exclusively owned by this wrapper.
        unsafe {
            std::slice::from_raw_parts_mut(self.blob.pbData, self.blob.cbData as usize).zeroize();
            let _ = LocalFree(Some(HLOCAL(self.blob.pbData.cast())));
        }
    }
}

/// ACL securer that grants full control to SYSTEM and Administrators only.
#[derive(Debug, Clone, Copy, Default)]
pub struct AclSecurer;

impl DirSecurer for AclSecurer {
    fn secure_dir(&self, dir: &Path) -> Result<()> {
        set_acl(dir, DIR_SDDL)
    }

    fn secure_file(&self, file: &Path) -> Result<()> {
        set_acl(file, FILE_SDDL)
    }
}

fn set_acl(path: &Path, sddl: &str) -> Result<()> {
    let descriptor = security_descriptor(sddl)?;
    let mut dacl_present = BOOL::default();
    let mut dacl_defaulted = BOOL::default();
    let mut dacl: *mut ACL = std::ptr::null_mut();
    // SAFETY: descriptor is a valid LocalAlloc-backed security descriptor and
    // all out-pointers refer to stack slots valid for this call.
    unsafe {
        GetSecurityDescriptorDacl(
            descriptor.descriptor,
            &mut dacl_present,
            &mut dacl,
            &mut dacl_defaulted,
        )
    }
    .map_err(|error| PlatformError::Platform(format!("set acl: {error}")))?;
    if !dacl_present.as_bool() || dacl.is_null() {
        return Err(PlatformError::Platform("set acl: missing dacl".to_owned()));
    }

    let path = wide_path(path)?;
    // SAFETY: path is NUL-terminated and the DACL points into the live
    // descriptor kept alive until SetNamedSecurityInfoW returns.
    let result = unsafe {
        SetNamedSecurityInfoW(
            PCWSTR(path.as_ptr()),
            SE_FILE_OBJECT,
            DACL_SECURITY_INFORMATION | PROTECTED_DACL_SECURITY_INFORMATION,
            None,
            None,
            Some(dacl.cast_const()),
            None,
        )
    };
    if result.0 == 0 {
        Ok(())
    } else {
        Err(PlatformError::Platform(format!("set acl: {}", result.0)))
    }
}

#[derive(Default)]
struct LocalSecurityDescriptor {
    descriptor: PSECURITY_DESCRIPTOR,
}

impl Drop for LocalSecurityDescriptor {
    fn drop(&mut self) {
        if self.descriptor.0.is_null() {
            return;
        }
        // SAFETY: ConvertStringSecurityDescriptorToSecurityDescriptorW returns
        // this descriptor through LocalAlloc and ownership ends here.
        unsafe {
            let _ = LocalFree(Some(HLOCAL(self.descriptor.0)));
        }
    }
}

fn security_descriptor(sddl: &str) -> Result<LocalSecurityDescriptor> {
    let sddl = wide_string(sddl);
    let mut descriptor = LocalSecurityDescriptor::default();
    // SAFETY: sddl is a valid NUL-terminated UTF-16 string and the output
    // pointers reference writable stack slots.
    unsafe {
        ConvertStringSecurityDescriptorToSecurityDescriptorW(
            PCWSTR(sddl.as_ptr()),
            SDDL_REVISION_1,
            &mut descriptor.descriptor,
            None,
        )
    }
    .map_err(|error| PlatformError::Platform(format!("set acl: {error}")))?;
    if descriptor.descriptor.0.is_null() {
        return Err(PlatformError::Platform(
            "set acl: null descriptor".to_owned(),
        ));
    }
    Ok(descriptor)
}

fn wide_string(value: &str) -> Vec<u16> {
    value.encode_utf16().chain(std::iter::once(0)).collect()
}

fn wide_path(path: &Path) -> Result<Vec<u16>> {
    if path.as_os_str().encode_wide().any(|unit| unit == 0) {
        return Err(PlatformError::Platform("set acl: invalid path".to_owned()));
    }
    Ok(path
        .as_os_str()
        .encode_wide()
        .chain(std::iter::once(0))
        .collect())
}

/// Construct the machine-level file key store for the supplied host paths.
pub fn machine_keystore(paths: &WinPaths) -> FileKeyStore<DpapiProtector, AclSecurer> {
    FileKeyStore::new(paths.keys_dir.clone(), DpapiProtector, AclSecurer)
}

/// Create and secure the host data directories.
pub fn secure_data_dir(paths: &WinPaths) -> Result<()> {
    fs::create_dir_all(&paths.data_dir)?;
    let securer = AclSecurer;
    securer.secure_dir(&paths.data_dir)?;
    fs::create_dir_all(&paths.keys_dir)?;
    securer.secure_dir(&paths.keys_dir)?;
    // Newly created logs inherit the already restricted parent DACL.
    fs::create_dir_all(&paths.logs_dir)?;
    Ok(())
}
