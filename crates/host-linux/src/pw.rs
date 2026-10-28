//! Safe, lock-serialized wrappers around the small PipeWire ABI in
//! [`crate::pw_ffi`].

use std::ffi::{c_char, c_void, CStr, CString};
use std::marker::PhantomData;
use std::os::fd::{IntoRawFd, OwnedFd};
use std::path::Path;
use std::ptr;
use std::slice;
use std::sync::{Arc, OnceLock};

use thiserror::Error;

use crate::dl::DynLib;
use crate::pw_ffi::{
    self, direction, PwBufferRaw, PwContextRaw, PwCoreRaw, PwFns, PwPropertiesRaw, PwStreamEvents,
    PwStreamRaw, SpaBuffer, SpaChunk, SpaDict, SpaDictItem, SpaHook, SpaPodRaw,
};

#[derive(Debug, Error, Clone)]
pub enum PwError {
    #[error("libpipewire: {0}")]
    Load(String),
    #[error("PipeWire {0} is older than 0.3.50")]
    TooOld(String),
    #[error("{0} failed")]
    Create(&'static str),
    #[error("{call} failed: {code}")]
    Call { call: &'static str, code: i32 },
}

pub struct PwLib {
    _lib: DynLib,
    pub(crate) fns: PwFns,
    version: String,
}

static PW_LIB: OnceLock<Result<Arc<PwLib>, PwError>> = OnceLock::new();

impl PwLib {
    /// Load PipeWire once for this process. The dynamic-library handle is kept
    /// alive for the process lifetime because all streams retain its symbols.
    pub fn load() -> Result<Arc<PwLib>, PwError> {
        PW_LIB
            .get_or_init(|| {
                let lib = DynLib::open(Path::new("libpipewire-0.3.so.0")).map_err(PwError::Load)?;
                let fns = PwFns::load(&lib).map_err(PwError::Load)?;

                // SAFETY: NULL argc/argv is the documented pw_init(NULL, NULL)
                // form, and this is executed once by the process OnceLock.
                unsafe { (fns.pw_init)(ptr::null_mut(), ptr::null_mut()) };
                // SAFETY: pw_get_library_version returns a process-owned,
                // NUL-terminated string for the lifetime of the library.
                let version_ptr = unsafe { (fns.pw_get_library_version)() };
                if version_ptr.is_null() {
                    return Err(PwError::Load("pw_get_library_version returned NULL".into()));
                }
                // SAFETY: PipeWire documents this result as a valid C string.
                let version = unsafe { CStr::from_ptr(version_ptr) }
                    .to_string_lossy()
                    .into_owned();
                let parsed = pw_ffi::parse_version(&version)
                    .ok_or_else(|| PwError::Load(format!("invalid PipeWire version {version}")))?;
                if parsed < pw_ffi::MIN_VERSION {
                    return Err(PwError::TooOld(version));
                }
                tracing::debug!(version = %version, "loaded PipeWire");
                Ok(Arc::new(PwLib {
                    _lib: lib,
                    fns,
                    version,
                }))
            })
            .clone()
    }

    pub fn version(&self) -> &str {
        &self.version
    }
}

pub enum Remote {
    /// The fd returned by portal OpenPipeWireRemote. PipeWire takes ownership
    /// of it, including on a failed connection attempt.
    Fd(OwnedFd),
    /// Connect to the default PipeWire daemon.
    Default,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum Direction {
    Input,
    Output,
}

pub struct StreamSpec {
    pub name: String,
    pub props: Vec<(String, String)>,
    pub direction: Direction,
    pub target: Option<u32>,
    pub flags: u32,
    pub params: Vec<Vec<u8>>,
}

pub trait StreamHandler: Send + 'static {
    fn state_changed(&mut self, _old: i32, _new: i32, _error: Option<&str>) {}
    fn param_changed(&mut self, _ctl: &mut StreamCtl<'_>, _id: u32, _pod: Option<&[u8]>) {}
    fn process(&mut self, _ctl: &mut StreamCtl<'_>) {}
}

struct Inner {
    handler: Box<dyn StreamHandler>,
    poisoned: bool,
    fns: *const PwFns,
    stream: *mut PwStreamRaw,
}

struct BufferStorage {
    data: Vec<Box<[u8]>>,
    chunks: Vec<SpaChunk>,
    metas: Vec<Box<[u8]>>,
}

pub struct StreamCtl<'a> {
    fns: &'a PwFns,
    stream: *mut PwStreamRaw,
}

impl StreamCtl<'_> {
    pub fn update_params(&mut self, params: &[Vec<u8>]) -> Result<(), PwError> {
        let pods: Vec<AlignedPod> = params.iter().map(|param| AlignedPod::new(param)).collect();
        let pointers: Vec<*const SpaPodRaw> = pods.iter().map(AlignedPod::as_ptr).collect();
        let count = u32::try_from(pointers.len()).map_err(|_| PwError::Create("params"))?;
        // SAFETY: the stream is live while a callback owns this control, and
        // `pods` keeps every 8-byte-aligned POD alive for the call.
        let code =
            unsafe { (self.fns.pw_stream_update_params)(self.stream, pointers.as_ptr(), count) };
        if code < 0 {
            Err(PwError::Call {
                call: "pw_stream_update_params",
                code,
            })
        } else {
            Ok(())
        }
    }

    pub fn dequeue(&self) -> Option<PwBuffer<'_>> {
        // SAFETY: the PipeWire callback owns the stream-loop lock and the
        // returned buffer remains owned by the stream until queue is called.
        let raw = unsafe { (self.fns.pw_stream_dequeue_buffer)(self.stream) };
        if raw.is_null() {
            None
        } else {
            // SAFETY: PipeWire returned a non-null pw_buffer belonging to this
            // stream, and the lifetime is tied to this callback control.
            Some(unsafe { PwBuffer::from_raw(raw) })
        }
    }

    pub fn queue(&self, buf: PwBuffer<'_>) {
        let raw = buf.raw;
        // SAFETY: raw was returned by this stream's dequeue operation and is
        // consumed exactly once by PipeWire here.
        let _ = unsafe { (self.fns.pw_stream_queue_buffer)(self.stream, raw) };
    }

    pub fn node_id(&self) -> u32 {
        // SAFETY: self.stream is live for the duration of the callback or
        // serialized PwStream operation.
        unsafe { (self.fns.pw_stream_get_node_id)(self.stream) }
    }
}

#[derive(Debug)]
pub struct PwBuffer<'a> {
    raw: *mut PwBufferRaw,
    _lifetime: PhantomData<&'a StreamCtl<'a>>,
}

