//! Software H.264 fallback using Cisco's OpenH264 library.
//!
//! Capture frames are BGRx.  OpenH264 accepts planar I420, so this module
//! keeps the conversion and the encoder wrapper together.  The OpenH264 API
//! is supplied by a loader because production loads Cisco's shared object,
//! while tests can opt into the crate's source-built implementation.

use std::path::{Path, PathBuf};
use std::sync::Arc;

use host_core::encode_model::{au_is_key, EncParams, RateCost};
use openh264::encoder::{
    BitRate, Encoder, EncoderConfig, FrameRate, IntraFramePeriod, RateControlMode, UsageType,
};
use openh264::formats::YUVSource;
use rsp_core::pb::Codec;

use crate::enc_worker::{BackendError, BackendFactory, EncBackend, EncPacket, SrcSize};
use crate::ff_model::LinuxBackend;
use crate::frame::LinuxFrame;

/// A planar I420 image owned by the encoder.
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct I420 {
    pub width: u32,
    pub height: u32,
    pub y: Vec<u8>,
    pub u: Vec<u8>,
    pub v: Vec<u8>,
}

impl I420 {
    fn new(width: u32, height: u32) -> Self {
        let y_len = width as usize * height as usize;
        let uv_len = (width as usize / 2) * (height as usize / 2);
        Self {
            width,
            height,
            y: vec![0; y_len],
            u: vec![0; uv_len],
            v: vec![0; uv_len],
        }
    }

    fn resize(&mut self, width: u32, height: u32) {
        self.width = width;
        self.height = height;
        let y_len = width as usize * height as usize;
        let uv_len = (width as usize / 2) * (height as usize / 2);
        self.y.resize(y_len, 0);
        self.u.resize(uv_len, 0);
        self.v.resize(uv_len, 0);
    }
}

impl YUVSource for I420 {
    fn dimensions(&self) -> (usize, usize) {
        (self.width as usize, self.height as usize)
    }

    fn strides(&self) -> (usize, usize, usize) {
        (
            self.width as usize,
            self.width as usize / 2,
            self.width as usize / 2,
        )
    }

    fn y(&self) -> &[u8] {
        &self.y
    }

    fn u(&self) -> &[u8] {
        &self.u
    }

    fn v(&self) -> &[u8] {
        &self.v
    }
}

/// Scale a BGRx image by taking the area average for every destination pixel.
///
/// The destination is tightly packed (`dw * 4` bytes per row).  Invalid or
/// zero-sized buffers are ignored; valid capture buffers are expected to have
/// `sstride >= sw * 4` and enough bytes for all rows.
pub fn scale_bgrx(src: &[u8], sw: u32, sh: u32, sstride: usize, dst: &mut [u8], dw: u32, dh: u32) {
    if sw == 0 || sh == 0 || dw == 0 || dh == 0 {
        return;
    }
    let sw_usize = sw as usize;
    let sh_usize = sh as usize;
    let dw_usize = dw as usize;
    let dh_usize = dh as usize;
    let src_row_len = match sw_usize.checked_mul(4) {
        Some(value) if sstride >= value => value,
        _ => return,
    };
    let src_need = match sstride
        .checked_mul(sh_usize.saturating_sub(1))
        .and_then(|value| value.checked_add(src_row_len))
    {
        Some(value) if value <= src.len() => value,
        _ => return,
    };
    let dst_need = match dw_usize
        .checked_mul(dh_usize)
        .and_then(|value| value.checked_mul(4))
    {
        Some(value) if value <= dst.len() => value,
        _ => return,
    };
    let _ = (src_need, dst_need);

    for y in 0..dh_usize {
        let y0 = (y as u64 * sh as u64 / dh as u64) as usize;
        let y1_formula = ((y as u64 + 1) * sh as u64 / dh as u64) as usize;
        let y1 = y1_formula.max(y0.saturating_add(1)).min(sh_usize);
        for x in 0..dw_usize {
            let x0 = (x as u64 * sw as u64 / dw as u64) as usize;
            let x1_formula = ((x as u64 + 1) * sw as u64 / dw as u64) as usize;
            let x1 = x1_formula.max(x0.saturating_add(1)).min(sw_usize);
            let count = (x1 - x0) * (y1 - y0);
            if count == 0 {
                continue;
            }
            let mut sums = [0u64; 4];
            for sy in y0..y1 {
                let row_start = sy * sstride;
                for sx in x0..x1 {
                    let pixel_start = row_start + sx * 4;
                    for channel in 0..4 {
                        sums[channel] += u64::from(src[pixel_start + channel]);
                    }
                }
            }
            let dst_start = (y * dw_usize + x) * 4;
            for channel in 0..4 {
                dst[dst_start + channel] =
                    ((sums[channel] + (count as u64 / 2)) / count as u64) as u8;
            }
        }
    }
}

