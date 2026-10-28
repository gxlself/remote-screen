//! dlopen / dlsym 的最小封装（V8 C1）。PipeWire和 FFmpeg共用。

use std::ffi::{CStr, CString};
use std::os::unix::ffi::OsStrExt;
use std::path::Path;

/// 一个 dlopen 句柄；drop 时 dlclose。PipeWire 的句柄放进 OnceLock，进程内不关闭。
#[derive(Debug)]
pub struct DynLib {
    handle: *mut libc::c_void,
    name: String,
}

// SAFETY: dlopen 句柄是进程级的，dlsym / dlclose 是线程安全的（POSIX）；DynLib 没有可变状态。
unsafe impl Send for DynLib {}
// SAFETY: 同上。
unsafe impl Sync for DynLib {}

impl DynLib {
    /// `path` 是 soname（如 `libpipewire-0.3.so.0`，走系统搜索路径）或绝对路径；RTLD_NOW | RTLD_LOCAL。
    pub fn open(path: &Path) -> Result<Self, String> {
        let name = path.display().to_string();
        let c = CString::new(path.as_os_str().as_bytes())
            .map_err(|_| format!("{name}: path contains NUL"))?;
        // SAFETY: c 是以 NUL 结尾的路径，dlopen 不保留这个指针。
        let handle = unsafe { libc::dlopen(c.as_ptr(), libc::RTLD_NOW | libc::RTLD_LOCAL) };
        if handle.is_null() {
            return Err(format!("dlopen {name}: {}", last_error()));
        }
        Ok(Self { handle, name })
    }

    /// 符号地址；没有时 Err。调用方负责 transmute 成正确的函数指针类型（写 SAFETY）。
    pub fn symbol(&self, symbol: &CStr) -> Result<*mut libc::c_void, String> {
        // SAFETY: handle 来自成功的 dlopen，symbol 以 NUL 结尾。
        let ptr = unsafe { libc::dlsym(self.handle, symbol.as_ptr()) };
        if ptr.is_null() {
            return Err(format!(
                "{}: missing symbol {}",
                self.name,
                symbol.to_string_lossy()
            ));
        }
        Ok(ptr)
    }

    pub fn name(&self) -> &str {
        &self.name
    }
}

impl Drop for DynLib {
    fn drop(&mut self) {
        // SAFETY: handle 来自成功的 dlopen，只在这里关闭一次。
        unsafe { libc::dlclose(self.handle) };
    }
}

fn last_error() -> String {
    // SAFETY: dlerror 返回线程局部的 C 字符串或 NULL。
    let ptr = unsafe { libc::dlerror() };
    if ptr.is_null() {
        return "unknown error".into();
    }
    // SAFETY: 非空时是 NUL 结尾的字符串，在本线程下一次 dl* 调用前有效。
    unsafe { CStr::from_ptr(ptr) }
        .to_string_lossy()
        .into_owned()
}