impl<'a> PwBuffer<'a> {
    /// Construct a view over a PipeWire buffer. This is exposed for ABI tests;
    /// production callers obtain buffers only through `StreamCtl::dequeue`.
    #[doc(hidden)]
    pub unsafe fn from_raw(raw: *mut PwBufferRaw) -> Self {
        Self {
            raw,
            _lifetime: PhantomData,
        }
    }

    fn spa_buffer(&self) -> Option<&SpaBuffer> {
        // SAFETY: callers only create this view from a live PipeWire buffer or
        // from the test's explicitly constructed equivalent.
        let raw = unsafe { self.raw.as_ref()? };
        // SAFETY: a pw_buffer's buffer pointer is the first field documented by
        // the PipeWire ABI.
        unsafe { raw.buffer.as_ref() }
    }

    pub fn n_datas(&self) -> usize {
        self.spa_buffer()
            .map_or(0, |buffer| buffer.n_datas as usize)
    }

    pub fn data(&self, index: usize) -> Option<DataView<'_>> {
        let buffer = self.spa_buffer()?;
        if index >= buffer.n_datas as usize || buffer.datas.is_null() {
            return None;
        }
        // SAFETY: index is bounded by the n_datas field supplied by PipeWire.
        let data = unsafe { &*buffer.datas.add(index) };
        let chunk = if data.chunk.is_null() {
            Chunk {
                offset: 0,
                size: 0,
                stride: 0,
                flags: 0,
            }
        } else {
            // SAFETY: a non-null chunk points at the documented four-word
            // chunk owned by the current spa_data.
            let chunk = unsafe { &*data.chunk };
            Chunk {
                offset: chunk.offset,
                size: chunk.size,
                stride: chunk.stride,
                flags: chunk.flags,
            }
        };
        let bytes = if data.data.is_null() || data.type_ == pw_ffi::data_type::DMA_BUF {
            None
        } else {
            // SAFETY: PipeWire's maxsize describes the mapped region beginning
            // at data, and the view is borrowed for this callback only.
            Some(unsafe { slice::from_raw_parts(data.data.cast::<u8>(), data.maxsize as usize) })
        };
        Some(DataView {
            data_type: data.type_,
            flags: data.flags,
            fd: data.fd,
            mapoffset: data.mapoffset,
            maxsize: data.maxsize,
            bytes,
            chunk,
        })
    }

    /// Return the mapped region for a producer. Unmapped and DMA-BUF data has
    /// no Rust slice representation.
    pub fn data_mut(&mut self, index: usize) -> Option<&mut [u8]> {
        let buffer = self.spa_buffer()?;
        if index >= buffer.n_datas as usize || buffer.datas.is_null() {
            return None;
        }
        // SAFETY: index is bounded by n_datas.
        let data = unsafe { &mut *buffer.datas.add(index) };
        if data.data.is_null() || data.type_ == pw_ffi::data_type::DMA_BUF {
            return None;
        }
        // SAFETY: the caller has exclusive access to this dequeued producer
        // buffer and maxsize describes the mapped region.
        Some(unsafe { slice::from_raw_parts_mut(data.data.cast::<u8>(), data.maxsize as usize) })
    }

    pub fn set_chunk(&mut self, index: usize, offset: u32, size: u32, stride: i32, flags: i32) {
        let Some(buffer) = self.spa_buffer() else {
            return;
        };
        if index >= buffer.n_datas as usize || buffer.datas.is_null() {
            return;
        }
        // SAFETY: index is bounded by n_datas.
        let data = unsafe { &mut *buffer.datas.add(index) };
        if data.chunk.is_null() {
            return;
        }
        // SAFETY: a non-null chunk is the four-word chunk for this data slot.
        let chunk = unsafe { &mut *data.chunk };
        chunk.offset = offset;
        chunk.size = size;
        chunk.stride = stride;
        chunk.flags = flags;
    }

    pub fn meta(&self, wanted_type: u32) -> Option<&[u8]> {
        let buffer = self.spa_buffer()?;
        if buffer.n_metas == 0 || buffer.metas.is_null() {
            return None;
        }
        // SAFETY: PipeWire's n_metas field bounds the metadata array.
        let metas = unsafe { slice::from_raw_parts(buffer.metas, buffer.n_metas as usize) };
        let meta = metas.iter().find(|meta| meta.type_ == wanted_type)?;
        if meta.size == 0 {
            return Some(&[]);
        }
        if meta.data.is_null() {
            return None;
        }
        // SAFETY: spa_meta.size describes the metadata block at data.
        Some(unsafe { slice::from_raw_parts(meta.data.cast::<u8>(), meta.size as usize) })
    }

    pub fn meta_mut(&mut self, wanted_type: u32) -> Option<&mut [u8]> {
        let buffer = self.spa_buffer()?;
        if buffer.n_metas == 0 || buffer.metas.is_null() {
            return None;
        }
        // SAFETY: PipeWire's n_metas field bounds the metadata array.
        let metas = unsafe { slice::from_raw_parts_mut(buffer.metas, buffer.n_metas as usize) };
        let meta = metas.iter_mut().find(|meta| meta.type_ == wanted_type)?;
        if meta.size == 0 {
            return Some(&mut []);
        }
        if meta.data.is_null() {
            return None;
        }
        // SAFETY: the caller exclusively owns this dequeued metadata block.
        Some(unsafe { slice::from_raw_parts_mut(meta.data.cast::<u8>(), meta.size as usize) })
    }
}

