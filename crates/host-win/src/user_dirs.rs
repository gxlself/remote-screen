#![cfg(windows)]
//! Resolve directories for the active console user, never SYSTEM's profile.
//!
//! Residual TOCTOU risk: a user can replace a checked directory with a junction
//! before SYSTEM writes to it, including during the ten-second cache lifetime
//! or a longer transfer. Eliminating that race requires impersonation while
//! writing, or rechecking the final destination through its open handle in the
//! host-core write path. This resolver does not provide that write-time guard.

use std::os::windows::io::{AsRawHandle, FromRawHandle, OwnedHandle};
use std::path::{Path, PathBuf};
use std::sync::Mutex;
use std::time::Instant;

use host_core::user_dirs::{DirsOwner, UserDirs, UserDirsSource};
use windows::core::{BOOL, GUID, HRESULT, PCWSTR, PWSTR};
use windows::Win32::Foundation::{LocalFree, ERROR_INSUFFICIENT_BUFFER, HANDLE, HLOCAL};
use windows::Win32::Security::Authorization::{GetNamedSecurityInfoW, SE_FILE_OBJECT};
use windows::Win32::Security::{
    AccessCheck, DuplicateTokenEx, MapGenericMask, SecurityIdentification, TokenImpersonation,
    DACL_SECURITY_INFORMATION, GENERIC_MAPPING, GROUP_SECURITY_INFORMATION,
    LABEL_SECURITY_INFORMATION, OWNER_SECURITY_INFORMATION, PRIVILEGE_SET, PSECURITY_DESCRIPTOR,
    TOKEN_IMPERSONATE, TOKEN_QUERY,
};
use windows::Win32::Storage::FileSystem::{
    CreateFileW, GetFileInformationByHandle, GetFinalPathNameByHandleW, BY_HANDLE_FILE_INFORMATION,
    FILE_ADD_FILE, FILE_ALL_ACCESS, FILE_ATTRIBUTE_DIRECTORY, FILE_FLAG_BACKUP_SEMANTICS,
    FILE_GENERIC_EXECUTE, FILE_GENERIC_READ, FILE_GENERIC_WRITE, FILE_NAME_NORMALIZED,
    FILE_READ_ATTRIBUTES, FILE_SHARE_DELETE, FILE_SHARE_READ, FILE_SHARE_WRITE,
    GETFINALPATHNAMEBYHANDLE_FLAGS, OPEN_EXISTING, VOLUME_NAME_DOS,
};
use windows::Win32::System::Com::CoTaskMemFree;
use windows::Win32::UI::Shell::{FOLDERID_Downloads, FOLDERID_Profile, KF_FLAG_DEFAULT};

use crate::dpapi::AclSecurer;
use crate::keyfile::DirSecurer;
use crate::paths::WinPaths;
use crate::user_dirs_model::{
    cache_fresh, choose, forbidden_roots, is_forbidden, strip_verbatim, Choice, Fallback, Probe,
    CACHE_TTL, RECEIVED_DIR,
};

pub struct WinUserDirs {
    fallback: PathBuf,
    forbidden: Vec<String>,
    cache: Mutex<Option<Cached>>,
    last_kind: Mutex<Option<ChoiceKind>>,
}

struct Cached {
    session: u32,
    at: Instant,
    user_token: Option<OwnedHandle>,
    imp_token: Option<OwnedHandle>,
    choice: Choice,
    dirs: UserDirs,
}

#[derive(Clone, Copy, PartialEq, Eq)]
enum ChoiceKind {
    User,
    Fallback(Fallback),
}

impl WinUserDirs {
    pub fn new(paths: &WinPaths) -> Self {
        let fallback = Self::fallback_path(paths);
        if std::fs::create_dir_all(&fallback).is_err() {
            tracing::warn!("user directories: fallback creation failed");
        }
        if AclSecurer.secure_dir(&fallback).is_err() {
            tracing::warn!("user directories: fallback ACL setup failed");
        }
        let mut forbidden = forbidden_roots(
            &|name| std::env::var(name).ok(),
            &paths.data_dir.to_string_lossy(),
        );
        for root in forbidden.clone() {
            if let Some(canonical) = canonical_dir(&root).filter(|canonical| {
                !forbidden
                    .iter()
                    .any(|value| value.eq_ignore_ascii_case(canonical))
            }) {
                forbidden.push(canonical);
            }
        }
        Self {
            fallback,
            forbidden,
            cache: Mutex::new(None),
            last_kind: Mutex::new(None),
        }
    }

    pub fn fallback_path(paths: &WinPaths) -> PathBuf {
        paths.data_dir.join(RECEIVED_DIR)
    }

