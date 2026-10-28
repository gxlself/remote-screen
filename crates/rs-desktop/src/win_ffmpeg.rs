#![cfg(windows)]
//! ：运行时加载 LGPL FFmpeg 8.x，d3d11va 硬解 HEVC / H.264 到 D3D11 纹理。
//!
//! This module deliberately keeps the FFmpeg C ABI private.  The public
//! surface exposes only Windows interfaces and the platform error types used
//! by the desktop session layer.

use crate::{
    platform::{PlatformError, VideoCodec},
    win_ffmpeg_abi::{
        self as abi, clean_log_line, decoder_name, err_name, ffmpeg_dir, hw_config_is_d3d11,
        log_target, off, LogAdmit, LogLimiter, AVERROR_EAGAIN, AVERROR_EOF, AVERROR_INVALIDDATA,
        AV_HWDEVICE_TYPE_D3D11VA, AV_PIX_FMT_D3D11, AV_PKT_FLAG_KEY, DECODER_OPTIONS,
        DEC_OPTION_OFFSETS,
    },
};
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    marker::PhantomData,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{Interface, PCSTR, PCWSTR},
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        Graphics::{
            Direct3D::{D3D_DRIVER_TYPE_HARDWARE, D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1},
            Direct3D11::{
                D3D11CreateDevice, ID3D11Device, ID3D11Multithread, ID3D11Texture2D,
                ID3D11VideoDevice, D3D11_CREATE_DEVICE_BGRA_SUPPORT,
                D3D11_CREATE_DEVICE_VIDEO_SUPPORT, D3D11_DECODER_PROFILE_H264_VLD_NOFGT,
                D3D11_DECODER_PROFILE_HEVC_VLD_MAIN, D3D11_SDK_VERSION,
            },
            Dxgi::Common::DXGI_FORMAT_NV12,
        },
        System::LibraryLoader::{
            GetProcAddress, LoadLibraryExW, LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR,
        },
    },
};

type Raw = *mut c_void;
type LogCallback = unsafe extern "C" fn(Raw, c_int, *const c_char, Raw);
type LogFormat =
    unsafe extern "C" fn(Raw, c_int, *const c_char, Raw, *mut c_char, c_int, *mut c_int) -> c_int;

#[derive(Debug, Clone)]
pub struct FfVersions {
    pub avcodec: u32,
    pub avutil: u32,
    pub license: String,
    pub configuration: String,
    pub dir: PathBuf,
}

/// A loaded module remains pinned for process lifetime.  FFmpeg keeps global
/// callback state, and contexts may outlive the `FfDecLib` Arc.
struct Module(HMODULE);

