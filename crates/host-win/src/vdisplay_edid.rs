//! Windows 虚拟显示器的 EDID 1.4 生成与编号（纯逻辑）。M3 · V1。

use host_core::vdisplay::VdMode;

/// The PNP manufacturer code used by the Remote Screen virtual display.
pub const MANUFACTURER: &str = "RSV";
/// Product code stored in the EDID base block.
pub const PRODUCT_CODE: u16 = 0x5653;
/// The largest monitor number represented by the EDID serial field.
pub const MAX_ID: u32 = 99;

const EDID_LEN: usize = 128;
const DTD_OFFSET: usize = 54;
const DESCRIPTOR_LEN: usize = 18;
const H_FRONT: u64 = 48;
const H_SYNC: u64 = 32;
const H_BACK: u64 = 80;
const H_BLANK: u64 = H_FRONT + H_SYNC + H_BACK;
const V_FRONT: u64 = 3;
const V_SYNC: u64 = 5;
const V_BLANK_MIN: u64 = 23;
const V_BLANK_DIVISOR: u64 = 40;
const MAX_DTD_FIELD: u64 = 4095;
const MAX_CLOCK_10KHZ: u64 = u16::MAX as u64;

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum EdidError {
    #[error("monitor id {0} out of range 1..=99")]
    BadId(u32),
    #[error("mode {width}x{height} cannot be described by an EDID detailed timing")]
    TooLarge { width: u32, height: u32 },
}

/// Return the EDID monitor name for an id.
pub fn monitor_name(id: u32) -> String {
    format!("RS Virtual {id:02}")
}

/// Allocate the lowest monitor id which is not present in `used`.
pub fn alloc_id(used: &[u32]) -> Option<u32> {
    (1..=MAX_ID).find(|id| !used.contains(id))
}

/// Return the mode list sent to the Windows display driver.
pub fn mode_list(mode: &VdMode) -> Vec<(u32, u32, u32)> {
    let mut modes = vec![(mode.width_px, mode.height_px, mode.refresh_hz)];
    if mode.refresh_hz != 60 {
        modes.push((mode.width_px, mode.height_px, 60));
    }
    modes
}

/// Build an EDID 1.4 base block for a virtual monitor.
pub fn build_edid(id: u32, mode: &VdMode) -> Result<[u8; 128], EdidError> {
    if !(1..=MAX_ID).contains(&id) {
        return Err(EdidError::BadId(id));
    }

    let dimensions = dtd_dimensions(mode)?;
    let (width_mm, height_mm) = physical_size_mm(mode);
    let width_cm = edid_centimetres(width_mm);
    let height_cm = edid_centimetres(height_mm);

    let mut edid = [0_u8; EDID_LEN];
    edid[..8].copy_from_slice(&[0x00, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0x00]);
    edid[8..10].copy_from_slice(&manufacturer_code(MANUFACTURER));
    edid[10..12].copy_from_slice(&PRODUCT_CODE.to_le_bytes());
    edid[12..16].copy_from_slice(&id.to_le_bytes());
    edid[16] = 0;
    edid[17] = 36;
    edid[18] = 1;
    edid[19] = 4;
    edid[20] = 0xa5;
    edid[21] = width_cm;
    edid[22] = height_cm;
    edid[23] = 0x78;
    edid[24] = 0x06;
    edid[25..35].copy_from_slice(&[0xee, 0x91, 0xa3, 0x54, 0x4c, 0x99, 0x26, 0x0f, 0x50, 0x54]);
    edid[35..38].fill(0);
    edid[38..54].fill(0x01);

    encode_dtd(
        &mut edid[DTD_OFFSET..DTD_OFFSET + DESCRIPTOR_LEN],
        &dimensions,
    );

    let name_offset = DTD_OFFSET + DESCRIPTOR_LEN;
    edid[name_offset..name_offset + 5].copy_from_slice(&[0, 0, 0, 0xfc, 0]);
    edid[name_offset + 5..name_offset + DESCRIPTOR_LEN]
        .copy_from_slice(monitor_name(id).as_bytes());

    for descriptor_offset in [
        name_offset + DESCRIPTOR_LEN,
        name_offset + 2 * DESCRIPTOR_LEN,
    ] {
        edid[descriptor_offset..descriptor_offset + 5].copy_from_slice(&[0, 0, 0, 0x10, 0]);
        edid[descriptor_offset + 5..descriptor_offset + DESCRIPTOR_LEN].fill(0);
    }

    edid[126] = 0;
    let checksum = edid[..127]
        .iter()
        .fold(0_u8, |sum, byte| sum.wrapping_add(*byte));
    edid[127] = 0_u8.wrapping_sub(checksum);
    Ok(edid)
}