#[derive(Debug)]
pub struct DataView<'a> {
    pub data_type: u32,
    pub flags: u32,
    pub fd: i64,
    pub mapoffset: u32,
    pub maxsize: u32,
    pub bytes: Option<&'a [u8]>,
    pub chunk: Chunk,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Chunk {
    pub offset: u32,
    pub size: u32,
    pub stride: i32,
    pub flags: i32,
}

impl DataView<'_> {
    pub fn valid(&self) -> Option<&[u8]> {
        let bytes = self.bytes?;
        let maxsize = usize::try_from(self.maxsize).ok()?;
        if maxsize == 0 || bytes.len() < maxsize {
            return None;
        }
        let start = usize::try_from(self.chunk.offset).ok()? % maxsize;
        let size = usize::try_from(self.chunk.size).ok()?;
        let end = start.checked_add(size)?;
        if end > maxsize {
            return None;
        }
        bytes.get(start..end)
    }
}

struct AlignedPod {
    words: Vec<u64>,
}

impl AlignedPod {
    fn new(bytes: &[u8]) -> Self {
        let words_len = bytes.len().saturating_add(7) / 8;
        let mut words = vec![0_u64; words_len.max(1)];
        // SAFETY: u64 storage is contiguous and at least bytes.len() bytes
        // long; u8 has no alignment requirement.
        let storage =
            unsafe { slice::from_raw_parts_mut(words.as_mut_ptr().cast::<u8>(), words.len() * 8) };
        storage[..bytes.len()].copy_from_slice(bytes);
        Self { words }
    }

