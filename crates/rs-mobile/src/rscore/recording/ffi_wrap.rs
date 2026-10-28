//! 原生录制密钥包裹回调（rs_mobile.h `rs_register_secret_wrapper`）。
use std::{
    ffi::c_void,
    panic::{catch_unwind, AssertUnwindSafe},
    sync::{Arc, RwLock},
};

use rs_recording::crypto::{SecretWrapper, WrapError};
use tracing::error;
use zeroize::Zeroizing;

use crate::rscore::ffi_session::guard;

pub type RsSecretWrapCb =
    extern "C" fn(*mut c_void, u8, *const u8, usize, *mut u8, *mut usize) -> i32;

const OP_WRAP: u8 = 1;
const OP_UNWRAP: u8 = 2;
/// 交给回调的输出容量（rs_mobile.h：Rust 给 256 字节）。
const OUT_CAP: usize = 256;
const KEY_LEN: usize = 32;

static WRAPPER: RwLock<Option<Arc<dyn SecretWrapper>>> = RwLock::new(None);

/// 当前的包裹实现：原生注册的，或测试用 `set_wrapper` 放进来的。
pub fn wrapper() -> Option<Arc<dyn SecretWrapper>> {
    WRAPPER.read().unwrap_or_else(|e| e.into_inner()).clone()
}

/// 原生注册与测试共用；None 清除。
pub fn set_wrapper(wrapper: Option<Arc<dyn SecretWrapper>>) {
    *WRAPPER.write().unwrap_or_else(|e| e.into_inner()) = wrapper;
}

struct FfiSecretWrapper {
    ctx: *mut c_void,
    cb: RsSecretWrapCb,
}

// SAFETY: Registration requires the native ctx/callback to remain valid and
// support arbitrary, potentially concurrent threads for the wrapper's lifetime.
unsafe impl Send for FfiSecretWrapper {}
// SAFETY: Same native thread-safety contract; the adapter itself is immutable.
unsafe impl Sync for FfiSecretWrapper {}

impl FfiSecretWrapper {
    /// 调一次回调，返回写出的长度。返回码按 rs_mobile.h：-2、-3 拒绝，其他非 0 不可用。
    fn call(&self, op: u8, input: &[u8], out: &mut [u8; OUT_CAP]) -> Result<usize, WrapError> {
        let mut len = out.len();
        let status = (self.cb)(
            self.ctx,
            op,
            input.as_ptr(),
            input.len(),
            out.as_mut_ptr(),
            &mut len,
        );
        match status {
            0 if len <= out.len() => Ok(len),
            0 | -2 => Err(WrapError::Rejected),
            -3 => {
                error!(op, "native secret wrapper reported a short output buffer");
                Err(WrapError::Rejected)
            }
            _ => Err(WrapError::Unavailable),
        }
    }
}

impl SecretWrapper for FfiSecretWrapper {
    fn wrap(&self, key: &[u8; 32]) -> Result<Vec<u8>, WrapError> {
        contain(|| {
            let mut out = [0u8; OUT_CAP];
            match self.call(OP_WRAP, key, &mut out)? {
                len @ 1..=255 => Ok(out[..len].to_vec()),
                _ => Err(WrapError::Rejected),
            }
        })
    }

    fn unwrap(&self, wrapped: &[u8]) -> Result<Zeroizing<[u8; 32]>, WrapError> {
        contain(|| {
            // 回调写进来的是明文文件密钥：缓冲在任何出口都清零。
            let mut out = Zeroizing::new([0u8; OUT_CAP]);
            if self.call(OP_UNWRAP, wrapped, &mut out)? != KEY_LEN {
                return Err(WrapError::Rejected);
            }
            let mut key = Zeroizing::new([0u8; KEY_LEN]);
            key.copy_from_slice(&out[..KEY_LEN]);
            Ok(key)
        })
    }

    fn kind(&self) -> u8 {
        1
    }
}

fn contain<T>(f: impl FnOnce() -> Result<T, WrapError>) -> Result<T, WrapError> {
    catch_unwind(AssertUnwindSafe(f)).unwrap_or_else(|_| {
        error!("panic contained in native secret wrapper adapter");
        Err(WrapError::Unavailable)
    })
}

/// 注册原生包裹回调；NULL cb 被忽略，保留上一次注册。
/// # Safety
/// 见 rs_mobile.h：ctx/cb 须在进程生命周期内有效，或者到下一次注册、且旧回调的调用全部结束为止。
/// 回调可能在任意线程（含 spawn_blocking）上并发调用，不得同步等主线程，不得 unwind。
#[no_mangle]
pub unsafe extern "C" fn rs_register_secret_wrapper(ctx: *mut c_void, cb: Option<RsSecretWrapCb>) {
    guard(|| match cb {
        Some(cb) => set_wrapper(Some(Arc::new(FfiSecretWrapper { ctx, cb }))),
        None => error!("ignored NULL native secret wrapper registration"),
    });
}
