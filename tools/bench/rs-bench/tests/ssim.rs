use std::fs;
use std::process::Command;
use std::time::Instant;

use rs_bench::scene::{
    render_static, static_rects, SceneRng, StaticRect, STATIC_COUNT, STATIC_SEED,
};
use rs_bench::ssim::{area_downscale, score_static, ssim_masked, target_size, Mask};
use rs_bench::timecode::{GrayImage, PxRect};
use serde_json::Value;

fn vectors() -> Value {
    serde_json::from_str(include_str!("../../vectors/static_scene.json"))
        .expect("static scene vector JSON must be valid")
}

fn fnv1a(data: &[u8]) -> u64 {
    let mut hash = 0xcbf29ce484222325u64;
    for &byte in data {
        hash ^= u64::from(byte);
        hash = hash.wrapping_mul(0x100000001b3);
    }
    hash
}

fn rect_from_value(value: &Value) -> StaticRect {
    let values = value.as_array().expect("rectangle vector must be an array");
    StaticRect {
        x: values[0].as_u64().unwrap() as u32,
        y: values[1].as_u64().unwrap() as u32,
        w: values[2].as_u64().unwrap() as u32,
        h: values[3].as_u64().unwrap() as u32,
        g: values[4].as_u64().unwrap() as u8,
    }
}

#[test]
fn rng_first3() {
    let root = vectors();
    let expected = root["xorshift32_first3"].as_array().unwrap();
    let mut rng = SceneRng::new(STATIC_SEED);
    for value in expected {
        assert_eq!(rng.next_u32(), value.as_u64().unwrap() as u32);
    }
    let mut zero = SceneRng::new(0);
    for _ in 0..3 {
        assert_eq!(zero.next_u32(), 0);
    }
}

#[test]
fn rects_vectors() {
    let root = vectors();
    assert_eq!(root["background"], 40);
    assert_eq!(root["count"], STATIC_COUNT);
    for size in root["sizes"].as_array().unwrap() {
        let width = size["width"].as_u64().unwrap() as u32;
        let height = size["height"].as_u64().unwrap() as u32;
        let rects = static_rects(width, height);
        let expected_first = size["first4"].as_array().unwrap();
        assert_eq!(rects.len(), STATIC_COUNT);
        for (actual, expected) in rects.iter().take(4).zip(expected_first) {
            assert_eq!(*actual, rect_from_value(expected));
        }
        assert_eq!(rects[STATIC_COUNT - 1], rect_from_value(&size["last"]));
        let checksum = rects.iter().fold(0u32, |sum, rect| {
            sum.wrapping_add(rect.x)
                .wrapping_add(rect.y)
                .wrapping_add(rect.w)
                .wrapping_add(rect.h)
                .wrapping_add(u32::from(rect.g))
        });
        assert_eq!(checksum, size["checksum"].as_u64().unwrap() as u32);
    }
    let edge = StaticRect {
        x: 639,
        y: 359,
        w: u32::MAX,
        h: u32::MAX,
        g: 152,
    };
    assert_eq!(
        edge.clipped(640, 360),
        PxRect {
            x0: 639,
            y0: 359,
            x1: 640,
            y1: 360,
        }
    );
}

#[test]
fn scene_zero_dimensions() {
    for (width, height) in [(0, 1), (1, 0), (0, 0)] {
        assert!(std::panic::catch_unwind(|| static_rects(width, height)).is_err());
        assert!(std::panic::catch_unwind(|| render_static(width, height)).is_err());
    }
}

#[test]
fn render_static_hash() {
    for (width, height, expected) in [
        (1280, 720, 0xf3d1790e82059e45),
        (1920, 1080, 0x8e6035632f8bcf3e),
        (640, 360, 0x30f779bc5cf06743),
    ] {
        let image = render_static(width, height);
        assert_eq!(fnv1a(&image.data), expected, "{width}x{height}");
    }
    let image = render_static(1920, 1080);
    assert_eq!(image.get(337, 60), 152);
    assert_eq!(image.get(683, 143), 152);
}

