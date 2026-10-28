//! Runtime FFmpeg 8.x loader and VA-API/NVENC encoders for Linux.
//!
//! FFmpeg is deliberately loaded with `dlopen`: the host remains usable on
//! systems without FFmpeg or without a matching hardware encoder.

use crate::{dl::DynLib, ff_model};
use host_core::{
    encode_model::{EncParams, FfOpt},
    ffmpeg_abi,
};
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    os::unix::ffi::OsStrExt,
    path::Path,
    ptr,
    sync::{Arc, Mutex, OnceLock},
    time::{SystemTime, UNIX_EPOCH},
};

use crate::ff_model::LinuxBackend;

type Raw = *mut c_void;

type LogCallback = unsafe extern "C" fn(Raw, c_int, *const c_char, Raw);
type AvCodecVersion = unsafe extern "C" fn() -> u32;
type AvLicense = unsafe extern "C" fn() -> *const c_char;
type AvConfiguration = unsafe extern "C" fn() -> *const c_char;
type AvLogSetLevel = unsafe extern "C" fn(c_int);
type AvLogSetCallback = unsafe extern "C" fn(Option<LogCallback>);
type AvFrameAlloc = unsafe extern "C" fn() -> Raw;
type AvFrameFree = unsafe extern "C" fn(*mut Raw);
type AvFrameGetBuffer = unsafe extern "C" fn(Raw, c_int) -> c_int;
type AvGetPixFmt = unsafe extern "C" fn(*const c_char) -> c_int;
type AvBufferRef = unsafe extern "C" fn(Raw) -> Raw;
type AvBufferUnref = unsafe extern "C" fn(*mut Raw);
type AvHwDeviceFindTypeByName = unsafe extern "C" fn(*const c_char) -> c_int;
type AvHwDeviceCtxCreate =
    unsafe extern "C" fn(*mut Raw, c_int, *const c_char, Raw, c_int) -> c_int;
type AvHwFrameCtxAlloc = unsafe extern "C" fn(Raw) -> Raw;
type AvHwFrameCtxInit = unsafe extern "C" fn(Raw) -> c_int;
type AvHwFrameGetBuffer = unsafe extern "C" fn(Raw, Raw, c_int) -> c_int;
type AvHwFrameTransferData = unsafe extern "C" fn(Raw, Raw, c_int) -> c_int;
type AvOptSet = unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int) -> c_int;
type AvOptFind = unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int, c_int) -> Raw;
type AvCodecFindEncoder = unsafe extern "C" fn(*const c_char) -> Raw;
type AvCodecAllocContext = unsafe extern "C" fn(Raw) -> Raw;
type AvCodecFreeContext = unsafe extern "C" fn(*mut Raw);
type AvCodecOpen2 = unsafe extern "C" fn(Raw, Raw, *mut Raw) -> c_int;
type AvCodecSendFrame = unsafe extern "C" fn(Raw, Raw) -> c_int;
type AvCodecReceivePacket = unsafe extern "C" fn(Raw, Raw) -> c_int;
type AvPacketAlloc = unsafe extern "C" fn() -> Raw;
type AvPacketFree = unsafe extern "C" fn(*mut Raw);
type AvPacketUnref = unsafe extern "C" fn(Raw);
type SwscaleVersion = unsafe extern "C" fn() -> u32;
type SwsGetContext = unsafe extern "C" fn(
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    c_int,
    Raw,
    Raw,
    *const f64,
) -> Raw;
type SwsScale = unsafe extern "C" fn(
    Raw,
    *const *const u8,
    *const c_int,
    c_int,
    c_int,
    *mut *mut u8,
    *const c_int,
) -> c_int;
type SwsFreeContext = unsafe extern "C" fn(Raw);

macro_rules! resolve_fn {
    ($module:expr, $name:literal, $ty:ty) => {{
        let symbol = $module.symbol(cconcat!($name)).map_err(FfError::Load)?;
        // SAFETY: symbol is resolved from the owning FFmpeg shared object and
        // the requested type exactly matches its C declaration.
        unsafe { std::mem::transmute::<*mut c_void, $ty>(symbol) }
    }};
}

macro_rules! cconcat {
    ($s:literal) => {
        c"$s"
    };
}

struct Api {
    avutil_version: AvCodecVersion,
    avutil_license: AvLicense,
    av_log_set_level: AvLogSetLevel,
    av_log_set_callback: AvLogSetCallback,
    av_frame_alloc: AvFrameAlloc,
    av_frame_free: AvFrameFree,
    av_frame_get_buffer: AvFrameGetBuffer,
    av_get_pix_fmt: AvGetPixFmt,
    av_buffer_ref: AvBufferRef,
    av_buffer_unref: AvBufferUnref,
    av_hwdevice_find_type_by_name: AvHwDeviceFindTypeByName,
    av_hwdevice_ctx_create: AvHwDeviceCtxCreate,
    av_hwframe_ctx_alloc: AvHwFrameCtxAlloc,
    av_hwframe_ctx_init: AvHwFrameCtxInit,
    av_hwframe_get_buffer: AvHwFrameGetBuffer,
    av_hwframe_transfer_data: AvHwFrameTransferData,
    av_opt_set: AvOptSet,
    av_opt_find: AvOptFind,
    avcodec_version: AvCodecVersion,
    avcodec_license: AvLicense,
    avcodec_configuration: AvConfiguration,
    avcodec_find_encoder_by_name: AvCodecFindEncoder,
    avcodec_alloc_context3: AvCodecAllocContext,
    avcodec_free_context: AvCodecFreeContext,
    avcodec_open2: AvCodecOpen2,
    avcodec_send_frame: AvCodecSendFrame,
    avcodec_receive_packet: AvCodecReceivePacket,
    av_packet_alloc: AvPacketAlloc,
    av_packet_free: AvPacketFree,
    av_packet_unref: AvPacketUnref,
    swscale_version: SwscaleVersion,
    sws_get_context: SwsGetContext,
    sws_scale: SwsScale,
    sws_free_context: SwsFreeContext,
}

