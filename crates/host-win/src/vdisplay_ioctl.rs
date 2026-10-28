//! Windows 虚拟显示器驱动的 IOCTL 协议（纯编解码，全平台编译）。
//! 与 drivers/win-iddcx/include/RsIddIoctl.h 逐字段对应。

use host_core::vdisplay::VdError;

pub const VERSION: u32 = 1;
pub const MAX_MODES: usize = 4;
pub const EDID_LEN: usize = 128;
pub const INTERFACE_GUID: &str = "{13037E20-EC6D-42CD-8293-42F53DA517A6}";
/// `windows::core::GUID::from_values` 的参数。
pub const INTERFACE_GUID_PARTS: (u32, u16, u16, [u8; 8]) = (
    0x13037E20,
    0xEC6D,
    0x42CD,
    [0x82, 0x93, 0x42, 0xF5, 0x3D, 0xA5, 0x17, 0xA6],
);

const VERSION_IN_LEN: usize = 4;
const MODE_LEN: usize = 12;
const INFO_LEN: usize = 48;
const ADD_IN_LEN: usize = 192;
const ADD_OUT_LEN: usize = 8;
const REMOVE_IN_LEN: usize = 8;
const REMOVE_ALL_OUT_LEN: usize = 4;

/// Builds a METHOD_BUFFERED read/write IOCTL for FILE_DEVICE_UNKNOWN.
pub const fn ioctl_code(function: u32) -> u32 {
    (0x22u32 << 16) | (3u32 << 14) | (function << 2)
}

