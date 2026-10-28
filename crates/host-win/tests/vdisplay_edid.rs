use host_core::vdisplay::VdMode;
use host_win::vdisplay_edid::{
    alloc_id, build_edid, checksum_ok, edid_name, edid_serial, mode_list, monitor_name,
    preferred_mode, EdidError,
};

fn m(width_px: u32, height_px: u32, scale: f32, refresh_hz: u32) -> VdMode {
    VdMode {
        width_px,
        height_px,
        scale,
        refresh_hz,
    }
}

#[test]
fn header_and_ids() {
    let edid = build_edid(1, &m(2732, 2048, 2.0, 60)).expect("valid EDID");
    assert_eq!(&edid[0..8], &[0, 0xff, 0xff, 0xff, 0xff, 0xff, 0xff, 0]);
    assert_eq!(&edid[8..10], &[0x4a, 0x76]);
    assert_eq!(&edid[10..12], &[0x53, 0x56]);
    assert_eq!(&edid[12..16], &[1, 0, 0, 0]);
    assert_eq!(edid[17], 36);
    assert_eq!(&edid[18..20], &[1, 4]);
    assert_eq!(edid[20], 0xa5);
    assert_eq!(&edid[21..23], &[36, 27]);
    assert_eq!(edid[126], 0);
    assert!(checksum_ok(&edid));
}

#[test]
fn ipad_dtd_bytes() {
    let edid = build_edid(1, &m(2732, 2048, 2.0, 60)).expect("valid EDID");
    assert_eq!(
        &edid[54..72],
        &[
            0x58, 0x8e, 0xac, 0xa0, 0xa0, 0x00, 0x34, 0x80, 0x30, 0x20, 0x35, 0x00, 0x69, 0x0f,
            0x11, 0x00, 0x00, 0x1a,
        ]
    );
}

#[test]
fn fhd_dtd_clock() {
    let edid = build_edid(1, &m(1920, 1080, 1.0, 60)).expect("valid EDID");
    assert_eq!(&edid[54..56], &[0xf8, 0x35]);
    assert_eq!(preferred_mode(&edid), Some((1920, 1080, 60)));
}

#[test]
fn name_descriptor() {
    let edid = build_edid(1, &m(1920, 1080, 1.0, 60)).expect("valid EDID");
    assert_eq!(&edid[72..77], &[0, 0, 0, 0xfc, 0]);
    assert_eq!(&edid[77..90], b"RS Virtual 01");
    assert_eq!(edid_name(&edid), Some("RS Virtual 01".to_owned()));
    assert_eq!(monitor_name(42), "RS Virtual 42");
}

#[test]
fn serial_roundtrip() {
    let mode = m(2732, 2048, 2.0, 60);
    for id in 1..=99 {
        let edid = build_edid(id, &mode).expect("valid id");
        assert_eq!(edid_serial(&edid), Some(id));
        assert!(checksum_ok(&edid));
    }
}

#[test]
fn bad_id() {
    let mode = m(1920, 1080, 1.0, 60);
    assert_eq!(build_edid(0, &mode), Err(EdidError::BadId(0)));
    assert_eq!(build_edid(100, &mode), Err(EdidError::BadId(100)));
}

#[test]
fn falls_back_to_30hz() {
    let edid = build_edid(1, &m(4000, 3000, 1.0, 60)).expect("30 Hz fits");
    assert_eq!(preferred_mode(&edid), Some((4000, 3000, 30)));
}

#[test]
fn too_large() {
    assert_eq!(
        build_edid(1, &m(4096, 2160, 1.0, 60)),
        Err(EdidError::TooLarge {
            width: 4096,
            height: 2160,
        })
    );
}

#[test]
fn max_windows_mode_fits_60() {
    let edid = build_edid(1, &m(3840, 2560, 1.0, 60)).expect("60 Hz fits");
    assert_eq!(preferred_mode(&edid), Some((3840, 2560, 60)));
}

#[test]
fn portrait() {
    let edid = build_edid(1, &m(2048, 2732, 2.0, 60)).expect("valid EDID");
    assert_eq!(preferred_mode(&edid), Some((2048, 2732, 60)));
    assert_eq!(&edid[21..23], &[27, 36]);
}

#[test]
fn size_clamp() {
    let edid = build_edid(1, &m(640, 480, 1.0, 60)).expect("valid EDID");
    assert_eq!(&edid[21..23], &[17, 13]);
}

#[test]
fn mode_list_rules() {
    assert_eq!(mode_list(&m(1920, 1080, 1.0, 60)), vec![(1920, 1080, 60)]);
    assert_eq!(
        mode_list(&m(1920, 1080, 1.0, 30)),
        vec![(1920, 1080, 30), (1920, 1080, 60)]
    );
}

#[test]
fn alloc_id_rules() {
    assert_eq!(alloc_id(&[]), Some(1));
    assert_eq!(alloc_id(&[1, 2, 4]), Some(3));
    let used: Vec<u32> = (1..=99).collect();
    assert_eq!(alloc_id(&used), None);
    assert_eq!(alloc_id(&[3, 1]), Some(2));
}

#[test]
fn checksum_detects_change() {
    let mut edid = build_edid(1, &m(1920, 1080, 1.0, 60)).expect("valid EDID");
    assert!(checksum_ok(&edid));
    edid[20] = edid[20].wrapping_add(1);
    assert!(!checksum_ok(&edid));
}

#[test]
fn parsers_reject_short() {
    let short = [0_u8; 10];
    assert!(!checksum_ok(&short));
    assert_eq!(edid_serial(&short), None);
    assert_eq!(edid_name(&short), None);
    assert_eq!(preferred_mode(&short), None);
}
