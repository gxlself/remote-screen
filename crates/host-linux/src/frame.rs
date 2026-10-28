//! Linux 平台帧：采集线程从 PipeWire / XShm 拷出的 BGRx 像素（V8 C1）。
//!
//! `CapturedFrame::native` 里放 `Arc<LinuxFrame>`；编码器用 [`LinuxFrame::of`] 取回。
//! 像素统一为 BGRx：SPA_VIDEO_FORMAT_BGRx = DRM XRGB8888 = AV_PIX_FMT_BGR0 =
//! X11 32 bpp 小端 ZPixmap。x 字节内容不保证。

use std::sync::Arc;

use host_core::platform::CapturedFrame;

/// 每像素字节数。
pub const BYTES_PER_PIXEL: u32 = 4;

/// 采集源给出的 32 位像素排列（内存字节顺序）。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum SrcFormat {
    /// B G R x，原样拷贝。
    Bgrx,
    /// B G R A，原样拷贝（A 当 x）。
    Bgra,
    /// R G B x，拷贝时交换 R、B。
    Rgbx,
    /// R G B A，拷贝时交换 R、B。
    Rgba,
}

/// 一帧 BGRx 像素。`pixels.len() == stride * height`，`stride >= width * 4`。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct LinuxFrame {
    pub width: u32,
    pub height: u32,
    pub stride: u32,
    /// 采集源重建（换屏、改尺寸、重开 portal 流）时加 1；编码器据此重开。
    pub generation: u64,
    pub pixels: Vec<u8>,
}

impl LinuxFrame {
    /// 从 host-core 帧取回 Linux 帧；不是本平台的帧返回 None。
    pub fn of(frame: &CapturedFrame) -> Option<&LinuxFrame> {
        frame.native.downcast_ref::<LinuxFrame>()
    }

    /// 第 `y` 行的 `width * 4` 字节；越界返回 None。
    pub fn row(&self, y: u32) -> Option<&[u8]> {
        if y >= self.height {
            return None;
        }
        let start = y as usize * self.stride as usize;
        let len = self.width as usize * BYTES_PER_PIXEL as usize;
        self.pixels.get(start..start.checked_add(len)?)
    }
}

/// 把 `src`（每行 `src_stride` 字节）的 `width × height` 像素按 `format` 换成 BGRx，
/// 写进 `dst`（每行 `dst_stride` 字节）。任何一边长度不够返回 false 且不写。
pub fn copy_to_bgrx(
    src: &[u8],
    src_stride: usize,
    format: SrcFormat,
    width: u32,
    height: u32,
    dst: &mut [u8],
    dst_stride: usize,
) -> bool {
    let row_len = width as usize * BYTES_PER_PIXEL as usize;
    let rows = height as usize;
    if rows == 0 || row_len == 0 {
        return true;
    }
    let need = |stride: usize| {
        stride
            .checked_mul(rows - 1)
            .and_then(|n| n.checked_add(row_len))
    };
    match (need(src_stride), need(dst_stride)) {
        (Some(s), Some(d))
            if s <= src.len()
                && d <= dst.len()
                && src_stride >= row_len
                && dst_stride >= row_len => {}
        _ => return false,
    }
    for y in 0..rows {
        let s = &src[y * src_stride..y * src_stride + row_len];
        let d = &mut dst[y * dst_stride..y * dst_stride + row_len];
        match format {
            SrcFormat::Bgrx | SrcFormat::Bgra => d.copy_from_slice(s),
            SrcFormat::Rgbx | SrcFormat::Rgba => {
                for (dp, sp) in d.chunks_exact_mut(4).zip(s.chunks_exact(4)) {
                    dp[0] = sp[2];
                    dp[1] = sp[1];
                    dp[2] = sp[0];
                    dp[3] = sp[3];
                }
            }
        }
    }
    true
}

/// 帧缓冲池：最多 `max` 块，所有引用释放后复用，避免每帧分配 8 MB。
#[derive(Debug)]
pub struct FramePool {
    slots: Vec<Arc<LinuxFrame>>,
    max: usize,
}

impl FramePool {
    pub fn new(max: usize) -> Self {
        Self {
            slots: Vec::new(),
            max: max.max(1),
        }
    }

    /// 取一块没人引用的缓冲，按尺寸调整后交给 `fill` 写像素（切片长度 = `stride * height`）。
    /// 全部被占用且已到上限返回 None，调用方丢掉这一帧。
    pub fn acquire(
        &mut self,
        width: u32,
        height: u32,
        stride: u32,
        generation: u64,
        fill: impl FnOnce(&mut [u8]),
    ) -> Option<Arc<LinuxFrame>> {
        let len = stride as usize * height as usize;
        let index = match self
            .slots
            .iter_mut()
            .position(|slot| Arc::get_mut(slot).is_some())
        {
            Some(index) => index,
            None if self.slots.len() < self.max => {
                self.slots.push(Arc::new(LinuxFrame {
                    width,
                    height,
                    stride,
                    generation,
                    pixels: Vec::new(),
                }));
                self.slots.len() - 1
            }
            None => return None,
        };
        let frame = Arc::get_mut(&mut self.slots[index]).expect("slot was checked to be unique");
        frame.width = width;
        frame.height = height;
        frame.stride = stride;
        frame.generation = generation;
        frame.pixels.resize(len, 0);
        fill(&mut frame.pixels[..len]);
        Some(Arc::clone(&self.slots[index]))
    }

    /// 丢掉池里的全部缓冲（尺寸大变后调用）；仍被引用的缓冲在最后一个引用释放时回收。
    pub fn clear(&mut self) {
        self.slots.clear();
    }

    pub fn len(&self) -> usize {
        self.slots.len()
    }

    pub fn is_empty(&self) -> bool {
        self.slots.is_empty()
    }
}