    pub fn explain(&self) -> (UserDirs, Option<Fallback>) {
        self.with_current(|cached| match cached {
            Some(cached) => (cached.dirs.clone(), fallback_reason(&cached.choice)),
            None => (self.dirs_for(&no_console()), Some(Fallback::NoConsole)),
        })
    }

    /// Keep the lock during refresh and use, so tokens cannot outlive their
    /// cached session and simultaneous callers never duplicate a probe.
    fn with_current<T>(&self, use_cached: impl FnOnce(Option<&Cached>) -> T) -> T {
        let mut cache = self.cache.lock().unwrap_or_else(|poisoned| {
            tracing::debug!("user directories: discarding poisoned cache");
            let mut cache = poisoned.into_inner();
            *cache = None;
            self.cache.clear_poison();
            cache
        });
        let Some(session) = crate::wts::active_console_session() else {
            *cache = None;
            self.record_choice(&no_console());
            return use_cached(None);
        };
        if !cache.as_ref().is_some_and(|cached| {
            cache_fresh(
                cached.session,
                cached.at,
                session,
                Instant::now(),
                CACHE_TTL,
            )
        }) {
            // Drop old handles before probing another session or refreshing.
            *cache = None;
            *cache = Some(self.probe(session));
        }
        if let Some(cached) = cache.as_ref() {
            self.record_choice(&cached.choice);
        }
        use_cached(cache.as_ref())
    }

    fn probe(&self, session: u32) -> Cached {
        let at = Instant::now();
        let user_token = crate::tray::user_token(session).ok();
        let mut probe = Probe {
            console: Some(session),
            has_user: user_token.is_some(),
            downloads: None,
            profile: None,
            downloads_writable: false,
        };
        let mut imp_token = None;
        if let Some(token) = user_token.as_ref() {
            probe.downloads = known_folder(&FOLDERID_Downloads, token);
            probe.profile = known_folder(&FOLDERID_Profile, token);
            imp_token = impersonation_token(token);
            if let (Some(downloads), Some(token)) = (
                probe
                    .downloads
                    .as_ref()
                    .filter(|downloads| !is_forbidden(downloads, &self.forbidden)),
                imp_token.as_ref(),
            ) {
                probe.downloads_writable = can_add_file(downloads, token);
            }
        } else {
            tracing::debug!("user directories: console user token unavailable");
        }
        let choice = choose(&probe, &self.forbidden);
        let dirs = self.dirs_for(&choice);
        Cached {
            session,
            at,
            user_token,
            imp_token,
            choice,
            dirs,
        }
    }

    fn dirs_for(&self, choice: &Choice) -> UserDirs {
        match choice {
            Choice::User { downloads, home } => UserDirs {
                owner: DirsOwner::User,
                downloads: PathBuf::from(downloads),
                home: home.as_ref().map(PathBuf::from),
            },
            Choice::Fallback { home, .. } => UserDirs {
                owner: DirsOwner::NoUser,
                downloads: self.fallback.clone(),
                home: home.as_ref().map(PathBuf::from),
            },
        }
    }

    fn record_choice(&self, choice: &Choice) {
        let kind = match fallback_reason(choice) {
            Some(reason) => ChoiceKind::Fallback(reason),
            None => ChoiceKind::User,
        };
        let mut last = self
            .last_kind
            .lock()
            .unwrap_or_else(|poisoned| poisoned.into_inner());
        if *last != Some(kind) {
            match kind {
                ChoiceKind::User => tracing::info!(kind = "User", "user directory choice changed"),
                ChoiceKind::Fallback(reason) => {
                    tracing::info!(kind = "Fallback", ?reason, "user directory choice changed")
                }
            }
            *last = Some(kind);
        }
    }
}

impl UserDirsSource for WinUserDirs {
    fn current(&self) -> UserDirs {
        self.explain().0
    }

    fn user_can_write(&self, dir: &Path) -> bool {
        let allowed = self.with_current(|cached| {
            let Some(cached) = cached.filter(|cached| cached.user_token.is_some()) else {
                return false;
            };
            let Some(token) = cached.imp_token.as_ref() else {
                return false;
            };
            let Some(path) = dir.to_str() else {
                return false;
            };
            let Some(canonical) = canonical_dir(path) else {
                return false;
            };
            !is_forbidden(&canonical, &self.forbidden) && can_add_file(&canonical, token)
        });
        if !allowed {
            tracing::debug!("user directories: explicit directory write denied");
        }
        allowed
    }
}

fn no_console() -> Choice {
    Choice::Fallback {
        reason: Fallback::NoConsole,
        home: None,
    }
}

