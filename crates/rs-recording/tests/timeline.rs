use proptest::prelude::*;
use rs_recording::record::GapReason;
use rs_recording::timeline::{
    EpochRemap, GapMark, MediaClock, Pacer, SeekIndex, SeekPoint, GAP_COMPRESSED_US,
    GAP_THRESHOLD_US,
};

const SEC: u64 = 1_000_000;

#[test]
fn media_clock_compresses_long_gaps() {
    let mut clock = MediaClock::new();
    assert_eq!(clock.map(1_000), (0, None));
    assert_eq!(clock.map(1_500), (500, None));
    assert_eq!(
        clock.map(10_000_000),
        (
            100_500,
            Some(GapMark {
                reason: GapReason::Clock,
                real_us: 9_998_500,
            })
        )
    );
    assert_eq!(clock.media_now(), 100_500);
}

#[test]
fn media_clock_paused_mark_applies_to_next_gap_only() {
    let mut clock = MediaClock::new();
    clock.map(0);
    clock.mark_paused();
    assert_eq!(
        clock.map(5 * SEC),
        (
            GAP_COMPRESSED_US,
            Some(GapMark {
                reason: GapReason::Paused,
                real_us: 5 * SEC,
            })
        )
    );
    assert_eq!(
        clock.map(10 * SEC),
        (
            2 * GAP_COMPRESSED_US,
            Some(GapMark {
                reason: GapReason::Clock,
                real_us: 5 * SEC,
            })
        )
    );
}

#[test]
fn media_clock_short_interval_clears_reconnect_mark() {
    let mut clock = MediaClock::new();
    clock.map(0);
    clock.mark_reconnect();
    assert_eq!(clock.map(SEC), (SEC, None));
    assert_eq!(
        clock.map(5 * SEC),
        (
            SEC + GAP_COMPRESSED_US,
            Some(GapMark {
                reason: GapReason::Clock,
                real_us: 4 * SEC,
            })
        )
    );
}

#[test]
fn media_clock_reconnect_mark_names_the_gap() {
    let mut clock = MediaClock::new();
    clock.map(0);
    clock.mark_reconnect();
    let (_, gap) = clock.map(4 * SEC);
    assert_eq!(gap.map(|g| g.reason), Some(GapReason::Reconnect));
}

#[test]
fn media_clock_ignores_time_going_backwards() {
    let mut clock = MediaClock::new();
    clock.map(1_000);
    assert_eq!(clock.map(5_000), (4_000, None));
    assert_eq!(clock.map(4_000), (4_000, None));
    assert_eq!(clock.media_now(), 4_000);
}

#[test]
fn media_clock_exact_threshold_is_not_a_gap() {
    let mut clock = MediaClock::new();
    clock.map(7);
    assert_eq!(clock.map(7 + GAP_THRESHOLD_US), (GAP_THRESHOLD_US, None));
    let (t, gap) = clock.map(8 + 2 * GAP_THRESHOLD_US);
    assert_eq!(t, GAP_THRESHOLD_US + GAP_COMPRESSED_US);
    assert_eq!(gap.map(|g| g.real_us), Some(GAP_THRESHOLD_US + 1));
}

proptest! {
    #[test]
    fn media_clock_is_monotonic(reals in proptest::collection::vec(any::<u64>(), 1..64)) {
        let mut clock = MediaClock::new();
        let mut last = 0;
        for (i, real) in reals.into_iter().enumerate() {
            let (t, gap) = clock.map(real);
            if i == 0 {
                prop_assert_eq!((t, gap), (0, None));
            }
            prop_assert!(t >= last);
            prop_assert_eq!(t, clock.media_now());
            if gap.is_some() {
                prop_assert!(t - last <= GAP_COMPRESSED_US);
            }
            last = t;
        }
    }
}

fn index() -> SeekIndex {
    SeekIndex::from_chunks(
        [
            (0, true, 0),
            (1, false, 10 * SEC),
            (2, true, 30 * SEC),
            (3, false, 40 * SEC),
            (4, true, 60 * SEC),
        ],
        75 * SEC,
    )
}

#[test]
fn seek_index_locates_previous_keyframe() {
    let index = index();
    let at = |chunk, t_us| Some(SeekPoint { chunk, t_us });
    assert_eq!(index.locate(45 * SEC), at(2, 30 * SEC));
    assert_eq!(index.locate(0), at(0, 0));
    assert_eq!(index.locate(100 * SEC), at(4, 60 * SEC));
    assert_eq!(index.locate(29_999_000), at(0, 0));
    assert_eq!(index.locate(30 * SEC), at(2, 30 * SEC));
    assert_eq!(index.duration_us(), 75 * SEC);
}

#[test]
fn seek_index_keeps_only_keyframe_chunks() {
    assert_eq!(
        index().points(),
        &[
            SeekPoint { chunk: 0, t_us: 0 },
            SeekPoint {
                chunk: 2,
                t_us: 30 * SEC
            },
            SeekPoint {
                chunk: 4,
                t_us: 60 * SEC
            },
        ]
    );
}

#[test]
fn seek_index_without_keyframes_is_empty() {
    let index = SeekIndex::from_chunks([(0, false, 0), (1, false, SEC)], 2 * SEC);
    assert!(index.points().is_empty());
    assert_eq!(index.locate(0), None);
    assert_eq!(index.locate(SEC), None);
    assert_eq!(SeekIndex::from_chunks([], 0).locate(0), None);
}