fn source_pixel(src: &[u8], stride: usize, x: usize, y: usize) -> (u8, u8, u8) {
    let Some(x_offset) = x.checked_mul(4) else {
        return (0, 0, 0);
    };
    let start = match y
        .checked_mul(stride)
        .and_then(|row| row.checked_add(x_offset))
    {
        Some(start) => start,
        None => return (0, 0, 0),
    };
    let Some(pixel) = start.checked_add(4).and_then(|end| src.get(start..end)) else {
        return (0, 0, 0);
    };
    (pixel[0], pixel[1], pixel[2])
}

fn round_div_i64(numerator: i64, denominator: i64) -> i64 {
    if numerator >= 0 {
        (numerator + denominator / 2) / denominator
    } else {
        -((-numerator + denominator / 2) / denominator)
    }
}

fn clamp_u8(value: i64) -> u8 {
    value.clamp(0, 255) as u8
}

/// Convert BGRx pixels to BT.709 limited-range planar I420.
///
/// The luma coefficients are rounded per pixel.  Chroma is accumulated over
/// each 2x2 block before rounding, which avoids bias on mixed-colour blocks.
pub fn bgrx_to_i420(src: &[u8], stride: usize, w: u32, h: u32, out: &mut I420) {
    out.resize(w, h);
    if w == 0 || h == 0 {
        return;
    }
    let width = w as usize;
    let height = h as usize;
    const SCALE: i64 = 10_000;
    const Y_DEN: i64 = 255 * SCALE;
    const C_DEN: i64 = 255 * SCALE;
    for y in 0..height {
        for x in 0..width {
            let (b, g, r) = source_pixel(src, stride, x, y);
            let y_num = 219 * (2_126 * i64::from(r) + 7_152 * i64::from(g) + 722 * i64::from(b));
            out.y[y * width + x] = clamp_u8(16 + round_div_i64(y_num, Y_DEN));
        }
    }

    let chroma_width = width / 2;
    let chroma_height = height / 2;
    for cy in 0..chroma_height {
        for cx in 0..chroma_width {
            let mut cb_num = 0i64;
            let mut cr_num = 0i64;
            for dy in 0..2 {
                for dx in 0..2 {
                    let (b, g, r) = source_pixel(src, stride, cx * 2 + dx, cy * 2 + dy);
                    cb_num += -1_146 * i64::from(r) - 3_854 * i64::from(g) + 5_000 * i64::from(b);
                    cr_num += 5_000 * i64::from(r) - 4_542 * i64::from(g) - 458 * i64::from(b);
                }
            }
            // Four pixels are averaged before applying the 224/255 scale.
            let denominator = C_DEN * 4;
            let index = cy * chroma_width + cx;
            out.u[index] = clamp_u8(128 + round_div_i64(224 * cb_num, denominator));
            out.v[index] = clamp_u8(128 + round_div_i64(224 * cr_num, denominator));
        }
    }
}

/// A factory for OpenH264 backends.
pub type ApiLoader = Arc<dyn Fn() -> Result<openh264::OpenH264API, String> + Send + Sync>;

pub struct OpenH264Factory {
    loader: ApiLoader,
}

impl OpenH264Factory {
    pub fn with_loader(loader: ApiLoader) -> Self {
        Self { loader }
    }

    /// Build a factory that loads a Cisco OpenH264 shared object on demand.
    pub fn from_blob(path: PathBuf) -> Self {
        Self::with_loader(Arc::new(move || {
            openh264::OpenH264API::from_blob_path(&path).map_err(|error| error.to_string())
        }))
    }

