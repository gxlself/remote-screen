//! Platform independent geometry and Media Foundation policy for W3.
//!
//! `rotation` is the clockwise display rotation reported by DXGI. `R90` means
//! that the Video Processor rotates the input clockwise by 90 degrees. The
//! direction is calibrated by the hardware probe on a real portrait
//! display; if a driver reports the opposite direction only the 90/270 mapping
//! in [`vp_rotation`] needs changing.

use rsp_core::pb::Codec;

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct RectU {
    pub left: u32,
    pub top: u32,
    pub right: u32,
    pub bottom: u32,
}

/// Round a dimension down to an even value.
pub fn even_down(v: u32) -> u32 {
    v & !1
}

pub fn rotated_size(width: u32, height: u32, rotation: u32) -> (u32, u32) {
    match rotation {
        90 | 270 => (height, width),
        _ => (width, height),
    }
}

/// Fit a source rectangle into a destination while preserving aspect ratio.
///
/// The multiplication is intentionally performed in `u64`: monitor and
/// encoder dimensions are `u32`, and comparing their products in `u32` can
/// overflow before the aspect-ratio decision is made.
pub fn fit_rect(src_w: u32, src_h: u32, dst_w: u32, dst_h: u32) -> RectU {
    if src_w == 0 || src_h == 0 || dst_w == 0 || dst_h == 0 {
        return RectU {
            left: 0,
            top: 0,
            right: dst_w,
            bottom: dst_h,
        };
    }

    let (width, height, left, top) =
        if u64::from(src_w) * u64::from(dst_h) >= u64::from(dst_w) * u64::from(src_h) {
            let width = dst_w.max(2);
            let height =
                even_down(((u64::from(src_h) * u64::from(dst_w)) / u64::from(src_w)) as u32).max(2);
            let top = even_down(dst_h.saturating_sub(height) / 2);
            (width, height, 0, top)
        } else {
            let height = dst_h.max(2);
            let width =
                even_down(((u64::from(src_w) * u64::from(dst_h)) / u64::from(src_h)) as u32).max(2);
            let left = even_down(dst_w.saturating_sub(width) / 2);
            (width, height, left, 0)
        };

    RectU {
        left,
        top,
        right: left.saturating_add(width),
        bottom: top.saturating_add(height),
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum VpRotation {
    Identity,
    R90,
    R180,
    R270,
}

pub fn vp_rotation(rotation: u32) -> VpRotation {
    match rotation {
        90 => VpRotation::R90,
        180 => VpRotation::R180,
        270 => VpRotation::R270,
        _ => VpRotation::Identity,
    }
}

/// Numeric values of `D3D11_VIDEO_PROCESSOR_ROTATION` in the Windows SDK.
pub fn vp_rotation_value(rotation: VpRotation) -> i32 {
    match rotation {
        VpRotation::Identity => 0,
        VpRotation::R90 => 1,
        VpRotation::R180 => 2,
        VpRotation::R270 => 3,
    }
}

/// DXGI color-space values used by the D3D11.1 Video Processor API.
pub const VP_INPUT_COLOR_SPACE: i32 = 0; // RGB_FULL_G22_NONE_P709
pub const VP_OUTPUT_COLOR_SPACE: i32 = 8; // YCBCR_STUDIO_G22_LEFT_P709

/// Pack the legacy `D3D11_VIDEO_PROCESSOR_COLOR_SPACE` bit fields.
pub fn legacy_color_space(rgb_full: bool, ycbcr_bt709: bool, nominal_range: u32) -> u32 {
    (u32::from(!rgb_full) << 1) | (u32::from(ycbcr_bt709) << 2) | ((nominal_range & 3) << 4)
}

pub const VP_INPUT_LEGACY: u32 = 0x04;
pub const VP_OUTPUT_LEGACY: u32 = 0x14;
pub const BG_YCBCRA: [f32; 4] = [16.0 / 255.0, 128.0 / 255.0, 128.0 / 255.0, 1.0];

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct VpKey {
    pub device_gen: u64,
    pub in_w: u32,
    pub in_h: u32,
    pub out_w: u32,
    pub out_h: u32,
    pub rotation: u32,
}

pub const VIEW_CACHE_MAX: usize = 32;

/// A small linear cache. The caches hold COM views in the Windows layer, so a
/// bounded, deterministic eviction policy is preferable to an unbounded map.
pub struct ViewCache<K, V> {
    max: usize,
    entries: Vec<(K, V)>,
}

impl<K, V> ViewCache<K, V> {
    pub fn new(max: usize) -> Self {
        Self {
            max,
            entries: Vec::new(),
        }
    }

    pub fn len(&self) -> usize {
        self.entries.len()
    }

    pub fn is_empty(&self) -> bool {
        self.entries.is_empty()
    }

    pub fn clear(&mut self) {
        self.entries.clear();
    }
}

impl<K: PartialEq, V> ViewCache<K, V> {
    pub fn get_or_try_insert<E>(
        &mut self,
        key: K,
        make: impl FnOnce() -> Result<V, E>,
    ) -> Result<&V, E> {
        if let Some(index) = self.entries.iter().position(|(cached, _)| *cached == key) {
            return Ok(&self.entries[index].1);
        }
        if self.entries.len() >= self.max {
            self.clear();
        }
        let value = make()?;
        self.entries.push((key, value));
        // The push above guarantees an element exists. Looking it up again
        // keeps the returned borrow independent of the closure's temporary.
        Ok(&self.entries[self.entries.len() - 1].1)
    }

    /// Short alias used by the Windows view setup code and model tests.
    pub fn get<E>(&mut self, key: K, make: impl FnOnce() -> Result<V, E>) -> Result<&V, E> {
        self.get_or_try_insert(key, make)
    }
}

pub fn luid_parts(luid: i64) -> (u32, i32) {
    ((luid as u64 & 0xffff_ffff) as u32, (luid >> 32) as i32)
}

pub fn luid_blob(luid: i64) -> [u8; 8] {
    let (low, high) = luid_parts(luid);
    let mut blob = [0u8; 8];
    blob[..4].copy_from_slice(&low.to_le_bytes());
    blob[4..].copy_from_slice(&high.to_le_bytes());
    blob
}

pub fn mf_vendor_attr(vendor_id: u32) -> String {
    format!("VEN_{vendor_id:04X}")
}

pub fn mf_pack(hi: u32, lo: u32) -> u64 {
    (u64::from(hi) << 32) | u64::from(lo)
}

pub fn hns_per_frame(fps: u32) -> i64 {
    if fps == 0 {
        return 0;
    }
    ((10_000_000u64 + u64::from(fps / 2)) / u64::from(fps)) as i64
}

pub fn sample_time(pts: i64, fps: u32) -> i64 {
    if fps == 0 {
        return 0;
    }
    ((i128::from(pts) * 10_000_000 + i128::from(fps / 2)) / i128::from(fps)) as i64
}

pub fn pts_from_sample_time(time: i64, fps: u32) -> i64 {
    if fps == 0 {
        return 0;
    }
    ((i128::from(time) * i128::from(fps) + 5_000_000) / 10_000_000) as i64
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfApi {
    LowLatencyMode,
    RateControlMode,
    MeanBitRate,
    BufferSize,
    GopSize,
    BPictureCount,
    QualityVsSpeed,
    ForceKeyFrame,
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfValue {
    U32(u32),
    Bool(bool),
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct MfProp {
    pub api: MfApi,
    pub value: MfValue,
    pub required: bool,
}

pub const MF_GOP_INFINITE: u32 = 0x7fff_ffff;

fn clamp_u32(value: u64) -> u32 {
    value.min(u64::from(u32::MAX)) as u32
}

fn vbv_size(bitrate_bps: u64, fps: u32) -> u32 {
    if fps == 0 {
        return 0;
    }
    clamp_u32(bitrate_bps.div_ceil(u64::from(fps)))
}

pub fn mf_codecapi(bitrate_bps: u64, fps: u32) -> Vec<MfProp> {
    vec![
        MfProp {
            api: MfApi::LowLatencyMode,
            value: MfValue::Bool(true),
            required: true,
        },
        MfProp {
            api: MfApi::RateControlMode,
            value: MfValue::U32(0),
            required: true,
        },
        MfProp {
            api: MfApi::MeanBitRate,
            value: MfValue::U32(clamp_u32(bitrate_bps)),
            required: true,
        },
        MfProp {
            api: MfApi::BufferSize,
            value: MfValue::U32(vbv_size(bitrate_bps, fps)),
            required: false,
        },
        MfProp {
            api: MfApi::GopSize,
            value: MfValue::U32(MF_GOP_INFINITE),
            required: false,
        },
        MfProp {
            api: MfApi::BPictureCount,
            value: MfValue::U32(0),
            required: false,
        },
        MfProp {
            api: MfApi::QualityVsSpeed,
            value: MfValue::U32(0),
            required: false,
        },
    ]
}

pub fn mf_rate_props(bitrate_bps: u64, fps: u32) -> Vec<MfProp> {
    vec![
        MfProp {
            api: MfApi::MeanBitRate,
            value: MfValue::U32(clamp_u32(bitrate_bps)),
            required: true,
        },
        MfProp {
            api: MfApi::BufferSize,
            value: MfValue::U32(vbv_size(bitrate_bps, fps)),
            required: false,
        },
    ]
}

pub fn mf_force_key() -> MfProp {
    MfProp {
        api: MfApi::ForceKeyFrame,
        value: MfValue::U32(1),
        required: true,
    }
}

pub fn mf_profile(codec: Codec) -> Option<u32> {
    match codec {
        Codec::H264 => Some(100),
        Codec::Hevc => Some(1),
        Codec::Unspecified => None,
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfAttr {
    FrameSize(u64),
    FrameRate(u64),
    AvgBitrate(u32),
    Interlace(u32),
    Profile(u32),
    YuvMatrix(u32),
    NominalRange(u32),
    Primaries(u32),
    Transfer(u32),
}

pub fn mf_type_attrs(
    codec: Codec,
    width: u32,
    height: u32,
    fps: u32,
    bitrate_bps: u64,
    output: bool,
) -> Vec<MfAttr> {
    let mut attrs = vec![
        MfAttr::FrameSize(mf_pack(width, height)),
        MfAttr::FrameRate(mf_pack(fps, 1)),
    ];
    if output {
        attrs.push(MfAttr::AvgBitrate(clamp_u32(bitrate_bps)));
    }
    attrs.push(MfAttr::Interlace(2));
    if output {
        if let Some(profile) = mf_profile(codec) {
            attrs.push(MfAttr::Profile(profile));
        }
    }
    attrs.extend([MfAttr::YuvMatrix(1), MfAttr::NominalRange(2)]);
    if output {
        attrs.extend([MfAttr::Primaries(2), MfAttr::Transfer(5)]);
    }
    attrs
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum MfEventKind {
    NeedInput,
    HaveOutput,
    DrainComplete,
    Error,
    Other,
}

pub fn mf_event_kind(met: u32) -> MfEventKind {
    match met {
        601 => MfEventKind::NeedInput,
        602 => MfEventKind::HaveOutput,
        603 => MfEventKind::DrainComplete,
        1 => MfEventKind::Error,
        _ => MfEventKind::Other,
    }
}

#[derive(Debug, Default)]
pub struct MfFlow {
    input: u32,
    output: u32,
}

impl MfFlow {
    pub fn new() -> Self {
        Self::default()
    }

    pub fn on_event(&mut self, kind: MfEventKind) {
        match kind {
            MfEventKind::NeedInput => self.input = self.input.saturating_add(1),
            MfEventKind::HaveOutput => self.output = self.output.saturating_add(1),
            MfEventKind::DrainComplete | MfEventKind::Error | MfEventKind::Other => {}
        }
    }

    pub fn take_input(&mut self) -> bool {
        take_credit(&mut self.input)
    }

    pub fn take_output(&mut self) -> bool {
        take_credit(&mut self.output)
    }

    pub fn reset(&mut self) {
        self.input = 0;
        self.output = 0;
    }
}

fn take_credit(credit: &mut u32) -> bool {
    if *credit == 0 {
        false
    } else {
        *credit -= 1;
        true
    }
}

pub const MF_INPUT_POOL: usize = 4;
pub const MF_ENCODE_WAIT_US: u64 = 30_000;

#[derive(Debug)]
pub struct SurfaceRing {
    slots: Vec<Option<i64>>,
    next: usize,
}

impl SurfaceRing {
    pub fn new(size: usize) -> Self {
        Self {
            slots: vec![None; size],
            next: 0,
        }
    }

    /// Inspect the ring without claiming a slot. `submitted` performs the
    /// state transition after the MFT accepts the sample.
    pub fn acquire(&self) -> Option<usize> {
        if self.slots.is_empty() {
            return None;
        }
        (0..self.slots.len())
            .map(|offset| (self.next + offset) % self.slots.len())
            .find(|&index| self.slots[index].is_none())
    }

    pub fn submitted(&mut self, slot: usize, pts: i64) {
        if slot >= self.slots.len() {
            return;
        }
        self.slots[slot] = Some(pts);
        self.next = (slot + 1) % self.slots.len().max(1);
    }

    pub fn released_through(&mut self, pts: i64) {
        for value in &mut self.slots {
            if value.is_some_and(|submitted| submitted <= pts) {
                *value = None;
            }
        }
    }

    pub fn busy_count(&self) -> usize {
        self.slots.iter().filter(|slot| slot.is_some()).count()
    }

    pub fn reset(&mut self) {
        self.slots.fill(None);
        self.next = 0;
    }
}
