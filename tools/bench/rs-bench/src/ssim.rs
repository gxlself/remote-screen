//! 画质 SSIM。

use std::path::Path;

use anyhow::Context;
use clap::Parser;
use serde::Serialize;

use crate::scene::render_static;
use crate::timecode::{unit_x, unit_y, GrayImage};

/// SSIM 的亮度常数 `(0.01 * 255)^2`。
pub const C1: f64 = 6.5025;
/// SSIM 的对比度常数 `(0.03 * 255)^2`。
pub const C2: f64 = 58.5225;
/// SSIM 窗口边长。
pub const WINDOW: u32 = 8;
/// SSIM 窗口步长。
pub const STRIDE: u32 = 4;
/// 比较图像的最大高度。
pub const MAX_TARGET_H: u32 = 360;

/// 将源图像按面积采样到目标尺寸。
pub fn area_downscale(img: &GrayImage, tw: u32, th: u32) -> GrayImage {
    assert!(img.width != 0, "source image width must be nonzero");
    assert!(img.height != 0, "source image height must be nonzero");
    assert!(tw != 0, "target image width must be nonzero");
    assert!(th != 0, "target image height must be nonzero");

    let mut output = GrayImage::new(tw, th, 0);
    for ty in 0..th {
        let (y0, y1) = footprint_bounds(ty, img.height, th);
        for tx in 0..tw {
            let (x0, x1) = footprint_bounds(tx, img.width, tw);
            let mut sum = 0u64;
            let count = u64::from(x1 - x0) * u64::from(y1 - y0);
            for sy in y0..y1 {
                let start = sy as usize * img.width as usize + x0 as usize;
                let end = sy as usize * img.width as usize + x1 as usize;
                for &value in &img.data[start..end] {
                    sum += u64::from(value);
                }
            }
            let value = (sum + count / 2) / count;
            output.data[ty as usize * tw as usize + tx as usize] = value as u8;
        }
    }
    output
}

/// 返回比较图像尺寸 `(tw, th)`。
pub fn target_size(frame_w: u32, frame_h: u32, host_w: u32, host_h: u32) -> (u32, u32) {
    assert!(frame_w != 0, "frame width must be nonzero");
    assert!(frame_h != 0, "frame height must be nonzero");
    assert!(host_w != 0, "host width must be nonzero");
    assert!(host_h != 0, "host height must be nonzero");

    let th = frame_h.min(host_h).min(MAX_TARGET_H);
    let numerator = 2u64 * u64::from(th) * u64::from(host_w) + u64::from(host_h);
    let denominator = 2u64 * u64::from(host_h);
    let tw = (numerator / denominator) as u32;
    (tw, th)
}

/// 比较窗口的逐像素遮罩。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Mask {
    width: u32,
    height: u32,
    pixels: Vec<bool>,
}

impl Mask {
    /// 创建一张没有遮罩像素的图。
    pub fn none(width: u32, height: u32) -> Mask {
        let count = (width as usize)
            .checked_mul(height as usize)
            .expect("mask dimensions overflow addressable memory");
        Mask {
            width,
            height,
            pixels: vec![false; count],
        }
    }

    /// 创建静态场景的顶部和时间码区域遮罩。
    pub fn static_scene(tw: u32, th: u32) -> Mask {
        let mut mask = Self::none(tw, th);
        let top = th / 10;
        let timecode_x0 = unit_x(1, tw).min(tw);
        let timecode_y0 = unit_y(13, th).min(th);
        let timecode_x1 = unit_x(13, tw).min(tw);
        let timecode_y1 = unit_y(23, th).min(th);

        for y in 0..th {
            let in_top = y < top;
            for x in 0..tw {
                let in_timecode =
                    x >= timecode_x0 && x < timecode_x1 && y >= timecode_y0 && y < timecode_y1;
                if in_top || in_timecode {
                    mask.pixels[y as usize * tw as usize + x as usize] = true;
                }
            }
        }
        mask
    }

    /// 返回一个像素是否被遮罩。
    pub fn is_masked(&self, x: u32, y: u32) -> bool {
        assert!(x < self.width, "mask x coordinate out of bounds");
        assert!(y < self.height, "mask y coordinate out of bounds");
        self.pixels[y as usize * self.width as usize + x as usize]
    }
}