#[test]
fn area_downscale_cases() {
    let image = GrayImage::from_raw(3, 2, vec![0, 1, 2, 3, 4, 5]).unwrap();
    assert_eq!(area_downscale(&image, 3, 2), image);

    let image = GrayImage::from_raw(4, 4, (0..16).collect()).unwrap();
    assert_eq!(area_downscale(&image, 2, 2).data, vec![3, 5, 11, 13]);

    let image = GrayImage::from_raw(5, 1, vec![10, 20, 30, 40, 50]).unwrap();
    assert_eq!(area_downscale(&image, 2, 1).data, vec![15, 40]);

    let image = GrayImage::from_raw(2, 1, vec![17, 231]).unwrap();
    assert_eq!(area_downscale(&image, 4, 1).data, vec![17, 17, 231, 231]);

    let image = GrayImage::from_raw(2, 4, vec![0, 100, 10, 110, 20, 120, 30, 130]).unwrap();
    assert_eq!(
        area_downscale(&image, 4, 2).data,
        vec![5, 5, 105, 105, 25, 25, 125, 125]
    );
    let image = GrayImage::from_raw(4, 2, vec![0, 10, 20, 30, 100, 110, 120, 130]).unwrap();
    assert_eq!(
        area_downscale(&image, 2, 4).data,
        vec![5, 25, 5, 25, 105, 125, 105, 125]
    );

    let image = render_static(1920, 1080);
    assert_eq!(
        fnv1a(&area_downscale(&image, 1440, 810).data),
        0x67f8fe842a5a3991
    );
}

#[test]
fn target_size_cases() {
    assert_eq!(target_size(1440, 810, 1920, 1080), (640, 360));
    assert_eq!(target_size(3840, 2160, 1920, 1080), (640, 360));
    assert_eq!(target_size(1366, 768, 1366, 768), (640, 360));
    assert_eq!(target_size(320, 180, 1920, 1080), (320, 180));
    assert_eq!(target_size(1, 360, 1, 1080), (0, 360));
}

#[test]
fn static_mask() {
    let mask = Mask::static_scene(640, 360);
    for (x, y) in [(300, 35), (10, 130), (129, 229)] {
        assert!(mask.is_masked(x, y), "({x},{y})");
    }
    for (x, y) in [(300, 36), (130, 130), (9, 200), (129, 230)] {
        assert!(!mask.is_masked(x, y), "({x},{y})");
    }
}

#[test]
fn constant_images() {
    let frame = GrayImage::new(64, 64, 100);
    let reference = GrayImage::new(64, 64, 110);
    let (ssim, windows) = ssim_masked(&frame, &reference, &Mask::none(64, 64), 0, 0).unwrap();
    assert_eq!(windows, 225);
    assert!((ssim - 0.9954764440915066).abs() <= 1e-12);
}

#[test]
fn identical() {
    let image = render_static(640, 360);
    let score = score_static(&image, &image, 2).unwrap();
    assert!(score.ssim >= 1.0 - 1e-12);
    assert_eq!(score.dx, 0);
    assert_eq!(score.dy, 0);
    assert_eq!(score.windows, 11856);
    assert_eq!((score.target_w, score.target_h), (640, 360));
}

#[test]
fn alignment_ties() {
    let image = GrayImage::new(64, 64, 100);
    let score = score_static(&image, &image, 2).unwrap();
    assert!((score.ssim - 1.0).abs() <= 1e-12);
    assert_eq!((score.dx, score.dy), (-2, -2));
    let score = score_static(&image, &image, 0).unwrap();
    assert_eq!((score.dx, score.dy), (0, 0));
}

#[test]
fn masked_changes_are_ignored() {
    let reference = render_static(640, 360);
    let mut frame = reference.clone();
    let mask = Mask::static_scene(frame.width, frame.height);
    for y in 0..frame.height {
        for x in 0..frame.width {
            if mask.is_masked(x, y) {
                frame.set(x, y, 255 - reference.get(x, y));
            }
        }
    }
    let (ssim, windows) = ssim_masked(&frame, &reference, &mask, 0, 0).unwrap();
    assert!((ssim - 1.0).abs() <= 1e-12);
    assert_eq!(windows, 11856);
}

fn brighten(image: &GrayImage) -> GrayImage {
    GrayImage::from_raw(
        image.width,
        image.height,
        image
            .data
            .iter()
            .map(|&value| value.saturating_add(20))
            .collect(),
    )
    .unwrap()
}

fn shift_right(image: &GrayImage, fill: u8) -> GrayImage {
    let mut shifted = GrayImage::new(image.width, image.height, fill);
    for y in 0..image.height {
        for x in 1..image.width {
            shifted.set(x, y, image.get(x - 1, y));
        }
    }
    shifted
}

fn blur_once(image: &GrayImage) -> GrayImage {
    let mut output = GrayImage::new(image.width, image.height, 0);
    for y in 0..image.height {
        for x in 0..image.width {
            let mut sum = 0u32;
            for oy in -1i32..=1 {
                for ox in -1i32..=1 {
                    let sx = (x as i32 + ox).clamp(0, image.width as i32 - 1) as u32;
                    let sy = (y as i32 + oy).clamp(0, image.height as i32 - 1) as u32;
                    sum += u32::from(image.get(sx, sy));
                }
            }
            output.set(x, y, ((sum + 4) / 9) as u8);
        }
    }
    output
}