    fn as_ptr(&self) -> *const SpaPodRaw {
        self.words.as_ptr().cast::<SpaPodRaw>()
    }
}

fn pod_slice<'a>(pod: *const SpaPodRaw) -> Option<&'a [u8]> {
    if pod.is_null() {
        return None;
    }
    // SAFETY: PipeWire presents a spa_pod with a readable u32 size header.
    let size = unsafe { ptr::read_unaligned(pod.cast::<u32>()) as usize };
    let total = size.checked_add(8)?.checked_add(7)? & !7;
    if total > isize::MAX as usize {
        return None;
    }
    // SAFETY: the callback's pod remains owned by PipeWire for its duration;
    // the computed size includes the header and SPA's eight-byte padding.
    Some(unsafe { slice::from_raw_parts(pod.cast::<u8>(), total) })
}

unsafe fn cleanup_start(
    fns: &PwFns,
    loop_ptr: *mut pw_ffi::PwThreadLoopRaw,
    context: *mut PwContextRaw,
    core: *mut PwCoreRaw,
    stream: *mut PwStreamRaw,
    started: bool,
    locked: bool,
) {
    if locked {
        if !stream.is_null() {
            // SAFETY: cleanup holds the loop lock, and stream is owned here.
            let _ = unsafe { (fns.pw_stream_disconnect)(stream) };
            // SAFETY: stream is no longer used after this call.
            unsafe { (fns.pw_stream_destroy)(stream) };
        }
        if !core.is_null() {
            // SAFETY: core belongs to this context and is disconnected once.
            let _ = unsafe { (fns.pw_core_disconnect)(core) };
        }
        // SAFETY: this lock was acquired by the current thread.
        unsafe { (fns.pw_thread_loop_unlock)(loop_ptr) };
    }
    if started {
        // SAFETY: the loop was successfully started above.
        unsafe { (fns.pw_thread_loop_stop)(loop_ptr) };
    }
    if !context.is_null() {
        // SAFETY: context is no longer connected after the stop/disconnect.
        unsafe { (fns.pw_context_destroy)(context) };
    }
    // SAFETY: loop_ptr was returned by pw_thread_loop_new and is destroyed once.
    unsafe { (fns.pw_thread_loop_destroy)(loop_ptr) };
}

