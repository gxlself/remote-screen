use host_core::vdisplay::VdError;
use host_win::vdisplay_ioctl::{
    decode_add, decode_add_out, decode_info, decode_remove_all_out, encode_add, encode_info,
    encode_remove, is_not_found, map_win32_error, version_in, AddIn, AddOut, Info, IoctlError,
    Mode, EDID_LEN, INTERFACE_GUID, INTERFACE_GUID_PARTS, IOCTL_ADD, IOCTL_GET_INFO, IOCTL_PING,
    IOCTL_REMOVE, IOCTL_REMOVE_ALL, MAX_MODES, VERSION,
};

fn sample_add() -> AddIn {
    let mut edid = [0u8; EDID_LEN];
    for (index, byte) in edid.iter_mut().enumerate() {
        *byte = index as u8;
    }
    AddIn {
        monitor_id: 3,
        modes: vec![
            Mode {
                width: 2732,
                height: 2048,
                refresh_hz: 60,
            },
            Mode {
                width: 2732,
                height: 2048,
                refresh_hz: 30,
            },
        ],
        edid,
    }
}

#[test]
fn ioctl_codes() {
    assert_eq!(IOCTL_GET_INFO, 0x0022E400);
    assert_eq!(IOCTL_ADD, 0x0022E404);
    assert_eq!(IOCTL_REMOVE, 0x0022E408);
    assert_eq!(IOCTL_REMOVE_ALL, 0x0022E40C);
    assert_eq!(IOCTL_PING, 0x0022E410);
}

#[test]
fn add_layout() {
    let add = sample_add();
    let bytes = encode_add(&add).expect("sample add encodes");
    assert_eq!(&bytes[0..4], &VERSION.to_le_bytes());
    assert_eq!(&bytes[4..8], &3u32.to_le_bytes());
    assert_eq!(&bytes[8..12], &2u32.to_le_bytes());
    assert_eq!(&bytes[12..24], &[0xAC, 0x0A, 0, 0, 0, 8, 0, 0, 60, 0, 0, 0]);
    assert_eq!(&bytes[24..36], &[0xAC, 0x0A, 0, 0, 0, 8, 0, 0, 30, 0, 0, 0]);
    assert!(bytes[36..60].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[60..64], &(EDID_LEN as u32).to_le_bytes());
    assert_eq!(&bytes[64..192], &add.edid);
    assert_eq!(decode_add(&bytes).expect("sample add decodes"), add);
    assert_eq!(
        decode_add(&[bytes.as_slice(), &[0xFF; 16]].concat()).expect("long add buffer"),
        add
    );
    for len in 0..bytes.len() {
        assert_eq!(
            decode_add(&bytes[..len]),
            Err(IoctlError::Short {
                need: 192,
                got: len
            })
        );
    }
    let mut bad_version = bytes;
    bad_version[..4].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(decode_add(&bad_version), Err(IoctlError::Version(2)));
}

#[test]
fn add_mode_bounds() {
    let mut add = sample_add();
    add.modes.clear();
    assert_eq!(encode_add(&add), Err(IoctlError::NoModes));
    add.modes = vec![
        Mode {
            width: 1,
            height: 1,
            refresh_hz: 1,
        };
        MAX_MODES + 1
    ];
    assert_eq!(
        encode_add(&add),
        Err(IoctlError::TooManyModes(MAX_MODES + 1))
    );
    add.modes.truncate(MAX_MODES);
    let mut bytes = encode_add(&add).expect("four modes encode");
    assert_eq!(decode_add(&bytes).expect("four modes decode"), add);
    bytes[8..12].copy_from_slice(&0u32.to_le_bytes());
    assert_eq!(decode_add(&bytes), Err(IoctlError::NoModes));
    for count in [5, u32::MAX] {
        bytes[8..12].copy_from_slice(&count.to_le_bytes());
        assert_eq!(
            decode_add(&bytes),
            Err(IoctlError::TooManyModes(count as usize))
        );
    }
}

#[test]
fn info_roundtrip() {
    let info = Info {
        version: VERSION,
        max_monitors: 4,
        max_long_px: 3840,
        max_short_px: 2560,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        active_monitors: 1,
    };
    let bytes = encode_info(&info);
    for (offset, value) in [
        (0, 1u32),
        (4, 4),
        (8, 3840),
        (12, 2560),
        (16, 640),
        (20, 480),
    ] {
        assert_eq!(&bytes[offset..offset + 4], &value.to_le_bytes());
    }
    assert_eq!(&bytes[24..28], &2u32.to_le_bytes());
    assert_eq!(&bytes[28..36], &[30, 0, 0, 0, 60, 0, 0, 0]);
    assert!(bytes[36..44].iter().all(|byte| *byte == 0));
    assert_eq!(&bytes[44..48], &1u32.to_le_bytes());
    assert_eq!(decode_info(&bytes).expect("info decodes"), info);
}