/// Check the checksum of an EDID base block.
pub fn checksum_ok(edid: &[u8]) -> bool {
    edid.len() >= EDID_LEN
        && edid[..EDID_LEN]
            .iter()
            .fold(0_u8, |sum, byte| sum.wrapping_add(*byte))
            == 0
}

/// Read the monitor serial number from an EDID base block.
pub fn edid_serial(edid: &[u8]) -> Option<u32> {
    if edid.len() < 16 {
        return None;
    }
    Some(u32::from_le_bytes(edid[12..16].try_into().ok()?))
}

/// Read the first monitor-name descriptor from an EDID base block.
pub fn edid_name(edid: &[u8]) -> Option<String> {
    if edid.len() < EDID_LEN {
        return None;
    }

    let descriptor = (DTD_OFFSET..126)
        .step_by(DESCRIPTOR_LEN)
        .map(|offset| &edid[offset..offset + DESCRIPTOR_LEN])
        .find(|descriptor| descriptor[3] == 0xfc)?;
    let mut name = &descriptor[5..18];
    if let Some(end) = name.iter().position(|byte| *byte == 0x0a) {
        name = &name[..end];
    }
    while matches!(name.last(), Some(0x00) | Some(0x20)) {
        name = &name[..name.len() - 1];
    }
    String::from_utf8(name.to_vec()).ok()
}

/// Decode the first detailed timing descriptor.
pub fn preferred_mode(edid: &[u8]) -> Option<(u32, u32, u32)> {
    if edid.len() < EDID_LEN {
        return None;
    }

    let dtd = (DTD_OFFSET..126)
        .step_by(DESCRIPTOR_LEN)
        .map(|offset| &edid[offset..offset + DESCRIPTOR_LEN])
        .find(|descriptor| descriptor[0] != 0 || descriptor[1] != 0)?;
    let clock_10khz = u64::from(u16::from_le_bytes([dtd[0], dtd[1]]));

    let h_active = u64::from(dtd[2]) | (u64::from(dtd[4] >> 4) << 8);
    let h_blank = u64::from(dtd[3]) | (u64::from(dtd[4] & 0x0f) << 8);
    let v_active = u64::from(dtd[5]) | (u64::from(dtd[7] >> 4) << 8);
    let v_blank = u64::from(dtd[6]) | (u64::from(dtd[7] & 0x0f) << 8);
    let h_total = h_active.checked_add(h_blank)?;
    let v_total = v_active.checked_add(v_blank)?;
    let total = h_total.checked_mul(v_total)?;
    if total == 0 {
        return None;
    }
    let numerator = clock_10khz.checked_mul(10_000)?;
    let refresh = numerator.checked_add(total / 2)? / total;
    let refresh = u32::try_from(refresh).ok()?;
    Some((
        u32::try_from(h_active).ok()?,
        u32::try_from(v_active).ok()?,
        refresh,
    ))
}

#[derive(Debug, Clone, Copy)]
struct DtdDimensions {
    clock_10khz: u64,
    h_active: u64,
    h_blank: u64,
    v_active: u64,
    v_blank: u64,
    h_mm: u64,
    v_mm: u64,
}