    /// Return the configured blob path, or the two platform default locations.
    pub fn blob_candidates(exe_dir: &Path, state_dir: &Path, env: Option<&str>) -> Vec<PathBuf> {
        if let Some(path) = env.map(str::trim).filter(|path| !path.is_empty()) {
            return vec![PathBuf::from(path)];
        }
        vec![
            exe_dir
                .join("..")
                .join("lib")
                .join("openh264")
                .join("libopenh264.so.7"),
            state_dir.join("codecs").join("libopenh264.so.7"),
        ]
    }
}

impl BackendFactory for OpenH264Factory {
    fn name(&self) -> &'static str {
        "openh264"
    }

    fn open(&self, params: &EncParams, src: SrcSize) -> Result<Box<dyn EncBackend>, BackendError> {
        if params.codec != Codec::H264 {
            return Err(BackendError::Unsupported(
                "openh264 only encodes H.264".into(),
            ));
        }
        let api = (self.loader)().map_err(BackendError::Unsupported)?;
        let bitrate = u32::try_from(params.bitrate_bps).map_err(|_| {
            BackendError::Failed("openh264 bitrate exceeds its u32 configuration limit".into())
        })?;
        let config = EncoderConfig::new()
            .bitrate(BitRate::from_bps(bitrate))
            .max_frame_rate(FrameRate::from_hz(params.fps as f32))
            .usage_type(UsageType::ScreenContentRealTime)
            .rate_control_mode(RateControlMode::Bitrate)
            .skip_frames(false)
            .intra_frame_period(IntraFramePeriod::from_num_frames(0));
        let encoder = Encoder::with_api_config(api, config)
            .map_err(|error| BackendError::Failed(error.to_string()))?;
        Ok(Box::new(OpenH264Backend {
            encoder,
            width: params.width,
            height: params.height,
            scaled_bgrx: if src.width != params.width || src.height != params.height {
                vec![0; params.width as usize * params.height as usize * 4]
            } else {
                Vec::new()
            },
            i420: I420::new(params.width, params.height),
            initialized: false,
        }))
    }
}

pub struct OpenH264Backend {
    encoder: Encoder,
    width: u32,
    height: u32,
    scaled_bgrx: Vec<u8>,
    i420: I420,
    initialized: bool,
}

impl EncBackend for OpenH264Backend {
    fn name(&self) -> &'static str {
        "openh264"
    }

    fn rate_cost(&self) -> RateCost {
        LinuxBackend::OpenH264.rate_cost()
    }

    fn encode(
        &mut self,
        frame: &LinuxFrame,
        pts: i64,
        idr: bool,
    ) -> Result<Vec<EncPacket>, BackendError> {
        let (pixels, stride) = if frame.width == self.width && frame.height == self.height {
            (&frame.pixels[..], frame.stride as usize)
        } else {
            let required = self.width as usize * self.height as usize * 4;
            if self.scaled_bgrx.len() != required {
                self.scaled_bgrx.resize(required, 0);
            }
            scale_bgrx(
                &frame.pixels,
                frame.width,
                frame.height,
                frame.stride as usize,
                &mut self.scaled_bgrx,
                self.width,
                self.height,
            );
            (&self.scaled_bgrx[..], self.width as usize * 4)
        };
        bgrx_to_i420(pixels, stride, self.width, self.height, &mut self.i420);
        // The first call initializes OpenH264 and is an IDR by definition;
        // ForceIntraFrame is only valid after that initialization.
        if idr && self.initialized {
            self.encoder.force_intra_frame();
        }
        let data = self
            .encoder
            .encode(&self.i420)
            .map_err(|error| BackendError::Failed(error.to_string()))?
            .to_vec();
        self.initialized = true;
        if data.is_empty() {
            return Ok(Vec::new());
        }
        Ok(vec![EncPacket {
            keyframe: au_is_key(Codec::H264, &data),
            data,
            pts,
        }])
    }

    fn set_rate(&mut self, _bitrate_bps: u64, _fps: u32) -> Result<(), BackendError> {
        Ok(())
    }
}