#[test]
fn info_errors() {
    let info = Info {
        version: VERSION,
        max_monitors: 4,
        max_long_px: 3840,
        max_short_px: 2560,
        min_long_px: 640,
        min_short_px: 480,
        refresh_rates: vec![30, 60],
        active_monitors: 1,
    };
    let bytes = encode_info(&info);
    for len in 0..bytes.len() {
        assert_eq!(
            decode_info(&bytes[..len]),
            Err(IoctlError::Short { need: 48, got: len })
        );
    }

    let mut version_bad = bytes;
    version_bad[0..4].copy_from_slice(&2u32.to_le_bytes());
    assert_eq!(decode_info(&version_bad), Err(IoctlError::Version(2)));

    let mut count_bad = bytes;
    count_bad[24..28].copy_from_slice(&5u32.to_le_bytes());
    assert_eq!(decode_info(&count_bad), Err(IoctlError::RefreshCount(5)));
    assert_eq!(
        decode_info(&[bytes.as_slice(), &[0u8; 16]].concat()).expect("long buffer"),
        info
    );
}

#[test]
fn remove_and_small() {
    assert_eq!(encode_remove(7), [1, 0, 0, 0, 7, 0, 0, 0]);
    assert_eq!(version_in(), [1, 0, 0, 0]);
    assert_eq!(
        decode_add_out(&[2, 0, 0, 0, 1, 0, 0, 0]).expect("add output"),
        AddOut {
            monitor_id: 2,
            connector_index: 1,
        }
    );
    assert_eq!(
        decode_remove_all_out(&[3, 0, 0, 0]).expect("remove all output"),
        3
    );
    assert_eq!(
        decode_add_out(&[2, 0, 0, 0, 1, 0, 0, 0, 0xFF]).expect("long add output"),
        AddOut {
            monitor_id: 2,
            connector_index: 1,
        }
    );
    assert_eq!(
        decode_remove_all_out(&[3, 0, 0, 0, 0xFF]).expect("long remove all output"),
        3
    );
    for len in 0..8 {
        assert_eq!(
            decode_add_out(&[0; 8][..len]),
            Err(IoctlError::Short { need: 8, got: len })
        );
    }
    for len in 0..4 {
        assert_eq!(
            decode_remove_all_out(&[0; 4][..len]),
            Err(IoctlError::Short { need: 4, got: len })
        );
    }
}

#[test]
fn error_map() {
    assert_eq!(map_win32_error(1816), VdError::Limit);
    for (code, message) in [
        (21, "virtual display adapter not ready"),
        (2, "virtual display driver not installed"),
        (3, "virtual display driver not installed"),
        (5, "virtual display driver access denied"),
    ] {
        assert_eq!(
            map_win32_error(code),
            VdError::Unsupported(message.to_owned())
        );
    }
    for (code, message) in [
        (87, "driver rejected parameters"),
        (183, "monitor id already in use"),
        (1168, "monitor not found"),
        (9999, "driver error 9999"),
    ] {
        assert_eq!(map_win32_error(code), VdError::Failed(message.to_owned()));
    }
    assert!(matches!(map_win32_error(9999), VdError::Failed(message) if message.contains("9999")));
    assert!(is_not_found(1168));
    assert!(!is_not_found(2));
}

#[test]
fn header_matches_rust() {
    let header = include_str!(concat!(
        env!("CARGO_MANIFEST_DIR"),
        "/../../drivers/win-iddcx/include/RsIddIoctl.h"
    ))
    .to_ascii_lowercase();
    for value in [
        "0x0022e400",
        "0x0022e404",
        "0x0022e408",
        "0x0022e40c",
        "0x0022e410",
        "sizeof(rsidd_info) == 48",
        "sizeof(rsidd_add_in) == 192",
        "sizeof(rsidd_mode) == 12",
        "sizeof(rsidd_add_out) == 8",
        "sizeof(rsidd_remove_in) == 8",
        "sizeof(rsidd_version_in) == 4",
        "sizeof(rsidd_remove_all_out) == 4",
        "0x13037e20, 0xec6d, 0x42cd",
        "rsidd_version 1u",
        "rsidd_max_modes 4u",
        "rsidd_edid_len 128u",
    ] {
        assert!(header.contains(value), "header missing {value}");
    }
}

#[test]
fn guid_parts_match_string() {
    let (a, b, c, d) = INTERFACE_GUID_PARTS;
    let formatted = format!(
        "{{{a:08X}-{b:04X}-{c:04X}-{d0:02X}{d1:02X}-{d2:02X}{d3:02X}{d4:02X}{d5:02X}{d6:02X}{d7:02X}}}",
        d0 = d[0],
        d1 = d[1],
        d2 = d[2],
        d3 = d[3],
        d4 = d[4],
        d5 = d[5],
        d6 = d[6],
        d7 = d[7],
    );
    assert_eq!(formatted, INTERFACE_GUID);
}
