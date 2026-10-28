//! Capture -> platform encoder -> QUIC datagrams or TCP whole frames.

use std::path::PathBuf;
use std::sync::{Arc, Mutex};

use rsp_core::{pb, stats::HostStatsCollector};
use tokio::sync::{mpsc, watch};
use tokio::task::JoinHandle;

use crate::platform::{
    CaptureConfig, CaptureHandle, DisplayInfo, EncoderConfig, PixelRect, Platform, PlatformError,
    Result, RoiRegion,
};
use policy::CodecFallback;

pub mod capture;
pub mod diagnostics;
pub mod encoder;
pub mod packetizer;
pub mod policy;
pub mod ratectl;
pub mod viewport;
pub mod viewport_geom;

/// 视口裁剪：只要显示器的 rect（原生像素），编码成 out_w × out_h（03 §5.5）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct CropPlan {
    pub rect: PixelRect,
    pub out_w: u32,
    pub out_h: u32,
}

/// Owns every producer and task in one stream, including cancellation.
pub struct VideoPipeline {
    pub width: u32,
    pub height: u32,
    pub fps: u16,
    _capture: Box<dyn CaptureHandle>,
    encoder: JoinHandle<()>,
    sender: JoinHandle<()>,
    keyframes: watch::Sender<u64>,
    rates: mpsc::Sender<(u32, u32)>,
    fec: watch::Sender<f32>,
    roi: watch::Sender<Vec<RoiRegion>>,
    roi_supported: bool,
    stats: Arc<Mutex<HostStatsCollector>>,
    codec: pb::Codec,
    tcp: bool,
    crop: Option<CropPlan>,
}

impl VideoPipeline {
    #[allow(clippy::too_many_arguments)]
    pub fn start<P: Platform>(
        platform: &P,
        display: &DisplayInfo,
        header_display_id: u8,
        conn: rsp_transport::Conn,
        epoch: u8,
        max_width: u32,
        fps: u16,
        bitrate_kbps: u32,
        codecs: &[pb::Codec],
        hq_refresh: bool,
        dump: Option<PathBuf>,
    ) -> Result<Self> {
        Self::start_with_crop(
            platform,
            display,
            header_display_id,
            conn,
            epoch,
            max_width,
            fps,
            bitrate_kbps,
            codecs,
            hq_refresh,
            dump,
            None,
        )
    }

