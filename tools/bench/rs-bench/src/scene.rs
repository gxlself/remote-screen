//! 静态场景生成器 S1。

use crate::timecode::{GrayImage, PxRect};

/// S1 的固定 xorshift32 种子。
pub const STATIC_SEED: u32 = 0x9E37_79B9;
/// S1 的底色。
pub const STATIC_BACKGROUND: u8 = 40;
/// S1 生成的矩形数量。
pub const STATIC_COUNT: usize = 160;

/// S1 使用的 xorshift32 随机数生成器。
#[derive(Debug, Clone, Copy)]
pub struct SceneRng {
    state: u32,
}

impl SceneRng {
    /// 创建一个使用给定种子的生成器。
    pub fn new(seed: u32) -> Self {
        Self { state: seed }
    }

    /// 生成下一个 u32。每一步都在 u32 的位宽内截断。
    pub fn next_u32(&mut self) -> u32 {
        self.state ^= self.state << 13;
        self.state ^= self.state >> 17;
        self.state ^= self.state << 5;
        self.state
    }
}

/// S1 中的一个未裁剪矩形。
#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct StaticRect {
    pub x: u32,
    pub y: u32,
    pub w: u32,
    pub h: u32,
    pub g: u8,
}

impl StaticRect {
    /// 将矩形转换为图像坐标中的半开区间，并裁到给定尺寸。
    pub fn clipped(&self, width: u32, height: u32) -> PxRect {
        PxRect {
            x0: self.x,
            y0: self.y,
            x1: self.x.saturating_add(self.w).min(width),
            y1: self.y.saturating_add(self.h).min(height),
        }
    }
}

/// 按 S1 顺序生成给定尺寸下的 160 个未裁剪矩形。
pub fn static_rects(width: u32, height: u32) -> Vec<StaticRect> {
    assert!(width != 0, "static scene width must be nonzero");
    assert!(height != 0, "static scene height must be nonzero");

    let max_w = (width / 4).max(1);
    let max_h = (height / 6).max(1);
    let mut rng = SceneRng::new(STATIC_SEED);
    let mut rects = Vec::with_capacity(STATIC_COUNT);
    for _ in 0..STATIC_COUNT {
        rects.push(StaticRect {
            x: rng.next_u32() % width,
            y: rng.next_u32() % height,
            w: 16 + rng.next_u32() % max_w,
            h: 8 + rng.next_u32() % max_h,
            g: (rng.next_u32() % 256) as u8,
        });
    }
    rects
}

/// 渲染 S1 静态场景。后生成的矩形覆盖先生成的矩形。
pub fn render_static(width: u32, height: u32) -> GrayImage {
    assert!(width != 0, "static scene width must be nonzero");
    assert!(height != 0, "static scene height must be nonzero");

    let mut image = GrayImage::new(width, height, STATIC_BACKGROUND);
    for rect in static_rects(width, height) {
        image.fill_rect(rect.clipped(width, height), rect.g);
    }
    image
}