impl Api {
    fn resolve(util: &DynLib, scale: &DynLib, codec: &DynLib) -> Result<Self, FfError> {
        Ok(Self {
            avutil_version: resolve_fn!(util, "avutil_version", AvCodecVersion),
            avutil_license: resolve_fn!(util, "avutil_license", AvLicense),
            av_log_set_level: resolve_fn!(util, "av_log_set_level", AvLogSetLevel),
            av_log_set_callback: resolve_fn!(util, "av_log_set_callback", AvLogSetCallback),
            av_frame_alloc: resolve_fn!(util, "av_frame_alloc", AvFrameAlloc),
            av_frame_free: resolve_fn!(util, "av_frame_free", AvFrameFree),
            av_frame_get_buffer: resolve_fn!(util, "av_frame_get_buffer", AvFrameGetBuffer),
            av_get_pix_fmt: resolve_fn!(util, "av_get_pix_fmt", AvGetPixFmt),
            av_buffer_ref: resolve_fn!(util, "av_buffer_ref", AvBufferRef),
            av_buffer_unref: resolve_fn!(util, "av_buffer_unref", AvBufferUnref),
            av_hwdevice_find_type_by_name: resolve_fn!(
                util,
                "av_hwdevice_find_type_by_name",
                AvHwDeviceFindTypeByName
            ),
            av_hwdevice_ctx_create: resolve_fn!(
                util,
                "av_hwdevice_ctx_create",
                AvHwDeviceCtxCreate
            ),
            av_hwframe_ctx_alloc: resolve_fn!(util, "av_hwframe_ctx_alloc", AvHwFrameCtxAlloc),
            av_hwframe_ctx_init: resolve_fn!(util, "av_hwframe_ctx_init", AvHwFrameCtxInit),
            av_hwframe_get_buffer: resolve_fn!(util, "av_hwframe_get_buffer", AvHwFrameGetBuffer),
            av_hwframe_transfer_data: resolve_fn!(
                util,
                "av_hwframe_transfer_data",
                AvHwFrameTransferData
            ),
            av_opt_set: resolve_fn!(util, "av_opt_set", AvOptSet),
            av_opt_find: resolve_fn!(util, "av_opt_find", AvOptFind),
            avcodec_version: resolve_fn!(codec, "avcodec_version", AvCodecVersion),
            avcodec_license: resolve_fn!(codec, "avcodec_license", AvLicense),
            avcodec_configuration: resolve_fn!(codec, "avcodec_configuration", AvConfiguration),
            avcodec_find_encoder_by_name: resolve_fn!(
                codec,
                "avcodec_find_encoder_by_name",
                AvCodecFindEncoder
            ),
            avcodec_alloc_context3: resolve_fn!(
                codec,
                "avcodec_alloc_context3",
                AvCodecAllocContext
            ),
            avcodec_free_context: resolve_fn!(codec, "avcodec_free_context", AvCodecFreeContext),
            avcodec_open2: resolve_fn!(codec, "avcodec_open2", AvCodecOpen2),
            avcodec_send_frame: resolve_fn!(codec, "avcodec_send_frame", AvCodecSendFrame),
            avcodec_receive_packet: resolve_fn!(
                codec,
                "avcodec_receive_packet",
                AvCodecReceivePacket
            ),
            av_packet_alloc: resolve_fn!(codec, "av_packet_alloc", AvPacketAlloc),
            av_packet_free: resolve_fn!(codec, "av_packet_free", AvPacketFree),
            av_packet_unref: resolve_fn!(codec, "av_packet_unref", AvPacketUnref),
            swscale_version: resolve_fn!(scale, "swscale_version", SwscaleVersion),
            sws_get_context: resolve_fn!(scale, "sws_getContext", SwsGetContext),
            sws_scale: resolve_fn!(scale, "sws_scale", SwsScale),
            sws_free_context: resolve_fn!(scale, "sws_freeContext", SwsFreeContext),
        })
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfVersions {
    pub avcodec: u32,
    pub avutil: u32,
    pub swscale: u32,
}

#[derive(Debug, Clone, thiserror::Error)]
pub enum FfError {
    #[error("load FFmpeg: {0}")]
    Load(String),
    #[error(transparent)]
    Abi(#[from] ffmpeg_abi::AbiError),
    #[error("unsupported: {0}")]
    Unsupported(String),
    #[error("{call} failed: {name}")]
    Failed {
        call: &'static str,
        code: i32,
        name: String,
    },
}

pub struct FfLib {
    api: Api,
    versions: FfVersions,
    _libs: [DynLib; 3],
}

static SHARED: OnceLock<Result<Arc<FfLib>, FfError>> = OnceLock::new();

impl FfLib {
    /// Load the three FFmpeg shared objects from an absolute directory.
    pub fn load(dir: &Path) -> Result<Arc<FfLib>, FfError> {
        if !dir.is_absolute() {
            return Err(FfError::Load(format!(
                "FFmpeg directory is not absolute: {}",
                dir.display()
            )));
        }
        let mut libs = Vec::with_capacity(ff_model::LIB_LOAD_ORDER.len());
        for name in ff_model::LIB_LOAD_ORDER {
            libs.push(DynLib::open(&dir.join(name)).map_err(FfError::Load)?);
        }
        let util = &libs[0];
        let scale = &libs[1];
        let codec = &libs[2];
        let api = Api::resolve(util, scale, codec)?;
        // SAFETY: version and metadata functions are pure accessors returning
        // process-lifetime values owned by the loaded FFmpeg modules.
        let (avcodec, avutil, swscale, avcodec_license, avutil_license, config) = unsafe {
            (
                (api.avcodec_version)(),
                (api.avutil_version)(),
                (api.swscale_version)(),
                static_string((api.avcodec_license)())?,
                static_string((api.avutil_license)())?,
                static_string((api.avcodec_configuration)())?,
            )
        };
        ffmpeg_abi::check_versions(avcodec, avutil)?;
        if ffmpeg_abi::version_parts(swscale).0 != ff_model::SWSCALE_MAJOR {
            return Err(FfError::Abi(ffmpeg_abi::AbiError::Version {
                lib: "swscale",
                found: swscale,
            }));
        }
        ffmpeg_abi::check_license(&avcodec_license, &avutil_license, &config)?;

        // Validate the opaque AVCodecContext layout before exposing any API.
        let candidates = ["hevc_vaapi", "h264_vaapi", "hevc_nvenc", "h264_nvenc"];
        let mut codec_ptr = ptr::null_mut();
        // SAFETY: encoder names are temporary NUL-terminated strings and the
        // returned descriptor belongs to the pinned avcodec module.
        for name in candidates {
            let cname = CString::new(name).map_err(|e| FfError::Load(e.to_string()))?;
            codec_ptr = unsafe { (api.avcodec_find_encoder_by_name)(cname.as_ptr()) };
            if !codec_ptr.is_null() {
                break;
            }
        }
        // SAFETY: FFmpeg accepts a null codec for an unconfigured context;
        // this also permits ABI validation on a build with no hardware codec.
        let mut ctx = unsafe { (api.avcodec_alloc_context3)(codec_ptr) };
        if ctx.is_null() {
            return Err(FfError::Failed {
                call: "avcodec_alloc_context3",
                code: ff_model::AVERROR_ENOMEM,
                name: ff_model::err_name(ff_model::AVERROR_ENOMEM),
            });
        }
        let offsets = (|| {
            let mut found = Vec::with_capacity(ffmpeg_abi::OPTION_OFFSETS.len());
            for &(name, _) in ffmpeg_abi::OPTION_OFFSETS {
                let cname = CString::new(name).map_err(|e| FfError::Load(e.to_string()))?;
                // SAFETY: ctx is live and av_opt_find returns an AVOption whose
                // offset field is validated by the shared ABI contract.
                let value = unsafe {
                    let option = (api.av_opt_find)(ctx, cname.as_ptr(), ptr::null(), 0, 0);
                    if option.is_null() {
                        None
                    } else {
                        Some(read_field::<i32>(option, ffmpeg_abi::off::option::OFFSET))
                    }
                };
                found.push((name, value));
            }
            ffmpeg_abi::check_offsets(&found).map_err(FfError::Abi)
        })();
        // SAFETY: ctx was allocated by avcodec_alloc_context3 and is freed once.
        unsafe { (api.avcodec_free_context)(&mut ctx) };
        offsets?;

        let lib = Arc::new(Self {
            api,
            versions: FfVersions {
                avcodec,
                avutil,
                swscale,
            },
            _libs: libs
                .try_into()
                .map_err(|_| FfError::Load("FFmpeg library count changed".into()))?,
        });
        install_log(&lib.api);
        Ok(lib)
    }

    /// Load once using the executable-relative directory policy.
    pub fn shared() -> Result<Arc<FfLib>, FfError> {
        SHARED
            .get_or_init(|| {
                let exe = std::env::current_exe().map_err(|e| FfError::Load(e.to_string()))?;
                let exe_dir = exe
                    .parent()
                    .ok_or_else(|| FfError::Load("current executable has no directory".into()))?;
                let dirs =
                    ff_model::lib_dirs(exe_dir, std::env::var("RS_FFMPEG_DIR").ok().as_deref());
                let Some(dir) = ff_model::pick_lib_dir(&dirs, Path::exists) else {
                    return Err(FfError::Load(format!(
                        "no FFmpeg 8 library set in {}",
                        dirs.iter()
                            .map(|p| p.display().to_string())
                            .collect::<Vec<_>>()
                            .join(", ")
                    )));
                };
                Self::load(&dir)
            })
            .as_ref()
            .map(Arc::clone)
            .map_err(Clone::clone)
    }

    pub fn versions(&self) -> &FfVersions {
        &self.versions
    }

    pub fn has_encoder(&self, name: &str) -> bool {
        let Ok(name) = CString::new(name) else {
            return false;
        };
        // SAFETY: name lives through the call; descriptor belongs to the loaded module.
        unsafe { !(self.api.avcodec_find_encoder_by_name)(name.as_ptr()).is_null() }
    }

    pub fn pix_fmt(&self, name: &str) -> Option<i32> {
        let Ok(name) = CString::new(name) else {
            return None;
        };
        // SAFETY: name lives through the call; av_get_pix_fmt does not retain it.
        let value = unsafe { (self.api.av_get_pix_fmt)(name.as_ptr()) };
        (value >= 0).then_some(value)
    }
}

fn static_string(value: *const c_char) -> Result<String, FfError> {
    if value.is_null() {
        return Err(FfError::Load(
            "FFmpeg returned a null metadata string".into(),
        ));
    }
    // SAFETY: FFmpeg metadata accessors return static NUL-terminated strings.
    Ok(unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned())
}

unsafe fn read_field<T: Copy>(base: Raw, offset: usize) -> T {
    // SAFETY: caller has established the FFmpeg ABI allocation and field type;
    // unaligned access is intentional for opaque C structures.
    unsafe { base.cast::<u8>().add(offset).cast::<T>().read_unaligned() }
}

unsafe fn write_field<T>(base: Raw, offset: usize, value: T) {
    // SAFETY: caller owns the allocation and the offset/type comes from the
    // checked FFmpeg ABI contract.
    unsafe {
        base.cast::<u8>()
            .add(offset)
            .cast::<T>()
            .write_unaligned(value)
    };
}

fn install_log(api: &Api) {
    let _ = LOG_LIMITER.get_or_init(|| Mutex::new(ffmpeg_abi::LogLimiter::new(20)));
    // SAFETY: api belongs to a library retained by the returned FfLib Arc.
    // FFmpeg invokes this callback only while its module remains loaded.
    unsafe {
        (api.av_log_set_level)(ffmpeg_abi::AV_LOG_ERROR);
        (api.av_log_set_callback)(Some(log_callback));
    }
}

static LOG_LIMITER: OnceLock<Mutex<ffmpeg_abi::LogLimiter>> = OnceLock::new();

unsafe extern "C" fn log_callback(_avcl: Raw, level: c_int, fmt: *const c_char, _vl: Raw) {
    let result = std::panic::catch_unwind(|| {
        if fmt.is_null() || ffmpeg_abi::log_target(level).is_none() {
            return;
        }
        // SAFETY: FFmpeg passes a live NUL-terminated format string. We only
        // log the string itself and intentionally do not inspect va_list data.
        let line = unsafe { CStr::from_ptr(fmt) };
        let line = ffmpeg_abi::clean_log_line(line.to_bytes());
        let Some(limiter) = LOG_LIMITER.get() else {
            return;
        };
        let Ok(mut limiter) = limiter.try_lock() else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        if matches!(limiter.admit(now), ffmpeg_abi::LogAdmit::Drop) {
            return;
        }
        drop(limiter);
        tracing::error!(target: "ffmpeg", "{line}");
    });
    if let Err(payload) = result {
        std::mem::forget(payload);
    }
}

#[derive(Debug, Clone)]
pub struct FfOpenReq<'a> {
    pub backend: LinuxBackend,
    pub params: EncParams,
    pub src_width: u32,
    pub src_height: u32,
    pub render_node: Option<&'a Path>,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub keyframe: bool,
}

struct Resources {
    lib: Arc<FfLib>,
    ctx: Raw,
    sw_frame: Raw,
    frames: Raw,
    device: Raw,
    sws: Raw,
    packet: Raw,
}

impl Resources {
    fn new(lib: &Arc<FfLib>) -> Self {
        Self {
            lib: Arc::clone(lib),
            ctx: ptr::null_mut(),
            sw_frame: ptr::null_mut(),
            frames: ptr::null_mut(),
            device: ptr::null_mut(),
            sws: ptr::null_mut(),
            packet: ptr::null_mut(),
        }
    }
}

impl Drop for Resources {
    fn drop(&mut self) {
        let api = &self.lib.api;
        // SAFETY: every pointer is either null or owned by this Resources value;
        // the order releases codec references before their frame/device pools.
        unsafe {
            if !self.ctx.is_null() {
                (api.avcodec_free_context)(&mut self.ctx);
            }
            if !self.sw_frame.is_null() {
                (api.av_frame_free)(&mut self.sw_frame);
            }
            if !self.frames.is_null() {
                (api.av_buffer_unref)(&mut self.frames);
            }
            if !self.device.is_null() {
                (api.av_buffer_unref)(&mut self.device);
            }
            if !self.sws.is_null() {
                (api.sws_free_context)(self.sws);
                self.sws = ptr::null_mut();
            }
            if !self.packet.is_null() {
                (api.av_packet_free)(&mut self.packet);
            }
        }
    }
}

pub struct FfEncoder {
    resources: Resources,
    lib: Arc<FfLib>,
    backend: LinuxBackend,
    codec_name: String,
    src_width: u32,
    src_height: u32,
}

impl FfEncoder {
    pub fn open(lib: &Arc<FfLib>, req: &FfOpenReq<'_>) -> Result<Self, FfError> {
        let codec_name = ff_model::ffmpeg_codec_name(req.backend, req.params.codec)
            .ok_or_else(|| FfError::Unsupported("unsupported Linux FFmpeg codec".into()))?;
        let cname = CString::new(codec_name).map_err(|e| FfError::Unsupported(e.to_string()))?;
        let width = checked_dim(req.params.width, "encoding width")?;
        let height = checked_dim(req.params.height, "encoding height")?;
        let src_width = checked_dim(req.src_width, "source width")?;
        let src_height = checked_dim(req.src_height, "source height")?;
        let mut resources = Resources::new(lib);
        let api = &lib.api;
        // SAFETY: cname is a live C string and the descriptor is owned by the
        // loaded avcodec module.
        let codec = unsafe { (api.avcodec_find_encoder_by_name)(cname.as_ptr()) };
        if codec.is_null() {
            return Err(FfError::Unsupported(format!(
                "encoder {codec_name} not available"
            )));
        }

        match req.backend {
            LinuxBackend::Vaapi => {
                let node = req
                    .render_node
                    .ok_or_else(|| FfError::Unsupported("VA-API requires a render node".into()))?;
                let node = CString::new(node.as_os_str().as_bytes())
                    .map_err(|_| FfError::Unsupported("render node contains NUL".into()))?;
                let device_type =
                    CString::new(ff_model::HWDEVICE_VAAPI).expect("static name has no NUL");
                // SAFETY: device type and render-node strings live through calls;
                // output pointer is written into the owned resources slot.
                let device_kind =
                    unsafe { (api.av_hwdevice_find_type_by_name)(device_type.as_ptr()) };
                if device_kind < 0 {
                    return Err(FfError::Unsupported(
                        "FFmpeg has no vaapi device type".into(),
                    ));
                }
                // SAFETY: FFmpeg initializes an owned AVBufferRef on success.
                let code = unsafe {
                    (api.av_hwdevice_ctx_create)(
                        &mut resources.device,
                        device_kind,
                        node.as_ptr(),
                        ptr::null_mut(),
                        0,
                    )
                };
                if code < 0 {
                    return Err(classify_open("av_hwdevice_ctx_create", code));
                }
                let vaapi_fmt = lib.pix_fmt(ff_model::HWDEVICE_VAAPI).ok_or_else(|| {
                    FfError::Unsupported("FFmpeg has no vaapi pixel format".into())
                })?;
                let nv12_fmt = lib.pix_fmt(ff_model::VAAPI_SW_FORMAT).ok_or_else(|| {
                    FfError::Unsupported("FFmpeg has no nv12 pixel format".into())
                })?;
                // SAFETY: device is a live AVBufferRef owned by resources.
                resources.frames = unsafe { (api.av_hwframe_ctx_alloc)(resources.device) };
                if resources.frames.is_null() {
                    return Err(failed("av_hwframe_ctx_alloc", ff_model::AVERROR_ENOMEM));
                }
                // SAFETY: AVBufferRef data points at the validated AVHWFramesContext.
                let frames_ctx = unsafe {
                    read_field::<Raw>(resources.frames, ffmpeg_abi::off::buffer_ref::DATA)
                };
                if frames_ctx.is_null() {
                    return Err(failed("av_hwframe_ctx_alloc", ff_model::AVERROR_EINVAL));
                }
                // SAFETY: fields are public FFmpeg AVHWFramesContext fields validated
                // by the ABI offsets shared with the Windows implementation.
                unsafe {
                    write_field(frames_ctx, ffmpeg_abi::off::hwframes::FORMAT, vaapi_fmt);
                    write_field(frames_ctx, ffmpeg_abi::off::hwframes::SW_FORMAT, nv12_fmt);
                    write_field(frames_ctx, ffmpeg_abi::off::hwframes::WIDTH, width);
                    write_field(frames_ctx, ffmpeg_abi::off::hwframes::HEIGHT, height);
                    write_field(
                        frames_ctx,
                        ffmpeg_abi::off::hwframes::INITIAL_POOL_SIZE,
                        4i32,
                    );
                }
                // SAFETY: resources.frames is the live, initialized AVHWFramesContext ref.
                let code = unsafe { (api.av_hwframe_ctx_init)(resources.frames) };
                if code < 0 {
                    return Err(classify_open("av_hwframe_ctx_init", code));
                }
                open_scaler_and_frame(
                    &mut resources,
                    src_width,
                    src_height,
                    req.params.width,
                    req.params.height,
                    lib.pix_fmt(ff_model::SRC_PIX_FMT).ok_or_else(|| {
                        FfError::Unsupported("FFmpeg has no bgr0 pixel format".into())
                    })?,
                    nv12_fmt,
                )?;
                open_codec_context(&mut resources, lib, codec, req, true)?;
            }
            LinuxBackend::Nvenc => {
                let src_fmt = lib.pix_fmt(ff_model::SRC_PIX_FMT).ok_or_else(|| {
                    FfError::Unsupported("FFmpeg has no bgr0 pixel format".into())
                })?;
                open_scaler_and_frame(
                    &mut resources,
                    src_width,
                    src_height,
                    req.params.width,
                    req.params.height,
                    src_fmt,
                    src_fmt,
                )?;
                open_codec_context(&mut resources, lib, codec, req, false)?;
            }
            LinuxBackend::OpenH264 => {
                return Err(FfError::Unsupported(
                    "OpenH264 is not an FFmpeg backend".into(),
                ));
            }
        }
        // SAFETY: packet allocation belongs to the loaded avcodec module.
        resources.packet = unsafe { (api.av_packet_alloc)() };
        if resources.packet.is_null() {
            return Err(failed("av_packet_alloc", ff_model::AVERROR_ENOMEM));
        }
        Ok(Self {
            resources,
            lib: Arc::clone(lib),
            backend: req.backend,
            codec_name: codec_name.into(),
            src_width: req.src_width,
            src_height: req.src_height,
        })
    }

    pub fn encode(
        &mut self,
        src: &crate::frame::LinuxFrame,
        pts: i64,
        idr: bool,
    ) -> Result<Vec<FfPacket>, FfError> {
        if src.width != self.src_width || src.height != self.src_height {
            return Err(failed("encode", ff_model::AVERROR_EINVAL));
        }
        let expected = (src.stride as usize)
            .checked_mul(src.height as usize)
            .ok_or_else(|| failed("encode", ff_model::AVERROR_EINVAL))?;
        if src.pixels.len() < expected || src.stride < src.width.saturating_mul(4) {
            return Err(failed("encode", ff_model::AVERROR_EINVAL));
        }
        let src_ptrs = [src.pixels.as_ptr(); 8];
        let mut src_strides = [0i32; 8];
        src_strides[0] =
            i32::try_from(src.stride).map_err(|_| failed("encode", ff_model::AVERROR_EINVAL))?;
        let frame = self.resources.sw_frame;
        let mut dst_ptrs = [ptr::null_mut(); 8];
        let mut dst_strides = [0i32; 8];
        // SAFETY: sw_frame was allocated and buffered during open; data and
        // linesize arrays are public AVFrame fields validated by the ABI.
        unsafe {
            for index in 0..8 {
                dst_ptrs[index] =
                    read_field::<*mut u8>(frame, ffmpeg_abi::off::frame::DATA + index * 8);
                dst_strides[index] =
                    read_field::<i32>(frame, ffmpeg_abi::off::frame::LINESIZE + index * 4);
            }
        }
        // SAFETY: all source/destination pointers point at live frame buffers;
        // sws_scale only reads the source pixels and writes the destination frame.
        let scaled = unsafe {
            (self.lib.api.sws_scale)(
                self.resources.sws,
                src_ptrs.as_ptr(),
                src_strides.as_ptr(),
                0,
                i32::try_from(src.height)
                    .map_err(|_| failed("sws_scale", ff_model::AVERROR_EINVAL))?,
                dst_ptrs.as_mut_ptr(),
                dst_strides.as_ptr(),
            )
        };
        if scaled <= 0 {
            return Err(failed("sws_scale", scaled));
        }
        // SAFETY: frame fields are writable and use the checked offsets.
        unsafe {
            write_field(frame, ffmpeg_abi::off::frame::PTS, pts);
            write_field(
                frame,
                ffmpeg_abi::off::frame::PICT_TYPE,
                if idr {
                    ffmpeg_abi::AV_PICTURE_TYPE_I
                } else {
                    ffmpeg_abi::AV_PICTURE_TYPE_NONE
                },
            );
        }
        let submit_frame = if self.backend == LinuxBackend::Vaapi {
            let (hw, free_frame) = {
                let api = &self.lib.api;
                // SAFETY: av_frame_alloc returns an owned frame or null.
                let hw = unsafe { (api.av_frame_alloc)() };
                let free_frame = api.av_frame_free;
                (hw, free_frame)
            };
            if hw.is_null() {
                return Err(failed("av_frame_alloc", ff_model::AVERROR_ENOMEM));
            }
            let result = (|| {
                // SAFETY: hw and resources.frames are live, exclusively owned references.
                let code =
                    unsafe { (self.lib.api.av_hwframe_get_buffer)(self.resources.frames, hw, 0) };
                if code < 0 {
                    return Err(failed("av_hwframe_get_buffer", code));
                }
                // SAFETY: both frames are live and their formats were configured at open.
                let code = unsafe { (self.lib.api.av_hwframe_transfer_data)(hw, frame, 0) };
                if code < 0 {
                    return Err(failed("av_hwframe_transfer_data", code));
                }
                // SAFETY: transferred hardware frame is live and writable.
                unsafe {
                    write_field(hw, ffmpeg_abi::off::frame::PTS, pts);
                    write_field(
                        hw,
                        ffmpeg_abi::off::frame::PICT_TYPE,
                        if idr {
                            ffmpeg_abi::AV_PICTURE_TYPE_I
                        } else {
                            ffmpeg_abi::AV_PICTURE_TYPE_NONE
                        },
                    );
                }
                Ok(hw)
            })();
            if result.is_err() {
                // SAFETY: hw was allocated above and has not escaped.
                unsafe { (free_frame)(&mut (hw as Raw)) };
            }
            let hw = result?;
            let packets = self.submit_and_receive(hw, pts);
            // SAFETY: avcodec_send_frame retains references it needs; hw was
            // allocated above and has not escaped, so this releases our reference.
            unsafe { (free_frame)(&mut (hw as Raw)) };
            return packets;
        } else {
            frame
        };
        self.submit_and_receive(submit_frame, pts)
    }

    fn submit_and_receive(&mut self, frame: Raw, pts: i64) -> Result<Vec<FfPacket>, FfError> {
        let send_frame = self.lib.api.avcodec_send_frame;
        let ctx = self.resources.ctx;
        // SAFETY: ctx/frame are owned by this encoder and used on one thread.
        let mut code = unsafe { (send_frame)(ctx, frame) };
        let mut packets = Vec::new();
        if code == ff_model::AVERROR_EAGAIN {
            packets.extend(self.receive_packets()?);
            // SAFETY: receive drained the owned codec context.
            code = unsafe { (send_frame)(ctx, frame) };
        }
        if code < 0 {
            return Err(failed("avcodec_send_frame", code));
        }
        packets.extend(self.receive_packets()?);
        tracing::trace!(target: "ffmpeg", pts, packets = packets.len(), "encoded frame");
        Ok(packets)
    }

    fn receive_packets(&mut self) -> Result<Vec<FfPacket>, FfError> {
        let api = &self.lib.api;
        let mut packets = Vec::new();
        loop {
            // SAFETY: packet is allocated once and exclusively used here.
            let code =
                unsafe { (api.avcodec_receive_packet)(self.resources.ctx, self.resources.packet) };
            match code {
                0 => {
                    // SAFETY: the packet was returned by receive_packet and is
                    // valid until the following unref.
                    let packet = unsafe { copy_packet(self.resources.packet) }?;
                    // SAFETY: packet was returned by receive_packet.
                    unsafe { (api.av_packet_unref)(self.resources.packet) };
                    packets.push(packet);
                }
                c if c == ff_model::AVERROR_EAGAIN || c == ff_model::AVERROR_EOF => {
                    return Ok(packets)
                }
                c => return Err(failed("avcodec_receive_packet", c)),
            }
        }
    }

    pub fn set_rate(&mut self, bitrate_bps: u64, fps: u32) -> Result<(), FfError> {
        let options = ff_model::ffmpeg_rate_options(self.backend, bitrate_bps, fps);
        apply_options(&self.lib.api, self.resources.ctx, &options)
    }

    pub fn codec_name(&self) -> &str {
        &self.codec_name
    }
}

impl Drop for FfEncoder {
    fn drop(&mut self) {
        // Resources performs the dependency-safe release order after this body.
        // A bounded flush lets delayed packets be consumed before the context is freed.
        let api = &self.lib.api;
        // SAFETY: context and packet are live while Resources remains owned.
        unsafe {
            let _ = (api.avcodec_send_frame)(self.resources.ctx, ptr::null_mut());
            for _ in 0..64 {
                let code = (api.avcodec_receive_packet)(self.resources.ctx, self.resources.packet);
                if code == 0 {
                    (api.av_packet_unref)(self.resources.packet);
                } else {
                    break;
                }
            }
        }
    }
}

fn checked_dim(value: u32, label: &'static str) -> Result<i32, FfError> {
    i32::try_from(value)
        .ok()
        .filter(|v| *v > 0)
        .ok_or_else(|| FfError::Unsupported(format!("{label} is invalid")))
}

fn failed(call: &'static str, code: i32) -> FfError {
    FfError::Failed {
        call,
        code,
        name: ff_model::err_name(code),
    }
}

fn classify_open(call: &'static str, code: i32) -> FfError {
    if ff_model::open_error_is_unsupported(code) {
        FfError::Unsupported(format!("{call}: {}", ff_model::err_name(code)))
    } else {
        failed(call, code)
    }
}

fn open_scaler_and_frame(
    resources: &mut Resources,
    src_width: i32,
    src_height: i32,
    dst_width: u32,
    dst_height: u32,
    src_fmt: i32,
    dst_fmt: i32,
) -> Result<(), FfError> {
    let dst_width = checked_dim(dst_width, "encoding width")?;
    let dst_height = checked_dim(dst_height, "encoding height")?;
    let api = &resources.lib.api;
    // SAFETY: dimensions and formats are validated integer values; null filter
    // pointers select FFmpeg's default scaler implementation.
    resources.sws = unsafe {
        (api.sws_get_context)(
            src_width,
            src_height,
            src_fmt,
            dst_width,
            dst_height,
            dst_fmt,
            2,
            ptr::null_mut(),
            ptr::null_mut(),
            ptr::null(),
        )
    };
    if resources.sws.is_null() {
        return Err(failed("sws_getContext", ff_model::AVERROR_ENOMEM));
    }
    // SAFETY: av_frame_alloc returns an owned frame or null.
    resources.sw_frame = unsafe { (api.av_frame_alloc)() };
    if resources.sw_frame.is_null() {
        return Err(failed("av_frame_alloc", ff_model::AVERROR_ENOMEM));
    }
    // SAFETY: format/size fields are public and validated by the ABI offsets.
    unsafe {
        write_field(resources.sw_frame, ffmpeg_abi::off::frame::FORMAT, dst_fmt);
        write_field(resources.sw_frame, ffmpeg_abi::off::frame::WIDTH, dst_width);
        write_field(
            resources.sw_frame,
            ffmpeg_abi::off::frame::HEIGHT,
            dst_height,
        );
    }
    // SAFETY: resources.sw_frame is a live frame with checked format/size fields.
    let code = unsafe { (api.av_frame_get_buffer)(resources.sw_frame, 32) };
    if code < 0 {
        return Err(failed("av_frame_get_buffer", code));
    }
    Ok(())
}

fn open_codec_context(
    resources: &mut Resources,
    lib: &Arc<FfLib>,
    codec: Raw,
    req: &FfOpenReq<'_>,
    hardware_frames: bool,
) -> Result<(), FfError> {
    let api = &lib.api;
    let options = ff_model::ffmpeg_options(req.backend, &req.params);
    let mut context = allocate_context(api, codec)?;
    if let Err(error) = apply_options(api, context, &options) {
        // SAFETY: context is still owned locally because option application
        // failed before it could be moved into Resources.
        unsafe { (api.avcodec_free_context)(&mut context) };
        return Err(error);
    }
    if hardware_frames {
        // SAFETY: resources.frames is a live AVBufferRef owned by this open attempt.
        let frames_ref = unsafe { (api.av_buffer_ref)(resources.frames) };
        if frames_ref.is_null() {
            // SAFETY: context is owned by this function until moved into resources.
            unsafe { (api.avcodec_free_context)(&mut context) };
            return Err(failed("av_buffer_ref", ff_model::AVERROR_ENOMEM));
        }
        // SAFETY: context is live and the frames reference is transferred to it.
        unsafe { write_field(context, ffmpeg_abi::off::ctx::HW_FRAMES_CTX, frames_ref) };
    }
    // SAFETY: context and codec are live allocations from the loaded FFmpeg API.
    let code = unsafe { (api.avcodec_open2)(context, codec, ptr::null_mut()) };
    if code >= 0 {
        resources.ctx = context;
        return Ok(());
    }
    // SAFETY: failed avcodec_open2 leaves an owned context to be freed.
    unsafe { (api.avcodec_free_context)(&mut context) };
    let Some(retry_options) = ff_model::ffmpeg_options_retry(req.backend, &req.params) else {
        return Err(classify_open("avcodec_open2", code));
    };
    if !ff_model::open_error_is_unsupported(code) {
        return Err(failed("avcodec_open2", code));
    }
    let mut retry = allocate_context(api, codec)?;
    let retry_result = (|| {
        apply_options(api, retry, &retry_options)?;
        if hardware_frames {
            // SAFETY: resources.frames is a live AVBufferRef retained for retry.
            let frames_ref = unsafe { (api.av_buffer_ref)(resources.frames) };
            if frames_ref.is_null() {
                return Err(failed("av_buffer_ref", ff_model::AVERROR_ENOMEM));
            }
            // SAFETY: retry context is live and takes ownership of this reference.
            unsafe { write_field(retry, ffmpeg_abi::off::ctx::HW_FRAMES_CTX, frames_ref) };
        }
        // SAFETY: retry and codec are live allocations from the loaded API.
        let code = unsafe { (api.avcodec_open2)(retry, codec, ptr::null_mut()) };
        if code < 0 {
            return Err(classify_open("avcodec_open2", code));
        }
        Ok(())
    })();
    if retry_result.is_err() {
        // SAFETY: retry context remains owned locally on every error path.
        unsafe { (api.avcodec_free_context)(&mut retry) };
    } else {
        resources.ctx = retry;
    }
    retry_result
}

fn allocate_context(api: &Api, codec: Raw) -> Result<Raw, FfError> {
    // SAFETY: codec is a descriptor returned by avcodec_find_encoder_by_name.
    let context = unsafe { (api.avcodec_alloc_context3)(codec) };
    if context.is_null() {
        Err(failed("avcodec_alloc_context3", ff_model::AVERROR_ENOMEM))
    } else {
        Ok(context)
    }
}

unsafe fn copy_packet(packet: Raw) -> Result<FfPacket, FfError> {
    // SAFETY: caller obtained packet from avcodec_receive_packet with code 0.
    let size = unsafe { read_field::<i32>(packet, ffmpeg_abi::off::packet::SIZE) };
    let data = unsafe { read_field::<*const u8>(packet, ffmpeg_abi::off::packet::DATA) };
    if size < 0 || (size > 0 && data.is_null()) {
        return Err(failed("av_packet", ff_model::AVERROR_EINVAL));
    }
    let bytes = if size == 0 {
        Vec::new()
    } else {
        // SAFETY: FFmpeg owns size bytes until av_packet_unref, which the caller
        // invokes immediately after this copy.
        unsafe { std::slice::from_raw_parts(data, size as usize) }.to_vec()
    };
    Ok(FfPacket {
        data: bytes,
        // SAFETY: packet remains valid until the caller unrefs it.
        pts: unsafe { read_field(packet, ffmpeg_abi::off::packet::PTS) },
        // SAFETY: packet remains valid until the caller unrefs it.
        keyframe: unsafe { read_field::<i32>(packet, ffmpeg_abi::off::packet::FLAGS) }
            & ffmpeg_abi::AV_PKT_FLAG_KEY
            != 0,
    })
}

fn apply_options(api: &Api, ctx: Raw, options: &[FfOpt]) -> Result<(), FfError> {
    for option in options {
        let name = CString::new(option.name).map_err(|e| FfError::Unsupported(e.to_string()))?;
        let value =
            CString::new(option.value.as_str()).map_err(|e| FfError::Unsupported(e.to_string()))?;
        // SAFETY: ctx is a live AVCodecContext and strings live for this call.
        let code = unsafe {
            (api.av_opt_set)(
                ctx,
                name.as_ptr(),
                value.as_ptr(),
                ffmpeg_abi::AV_OPT_SEARCH_CHILDREN,
            )
        };
        if code < 0 {
            if option.required {
                return Err(FfError::Unsupported(format!("option {}", option.name)));
            }
            tracing::debug!(target: "ffmpeg", option = option.name, error = %ff_model::err_name(code), "optional option rejected");
        }
    }
    Ok(())
}
