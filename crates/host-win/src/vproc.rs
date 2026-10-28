#![cfg(windows)]
//! D3D11 Video Processor conversion used by both FFmpeg and MF paths.
//!
//! The encoder creates one processor for each `VpKey { device_gen, in = native
//! texture size, out = encoder size, rotation }`. It calls [`VideoProc::blit`]
//! and the subsequent encoder operation on the same device immediate context,
//! then releases the `CapturedFrame`; ordered GPU commands provide the needed
//! synchronization. FFmpeg writes to its input texture/array slice, while the
//! MF path writes to the texture returned by `MfEncoder::input_slot` (and drops
//! the frame when that method returns `None`).

use std::mem::ManuallyDrop;

use host_core::platform::PlatformError;
use tracing::warn;
use windows::{
    core::Interface,
    Win32::{
        Foundation::RECT,
        Graphics::{
            Direct3D::{
                D3D_DRIVER_TYPE_UNKNOWN, D3D_FEATURE_LEVEL_10_0, D3D_FEATURE_LEVEL_10_1,
                D3D_FEATURE_LEVEL_11_0, D3D_FEATURE_LEVEL_11_1,
            },
            Direct3D11::*,
            Dxgi::{
                Common::{DXGI_FORMAT_B8G8R8A8_UNORM, DXGI_FORMAT_NV12, DXGI_RATIONAL},
                CreateDXGIFactory1, IDXGIAdapter, IDXGIAdapter1, IDXGIFactory1,
                DXGI_ADAPTER_FLAG_SOFTWARE,
            },
        },
    },
};

use crate::vpmf_model::{
    fit_rect, legacy_color_space, rotated_size, vp_rotation, vp_rotation_value, RectU, VpKey,
    VpRotation, BG_YCBCRA, VIEW_CACHE_MAX, VP_INPUT_COLOR_SPACE, VP_INPUT_LEGACY,
    VP_OUTPUT_COLOR_SPACE, VP_OUTPUT_LEGACY,
};

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AdapterInfo {
    pub index: u32,
    pub description: String,
    pub vendor_id: u32,
    pub luid: i64,
    pub has_output: bool,
}

fn platform(error: impl std::fmt::Display) -> PlatformError {
    PlatformError::Platform(error.to_string())
}

fn unsupported(message: impl Into<String>) -> PlatformError {
    PlatformError::Platform(format!("unsupported: {}", message.into()))
}

fn description(value: &[u16]) -> String {
    let end = value
        .iter()
        .position(|&unit| unit == 0)
        .unwrap_or(value.len());
    String::from_utf16_lossy(&value[..end])
}

fn adapter_luid(desc: &windows::Win32::Graphics::Dxgi::DXGI_ADAPTER_DESC1) -> i64 {
    (i64::from(desc.AdapterLuid.HighPart) << 32) | i64::from(desc.AdapterLuid.LowPart)
}

fn adapters_with_handles() -> windows::core::Result<Vec<(AdapterInfo, IDXGIAdapter1)>> {
    let factory: IDXGIFactory1 = unsafe {
        // SAFETY: `CreateDXGIFactory1` writes one owned factory interface.
        CreateDXGIFactory1()?
    };
    let mut result = Vec::new();
    let mut index = 0;
    loop {
        let adapter = match unsafe {
            // SAFETY: the factory is live and enumeration owns the returned interface.
            factory.EnumAdapters1(index)
        } {
            Ok(adapter) => adapter,
            Err(_) => break,
        };
        let desc = unsafe {
            // SAFETY: adapter is a live DXGI interface and returns an owned value.
            adapter.GetDesc1()?
        };
        index += 1;
        if desc.Flags & DXGI_ADAPTER_FLAG_SOFTWARE.0 as u32 != 0 {
            continue;
        }
        let has_output = unsafe {
            // SAFETY: probing output 0 only reads topology state from a live adapter.
            adapter
                .EnumOutputs(0)
                .and_then(|output| {
                    output
                        .GetDesc()
                        .map(|desc| desc.AttachedToDesktop.as_bool())
                })
                .unwrap_or(false)
        };
        result.push((
            AdapterInfo {
                index: index - 1,
                description: description(&desc.Description),
                vendor_id: desc.VendorId,
                luid: adapter_luid(&desc),
                has_output,
            },
            adapter,
        ));
    }
    Ok(result)
}

pub fn adapters() -> Result<Vec<AdapterInfo>, PlatformError> {
    adapters_with_handles()
        .map(|items| items.into_iter().map(|(info, _)| info).collect())
        .map_err(platform)
}