#[test]
fn reference_values() {
    let reference = render_static(640, 360);
    let score = score_static(&brighten(&reference), &reference, 2).unwrap();
    assert!((score.ssim - 0.9290759745108877).abs() <= 1e-9);
    assert_eq!(
        (
            score.dx,
            score.dy,
            score.windows,
            score.target_w,
            score.target_h
        ),
        (0, 0, 11856, 640, 360)
    );

    let score = score_static(&shift_right(&reference, 40), &reference, 2).unwrap();
    assert!((score.ssim - 1.0).abs() <= 1e-12);
    assert_eq!((score.dx, score.dy, score.windows), (-1, 0, 11776));

    let score = score_static(&blur_once(&reference), &reference, 2).unwrap();
    assert!((score.ssim - 0.9135663123548301).abs() <= 1e-9);
    assert_eq!((score.dx, score.dy, score.windows), (0, 0, 11856));

    let large = render_static(1920, 1080);
    let frame = area_downscale(&large, 1440, 810);
    let score = score_static(&frame, &large, 2).unwrap();
    assert!((score.ssim - 0.9954423940950988).abs() <= 1e-9);
    assert_eq!((score.dx, score.dy, score.windows), (0, 0, 11856));
}

#[test]
fn monotonic_blur() {
    let reference = render_static(640, 360);
    let once = score_static(&blur_once(&reference), &reference, 2).unwrap();
    let twice_image = blur_once(&blur_once(&reference));
    let twice = score_static(&twice_image, &reference, 2).unwrap();
    assert!(once.ssim < 1.0);
    assert!(twice.ssim < once.ssim);
}

#[test]
fn size_mismatch_none() {
    let frame = GrayImage::new(16, 16, 100);
    let reference = GrayImage::new(17, 16, 100);
    assert!(ssim_masked(&frame, &reference, &Mask::none(16, 16), 0, 0).is_none());
    assert!(ssim_masked(&frame, &frame, &Mask::none(15, 16), 0, 0).is_none());
    assert!(ssim_masked(&frame, &frame, &Mask::none(16, 16), i32::MIN, i32::MAX).is_none());
    let image = GrayImage::new(8, 8, 100);
    let mask = Mask::static_scene(8, 8);
    assert!(mask.is_masked(0, 3));
    assert!(ssim_masked(&image, &image, &mask, 0, 0).is_none());
    assert!(score_static(&image, &image, 2).is_err());
    assert!(ssim_masked(
        &GrayImage::new(7, 7, 100),
        &GrayImage::new(7, 7, 100),
        &Mask::none(7, 7),
        0,
        0
    )
    .is_none());
    assert!(score_static(&GrayImage::new(0, 8, 0), &image, 2).is_err());
}