/// 比较一对图像在一个位移下的窗口 SSIM。
pub fn ssim_masked(
    frame: &GrayImage,
    reference: &GrayImage,
    mask: &Mask,
    dx: i32,
    dy: i32,
) -> Option<(f64, usize)> {
    if frame.width != reference.width
        || frame.height != reference.height
        || mask.width != frame.width
        || mask.height != frame.height
    {
        return None;
    }

    let width = frame.width;
    let height = frame.height;
    if width < WINDOW || height < WINDOW {
        return None;
    }
    let mut total = 0.0f64;
    let mut windows = 0usize;
    let dx = i64::from(dx);
    let dy = i64::from(dy);

    for y in (0..=height - WINDOW).step_by(STRIDE as usize) {
        for x in (0..=width - WINDOW).step_by(STRIDE as usize) {
            let reference_x = i64::from(x) + dx;
            let reference_y = i64::from(y) + dy;
            let reference_x_end = reference_x + i64::from(WINDOW);
            let reference_y_end = reference_y + i64::from(WINDOW);
            if reference_x >= 0
                && reference_y >= 0
                && reference_x_end <= i64::from(width)
                && reference_y_end <= i64::from(height)
            {
                let mut masked = false;
                let mut sx = 0u64;
                let mut sy = 0u64;
                let mut sxx = 0u64;
                let mut syy = 0u64;
                let mut sxy = 0u64;
                for wy in 0..WINDOW {
                    for wx in 0..WINDOW {
                        let frame_x = x + wx;
                        let frame_y = y + wy;
                        if mask.is_masked(frame_x, frame_y) {
                            masked = true;
                            break;
                        }
                        let frame_value = u64::from(frame.get(frame_x, frame_y));
                        let reference_value = u64::from(reference.get(
                            (reference_x + i64::from(wx)) as u32,
                            (reference_y + i64::from(wy)) as u32,
                        ));
                        sx += frame_value;
                        sy += reference_value;
                        sxx += frame_value * frame_value;
                        syy += reference_value * reference_value;
                        sxy += frame_value * reference_value;
                    }
                    if masked {
                        break;
                    }
                }
                if !masked {
                    let mean_x = sx as f64 / 64.0;
                    let mean_y = sy as f64 / 64.0;
                    let variance_x = (64i64 * sxx as i64 - sx as i64 * sx as i64) as f64 / 4096.0;
                    let variance_y = (64i64 * syy as i64 - sy as i64 * sy as i64) as f64 / 4096.0;
                    let covariance = (64i64 * sxy as i64 - sx as i64 * sy as i64) as f64 / 4096.0;
                    let numerator = (2.0 * mean_x * mean_y + C1) * (2.0 * covariance + C2);
                    let denominator =
                        (mean_x * mean_x + mean_y * mean_y + C1) * (variance_x + variance_y + C2);
                    total += numerator / denominator;
                    windows += 1;
                }
            }
        }
    }

    (windows != 0).then_some((total / windows as f64, windows))
}

/// 一帧静态场景的对齐和 SSIM 结果。
#[derive(Debug, Clone, PartialEq)]
pub struct FrameScore {
    pub ssim: f64,
    pub dx: i32,
    pub dy: i32,
    pub windows: usize,
    pub target_w: u32,
    pub target_h: u32,
}

/// 对传入的静态参考图，在给定半径内寻找 SSIM 最优对齐。
pub fn score_static(
    frame: &GrayImage,
    reference: &GrayImage,
    radius: u32,
) -> Result<FrameScore, String> {
    if frame.width == 0 || frame.height == 0 || reference.width == 0 || reference.height == 0 {
        return Err("images must have nonzero dimensions".to_owned());
    }
    let (target_w, target_h) =
        target_size(frame.width, frame.height, reference.width, reference.height);
    if target_w == 0 || target_h == 0 {
        return Err("target image dimensions are zero".to_owned());
    }
    let radius = i32::try_from(radius).map_err(|_| "alignment radius is too large".to_owned())?;
    let frame_scaled = area_downscale(frame, target_w, target_h);
    let reference_scaled = area_downscale(reference, target_w, target_h);
    let mask = Mask::static_scene(target_w, target_h);

    let mut best: Option<(f64, i32, i32, usize)> = None;
    for dy in -radius..=radius {
        for dx in -radius..=radius {
            let Some((ssim, windows)) =
                ssim_masked(&frame_scaled, &reference_scaled, &mask, dx, dy)
            else {
                continue;
            };
            if best.as_ref().is_none_or(|current| ssim > current.0) {
                best = Some((ssim, dx, dy, windows));
            }
        }
    }

    let Some((ssim, dx, dy, windows)) = best else {
        return Err("no usable SSIM windows".to_owned());
    };
    Ok(FrameScore {
        ssim,
        dx,
        dy,
        windows,
        target_w,
        target_h,
    })
}

#[derive(Debug, Parser)]
#[command(name = "rs-bench ssim", about = "compare static scene frame quality")]
struct SsimArgs {
    #[arg(long, required = true, value_name = "SCENE")]
    scene: String,
    #[arg(long, required = true, value_name = "WxH")]
    host_size: String,
    #[arg(long = "frame", required = true, value_name = "PGM")]
    frames: Vec<String>,
    #[arg(long, default_value_t = 2, value_name = "PIXELS")]
    align: u32,
    #[arg(long, value_name = "PATH|-")]
    json: Option<String>,
}

#[derive(Debug, Serialize)]
struct JsonFrame {
    path: String,
    ssim: f64,
    dx: i32,
    dy: i32,
    windows: usize,
}

