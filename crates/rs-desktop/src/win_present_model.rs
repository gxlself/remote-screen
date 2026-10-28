//! ：Windows 视频呈现的纯逻辑（所有目标编译）。

use std::collections::VecDeque;

use crate::pointer::Viewport;

/// 视频在客户区（物理像素）里的目标矩形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectPx {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// Fit the video into the client area using the same geometry as pointer
/// mapping.  Rounding the two edges independently avoids a second geometry
/// implementation in the Windows layer.
pub fn target_rect(client_w: u32, client_h: u32, video_w: u32, video_h: u32) -> RectPx {
    let viewport = Viewport {
        view_w: f64::from(client_w),
        view_h: f64::from(client_h),
        video_w,
        video_h,
    };
    let (left, top, width, height) = viewport.content_rect();
    let round_edge = |value: f64| -> u32 {
        if !value.is_finite() || value <= 0.0 {
            0
        } else if value >= f64::from(u32::MAX) {
            u32::MAX
        } else {
            value.round() as u32
        }
    };
    RectPx {
        left: round_edge(left),
        top: round_edge(top),
        right: round_edge(left + width),
        bottom: round_edge(top + height),
    }
}

/// DXGI_COLOR_SPACE_TYPE: YCbCr studio range BT.709 input and full-range RGB
/// BT.709 output.
pub const STREAM_COLOR_SPACE: i32 = 8;
pub const OUTPUT_COLOR_SPACE: i32 = 0;

/// Legacy D3D11 video-processor color-space bitfields.
pub const STREAM_LEGACY: u32 = 0x14;
pub const OUTPUT_LEGACY: u32 = 0x04;

/// State machine for the DXGI frame-latency waitable object.
#[derive(Debug, Default)]
pub struct PresentPacer {
    can_present: bool,
    pending: Option<u32>,
    redraw: bool,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum PresentWhat {
    Pending(u32),
    Redraw,
}

impl PresentPacer {
    pub fn new() -> Self {
        Self::default()
    }

    /// Store only the newest decoded frame and return the superseded id.
    pub fn on_decoded(&mut self, frame_id: u32) -> Option<u32> {
        self.pending.replace(frame_id)
    }

    pub fn on_swapchain_ready(&mut self) {
        self.can_present = true;
    }

    pub fn request_redraw(&mut self) {
        self.redraw = true;
    }

    /// Consume one waitable-object credit and one item to display.
    #[allow(clippy::should_implement_trait)]
    pub fn next(&mut self) -> Option<PresentWhat> {
        if !self.can_present {
            return None;
        }
        let what = if let Some(frame_id) = self.pending.take() {
            // A fresh frame also satisfies a pending redraw request.
            self.redraw = false;
            PresentWhat::Pending(frame_id)
        } else if self.redraw {
            self.redraw = false;
            PresentWhat::Redraw
        } else {
            return None;
        };
        self.can_present = false;
        Some(what)
    }

    pub fn wants_waitable(&self) -> bool {
        !self.can_present
    }
}

/// Small FIFO cache used for D3D11 processor views.
pub struct ViewCache<K: PartialEq, V> {
    max: usize,
    entries: Vec<(K, V)>,
}

impl<K: PartialEq, V> ViewCache<K, V> {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            entries: Vec::new(),
        }
    }

    pub fn get_or_insert_with<E>(
        &mut self,
        key: K,
        f: impl FnOnce() -> Result<V, E>,
    ) -> Result<&V, E> {
        if let Some(index) = self.entries.iter().position(|(cached, _)| *cached == key) {
            return Ok(&self.entries[index].1);
        }
        if self.max == 0 {
            // A zero-sized cache cannot retain a value.  Still run the closure
            // and return its value for the duration of this call.
            let value = f()?;
            self.entries.clear();
            self.entries.push((key, value));
            return Ok(&self.entries[0].1);
        }
        let value = f()?;
        if self.entries.len() >= self.max {
            self.entries.remove(0);
        }
        self.entries.push((key, value));
        Ok(&self.entries[self.entries.len() - 1].1)
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }

    pub fn get(&self, key: &K) -> Option<&V> {
        self.entries
            .iter()
            .find(|(cached, _)| cached == key)
            .map(|(_, value)| value)
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }
}

pub const VIEW_CACHE_MAX: usize = 32;

/// At most three device rebuilds in any ten-second window.
pub struct ResetBudget {
    recent: VecDeque<u64>,
}

impl ResetBudget {
    pub fn new() -> Self {
        Self {
            recent: VecDeque::new(),
        }
    }

    pub fn try_take(&mut self, now_us: u64) -> bool {
        while self
            .recent
            .front()
            .is_some_and(|&at| now_us.saturating_sub(at) >= 10_000_000)
        {
            self.recent.pop_front();
        }
        if self.recent.len() >= 3 {
            return false;
        }
        self.recent.push_back(now_us);
        true
    }
}

impl Default for ResetBudget {
    fn default() -> Self {
        Self::new()
    }
}