#[test]
fn cli_json() {
    let root = std::env::temp_dir().join(format!("rs-bench-ssim-{}", std::process::id()));
    let _ = fs::remove_dir_all(&root);
    fs::create_dir_all(&root).unwrap();
    let reference = render_static(640, 360);
    let first = root.join("a.pgm");
    let second = root.join("b.pgm");
    fs::write(&first, reference.to_pgm()).unwrap();
    fs::write(&second, shift_right(&reference, 40).to_pgm()).unwrap();
    fs::write(root.join("c.pgm"), brighten(&reference).to_pgm()).unwrap();

    let output = Command::new(env!("CARGO_BIN_EXE_rs-bench"))
        .current_dir(&root)
        .args([
            "ssim",
            "--scene",
            "static",
            "--host-size",
            "640x360",
            "--frame",
            "a.pgm",
            "--frame",
            "./b.pgm",
            "--json",
            "-",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stderr.is_empty());
    assert_eq!(
        output.stdout.iter().filter(|&&byte| byte == b'\n').count(),
        1
    );
    let json: Value = serde_json::from_slice(&output.stdout).unwrap();
    assert_eq!(json["schema"], 1);
    assert_eq!(json["scene"], "static");
    assert_eq!(
        (json["host_w"].as_u64(), json["host_h"].as_u64()),
        (Some(640), Some(360))
    );
    assert_eq!(json["frames"].as_array().unwrap().len(), 2);
    assert_eq!(json["frames"][0]["ssim"], 1.0);
    assert_eq!(json["frames"][1]["ssim"], 1.0);
    assert_eq!(json["frames"][1]["dx"], -1);
    assert_eq!(json["frames"][0]["path"], "a.pgm");
    assert_eq!(json["frames"][1]["path"], "./b.pgm");
    assert_eq!(json["frames"][0]["windows"], 11856);
    assert_eq!(json["frames"][1]["windows"], 11776);
    assert_eq!(json["mean"], 1.0);
    assert_eq!(json["target_w"], 640);
    assert_eq!(json["target_h"], 360);

    let output = Command::new(env!("CARGO_BIN_EXE_rs-bench"))
        .current_dir(&root)
        .args([
            "ssim",
            "--scene",
            "static",
            "--host-size",
            "640x360",
            "--frame",
            "a.pgm",
            "--frame",
            "c.pgm",
            "--align",
            "0",
            "--json",
            "scores.json",
        ])
        .output()
        .unwrap();
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    assert!(output.stdout.is_empty());
    let encoded = fs::read(root.join("scores.json")).unwrap();
    assert_eq!(encoded.iter().filter(|&&byte| byte == b'\n').count(), 1);
    assert_eq!(encoded.last(), Some(&b'\n'));
    let json: Value = serde_json::from_slice(&encoded).unwrap();
    assert_eq!(json["frames"][1]["ssim"], 0.9291);
    assert_eq!(json["mean"], 0.9645);

    let output = Command::new(env!("CARGO_BIN_EXE_rs-bench"))
        .current_dir(&root)
        .args([
            "ssim",
            "--scene",
            "static",
            "--host-size",
            "640x360",
            "--frame",
            "a.pgm",
            "--align",
            "0",
        ])
        .output()
        .unwrap();
    assert!(output.status.success());
    assert_eq!(
        String::from_utf8(output.stdout).unwrap(),
        "a.pgm: ssim 1.0000 (dx 0, dy 0, 11856 windows)\nmean 1.0000\n"
    );
    let _ = fs::remove_dir_all(root);
}

#[test]
fn cli_errors() {
    let exe = env!("CARGO_BIN_EXE_rs-bench");
    let root = std::env::temp_dir().join(format!("rs-bench-ssim-errors-{}", std::process::id()));
    fs::create_dir_all(&root).unwrap();
    fs::write(root.join("valid.pgm"), render_static(64, 64).to_pgm()).unwrap();
    fs::write(root.join("bad.pgm"), b"P2\n8 8\n255\ninvalid pixels").unwrap();
    fs::write(root.join("small.pgm"), GrayImage::new(8, 8, 100).to_pgm()).unwrap();
    fs::write(root.join("zero.pgm"), GrayImage::new(0, 8, 100).to_pgm()).unwrap();
    for invalid in ["missing.pgm", "bad.pgm", "small.pgm", "zero.pgm"] {
        let output = Command::new(exe)
            .current_dir(&root)
            .args([
                "ssim",
                "--scene",
                "static",
                "--host-size",
                "64x64",
                "--frame",
                "valid.pgm",
                "--frame",
                invalid,
                "--json",
                "scores.json",
            ])
            .output()
            .unwrap();
        assert_eq!(output.status.code(), Some(1), "{invalid}");
        assert!(String::from_utf8_lossy(&output.stderr).contains(invalid));
        assert!(output.stdout.is_empty());
        assert!(!root.join("scores.json").exists());
    }

    let scene = Command::new(exe)
        .args([
            "ssim",
            "--scene",
            "scroll",
            "--host-size",
            "640x360",
            "--frame",
            "missing.pgm",
        ])
        .output()
        .unwrap();
    assert_eq!(scene.status.code(), Some(2));

    assert!(!scene.stderr.is_empty());
    for invalid_size in ["640", "0x360", "640x0", "+640x360", "640X360", "640x360x1"] {
        let size = Command::new(exe)
            .args([
                "ssim",
                "--scene",
                "static",
                "--host-size",
                invalid_size,
                "--frame",
                "missing.pgm",
            ])
            .output()
            .unwrap();
        assert_eq!(size.status.code(), Some(2), "{invalid_size}");
        assert!(!size.stderr.is_empty());
    }
    fs::remove_dir_all(root).unwrap();
}

#[test]
fn perf_print() {
    let reference = render_static(1920, 1080);
    let frame = area_downscale(&reference, 1440, 810);
    let start = Instant::now();
    let score = score_static(&frame, &reference, 2).unwrap();
    eprintln!(
        "perf score_static (1440x810 frame, 1920x1080 host): {:?} ({score:?})",
        start.elapsed()
    );
}
