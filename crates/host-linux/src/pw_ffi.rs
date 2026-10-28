//! The small subset of the PipeWire ABI used by the Linux host.
//!
//! PipeWire is deliberately loaded at run time. Keeping these declarations
//! here avoids requiring PipeWire development headers (or bindgen) when the
//! host is built.

use std::ffi::{c_char, c_int, c_void, CStr};

use crate::dl::DynLib;

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaBuffer {
    pub n_metas: u32,
    pub n_datas: u32,
    pub metas: *mut SpaMeta,
    pub datas: *mut SpaData,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaMeta {
    pub type_: u32,
    pub size: u32,
    pub data: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaData {
    pub type_: u32,
    pub flags: u32,
    pub fd: i64,
    pub mapoffset: u32,
    pub maxsize: u32,
    pub data: *mut c_void,
    pub chunk: *mut SpaChunk,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaChunk {
    pub offset: u32,
    pub size: u32,
    pub stride: i32,
    pub flags: i32,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaDictItem {
    pub key: *const c_char,
    pub value: *const c_char,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaDict {
    pub flags: u32,
    pub n_items: u32,
    pub items: *const SpaDictItem,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaCallbacks {
    pub funcs: *const c_void,
    pub data: *mut c_void,
}

/// The hook is intentionally represented with pointers. `spa_list` is two
/// pointers and `spa_callbacks` is two pointers on the supported LP64 targets.
#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaHook {
    pub link_next: *mut c_void,
    pub link_prev: *mut c_void,
    pub cb: SpaCallbacks,
    pub removed: Option<unsafe extern "C" fn(*mut SpaHook)>,
    pub priv_: *mut c_void,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwBufferRaw {
    pub buffer: *mut SpaBuffer,
    pub user_data: *mut c_void,
    pub size: u64,
    pub requested: u64,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwStreamEvents {
    pub version: u32,
    pub destroy: Option<unsafe extern "C" fn(*mut c_void)>,
    pub state_changed: Option<unsafe extern "C" fn(*mut c_void, c_int, c_int, *const c_char)>,
    pub control_info: Option<unsafe extern "C" fn(*mut c_void, u32, *const PwStreamControlRaw)>,
    pub io_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *mut c_void, u32)>,
    pub param_changed: Option<unsafe extern "C" fn(*mut c_void, u32, *const SpaPodRaw)>,
    pub add_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBufferRaw)>,
    pub remove_buffer: Option<unsafe extern "C" fn(*mut c_void, *mut PwBufferRaw)>,
    pub process: Option<unsafe extern "C" fn(*mut c_void)>,
    pub drained: Option<unsafe extern "C" fn(*mut c_void)>,
    pub command: Option<unsafe extern "C" fn(*mut c_void, *const SpaPodRaw)>,
    pub trigger_done: Option<unsafe extern "C" fn(*mut c_void)>,
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwThreadLoopRaw {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwLoopRaw {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwContextRaw {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwCoreRaw {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwPropertiesRaw {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwStreamRaw {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct PwStreamControlRaw {
    _p: [u8; 0],
}

#[repr(C)]
#[derive(Debug, Clone, Copy)]
pub struct SpaPodRaw {
    _p: [u8; 0],
}

pub mod state {
    pub const ERROR: i32 = -1;
    pub const UNCONNECTED: i32 = 0;
    pub const CONNECTING: i32 = 1;
    pub const PAUSED: i32 = 2;
    pub const STREAMING: i32 = 3;
}

pub mod direction {
    pub const INPUT: i32 = 0;
    pub const OUTPUT: i32 = 1;
}

pub mod stream_flag {
    pub const AUTOCONNECT: u32 = 1 << 0;
    pub const INACTIVE: u32 = 1 << 1;
    pub const MAP_BUFFERS: u32 = 1 << 2;
    pub const DRIVER: u32 = 1 << 3;
    pub const RT_PROCESS: u32 = 1 << 4;
    pub const DONT_RECONNECT: u32 = 1 << 7;
    pub const ALLOC_BUFFERS: u32 = 1 << 8;
}

pub mod data_type {
    pub const MEM_PTR: u32 = 1;
    pub const MEM_FD: u32 = 2;
    pub const DMA_BUF: u32 = 3;
}

pub const SPA_DATA_MEM_PTR: u32 = data_type::MEM_PTR;
pub const SPA_DATA_MEM_FD: u32 = data_type::MEM_FD;
pub const SPA_DATA_DMA_BUF: u32 = data_type::DMA_BUF;

pub const PW_ID_ANY: u32 = u32::MAX;
pub const PW_VERSION_STREAM_EVENTS: u32 = 2;

pub type PwInit = unsafe extern "C" fn(*mut c_int, *mut *mut *mut c_char);
pub type PwGetLibraryVersion = unsafe extern "C" fn() -> *const c_char;
pub type PwThreadLoopNew =
    unsafe extern "C" fn(*const c_char, *const SpaDict) -> *mut PwThreadLoopRaw;
pub type PwThreadLoopGetLoop = unsafe extern "C" fn(*mut PwThreadLoopRaw) -> *mut PwLoopRaw;
pub type PwThreadLoopStart = unsafe extern "C" fn(*mut PwThreadLoopRaw) -> c_int;
pub type PwThreadLoopStop = unsafe extern "C" fn(*mut PwThreadLoopRaw);
pub type PwThreadLoopLock = unsafe extern "C" fn(*mut PwThreadLoopRaw);
pub type PwThreadLoopUnlock = unsafe extern "C" fn(*mut PwThreadLoopRaw);
pub type PwThreadLoopDestroy = unsafe extern "C" fn(*mut PwThreadLoopRaw);
pub type PwContextNew =
    unsafe extern "C" fn(*mut PwLoopRaw, *const SpaDict, usize) -> *mut PwContextRaw;
pub type PwContextDestroy = unsafe extern "C" fn(*mut PwContextRaw);
pub type PwContextConnect =
    unsafe extern "C" fn(*mut PwContextRaw, *mut PwPropertiesRaw, usize) -> *mut PwCoreRaw;
pub type PwContextConnectFd =
    unsafe extern "C" fn(*mut PwContextRaw, c_int, *mut PwPropertiesRaw, usize) -> *mut PwCoreRaw;
pub type PwCoreDisconnect = unsafe extern "C" fn(*mut PwCoreRaw) -> c_int;
pub type PwPropertiesNewDict = unsafe extern "C" fn(*const SpaDict) -> *mut PwPropertiesRaw;
pub type PwStreamNew =
    unsafe extern "C" fn(*mut PwCoreRaw, *const c_char, *mut PwPropertiesRaw) -> *mut PwStreamRaw;
pub type PwStreamAddListener =
    unsafe extern "C" fn(*mut PwStreamRaw, *mut SpaHook, *const PwStreamEvents, *mut c_void);
pub type PwStreamConnect =
    unsafe extern "C" fn(*mut PwStreamRaw, c_int, u32, u32, *const *const SpaPodRaw, u32) -> c_int;
pub type PwStreamDisconnect = unsafe extern "C" fn(*mut PwStreamRaw) -> c_int;
pub type PwStreamDestroy = unsafe extern "C" fn(*mut PwStreamRaw);
pub type PwStreamUpdateParams =
    unsafe extern "C" fn(*mut PwStreamRaw, *const *const SpaPodRaw, u32) -> c_int;
pub type PwStreamDequeueBuffer = unsafe extern "C" fn(*mut PwStreamRaw) -> *mut PwBufferRaw;
pub type PwStreamQueueBuffer = unsafe extern "C" fn(*mut PwStreamRaw, *mut PwBufferRaw) -> c_int;
pub type PwStreamGetNodeId = unsafe extern "C" fn(*mut PwStreamRaw) -> u32;
pub type PwStreamTriggerProcess = unsafe extern "C" fn(*mut PwStreamRaw) -> c_int;
pub type PwStreamSetActive = unsafe extern "C" fn(*mut PwStreamRaw, bool) -> c_int;

pub struct PwFns {
    pub pw_init: PwInit,
    pub pw_get_library_version: PwGetLibraryVersion,
    pub pw_thread_loop_new: PwThreadLoopNew,
    pub pw_thread_loop_get_loop: PwThreadLoopGetLoop,
    pub pw_thread_loop_start: PwThreadLoopStart,
    pub pw_thread_loop_stop: PwThreadLoopStop,
    pub pw_thread_loop_lock: PwThreadLoopLock,
    pub pw_thread_loop_unlock: PwThreadLoopUnlock,
    pub pw_thread_loop_destroy: PwThreadLoopDestroy,
    pub pw_context_new: PwContextNew,
    pub pw_context_destroy: PwContextDestroy,
    pub pw_context_connect: PwContextConnect,
    pub pw_context_connect_fd: PwContextConnectFd,
    pub pw_core_disconnect: PwCoreDisconnect,
    pub pw_properties_new_dict: PwPropertiesNewDict,
    pub pw_stream_new: PwStreamNew,
    pub pw_stream_add_listener: PwStreamAddListener,
    pub pw_stream_connect: PwStreamConnect,
    pub pw_stream_disconnect: PwStreamDisconnect,
    pub pw_stream_destroy: PwStreamDestroy,
    pub pw_stream_update_params: PwStreamUpdateParams,
    pub pw_stream_dequeue_buffer: PwStreamDequeueBuffer,
    pub pw_stream_queue_buffer: PwStreamQueueBuffer,
    pub pw_stream_get_node_id: PwStreamGetNodeId,
    pub pw_stream_trigger_process: PwStreamTriggerProcess,
    pub pw_stream_set_active: PwStreamSetActive,
}

macro_rules! load_symbol {
    ($lib:expr, $name:literal, $ty:ty) => {{
        let symbol = CStr::from_bytes_with_nul(concat!($name, "\0").as_bytes())
            .expect("static PipeWire symbol has no interior NUL");
        let address = $lib.symbol(symbol)?;
        // SAFETY: Each symbol name and function pointer type below comes from
        // the matching public PipeWire C declaration in the supported ABI.
        // SAFETY: See the symbol declaration comment above.
        unsafe { std::mem::transmute::<*mut c_void, $ty>(address) }
    }};
}

impl PwFns {
    pub fn load(lib: &DynLib) -> Result<Self, String> {
        Ok(Self {
            pw_init: load_symbol!(lib, "pw_init", PwInit),
            pw_get_library_version: load_symbol!(
                lib,
                "pw_get_library_version",
                PwGetLibraryVersion
            ),
            pw_thread_loop_new: load_symbol!(lib, "pw_thread_loop_new", PwThreadLoopNew),
            pw_thread_loop_get_loop: load_symbol!(
                lib,
                "pw_thread_loop_get_loop",
                PwThreadLoopGetLoop
            ),
            pw_thread_loop_start: load_symbol!(lib, "pw_thread_loop_start", PwThreadLoopStart),
            pw_thread_loop_stop: load_symbol!(lib, "pw_thread_loop_stop", PwThreadLoopStop),
            pw_thread_loop_lock: load_symbol!(lib, "pw_thread_loop_lock", PwThreadLoopLock),
            pw_thread_loop_unlock: load_symbol!(lib, "pw_thread_loop_unlock", PwThreadLoopUnlock),
            pw_thread_loop_destroy: load_symbol!(
                lib,
                "pw_thread_loop_destroy",
                PwThreadLoopDestroy
            ),
            pw_context_new: load_symbol!(lib, "pw_context_new", PwContextNew),
            pw_context_destroy: load_symbol!(lib, "pw_context_destroy", PwContextDestroy),
            pw_context_connect: load_symbol!(lib, "pw_context_connect", PwContextConnect),
            pw_context_connect_fd: load_symbol!(lib, "pw_context_connect_fd", PwContextConnectFd),
            pw_core_disconnect: load_symbol!(lib, "pw_core_disconnect", PwCoreDisconnect),
            pw_properties_new_dict: load_symbol!(
                lib,
                "pw_properties_new_dict",
                PwPropertiesNewDict
            ),
            pw_stream_new: load_symbol!(lib, "pw_stream_new", PwStreamNew),
            pw_stream_add_listener: load_symbol!(
                lib,
                "pw_stream_add_listener",
                PwStreamAddListener
            ),
            pw_stream_connect: load_symbol!(lib, "pw_stream_connect", PwStreamConnect),
            pw_stream_disconnect: load_symbol!(lib, "pw_stream_disconnect", PwStreamDisconnect),
            pw_stream_destroy: load_symbol!(lib, "pw_stream_destroy", PwStreamDestroy),
            pw_stream_update_params: load_symbol!(
                lib,
                "pw_stream_update_params",
                PwStreamUpdateParams
            ),
            pw_stream_dequeue_buffer: load_symbol!(
                lib,
                "pw_stream_dequeue_buffer",
                PwStreamDequeueBuffer
            ),
            pw_stream_queue_buffer: load_symbol!(
                lib,
                "pw_stream_queue_buffer",
                PwStreamQueueBuffer
            ),
            pw_stream_get_node_id: load_symbol!(lib, "pw_stream_get_node_id", PwStreamGetNodeId),
            pw_stream_trigger_process: load_symbol!(
                lib,
                "pw_stream_trigger_process",
                PwStreamTriggerProcess
            ),
            pw_stream_set_active: load_symbol!(lib, "pw_stream_set_active", PwStreamSetActive),
        })
    }
}

pub const MIN_VERSION: (u32, u32, u32) = (0, 3, 50);

pub fn parse_version(s: &str) -> Option<(u32, u32, u32)> {
    let mut parts = s.split('.');
    let major = parts.next()?.parse().ok()?;
    let minor = parts.next().map_or(Ok(0), str::parse).ok()?;
    let patch = parts.next().map_or(Ok(0), str::parse).ok()?;
    if parts.next().is_some() {
        return None;
    }
    Some((major, minor, patch))
}
