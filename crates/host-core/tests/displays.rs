use std::collections::HashSet;

use host_core::platform::DisplayInfo;
use host_core::session::displays::{
    epoch_byte, fit_stream_size, ChangeFlags, DisplayAction, DisplayDiff, DisplayRegistry,
    RebuildReason, SelectError, StreamDisplay,
};

const MAIN: u32 = 69_733_248;
const PORTRAIT: u32 = 724_041_365;

fn display(id: u32) -> DisplayInfo {
    DisplayInfo {
        id,
        name: format!("Display {id}"),
        width_px: 2560,
        height_px: 1440,
        scale: 2.0,
        x: 0,
        y: 0,
        refresh_hz: 60,
        primary: true,
        rotation: 0,
    }
}

fn main_display() -> DisplayInfo {
    display(MAIN)
}

fn portrait_display() -> DisplayInfo {
    DisplayInfo {
        id: PORTRAIT,
        name: "Portrait".into(),
        width_px: 1440,
        height_px: 2560,
        scale: 2.0,
        x: 1280,
        y: 0,
        refresh_hz: 60,
        primary: false,
        rotation: 90,
    }
}

fn two_display_registry() -> DisplayRegistry {
    let mut registry = DisplayRegistry::new();
    registry.update(vec![portrait_display(), main_display()]);
    registry
}

fn rebuild(
    wire_id: u32,
    platform_id: u32,
    epoch: u32,
    display: DisplayInfo,
    reason: RebuildReason,
) -> DisplayAction {
    DisplayAction::Rebuild {
        wire_id,
        platform_id,
        epoch,
        display,
        reason,
    }
}

#[test]
fn first_update_assigns_stable_wire_ids_and_lists_primary_first() {
    let mut registry = DisplayRegistry::new();
    let diff = registry.update(vec![portrait_display(), main_display()]);

    assert_eq!(diff.added, [1, 2]);
    assert!(diff.removed.is_empty());
    assert!(diff.changed.is_empty());
    assert_eq!(registry.wire_id(MAIN), Some(1));
    assert_eq!(registry.wire_id(PORTRAIT), Some(2));
    assert_eq!(registry.platform_id(1), Some(MAIN));
    assert_eq!(registry.platform_id(2), Some(PORTRAIT));
    assert_eq!(registry.primary(), Some(1));
    assert_eq!(registry.get(2), Some(&portrait_display()));

    let list = registry.list();
    assert_eq!(list.displays.len(), 2);
    assert_eq!(list.displays[0].id, 1);
    assert_eq!(list.displays[0].name, main_display().name);
    assert_eq!(list.displays[0].width_px, 2560);
    assert_eq!(list.displays[0].height_px, 1440);
    assert_eq!(list.displays[0].scale, 2.0);
    assert_eq!(list.displays[1].id, 2);
    assert_eq!(list.displays[1].rotation, 90);
    assert_eq!(registry.displays_pb(), list.displays);
}

#[test]
fn unplugged_displays_keep_their_wire_id_and_new_displays_use_the_next_free_id() {
    let mut registry = two_display_registry();

    let removed = registry.update(vec![main_display()]);
    assert_eq!(removed.removed, [2]);
    assert!(removed.added.is_empty());
    assert_eq!(registry.wire_id(PORTRAIT), Some(2));
    assert_eq!(registry.get(2), None);
    assert!(!registry.is_empty());

    let returned = registry.update(vec![main_display(), portrait_display()]);
    assert_eq!(returned.added, [2]);
    assert_eq!(registry.get(2), Some(&portrait_display()));

    let mut third = display(123_456_789);
    third.name = "Third".into();
    third.x = 3000;
    third.primary = false;
    let added = registry.update(vec![main_display(), portrait_display(), third]);
    assert_eq!(added.added, [3]);
    assert_eq!(registry.wire_id(123_456_789), Some(3));
}

