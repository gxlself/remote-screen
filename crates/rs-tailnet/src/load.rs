//! Process-wide loading. A successfully loaded Go runtime is never unloaded.

use std::{
    ffi::{CStr, CString},
    os::unix::ffi::OsStrExt,
    path::{Path, PathBuf},
    sync::{Mutex, OnceLock},
};

use crate::abi::{Api, ABI_VERSION, SYMBOLS};

#[derive(Debug, thiserror::Error)]
pub enum LoadError {
    #[error("tailnet library {loaded:?} already loaded; requested {requested:?}")]
    AlreadyLoaded { loaded: PathBuf, requested: PathBuf },
    #[error("cannot open tailnet library {path:?}: {message}")]
    Open { path: PathBuf, message: String },
    #[error("tailnet library missing symbol {name}")]
    Symbol { name: &'static str },
    #[error("rs_ts_abi_version returned {found}; expected {expected}")]
    AbiMismatch { found: i32, expected: i32 },
    #[error("tailnet library unavailable: {0}")]
    Unavailable(&'static str),
}

static LOADED: OnceLock<Mutex<Option<(PathBuf, &'static Api)>>> = OnceLock::new();

pub fn load(path: &Path) -> Result<&'static Api, LoadError> {
    let mut loaded = LOADED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|error| error.into_inner());
    if let Some((previous, api)) = loaded.as_ref() {
        return if previous == path {
            Ok(api)
        } else {
            Err(LoadError::AlreadyLoaded {
                loaded: previous.clone(),
                requested: path.to_owned(),
            })
        };
    }
    let cpath = CString::new(path.as_os_str().as_bytes()).map_err(|_| LoadError::Open {
        path: path.to_owned(),
        message: "path contains NUL".into(),
    })?;
    // SAFETY: cpath is a valid NUL-terminated path and the flags are POSIX flags.
    let handle = unsafe { libc::dlopen(cpath.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
    if handle.is_null() {
        return Err(LoadError::Open {
            path: path.to_owned(),
            message: dl_error(),
        });
    }
    let result = (|| {
        macro_rules! symbol {
            ($index:expr, $ty:ty) => {{
                let name = SYMBOLS[$index];
                let cname = CString::new(name).expect("ABI symbol names contain no NUL");
                // SAFETY: handle came from successful dlopen, and cname is a C string.
                let pointer = unsafe { libc::dlsym(handle, cname.as_ptr()) };
                if pointer.is_null() {
                    return Err(LoadError::Symbol { name });
                }
                // SAFETY: the named ABI v1 symbol has exactly this C signature;
                // POSIX specifies that dlsym function addresses can be cast this way.
                unsafe { std::mem::transmute::<*mut libc::c_void, $ty>(pointer) }
            }};
        }
        use std::ffi::c_char;
        let api = Api {
            abi_version: symbol!(0, unsafe extern "C" fn() -> i32),
            start: symbol!(
                1,
                unsafe extern "C" fn(*const c_char, *mut c_char, usize) -> i32
            ),
            stop: symbol!(2, unsafe extern "C" fn() -> i32),
            login: symbol!(3, unsafe extern "C" fn(*mut c_char, usize) -> i32),
            logout: symbol!(4, unsafe extern "C" fn(*mut c_char, usize) -> i32),
            rebind: symbol!(5, unsafe extern "C" fn() -> i32),
            status: symbol!(6, unsafe extern "C" fn(*mut c_char, usize) -> i64),
            next_event: symbol!(7, unsafe extern "C" fn(*mut c_char, usize, i32) -> i64),
            dial_udp: symbol!(
                8,
                unsafe extern "C" fn(*const c_char, u16, i32, *mut c_char, usize) -> i32
            ),
            dial_tcp: symbol!(
                9,
                unsafe extern "C" fn(*const c_char, u16, i32, *mut c_char, usize) -> i32
            ),
        };
        check_version(&api)?;
        Ok(api)
    })();
    let api = match result {
        Ok(api) => Box::leak(Box::new(api)),
        Err(error) => {
            // SAFETY: this handle is owned here and no function pointer escaped.
            unsafe { libc::dlclose(handle) };
            return Err(error);
        }
    };
    *loaded = Some((path.to_owned(), api));
    Ok(api)
}

fn dl_error() -> String {
    // SAFETY: dlerror returns null or a thread-local NUL-terminated error string.
    let error = unsafe { libc::dlerror() };
    if error.is_null() {
        "dlopen failed without an error message".into()
    } else {
        // SAFETY: error is nonnull and remains valid until the next loader call.
        unsafe { CStr::from_ptr(error) }
            .to_string_lossy()
            .into_owned()
    }
}

fn check_version(api: &Api) -> Result<(), LoadError> {
    // SAFETY: the table points to the loaded ABI's argument-free version function.
    let found = unsafe { (api.abi_version)() };
    if found == ABI_VERSION {
        Ok(())
    } else {
        Err(LoadError::AbiMismatch {
            found,
            expected: ABI_VERSION,
        })
    }
}

#[cfg(not(all(feature = "static-link", target_os = "ios")))]
pub fn load_default() -> Result<&'static Api, LoadError> {
    if let Some(path) = std::env::var_os("RS_TAILNET_LIB").filter(|value| !value.is_empty()) {
        return load(Path::new(&path));
    }
    #[cfg(target_os = "android")]
    return load(Path::new("librs_tailnet.so"));
    #[cfg(not(target_os = "android"))]
    Err(LoadError::Unavailable("set RS_TAILNET_LIB"))
}

#[cfg(all(feature = "static-link", target_os = "ios"))]
pub fn load_default() -> Result<&'static Api, LoadError> {
    use std::ffi::c_char;
    // SAFETY: these declarations match ABI v1; build.rs supplies its static archive.
    unsafe extern "C" {
        fn rs_ts_abi_version() -> i32;
        fn rs_ts_start(config: *const c_char, err: *mut c_char, cap: usize) -> i32;
        fn rs_ts_stop() -> i32;
        fn rs_ts_login(err: *mut c_char, cap: usize) -> i32;
        fn rs_ts_logout(err: *mut c_char, cap: usize) -> i32;
        fn rs_ts_rebind() -> i32;
        fn rs_ts_status(buf: *mut c_char, cap: usize) -> i64;
        fn rs_ts_next_event(buf: *mut c_char, cap: usize, timeout: i32) -> i64;
        fn rs_ts_dial_udp(
            host: *const c_char,
            port: u16,
            timeout: i32,
            err: *mut c_char,
            cap: usize,
        ) -> i32;
        fn rs_ts_dial_tcp(
            host: *const c_char,
            port: u16,
            timeout: i32,
            err: *mut c_char,
            cap: usize,
        ) -> i32;
    }
    static API: Api = Api {
        abi_version: rs_ts_abi_version,
        start: rs_ts_start,
        stop: rs_ts_stop,
        login: rs_ts_login,
        logout: rs_ts_logout,
        rebind: rs_ts_rebind,
        status: rs_ts_status,
        next_event: rs_ts_next_event,
        dial_udp: rs_ts_dial_udp,
        dial_tcp: rs_ts_dial_tcp,
    };
    let mut loaded = LOADED
        .get_or_init(|| Mutex::new(None))
        .lock()
        .unwrap_or_else(|e| e.into_inner());
    let path = PathBuf::from("<static rs_tailnet>");
    if let Some((previous, api)) = loaded.as_ref() {
        if !std::ptr::eq(*api, &API) {
            return Err(LoadError::AlreadyLoaded {
                loaded: previous.clone(),
                requested: path,
            });
        }
        return Ok(api);
    }
    check_version(&API)?;
    *loaded = Some((path, &API));
    Ok(&API)
}