static EVENTS: PwStreamEvents = PwStreamEvents {
    version: pw_ffi::PW_VERSION_STREAM_EVENTS,
    destroy: None,
    state_changed: Some(on_state_changed),
    control_info: None,
    io_changed: None,
    param_changed: Some(on_param_changed),
    add_buffer: Some(on_add_buffer),
    remove_buffer: Some(on_remove_buffer),
    process: Some(on_process),
    drained: None,
    command: None,
    trigger_done: None,
};

unsafe extern "C" fn on_add_buffer(_data: *mut c_void, raw: *mut PwBufferRaw) {
    if raw.is_null() {
        return;
    }
    // SAFETY: PipeWire supplies a live pw_buffer while invoking add_buffer.
    let buffer = unsafe { (*raw).buffer.as_mut() };
    let Some(buffer) = buffer else { return };
    let mut storage = BufferStorage {
        data: Vec::new(),
        chunks: Vec::with_capacity(buffer.n_datas as usize),
        metas: Vec::new(),
    };
    if !buffer.datas.is_null() {
        // SAFETY: n_datas bounds the spa_data array supplied by PipeWire.
        let datas = unsafe { slice::from_raw_parts_mut(buffer.datas, buffer.n_datas as usize) };
        for item in datas {
            if item.data.is_null() && item.maxsize > 0 {
                let mut allocation = vec![0_u8; item.maxsize as usize].into_boxed_slice();
                item.data = allocation.as_mut_ptr().cast::<c_void>();
                storage.data.push(allocation);
            }
            if item.chunk.is_null() {
                storage.chunks.push(SpaChunk {
                    offset: 0,
                    size: 0,
                    stride: 0,
                    flags: 0,
                });
                item.chunk = storage
                    .chunks
                    .last_mut()
                    .map_or(ptr::null_mut(), |chunk| chunk as *mut SpaChunk);
            }
        }
    }
    if !buffer.metas.is_null() {
        // SAFETY: n_metas bounds the spa_meta array supplied by PipeWire.
        let metas = unsafe { slice::from_raw_parts_mut(buffer.metas, buffer.n_metas as usize) };
        for item in metas {
            if item.data.is_null() && item.size > 0 {
                let mut allocation = vec![0_u8; item.size as usize].into_boxed_slice();
                item.data = allocation.as_mut_ptr().cast::<c_void>();
                storage.metas.push(allocation);
            }
        }
    }
    // SAFETY: Box is intentionally transferred through the opaque user_data
    // slot and reclaimed by on_remove_buffer before stream destruction.
    unsafe { (*raw).user_data = Box::into_raw(Box::new(storage)).cast::<c_void>() };
}

unsafe extern "C" fn on_remove_buffer(_data: *mut c_void, raw: *mut PwBufferRaw) {
    if raw.is_null() {
        return;
    }
    // SAFETY: user_data was installed by on_add_buffer exactly once. Taking
    // the Box back also releases all data, chunk, and metadata allocations.
    let user_data = unsafe { (*raw).user_data };
    if !user_data.is_null() {
        // SAFETY: pointer originated from Box::into_raw in on_add_buffer.
        drop(unsafe { Box::from_raw(user_data.cast::<BufferStorage>()) });
        // SAFETY: raw is the live pw_buffer being removed.
        unsafe { (*raw).user_data = ptr::null_mut() };
    }
}