#[test]
fn duplicate_platform_ids_use_the_first_entry_and_new_displays_sort_by_position() {
    let mut registry = DisplayRegistry::new();
    let mut duplicate = main_display();
    duplicate.name = "Ignored duplicate".into();
    duplicate.width_px = 800;
    let mut right = display(300);
    right.x = 3840;
    right.primary = false;
    let mut left = display(200);
    left.x = 2560;
    left.primary = false;

    let diff = registry.update(vec![right, main_display(), duplicate, left]);
    assert_eq!(diff.added, [1, 2, 3]);
    assert_eq!(registry.wire_id(MAIN), Some(1));
    assert_eq!(registry.wire_id(300), Some(3));
    assert_eq!(registry.wire_id(200), Some(2));
    assert_eq!(
        registry.list().displays[1..]
            .iter()
            .map(|display| display.id)
            .collect::<Vec<_>>(),
        [2, 3]
    );
    assert_eq!(registry.get(1).map(|display| display.width_px), Some(2560));
}

#[test]
fn update_reports_field_changes_and_identical_lists_are_empty() {
    let mut registry = two_display_registry();
    let idle = registry.update(vec![portrait_display(), main_display()]);
    assert!(idle.is_empty());
    assert_eq!(idle, DisplayDiff::default());

    let mut resized = main_display();
    resized.width_px = 1920;
    resized.height_px = 1080;
    let diff = registry.update(vec![portrait_display(), resized.clone()]);
    assert_eq!(
        diff.changed,
        [(
            1,
            ChangeFlags {
                size: true,
                ..ChangeFlags::default()
            }
        )]
    );

    let mut scaled = resized;
    scaled.scale = 1.0;
    let diff = registry.update(vec![portrait_display(), scaled.clone()]);
    assert_eq!(
        diff.changed,
        [(
            1,
            ChangeFlags {
                scale: true,
                ..ChangeFlags::default()
            }
        )]
    );

    let mut moved = scaled;
    moved.x = -100;
    let diff = registry.update(vec![portrait_display(), moved]);
    assert_eq!(
        diff.changed,
        [(
            1,
            ChangeFlags {
                position: true,
                ..ChangeFlags::default()
            }
        )]
    );
}

#[test]
fn resolve_uses_primary_flag_then_sorted_first_and_rejects_unknown_ids() {
    let registry = two_display_registry();
    assert_eq!(registry.resolve(0), Some(1));
    assert_eq!(registry.resolve(1), Some(1));
    assert_eq!(registry.resolve(2), Some(2));
    assert_eq!(registry.resolve(99), None);

    let mut registry = DisplayRegistry::new();
    let mut left = main_display();
    left.primary = false;
    let mut right = portrait_display();
    right.primary = false;
    registry.update(vec![right, left]);
    assert_eq!(registry.primary(), Some(1));
    assert_eq!(registry.resolve(0), Some(1));

    let empty = DisplayRegistry::new();
    assert_eq!(empty.primary(), None);
    assert_eq!(empty.resolve(0), None);
    assert!(empty.is_empty());
}

#[test]
fn start_always_rebuilds_and_advances_epoch() {
    let registry = two_display_registry();
    let mut stream = StreamDisplay::new();

    let first = stream.start(&registry, 0).expect("start");
    assert_eq!(
        first,
        rebuild(1, MAIN, 1, main_display(), RebuildReason::Start)
    );
    assert_eq!(stream.selected(), Some(1));
    assert_eq!(stream.epoch(), 1);
    assert!(!stream.is_suspended());

    let second = stream.start(&registry, 0).expect("restart");
    assert_eq!(
        second,
        rebuild(1, MAIN, 2, main_display(), RebuildReason::Start)
    );
    assert_eq!(stream.epoch(), 2);
}

