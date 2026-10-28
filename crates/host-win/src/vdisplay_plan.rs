//! Windows 虚拟屏服务侧的纯逻辑（M3 · V1）：能力换算、ADD 请求构造、显示器名合并。

use host_core::vdisplay::{VdCaps, VdError, VdMode};

use crate::{
    vdisplay_edid,
    vdisplay_ioctl::{AddIn, Info, Mode},
};

/// Convert the driver's GET_INFO response to the host-independent capability type.
pub fn caps_from_info(info: &Info) -> VdCaps {
    VdCaps {
        max_long_px: info.max_long_px,
        max_short_px: info.max_short_px,
        min_long_px: info.min_long_px,
        min_short_px: info.min_short_px,
        refresh_rates: info.refresh_rates.clone(),
        scales: vec![1.0, 1.5, 2.0],
        align_px: 8,
        max_displays: info.max_monitors.min(4),
    }
}

/// Construct the driver's ADD request, including its generated EDID.
pub fn build_add(id: u32, mode: &VdMode) -> Result<AddIn, VdError> {
    let edid =
        vdisplay_edid::build_edid(id, mode).map_err(|error| VdError::Failed(error.to_string()))?;
    let modes = vdisplay_edid::mode_list(mode)
        .into_iter()
        .map(|(width, height, refresh_hz)| Mode {
            width,
            height,
            refresh_hz,
        })
        .collect();
    Ok(AddIn {
        monitor_id: id,
        modes,
        edid,
    })
}

/// Prefer a non-empty, trimmed EDID name over the GDI fallback.
pub fn merge_name(edid: Option<&str>, fallback: &str) -> String {
    edid.map(str::trim)
        .filter(|name| !name.is_empty())
        .unwrap_or(fallback)
        .to_owned()
}