unsafe extern "C" fn on_state_changed(data: *mut c_void, old: i32, new: i32, error: *const c_char) {
    if data.is_null() {
        return;
    }
    // SAFETY: listener data is the stable Box<Inner> allocated by PwStream.
    let inner = unsafe { &mut *data.cast::<Inner>() };
    if inner.poisoned {
        return;
    }
    let error = if error.is_null() {
        None
    } else {
        // SAFETY: PipeWire supplies a NUL-terminated state error string.
        unsafe { CStr::from_ptr(error) }.to_str().ok()
    };
    tracing::debug!(old, new, error = ?error, "PipeWire stream state changed");
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inner.handler.state_changed(old, new, error);
    }));
    if result.is_err() {
        inner.poisoned = true;
        tracing::error!("PipeWire stream handler panicked");
    }
}

unsafe extern "C" fn on_param_changed(data: *mut c_void, id: u32, pod: *const SpaPodRaw) {
    if data.is_null() {
        return;
    }
    // SAFETY: listener data is the stable Box<Inner> allocated by PwStream.
    let inner = unsafe { &mut *data.cast::<Inner>() };
    if inner.poisoned {
        return;
    }
    let pod = pod_slice(pod);
    // SAFETY: PwStream stores the function-table pointer together with Inner
    // and keeps the Arc<PwLib> alive until after listener destruction.
    let fns = unsafe { &*inner.fns };
    let mut ctl = StreamCtl {
        fns,
        stream: inner.stream,
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inner.handler.param_changed(&mut ctl, id, pod);
    }));
    if result.is_err() {
        inner.poisoned = true;
        tracing::error!("PipeWire stream handler panicked");
    }
}

unsafe extern "C" fn on_process(data: *mut c_void) {
    if data.is_null() {
        return;
    }
    // SAFETY: listener data is the stable Box<Inner> allocated by PwStream.
    let inner = unsafe { &mut *data.cast::<Inner>() };
    if inner.poisoned {
        return;
    }
    // SAFETY: PwStream stores the function-table pointer together with Inner
    // and keeps the Arc<PwLib> alive until after listener destruction.
    let fns = unsafe { &*inner.fns };
    let mut ctl = StreamCtl {
        fns,
        stream: inner.stream,
    };
    let result = std::panic::catch_unwind(std::panic::AssertUnwindSafe(|| {
        inner.handler.process(&mut ctl);
    }));
    if result.is_err() {
        inner.poisoned = true;
        tracing::error!("PipeWire stream handler panicked");
    }
}

pub struct PwStream {
    lib: Arc<PwLib>,
    loop_ptr: *mut pw_ffi::PwThreadLoopRaw,
    context: *mut PwContextRaw,
    core: *mut PwCoreRaw,
    stream: *mut PwStreamRaw,
    hook: Option<Box<SpaHook>>,
    inner: Option<Box<Inner>>,
    _not_sync: PhantomData<std::cell::Cell<()>>,
}