fn dtd_dimensions(mode: &VdMode) -> Result<DtdDimensions, EdidError> {
    let h_active = u64::from(mode.width_px);
    let v_active = u64::from(mode.height_px);
    if h_active > MAX_DTD_FIELD || v_active > MAX_DTD_FIELD {
        return Err(EdidError::TooLarge {
            width: mode.width_px,
            height: mode.height_px,
        });
    }

    let h_total = h_active.checked_add(H_BLANK).ok_or(EdidError::TooLarge {
        width: mode.width_px,
        height: mode.height_px,
    })?;
    let v_blank = V_BLANK_MIN.max(v_active.div_ceil(V_BLANK_DIVISOR));
    let v_total = v_active.checked_add(v_blank).ok_or(EdidError::TooLarge {
        width: mode.width_px,
        height: mode.height_px,
    })?;
    let clock_10khz =
        clock_for(h_total, v_total, u64::from(mode.refresh_hz)).ok_or(EdidError::TooLarge {
            width: mode.width_px,
            height: mode.height_px,
        })?;
    let clock_10khz = if clock_10khz <= MAX_CLOCK_10KHZ {
        clock_10khz
    } else {
        clock_for(h_total, v_total, 30)
            .filter(|clock| *clock <= MAX_CLOCK_10KHZ)
            .ok_or(EdidError::TooLarge {
                width: mode.width_px,
                height: mode.height_px,
            })?
    };

    let (width_mm, height_mm) = physical_size_mm(mode);
    Ok(DtdDimensions {
        clock_10khz,
        h_active,
        h_blank: H_BLANK,
        v_active,
        v_blank,
        h_mm: u64::from(width_mm.min(MAX_DTD_FIELD as u32)),
        v_mm: u64::from(height_mm.min(MAX_DTD_FIELD as u32)),
    })
}

fn clock_for(h_total: u64, v_total: u64, refresh_hz: u64) -> Option<u64> {
    let pixels = h_total.checked_mul(v_total)?;
    let numerator = pixels.checked_mul(refresh_hz)?;
    (numerator / 10_000).checked_add(u64::from(numerator % 10_000 != 0))
}

fn encode_dtd(dst: &mut [u8], dtd: &DtdDimensions) {
    debug_assert_eq!(dst.len(), DESCRIPTOR_LEN);
    dst.fill(0);
    dst[0..2].copy_from_slice(&(dtd.clock_10khz as u16).to_le_bytes());
    dst[2] = dtd.h_active as u8;
    dst[3] = dtd.h_blank as u8;
    dst[4] = ((dtd.h_active >> 8) as u8) << 4 | (dtd.h_blank >> 8) as u8;
    dst[5] = dtd.v_active as u8;
    dst[6] = dtd.v_blank as u8;
    dst[7] = ((dtd.v_active >> 8) as u8) << 4 | (dtd.v_blank >> 8) as u8;
    dst[8] = H_FRONT as u8;
    dst[9] = H_SYNC as u8;
    dst[10] = (V_FRONT as u8) << 4 | V_SYNC as u8;
    dst[11] = ((H_FRONT >> 8) as u8) << 6
        | ((H_SYNC >> 8) as u8) << 4
        | ((V_FRONT >> 4) as u8) << 2
        | (V_SYNC >> 4) as u8;
    dst[12] = dtd.h_mm as u8;
    dst[13] = dtd.v_mm as u8;
    dst[14] = ((dtd.h_mm >> 8) as u8) << 4 | (dtd.v_mm >> 8) as u8;
    dst[17] = 0x1a;
}

fn manufacturer_code(manufacturer: &str) -> [u8; 2] {
    let bytes = manufacturer.as_bytes();
    let first = bytes.first().copied().unwrap_or(b'A').saturating_sub(b'A') + 1;
    let second = bytes.get(1).copied().unwrap_or(b'A').saturating_sub(b'A') + 1;
    let third = bytes.get(2).copied().unwrap_or(b'A').saturating_sub(b'A') + 1;
    let code = (u16::from(first) << 10) | (u16::from(second) << 5) | u16::from(third);
    code.to_be_bytes()
}

fn edid_centimetres(mm: u32) -> u8 {
    let rounded = (u64::from(mm) + 5) / 10;
    rounded.clamp(1, u64::from(u8::MAX)) as u8
}

// Keep this calculation aligned with host_core::vdisplay::mode::physical_size_mm.
// Keep the local compatibility copy until that pre-seeded module is merged into this
// branch; it also keeps this pure EDID module buildable in isolation.
fn physical_size_mm(mode: &VdMode) -> (u32, u32) {
    let scale = f64::from(mode.scale.max(1.0));
    let width = (f64::from(mode.width_px) * 25.4 / (96.0 * scale)).round() as u32;
    let height = (f64::from(mode.height_px) * 25.4 / (96.0 * scale)).round() as u32;
    (width, height)
}