    #[allow(clippy::too_many_arguments)]
    pub fn start_with_crop<P: Platform>(
        platform: &P,
        display: &DisplayInfo,
        header_display_id: u8,
        conn: rsp_transport::Conn,
        epoch: u8,
        max_width: u32,
        fps: u16,
        bitrate_kbps: u32,
        codecs: &[pb::Codec],
        hq_refresh: bool,
        dump: Option<PathBuf>,
        crop: Option<CropPlan>,
    ) -> Result<Self> {
        diagnostics::add(diagnostics::Counter::Starts, 1);
        let tcp = conn.kind() == rsp_transport::TransportKind::Tcp;
        let fps = if tcp { fps.min(30) } else { fps };
        let bitrate_kbps = if tcp {
            bitrate_kbps.clamp(2_000, 6_000)
        } else {
            bitrate_kbps
        };
        let crop = match crop {
            Some(plan) => {
                if !platform.capture_region_supported() {
                    return Err(PlatformError::Unsupported);
                }
                let rect_right = plan.rect.x.checked_add(plan.rect.w);
                let rect_bottom = plan.rect.y.checked_add(plan.rect.h);
                let valid_rect = plan.rect.w >= 2
                    && plan.rect.h >= 2
                    && rect_right.is_some_and(|right| right <= display.width_px)
                    && rect_bottom.is_some_and(|bottom| bottom <= display.height_px);
                let valid_output = plan.out_w >= 2
                    && plan.out_h >= 2
                    && plan.out_w % 2 == 0
                    && plan.out_h % 2 == 0;
                if !valid_rect || !valid_output {
                    return Err(PlatformError::Platform("invalid crop plan".into()));
                }
                let (width, height) = if tcp && plan.out_h > 1080 {
                    let width = (u64::from(plan.out_w) * 1080 / u64::from(plan.out_h)) as u32 & !1;
                    (width.max(2), 1080)
                } else {
                    (plan.out_w, plan.out_h)
                };
                Some(CropPlan {
                    rect: plan.rect,
                    out_w: width,
                    out_h: height,
                })
            }
            None => None,
        };
        let (width, height) = match crop {
            Some(plan) => (plan.out_w, plan.out_h),
            None => capture::dimensions(display, max_width, if tcp { 1080 } else { 0 })?,
        };
        let (frame_tx, frame_rx) = mpsc::channel(4);
        let (encoded_tx, encoded_rx) = mpsc::channel(4);
        let (keyframes, key_rx) = watch::channel(0u64);
        let (encoded_keyframes, encoded_key_rx) = watch::channel(None);
        let (repairs, repair_rx) = watch::channel(None);
        let (roi, roi_rx) = watch::channel(Vec::new());
        let (rates, rate_rx) = mpsc::channel(4);
        let (fec, fec_rx) = watch::channel(if tcp {
            0.0
        } else {
            rsp_core::fec::redundancy_ratio(0.0)
        });
        if tcp {
            let requests = keyframes.clone();
            let _ = conn.set_keyframe_request_callback(Arc::new(move || {
                requests.send_modify(|n| *n = n.wrapping_add(1));
            }));
        }
        let stats = Arc::new(Mutex::new(HostStatsCollector::starting_at(
            rsp_core::clock::unix_micros(),
        )));
        let mut fallback = CodecFallback::new(codecs, &[]);
        let (codec, encoder) = loop {
            let codec = fallback.current().ok_or(PlatformError::Unsupported)?;
            match platform.encoder(EncoderConfig {
                codec,
                width,
                height,
                fps: u32::from(fps),
                bitrate_kbps,
                sink: encoded_tx.clone(),
            }) {
                Ok(encoder) => break (codec, encoder),
                Err(PlatformError::Unsupported) if fallback.on_unsupported().is_some() => {}
                Err(error) => return Err(error),
            }
        };
        let roi_supported = encoder.roi_supported();
        let capture = match crop {
            Some(plan) => platform.capture_region(
                display.id,
                plan.rect,
                CaptureConfig {
                    fps: u32::from(fps),
                    max_width: width,
                    max_height: height,
                },
                frame_tx,
            )?,
            None => platform.capture(
                display.id,
                CaptureConfig {
                    fps: u32::from(fps),
                    max_width: width,
                    max_height: height,
                },
                frame_tx,
            )?,
        };
        Ok(Self {
            width,
            height,
            fps,
            _capture: capture,
            encoder: tokio::spawn(encoder::run(
                encoder,
                encoded_tx,
                frame_rx,
                key_rx,
                rate_rx,
                encoded_key_rx,
                repair_rx,
                roi_rx,
                hq_refresh,
                Arc::clone(&stats),
            )),
            sender: tokio::spawn(packetizer::run(
                conn,
                encoded_rx,
                dump,
                header_display_id,
                epoch,
                fec_rx,
                encoded_keyframes,
                repairs,
                Arc::clone(&stats),
            )),
            keyframes,
            rates,
            fec,
            roi,
            roi_supported,
            stats,
            codec,
            tcp,
            crop,
        })
    }

    /// 实际生效的裁剪（out_w/out_h 为最终编码尺寸）；整屏 → None。
    pub fn crop(&self) -> Option<CropPlan> {
        self.crop
    }

    /// 打开编码器时记下的 `Encoder::roi_supported()`。
    pub fn roi_supported(&self) -> bool {
        self.roi_supported
    }

    /// 替换 ROI 列表（编码像素）；不支持 ROI 时什么都不做；空列表 = 清除。
    pub fn set_roi(&self, regions: Vec<RoiRegion>) {
        if self.roi_supported {
            self.roi.send_replace(regions);
        }
    }

    pub fn set_rate(&self, kbps: u32, fps: u32) {
        let (kbps, fps) = if self.tcp {
            (kbps.clamp(2_000, 6_000), fps.min(30))
        } else {
            (kbps, fps)
        };
        let _ = self.rates.try_send((kbps, fps));
    }

    pub fn set_fec_ratio(&self, r: f32) {
        self.fec.send_replace(if self.tcp { 0.0 } else { r });
    }

    pub fn stats(&self) -> Arc<Mutex<HostStatsCollector>> {
        Arc::clone(&self.stats)
    }

    pub fn request_keyframe(&self) {
        // Keep the count even when several requests arrive before the task wakes.
        self.keyframes
            .send_modify(|count| *count = count.wrapping_add(1));
    }

    pub fn codec(&self) -> pb::Codec {
        self.codec
    }
}

impl Drop for VideoPipeline {
    fn drop(&mut self) {
        // Abort consumers before the capture handle drains its callbacks.
        self.encoder.abort();
        self.sender.abort();
    }
}

pub mod audio;
pub mod backlog;