impl PwStream {
    pub fn start(
        lib: Arc<PwLib>,
        remote: Remote,
        spec: StreamSpec,
        handler: Box<dyn StreamHandler>,
    ) -> Result<Self, PwError> {
        let name = CString::new(format!("rs-pw-{}", spec.name))
            .map_err(|_| PwError::Create("pw_thread_loop_new"))?;
        let StreamSpec {
            name: stream_name_text,
            props,
            direction: stream_direction,
            target,
            flags,
            params: spec_params,
        } = spec;
        let stream_name =
            CString::new(stream_name_text).map_err(|_| PwError::Create("pw_stream_new"))?;
        let mut property_strings = Vec::with_capacity(props.len() * 2);
        for (key, value) in props {
            property_strings
                .push(CString::new(key).map_err(|_| PwError::Create("pw_properties_new_dict"))?);
            property_strings
                .push(CString::new(value).map_err(|_| PwError::Create("pw_properties_new_dict"))?);
        }
        let mut items = Vec::with_capacity(property_strings.len() / 2);
        for pair in property_strings.chunks_exact(2) {
            items.push(SpaDictItem {
                key: pair[0].as_ptr(),
                value: pair[1].as_ptr(),
            });
        }
        let n_items = u32::try_from(items.len()).map_err(|_| PwError::Create("properties"))?;
        let dict = SpaDict {
            flags: 0,
            n_items,
            items: items.as_ptr(),
        };
        let params: Vec<AlignedPod> = spec_params
            .iter()
            .map(|param| AlignedPod::new(param))
            .collect();
        let param_ptrs: Vec<*const SpaPodRaw> = params.iter().map(AlignedPod::as_ptr).collect();
        let param_count = u32::try_from(param_ptrs.len()).map_err(|_| PwError::Create("params"))?;

        let fns = &lib.fns;
        // SAFETY: all pointers below are checked for NULL before use and are
        // destroyed in reverse order by cleanup_start on every failure path.
        let loop_ptr = unsafe { (fns.pw_thread_loop_new)(name.as_ptr(), ptr::null()) };
        if loop_ptr.is_null() {
            return Err(PwError::Create("pw_thread_loop_new"));
        }
        let loop_raw = unsafe { (fns.pw_thread_loop_get_loop)(loop_ptr) };
        if loop_raw.is_null() {
            unsafe {
                cleanup_start(
                    fns,
                    loop_ptr,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    false,
                    false,
                )
            };
            return Err(PwError::Create("pw_thread_loop_get_loop"));
        }
        let context = unsafe { (fns.pw_context_new)(loop_raw, ptr::null(), 0) };
        if context.is_null() {
            unsafe {
                cleanup_start(
                    fns,
                    loop_ptr,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    ptr::null_mut(),
                    false,
                    false,
                )
            };
            return Err(PwError::Create("pw_context_new"));
        }
        let code = unsafe { (fns.pw_thread_loop_start)(loop_ptr) };
        if code < 0 {
            unsafe {
                cleanup_start(
                    fns,
                    loop_ptr,
                    context,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    false,
                    false,
                )
            };
            return Err(PwError::Call {
                call: "pw_thread_loop_start",
                code,
            });
        }

        unsafe { (fns.pw_thread_loop_lock)(loop_ptr) };
        let core = match remote {
            Remote::Default => unsafe { (fns.pw_context_connect)(context, ptr::null_mut(), 0) },
            Remote::Fd(fd) => {
                let raw_fd = fd.into_raw_fd();
                unsafe { (fns.pw_context_connect_fd)(context, raw_fd, ptr::null_mut(), 0) }
            }
        };
        if core.is_null() {
            unsafe {
                cleanup_start(
                    fns,
                    loop_ptr,
                    context,
                    ptr::null_mut(),
                    ptr::null_mut(),
                    true,
                    true,
                )
            };
            return Err(PwError::Create("pw_context_connect"));
        }
        let properties: *mut PwPropertiesRaw = if items.is_empty() {
            ptr::null_mut()
        } else {
            unsafe { (fns.pw_properties_new_dict)(&dict) }
        };
        if !items.is_empty() && properties.is_null() {
            unsafe { cleanup_start(fns, loop_ptr, context, core, ptr::null_mut(), true, true) };
            return Err(PwError::Create("pw_properties_new_dict"));
        }
        // PipeWire takes ownership of the properties object on stream_new.
        // With no properties there is no object to transfer.
        let stream = unsafe { (fns.pw_stream_new)(core, stream_name.as_ptr(), properties) };
        if stream.is_null() {
            unsafe { cleanup_start(fns, loop_ptr, context, core, ptr::null_mut(), true, true) };
            return Err(PwError::Create("pw_stream_new"));
        }

        let mut hook = Box::new(SpaHook {
            link_next: ptr::null_mut(),
            link_prev: ptr::null_mut(),
            cb: pw_ffi::SpaCallbacks {
                funcs: ptr::null(),
                data: ptr::null_mut(),
            },
            removed: None,
            priv_: ptr::null_mut(),
        });
        let mut inner = Box::new(Inner {
            handler,
            poisoned: false,
            fns: fns as *const PwFns,
            stream,
        });
        let inner_ptr = (&mut *inner as *mut Inner).cast::<c_void>();
        // SAFETY: hook and inner are stable heap allocations for the stream's
        // complete lifetime; EVENTS has static storage. The C API returns
        // void, so listener registration has no failure return value.
        unsafe { (fns.pw_stream_add_listener)(stream, &mut *hook, &EVENTS, inner_ptr) };
        let direction = match stream_direction {
            Direction::Input => direction::INPUT,
            Direction::Output => direction::OUTPUT,
        };
        let target = target.unwrap_or(pw_ffi::PW_ID_ANY);
        let code = unsafe {
            (fns.pw_stream_connect)(
                stream,
                direction,
                target,
                flags,
                param_ptrs.as_ptr(),
                param_count,
            )
        };
        if code < 0 {
            unsafe { cleanup_start(fns, loop_ptr, context, core, stream, true, true) };
            return Err(PwError::Call {
                call: "pw_stream_connect",
                code,
            });
        }
        unsafe { (fns.pw_thread_loop_unlock)(loop_ptr) };
        Ok(Self {
            lib,
            loop_ptr,
            context,
            core,
            stream,
            hook: Some(hook),
            inner: Some(inner),
            _not_sync: PhantomData,
        })
    }