pub fn create_device(luid: i64) -> Result<ID3D11Device, PlatformError> {
    let adapter = adapters_with_handles()
        .map_err(platform)?
        .into_iter()
        .find(|(info, _)| info.luid == luid)
        .map(|(_, adapter)| adapter)
        .ok_or_else(|| platform("adapter not found"))?;
    let adapter: IDXGIAdapter = adapter.cast().map_err(platform)?;
    let mut device = None;
    let mut context = None;
    let levels = [
        D3D_FEATURE_LEVEL_11_1,
        D3D_FEATURE_LEVEL_11_0,
        D3D_FEATURE_LEVEL_10_1,
        D3D_FEATURE_LEVEL_10_0,
    ];
    unsafe {
        // SAFETY: adapter and output pointers are live for this synchronous call;
        // VIDEO_SUPPORT is deliberately required and there is no software retry.
        D3D11CreateDevice(
            Some(&adapter),
            D3D_DRIVER_TYPE_UNKNOWN,
            Default::default(),
            D3D11_CREATE_DEVICE_BGRA_SUPPORT | D3D11_CREATE_DEVICE_VIDEO_SUPPORT,
            Some(&levels),
            D3D11_SDK_VERSION,
            Some(&mut device),
            None,
            Some(&mut context),
        )
        .map_err(platform)?;
    }
    let device = device.ok_or_else(|| platform("D3D11CreateDevice returned no device"))?;
    let _context = context.ok_or_else(|| platform("D3D11CreateDevice returned no context"))?;
    let multithread: ID3D11Multithread = device.cast().map_err(platform)?;
    unsafe {
        // SAFETY: `multithread` is queried from the live D3D11 device.
        let _ = multithread.SetMultithreadProtected(true);
    }
    Ok(device)
}

fn to_rect(rect: RectU) -> RECT {
    RECT {
        left: rect.left.min(i32::MAX as u32) as i32,
        top: rect.top.min(i32::MAX as u32) as i32,
        right: rect.right.min(i32::MAX as u32) as i32,
        bottom: rect.bottom.min(i32::MAX as u32) as i32,
    }
}

fn desc_for_input() -> D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
    D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC {
        FourCC: 0,
        ViewDimension: D3D11_VPIV_DIMENSION_TEXTURE2D,
        Anonymous: D3D11_VIDEO_PROCESSOR_INPUT_VIEW_DESC_0 {
            Texture2D: D3D11_TEX2D_VPIV {
                MipSlice: 0,
                ArraySlice: 0,
            },
        },
    }
}

fn desc_for_output(array_size: u32, array_index: u32) -> D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
    if array_size <= 1 {
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2D,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2D: D3D11_TEX2D_VPOV { MipSlice: 0 },
            },
        }
    } else {
        D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC {
            ViewDimension: D3D11_VPOV_DIMENSION_TEXTURE2DARRAY,
            Anonymous: D3D11_VIDEO_PROCESSOR_OUTPUT_VIEW_DESC_0 {
                Texture2DArray: D3D11_TEX2D_ARRAY_VPOV {
                    MipSlice: 0,
                    FirstArraySlice: array_index,
                    ArraySize: 1,
                },
            },
        }
    }
}

pub struct VideoProc {
    key: VpKey,
    rotation_supported: bool,
    device: ID3D11VideoDevice,
    context: ID3D11VideoContext,
    enumerator: ID3D11VideoProcessorEnumerator,
    processor: ID3D11VideoProcessor,
    input_views:
        crate::vpmf_model::ViewCache<usize, (ID3D11Texture2D, ID3D11VideoProcessorInputView)>,
    output_views: crate::vpmf_model::ViewCache<
        (usize, u32),
        (ID3D11Texture2D, ID3D11VideoProcessorOutputView),
    >,
}