#[derive(Debug, Serialize)]
struct JsonOutput {
    schema: u8,
    scene: &'static str,
    host_w: u32,
    host_h: u32,
    target_w: u32,
    target_h: u32,
    frames: Vec<JsonFrame>,
    mean: f64,
}

/// `rs-bench ssim` 的命令行入口。
pub fn cli(args: Vec<String>) -> anyhow::Result<i32> {
    let options = match SsimArgs::try_parse_from(args) {
        Ok(options) => options,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return Ok(code);
        }
    };

    if options.scene != "static" {
        eprintln!(
            "error: --scene only accepts 'static', got '{}'",
            options.scene
        );
        return Ok(2);
    }
    let (host_w, host_h) = match parse_host_size(&options.host_size) {
        Ok(size) => size,
        Err(error) => {
            eprintln!(
                "error: invalid --host-size '{}': {error}",
                options.host_size
            );
            return Ok(2);
        }
    };

    let reference = render_static(host_w, host_h);
    let mut scores = Vec::with_capacity(options.frames.len());
    for path in &options.frames {
        let bytes = match std::fs::read(path) {
            Ok(bytes) => bytes,
            Err(error) => {
                eprintln!("{path}: cannot read frame: {error}");
                return Ok(1);
            }
        };
        let frame = match GrayImage::from_pgm(&bytes) {
            Ok(frame) => frame,
            Err(error) => {
                eprintln!("{path}: invalid PGM: {error}");
                return Ok(1);
            }
        };
        let score = match score_static(&frame, &reference, options.align) {
            Ok(score) => score,
            Err(error) => {
                eprintln!("{path}: {error}");
                return Ok(1);
            }
        };
        scores.push((path.clone(), score));
    }

    let mean = scores.iter().map(|(_, score)| score.ssim).sum::<f64>() / scores.len() as f64;
    if let Some(json_path) = options.json {
        let first = &scores[0].1;
        let output = JsonOutput {
            schema: 1,
            scene: "static",
            host_w,
            host_h,
            target_w: first.target_w,
            target_h: first.target_h,
            frames: scores
                .iter()
                .map(|(path, score)| JsonFrame {
                    path: path.clone(),
                    ssim: round4(score.ssim),
                    dx: score.dx,
                    dy: score.dy,
                    windows: score.windows,
                })
                .collect(),
            mean: round4(mean),
        };
        let encoded = serde_json::to_string(&output).context("encode SSIM JSON")?;
        if json_path == "-" {
            println!("{encoded}");
        } else if let Err(error) = std::fs::write(Path::new(&json_path), format!("{encoded}\n")) {
            eprintln!("{json_path}: cannot write JSON: {error}");
            return Ok(1);
        }
    } else {
        for (path, score) in &scores {
            println!(
                "{path}: ssim {:.4} (dx {}, dy {}, {} windows)",
                round4(score.ssim),
                score.dx,
                score.dy,
                score.windows
            );
        }
        println!("mean {:.4}", round4(mean));
    }
    Ok(0)
}

fn parse_host_size(value: &str) -> Result<(u32, u32), &'static str> {
    let (width, height) = value.split_once('x').ok_or("expected <width>x<height>")?;
    if width.is_empty()
        || height.is_empty()
        || !width.bytes().all(|byte| byte.is_ascii_digit())
        || !height.bytes().all(|byte| byte.is_ascii_digit())
    {
        return Err("expected decimal <width>x<height>");
    }
    let width = width
        .parse::<u32>()
        .map_err(|_| "width must be an integer")?;
    let height = height
        .parse::<u32>()
        .map_err(|_| "height must be an integer")?;
    if width == 0 || height == 0 {
        return Err("width and height must be nonzero");
    }
    Ok((width, height))
}

fn round4(value: f64) -> f64 {
    (value * 10_000.0).round() / 10_000.0
}

fn footprint_bounds(index: u32, source: u32, target: u32) -> (u32, u32) {
    let denominator = 2i64 * i64::from(target);
    let lo_numerator = 2i64 * i64::from(index) * i64::from(source) - i64::from(target);
    let hi_numerator = 2i64 * (i64::from(index) + 1) * i64::from(source) - i64::from(target);
    let lo = ceil_div(lo_numerator, denominator).clamp(0, i64::from(source)) as u32;
    let hi = ceil_div(hi_numerator, denominator).clamp(0, i64::from(source)) as u32;
    if hi > lo {
        (lo, hi)
    } else {
        let center = ((2 * i64::from(index) + 1) * i64::from(source) / denominator)
            .clamp(0, i64::from(source - 1)) as u32;
        (center, center + 1)
    }
}

fn ceil_div(numerator: i64, denominator: i64) -> i64 {
    debug_assert!(denominator > 0);
    numerator.div_euclid(denominator) + i64::from(numerator.rem_euclid(denominator) != 0)
}
