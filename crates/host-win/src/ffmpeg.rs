#![cfg(windows)]
//! Runtime-only LGPL FFmpeg 8.x loading and D3D11/QSV hardware frames.
//!
//! The encoder uses `FfLib::shared()` and opens on `WinFrame.device`. The FFmpeg adapter writes only
//! `input().width × height` into the texture's `array_index` slice, then the encoder
//! calls `encode` on the same immediate context. Match packets to capture time
//! by pts. `Unsupported` selects the next backend; `Failed` is a runtime error
//! (the encoder checks GetDeviceRemovedReason). NVENC/QSV bitrate options reconfigure on
//! the next frame, with an IDR; AMF requires dropping and reopening the encoder.
//! Open, use and drop each encoder on the same encoding thread.
//!
//! FFmpeg 8.0 hwcontext_d3d11va.c fills missing device_context/video_device/
//! video_context/lock fields and Releases device on uninit: transfer one extra
//! AddRef. Its frames_init allocates an ArraySize=initial_pool_size texture with
//! the supplied BindFlags. QSV frame_type=0 selects PROCESSOR_TARGET child
//! textures with RENDER_TARGET; mapping D3D11 ← QSV exposes texture in data[0]
//! and array index in data[1]. NVENC/AMF accept D3D11 + hw_frames_ctx, QSV takes
//! QSV frames. As in Sunshine, a single texture pool and one AVFrame are reused;
//! send_frame takes references. No frame contents are logged.
//!
//! Native frame layout: native QSV frames hold their surface in data[3], not
//! data[0]. Validate that slot before mapping; validate data[0] on D3D11 frames.
//! See <https://github.com/FFmpeg/FFmpeg/blob/n8.1.1/libavutil/hwcontext_qsv.c>
//! (`qsv_get_buffer` and `qsv_map_from`). No additional structure fields are written.