fn fallback_reason(choice: &Choice) -> Option<Fallback> {
    match choice {
        Choice::User { .. } => None,
        Choice::Fallback { reason, .. } => Some(*reason),
    }
}

// The windows 0.62 wrapper hides ppszPath on HRESULT failure, but the API
// requires CoTaskMemFree on both outcomes. Preserve that output via the ABI.
#[link(name = "shell32", kind = "raw-dylib")]
// SAFETY: This is SHGetKnownFolderPath's documented system ABI and types.
unsafe extern "system" {
    #[link_name = "SHGetKnownFolderPath"]
    fn sh_get_known_folder_path(
        id: *const GUID,
        flags: u32,
        token: HANDLE,
        path: *mut PWSTR,
    ) -> HRESULT;
}

#[derive(Default)]
struct KnownFolderPath(PWSTR);

impl Drop for KnownFolderPath {
    fn drop(&mut self) {
        // SAFETY: The output is null or solely owned Shell task memory, on
        // success and failure alike. CoTaskMemFree accepts a null pointer.
        unsafe { CoTaskMemFree(Some(self.0 .0.cast())) };
    }
}

fn known_folder(id: &GUID, token: &OwnedHandle) -> Option<String> {
    let mut output = KnownFolderPath::default();
    // SAFETY: GUID and token are live, and the guard owns the writable output
    // slot and any allocation returned by Shell, even on failure.
    let result = unsafe {
        sh_get_known_folder_path(
            id,
            KF_FLAG_DEFAULT.0 as u32,
            HANDLE(token.as_raw_handle()),
            &mut output.0,
        )
    };
    if result.is_err() || output.0.is_null() {
        tracing::debug!("user directories: known folder query failed");
        return None;
    }
    // SAFETY: Successful Shell output is a NUL-terminated UTF-16 string kept
    // alive by output until conversion completes.
    let path = match unsafe { output.0.to_string() } {
        Ok(path) => path,
        Err(_) => {
            tracing::debug!("user directories: known folder is not Unicode");
            return None;
        }
    };
    let stripped = strip_verbatim(&path.replace('/', "\\"));
    if stripped.starts_with(r"\\") {
        // Retain it for choose's Forbidden classification; never open UNC.
        return Some(stripped);
    }
    canonical_dir(&path)
}

fn wide(path: &str) -> Option<Vec<u16>> {
    if path.is_empty() || path.contains('\0') {
        tracing::debug!("user directories: invalid path text");
        return None;
    }
    Some(path.encode_utf16().chain(Some(0)).collect())
}

fn canonical_dir(path: &str) -> Option<String> {
    let path = path.replace('/', "\\");
    if strip_verbatim(&path).starts_with(r"\\") {
        tracing::debug!("user directories: network path rejected");
        return None;
    }
    let path = wide(&path)?;
    // SAFETY: path is NUL-terminated and live; no pointers/handles are inherited.
    // Omitting OPEN_REPARSE_POINT intentionally follows junctions and symlinks.
    let handle = match unsafe {
        CreateFileW(
            PCWSTR(path.as_ptr()),
            FILE_READ_ATTRIBUTES.0,
            FILE_SHARE_READ | FILE_SHARE_WRITE | FILE_SHARE_DELETE,
            None,
            OPEN_EXISTING,
            FILE_FLAG_BACKUP_SEMANTICS,
            None,
        )
    } {
        Ok(handle) => handle,
        Err(_) => {
            tracing::debug!("user directories: directory open failed");
            return None;
        }
    };
    // SAFETY: CreateFileW returned a valid unique handle on success.
    let handle = unsafe { OwnedHandle::from_raw_handle(handle.0) };
    let mut info = BY_HANDLE_FILE_INFORMATION::default();
    // SAFETY: The handle is live and info is writable for the whole call.
    if unsafe { GetFileInformationByHandle(HANDLE(handle.as_raw_handle()), &mut info) }.is_err()
        || info.dwFileAttributes & FILE_ATTRIBUTE_DIRECTORY.0 == 0
    {
        tracing::debug!("user directories: directory attributes unavailable or not a directory");
        return None;
    }
    let mut buffer = vec![0_u16; 512];
    for attempt in 0..2 {
        // SAFETY: The handle and writable buffer remain live; the API observes
        // the slice length, and returns the required length if it does not fit.
        let length = unsafe {
            GetFinalPathNameByHandleW(
                HANDLE(handle.as_raw_handle()),
                &mut buffer,
                GETFINALPATHNAMEBYHANDLE_FLAGS(FILE_NAME_NORMALIZED.0 | VOLUME_NAME_DOS.0),
            )
        } as usize;
        if length == 0 {
            break;
        }
        if length < buffer.len() {
            return match String::from_utf16(&buffer[..length]) {
                Ok(path) => Some(strip_verbatim(&path)),
                Err(_) => {
                    tracing::debug!("user directories: canonical path is not Unicode");
                    None
                }
            };
        }
        if attempt == 0 {
            buffer.resize(length.checked_add(1)?, 0);
        }
    }
    tracing::debug!("user directories: final path query failed");
    None
}