impl VideoProc {
    pub fn new(device: &ID3D11Device, key: VpKey, fps: u32) -> Result<VideoProc, PlatformError> {
        let video_device: ID3D11VideoDevice = device
            .cast()
            .map_err(|_| unsupported("D3D11 VIDEO_SUPPORT"))?;
        let context = unsafe {
            // SAFETY: the device is live and owns its immediate context.
            device.GetImmediateContext()
        }
        .map_err(platform)?;
        let video_context: ID3D11VideoContext = context
            .cast()
            .map_err(|_| unsupported("D3D11 video context"))?;
        let context1 = context.cast::<ID3D11VideoContext1>().ok();
        let fps = fps.max(1);
        let content = D3D11_VIDEO_PROCESSOR_CONTENT_DESC {
            InputFrameFormat: D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            InputFrameRate: DXGI_RATIONAL {
                Numerator: fps,
                Denominator: 1,
            },
            InputWidth: key.in_w,
            InputHeight: key.in_h,
            OutputFrameRate: DXGI_RATIONAL {
                Numerator: fps,
                Denominator: 1,
            },
            OutputWidth: key.out_w,
            OutputHeight: key.out_h,
            Usage: D3D11_VIDEO_USAGE_OPTIMAL_SPEED,
        };
        let enumerator = unsafe {
            // SAFETY: content is a fully initialized descriptor owned by this call.
            video_device.CreateVideoProcessorEnumerator(&content)
        }
        .map_err(platform)?;
        let input_support = unsafe {
            // SAFETY: enumerator is live and the format value is from DXGI.
            enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_B8G8R8A8_UNORM)
        }
        .map_err(platform)?;
        let output_support = unsafe {
            // SAFETY: enumerator is live and the format value is from DXGI.
            enumerator.CheckVideoProcessorFormat(DXGI_FORMAT_NV12)
        }
        .map_err(platform)?;
        if input_support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_INPUT.0 as u32 == 0 {
            return Err(unsupported("BGRA Video Processor input"));
        }
        if output_support & D3D11_VIDEO_PROCESSOR_FORMAT_SUPPORT_OUTPUT.0 as u32 == 0 {
            return Err(unsupported("NV12 Video Processor output"));
        }
        let mut caps = D3D11_VIDEO_PROCESSOR_CAPS::default();
        unsafe {
            // SAFETY: caps points to writable storage and enumerator is live.
            enumerator.GetVideoProcessorCaps(&mut caps)
        }
        .map_err(platform)?;
        let rotation_supported =
            caps.FeatureCaps & D3D11_VIDEO_PROCESSOR_FEATURE_CAPS_ROTATION.0 as u32 != 0;
        let processor = unsafe {
            // SAFETY: enumerator is live and processor index 0 is valid after caps query.
            video_device.CreateVideoProcessor(&enumerator, 0)
        }
        .map_err(platform)?;
        let effective_rotation = if rotation_supported { key.rotation } else { 0 };
        if !rotation_supported && !matches!(vp_rotation(key.rotation), VpRotation::Identity) {
            warn!(
                rotation = key.rotation,
                "D3D11 Video Processor does not support rotation; using identity"
            );
        }
        let input_rect = to_rect(RectU {
            left: 0,
            top: 0,
            right: key.in_w,
            bottom: key.in_h,
        });
        let (rotated_w, rotated_h) = rotated_size(key.in_w, key.in_h, effective_rotation);
        let target_rect = to_rect(fit_rect(rotated_w, rotated_h, key.out_w, key.out_h));
        let output_rect = to_rect(RectU {
            left: 0,
            top: 0,
            right: key.out_w,
            bottom: key.out_h,
        });
        let color = D3D11_VIDEO_COLOR {
            Anonymous: D3D11_VIDEO_COLOR_0 {
                YCbCr: D3D11_VIDEO_COLOR_YCbCrA {
                    Y: BG_YCBCRA[0],
                    Cb: BG_YCBCRA[1],
                    Cr: BG_YCBCRA[2],
                    A: BG_YCBCRA[3],
                },
            },
        };
        unsafe {
            // SAFETY: all pointers reference stack values for the duration of each
            // synchronous setter; `processor` and `video_context` are live COM objects.
            video_context.VideoProcessorSetStreamFrameFormat(
                &processor,
                0,
                D3D11_VIDEO_FRAME_FORMAT_PROGRESSIVE,
            );
            video_context.VideoProcessorSetStreamAutoProcessingMode(&processor, 0, false);
            video_context.VideoProcessorSetStreamSourceRect(&processor, 0, true, Some(&input_rect));
            if rotation_supported {
                video_context.VideoProcessorSetStreamRotation(
                    &processor,
                    0,
                    !matches!(vp_rotation(effective_rotation), VpRotation::Identity),
                    D3D11_VIDEO_PROCESSOR_ROTATION(vp_rotation_value(vp_rotation(
                        effective_rotation,
                    ))),
                );
            }
            video_context.VideoProcessorSetStreamDestRect(&processor, 0, true, Some(&target_rect));
            video_context.VideoProcessorSetOutputTargetRect(&processor, true, Some(&output_rect));
            video_context.VideoProcessorSetOutputBackgroundColor(&processor, true, &color);
            if let Some(context1) = &context1 {
                context1.VideoProcessorSetStreamColorSpace1(
                    &processor,
                    0,
                    windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_RGB_FULL_G22_NONE_P709,
                );
                context1.VideoProcessorSetOutputColorSpace1(
                    &processor,
                    windows::Win32::Graphics::Dxgi::Common::DXGI_COLOR_SPACE_YCBCR_STUDIO_G22_LEFT_P709,
                );
            } else {
                let input_color = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                    _bitfield: VP_INPUT_LEGACY,
                };
                let output_color = D3D11_VIDEO_PROCESSOR_COLOR_SPACE {
                    _bitfield: VP_OUTPUT_LEGACY,
                };
                video_context.VideoProcessorSetStreamColorSpace(&processor, 0, &input_color);
                video_context.VideoProcessorSetOutputColorSpace(&processor, &output_color);
            }
        }
        // Keep the pure constants referenced here as a guard against changing
        // the Windows SDK values independently of the model module.
        let _ = (
            legacy_color_space(true, true, 0),
            VP_INPUT_COLOR_SPACE,
            VP_OUTPUT_COLOR_SPACE,
        );
        Ok(Self {
            key,
            rotation_supported,
            device: video_device,
            context: video_context,
            enumerator,
            processor,
            input_views: crate::vpmf_model::ViewCache::new(VIEW_CACHE_MAX),
            output_views: crate::vpmf_model::ViewCache::new(8),
        })
    }

    pub fn key(&self) -> VpKey {
        self.key
    }

    pub fn rotation_supported(&self) -> bool {
        self.rotation_supported
    }

    pub fn blit(
        &mut self,
        src: &ID3D11Texture2D,
        dst: &ID3D11Texture2D,
        dst_array_index: u32,
    ) -> Result<(), PlatformError> {
        let mut src_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            // SAFETY: `src` is a live texture owned by the caller.
            src.GetDesc(&mut src_desc);
        }
        if src_desc.Width != self.key.in_w || src_desc.Height != self.key.in_h {
            return Err(platform("vp input size changed"));
        }
        if src_desc.Format != DXGI_FORMAT_B8G8R8A8_UNORM {
            return Err(platform("vp input format changed"));
        }
        let input_key = src.as_raw() as usize;
        let input_view = {
            let device = self.device.clone();
            let enumerator = self.enumerator.clone();
            self.input_views
                .get_or_try_insert(input_key, || {
                    let desc = desc_for_input();
                    let mut view = None;
                    unsafe {
                        // SAFETY: source texture, enumerator and output pointer are live;
                        // descriptor selects mip 0 / array slice 0.
                        device.CreateVideoProcessorInputView(
                            src,
                            &enumerator,
                            &desc,
                            Some(&mut view),
                        )
                    }
                    .map_err(platform)?;
                    Ok::<_, PlatformError>((
                        src.clone(),
                        view.ok_or_else(|| platform("missing input view"))?,
                    ))
                })?
                .1
                .clone()
        };
        let mut dst_desc = D3D11_TEXTURE2D_DESC::default();
        unsafe {
            // SAFETY: `dst` is a live texture owned by the caller.
            dst.GetDesc(&mut dst_desc);
        }
        if dst_desc.Format != DXGI_FORMAT_NV12
            || dst_desc.Width != self.key.out_w
            || dst_desc.Height != self.key.out_h
        {
            return Err(platform("vp output texture changed"));
        }
        if dst_array_index >= dst_desc.ArraySize.max(1) {
            return Err(platform("vp output array index out of range"));
        }
        let output_key = (dst.as_raw() as usize, dst_array_index);
        let output_view = {
            let device = self.device.clone();
            let enumerator = self.enumerator.clone();
            self.output_views
                .get_or_try_insert(output_key, || {
                    let desc = desc_for_output(dst_desc.ArraySize, dst_array_index);
                    let mut view = None;
                    unsafe {
                        // SAFETY: destination texture, enumerator and output pointer are
                        // live; descriptor selects one mip and one array slice.
                        device.CreateVideoProcessorOutputView(
                            dst,
                            &enumerator,
                            &desc,
                            Some(&mut view),
                        )
                    }
                    .map_err(platform)?;
                    Ok::<_, PlatformError>((
                        dst.clone(),
                        view.ok_or_else(|| platform("missing output view"))?,
                    ))
                })?
                .1
                .clone()
        };
        let mut stream = D3D11_VIDEO_PROCESSOR_STREAM {
            Enable: true.into(),
            pInputSurface: ManuallyDrop::new(Some(input_view)),
            ..Default::default()
        };
        let result = unsafe {
            // SAFETY: processor, output view and stream are live for this ordered
            // immediate-context submission. The stream's ManuallyDrop field is
            // explicitly dropped immediately below after VideoProcessorBlt.
            self.context.VideoProcessorBlt(
                &self.processor,
                &output_view,
                0,
                std::slice::from_ref(&stream),
            )
        }
        .map_err(platform);
        unsafe {
            // SAFETY: VideoProcessorBlt has synchronously consumed the stream
            // descriptor; this releases the COM input-view reference exactly once.
            ManuallyDrop::drop(&mut stream.pInputSurface);
        }
        result
    }
}