#[test]
fn select_is_idempotent_and_errors_leave_state_unchanged() {
    let registry = two_display_registry();
    let mut stream = StreamDisplay::new();

    assert_eq!(stream.select(&registry, 2), Ok(DisplayAction::None));
    assert_eq!(stream.selected(), None);
    assert_eq!(stream.epoch(), 0);

    stream.start(&registry, 1).expect("start");
    assert_eq!(stream.select(&registry, 1), Ok(DisplayAction::None));
    assert_eq!(stream.epoch(), 1);

    let selected = stream.select(&registry, 2).expect("select");
    assert_eq!(
        selected,
        rebuild(2, PORTRAIT, 2, portrait_display(), RebuildReason::Selected)
    );
    assert_eq!(stream.selected(), Some(2));
    assert_eq!(stream.epoch(), 2);

    assert_eq!(
        stream.select(&registry, 77),
        Err(SelectError::UnknownDisplay(77))
    );
    assert_eq!(stream.selected(), Some(2));
    assert_eq!(stream.epoch(), 2);

    let mut empty_registry = DisplayRegistry::new();
    empty_registry.update(vec![]);
    assert_eq!(
        stream.select(&empty_registry, 2),
        Err(SelectError::NoDisplays)
    );
    assert_eq!(stream.selected(), Some(2));
    assert_eq!(stream.epoch(), 2);
}

#[test]
fn selected_display_resize_and_rotation_rebuild_while_other_changes_do_not() {
    let mut registry = two_display_registry();
    let mut stream = StreamDisplay::new();
    stream.start(&registry, 1).expect("start");

    let mut resized = main_display();
    resized.width_px = 1920;
    resized.height_px = 1080;
    let diff = registry.update(vec![portrait_display(), resized.clone()]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        rebuild(1, MAIN, 2, resized.clone(), RebuildReason::Resized)
    );

    let mut rotated = resized;
    rotated.rotation = 90;
    let diff = registry.update(vec![portrait_display(), rotated.clone()]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        rebuild(1, MAIN, 3, rotated.clone(), RebuildReason::Rotated)
    );

    let mut cosmetic = rotated;
    cosmetic.scale = 1.0;
    cosmetic.x = 20;
    let diff = registry.update(vec![portrait_display(), cosmetic]);
    assert!(!diff.is_empty());
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        DisplayAction::None
    );
    assert_eq!(stream.epoch(), 3);
}

#[test]
fn removal_falls_back_to_primary_then_suspends_and_reappears() {
    let mut registry = two_display_registry();
    let mut stream = StreamDisplay::new();
    stream.start(&registry, 2).expect("start portrait");

    let diff = registry.update(vec![main_display()]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        rebuild(1, MAIN, 2, main_display(), RebuildReason::Removed)
    );
    assert_eq!(stream.selected(), Some(1));

    let diff = registry.update(vec![]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        DisplayAction::Suspend { epoch: 2 }
    );
    assert_eq!(stream.selected(), Some(1));
    assert_eq!(stream.epoch(), 2);
    assert!(stream.is_suspended());

    let diff = registry.update(vec![portrait_display()]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        rebuild(
            2,
            PORTRAIT,
            3,
            portrait_display(),
            RebuildReason::Reappeared
        )
    );
    assert_eq!(stream.selected(), Some(2));
    assert!(!stream.is_suspended());

    let mut registry = two_display_registry();
    let mut stream = StreamDisplay::new();
    stream.start(&registry, 1).expect("start main");
    let diff = registry.update(vec![main_display()]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        DisplayAction::None
    );
    assert_eq!(stream.epoch(), 1);
}

#[test]
fn registry_changes_before_start_are_ignored() {
    let mut registry = two_display_registry();
    let mut stream = StreamDisplay::new();

    let diff = registry.update(vec![main_display()]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        DisplayAction::None
    );
    let diff = registry.update(vec![]);
    assert_eq!(
        stream.on_registry_change(&registry, &diff),
        DisplayAction::None
    );
    assert_eq!(stream.selected(), None);
    assert_eq!(stream.epoch(), 0);
    assert!(!stream.is_suspended());
}