impl Module {
    fn open(path: &Path) -> Result<Self, PlatformError> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if !path.is_absolute() || wide.contains(&0) {
            return Err(PlatformError::Unsupported(format!(
                "load {}: invalid absolute path",
                path.display()
            )));
        }
        wide.push(0);
        // SAFETY: `wide` is a terminated absolute UTF-16 path and the flags
        // restrict dependency lookup to the DLL directory/default directories.
        unsafe {
            LoadLibraryExW(
                PCWSTR(wide.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .map(Self)
        .map_err(|error| PlatformError::Unsupported(format!("load {}: {error}", path.display())))
    }

    fn symbol(&self, name: &CStr) -> Result<unsafe extern "system" fn() -> isize, PlatformError> {
        // SAFETY: the module is live and `name` is a terminated C symbol name.
        unsafe { GetProcAddress(self.0, PCSTR(name.as_ptr().cast())) }.ok_or_else(|| {
            PlatformError::Unsupported(format!("missing FFmpeg symbol {}", name.to_string_lossy()))
        })
    }
}

impl Drop for Module {
    fn drop(&mut self) {
        // A successful `FfDecLib` load forgets both Module values.  This path
        // only handles partial loading before the library is published.
        // SAFETY: this Module owns one LoadLibraryExW reference.
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}

struct Api {
    avcodec_version: unsafe extern "C" fn() -> u32,
    avcodec_license: unsafe extern "C" fn() -> *const c_char,
    avcodec_configuration: unsafe extern "C" fn() -> *const c_char,
    avcodec_find_decoder_by_name: unsafe extern "C" fn(*const c_char) -> Raw,
    avcodec_get_hw_config: unsafe extern "C" fn(Raw, c_int) -> Raw,
    avcodec_alloc_context3: unsafe extern "C" fn(Raw) -> Raw,
    avcodec_open2: unsafe extern "C" fn(Raw, Raw, *mut Raw) -> c_int,
    avcodec_send_packet: unsafe extern "C" fn(Raw, Raw) -> c_int,
    avcodec_receive_frame: unsafe extern "C" fn(Raw, Raw) -> c_int,
    avcodec_flush_buffers: unsafe extern "C" fn(Raw),
    avcodec_free_context: unsafe extern "C" fn(*mut Raw),
    av_packet_alloc: unsafe extern "C" fn() -> Raw,
    av_packet_free: unsafe extern "C" fn(*mut Raw),
    av_packet_unref: unsafe extern "C" fn(Raw),
    av_new_packet: unsafe extern "C" fn(Raw, c_int) -> c_int,
    avutil_version: unsafe extern "C" fn() -> u32,
    avutil_license: unsafe extern "C" fn() -> *const c_char,
    av_frame_alloc: unsafe extern "C" fn() -> Raw,
    av_frame_free: unsafe extern "C" fn(*mut Raw),
    av_frame_unref: unsafe extern "C" fn(Raw),
    av_frame_move_ref: unsafe extern "C" fn(Raw, Raw),
    av_buffer_ref: unsafe extern "C" fn(Raw) -> Raw,
    av_buffer_unref: unsafe extern "C" fn(*mut Raw),
    av_hwdevice_ctx_alloc: unsafe extern "C" fn(c_int) -> Raw,
    av_hwdevice_ctx_init: unsafe extern "C" fn(Raw) -> c_int,
    av_opt_set: unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int) -> c_int,
    av_opt_find: unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int, c_int) -> Raw,
    av_log_set_callback: unsafe extern "C" fn(Option<LogCallback>),
    av_log_set_level: unsafe extern "C" fn(c_int),
    av_log_format_line2: LogFormat,
}

macro_rules! resolve {
    ($module:expr, $name:literal, $ty:ty) => {{
        // SAFETY: each requested symbol is resolved from its owning FFmpeg DLL;
        // the declared function type is the public FFmpeg 8 ABI signature.
        unsafe {
            std::mem::transmute::<unsafe extern "system" fn() -> isize, $ty>($module.symbol(
                CStr::from_bytes_with_nul_unchecked(concat!($name, "\\0").as_bytes()),
            )?)
        }
    }};
}

impl Api {
    fn resolve(util: &Module, codec: &Module) -> Result<Self, PlatformError> {
        Ok(Self {
            avcodec_version: resolve!(codec, "avcodec_version", unsafe extern "C" fn() -> u32),
            avcodec_license: resolve!(
                codec,
                "avcodec_license",
                unsafe extern "C" fn() -> *const c_char
            ),
            avcodec_configuration: resolve!(
                codec,
                "avcodec_configuration",
                unsafe extern "C" fn() -> *const c_char
            ),
            avcodec_find_decoder_by_name: resolve!(
                codec,
                "avcodec_find_decoder_by_name",
                unsafe extern "C" fn(*const c_char) -> Raw
            ),
            avcodec_get_hw_config: resolve!(
                codec,
                "avcodec_get_hw_config",
                unsafe extern "C" fn(Raw, c_int) -> Raw
            ),
            avcodec_alloc_context3: resolve!(
                codec,
                "avcodec_alloc_context3",
                unsafe extern "C" fn(Raw) -> Raw
            ),
            avcodec_open2: resolve!(
                codec,
                "avcodec_open2",
                unsafe extern "C" fn(Raw, Raw, *mut Raw) -> c_int
            ),
            avcodec_send_packet: resolve!(
                codec,
                "avcodec_send_packet",
                unsafe extern "C" fn(Raw, Raw) -> c_int
            ),
            avcodec_receive_frame: resolve!(
                codec,
                "avcodec_receive_frame",
                unsafe extern "C" fn(Raw, Raw) -> c_int
            ),
            avcodec_flush_buffers: resolve!(
                codec,
                "avcodec_flush_buffers",
                unsafe extern "C" fn(Raw)
            ),
            avcodec_free_context: resolve!(
                codec,
                "avcodec_free_context",
                unsafe extern "C" fn(*mut Raw)
            ),
            av_packet_alloc: resolve!(codec, "av_packet_alloc", unsafe extern "C" fn() -> Raw),
            av_packet_free: resolve!(codec, "av_packet_free", unsafe extern "C" fn(*mut Raw)),
            av_packet_unref: resolve!(codec, "av_packet_unref", unsafe extern "C" fn(Raw)),
            av_new_packet: resolve!(
                codec,
                "av_new_packet",
                unsafe extern "C" fn(Raw, c_int) -> c_int
            ),
            avutil_version: resolve!(util, "avutil_version", unsafe extern "C" fn() -> u32),
            avutil_license: resolve!(
                util,
                "avutil_license",
                unsafe extern "C" fn() -> *const c_char
            ),
            av_frame_alloc: resolve!(util, "av_frame_alloc", unsafe extern "C" fn() -> Raw),
            av_frame_free: resolve!(util, "av_frame_free", unsafe extern "C" fn(*mut Raw)),
            av_frame_unref: resolve!(util, "av_frame_unref", unsafe extern "C" fn(Raw)),
            av_frame_move_ref: resolve!(util, "av_frame_move_ref", unsafe extern "C" fn(Raw, Raw)),
            av_buffer_ref: resolve!(util, "av_buffer_ref", unsafe extern "C" fn(Raw) -> Raw),
            av_buffer_unref: resolve!(util, "av_buffer_unref", unsafe extern "C" fn(*mut Raw)),
            av_hwdevice_ctx_alloc: resolve!(
                util,
                "av_hwdevice_ctx_alloc",
                unsafe extern "C" fn(c_int) -> Raw
            ),
            av_hwdevice_ctx_init: resolve!(
                util,
                "av_hwdevice_ctx_init",
                unsafe extern "C" fn(Raw) -> c_int
            ),
            av_opt_set: resolve!(
                util,
                "av_opt_set",
                unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int) -> c_int
            ),
            av_opt_find: resolve!(
                util,
                "av_opt_find",
                unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int, c_int) -> Raw
            ),
            av_log_set_callback: resolve!(
                util,
                "av_log_set_callback",
                unsafe extern "C" fn(Option<LogCallback>)
            ),
            av_log_set_level: resolve!(util, "av_log_set_level", unsafe extern "C" fn(c_int)),
            av_log_format_line2: resolve!(util, "av_log_format_line2", LogFormat),
        })
    }
}