use host_core::ffmpeg_abi::*;
use host_core::platform::PlatformError;
use std::{
    ffi::{c_char, c_int, c_void, CStr, CString},
    marker::PhantomData,
    os::windows::ffi::OsStrExt,
    path::{Path, PathBuf},
    ptr,
    rc::Rc,
    sync::{Arc, Mutex, OnceLock},
    time::{Instant, SystemTime, UNIX_EPOCH},
};
use windows::{
    core::{Interface, PCSTR, PCWSTR},
    Win32::{
        Foundation::{FreeLibrary, HMODULE},
        Graphics::Direct3D11::{ID3D11Device, ID3D11Texture2D},
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

pub struct FfVersions {
    pub avcodec: u32,
    pub avutil: u32,
    pub license: String,
    pub configuration: String,
    pub dir: PathBuf,
}

/// Handles are stored as integers and retained for the process lifetime.
/// SAFETY: only immutable function pointers / metadata are shared. FFmpeg's
/// library entry points are thread safe; mutable contexts stay in FfEncoder.
pub struct FfLib {
    api: Api,
    versions: FfVersions,
    _modules: [usize; 2],
}

struct Module(HMODULE);
impl Module {
    fn open(path: &Path) -> Result<Self, PlatformError> {
        let mut wide: Vec<u16> = path.as_os_str().encode_wide().collect();
        if !path.is_absolute() || wide.contains(&0) {
            return Err(platform(format!(
                "load {}: invalid absolute path",
                path.display()
            )));
        }
        wide.push(0);
        // SAFETY: terminated absolute UTF-16 path; flags restrict dependency lookup.
        unsafe {
            LoadLibraryExW(
                PCWSTR(wide.as_ptr()),
                None,
                LOAD_LIBRARY_SEARCH_DLL_LOAD_DIR | LOAD_LIBRARY_SEARCH_DEFAULT_DIRS,
            )
        }
        .map(Self)
        .map_err(|e| platform(format!("load {}: {e}", path.display())))
    }
    fn symbol(&self, name: &CStr) -> Result<unsafe extern "system" fn() -> isize, PlatformError> {
        // SAFETY: this module is loaded and name is a terminated C symbol name.
        unsafe { GetProcAddress(self.0, PCSTR(name.as_ptr().cast())) }
            .ok_or_else(|| platform(format!("missing FFmpeg symbol {}", name.to_string_lossy())))
    }
}
impl Drop for Module {
    fn drop(&mut self) {
        // SAFETY: owns one LoadLibraryExW reference; successful loads forget this guard.
        unsafe {
            let _ = FreeLibrary(self.0);
        }
    }
}
fn platform(message: impl Into<String>) -> PlatformError {
    PlatformError::Platform(message.into())
}

struct Api {
    avutil_version: unsafe extern "C" fn() -> u32,
    avutil_license: unsafe extern "C" fn() -> *const c_char,
    av_log_set_callback: unsafe extern "C" fn(Option<LogCallback>),
    av_log_set_level: unsafe extern "C" fn(c_int),
    av_log_format_line2: unsafe extern "C" fn(
        Raw,
        c_int,
        *const c_char,
        Raw,
        *mut c_char,
        c_int,
        *mut c_int,
    ) -> c_int,
    av_frame_alloc: unsafe extern "C" fn() -> Raw,
    av_frame_free: unsafe extern "C" fn(*mut Raw),
    av_frame_new_side_data: unsafe extern "C" fn(Raw, c_int, usize) -> Raw,
    av_frame_remove_side_data: unsafe extern "C" fn(Raw, c_int),
    av_buffer_ref: unsafe extern "C" fn(Raw) -> Raw,
    av_buffer_unref: unsafe extern "C" fn(*mut Raw),
    av_hwdevice_ctx_alloc: unsafe extern "C" fn(c_int) -> Raw,
    av_hwdevice_ctx_init: unsafe extern "C" fn(Raw) -> c_int,
    av_hwdevice_ctx_create_derived: unsafe extern "C" fn(*mut Raw, c_int, Raw, c_int) -> c_int,
    av_hwframe_ctx_alloc: unsafe extern "C" fn(Raw) -> Raw,
    av_hwframe_ctx_init: unsafe extern "C" fn(Raw) -> c_int,
    av_hwframe_get_buffer: unsafe extern "C" fn(Raw, Raw, c_int) -> c_int,
    av_hwframe_map: unsafe extern "C" fn(Raw, Raw, c_int) -> c_int,
    av_opt_set: unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int) -> c_int,
    av_opt_find: unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int, c_int) -> Raw,
    avcodec_version: unsafe extern "C" fn() -> u32,
    avcodec_license: unsafe extern "C" fn() -> *const c_char,
    avcodec_configuration: unsafe extern "C" fn() -> *const c_char,
    avcodec_find_encoder_by_name: unsafe extern "C" fn(*const c_char) -> Raw,
    avcodec_alloc_context3: unsafe extern "C" fn(Raw) -> Raw,
    avcodec_free_context: unsafe extern "C" fn(*mut Raw),
    avcodec_open2: unsafe extern "C" fn(Raw, Raw, *mut Raw) -> c_int,
    avcodec_send_frame: unsafe extern "C" fn(Raw, Raw) -> c_int,
    avcodec_receive_packet: unsafe extern "C" fn(Raw, Raw) -> c_int,
    av_packet_alloc: unsafe extern "C" fn() -> Raw,
    av_packet_free: unsafe extern "C" fn(*mut Raw),
    av_packet_unref: unsafe extern "C" fn(Raw),
}
impl Api {
    fn resolve(util: &Module, codec: &Module) -> Result<Self, PlatformError> {
        Ok(Self {
            // SAFETY: unsigned avutil_version(void);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avutil_version: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn() -> u32,
                >(util.symbol(c"avutil_version")?)
            },
            // SAFETY: const char *avutil_license(void);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avutil_license: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn() -> *const c_char,
                >(util.symbol(c"avutil_license")?)
            },
            // SAFETY: void av_log_set_callback(void (*callback)(void*, int, const char*, va_list));
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_log_set_callback: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Option<LogCallback>),
                >(util.symbol(c"av_log_set_callback")?)
            },
            // SAFETY: void av_log_set_level(int level);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_log_set_level: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(c_int),
                >(util.symbol(c"av_log_set_level")?)
            },
            // SAFETY: int av_log_format_line2(void *ptr, int level, const char *fmt, va_list vl, char *line, int line_size, int *print_prefix);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_log_format_line2: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(
                        Raw,
                        c_int,
                        *const c_char,
                        Raw,
                        *mut c_char,
                        c_int,
                        *mut c_int,
                    ) -> c_int,
                >(util.symbol(c"av_log_format_line2")?)
            },
            // SAFETY: AVFrame *av_frame_alloc(void);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_frame_alloc: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn() -> Raw,
                >(util.symbol(c"av_frame_alloc")?)
            },
            // SAFETY: void av_frame_free(AVFrame **frame);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_frame_free: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(*mut Raw),
                >(util.symbol(c"av_frame_free")?)
            },
            // SAFETY: AVFrameSideData *av_frame_new_side_data(AVFrame *frame, enum AVFrameSideDataType type, size_t size);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_frame_new_side_data: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, c_int, usize) -> Raw,
                >(util.symbol(c"av_frame_new_side_data")?)
            },
            // SAFETY: void av_frame_remove_side_data(AVFrame *frame, enum AVFrameSideDataType type);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_frame_remove_side_data: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, c_int),
                >(util.symbol(c"av_frame_remove_side_data")?)
            },
            // SAFETY: AVBufferRef *av_buffer_ref(const AVBufferRef *buf);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_buffer_ref: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw) -> Raw,
                >(util.symbol(c"av_buffer_ref")?)
            },
            // SAFETY: void av_buffer_unref(AVBufferRef **buf);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_buffer_unref: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(*mut Raw),
                >(util.symbol(c"av_buffer_unref")?)
            },
            // SAFETY: AVBufferRef *av_hwdevice_ctx_alloc(enum AVHWDeviceType type);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_hwdevice_ctx_alloc: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(c_int) -> Raw,
                >(util.symbol(c"av_hwdevice_ctx_alloc")?)
            },
            // SAFETY: int av_hwdevice_ctx_init(AVBufferRef *ref);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_hwdevice_ctx_init: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw) -> c_int,
                >(util.symbol(c"av_hwdevice_ctx_init")?)
            },
            // SAFETY: int av_hwdevice_ctx_create_derived(AVBufferRef **dst_ctx, enum AVHWDeviceType type, AVBufferRef *src_ctx, int flags);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_hwdevice_ctx_create_derived: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(*mut Raw, c_int, Raw, c_int) -> c_int,
                >(util.symbol(c"av_hwdevice_ctx_create_derived")?)
            },
            // SAFETY: AVBufferRef *av_hwframe_ctx_alloc(AVBufferRef *device_ctx);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_hwframe_ctx_alloc: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw) -> Raw,
                >(util.symbol(c"av_hwframe_ctx_alloc")?)
            },
            // SAFETY: int av_hwframe_ctx_init(AVBufferRef *ref);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_hwframe_ctx_init: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw) -> c_int,
                >(util.symbol(c"av_hwframe_ctx_init")?)
            },
            // SAFETY: int av_hwframe_get_buffer(AVBufferRef *hwframe_ctx, AVFrame *frame, int flags);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_hwframe_get_buffer: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, Raw, c_int) -> c_int,
                >(util.symbol(c"av_hwframe_get_buffer")?)
            },
            // SAFETY: int av_hwframe_map(AVFrame *dst, const AVFrame *src, int flags);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_hwframe_map: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, Raw, c_int) -> c_int,
                >(util.symbol(c"av_hwframe_map")?)
            },
            // SAFETY: int av_opt_set(void *obj, const char *name, const char *val, int search_flags);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_opt_set: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int) -> c_int,
                >(util.symbol(c"av_opt_set")?)
            },
            // SAFETY: const AVOption *av_opt_find(void *obj, const char *name, const char *unit, int opt_flags, int search_flags);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_opt_find: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, *const c_char, *const c_char, c_int, c_int) -> Raw,
                >(util.symbol(c"av_opt_find")?)
            },
            // SAFETY: unsigned avcodec_version(void);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_version: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn() -> u32,
                >(codec.symbol(c"avcodec_version")?)
            },
            // SAFETY: const char *avcodec_license(void);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_license: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn() -> *const c_char,
                >(codec.symbol(c"avcodec_license")?)
            },
            // SAFETY: const char *avcodec_configuration(void);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_configuration: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn() -> *const c_char,
                >(codec.symbol(c"avcodec_configuration")?)
            },
            // SAFETY: const AVCodec *avcodec_find_encoder_by_name(const char *name);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_find_encoder_by_name: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(*const c_char) -> Raw,
                >(codec.symbol(c"avcodec_find_encoder_by_name")?)
            },
            // SAFETY: AVCodecContext *avcodec_alloc_context3(const AVCodec *codec);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_alloc_context3: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw) -> Raw,
                >(codec.symbol(c"avcodec_alloc_context3")?)
            },
            // SAFETY: void avcodec_free_context(AVCodecContext **avctx);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_free_context: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(*mut Raw),
                >(codec.symbol(c"avcodec_free_context")?)
            },
            // SAFETY: int avcodec_open2(AVCodecContext *avctx, const AVCodec *codec, AVDictionary **options);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_open2: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, Raw, *mut Raw) -> c_int,
                >(codec.symbol(c"avcodec_open2")?)
            },
            // SAFETY: int avcodec_send_frame(AVCodecContext *avctx, const AVFrame *frame);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_send_frame: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, Raw) -> c_int,
                >(codec.symbol(c"avcodec_send_frame")?)
            },
            // SAFETY: int avcodec_receive_packet(AVCodecContext *avctx, AVPacket *avpkt);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            avcodec_receive_packet: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(Raw, Raw) -> c_int,
                >(codec.symbol(c"avcodec_receive_packet")?)
            },
            // SAFETY: AVPacket *av_packet_alloc(void);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_packet_alloc: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn() -> Raw,
                >(codec.symbol(c"av_packet_alloc")?)
            },
            // SAFETY: void av_packet_free(AVPacket **pkt);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_packet_free: unsafe {
                std::mem::transmute::<
                    unsafe extern "system" fn() -> isize,
                    unsafe extern "C" fn(*mut Raw),
                >(codec.symbol(c"av_packet_free")?)
            },
            // SAFETY: void av_packet_unref(AVPacket *pkt);
            // Win64 uses one calling convention; symbol is from its owning DLL.
            av_packet_unref: unsafe {
                std::mem::transmute::<unsafe extern "system" fn() -> isize, unsafe extern "C" fn(Raw)>(
                    codec.symbol(c"av_packet_unref")?,
                )
            },
        })
    }
}

