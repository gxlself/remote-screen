//! 视口裁剪 / ROI 的假实现与记录。

use std::sync::atomic::{AtomicBool, AtomicUsize, Ordering};
use std::sync::{Arc, Mutex};
use std::time::Duration;

use host_core::platform::{
    CaptureConfig, CaptureHandle, CapturedFrame, DisplayInfo, FrameSink, PixelRect, PlatformError,
    Result, RoiRegion,
};
use rsp_core::clock::unix_micros;
use tokio::task::JoinHandle;

use crate::SyntheticFrame;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RegionCapture {
    pub display: u32,
    pub rect: PixelRect,
    pub width: u32,
    pub height: u32,
}

#[derive(Default)]
struct Inner {
    region_supported: AtomicBool,
    region_fails: AtomicBool,
    roi_supported: AtomicBool,
    region_captures: Mutex<Vec<RegionCapture>>,
    active_region_captures: AtomicUsize,
    roi_calls: Mutex<Vec<Vec<RoiRegion>>>,
}

#[derive(Clone, Default)]
pub struct FakeViewport {
    inner: Arc<Inner>,
}

impl FakeViewport {
    pub fn set_region_supported(&self, on: bool) {
        self.inner.region_supported.store(on, Ordering::Relaxed);
    }

    pub fn set_region_fails(&self, on: bool) {
        self.inner.region_fails.store(on, Ordering::Relaxed);
    }

    pub fn set_roi_supported(&self, on: bool) {
        self.inner.roi_supported.store(on, Ordering::Relaxed);
    }

    pub fn region_captures(&self) -> Vec<RegionCapture> {
        self.inner
            .region_captures
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn active_region_captures(&self) -> usize {
        self.inner.active_region_captures.load(Ordering::Relaxed)
    }

    pub fn roi_calls(&self) -> Vec<Vec<RoiRegion>> {
        self.inner
            .roi_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .clone()
    }

    pub fn capture_region_supported(&self) -> bool {
        self.inner.region_supported.load(Ordering::Relaxed)
            || self.inner.region_fails.load(Ordering::Relaxed)
    }

    pub fn capture_region(
        &self,
        display: &DisplayInfo,
        rect: PixelRect,
        cfg: CaptureConfig,
        sink: FrameSink,
    ) -> Result<Box<dyn CaptureHandle>> {
        if !self.capture_region_supported() {
            return Err(PlatformError::Unsupported);
        }
        if self.inner.region_fails.load(Ordering::Relaxed) {
            return Err(PlatformError::Platform("injected region failure".into()));
        }
        if !(1..=120).contains(&cfg.fps) {
            return Err(PlatformError::Platform("capture fps must be 1..120".into()));
        }
        let right = rect.x.checked_add(rect.w);
        let bottom = rect.y.checked_add(rect.h);
        if rect.w < 2
            || rect.h < 2
            || right.is_none_or(|right| right > display.width_px)
            || bottom.is_none_or(|bottom| bottom > display.height_px)
        {
            return Err(PlatformError::Platform("crop outside display".into()));
        }
        let runtime = tokio::runtime::Handle::try_current()
            .map_err(|error| PlatformError::Platform(error.to_string()))?;
        self.inner
            .region_captures
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(RegionCapture {
                display: display.id,
                rect,
                width: cfg.max_width,
                height: cfg.max_height,
            });
        self.inner
            .active_region_captures
            .fetch_add(1, Ordering::Relaxed);
        let inner = Arc::clone(&self.inner);
        let display_id = display.id;
        let width = cfg.max_width;
        let height = cfg.max_height;
        let task = runtime.spawn(async move {
            let mut tick = tokio::time::interval(Duration::from_secs_f64(1.0 / f64::from(cfg.fps)));
            tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
            let mut frame_number = 0u64;
            loop {
                tick.tick().await;
                let capture_ts_us = unix_micros();
                let frame = CapturedFrame {
                    display_id,
                    width,
                    height,
                    capture_ts_us,
                    native: Arc::new(SyntheticFrame {
                        frame_number,
                        capture_ts_us,
                    }),
                };
                if let Err(tokio::sync::mpsc::error::TrySendError::Closed(_)) = sink.try_send(frame)
                {
                    return;
                }
                frame_number = frame_number.wrapping_add(1);
            }
        });
        Ok(Box::new(FakeRegionCapture { task, inner }))
    }

    pub fn roi_supported(&self) -> bool {
        self.inner.roi_supported.load(Ordering::Relaxed)
    }

    pub fn set_roi(&self, regions: &[RoiRegion]) -> Result<()> {
        if !self.roi_supported() {
            return Err(PlatformError::Unsupported);
        }
        self.inner
            .roi_calls
            .lock()
            .unwrap_or_else(|p| p.into_inner())
            .push(regions.to_vec());
        Ok(())
    }
}

struct FakeRegionCapture {
    task: JoinHandle<()>,
    inner: Arc<Inner>,
}

impl CaptureHandle for FakeRegionCapture {}

impl Drop for FakeRegionCapture {
    fn drop(&mut self) {
        self.task.abort();
        self.inner
            .active_region_captures
            .fetch_sub(1, Ordering::Relaxed);
    }
}
