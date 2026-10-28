//! 合成测试图像。

use crate::decode::Rect;
use crate::timecode::{render_code, Code, GrayImage};

/// 合成视口使用的确定性灰度背景。
#[derive(Debug, Clone, Copy)]
pub enum Background {
    Solid(u8),
    Noise { seed: u32 },
    Stripes { period: u32, offset: u32 },
}

/// xorshift32 伪随机数生成器；零状态按种子 1 处理。
pub struct Xorshift32(pub u32);

impl Xorshift32 {
    pub fn next_u32(&mut self) -> u32 {
        if self.0 == 0 {
            self.0 = 1;
        }
        self.0 ^= self.0 << 13;
        self.0 ^= self.0 >> 17;
        self.0 ^= self.0 << 5;
        self.0
    }
}

/// 先填背景，再在其上绘制可选的 RSTC1 时间码。
pub fn viewport(width: u32, height: u32, bg: Background, code: Option<Code>) -> GrayImage {
    let mut image = GrayImage::new(width, height, 0);
    match bg {
        Background::Solid(value) => image.data.fill(value),
        Background::Noise { seed } => {
            let mut rng = Xorshift32(seed);
            for value in &mut image.data {
                *value = rng.next_u32() as u8;
            }
        }
        Background::Stripes { period, offset } => {
            let period = period.max(1);
            for y in 0..height {
                for x in 0..width {
                    let stripe = ((u64::from(x) + u64::from(offset)) / u64::from(period)) % 2 == 0;
                    image.set(x, y, if stripe { 230 } else { 25 });
                }
            }
        }
    }
    if let Some(code) = code {
        render_code(&mut image, code);
    }
    image
}

/// 客户端窗口尺寸与远端画面的目标矩形。
#[derive(Debug, Clone, Copy)]
pub struct Placement {
    pub win_w: u32,
    pub win_h: u32,
    pub rect: Rect,
    pub fill: u8,
}

/// 按像素中心映射，把视口双线性缩放到窗口中的矩形内。
pub fn place(view: &GrayImage, p: &Placement) -> GrayImage {
    let mut image = GrayImage::new(p.win_w, p.win_h, p.fill);
    if view.width == 0
        || view.height == 0
        || !p.rect.x.is_finite()
        || !p.rect.y.is_finite()
        || !p.rect.w.is_finite()
        || !p.rect.h.is_finite()
        || p.rect.w <= 0.0
        || p.rect.h <= 0.0
    {
        return image;
    }

    let x_end = p.rect.x + p.rect.w;
    let y_end = p.rect.y + p.rect.h;
    let x_start = (p.rect.x - 0.5).ceil().clamp(0.0, p.win_w as f64) as u32;
    let x_end = (x_end - 0.5).ceil().clamp(0.0, p.win_w as f64) as u32;
    let y_start = (p.rect.y - 0.5).ceil().clamp(0.0, p.win_h as f64) as u32;
    let y_end = (y_end - 0.5).ceil().clamp(0.0, p.win_h as f64) as u32;
    for y in y_start..y_end {
        let py = y as f64 + 0.5;
        let sy = (((py - p.rect.y) / p.rect.h) * view.height as f64 - 0.5)
            .clamp(0.0, view.height as f64 - 1.0);
        let y0 = sy.floor() as u32;
        let y1 = (y0 + 1).min(view.height - 1);
        let ty = sy - y0 as f64;
        for x in x_start..x_end {
            let px = x as f64 + 0.5;
            let sx = ((px - p.rect.x) / p.rect.w) * view.width as f64 - 0.5;
            let sx = sx.clamp(0.0, view.width as f64 - 1.0);
            let x0 = sx.floor() as u32;
            let x1 = (x0 + 1).min(view.width - 1);
            let tx = sx - x0 as f64;
            let top = view.get(x0, y0) as f64 * (1.0 - tx) + view.get(x1, y0) as f64 * tx;
            let bottom = view.get(x0, y1) as f64 * (1.0 - tx) + view.get(x1, y1) as f64 * tx;
            let value = (top * (1.0 - ty) + bottom * ty).round().clamp(0.0, 255.0) as u8;
            image.set(x, y, value);
        }
    }
    image
}

/// 将整宽条带 `[y0, y1)` 与图像相交的部分填为固定灰度。
pub fn add_band(img: &mut GrayImage, y0: u32, y1: u32, value: u8) {
    let y0 = y0.min(img.height);
    let y1 = y1.min(img.height);
    if y0 >= y1 {
        return;
    }
    let width = img.width as usize;
    for y in y0..y1 {
        let start = y as usize * width;
        img.data[start..start + width].fill(value);
    }
}

/// 3×3 均值模糊，边界像素复制延伸，结果四舍五入。
pub fn box_blur3(img: &GrayImage) -> GrayImage {
    if img.width == 0 || img.height == 0 {
        return img.clone();
    }
    let mut output = GrayImage::new(img.width, img.height, 0);
    for y in 0..img.height {
        for x in 0..img.width {
            let mut sum = 0u32;
            for dy in -1i64..=1 {
                let yy = (y as i64 + dy).clamp(0, img.height as i64 - 1) as u32;
                for dx in -1i64..=1 {
                    let xx = (x as i64 + dx).clamp(0, img.width as i64 - 1) as u32;
                    sum += img.get(xx, yy) as u32;
                }
            }
            output.set(x, y, ((sum + 4) / 9) as u8);
        }
    }
    output
}

/// 添加范围为 `[-amp, amp]` 的确定性噪声并饱和截断。
pub fn add_noise(img: &mut GrayImage, amp: u8, seed: u32) {
    let mut rng = Xorshift32(seed);
    let span = amp as u16 * 2 + 1;
    for value in &mut img.data {
        let offset = (rng.next_u32() % u32::from(span)) as i16 - amp as i16;
        *value = (*value as i16 + offset).clamp(0, 255) as u8;
    }
}

/// 按块原始均值混合并量化；零块大小或零步长不作处理。
pub fn block_quantize(img: &mut GrayImage, block: u32, step: u8) {
    if img.width == 0 || img.height == 0 || block == 0 || step == 0 {
        return;
    }
    let step = u64::from(step);
    let mut y0 = 0;
    while y0 < img.height {
        let y1 = y0.saturating_add(block).min(img.height);
        let mut x0 = 0;
        while x0 < img.width {
            let x1 = x0.saturating_add(block).min(img.width);
            let count = u64::from(x1 - x0) * u64::from(y1 - y0);
            let mut sum = 0u64;
            for y in y0..y1 {
                for x in x0..x1 {
                    sum += u64::from(img.get(x, y));
                }
            }
            let mean = sum / count;
            for y in y0..y1 {
                for x in x0..x1 {
                    let value = u64::from(img.get(x, y));
                    let quantized = ((value + mean) / 2 / step) * step + step / 2;
                    img.set(x, y, quantized.min(255) as u8);
                }
            }
            x0 = x1;
        }
        y0 = y1;
    }
}