pub const IOCTL_GET_INFO: u32 = ioctl_code(0x900);
pub const IOCTL_ADD: u32 = ioctl_code(0x901);
pub const IOCTL_REMOVE: u32 = ioctl_code(0x902);
pub const IOCTL_REMOVE_ALL: u32 = ioctl_code(0x903);
pub const IOCTL_PING: u32 = ioctl_code(0x904);

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct Mode {
    pub width: u32,
    pub height: u32,
    pub refresh_hz: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct Info {
    pub version: u32,
    pub max_monitors: u32,
    pub max_long_px: u32,
    pub max_short_px: u32,
    pub min_long_px: u32,
    pub min_short_px: u32,
    pub refresh_rates: Vec<u32>,
    pub active_monitors: u32,
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct AddIn {
    pub monitor_id: u32,
    pub modes: Vec<Mode>,
    pub edid: [u8; EDID_LEN],
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub struct AddOut {
    pub monitor_id: u32,
    pub connector_index: u32,
}

#[derive(Debug, Clone, PartialEq, Eq, thiserror::Error)]
pub enum IoctlError {
    #[error("short buffer: need {need}, got {got}")]
    Short { need: usize, got: usize },
    #[error("unsupported driver protocol version {0}")]
    Version(u32),
    #[error("too many modes: {0}")]
    TooManyModes(usize),
    #[error("no modes")]
    NoModes,
    #[error("bad refresh count {0}")]
    RefreshCount(u32),
}

const fn read_u32(buf: &[u8], offset: usize) -> u32 {
    u32::from_le_bytes([
        buf[offset],
        buf[offset + 1],
        buf[offset + 2],
        buf[offset + 3],
    ])
}

fn write_u32(buf: &mut [u8], offset: usize, value: u32) {
    buf[offset..offset + 4].copy_from_slice(&value.to_le_bytes());
}

fn require_len(buf: &[u8], need: usize) -> Result<(), IoctlError> {
    if buf.len() < need {
        Err(IoctlError::Short {
            need,
            got: buf.len(),
        })
    } else {
        Ok(())
    }
}

pub fn version_in() -> [u8; VERSION_IN_LEN] {
    VERSION.to_le_bytes()
}

pub fn encode_add(add: &AddIn) -> Result<[u8; ADD_IN_LEN], IoctlError> {
    if add.modes.is_empty() {
        return Err(IoctlError::NoModes);
    }
    if add.modes.len() > MAX_MODES {
        return Err(IoctlError::TooManyModes(add.modes.len()));
    }

    let mut out = [0u8; ADD_IN_LEN];
    write_u32(&mut out, 0, VERSION);
    write_u32(&mut out, 4, add.monitor_id);
    write_u32(&mut out, 8, add.modes.len() as u32);
    for (index, mode) in add.modes.iter().enumerate() {
        let offset = 12 + index * MODE_LEN;
        write_u32(&mut out, offset, mode.width);
        write_u32(&mut out, offset + 4, mode.height);
        write_u32(&mut out, offset + 8, mode.refresh_hz);
    }
    write_u32(&mut out, 60, EDID_LEN as u32);
    out[64..64 + EDID_LEN].copy_from_slice(&add.edid);
    Ok(out)
}

pub fn decode_add(buf: &[u8]) -> Result<AddIn, IoctlError> {
    require_len(buf, ADD_IN_LEN)?;
    let version = read_u32(buf, 0);
    if version != VERSION {
        return Err(IoctlError::Version(version));
    }

    let mode_count = read_u32(buf, 8);
    if mode_count == 0 {
        return Err(IoctlError::NoModes);
    }
    if mode_count > MAX_MODES as u32 {
        return Err(IoctlError::TooManyModes(mode_count as usize));
    }

    let mut modes = Vec::with_capacity(mode_count as usize);
    for index in 0..mode_count as usize {
        let offset = 12 + index * MODE_LEN;
        modes.push(Mode {
            width: read_u32(buf, offset),
            height: read_u32(buf, offset + 4),
            refresh_hz: read_u32(buf, offset + 8),
        });
    }

    let mut edid = [0u8; EDID_LEN];
    edid.copy_from_slice(&buf[64..64 + EDID_LEN]);
    Ok(AddIn {
        monitor_id: read_u32(buf, 4),
        modes,
        edid,
    })
}

pub fn decode_add_out(buf: &[u8]) -> Result<AddOut, IoctlError> {
    require_len(buf, ADD_OUT_LEN)?;
    Ok(AddOut {
        monitor_id: read_u32(buf, 0),
        connector_index: read_u32(buf, 4),
    })
}

/// Encodes at most the first four refresh rates into the fixed-size payload.
pub fn encode_info(info: &Info) -> [u8; INFO_LEN] {
    let mut out = [0u8; INFO_LEN];
    write_u32(&mut out, 0, info.version);
    write_u32(&mut out, 4, info.max_monitors);
    write_u32(&mut out, 8, info.max_long_px);
    write_u32(&mut out, 12, info.max_short_px);
    write_u32(&mut out, 16, info.min_long_px);
    write_u32(&mut out, 20, info.min_short_px);
    let refresh_count = info.refresh_rates.len().min(MAX_MODES);
    write_u32(&mut out, 24, refresh_count as u32);
    for (index, refresh) in info.refresh_rates.iter().take(MAX_MODES).enumerate() {
        write_u32(&mut out, 28 + index * 4, *refresh);
    }
    write_u32(&mut out, 44, info.active_monitors);
    out
}

pub fn decode_info(buf: &[u8]) -> Result<Info, IoctlError> {
    require_len(buf, INFO_LEN)?;
    let version = read_u32(buf, 0);
    if version != VERSION {
        return Err(IoctlError::Version(version));
    }

    let refresh_count = read_u32(buf, 24);
    if refresh_count > MAX_MODES as u32 {
        return Err(IoctlError::RefreshCount(refresh_count));
    }
    let mut refresh_rates = Vec::with_capacity(refresh_count as usize);
    for index in 0..refresh_count as usize {
        refresh_rates.push(read_u32(buf, 28 + index * 4));
    }
    Ok(Info {
        version,
        max_monitors: read_u32(buf, 4),
        max_long_px: read_u32(buf, 8),
        max_short_px: read_u32(buf, 12),
        min_long_px: read_u32(buf, 16),
        min_short_px: read_u32(buf, 20),
        refresh_rates,
        active_monitors: read_u32(buf, 44),
    })
}

pub fn encode_remove(monitor_id: u32) -> [u8; REMOVE_IN_LEN] {
    let mut out = [0u8; REMOVE_IN_LEN];
    write_u32(&mut out, 0, VERSION);
    write_u32(&mut out, 4, monitor_id);
    out
}

pub fn decode_remove_all_out(buf: &[u8]) -> Result<u32, IoctlError> {
    require_len(buf, REMOVE_ALL_OUT_LEN)?;
    Ok(read_u32(buf, 0))
}

/// DeviceIoControl 失败时 GetLastError 的值到 `VdError` 的映射。
pub fn map_win32_error(code: u32) -> VdError {
    match code {
        1816 => VdError::Limit,
        21 => VdError::Unsupported("virtual display adapter not ready".to_owned()),
        2 | 3 => VdError::Unsupported("virtual display driver not installed".to_owned()),
        5 => VdError::Unsupported("virtual display driver access denied".to_owned()),
        87 => VdError::Failed("driver rejected parameters".to_owned()),
        183 => VdError::Failed("monitor id already in use".to_owned()),
        1168 => VdError::Failed("monitor not found".to_owned()),
        other => VdError::Failed(format!("driver error {other}")),
    }
}

/// REMOVE 时「找不到」不算错（destroy 幂等）。
pub const fn is_not_found(code: u32) -> bool {
    code == 1168
}