static LOADED: Mutex<Option<Arc<FfLib>>> = Mutex::new(None);
static SHARED: OnceLock<Result<Arc<FfLib>, String>> = OnceLock::new();
impl FfLib {
    pub fn load(dir: &Path) -> Result<Arc<Self>, PlatformError> {
        if usize::BITS != 64 {
            return Err(platform("FFmpeg ABI requires 64-bit Windows"));
        }
        let dir =
            std::path::absolute(dir).map_err(|e| platform(format!("FFmpeg directory: {e}")))?;
        let mut loaded = LOADED
            .lock()
            .map_err(|_| platform("FFmpeg loader mutex poisoned"))?;
        if let Some(lib) = loaded.as_ref() {
            return if lib.versions.dir == dir {
                Ok(Arc::clone(lib))
            } else {
                Err(platform(format!(
                    "FFmpeg already loaded from {}",
                    lib.versions.dir.display()
                )))
            };
        }
        let util = Module::open(&dir.join(AVUTIL_DLL))?;
        let codec = Module::open(&dir.join(AVCODEC_DLL))?;
        let api = Api::resolve(&util, &codec)?;
        // SAFETY: resolved version and static C string accessors have no preconditions.
        let versions = unsafe {
            let avcodec = (api.avcodec_version)();
            let avutil = (api.avutil_version)();
            check_versions(avcodec, avutil).map_err(|e| platform(e.to_string()))?;
            let license = static_string((api.avcodec_license)())?;
            let util_license = static_string((api.avutil_license)())?;
            let configuration = static_string((api.avcodec_configuration)())?;
            check_license(&license, &util_license, &configuration)
                .map_err(|e| platform(e.to_string()))?;
            FfVersions {
                avcodec,
                avutil,
                license,
                configuration,
                dir,
            }
        };
        // SAFETY: null codec is supported for inspecting AVCodecContext AVOptions.
        let mut ctx = unsafe { (api.avcodec_alloc_context3)(ptr::null_mut()) };
        if ctx.is_null() {
            return Err(platform("avcodec_alloc_context3: ENOMEM"));
        }
        // Free even if an option lookup/string conversion fails.
        let offsets = (|| {
            let mut found = Vec::with_capacity(OPTION_OFFSETS.len());
            for &(name, _) in OPTION_OFFSETS {
                let cname = CString::new(name).map_err(|e| platform(e.to_string()))?;
                // SAFETY: live context and C name; public AVOption layout is stable.
                let value = unsafe {
                    let opt = (api.av_opt_find)(ctx, cname.as_ptr(), ptr::null(), 0, 0);
                    if opt.is_null() {
                        None
                    } else {
                        Some(read_field::<i32>(opt, off::option::OFFSET))
                    }
                };
                found.push((name, value));
            }
            check_offsets(&found).map_err(|e| platform(e.to_string()))
        })();
        // SAFETY: ctx owns the allocation above, and no options outlive it.
        unsafe {
            (api.avcodec_free_context)(&mut ctx);
        }
        offsets?;
        let lib = Arc::new(Self {
            api,
            versions,
            _modules: [util.0 .0 as usize, codec.0 .0 as usize],
        });
        // Success pins the code for both library clients and FFmpeg's global callback.
        std::mem::forget(codec);
        std::mem::forget(util);
        lib.install_log();
        *loaded = Some(Arc::clone(&lib));
        Ok(lib)
    }
    pub fn shared() -> Result<Arc<Self>, PlatformError> {
        SHARED
            .get_or_init(|| {
                let result = (|| {
                    let exe = std::env::current_exe().map_err(|e| platform(e.to_string()))?;
                    let parent = exe
                        .parent()
                        .ok_or_else(|| platform("current executable has no directory"))?;
                    Self::load(&ffmpeg_dir(
                        parent,
                        std::env::var(FFMPEG_DIR_ENV).ok().as_deref(),
                    ))
                })();
                result.map_err(|error| {
                    tracing::warn!(%error, "FFmpeg unavailable");
                    match error {
                        PlatformError::Platform(message) => message,
                        other => other.to_string(),
                    }
                })
            })
            .as_ref()
            .map(Arc::clone)
            .map_err(|message| platform(message.clone()))
    }
    pub fn versions(&self) -> &FfVersions {
        &self.versions
    }
    pub fn has_encoder(&self, name: &str) -> bool {
        let Ok(name) = CString::new(name) else {
            return false;
        };
        // SAFETY: C name lives through the call; descriptor belongs to the pinned DLL.
        unsafe { !(self.api.avcodec_find_encoder_by_name)(name.as_ptr()).is_null() }
    }
    fn install_log(&self) {
        let level = if std::env::var("RS_FFMPEG_LOG").as_deref() == Ok("verbose") {
            AV_LOG_VERBOSE
        } else {
            AV_LOG_WARNING
        };
        LOG.get_or_init(|| LogState {
            format: self.api.av_log_format_line2,
            level,
            limiter: Mutex::new(LogLimiter::new(20)),
        });
        // SAFETY: installed once under LOADED; DLL and callback state live forever.
        unsafe {
            (self.api.av_log_set_level)(level);
            (self.api.av_log_set_callback)(Some(log_callback));
        }
    }
}
unsafe fn static_string(value: *const c_char) -> Result<String, PlatformError> {
    if value.is_null() {
        return Err(platform("null FFmpeg metadata string"));
    }
    // SAFETY: caller supplies a static, terminated FFmpeg metadata string.
    Ok(unsafe { CStr::from_ptr(value) }
        .to_string_lossy()
        .into_owned())
}
unsafe fn read_field<T: Copy>(base: Raw, offset: usize) -> T {
    // SAFETY: caller establishes the allocation/layout and field type; unaligned
    // access also handles explicitly packed C fields without Rust references.
    unsafe { base.cast::<u8>().add(offset).cast::<T>().read_unaligned() }
}
unsafe fn write_field<T>(base: Raw, offset: usize, value: T) {
    // SAFETY: caller owns the allocation and establishes the field's ABI type.
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
        let Some(state) = LOG.get() else {
            return;
        };
        let Some(target) = log_target(level) else {
            return;
        };
        if level > state.level || fmt.is_null() {
            return;
        }
        let mut buf = [0u8; 1024];
        let mut prefix = 1;
        // SAFETY: callback arguments come from FFmpeg. Win64 va_list is char*;
        // forward it unchanged to FFmpeg with a live, bounded output buffer.
        unsafe {
            (state.format)(
                avcl,
                level,
                fmt,
                vl,
                buf.as_mut_ptr().cast(),
                buf.len() as i32,
                &mut prefix,
            );
        }
        let len = buf.iter().position(|b| *b == 0).unwrap_or(buf.len());
        let line = clean_log_line(&buf[..len]);
        let Ok(mut limiter) = state.limiter.try_lock() else {
            return;
        };
        let now = SystemTime::now()
            .duration_since(UNIX_EPOCH)
            .map(|d| d.as_micros().min(u64::MAX as u128) as u64)
            .unwrap_or(0);
        let admit = limiter.admit(now);
        drop(limiter);
        if let LogAdmit::PassWithSuppressed(n) = admit {
            emit_log(target, &format!("suppressed {n} lines"));
        }
        if admit != LogAdmit::Drop {
            emit_log(target, &line);
        }
    });
    if let Err(payload) = result {
        // A subscriber may panic with a payload whose Drop also panics. Do not
        // let destruction of that payload unwind through the C callback.
        std::mem::forget(payload);
    }
}
fn emit_log(target: LogTarget, line: &str) {
    match target {
        LogTarget::Error => tracing::error!(target: "ffmpeg", "{line}"),
        LogTarget::Warn => tracing::warn!(target: "ffmpeg", "{line}"),
        LogTarget::Debug => tracing::debug!(target: "ffmpeg", "{line}"),
        LogTarget::Trace => tracing::trace!(target: "ffmpeg", "{line}"),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum HwPath {
    D3d11,
    Qsv,
}
pub struct FfOpen<'a> {
    pub codec_name: &'a str,
    pub hw: HwPath,
    pub width: u32,
    pub height: u32,
    pub device: &'a ID3D11Device,
    pub options: &'a [(&'a str, &'a str, bool)],
}
pub struct InputSurface {
    pub texture: ID3D11Texture2D,
    pub array_index: u32,
    pub width: u32,
    pub height: u32,
}
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct FfPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub keyframe: bool,
}
#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum FfError {
    #[error("ffmpeg unsupported: {0}")]
    Unsupported(String),
    #[error("ffmpeg {op} failed: {name} ({code})")]
    Failed {
        op: &'static str,
        code: i32,
        name: String,
    },
}
fn failed(op: &'static str, code: i32) -> FfError {
    FfError::Failed {
        op,
        code,
        name: err_name(code),
    }
}
fn check(op: &'static str, code: i32) -> Result<(), FfError> {
    if code < 0 {
        Err(failed(op, code))
    } else {
        Ok(())
    }
}
fn nonnull(op: &'static str, p: Raw) -> Result<Raw, FfError> {
    if p.is_null() {
        Err(failed(op, AVERROR_ENOMEM))
    } else {
        Ok(p)
    }
}
macro_rules! owner {
    ($name:ident, $free:ident) => {
        struct $name {
            ptr: Raw,
            lib: Arc<FfLib>,
        }
        impl $name {
            fn empty(lib: &Arc<FfLib>) -> Self {
                Self {
                    ptr: ptr::null_mut(),
                    lib: Arc::clone(lib),
                }
            }
        }
        impl Drop for $name {
            fn drop(&mut self) {
                // SAFETY: owns this nullable FFmpeg allocation; matching free
                // function takes its address, releases it, and sets it to NULL.
                unsafe {
                    (self.lib.api.$free)(&mut self.ptr);
                }
            }
        }
    };
}
owner!(Frame, av_frame_free);
owner!(Packet, av_packet_free);
owner!(Context, avcodec_free_context);
owner!(Buffer, av_buffer_unref);

