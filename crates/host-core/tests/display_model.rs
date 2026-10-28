use host_core::display_model::{
    display_id, displays_changed, rect_for, rotation_degrees, to_display_infos, virtual_desk,
    MonitorRecord, RectPx,
};

fn record(
    device_name: &str,
    rect: RectPx,
    primary: bool,
    dpi: u32,
    refresh_hz: u32,
    orientation: u32,
) -> MonitorRecord {
    MonitorRecord {
        device_name: device_name.to_owned(),
        friendly_name: String::new(),
        rect,
        primary,
        dpi,
        refresh_hz,
        orientation,
    }
}

#[test]
fn display_ids_parse_display_names_and_hash_other_names() {
    assert_eq!(display_id(r"\\.\DISPLAY1"), 1);
    assert_eq!(display_id(r"\\.\display12"), 12);

    for name in [r"\\.\DISPLAY0", r"\\.\DISPLAY", r"\\.\DISPLAYx", "foo"] {
        assert_ne!(display_id(name) & 0x8000_0000, 0);
        assert_eq!(display_id(name), display_id(name));
    }
    assert_ne!(display_id("foo"), display_id("bar"));
}

#[test]
fn primary_display_is_first_and_values_are_converted() {
    let records = vec![
        record(
            r"\\.\DISPLAY2",
            RectPx {
                left: -1280,
                top: 0,
                width: 1280,
                height: 1024,
            },
            false,
            96,
            75,
            0,
        ),
        record(
            r"\\.\DISPLAY1",
            RectPx {
                left: 0,
                top: 0,
                width: 2880,
                height: 1800,
            },
            true,
            144,
            0,
            0,
        ),
    ];

    let displays = to_display_infos(&records);
    assert_eq!(
        displays
            .iter()
            .map(|display| display.id)
            .collect::<Vec<_>>(),
        [1, 2]
    );
    assert_eq!(displays[0].scale, 1.5);
    assert_eq!(displays[0].refresh_hz, 60);
    assert!(displays[0].primary);
    assert_eq!(displays[0].x, 0);
    assert_eq!(displays[1].scale, 1.0);
    assert_eq!(displays[1].x, -1280);
    assert_eq!(displays[1].refresh_hz, 75);
}

#[test]
fn displays_without_primary_are_sorted_by_position() {
    let records = vec![
        record(
            r"\\.\DISPLAY3",
            RectPx {
                left: 10,
                top: -5,
                width: 100,
                height: 100,
            },
            false,
            0,
            1,
            0,
        ),
        record(
            r"\\.\DISPLAY1",
            RectPx {
                left: -10,
                top: 20,
                width: 100,
                height: 100,
            },
            false,
            0,
            1,
            0,
        ),
        record(
            r"\\.\DISPLAY2",
            RectPx {
                left: -10,
                top: -20,
                width: 100,
                height: 100,
            },
            false,
            0,
            1,
            0,
        ),
    ];

    let displays = to_display_infos(&records);
    assert_eq!(
        displays
            .iter()
            .map(|display| display.id)
            .collect::<Vec<_>>(),
        [2, 1, 3]
    );
    assert_eq!(displays[0].scale, 1.0);
    assert_eq!(rotation_degrees(1), 90);
    assert_eq!(rotation_degrees(7), 0);
}

#[test]
fn names_and_rotation_are_preserved() {
    let mut named = record(
        r"\\.\DISPLAY1",
        RectPx {
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
        },
        true,
        96,
        60,
        1,
    );
    named.friendly_name = "Panel".to_owned();
    let unnamed = record(
        r"\\.\DISPLAY2",
        RectPx {
            left: 1920,
            top: 0,
            width: 1920,
            height: 1080,
        },
        false,
        96,
        60,
        7,
    );

    let displays = to_display_infos(&[named, unnamed]);
    assert_eq!(displays[0].name, "Panel");
    assert_eq!(displays[0].rotation, 90);
    assert_eq!(displays[1].name, r"\\.\DISPLAY2");
    assert_eq!(displays[1].rotation, 0);
}

#[test]
fn virtual_desktop_and_rect_lookup() {
    let records = vec![
        record(
            r"\\.\DISPLAY2",
            RectPx {
                left: -1280,
                top: 0,
                width: 1280,
                height: 1024,
            },
            false,
            96,
            75,
            0,
        ),
        record(
            r"\\.\DISPLAY1",
            RectPx {
                left: 0,
                top: 0,
                width: 2880,
                height: 1800,
            },
            true,
            144,
            60,
            0,
        ),
    ];
    assert_eq!(
        virtual_desk(&records),
        RectPx {
            left: -1280,
            top: 0,
            width: 4160,
            height: 1800,
        }
    );
    assert_eq!(
        virtual_desk(&[]),
        RectPx {
            left: 0,
            top: 0,
            width: 1,
            height: 1
        }
    );
    assert_eq!(rect_for(&records, 2), Some(records[0].rect));
    assert_eq!(rect_for(&records, 9), None);
}

#[test]
fn display_changes_include_fields_and_length() {
    let records = vec![record(
        r"\\.\DISPLAY1",
        RectPx {
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
        },
        true,
        96,
        60,
        0,
    )];
    let displays = to_display_infos(&records);
    assert!(!displays_changed(&displays, &displays));

    let mut changed = displays.clone();
    changed[0].refresh_hz = 75;
    assert!(displays_changed(&displays, &changed));
    assert!(displays_changed(&displays, &[]));
}

#[test]
fn duplicate_device_names_keep_the_first_record() {
    let first = record(
        r"\\.\DISPLAY1",
        RectPx {
            left: 0,
            top: 0,
            width: 1920,
            height: 1080,
        },
        true,
        96,
        60,
        0,
    );
    let second = record(
        r"\\.\DISPLAY1",
        RectPx {
            left: 500,
            top: 500,
            width: 640,
            height: 480,
        },
        false,
        144,
        75,
        2,
    );
    let displays = to_display_infos(&[first, second]);
    assert_eq!(displays.len(), 1);
    assert_eq!(displays[0].x, 0);
    assert_eq!(displays[0].width_px, 1920);
}