fn impersonation_token(token: &OwnedHandle) -> Option<OwnedHandle> {
    let mut duplicate = HANDLE::default();
    // SAFETY: The source token is live and duplicate is a writable handle slot.
    // Identification suffices for AccessCheck; no thread impersonation occurs.
    if unsafe {
        DuplicateTokenEx(
            HANDLE(token.as_raw_handle()),
            TOKEN_QUERY | TOKEN_IMPERSONATE,
            None,
            SecurityIdentification,
            TokenImpersonation,
            &mut duplicate,
        )
    }
    .is_err()
    {
        tracing::debug!("user directories: impersonation token duplication failed");
        return None;
    }
    // SAFETY: DuplicateTokenEx transferred a valid unique token on success.
    Some(unsafe { OwnedHandle::from_raw_handle(duplicate.0) })
}

#[derive(Default)]
struct SecurityDescriptor(PSECURITY_DESCRIPTOR);

impl Drop for SecurityDescriptor {
    fn drop(&mut self) {
        if !self.0 .0.is_null() {
            // SAFETY: GetNamedSecurityInfoW allocated this uniquely owned
            // descriptor with LocalAlloc; all borrowed fields are now unused.
            unsafe {
                let _ = LocalFree(Some(HLOCAL(self.0 .0)));
            }
        }
    }
}

fn can_add_file(canonical: &str, token: &OwnedHandle) -> bool {
    let Some(path) = wide(canonical) else {
        return false;
    };
    let mut descriptor = SecurityDescriptor::default();
    // SAFETY: path is NUL-terminated and descriptor is a writable output slot.
    // The guard frees the allocation on every exit, including API failure.
    let result = unsafe {
        GetNamedSecurityInfoW(
            PCWSTR(path.as_ptr()),
            SE_FILE_OBJECT,
            OWNER_SECURITY_INFORMATION
                | GROUP_SECURITY_INFORMATION
                | DACL_SECURITY_INFORMATION
                | LABEL_SECURITY_INFORMATION,
            None,
            None,
            None,
            None,
            &mut descriptor.0,
        )
    };
    if result.0 != 0 || descriptor.0 .0.is_null() {
        tracing::debug!("user directories: security descriptor query failed");
        return false;
    }
    let mapping = GENERIC_MAPPING {
        GenericRead: FILE_GENERIC_READ.0,
        GenericWrite: FILE_GENERIC_WRITE.0,
        GenericExecute: FILE_GENERIC_EXECUTE.0,
        GenericAll: FILE_ALL_ACCESS.0,
    };
    let mut desired = FILE_ADD_FILE.0;
    // SAFETY: desired is writable and mapping is a live file access mapping.
    unsafe { MapGenericMask(&mut desired, &mapping) };
    // Typed storage guarantees PRIVILEGE_SET alignment, including its variable
    // trailing privilege array. The allocation is used only as output bytes.
    let mut privileges = vec![PRIVILEGE_SET::default()];
    for attempt in 0..2 {
        let Ok(capacity) = u32::try_from(std::mem::size_of_val(privileges.as_slice())) else {
            return false;
        };
        let mut needed = capacity;
        let mut granted = 0;
        let mut status = BOOL::default();
        // SAFETY: Descriptor and impersonation token remain live; the privilege
        // allocation is aligned and contains capacity writable bytes. All other
        // outputs point to live stack slots and no output is read on failure.
        let result = unsafe {
            AccessCheck(
                descriptor.0,
                HANDLE(token.as_raw_handle()),
                desired,
                &mapping,
                Some(privileges.as_mut_ptr()),
                &mut needed,
                &mut granted,
                &mut status,
            )
        };
        match result {
            Ok(()) => {
                if !status.as_bool() {
                    tracing::debug!("user directories: FILE_ADD_FILE access denied");
                }
                return status.as_bool();
            }
            Err(error)
                if attempt == 0
                    && error.code() == HRESULT::from_win32(ERROR_INSUFFICIENT_BUFFER.0)
                    && needed > capacity =>
            {
                privileges.resize(
                    (needed as usize).div_ceil(size_of::<PRIVILEGE_SET>()),
                    PRIVILEGE_SET::default(),
                );
            }
            Err(_) => break,
        }
    }
    tracing::debug!("user directories: access check failed");
    false
}