// Declaration order is release order, including every partial-open error path.
struct Resources {
    mapped: Frame,
    frame: Frame,
    packet: Packet,
    ctx: Context,
    frames: Buffer,
    qsv: Buffer,
    d3d: Buffer,
}
impl Resources {
    fn new(lib: &Arc<FfLib>) -> Self {
        Self {
            mapped: Frame::empty(lib),
            frame: Frame::empty(lib),
            packet: Packet::empty(lib),
            ctx: Context::empty(lib),
            frames: Buffer::empty(lib),
            qsv: Buffer::empty(lib),
            d3d: Buffer::empty(lib),
        }
    }
}
pub struct FfEncoder {
    input: InputSurface,
    res: Resources,
    lib: Arc<FfLib>,
    codec_name: String,
    // A context and its mutable single-surface pool must never cross threads.
    _thread: PhantomData<Rc<()>>,
}
impl FfEncoder {
    pub fn open(lib: &Arc<FfLib>, req: &FfOpen<'_>) -> Result<Self, FfError> {
        let name = CString::new(req.codec_name)
            .map_err(|_| FfError::Unsupported("codec name contains NUL".into()))?;
        let mut res = Resources::new(lib);
        let api = &lib.api;
        // SAFETY: validated FFmpeg ABI; all allocations immediately enter RAII
        // owners. Only documented writable public fields are touched. FFmpeg
        // owns the extra device AddRef and the context's frames reference.
        let input = unsafe {
            let codec = (api.avcodec_find_encoder_by_name)(name.as_ptr());
            if codec.is_null() {
                return Err(FfError::Unsupported(format!(
                    "encoder {} not in this FFmpeg build",
                    req.codec_name
                )));
            }
            let width = i32::try_from(req.width)
                .ok()
                .filter(|v| *v > 0 && *v % 2 == 0);
            let height = i32::try_from(req.height)
                .ok()
                .filter(|v| *v > 0 && *v % 2 == 0);
            let (Some(width), Some(height)) = (width, height) else {
                return Err(FfError::Unsupported(
                    "NV12 dimensions must be positive, even i32 values".into(),
                ));
            };
            res.d3d.ptr = nonnull(
                "av_hwdevice_ctx_alloc",
                (api.av_hwdevice_ctx_alloc)(AV_HWDEVICE_TYPE_D3D11VA),
            )?;
            let dev = read_field::<Raw>(res.d3d.ptr, off::buffer_ref::DATA);
            let hw = read_field::<Raw>(dev, off::hwdev::HWCTX);
            write_field(hw, off::d3d11_dev::DEVICE, req.device.clone().into_raw());
            let code = (api.av_hwdevice_ctx_init)(res.d3d.ptr);
            if code < 0 {
                return Err(FfError::Unsupported(format!(
                    "d3d11va device init: {}",
                    err_name(code)
                )));
            }
            let (device, format) = match req.hw {
                HwPath::D3d11 => (res.d3d.ptr, AV_PIX_FMT_D3D11),
                HwPath::Qsv => {
                    let code = (api.av_hwdevice_ctx_create_derived)(
                        &mut res.qsv.ptr,
                        AV_HWDEVICE_TYPE_QSV,
                        res.d3d.ptr,
                        0,
                    );
                    if code < 0 {
                        return Err(FfError::Unsupported(format!(
                            "qsv device derive: {}",
                            err_name(code)
                        )));
                    }
                    (
                        nonnull("av_hwdevice_ctx_create_derived", res.qsv.ptr)?,
                        AV_PIX_FMT_QSV,
                    )
                }
            };
            res.frames.ptr = nonnull("av_hwframe_ctx_alloc", (api.av_hwframe_ctx_alloc)(device))?;
            let frames = read_field::<Raw>(res.frames.ptr, off::buffer_ref::DATA);
            write_field(frames, off::hwframes::FORMAT, format);
            write_field(frames, off::hwframes::SW_FORMAT, AV_PIX_FMT_NV12);
            write_field(frames, off::hwframes::WIDTH, width);
            write_field(frames, off::hwframes::HEIGHT, height);
            write_field(frames, off::hwframes::INITIAL_POOL_SIZE, 1i32);
            if req.hw == HwPath::D3d11 {
                let hw = read_field::<Raw>(frames, off::hwframes::HWCTX);
                write_field(hw, off::d3d11_frames::BIND_FLAGS, D3D11_BIND_RENDER_TARGET);
                write_field(hw, off::d3d11_frames::MISC_FLAGS, 0u32);
            }
            check(
                "av_hwframe_ctx_init",
                (api.av_hwframe_ctx_init)(res.frames.ptr),
            )?;
            res.ctx.ptr = nonnull(
                "avcodec_alloc_context3",
                (api.avcodec_alloc_context3)(codec),
            )?;
            apply_options(api, res.ctx.ptr, req.options)?;
            let frames_ref = nonnull("av_buffer_ref", (api.av_buffer_ref)(res.frames.ptr))?;
            write_field(res.ctx.ptr, off::ctx::HW_FRAMES_CTX, frames_ref);
            let code = (api.avcodec_open2)(res.ctx.ptr, codec, ptr::null_mut());
            if code < 0 {
                return Err(if open_error_is_unsupported(code) {
                    FfError::Unsupported(format!("avcodec_open2: {}", err_name(code)))
                } else {
                    failed("avcodec_open2", code)
                });
            }
            res.frame.ptr = nonnull("av_frame_alloc", (api.av_frame_alloc)())?;
            check(
                "av_hwframe_get_buffer",
                (api.av_hwframe_get_buffer)(res.frames.ptr, res.frame.ptr, 0),
            )?;
            validate_frame(res.frame.ptr, format, width, height)?;
            let surface = if req.hw == HwPath::Qsv {
                res.mapped.ptr = nonnull("av_frame_alloc", (api.av_frame_alloc)())?;
                write_field(res.mapped.ptr, off::frame::FORMAT, AV_PIX_FMT_D3D11);
                check(
                    "av_hwframe_map",
                    (api.av_hwframe_map)(
                        res.mapped.ptr,
                        res.frame.ptr,
                        AV_HWFRAME_MAP_WRITE | AV_HWFRAME_MAP_OVERWRITE,
                    ),
                )?;
                validate_frame(res.mapped.ptr, AV_PIX_FMT_D3D11, width, height)?;
                res.mapped.ptr
            } else {
                res.frame.ptr
            };
            let texture = read_field::<Raw>(surface, off::frame::DATA);
            let texture = ID3D11Texture2D::from_raw_borrowed(&texture)
                .ok_or_else(|| failed("abi_frame", 0))?
                .clone();
            let index =
                read_field::<Raw>(surface, off::frame::DATA + std::mem::size_of::<Raw>()) as usize;
            let array_index = u32::try_from(index).map_err(|_| failed("abi_frame", 0))?;
            res.packet.ptr = nonnull("av_packet_alloc", (api.av_packet_alloc)())?;
            InputSurface {
                texture,
                array_index,
                width: req.width,
                height: req.height,
            }
        };
        Ok(Self {
            input,
            res,
            lib: Arc::clone(lib),
            codec_name: req.codec_name.into(),
            _thread: PhantomData,
        })
    }
    pub fn input(&self) -> &InputSurface {
        &self.input
    }
    pub fn codec_name(&self) -> &str {
        &self.codec_name
    }
    pub fn set_options(&mut self, options: &[(&str, &str, bool)]) -> Result<(), FfError> {
        // SAFETY: this encoder exclusively owns the live context.
        unsafe { apply_options(&self.lib.api, self.res.ctx.ptr, options) }
    }
    pub fn encode(
        &mut self,
        pts: i64,
        idr: bool,
        roi_qoffset: Option<(i32, i32)>,
    ) -> Result<Vec<FfPacket>, FfError> {
        let api = &self.lib.api;
        let frame = self.res.frame.ptr;
        // SAFETY: frame owns its storage; only documented fields and allocated
        // side-data bytes are written. QSV submits its original QSV frame.
        unsafe {
            write_field(frame, off::frame::PTS, pts);
            write_field(
                frame,
                off::frame::PICT_TYPE,
                if idr {
                    AV_PICTURE_TYPE_I
                } else {
                    AV_PICTURE_TYPE_NONE
                },
            );
            if let Some(q) = roi_qoffset {
                let side = nonnull(
                    "av_frame_new_side_data",
                    (api.av_frame_new_side_data)(
                        frame,
                        AV_FRAME_DATA_REGIONS_OF_INTEREST,
                        ROI_SIZE,
                    ),
                )?;
                let data = read_field::<*mut u8>(side, off::side_data::DATA);
                ptr::copy_nonoverlapping(
                    roi_bytes(self.input.width, self.input.height, q).as_ptr(),
                    data,
                    ROI_SIZE,
                );
            }
        }
        let mut packets = Vec::new();
        let submitted = (|| -> Result<Instant, FfError> {
            let mut sent_at = Instant::now();
            // SAFETY: live context and frame, owned exclusively by this encoder.
            let mut code = unsafe { (api.avcodec_send_frame)(self.res.ctx.ptr, frame) };
            if code == AVERROR_EAGAIN {
                self.receive(&mut packets, None)?;
                sent_at = Instant::now();
                // SAFETY: draining made room; frame and ROI are still retained.
                code = unsafe { (api.avcodec_send_frame)(self.res.ctx.ptr, frame) };
            }
            check("avcodec_send_frame", code)?;
            Ok(sent_at)
        })();
        // SAFETY: remove even after send/drain failure; FFmpeg retained any
        // side-data references it needs during the successful send_frame call.
        unsafe {
            (api.av_frame_remove_side_data)(frame, AV_FRAME_DATA_REGIONS_OF_INTEREST);
        }
        let sent_at = submitted?;
        self.receive(&mut packets, Some((pts, sent_at)))?;
        tracing::trace!(target: "ffmpeg", pts, packets = packets.len(), "encoded frame");
        Ok(packets)
    }
    fn receive(
        &self,
        packets: &mut Vec<FfPacket>,
        wait: Option<(i64, Instant)>,
    ) -> Result<(), FfError> {
        let api = &self.lib.api;
        loop {
            // SAFETY: live context, reusable allocated packet, exclusive encoder use.
            let code =
                unsafe { (api.avcodec_receive_packet)(self.res.ctx.ptr, self.res.packet.ptr) };
            match code {
                0 => {
                    // SAFETY: receive succeeded; fields belong to this valid packet.
                    let packet = unsafe { copy_packet(self.res.packet.ptr) };
                    // SAFETY: release the received payload even if validation failed.
                    unsafe {
                        (api.av_packet_unref)(self.res.packet.ptr);
                    }
                    packets.push(packet?);
                }
                AVERROR_EAGAIN => {
                    if wait.is_some_and(|(pts, start)| {
                        !packets.iter().any(|p| p.pts == pts)
                            && start.elapsed().as_micros() < u128::from(RECEIVE_WAIT_US)
                    }) {
                        std::thread::yield_now();
                    } else {
                        return Ok(());
                    }
                }
                AVERROR_EOF => return Ok(()),
                _ => return Err(failed("avcodec_receive_packet", code)),
            }
        }
    }
}
unsafe fn apply_options(
    api: &Api,
    ctx: Raw,
    options: &[(&str, &str, bool)],
) -> Result<(), FfError> {
    for &(name, value, required) in options {
        let pair = CString::new(name).and_then(|n| CString::new(value).map(|v| (n, v)));
        let code = match pair {
            // SAFETY: caller owns ctx; both C strings live through the call.
            Ok((n, v)) => unsafe {
                (api.av_opt_set)(ctx, n.as_ptr(), v.as_ptr(), AV_OPT_SEARCH_CHILDREN)
            },
            Err(_) => AVERROR_EINVAL,
        };
        if code < 0 {
            if required {
                return Err(FfError::Unsupported(format!(
                    "option {name}={value}: {}",
                    err_name(code)
                )));
            }
            tracing::debug!(target: "ffmpeg", option = name, error = %err_name(code), "optional option rejected");
        }
    }
    Ok(())
}
unsafe fn validate_frame(frame: Raw, format: i32, width: i32, height: i32) -> Result<(), FfError> {
    // SAFETY: allocated frame from the checked ABI, only documented fields read.
    let (got_format, got_width, got_height, data) = unsafe {
        (
            read_field::<i32>(frame, off::frame::FORMAT),
            read_field::<i32>(frame, off::frame::WIDTH),
            read_field::<i32>(frame, off::frame::HEIGHT),
            read_field::<Raw>(frame, hw_frame_data_offset(format)),
        )
    };
    if (got_format, got_width, got_height) != (format, width, height) || data.is_null() {
        return Err(FfError::Failed {
            op: "abi_frame",
            code: 0,
            name: format!(
                "format/size/data mismatch: {got_format}/{got_width}x{got_height}/null={}",
                data.is_null()
            ),
        });
    }
    Ok(())
}
unsafe fn copy_packet(packet: Raw) -> Result<FfPacket, FfError> {
    // SAFETY: caller supplies a packet successfully returned by receive_packet.
    unsafe {
        let size = read_field::<i32>(packet, off::packet::SIZE);
        let data = read_field::<*const u8>(packet, off::packet::DATA);
        if size < 0 || (size > 0 && data.is_null()) {
            return Err(failed("abi_packet", 0));
        }
        let data = if size == 0 {
            Vec::new()
        } else {
            std::slice::from_raw_parts(data, size as usize).to_vec()
        };
        Ok(FfPacket {
            data,
            pts: read_field(packet, off::packet::PTS),
            keyframe: read_field::<i32>(packet, off::packet::FLAGS) & AV_PKT_FLAG_KEY != 0,
        })
    }
}
impl Drop for FfEncoder {
    fn drop(&mut self) {
        let api = &self.lib.api;
        let start = Instant::now();
        // SAFETY: resources remain alive throughout flushing. NULL signals EOF;
        // unref each discarded packet; Resources then frees in dependency order.
        unsafe {
            let mut sent = (api.avcodec_send_frame)(self.res.ctx.ptr, ptr::null_mut());
            while start.elapsed().as_micros() < u128::from(FLUSH_WAIT_US) {
                let code = (api.avcodec_receive_packet)(self.res.ctx.ptr, self.res.packet.ptr);
                match code {
                    0 => (api.av_packet_unref)(self.res.packet.ptr),
                    AVERROR_EOF => break,
                    AVERROR_EAGAIN => {
                        if sent == AVERROR_EAGAIN {
                            sent = (api.avcodec_send_frame)(self.res.ctx.ptr, ptr::null_mut());
                        }
                        if sent < 0 && sent != AVERROR_EAGAIN {
                            break;
                        }
                        std::thread::yield_now();
                    }
                    _ => break,
                }
            }
        }
    }
}