#[test]
fn seek_index_drops_points_that_go_backwards() {
    let index = SeekIndex::from_chunks(
        [
            (0, true, 10 * SEC),
            (1, true, 30 * SEC),
            (2, true, 20 * SEC),
            (3, true, 40 * SEC),
        ],
        50 * SEC,
    );
    assert_eq!(
        index.points(),
        &[
            SeekPoint {
                chunk: 0,
                t_us: 10 * SEC
            },
            SeekPoint {
                chunk: 1,
                t_us: 30 * SEC
            },
            SeekPoint {
                chunk: 3,
                t_us: 40 * SEC
            },
        ]
    );
    assert_eq!(
        index.locate(5 * SEC),
        Some(SeekPoint {
            chunk: 0,
            t_us: 10 * SEC
        })
    );
    assert_eq!(
        index.locate(35 * SEC),
        Some(SeekPoint {
            chunk: 1,
            t_us: 30 * SEC
        })
    );
}

#[test]
fn pacer_play_pause_seek() {
    let mut pacer = Pacer::new();
    assert!(!pacer.is_playing());
    assert_eq!(pacer.position(0), 0);
    assert_eq!(pacer.position(123_456), 0);
    assert_eq!(pacer.due_at(0), None);

    pacer.play(1_000);
    assert!(pacer.is_playing());
    assert_eq!(pacer.position(1_500), 500);
    assert_eq!(pacer.due_at(2_000), Some(3_000));

    pacer.pause(2_000);
    assert!(!pacer.is_playing());
    assert_eq!(pacer.position(9_999), 1_000);
    assert_eq!(pacer.due_at(2_000), None);

    pacer.seek(30_000, 3_000);
    assert!(!pacer.is_playing());
    assert_eq!(pacer.position(3_500), 30_000);

    pacer.play(4_000);
    assert_eq!(pacer.position(5_000), 31_000);
}

#[test]
fn pacer_repeated_play_or_pause_keeps_position() {
    let mut pacer = Pacer::new();
    pacer.play(1_000);
    pacer.play(5_000);
    assert_eq!(pacer.position(6_000), 5_000);
    assert_eq!(pacer.due_at(5_000), Some(6_000));

    pacer.pause(6_000);
    pacer.pause(9_000);
    assert_eq!(pacer.position(9_500), 5_000);
    pacer.play(10_000);
    assert_eq!(pacer.position(10_250), 5_250);
}

#[test]
fn pacer_seek_while_playing_keeps_playing() {
    let mut pacer = Pacer::new();
    pacer.play(0);
    pacer.seek(60_000, 2_000);
    assert!(pacer.is_playing());
    assert_eq!(pacer.position(2_000), 60_000);
    assert_eq!(pacer.position(2_500), 60_500);
    assert_eq!(pacer.due_at(61_000), Some(3_000));
}

#[test]
fn pacer_past_media_is_due_now_or_earlier() {
    let mut pacer = Pacer::new();
    pacer.seek(10_000, 0);
    pacer.play(500);
    let now = 2_000;
    assert_eq!(pacer.position(now), 11_500);
    assert_eq!(pacer.due_at(11_500), Some(now));
    assert!(pacer.due_at(11_000).unwrap() <= now);
    assert_eq!(pacer.due_at(0), Some(0));
}

proptest! {
    #[test]
    fn pacer_due_at_inverts_position(
        start in 0u64..1 << 40,
        play_at in 0u64..1 << 40,
        elapsed in 0u64..1 << 40,
        back in 0u64..1 << 41,
    ) {
        let mut pacer = Pacer::new();
        pacer.seek(start, 0);
        pacer.play(play_at);
        let now = play_at + elapsed;
        let pos = pacer.position(now);
        prop_assert_eq!(pos, start + elapsed);
        prop_assert_eq!(pacer.due_at(pos), Some(now));
        prop_assert!(pacer.due_at(pos.saturating_sub(back)).unwrap() <= now);
    }
}

#[test]
fn epoch_remap_renews_on_config_change_and_seek() {
    let mut remap = EpochRemap::new();
    assert_eq!(remap.on_config(7), 1);
    assert_eq!(remap.on_config(7), 1);
    assert_eq!(remap.on_config(8), 2);
    remap.on_seek();
    assert_eq!(remap.on_config(8), 3);
    assert_eq!(remap.frame_epoch(), 3);
    assert!(remap.frame_matches(8));
    assert!(!remap.frame_matches(7));
    assert_eq!(remap.on_config(8), 3);
}

#[test]
fn epoch_remap_first_config_always_renews() {
    let mut remap = EpochRemap::new();
    assert_eq!(remap.frame_epoch(), 0);
    assert!(!remap.frame_matches(0));
    assert_eq!(remap.on_config(0), 1);
    assert!(remap.frame_matches(0));

    let mut remap = EpochRemap::with_start(41);
    assert_eq!(remap.on_config(9), 42);
}

#[test]
fn epoch_remap_wraps_to_one() {
    let mut remap = EpochRemap::with_start(u32::MAX);
    assert_eq!(remap.on_config(1), 1);
    assert_eq!(remap.frame_epoch(), 1);

    let mut remap = EpochRemap::with_start(u32::MAX - 1);
    assert_eq!(remap.on_config(1), u32::MAX);
    remap.on_seek();
    assert_eq!(remap.on_config(1), 1);
}

#[test]
fn epoch_remap_compares_low_byte_of_recorded_epoch() {
    let mut remap = EpochRemap::with_start(255);
    assert_eq!(remap.on_config(0x1_02), 256);
    assert_eq!(remap.frame_epoch(), 0);
    assert!(remap.frame_matches(0x02));
    assert!(!remap.frame_matches(0x01));
}