pub struct FfDecLib {
    api: Api,
    versions: FfVersions,
    // Handles are intentionally retained until process exit.
    _modules: [usize; 2],
}

static SHARED: OnceLock<Result<Arc<FfDecLib>, String>> = OnceLock::new();

impl FfDecLib {
    pub fn shared() -> Result<Arc<FfDecLib>, PlatformError> {
        SHARED
            .get_or_init(|| {
                let result = (|| {
                    let exe = std::env::current_exe().map_err(|error| error.to_string())?;
                    let parent = exe
                        .parent()
                        .ok_or_else(|| "current executable has no directory".to_owned())?;
                    let dir =
                        ffmpeg_dir(parent, std::env::var(abi::FFMPEG_DIR_ENV).ok().as_deref());
                    Self::load(&dir).map_err(|error| error.to_string())
                })();
                result.map_err(|error| {
                    tracing::warn!(%error, "FFmpeg unavailable");
                    error
                })
            })
            .as_ref()
            .map(Arc::clone)
            .map_err(|error| PlatformError::Unsupported(error.clone()))
    }

    fn load(dir: &Path) -> Result<Arc<Self>, PlatformError> {
        if usize::BITS != 64 {
            return Err(PlatformError::Unsupported(
                "FFmpeg ABI requires 64-bit Windows".into(),
            ));
        }
        let dir = std::path::absolute(dir)
            .map_err(|error| PlatformError::Unsupported(format!("FFmpeg directory: {error}")))?;
        let util = Module::open(&dir.join(abi::AVUTIL_DLL))?;
        let codec = Module::open(&dir.join(abi::AVCODEC_DLL))?;
        let api = Api::resolve(&util, &codec)?;
        // SAFETY: version/license/configuration functions return static metadata
        // strings owned by the loaded DLL.
        let (avcodec, avutil, license, util_license, configuration) = unsafe {
            let avcodec = (api.avcodec_version)();
            let avutil = (api.avutil_version)();
            let license = static_string((api.avcodec_license)())?;
            let util_license = static_string((api.avutil_license)())?;
            let configuration = static_string((api.avcodec_configuration)())?;
            (avcodec, avutil, license, util_license, configuration)
        };
        abi::check_versions(avcodec, avutil)
            .map_err(|error| PlatformError::Unsupported(error.to_string()))?;
        abi::check_license(&license, &util_license, &configuration)
            .map_err(|error| PlatformError::Unsupported(error.to_string()))?;

        // Resolve the AVOption layout before any context field is written.
        // SAFETY: null codec is permitted for allocating a context used only
        // for AVOption layout inspection.
        let mut probe = unsafe { (api.avcodec_alloc_context3)(ptr::null_mut()) };
        if probe.is_null() {
            return Err(PlatformError::Unsupported(
                "avcodec_alloc_context3: out of memory".into(),
            ));
        }
        let offsets_result = (|| {
            let mut found = Vec::with_capacity(DEC_OPTION_OFFSETS.len());
            for &(name, _) in DEC_OPTION_OFFSETS {
                let cname = CString::new(name)
                    .map_err(|error| PlatformError::Unsupported(error.to_string()))?;
                // SAFETY: `probe` is a live AVCodecContext and `cname` lives for
                // the duration of this lookup.
                let value = unsafe {
                    let option = (api.av_opt_find)(probe, cname.as_ptr(), ptr::null(), 0, 0);
                    if option.is_null() {
                        None
                    } else {
                        Some(read_field::<i32>(option, off::option::OFFSET))
                    }
                };
                found.push((name, value));
            }
            abi::check_offsets(&found)
                .map_err(|error| PlatformError::Unsupported(error.to_string()))
        })();
        // SAFETY: `probe` is the sole owner of this temporary context.
        unsafe { (api.avcodec_free_context)(&mut probe) };
        offsets_result?;

        let lib = Arc::new(Self {
            api,
            versions: FfVersions {
                avcodec,
                avutil,
                license,
                configuration,
                dir,
            },
            _modules: [util.0 .0 as usize, codec.0 .0 as usize],
        });
        std::mem::forget(util);
        std::mem::forget(codec);
        lib.install_log();
        Ok(lib)
    }

