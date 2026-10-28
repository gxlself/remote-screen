#![cfg(windows)]
//! Asynchronous Media Foundation hardware encoder fallback.
//!
//! The caller must have called `CoInitializeEx(None, COINIT_MULTITHREADED)` on
//! its thread. `mf_startup` initializes Media Foundation once for the process.
//! The encoder feeds the texture returned by [`MfEncoder::input_slot`] after a Video
//! Processor blit on the same immediate context. A `None` slot is a deliberate
//! busy/drop decision. Rate changes do not request an IDR; one-frame repair
//! traffic is passed as `bump_bps`.

use std::{
    mem::ManuallyDrop,
    ptr,
    sync::{mpsc, OnceLock},
    thread::{self, JoinHandle},
    time::{Duration, Instant},
};

use host_core::platform::PlatformError;
use rsp_core::pb::Codec;
use tracing::{debug, trace, warn};
use windows::{
    core::{Interface, GUID},
    Win32::{
        Foundation::{E_FAIL, VARIANT_BOOL},
        Graphics::{
            Direct3D11::{
                ID3D11Device, ID3D11Texture2D, D3D11_BIND_RENDER_TARGET,
                D3D11_BIND_SHADER_RESOURCE, D3D11_TEXTURE2D_DESC, D3D11_USAGE_DEFAULT,
            },
            Dxgi::Common::{DXGI_FORMAT_NV12, DXGI_SAMPLE_DESC},
        },
        Media::MediaFoundation::*,
        System::{
            Com::{CoInitializeEx, CoTaskMemFree, COINIT_MULTITHREADED},
            Variant::{VARIANT, VARIANT_0, VARIANT_0_0, VARIANT_0_0_0, VT_BOOL, VT_UI4},
        },
    },
};

use crate::vpmf_model::{
    hns_per_frame, luid_blob, mf_codecapi, mf_event_kind, mf_rate_props, mf_type_attrs,
    pts_from_sample_time, sample_time, MfApi, MfAttr, MfEventKind, MfFlow, MfProp, MfValue,
    SurfaceRing, MF_ENCODE_WAIT_US, MF_INPUT_POOL,
};

static MF_STARTED: OnceLock<Result<(), String>> = OnceLock::new();

fn platform(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Platform(error.to_string())
}

fn hr_failed(op: &'static str, error: &windows::core::Error) -> MfError {
    MfError::Failed {
        op,
        hr: error.code().0,
    }
}

fn unsupported(message: impl Into<String>) -> MfError {
    MfError::Unsupported(message.into())
}