    pub fn node_id(&self) -> u32 {
        // SAFETY: the loop is live and the lock serializes access with the
        // PipeWire callback thread.
        unsafe { (self.lib.fns.pw_thread_loop_lock)(self.loop_ptr) };
        let node_id = unsafe { (self.lib.fns.pw_stream_get_node_id)(self.stream) };
        unsafe { (self.lib.fns.pw_thread_loop_unlock)(self.loop_ptr) };
        node_id
    }

    pub fn set_active(&self, on: bool) -> Result<(), PwError> {
        // SAFETY: lock serializes this operation with callbacks and Drop.
        unsafe { (self.lib.fns.pw_thread_loop_lock)(self.loop_ptr) };
        let code = unsafe { (self.lib.fns.pw_stream_set_active)(self.stream, on) };
        unsafe { (self.lib.fns.pw_thread_loop_unlock)(self.loop_ptr) };
        if code < 0 {
            Err(PwError::Call {
                call: "pw_stream_set_active",
                code,
            })
        } else {
            Ok(())
        }
    }

    pub fn trigger(&self) -> Result<(), PwError> {
        // SAFETY: lock serializes this operation with callbacks and Drop.
        unsafe { (self.lib.fns.pw_thread_loop_lock)(self.loop_ptr) };
        let code = unsafe { (self.lib.fns.pw_stream_trigger_process)(self.stream) };
        unsafe { (self.lib.fns.pw_thread_loop_unlock)(self.loop_ptr) };
        if code < 0 {
            Err(PwError::Call {
                call: "pw_stream_trigger_process",
                code,
            })
        } else {
            Ok(())
        }
    }
}

// SAFETY: PipeWire objects are only touched while holding their thread-loop
// lock; the listener data and hook remain pinned until the stream is dropped.
unsafe impl Send for PwStream {}

impl Drop for PwStream {
    fn drop(&mut self) {
        // SAFETY: all objects are owned by this value and the lock prevents any
        // callback from racing the disconnect/destroy sequence.
        unsafe { (self.lib.fns.pw_thread_loop_lock)(self.loop_ptr) };
        let _ = unsafe { (self.lib.fns.pw_stream_disconnect)(self.stream) };
        unsafe { (self.lib.fns.pw_stream_destroy)(self.stream) };
        let _ = unsafe { (self.lib.fns.pw_core_disconnect)(self.core) };
        unsafe { (self.lib.fns.pw_thread_loop_unlock)(self.loop_ptr) };
        unsafe { (self.lib.fns.pw_thread_loop_stop)(self.loop_ptr) };
        unsafe { (self.lib.fns.pw_context_destroy)(self.context) };
        unsafe { (self.lib.fns.pw_thread_loop_destroy)(self.loop_ptr) };
        let _ = self.hook.take();
        let _ = self.inner.take();
    }
}