    pub fn versions(&self) -> &FfVersions {
        &self.versions
    }

    pub fn supports(&self, codec: VideoCodec) -> bool {
        let name = decoder_name(codec);
        // SAFETY: `name` is a static terminated string and the decoder pointer
        // belongs to the pinned codec DLL.
        let decoder = unsafe { (self.api.avcodec_find_decoder_by_name)(name.as_ptr()) };
        if decoder.is_null() {
            return false;
        }
        for index in 0..64 {
            // SAFETY: FFmpeg returns a static config pointer or null at the end.
            let config = unsafe { (self.api.avcodec_get_hw_config)(decoder, index) };
            if config.is_null() {
                break;
            }
            // SAFETY: AVCodecHWConfig is three public i32 fields.
            let (pix_fmt, methods, device_type) = unsafe {
                (
                    read_field::<i32>(config, off::hw_config::PIX_FMT),
                    read_field::<i32>(config, off::hw_config::METHODS),
                    read_field::<i32>(config, off::hw_config::DEVICE_TYPE),
                )
            };
            if hw_config_is_d3d11(pix_fmt, methods, device_type) {
                return true;
            }
        }
        false
    }

    fn install_log(&self) {
        static INSTALLED: OnceLock<()> = OnceLock::new();
        INSTALLED.get_or_init(|| {
            LOG.get_or_init(|| LogState {
                format: self.api.av_log_format_line2,
                level: 24,
                limiter: Mutex::new(LogLimiter::new(20)),
            });
            // SAFETY: the callback and function pointers remain valid while the
            // DLL is pinned for process lifetime.
            unsafe {
                (self.api.av_log_set_level)(24);
                (self.api.av_log_set_callback)(Some(log_callback));
            }
        });
    }
}

unsafe fn static_string(value: *const c_char) -> Result<String, PlatformError> {
    if value.is_null() {
        return Err(PlatformError::Unsupported(
            "null FFmpeg metadata string".into(),
        ));
    }
    // SAFETY: FFmpeg metadata accessors return static terminated strings.
    Ok(unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned())
}

unsafe fn read_field<T: Copy>(base: Raw, offset: usize) -> T {
    // SAFETY: callers establish that `base` points to the checked FFmpeg ABI
    // object and that `offset`/T are the corresponding public field.
    unsafe { base.cast::<u8>().add(offset).cast::<T>().read_unaligned() }
}

unsafe fn write_field<T>(base: Raw, offset: usize, value: T) {
    // SAFETY: callers own the checked FFmpeg object and write only documented
    // public fields through the ABI offsets.
    unsafe {
        base.cast::<u8>()
            .add(offset)
            .cast::<T>()
            .write_unaligned(value);
    }
}

struct LogState {
    format: LogFormat,
    level: i32,
    limiter: Mutex<LogLimiter>,
}
static LOG: OnceLock<LogState> = OnceLock::new();

