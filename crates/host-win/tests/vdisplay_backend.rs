use host_core::vdisplay::{mode, VdCaps, VdError, VdMode};
use host_win::{
    vdisplay_edid,
    vdisplay_ioctl::{self, Info},
    vdisplay_plan,
};

#[test]
fn add_request_for_ipad() {
    let add = vdisplay_plan::build_add(
        1,
        &VdMode {
            width_px: 2728,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        },
    )
    .expect("valid iPad mode");
    let encoded = vdisplay_ioctl::encode_add(&add).expect("encodable ADD");
    assert_eq!(&encoded[4..8], &1u32.to_le_bytes());
    assert_eq!(&encoded[8..12], &1u32.to_le_bytes());
    assert_eq!(&encoded[12..16], &2728u32.to_le_bytes());
    assert_eq!(&encoded[16..20], &2048u32.to_le_bytes());
    assert_eq!(&encoded[20..24], &60u32.to_le_bytes());
    assert_eq!(vdisplay_edid::edid_serial(&add.edid), Some(1));
    assert_eq!(
        vdisplay_edid::edid_name(&add.edid).as_deref(),
        Some("RS Virtual 01")
    );
}

#[test]
fn add_request_30hz_has_two_modes() {
    let add = vdisplay_plan::build_add(
        1,
        &VdMode {
            width_px: 1920,
            height_px: 1080,
            scale: 1.0,
            refresh_hz: 30,
        },
    )
    .expect("valid mode");
    assert_eq!(
        add.modes
            .iter()
            .map(|mode| (mode.width, mode.height, mode.refresh_hz))
            .collect::<Vec<_>>(),
        vec![(1920, 1080, 30), (1920, 1080, 60)]
    );
}

#[test]
fn add_request_portrait() {
    let add = vdisplay_plan::build_add(
        1,
        &VdMode {
            width_px: 1176,
            height_px: 2552,
            scale: 2.0,
            refresh_hz: 60,
        },
    )
    .expect("valid portrait mode");
    assert_eq!((add.modes[0].width, add.modes[0].height), (1176, 2552));
}

#[test]
fn add_request_too_large() {
    let result = vdisplay_plan::build_add(
        1,
        &VdMode {
            width_px: 4096,
            height_px: 2160,
            scale: 1.0,
            refresh_hz: 60,
        },
    );
    assert!(matches!(result, Err(VdError::Failed(_))));
}

#[test]
fn caps_from_info_values() {
    let info = Info {
        version: 1,
        max_monitors: 8,
        max_long_px: 3840,
        max_short_px: 2560,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        active_monitors: 0,
    };
    assert_eq!(
        vdisplay_plan::caps_from_info(&info),
        VdCaps {
            max_long_px: 3840,
            max_short_px: 2560,
            min_long_px: 640,
            min_short_px: 480,
            refresh_rates: vec![30, 60],
            scales: vec![1.0, 1.5, 2.0],
            align_px: 8,
            max_displays: 4,
        }
    );
}

#[test]
fn caps_choose_ipad() {
    let caps = VdCaps {
        max_long_px: 3840,
        max_short_px: 2560,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        scales: vec![1.0, 1.5, 2.0],
        align_px: 8,
        max_displays: 4,
    };
    let selected = mode::choose(
        &mode::VdRequest {
            width_px: 2732,
            height_px: 2048,
            scale: 0.0,
            refresh_hz: 0,
        },
        &caps,
    )
    .expect("iPad mode fits");
    assert_eq!(
        selected,
        VdMode {
            width_px: 2728,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        }
    );
}

#[test]
fn merge_name_rules() {
    assert_eq!(
        vdisplay_plan::merge_name(Some("DELL U2723QE"), "Generic PnP Monitor"),
        "DELL U2723QE"
    );
    assert_eq!(vdisplay_plan::merge_name(Some("  "), "X"), "X");
    assert_eq!(vdisplay_plan::merge_name(None, "X"), "X");
    assert_eq!(
        vdisplay_plan::merge_name(Some(" RS Virtual 01 "), "X"),
        "RS Virtual 01"
    );
}