#[test]
fn fit_stream_size_respects_orientation_and_even_dimensions() {
    let landscape = main_display();
    assert_eq!(fit_stream_size(&landscape, 1920, 1080), (1920, 1080));

    let portrait = portrait_display();
    assert_eq!(fit_stream_size(&portrait, 1920, 1080), (1080, 1920));

    let small = DisplayInfo {
        width_px: 1280,
        height_px: 720,
        ..main_display()
    };
    assert_eq!(fit_stream_size(&small, 1920, 1080), (1280, 720));

    let odd = DisplayInfo {
        width_px: 1921,
        height_px: 1081,
        ..main_display()
    };
    assert_eq!(fit_stream_size(&odd, 0, 0), (1920, 1080));

    let tiny = DisplayInfo {
        width_px: 1,
        height_px: 1,
        ..main_display()
    };
    assert_eq!(fit_stream_size(&tiny, 1920, 1080), (2, 2));
}

#[test]
fn start_without_displays_is_an_error_and_leaves_state_unchanged() {
    let mut stream = StreamDisplay::new();
    let registry = DisplayRegistry::new();

    assert_eq!(stream.start(&registry, 0), Err(SelectError::NoDisplays));
    assert_eq!(stream.start(&registry, 77), Err(SelectError::NoDisplays));
    assert_eq!(stream.selected(), None);
    assert_eq!(stream.epoch(), 0);
    assert!(!stream.is_suspended());
}

#[test]
fn epoch_byte_keeps_the_low_eight_bits() {
    assert_eq!(epoch_byte(255), 255);
    assert_eq!(epoch_byte(256), 0);
    assert_eq!(epoch_byte(257), 1);
}

#[test]
fn wire_ids_stay_in_range_and_unique_after_exhaustion() {
    let mut registry = DisplayRegistry::new();
    for index in 0..300 {
        let screen = display(10_000 + index);
        let inserted = registry.update(vec![screen]);
        assert_eq!(inserted.added.len(), 1);
        assert!((1..=255).contains(&inserted.added[0]));
        let removed = registry.update(vec![]);
        assert_eq!(removed.removed, inserted.added);
    }

    let simultaneous: Vec<DisplayInfo> = (0..5)
        .map(|index| {
            let mut screen = display(20_000 + index);
            screen.x = index as i32 * 100;
            screen.primary = index == 0;
            screen
        })
        .collect();
    let diff = registry.update(simultaneous.clone());
    assert_eq!(diff.added.len(), 5);
    let wire_ids: Vec<u32> = simultaneous
        .iter()
        .map(|screen| u32::from(registry.wire_id(screen.id).expect("wire id")))
        .collect();
    assert!(wire_ids.iter().all(|wire_id| (1..=255).contains(wire_id)));
    assert_eq!(wire_ids.iter().copied().collect::<HashSet<_>>().len(), 5);
}

#[test]
fn exhausted_registry_reclaims_the_oldest_disconnected_wire_id() {
    let mut registry = DisplayRegistry::new();
    let all: Vec<DisplayInfo> = (0..255)
        .map(|index| {
            let mut screen = display(30_000 + index);
            screen.x = index as i32;
            screen.primary = index == 0;
            screen
        })
        .collect();
    assert_eq!(registry.update(all.clone()).added.len(), 255);
    assert_eq!(registry.wire_id(30_000), Some(1));
    assert_eq!(registry.wire_id(30_254), Some(255));

    assert_eq!(registry.update(all[..254].to_vec()).removed, [255]);
    assert_eq!(registry.update(all[..253].to_vec()).removed, [254]);

    let mut fresh = display(40_000);
    fresh.x = 4000;
    let diff = registry.update(vec![fresh]);
    assert_eq!(diff.added, [255]);
    assert_eq!(registry.wire_id(40_000), Some(255));
    assert_eq!(registry.wire_id(30_254), None);
    assert_eq!(registry.platform_id(255), Some(40_000));
}