unsafe extern "C" fn log_callback(avcl: Raw, level: c_int, fmt: *const c_char, vl: Raw) {
    let result = std::panic::catch_unwind(|| {
        let Some(state) = LOG.get() else { return };
        let Some(target) = log_target(level) else {
            return;
        };
        if level > state.level || fmt.is_null() {
            return;
        }
        let mut buffer = [0u8; 1024];
        let mut prefix = 1;
        // SAFETY: FFmpeg supplies the va_list; the output buffer is fixed and
        // bounded, and `format_line2` is the resolved avutil function.
        unsafe {
            (state.format)(
                avcl,
                level,
                fmt,
                vl,
                buffer.as_mut_ptr().cast(),
                buffer.len() as c_int,
                &mut prefix,
            );
        }
        let length = buffer
            .iter()
            .position(|byte| *byte == 0)
            .unwrap_or(buffer.len());
        let line = clean_log_line(&buffer[..length]);
        let Ok(mut limiter) = state.limiter.try_lock() else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|duration| duration.as_micros().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        let admit = limiter.admit(now);
        drop(limiter);
        if let LogAdmit::PassWithSuppressed(count) = admit {
            emit_log(target, &format!("suppressed {count} lines"));
        }
        if admit != LogAdmit::Drop {
            emit_log(target, &line);
        }
    });
    if let Err(payload) = result {
        std::mem::forget(payload);
    }
}

fn emit_log(target: abi::LogTarget, line: &str) {
    match target {
        abi::LogTarget::Error => tracing::error!(target: "ffmpeg", "{line}"),
        abi::LogTarget::Warn => tracing::warn!(target: "ffmpeg", "{line}"),
        abi::LogTarget::Debug => tracing::debug!(target: "ffmpeg", "{line}"),
        abi::LogTarget::Trace => tracing::trace!(target: "ffmpeg", "{line}"),
    }
}