pub fn mf_startup() -> Result<(), PlatformError> {
    let result = MF_STARTED.get_or_init(|| {
        unsafe {
            // SAFETY: MFStartup is process-global and guarded by OnceLock.
            MFStartup(MF_VERSION, MFSTARTUP_LITE).map_err(|error| error.to_string())
        }
    });
    result.clone().map_err(PlatformError::Platform)
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MfEncoderInfo {
    pub name: String,
    pub vendor: String,
    pub is_async: bool,
    pub d3d11_aware: bool,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct MfPacket {
    pub data: Vec<u8>,
    pub pts: i64,
    pub keyframe: bool,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum MfError {
    #[error("mf unsupported: {0}")]
    Unsupported(String),
    #[error("mf {op} failed: {hr:#010x}")]
    Failed { op: &'static str, hr: i32 },
    #[error("mf encoder busy")]
    Busy,
}

pub struct MfOpen<'a> {
    pub device: &'a ID3D11Device,
    pub adapter_luid: i64,
    pub vendor_id: u32,
    pub codec: Codec,
    pub width: u32,
    pub height: u32,
    pub fps: u32,
    pub bitrate_bps: u64,
}

fn codec_subtype(codec: Codec) -> Option<GUID> {
    match codec {
        Codec::H264 => Some(MFVideoFormat_H264),
        Codec::Hevc => Some(MFVideoFormat_HEVC),
        Codec::Unspecified => None,
    }
}

fn codec_name(codec: Codec) -> &'static str {
    match codec {
        Codec::H264 => "h264",
        Codec::Hevc => "hevc",
        Codec::Unspecified => "unspecified",
    }
}

fn prop_guid(api: MfApi) -> GUID {
    match api {
        MfApi::LowLatencyMode => CODECAPI_AVLowLatencyMode,
        MfApi::RateControlMode => CODECAPI_AVEncCommonRateControlMode,
        MfApi::MeanBitRate => CODECAPI_AVEncCommonMeanBitRate,
        MfApi::BufferSize => CODECAPI_AVEncCommonBufferSize,
        MfApi::GopSize => CODECAPI_AVEncMPVGOPSize,
        MfApi::BPictureCount => CODECAPI_AVEncMPVDefaultBPictureCount,
        MfApi::QualityVsSpeed => CODECAPI_AVEncCommonQualityVsSpeed,
        MfApi::ForceKeyFrame => CODECAPI_AVEncVideoForceKeyFrame,
    }
}

fn variant_value(value: MfValue) -> VARIANT {
    let (vt, inner) = match value {
        MfValue::U32(value) => (VT_UI4, VARIANT_0_0_0 { ulVal: value }),
        MfValue::Bool(value) => (
            VT_BOOL,
            VARIANT_0_0_0 {
                boolVal: VARIANT_BOOL(if value { -1 } else { 0 }),
            },
        ),
    };
    VARIANT {
        Anonymous: VARIANT_0 {
            Anonymous: ManuallyDrop::new(VARIANT_0_0 {
                vt,
                wReserved1: 0,
                wReserved2: 0,
                wReserved3: 0,
                Anonymous: inner,
            }),
        },
    }
}

fn set_codec_prop(api: &ICodecAPI, prop: MfProp) -> Result<(), windows::core::Error> {
    let value = variant_value(prop.value);
    unsafe {
        // SAFETY: `value` is a fully initialized stack VARIANT and api is a live
        // COM interface. SetValue does not retain the pointer.
        api.SetValue(&prop_guid(prop.api), &value)
    }
}

fn activate_name(activate: &IMFActivate) -> String {
    let length = unsafe {
        // SAFETY: activate is a live IMFAttributes interface.
        activate.GetStringLength(&MFT_FRIENDLY_NAME_Attribute)
    }
    .unwrap_or(0) as usize;
    if length == 0 {
        return "Media Foundation encoder".into();
    }
    let mut value = vec![0u16; length.saturating_add(1)];
    let _ = unsafe {
        // SAFETY: buffer is writable and sized from GetStringLength.
        activate.GetString(&MFT_FRIENDLY_NAME_Attribute, &mut value, None)
    };
    String::from_utf16_lossy(&value[..value.iter().position(|&c| c == 0).unwrap_or(value.len())])
}

struct EnumActivates {
    items: Vec<IMFActivate>,
    used_fallback: bool,
}

fn enum_activates(
    luid: Option<i64>,
    codec: Codec,
    use_luid: bool,
) -> Result<EnumActivates, windows::core::Error> {
    let subtype = codec_subtype(codec).ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?;
    let input = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: MFVideoFormat_NV12,
    };
    let output = MFT_REGISTER_TYPE_INFO {
        guidMajorType: MFMediaType_Video,
        guidSubtype: subtype,
    };
    let attrs = unsafe {
        // SAFETY: MFCreateAttributes writes one owned attribute store.
        let mut attrs = None;
        MFCreateAttributes(&mut attrs, 2)?;
        attrs.ok_or_else(|| windows::core::Error::from_hresult(E_FAIL))?
    };
    if use_luid {
        if let Some(luid) = luid {
            unsafe {
                // SAFETY: blob is copied by the attribute store during this call.
                attrs.SetBlob(&MFT_ENUM_ADAPTER_LUID, &luid_blob(luid))?;
            }
        }
    }
    let mut ptr_activates: *mut Option<IMFActivate> = ptr::null_mut();
    let mut count = 0u32;
    let mut used_fallback = false;
    let flags = MFT_ENUM_FLAG_HARDWARE | MFT_ENUM_FLAG_SORTANDFILTER;
    let enum_result = unsafe {
        // SAFETY: pointers reference initialized type descriptors and writable
        // output storage; MF allocates the array with CoTaskMemAlloc.
        MFTEnum2(
            MFT_CATEGORY_VIDEO_ENCODER,
            flags,
            Some(&input),
            Some(&output),
            &attrs,
            &mut ptr_activates,
            &mut count,
        )
    };
    if enum_result.is_err() {
        used_fallback = true;
        ptr_activates = ptr::null_mut();
        count = 0;
        unsafe {
            // SAFETY: fallback uses the same descriptors and output ownership.
            MFTEnumEx(
                MFT_CATEGORY_VIDEO_ENCODER,
                flags,
                Some(&input),
                Some(&output),
                &mut ptr_activates,
                &mut count,
            )?;
        }
    }
    if ptr_activates.is_null() || count == 0 {
        return Ok(EnumActivates {
            items: Vec::new(),
            used_fallback,
        });
    }
    let mut result = Vec::with_capacity(count as usize);
    for index in 0..count as usize {
        unsafe {
            // SAFETY: MF returned `count` contiguous Option<IMFActivate> entries.
            let entry = ptr_activates.add(index).read();
            if let Some(activate) = entry {
                result.push(activate);
            }
        }
    }
    unsafe {
        // SAFETY: the array allocation belongs to MF and all entries were moved
        // out before freeing the outer allocation.
        CoTaskMemFree(Some(ptr_activates.cast()));
    }
    Ok(EnumActivates {
        items: result,
        used_fallback,
    })
}

fn activate_matches_vendor(activate: &IMFActivate, vendor_id: u32) -> bool {
    let expected = crate::vpmf_model::mf_vendor_attr(vendor_id);
    let length = unsafe {
        // SAFETY: live attribute store.
        activate.GetStringLength(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
    }
    .unwrap_or(0) as usize;
    if length == 0 {
        return false;
    }
    let mut chars = vec![0u16; length.saturating_add(1)];
    if unsafe {
        // SAFETY: writable attribute buffer sized from the reported length.
        activate.GetString(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute, &mut chars, None)
    }
    .is_err()
    {
        return false;
    }
    let actual = String::from_utf16_lossy(
        &chars[..chars.iter().position(|&c| c == 0).unwrap_or(chars.len())],
    );
    actual.eq_ignore_ascii_case(&expected)
}

fn adapter_vendor_id(luid: i64) -> Option<u32> {
    crate::vproc::adapters()
        .ok()
        .and_then(|items| items.into_iter().find(|item| item.luid == luid))
        .map(|item| item.vendor_id)
}

pub fn list_encoders(luid: Option<i64>, codec: Codec) -> Result<Vec<MfEncoderInfo>, PlatformError> {
    mf_startup()?;
    let enumeration = enum_activates(luid, codec, luid.is_some()).map_err(platform)?;
    let mut activates = enumeration.items;
    let vendor_id = luid.and_then(adapter_vendor_id);
    if enumeration.used_fallback {
        // MFTEnumEx has no adapter-LUID filter. Resolve the adapter vendor from
        // DXGI and apply the documented VEN_xxxx attribute filter only on this
        // fallback path; MFTEnum2 already scoped successful results by LUID.
        if let Some(vendor_id) = vendor_id {
            activates.retain(|activate| activate_matches_vendor(activate, vendor_id));
        }
    }
    // If the fallback cannot identify the requested adapter, avoid returning
    // an unscoped hardware encoder from another adapter.
    if enumeration.used_fallback && vendor_id.is_none() && luid.is_some() {
        activates.clear();
    }
    let mut result = Vec::new();
    for activate in activates {
        let name = activate_name(&activate);
        let vendor = {
            let length = unsafe {
                // SAFETY: live attribute store.
                activate.GetStringLength(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute)
            }
            .unwrap_or(0) as usize;
            if length == 0 {
                String::new()
            } else {
                let mut chars = vec![0u16; length.saturating_add(1)];
                let _ = unsafe {
                    // SAFETY: writable attribute buffer.
                    activate.GetString(&MFT_ENUM_HARDWARE_VENDOR_ID_Attribute, &mut chars, None)
                };
                String::from_utf16_lossy(
                    &chars[..chars.iter().position(|&c| c == 0).unwrap_or(chars.len())],
                )
            }
        };
        let attributes = unsafe {
            // SAFETY: activation returns a new transform whose attributes are read-only here.
            activate.ActivateObject::<IMFTransform>()
        };
        let (is_async, d3d11_aware) = match attributes {
            Ok(transform) => {
                let attrs = unsafe {
                    // SAFETY: transform is a live COM interface and this is a read-only query.
                    transform.GetAttributes()
                }
                .ok();
                let values = attrs
                    .map(|attrs| unsafe {
                        // SAFETY: values are read-only attribute queries.
                        (
                            attrs.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0,
                            attrs.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0,
                        )
                    })
                    .unwrap_or((false, false));
                unsafe {
                    // SAFETY: shutting down the temporary activated object.
                    activate.ShutdownObject().ok();
                }
                values
            }
            Err(_) => {
                unsafe {
                    // SAFETY: the activation object is owned by this loop
                    // iteration and the temporary transform was not created.
                    activate.ShutdownObject().ok();
                }
                (false, false)
            }
        };
        result.push(MfEncoderInfo {
            name,
            vendor,
            is_async,
            d3d11_aware,
        });
    }
    Ok(result)
}

fn set_type_attrs(
    media_type: &IMFMediaType,
    codec: Codec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate: u64,
    output: bool,
) -> Result<(), MfError> {
    for attr in mf_type_attrs(codec, width, height, fps, bitrate, output) {
        let result = unsafe {
            // SAFETY: media_type is a live attribute store and values are plain integers.
            match attr {
                MfAttr::FrameSize(value) | MfAttr::FrameRate(value) => media_type.SetUINT64(
                    if matches!(attr, MfAttr::FrameSize(_)) {
                        &MF_MT_FRAME_SIZE
                    } else {
                        &MF_MT_FRAME_RATE
                    },
                    value,
                ),
                MfAttr::AvgBitrate(value) => media_type.SetUINT32(&MF_MT_AVG_BITRATE, value),
                MfAttr::Interlace(value) => media_type.SetUINT32(&MF_MT_INTERLACE_MODE, value),
                MfAttr::Profile(value) => media_type.SetUINT32(&MF_MT_MPEG2_PROFILE, value),
                MfAttr::YuvMatrix(value) => media_type.SetUINT32(&MF_MT_YUV_MATRIX, value),
                MfAttr::NominalRange(value) => {
                    media_type.SetUINT32(&MF_MT_VIDEO_NOMINAL_RANGE, value)
                }
                MfAttr::Primaries(value) => media_type.SetUINT32(&MF_MT_VIDEO_PRIMARIES, value),
                MfAttr::Transfer(value) => media_type.SetUINT32(&MF_MT_TRANSFER_FUNCTION, value),
            }
        };
        result.map_err(|error| hr_failed("set media type attribute", &error))?;
    }
    Ok(())
}

fn create_nv12_texture(
    device: &ID3D11Device,
    width: u32,
    height: u32,
) -> Result<ID3D11Texture2D, MfError> {
    let mut desc = D3D11_TEXTURE2D_DESC {
        Width: width,
        Height: height,
        MipLevels: 1,
        ArraySize: 1,
        Format: DXGI_FORMAT_NV12,
        SampleDesc: DXGI_SAMPLE_DESC {
            Count: 1,
            Quality: 0,
        },
        Usage: D3D11_USAGE_DEFAULT,
        BindFlags: (D3D11_BIND_RENDER_TARGET | D3D11_BIND_SHADER_RESOURCE).0 as u32,
        CPUAccessFlags: 0,
        MiscFlags: 0,
    };
    let mut texture = None;
    let first_result = unsafe {
        // SAFETY: descriptor is fully initialized and output pointer is writable.
        device.CreateTexture2D(&desc, None, Some(&mut texture))
    };
    if first_result.is_err() {
        // Some drivers reject an NV12 texture carrying a shader-resource bind
        // flag even though the Video Processor only needs a render target. The
        // render-target-only descriptor is the documented fallback.
        desc.BindFlags = D3D11_BIND_RENDER_TARGET.0 as u32;
        texture = None;
        unsafe {
            // SAFETY: fallback descriptor is fully initialized and the output pointer is writable.
            device
                .CreateTexture2D(&desc, None, Some(&mut texture))
                .map_err(|error| hr_failed("CreateTexture2D", &error))?;
        }
    }
    texture.ok_or_else(|| unsupported("CreateTexture2D returned no texture"))
}

struct EventMsg {
    kind: MfEventKind,
    status: i32,
}

struct ActivateGuard(Option<IMFActivate>);

impl Drop for ActivateGuard {
    fn drop(&mut self) {
        if let Some(activate) = self.0.take() {
            unsafe {
                // SAFETY: the activation object is owned by this guard and has
                // not been transferred into a live MfEncoder.
                let _ = activate.ShutdownObject();
            }
        }
    }
}

pub struct MfEncoder {
    transform: IMFTransform,
    activate: IMFActivate,
    shutdown: Option<IMFShutdown>,
    manager: IMFDXGIDeviceManager,
    events: mpsc::Receiver<EventMsg>,
    event_thread: Option<JoinHandle<()>>,
    flow: MfFlow,
    ring: SurfaceRing,
    textures: Vec<ID3D11Texture2D>,
    name: String,
    fps: u32,
    bitrate_bps: u64,
    pending_restore: Option<u64>,
    output_provides_samples: bool,
    output_size: u32,
}

impl MfEncoder {
    pub fn open(req: &MfOpen<'_>) -> Result<MfEncoder, MfError> {
        mf_startup().map_err(|error| unsupported(error.to_string()))?;
        if req.width == 0 || req.height == 0 || req.fps == 0 {
            return Err(unsupported("invalid MF dimensions or fps"));
        }
        let enumeration = enum_activates(Some(req.adapter_luid), req.codec, true)
            .map_err(|error| hr_failed("MFTEnum2", &error))?;
        let mut activates = enumeration.items;
        if enumeration.used_fallback {
            activates.retain(|activate| activate_matches_vendor(activate, req.vendor_id));
        }
        let activate = activates
            .into_iter()
            .next()
            .ok_or_else(|| unsupported(format!("no hardware {} MFT", codec_name(req.codec))))?;
        let mut activate_guard = ActivateGuard(Some(activate));
        let activate = activate_guard
            .0
            .as_ref()
            .ok_or_else(|| unsupported("missing MF activation object"))?;
        let name = activate_name(activate);
        let transform: IMFTransform = unsafe {
            // SAFETY: activation object is live and requests the documented transform interface.
            activate
                .ActivateObject()
                .map_err(|error| hr_failed("ActivateObject", &error))?
        };
        let attributes = unsafe {
            // SAFETY: transform is live and exposes its attribute store.
            transform
                .GetAttributes()
                .map_err(|error| hr_failed("GetAttributes", &error))?
        };
        let is_async = unsafe {
            // SAFETY: read-only attribute query.
            attributes.GetUINT32(&MF_TRANSFORM_ASYNC).unwrap_or(0) != 0
        };
        let d3d11_aware = unsafe {
            // SAFETY: read-only attribute query.
            attributes.GetUINT32(&MF_SA_D3D11_AWARE).unwrap_or(0) != 0
        };
        if !is_async || !d3d11_aware {
            return Err(unsupported("MFT is not asynchronous D3D11-aware"));
        }
        unsafe {
            // SAFETY: setting the documented async unlock attribute on a live MFT.
            attributes
                .SetUINT32(&MF_TRANSFORM_ASYNC_UNLOCK, 1)
                .map_err(|error| hr_failed("MF_TRANSFORM_ASYNC_UNLOCK", &error))?;
        }
        let mut token = 0u32;
        let manager = unsafe {
            // SAFETY: MF allocates a DXGI manager and writes the reset token.
            let mut manager = None;
            MFCreateDXGIDeviceManager(&mut token, &mut manager)
                .map_err(|error| hr_failed("MFCreateDXGIDeviceManager", &error))?;
            manager.ok_or_else(|| unsupported("missing DXGI device manager"))?
        };
        unsafe {
            // SAFETY: manager owns the D3D11 device reference for the MFT lifetime.
            manager
                .ResetDevice(req.device, token)
                .map_err(|error| hr_failed("ResetDevice", &error))?;
            transform
                .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, manager.as_raw() as usize)
                .map_err(|error| hr_failed("MFT_MESSAGE_SET_D3D_MANAGER", &error))?;
        }
        let codec_api: ICodecAPI = transform.cast().map_err(|_| unsupported("ICodecAPI"))?;
        for prop in mf_codecapi(req.bitrate_bps, req.fps) {
            if let Err(error) = set_codec_prop(&codec_api, prop) {
                if prop.required {
                    return Err(hr_failed("codecapi", &error));
                }
                debug!(api = ?prop.api, "optional MF codec property rejected");
            }
        }
        let subtype = codec_subtype(req.codec).ok_or_else(|| unsupported("unspecified codec"))?;
        let output_type = unsafe {
            // SAFETY: MF allocates a media type.
            MFCreateMediaType().map_err(|error| hr_failed("MFCreateMediaType", &error))?
        };
        unsafe {
            // SAFETY: media type attribute store is live.
            output_type
                .SetGUID(&MF_MT_MAJOR_TYPE, &MFMediaType_Video)
                .map_err(|error| hr_failed("MF_MT_MAJOR_TYPE", &error))?;
            output_type
                .SetGUID(&MF_MT_SUBTYPE, &subtype)
                .map_err(|error| hr_failed("MF_MT_SUBTYPE", &error))?;
        }
        set_type_attrs(
            &output_type,
            req.codec,
            req.width,
            req.height,
            req.fps,
            req.bitrate_bps,
            true,
        )?;
        unsafe {
            // SAFETY: output type is fully populated and owned by this call.
            transform
                .SetOutputType(0, &output_type, 0)
                .map_err(|error| hr_failed("SetOutputType", &error))?;
        }
        let mut input_type = None;
        for index in 0..32u32 {
            let candidate = unsafe {
                // SAFETY: transform is live; the stream and type index are bounded enumeration inputs.
                transform.GetInputAvailableType(0, index)
            };
            let Ok(candidate) = candidate else { break };
            let is_nv12 = unsafe {
                // SAFETY: candidate is a live media type and this only reads its subtype.
                candidate.GetGUID(&MF_MT_SUBTYPE).ok() == Some(MFVideoFormat_NV12)
            };
            if is_nv12 {
                set_type_attrs(
                    &candidate,
                    req.codec,
                    req.width,
                    req.height,
                    req.fps,
                    req.bitrate_bps,
                    false,
                )?;
                input_type = Some(candidate);
                break;
            }
        }
        let input_type = input_type.ok_or_else(|| unsupported("MFT has no NV12 input type"))?;
        unsafe {
            // SAFETY: selected input type is owned and fully described.
            transform
                .SetInputType(0, &input_type, 0)
                .map_err(|error| hr_failed("SetInputType", &error))?;
        }
        let output_info = unsafe {
            // SAFETY: stream 0 is the encoder output stream.
            transform
                .GetOutputStreamInfo(0)
                .map_err(|error| hr_failed("GetOutputStreamInfo", &error))?
        };
        let textures = (0..MF_INPUT_POOL)
            .map(|_| create_nv12_texture(req.device, req.width, req.height))
            .collect::<Result<Vec<_>, _>>()?;
        let ring = SurfaceRing::new(textures.len());
        let event_generator: IMFMediaEventGenerator = transform
            .cast()
            .map_err(|_| unsupported("IMFMediaEventGenerator"))?;
        unsafe {
            // SAFETY: live async transform receives stream lifecycle messages in order.
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_BEGIN_STREAMING, 0)
                .map_err(|error| hr_failed("BEGIN_STREAMING", &error))?;
            transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_START_OF_STREAM, 0)
                .map_err(|error| hr_failed("START_OF_STREAM", &error))?;
        }
        // The generated interface wrapper is intentionally !Send. The async MFT
        // event interface is free-threaded, so transfer one owned COM pointer as
        // an integer and reconstruct it on the MTA event thread.
        let event_generator_raw = event_generator.into_raw() as usize;
        let shutdown = transform.cast::<IMFShutdown>().ok();
        let (event_tx, event_rx) = mpsc::channel();
        let event_thread =
            match thread::Builder::new()
                .name("rs-mf-events".into())
                .spawn(move || {
                    unsafe {
                        // SAFETY: this thread owns its MTA initialization and the
                        // event interface is free-threaded for async MFTs.
                        let _ = CoInitializeEx(None, COINIT_MULTITHREADED);
                    }
                    let event_generator = unsafe {
                        // SAFETY: `event_generator_raw` is the owned pointer moved
                        // from the opening thread; this thread takes its one ref
                        // after entering its MTA apartment.
                        IMFMediaEventGenerator::from_raw(event_generator_raw as *mut _)
                    };
                    loop {
                        let event = unsafe {
                            // SAFETY: the event interface is owned by this MTA thread and GetEvent is blocking.
                            event_generator.GetEvent(MEDIA_EVENT_GENERATOR_GET_EVENT_FLAGS(0))
                        };
                        let Ok(event) = event else { break };
                        let kind = unsafe {
                            // SAFETY: event is a live IMFMediaEvent returned by GetEvent.
                            event.GetType()
                        }
                        .map(mf_event_kind)
                        .unwrap_or(MfEventKind::Other);
                        let status = unsafe {
                            // SAFETY: event is a live IMFMediaEvent returned by GetEvent.
                            event.GetStatus()
                        }
                        .map(|hr| hr.0)
                        .unwrap_or(E_FAIL.0);
                        if event_tx.send(EventMsg { kind, status }).is_err() {
                            break;
                        }
                    }
                }) {
                Ok(thread) => thread,
                Err(error) => {
                    unsafe {
                        // SAFETY: the event thread did not take ownership, so the
                        // raw interface reference is still owned by this path.
                        drop(IMFMediaEventGenerator::from_raw(
                            event_generator_raw as *mut _,
                        ));
                    }
                    return Err(unsupported(format!("event thread: {error}")));
                }
            };
        let activate = activate_guard
            .0
            .take()
            .ok_or_else(|| unsupported("missing MF activation object"))?;
        Ok(Self {
            transform,
            activate,
            shutdown,
            manager,
            events: event_rx,
            event_thread: Some(event_thread),
            flow: MfFlow::new(),
            ring,
            textures,
            name,
            fps: req.fps,
            bitrate_bps: req.bitrate_bps,
            pending_restore: None,
            output_provides_samples: output_info.dwFlags
                & MFT_OUTPUT_STREAM_PROVIDES_SAMPLES.0 as u32
                != 0,
            output_size: output_info.cbSize,
        })
    }

    pub fn name(&self) -> &str {
        &self.name
    }

    pub fn input_slot(&self) -> Option<(usize, ID3D11Texture2D)> {
        self.ring
            .acquire()
            .map(|slot| (slot, self.textures[slot].clone()))
    }

    fn receive_events(&mut self) -> Result<(), MfError> {
        while let Ok(event) = self.events.try_recv() {
            if event.kind == MfEventKind::Error || event.status < 0 {
                return Err(MfError::Failed {
                    op: "mf event",
                    hr: event.status,
                });
            }
            self.flow.on_event(event.kind);
        }
        Ok(())
    }

    fn set_props(&self, props: &[MfProp]) -> Result<(), MfError> {
        let api: ICodecAPI = self
            .transform
            .cast()
            .map_err(|_| unsupported("ICodecAPI"))?;
        for prop in props {
            if let Err(error) = set_codec_prop(&api, *prop) {
                if prop.required {
                    return Err(hr_failed("codecapi", &error));
                }
                debug!(api = ?prop.api, "optional MF codec property rejected");
            }
        }
        Ok(())
    }

    fn drain_announced_output(&mut self) -> Result<Vec<MfPacket>, MfError> {
        let mut packets = Vec::new();
        while self.flow.take_output() {
            packets.extend(self.take_output()?);
        }
        Ok(packets)
    }

    fn take_output(&mut self) -> Result<Vec<MfPacket>, MfError> {
        let mut packets = Vec::new();
        loop {
            let sample = if self.output_provides_samples {
                None
            } else {
                let sample = unsafe {
                    // SAFETY: MFCreateSample writes one owned sample interface.
                    MFCreateSample().map_err(|error| hr_failed("MFCreateSample", &error))?
                };
                let buffer = unsafe {
                    // SAFETY: MF allocates a writable output buffer of the stream's advertised size.
                    MFCreateMemoryBuffer(self.output_size)
                        .map_err(|error| hr_failed("MFCreateMemoryBuffer", &error))?
                };
                unsafe {
                    // SAFETY: sample and buffer are live interfaces owned by this output call.
                    sample
                        .AddBuffer(&buffer)
                        .map_err(|error| hr_failed("AddBuffer(output)", &error))?;
                }
                Some(sample)
            };
            let mut output = MFT_OUTPUT_DATA_BUFFER {
                dwStreamID: 0,
                pSample: ManuallyDrop::new(sample),
                dwStatus: 0,
                pEvents: ManuallyDrop::new(None),
            };
            let mut status = 0u32;
            let result = unsafe {
                // SAFETY: output points to one initialized MFT_OUTPUT_DATA_BUFFER;
                // ManuallyDrop fields are released below after the call.
                self.transform
                    .ProcessOutput(0, std::slice::from_mut(&mut output), &mut status)
            };
            if let Err(error) = result {
                let code = error.code().0;
                unsafe {
                    // SAFETY: ProcessOutput has returned and no longer owns these fields.
                    ManuallyDrop::drop(&mut output.pSample);
                    ManuallyDrop::drop(&mut output.pEvents);
                }
                if code == MF_E_TRANSFORM_NEED_MORE_INPUT.0 {
                    break;
                }
                if code == MF_E_TRANSFORM_STREAM_CHANGE.0 {
                    if let Ok(new_type) = unsafe {
                        // SAFETY: transform is live and stream 0/type 0 are the documented output query.
                        self.transform.GetOutputAvailableType(0, 0)
                    } {
                        unsafe {
                            // SAFETY: the new output type is a live media type returned by MF.
                            self.transform
                                .SetOutputType(0, &new_type, 0)
                                .map_err(|e| hr_failed("SetOutputType(stream change)", &e))?;
                        }
                        continue;
                    }
                }
                return Err(hr_failed("ProcessOutput", &error));
            }
            let sample = unsafe {
                // SAFETY: ProcessOutput has returned; ownership of pSample is ours.
                ManuallyDrop::take(&mut output.pSample)
            };
            let _events = unsafe {
                // SAFETY: ProcessOutput has returned; ownership of pEvents is ours.
                ManuallyDrop::take(&mut output.pEvents)
            };
            if let Some(sample) = sample {
                let buffer = unsafe {
                    // SAFETY: sample is a live output sample and the call returns an owned buffer.
                    sample
                        .ConvertToContiguousBuffer()
                        .map_err(|error| hr_failed("ConvertToContiguousBuffer", &error))?
                };
                let mut data = ptr::null_mut();
                let mut max = 0u32;
                let mut current = 0u32;
                unsafe {
                    // SAFETY: buffer is locked only for the duration of the copy.
                    buffer
                        .Lock(&mut data, Some(&mut max), Some(&mut current))
                        .map_err(|error| hr_failed("Lock output", &error))?;
                    let bytes = std::slice::from_raw_parts(data, current as usize).to_vec();
                    buffer
                        .Unlock()
                        .map_err(|error| hr_failed("Unlock output", &error))?;
                    let time = sample.GetSampleTime().unwrap_or(0);
                    let keyframe =
                        sample.GetUINT32(&MFSampleExtension_CleanPoint).unwrap_or(0) == 1;
                    let pts = pts_from_sample_time(time, self.fps);
                    self.ring.released_through(pts);
                    packets.push(MfPacket {
                        data: bytes,
                        pts,
                        keyframe,
                    });
                }
            }
            if output.dwStatus & MFT_OUTPUT_DATA_BUFFER_INCOMPLETE.0 as u32 == 0 {
                break;
            }
        }
        Ok(packets)
    }

    pub fn encode(
        &mut self,
        slot: usize,
        pts: i64,
        idr: bool,
        bump_bps: Option<u64>,
    ) -> Result<Vec<MfPacket>, MfError> {
        self.receive_events()?;
        if let Some(bps) = bump_bps {
            self.set_props(&mf_rate_props(bps, self.fps))?;
            self.pending_restore = Some(self.bitrate_bps);
        } else if let Some(bps) = self.pending_restore.take() {
            self.set_props(&mf_rate_props(bps, self.fps))?;
        }
        if idr {
            self.set_props(&[crate::vpmf_model::mf_force_key()])?;
        }
        let start = Instant::now();
        let mut packets = Vec::new();
        while !self.flow.take_input() {
            self.receive_events()?;
            packets.extend(self.drain_announced_output()?);
            if start.elapsed() >= Duration::from_micros(MF_ENCODE_WAIT_US) {
                return Err(MfError::Busy);
            }
            thread::yield_now();
        }
        if slot >= self.textures.len() {
            return Err(unsupported("input slot out of range"));
        }
        let sample = unsafe {
            // SAFETY: MFCreateSample writes one owned sample interface.
            MFCreateSample().map_err(|error| hr_failed("MFCreateSample", &error))?
        };
        let buffer = unsafe {
            // SAFETY: texture is a live D3D11 resource and MF receives a zero-copy surface buffer.
            MFCreateDXGISurfaceBuffer(&ID3D11Texture2D::IID, &self.textures[slot], 0, false)
                .map_err(|error| hr_failed("MFCreateDXGISurfaceBuffer", &error))?
        };
        unsafe {
            // SAFETY: sample and buffer are live COM interfaces; timestamps use MF hns.
            sample
                .AddBuffer(&buffer)
                .map_err(|error| hr_failed("AddBuffer", &error))?;
            sample
                .SetSampleTime(sample_time(pts, self.fps))
                .map_err(|error| hr_failed("SetSampleTime", &error))?;
            sample
                .SetSampleDuration(hns_per_frame(self.fps))
                .map_err(|error| hr_failed("SetSampleDuration", &error))?;
            self.transform
                .ProcessInput(0, &sample, 0)
                .map_err(|error| hr_failed("ProcessInput", &error))?;
        }
        self.ring.submitted(slot, pts);
        packets.extend(self.take_output()?);
        while packets.iter().all(|packet| packet.pts < pts)
            && start.elapsed() < Duration::from_micros(MF_ENCODE_WAIT_US)
        {
            self.receive_events()?;
            packets.extend(self.take_output()?);
            if packets.iter().any(|packet| packet.pts >= pts) {
                break;
            }
            thread::yield_now();
        }
        trace!(pts, packet_count = packets.len(), "mf encode");
        Ok(packets)
    }

    pub fn poll(&mut self) -> Result<Vec<MfPacket>, MfError> {
        self.receive_events()?;
        let mut packets = self.drain_announced_output()?;
        packets.extend(self.take_output()?);
        Ok(packets)
    }

    pub fn set_bitrate(&mut self, bps: u64) -> Result<(), MfError> {
        self.set_props(&mf_rate_props(bps, self.fps))?;
        self.bitrate_bps = bps;
        self.pending_restore = None;
        Ok(())
    }
}

impl Drop for MfEncoder {
    fn drop(&mut self) {
        let _ = self.manager.as_raw();
        unsafe {
            // SAFETY: stream lifecycle messages are best-effort during drop.
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_OF_STREAM, 0);
            let _ = self.transform.ProcessMessage(MFT_MESSAGE_COMMAND_FLUSH, 0);
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_NOTIFY_END_STREAMING, 0);
            if let Some(shutdown) = &self.shutdown {
                let _ = shutdown.Shutdown();
            }
            let _ = self
                .transform
                .ProcessMessage(MFT_MESSAGE_SET_D3D_MANAGER, 0);
            let _ = self.activate.ShutdownObject();
        }
        if let Some(thread) = self.event_thread.take() {
            // Shutdown wakes GetEvent; keep teardown bounded if a vendor MFT
            // violates that contract and leaves its event call blocked.
            let (done_tx, done_rx) = mpsc::channel();
            let _ = thread::Builder::new()
                .name("rs-mf-join".into())
                .spawn(move || {
                    let _ = thread.join();
                    let _ = done_tx.send(());
                });
            if done_rx.recv_timeout(Duration::from_millis(200)).is_err() {
                warn!("MF event thread did not stop within 200 ms; detaching");
            }
        }
    }
}