pub fn create_device() -> Result<ID3D11Device, PlatformError> {
    let levels = [D3D_FEATURE_LEVEL_11_1, D3D_FEATURE_LEVEL_11_0];
    let mut device = None;
    let mut context = None;
    // SAFETY: output pointers are valid for this synchronous call; the device
    // is created with the video and BGRA capabilities required by d3d11va.
    unsafe {
        D3D11CreateDevice(
            None,
            D3D_DRIVER_TYPE_HARDWARE,
            HMODULE::default(),
            D3D11_CREATE_DEVICE_VIDEO_SUPPORT | D3D11_CREATE_DEVICE_BGRA_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
    }
    .map_err(|error| PlatformError::Failed(format!("D3D11CreateDevice: {error}")))?;
    let device = device
        .ok_or_else(|| PlatformError::Failed("D3D11CreateDevice returned no device".into()))?;
    if context.is_none() {
        return Err(PlatformError::Failed(
            "D3D11CreateDevice returned no context".into(),
        ));
    }
    let multithread: ID3D11Multithread = device
        .cast()
        .map_err(|error| PlatformError::Failed(format!("D3D11 multithread interface: {error}")))?;
    // SAFETY: `multithread` was queried from the live device.
    unsafe {
        let _ = multithread.SetMultithreadProtected(true);
    }
    Ok(device)
}

pub fn device_supports(device: &ID3D11Device, codec: VideoCodec) -> bool {
    let video: ID3D11VideoDevice = match device.cast() {
        Ok(value) => value,
        Err(_) => return false,
    };
    let profile = match codec {
        VideoCodec::Hevc => &D3D11_DECODER_PROFILE_HEVC_VLD_MAIN,
        VideoCodec::H264 => &D3D11_DECODER_PROFILE_H264_VLD_NOFGT,
    };
    // SAFETY: `video` is a live interface and `profile`/format are static API
    // values. The method only writes its BOOL result.
    unsafe {
        video
            .CheckVideoDecoderFormat(profile, DXGI_FORMAT_NV12)
            .map(|supported| supported.as_bool())
            .unwrap_or(false)
    }
}

static CAN_HEVC: OnceLock<bool> = OnceLock::new();
static CAN_H264: OnceLock<bool> = OnceLock::new();

pub fn can_decode(codec: VideoCodec) -> bool {
    let cache = match codec {
        VideoCodec::Hevc => &CAN_HEVC,
        VideoCodec::H264 => &CAN_H264,
    };
    *cache.get_or_init(|| {
        let result = (|| {
            let lib = FfDecLib::shared().ok()?;
            if !lib.supports(codec) {
                return Some(false);
            }
            let device = create_device().ok()?;
            Some(device_supports(&device, codec))
        })()
        .unwrap_or(false);
        tracing::info!(codec = ?codec, supported = result, "FFmpeg d3d11va decode capability");
        result
    })
}

struct FrameOwner {
    ptr: Raw,
    lib: Arc<FfDecLib>,
}

impl Drop for FrameOwner {
    fn drop(&mut self) {
        // SAFETY: this owner holds the sole nullable AVFrame allocation.
        unsafe { (self.lib.api.av_frame_free)(&mut self.ptr) };
    }
}

pub struct DecodedFrame {
    _frame: FrameOwner,
    pub frame_id: u32,
    pub texture: ID3D11Texture2D,
    pub array_index: u32,
    pub width: u32,
    pub height: u32,
}

#[derive(Debug, thiserror::Error)]
pub enum DecodeError {
    #[error("invalid bitstream")]
    Invalid,
    #[error("decoder output is not D3D11 (software fallback)")]
    NotHardware,
    #[error("device removed: {0:#010x}")]
    DeviceRemoved(i32),
    #[error("ffmpeg: {0}")]
    Failed(String),
}

pub struct FfDecoder {
    lib: Arc<FfDecLib>,
    ctx: Raw,
    pkt: Raw,
    frame: Raw,
    hw_device_ref: Raw,
    device: ID3D11Device,
    codec: VideoCodec,
    _thread: PhantomData<Rc<()>>,
}

impl FfDecoder {
    #[allow(unused_assignments)]
    pub fn open(
        lib: &Arc<FfDecLib>,
        codec: VideoCodec,
        device: &ID3D11Device,
    ) -> Result<Self, PlatformError> {
        let mut hw_device_ref = ptr::null_mut();
        let mut ctx = ptr::null_mut();
        let mut pkt = ptr::null_mut();
        let mut frame = ptr::null_mut();
        // SAFETY: every raw allocation is immediately owned by the local cleanup
        // path and only checked public FFmpeg fields are accessed.
        let result = unsafe {
            let decoder = (lib.api.avcodec_find_decoder_by_name)(decoder_name(codec).as_ptr());
            if decoder.is_null() {
                return Err(PlatformError::Unsupported(format!(
                    "decoder {} is unavailable",
                    decoder_name(codec).to_string_lossy()
                )));
            }
            hw_device_ref = (lib.api.av_hwdevice_ctx_alloc)(AV_HWDEVICE_TYPE_D3D11VA);
            if hw_device_ref.is_null() {
                return Err(PlatformError::Unsupported(
                    "av_hwdevice_ctx_alloc failed".into(),
                ));
            }
            let hw_device = read_field::<Raw>(hw_device_ref, off::buffer_ref::DATA);
            if hw_device.is_null()
                || read_field::<i32>(hw_device, off::hwdev::TYPE) != AV_HWDEVICE_TYPE_D3D11VA
            {
                return Err(PlatformError::Unsupported(
                    "FFmpeg D3D11VA ABI mismatch".into(),
                ));
            }
            let hw_context = read_field::<Raw>(hw_device, off::hwdev::HWCTX);
            if hw_context.is_null() {
                return Err(PlatformError::Unsupported(
                    "FFmpeg D3D11VA context is null".into(),
                ));
            }
            // `into_raw` transfers an extra AddRef to FFmpeg; the hwcontext
            // uninitializer releases this reference when the buffer is freed.
            let raw_device = device.clone().into_raw();
            write_field(hw_context, off::d3d11_dev::DEVICE, raw_device);
            let code = (lib.api.av_hwdevice_ctx_init)(hw_device_ref);
            if code < 0 {
                return Err(PlatformError::Unsupported(format!(
                    "D3D11VA device init: {}",
                    err_name(code)
                )));
            }
            ctx = (lib.api.avcodec_alloc_context3)(decoder);
            if ctx.is_null() {
                return Err(PlatformError::Failed(
                    "avcodec_alloc_context3 failed".into(),
                ));
            }
            let mut found = Vec::with_capacity(DEC_OPTION_OFFSETS.len());
            for &(name, _) in DEC_OPTION_OFFSETS {
                let name = CString::new(name)
                    .map_err(|error| PlatformError::Unsupported(error.to_string()))?;
                let option = (lib.api.av_opt_find)(ctx, name.as_ptr(), ptr::null(), 0, 0);
                found.push((
                    name,
                    if option.is_null() {
                        None
                    } else {
                        Some(read_field::<i32>(option, off::option::OFFSET))
                    },
                ));
            }
            let found_refs: Vec<(&str, Option<i32>)> = found
                .iter()
                .map(|(name, value)| (name.to_str().unwrap_or(""), *value))
                .collect();
            abi::check_offsets(&found_refs)
                .map_err(|error| PlatformError::Unsupported(error.to_string()))?;
            for &(name, value) in DECODER_OPTIONS {
                let name = CString::new(name)
                    .map_err(|error| PlatformError::Unsupported(error.to_string()))?;
                let value = CString::new(value)
                    .map_err(|error| PlatformError::Unsupported(error.to_string()))?;
                let code = (lib.api.av_opt_set)(ctx, name.as_ptr(), value.as_ptr(), 1);
                if code < 0 {
                    return Err(PlatformError::Unsupported(format!(
                        "option {}={}: {}",
                        name.to_string_lossy(),
                        value.to_string_lossy(),
                        err_name(code)
                    )));
                }
            }
            let context_ref = (lib.api.av_buffer_ref)(hw_device_ref);
            if context_ref.is_null() {
                return Err(PlatformError::Failed("av_buffer_ref failed".into()));
            }
            write_field(ctx, off::ctx::HW_DEVICE_CTX, context_ref);
            let code = (lib.api.avcodec_open2)(ctx, decoder, ptr::null_mut());
            if code < 0 {
                return Err(PlatformError::Unsupported(format!(
                    "avcodec_open2: {}",
                    err_name(code)
                )));
            }
            pkt = (lib.api.av_packet_alloc)();
            frame = (lib.api.av_frame_alloc)();
            if pkt.is_null() || frame.is_null() {
                return Err(PlatformError::Failed(
                    "FFmpeg frame/packet allocation failed".into(),
                ));
            }
            Ok(())
        };
        if let Err(error) = result {
            // SAFETY: partial allocations are nullable and owned by this path.
            unsafe {
                if !frame.is_null() {
                    (lib.api.av_frame_free)(&mut frame);
                }
                if !pkt.is_null() {
                    (lib.api.av_packet_free)(&mut pkt);
                }
                if !ctx.is_null() {
                    (lib.api.avcodec_free_context)(&mut ctx);
                }
                if !hw_device_ref.is_null() {
                    (lib.api.av_buffer_unref)(&mut hw_device_ref);
                }
            }
            return Err(error);
        }
        Ok(Self {
            lib: Arc::clone(lib),
            ctx,
            pkt,
            frame,
            hw_device_ref,
            device: device.clone(),
            codec,
            _thread: PhantomData,
        })
    }

    pub fn decode(
        &mut self,
        frame_id: u32,
        annexb: &[u8],
        keyframe: bool,
    ) -> Result<Vec<DecodedFrame>, DecodeError> {
        let size = i32::try_from(annexb.len()).map_err(|_| DecodeError::Invalid)?;
        // SAFETY: packet is owned by this decoder; `av_new_packet` allocates its
        // writable payload and the copy stays within the reported length.
        let code = unsafe {
            let code = (self.lib.api.av_new_packet)(self.pkt, size);
            if code >= 0 {
                let data = read_field::<*mut u8>(self.pkt, off::packet::DATA);
                if size > 0 && data.is_null() {
                    (self.lib.api.av_packet_unref)(self.pkt);
                    return Err(DecodeError::Failed("FFmpeg packet data is null".into()));
                }
                if size > 0 {
                    ptr::copy_nonoverlapping(annexb.as_ptr(), data, annexb.len());
                }
                write_field(self.pkt, off::packet::PTS, frame_id as i64);
                write_field(
                    self.pkt,
                    off::packet::FLAGS,
                    if keyframe { AV_PKT_FLAG_KEY } else { 0 },
                );
            }
            code
        };
        if code < 0 {
            return Err(if code == AVERROR_INVALIDDATA {
                DecodeError::Invalid
            } else {
                DecodeError::Failed(err_name(code))
            });
        }
        // SAFETY: send and unref operate on this decoder's exclusive packet.
        let code = unsafe { (self.lib.api.avcodec_send_packet)(self.ctx, self.pkt) };
        // SAFETY: the packet remains owned by this decoder and send_packet has
        // completed before its payload is released.
        unsafe { (self.lib.api.av_packet_unref)(self.pkt) };
        if code < 0 {
            return Err(self.send_error(code));
        }
        self.receive_frames()
    }

    pub fn drain(&mut self) -> Vec<DecodedFrame> {
        // SAFETY: null packet signals decoder drain; context is exclusively used
        // on this decoder thread.
        let sent = unsafe { (self.lib.api.avcodec_send_packet)(self.ctx, ptr::null_mut()) };
        if sent < 0 && sent != AVERROR_EAGAIN && sent != AVERROR_EOF {
            let _ = self.send_error(sent);
        }
        let frames = self.receive_frames().unwrap_or_default();
        // SAFETY: context remains live and can be reused after flushing.
        unsafe { (self.lib.api.avcodec_flush_buffers)(self.ctx) };
        frames
    }

    pub fn codec(&self) -> VideoCodec {
        self.codec
    }

    fn send_error(&self, code: i32) -> DecodeError {
        // SAFETY: the device interface is live for the decoder lifetime.
        match unsafe { self.device.GetDeviceRemovedReason() } {
            Ok(()) => DecodeError::Failed(err_name(code)),
            Err(error) => DecodeError::DeviceRemoved(error.code().0),
        }
    }

    fn receive_frames(&mut self) -> Result<Vec<DecodedFrame>, DecodeError> {
        let mut output = Vec::new();
        loop {
            // SAFETY: frame is owned by this decoder and reused after each move
            // or unref.
            let code = unsafe { (self.lib.api.avcodec_receive_frame)(self.ctx, self.frame) };
            match code {
                0 => output.push(self.take_frame()?),
                AVERROR_EAGAIN | AVERROR_EOF => break,
                _ => return Err(self.send_error(code)),
            }
        }
        Ok(output)
    }

    fn take_frame(&mut self) -> Result<DecodedFrame, DecodeError> {
        // SAFETY: receive_frame populated the checked AVFrame fields.
        let (format, width, height, pts, texture_raw, index_raw) = unsafe {
            (
                read_field::<i32>(self.frame, off::frame::FORMAT),
                read_field::<i32>(self.frame, off::frame::WIDTH),
                read_field::<i32>(self.frame, off::frame::HEIGHT),
                read_field::<i64>(self.frame, off::frame::PTS),
                read_field::<Raw>(self.frame, off::frame::DATA),
                read_field::<Raw>(self.frame, off::frame::DATA + std::mem::size_of::<Raw>()),
            )
        };
        if format != AV_PIX_FMT_D3D11 {
            // SAFETY: discard a software frame before returning the explicit
            // no-fallback error; this leaves the reusable frame clean.
            unsafe { (self.lib.api.av_frame_unref)(self.frame) };
            return Err(DecodeError::NotHardware);
        }
        if texture_raw.is_null() || width <= 0 || height <= 0 {
            // SAFETY: the received frame is owned by this decoder and is being
            // discarded before reuse.
            unsafe { (self.lib.api.av_frame_unref)(self.frame) };
            return Err(DecodeError::Failed("invalid D3D11 frame".into()));
        }
        let frame_id = match u32::try_from(pts) {
            Ok(value) => value,
            Err(_) => {
                // SAFETY: the received frame is owned by this decoder and is
                // being discarded because its timestamp is not a frame id.
                unsafe { (self.lib.api.av_frame_unref)(self.frame) };
                return Err(DecodeError::Failed("invalid frame pts".into()));
            }
        };
        let texture = unsafe {
            // SAFETY: data[0] is the live ID3D11Texture2D supplied by d3d11va;
            // cloning the borrowed interface performs one AddRef.
            match ID3D11Texture2D::from_raw_borrowed(&texture_raw) {
                Some(texture) => texture.clone(),
                None => {
                    // SAFETY: the received frame is owned by this decoder and
                    // is being discarded because its texture pointer is bad.
                    (self.lib.api.av_frame_unref)(self.frame);
                    return Err(DecodeError::Failed("invalid D3D11 texture".into()));
                }
            }
        };
        let array_index = match usize::try_from(index_raw as isize)
            .ok()
            .and_then(|value| u32::try_from(value).ok())
        {
            Some(value) => value,
            None => {
                // SAFETY: the received frame is owned by this decoder and is
                // being discarded because its array index is invalid.
                unsafe { (self.lib.api.av_frame_unref)(self.frame) };
                return Err(DecodeError::Failed("invalid D3D11 array index".into()));
            }
        };
        // SAFETY: the loaded avutil function allocates a fresh AVFrame or
        // returns null; no aliases to this allocation exist yet.
        let owned = unsafe { (self.lib.api.av_frame_alloc)() };
        if owned.is_null() {
            // SAFETY: the received frame is owned by this decoder and is being
            // discarded because the output allocation failed.
            unsafe { (self.lib.api.av_frame_unref)(self.frame) };
            return Err(DecodeError::Failed("av_frame_alloc failed".into()));
        }
        // SAFETY: both frames are valid allocations; move_ref transfers all
        // references to the output owner and resets the reusable source.
        unsafe { (self.lib.api.av_frame_move_ref)(owned, self.frame) };
        Ok(DecodedFrame {
            _frame: FrameOwner {
                ptr: owned,
                lib: Arc::clone(&self.lib),
            },
            frame_id,
            texture,
            array_index,
            width: width as u32,
            height: height as u32,
        })
    }
}

impl Drop for FfDecoder {
    fn drop(&mut self) {
        // SAFETY: all pointers are owned by this decoder. Context is freed before
        // the explicit hardware reference, matching FFmpeg's ownership graph.
        unsafe {
            if !self.frame.is_null() {
                (self.lib.api.av_frame_free)(&mut self.frame);
            }
            if !self.pkt.is_null() {
                (self.lib.api.av_packet_free)(&mut self.pkt);
            }
            if !self.ctx.is_null() {
                (self.lib.api.avcodec_free_context)(&mut self.ctx);
            }
            if !self.hw_device_ref.is_null() {
                (self.lib.api.av_buffer_unref)(&mut self.hw_device_ref);
            }
        }
    }
}
