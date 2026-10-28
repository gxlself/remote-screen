use rs_audio::jitter::{JitterBuffer, JitterConfig, JitterStats, Pop, PushResult};
use rs_audio::{FRAME_US, MAX_PACKET};

const HOST_US: u64 = 1_000_000_000;
const RECV_US: u64 = 2_000_000_000;

fn push_frame(buffer: &mut JitterBuffer, seq: u16, frame: u64) -> PushResult {
    buffer.push(
        seq,
        HOST_US + frame * FRAME_US,
        &seq.to_be_bytes(),
        RECV_US + frame * FRAME_US,
    )
}

fn packet(buffer: &mut JitterBuffer, expected: u16, out: &mut [u8]) -> bool {
    let result = buffer.pop(out);
    let Pop::Packet {
        seq,
        len,
        discontinuity,
        ..
    } = result
    else {
        panic!("expected packet {expected}, got {result:?}");
    };
    assert_eq!(seq, expected);
    assert_eq!(&out[..len], &expected.to_be_bytes());
    discontinuity
}

#[test]
fn ordered_zero_jitter_starts_on_fourth_tick() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for frame in 0..1_000 {
        assert_eq!(
            push_frame(&mut buffer, frame, frame.into()),
            PushResult::Accepted
        );
        if frame < 3 {
            assert_eq!(buffer.pop(&mut out), Pop::Empty);
        } else {
            assert!(!packet(&mut buffer, frame - 3, &mut out));
        }
        assert_eq!(buffer.target_ms(), 40);
    }
    assert_eq!(buffer.stats().received, 1_000);
    assert_eq!(buffer.stats().lost, 0);
    assert_eq!(buffer.stats().underruns, 0);
}

#[test]
fn missing_frame_copies_next_packet_for_fec_without_consuming_it() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    for seq in 0..10 {
        if seq != 7 {
            assert_eq!(
                push_frame(&mut buffer, seq, seq.into()),
                PushResult::Accepted
            );
        }
    }
    let mut out = [0; MAX_PACKET];
    for seq in 0..=6 {
        packet(&mut buffer, seq, &mut out);
    }
    assert_eq!(
        buffer.pop(&mut out),
        Pop::Missing {
            seq: 7,
            fec_len: Some(2)
        }
    );
    assert_eq!(&out[..2], &8_u16.to_be_bytes());
    assert!(packet(&mut buffer, 8, &mut out));
    assert!(!packet(&mut buffer, 9, &mut out));
    assert_eq!(buffer.stats().lost, 1);
}

#[test]
fn consecutive_losses_only_offer_fec_for_the_immediate_successor() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    for seq in 0..10 {
        if seq != 7 && seq != 8 {
            push_frame(&mut buffer, seq, seq.into());
        }
    }
    let mut out = [0; MAX_PACKET];
    for seq in 0..=6 {
        packet(&mut buffer, seq, &mut out);
    }
    assert_eq!(
        buffer.pop(&mut out),
        Pop::Missing {
            seq: 7,
            fec_len: None
        }
    );
    assert_eq!(
        buffer.pop(&mut out),
        Pop::Missing {
            seq: 8,
            fec_len: Some(2)
        }
    );
    assert_eq!(&out[..2], &9_u16.to_be_bytes());
    assert!(packet(&mut buffer, 9, &mut out));
    assert_eq!(buffer.stats().lost, 2);
}

#[test]
fn packet_arriving_after_its_playout_is_late_and_raises_target() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for seq in 0..=23 {
        if seq != 20 {
            push_frame(&mut buffer, seq, seq.into());
        }
        let result = buffer.pop(&mut out);
        if seq == 23 {
            assert_eq!(
                result,
                Pop::Missing {
                    seq: 20,
                    fec_len: Some(2)
                }
            );
        }
    }
    assert_eq!(buffer.target_ms(), 40);
    assert_eq!(push_frame(&mut buffer, 20, 20), PushResult::Late);
    assert_eq!(buffer.target_ms(), 50);
    assert_eq!(buffer.stats().late, 1);
}

#[test]
fn duplicate_does_not_replace_payload_or_add_a_transit_sample() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    assert_eq!(push_frame(&mut buffer, 0, 0), PushResult::Accepted);
    assert_eq!(
        buffer.push(0, HOST_US, &[99], RECV_US + 1_000_000),
        PushResult::Duplicate
    );
    for seq in 1..4 {
        push_frame(&mut buffer, seq, seq.into());
    }
    assert_eq!(buffer.target_ms(), 40);
    assert_eq!(buffer.stats().duplicate, 1);
    let mut out = [0; MAX_PACKET];
    packet(&mut buffer, 0, &mut out);
}

#[test]
fn reordered_packets_before_playout_are_sorted_including_an_earlier_first_packet() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    for seq in [8, 7, 9, 10] {
        assert_eq!(
            push_frame(&mut buffer, seq, seq.into()),
            PushResult::Accepted
        );
    }
    let mut out = [0; MAX_PACKET];
    for seq in 7..=10 {
        assert!(!packet(&mut buffer, seq, &mut out));
    }
    assert_eq!(buffer.stats().lost, 0);
}

#[derive(Debug)]
struct Tick {
    time_us: u64,
    target_ms: u32,
    underruns: u64,
}

// Arrival timestamps and the 10 ms playout ticks are entirely virtual. Sorting
// delayed arrivals also exercises real packet reordering, not just transit math.
fn network(delays_us: &[i64]) -> (JitterBuffer, Vec<Tick>) {
    let mut arrivals: Vec<_> = delays_us
        .iter()
        .enumerate()
        .map(|(frame, delay)| {
            let recv = (frame as i64 * FRAME_US as i64 + 50_000 + delay) as u64;
            (recv, frame)
        })
        .collect();
    arrivals.sort_unstable();
    let end = arrivals.last().unwrap().0.div_ceil(FRAME_US) * FRAME_US;
    let mut next_arrival = 0;
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    let mut ticks = Vec::new();
    for time_us in (0..=end).step_by(FRAME_US as usize) {
        while let Some(&(recv, frame)) = arrivals.get(next_arrival) {
            if recv > time_us {
                break;
            }
            buffer.push(
                frame as u16,
                HOST_US + frame as u64 * FRAME_US,
                &(frame as u16).to_be_bytes(),
                RECV_US + recv,
            );
            next_arrival += 1;
        }
        buffer.pop(&mut out);
        ticks.push(Tick {
            time_us,
            target_ms: buffer.target_ms(),
            underruns: buffer.stats().underruns,
        });
    }
    (buffer, ticks)
}

fn uniform_jitter(frames: usize) -> Vec<i64> {
    let mut seed = 0x1357_2468_u32;
    (0..frames)
        .map(|_| {
            seed = seed.wrapping_mul(1_664_525).wrapping_add(1_013_904_223);
            i64::from(seed % 30_001) - 15_000
        })
        .collect()
}

#[test]
fn uniform_fifteen_ms_jitter_adapts_within_two_seconds() {
    let (buffer, ticks) = network(&uniform_jitter(1_000));
    let at_two_seconds = ticks.iter().find(|tick| tick.time_us == 2_000_000).unwrap();
    assert!(
        (50..=60).contains(&at_two_seconds.target_ms),
        "{at_two_seconds:?}"
    );
    assert!(
        buffer.stats().underruns - at_two_seconds.underruns <= 1,
        "{:?}",
        buffer.stats()
    );
    assert_eq!(buffer.stats().lost, 0);
}

#[test]
fn intermittent_sixty_ms_spikes_never_exceed_eighty_ms_target() {
    let delays: Vec<_> = (0..1_000)
        .map(|frame| if frame % 15 == 0 { 60_000 } else { 0 })
        .collect();
    let (buffer, ticks) = network(&delays);
    assert!(ticks.iter().all(|tick| (40..=80).contains(&tick.target_ms)));
    assert!(ticks.iter().any(|tick| tick.target_ms == 80));
    assert_eq!(buffer.target_ms(), 80);
}

#[test]
fn removing_jitter_returns_target_to_forty_within_thirteen_seconds() {
    let mut delays = uniform_jitter(1_000);
    delays.extend([0; 1_300]);
    let (_, ticks) = network(&delays);
    let stable = ticks
        .iter()
        .find(|tick| tick.time_us == 10_000_000)
        .unwrap();
    assert!((50..=60).contains(&stable.target_ms));
    assert!(ticks
        .iter()
        .filter(|tick| (10_000_000..15_000_000).contains(&tick.time_us))
        .all(|tick| tick.target_ms >= stable.target_ms));
    assert_eq!(ticks.last().unwrap().target_ms, 40);
}

#[test]
fn slow_decrease_waits_five_seconds_then_two_seconds_per_step() {
    // A zero-length sample window makes want exactly 40, isolating hysteresis.
    let mut buffer = JitterBuffer::new(JitterConfig {
        window_us: 0,
        ..JitterConfig::default()
    });
    let mut out = [0; MAX_PACKET];
    for seq in 0..4 {
        push_frame(&mut buffer, seq, seq.into());
    }
    packet(&mut buffer, 0, &mut out);
    for _ in 0..4 {
        assert_eq!(push_frame(&mut buffer, 0, 0), PushResult::Late);
    }
    assert_eq!(buffer.target_ms(), 80);
    for seq in 4..=1_104 {
        push_frame(&mut buffer, seq, seq.into());
        buffer.pop(&mut out);
        let expected = match seq {
            4..=503 => 80,
            504..=703 => 70,
            704..=903 => 60,
            904..=1_103 => 50,
            _ => 40,
        };
        assert_eq!(buffer.target_ms(), expected, "at frame {seq}");
    }
}

#[test]
fn sequence_wraparound_does_not_create_missing_frames() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for frame in 0..12_u16 {
        let seq = 65_530_u16.wrapping_add(frame);
        push_frame(&mut buffer, seq, frame.into());
        if frame < 3 {
            assert_eq!(buffer.pop(&mut out), Pop::Empty);
        } else {
            packet(&mut buffer, seq.wrapping_sub(3), &mut out);
        }
    }
    for seq in 3..=5 {
        packet(&mut buffer, seq, &mut out);
    }
    assert_eq!(buffer.stats().lost, 0);
    assert_eq!(buffer.stats().resyncs, 0);
}

#[test]
fn large_forward_and_backward_jumps_resync_and_prebuffer() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for seq in 0..4 {
        push_frame(&mut buffer, seq, seq.into());
    }
    packet(&mut buffer, 0, &mut out);
    assert_eq!(push_frame(&mut buffer, 5_001, 4), PushResult::Resync);
    assert_eq!(buffer.pop(&mut out), Pop::Empty);
    for seq in 5_002..=5_004 {
        push_frame(&mut buffer, seq, u64::from(seq - 4_997));
    }
    assert!(packet(&mut buffer, 5_001, &mut out));
    assert_eq!(push_frame(&mut buffer, 0, 8), PushResult::Resync);
    assert_eq!(buffer.stats().resyncs, 2);
    assert_eq!(buffer.level_ms(), 10);
    assert_eq!(buffer.pop(&mut out), Pop::Empty);
}

#[test]
fn ninety_ms_burst_is_kept_and_overflowing_burst_trims_to_target() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    for seq in 0..9 {
        buffer.push(
            seq,
            HOST_US + u64::from(seq) * FRAME_US,
            &seq.to_be_bytes(),
            RECV_US + 200_000,
        );
    }
    assert_eq!(buffer.level_ms(), 90);
    assert_eq!(buffer.stats().overflow_drops, 0);
    buffer.reset();
    for seq in 0..20 {
        buffer.push(
            seq,
            HOST_US + u64::from(seq) * FRAME_US,
            &seq.to_be_bytes(),
            RECV_US + 200_000,
        );
    }
    assert!(buffer.stats().overflow_drops > 0);
    assert_eq!(buffer.level_ms(), buffer.target_ms());
    let first = 20 - (buffer.level_ms() / 10) as u16;
    let mut out = [0; MAX_PACKET];
    assert!(packet(&mut buffer, first, &mut out));
    assert!(!packet(&mut buffer, first + 1, &mut out));
}

#[test]
fn silence_conceals_three_frames_then_rebuffers_from_resumed_sequence() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for seq in 0..100 {
        push_frame(&mut buffer, seq, seq.into());
        buffer.pop(&mut out);
    }
    for seq in 97..100 {
        packet(&mut buffer, seq, &mut out);
    }
    for seq in 100..103 {
        assert_eq!(buffer.pop(&mut out), Pop::Missing { seq, fec_len: None });
    }
    for _ in 0..44 {
        assert_eq!(buffer.pop(&mut out), Pop::Empty);
    }
    assert_eq!(buffer.stats().underruns, 3);
    assert_eq!(buffer.stats().lost, 0);
    // The timestamp jump distinguishes this from packets delayed by the network.
    assert_eq!(push_frame(&mut buffer, 100, 150), PushResult::Resync);
    assert_eq!(buffer.stats().late, 0);
    assert_eq!(buffer.pop(&mut out), Pop::Empty);
    for seq in 101..107 {
        assert_eq!(
            push_frame(&mut buffer, seq, u64::from(seq) + 50),
            PushResult::Accepted
        );
    }
    assert!(packet(&mut buffer, 100, &mut out));
    assert!(!packet(&mut buffer, 101, &mut out));
}

#[test]
fn delayed_network_burst_does_not_revive_already_concealed_frames() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for seq in 0..100 {
        push_frame(&mut buffer, seq, seq.into());
        buffer.pop(&mut out);
    }
    for seq in 97..100 {
        packet(&mut buffer, seq, &mut out);
    }
    for seq in 100..103 {
        assert_eq!(buffer.pop(&mut out), Pop::Missing { seq, fec_len: None });
    }
    for _ in 0..24 {
        assert_eq!(buffer.pop(&mut out), Pop::Empty);
    }
    for seq in 100..=130 {
        let result = buffer.push(
            seq,
            HOST_US + u64::from(seq) * FRAME_US,
            &seq.to_be_bytes(),
            RECV_US + 1_300_000,
        );
        assert_eq!(
            result,
            if seq < 103 {
                PushResult::Late
            } else {
                PushResult::Accepted
            }
        );
    }
    assert_eq!(buffer.stats().late, 3);
    assert_eq!(buffer.stats().resyncs, 0);
    assert!(buffer.level_ms() <= 100);
    let first = 131 - (buffer.level_ms() / 10) as u16;
    assert!(first >= 103);
    assert!(packet(&mut buffer, first, &mut out));
}

#[test]
fn sustained_high_level_skips_exactly_one_frame_after_one_hundred_pops() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for seq in 0..7 {
        push_frame(&mut buffer, seq, seq.into());
    }
    for seq in 0..99 {
        assert_eq!(buffer.level_ms(), 70);
        assert!(!packet(&mut buffer, seq, &mut out));
        push_frame(&mut buffer, seq + 7, u64::from(seq + 7));
    }
    assert_eq!(buffer.stats().accelerated, 0);
    assert!(packet(&mut buffer, 100, &mut out));
    assert_eq!(buffer.stats().accelerated, 1);
    assert_eq!(buffer.stats().overflow_drops, 0);
    assert!(!packet(&mut buffer, 101, &mut out));
    assert_eq!(buffer.stats().accelerated, 1);
}

#[test]
fn invalid_payloads_do_not_change_state_and_maximum_payload_roundtrips() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let before = buffer.stats();
    assert_eq!(buffer.push(0, 0, &[], 0), PushResult::Invalid);
    assert_eq!(
        buffer.push(5_000, 0, &[0; MAX_PACKET + 1], 0),
        PushResult::Invalid
    );
    assert_eq!(buffer.stats(), before);
    for seq in 0..4 {
        assert_eq!(
            buffer.push(
                seq,
                u64::from(seq) * FRAME_US,
                &[42; MAX_PACKET],
                u64::from(seq) * FRAME_US
            ),
            PushResult::Accepted
        );
    }
    let mut out = [0; MAX_PACKET];
    assert!(matches!(
        buffer.pop(&mut out),
        Pop::Packet {
            seq: 0,
            len: MAX_PACKET,
            ..
        }
    ));
    assert_eq!(out, [42; MAX_PACKET]);
}

#[test]
fn reset_clears_playout_statistics_and_adaptation() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for seq in 0..4 {
        push_frame(&mut buffer, seq, seq.into());
    }
    packet(&mut buffer, 0, &mut out);
    assert_eq!(push_frame(&mut buffer, 0, 0), PushResult::Late);
    buffer.reset();
    assert_eq!(
        buffer.stats(),
        JitterStats {
            target_ms: 40,
            ..JitterStats::default()
        }
    );
    assert_eq!(buffer.pop(&mut out), Pop::Empty);
    for seq in 0..4 {
        assert_eq!(
            push_frame(&mut buffer, seq, seq.into()),
            PushResult::Accepted
        );
    }
    assert!(!packet(&mut buffer, 0, &mut out));
}

#[test]
fn short_output_slice_does_not_consume_a_packet() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    for seq in 0..4 {
        push_frame(&mut buffer, seq, seq.into());
    }
    assert_eq!(buffer.pop(&mut []), Pop::Empty);
    assert_eq!(buffer.level_ms(), 40);
    packet(&mut buffer, 0, &mut [0; MAX_PACKET]);
}

#[test]
fn negative_transit_and_extreme_timestamps_do_not_overflow() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    for seq in 0..4 {
        assert_eq!(
            buffer.push(
                seq,
                HOST_US + u64::from(seq) * FRAME_US,
                &[1],
                u64::from(seq) * FRAME_US
            ),
            PushResult::Accepted
        );
    }
    assert_eq!(buffer.target_ms(), 40);
    // Retain both extremes; with the default window the first sample would
    // correctly expire when receive time jumps from zero to u64::MAX.
    let mut buffer = JitterBuffer::new(JitterConfig {
        window_us: u64::MAX,
        ..JitterConfig::default()
    });
    assert_eq!(buffer.push(0, u64::MAX, &[1], 0), PushResult::Accepted);
    assert_eq!(buffer.push(1, 0, &[2], u64::MAX), PushResult::Accepted);
    assert_eq!(buffer.target_ms(), 80);
}

#[test]
fn full_small_capacity_discards_oldest_without_sequence_slot_collisions() {
    let cfg = JitterConfig {
        capacity: 5,
        ..JitterConfig::default()
    };
    let mut buffer = JitterBuffer::new(cfg);
    for frame in 0..7_u16 {
        assert_eq!(
            push_frame(&mut buffer, 65_534_u16.wrapping_add(frame), frame.into()),
            PushResult::Accepted
        );
    }
    assert_eq!(buffer.stats().overflow_drops, 2);
    assert_eq!(buffer.level_ms(), 50);
    let mut out = [0; MAX_PACKET];
    assert!(packet(&mut buffer, 0, &mut out));
    for seq in 1..5 {
        assert!(!packet(&mut buffer, seq, &mut out));
    }
}

#[test]
fn rebuffering_starts_at_oldest_received_packet_and_allows_reordering_above_floor() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    let mut out = [0; MAX_PACKET];
    for seq in 0..4 {
        push_frame(&mut buffer, seq, seq.into());
    }
    for seq in 0..4 {
        packet(&mut buffer, seq, &mut out);
    }
    for seq in 4..7 {
        assert_eq!(buffer.pop(&mut out), Pop::Missing { seq, fec_len: None });
    }
    assert_eq!(buffer.pop(&mut out), Pop::Empty);
    // 7 and 8 never arrive. 10 arrives before 9; neither was concealed.
    assert_eq!(push_frame(&mut buffer, 10, 10), PushResult::Accepted);
    assert_eq!(buffer.level_ms(), 10);
    assert_eq!(push_frame(&mut buffer, 9, 9), PushResult::Accepted);
    assert_eq!(buffer.level_ms(), 20);
    assert_eq!(push_frame(&mut buffer, 6, 6), PushResult::Late);
    for seq in 11..16 {
        assert_eq!(
            push_frame(&mut buffer, seq, seq.into()),
            PushResult::Accepted
        );
    }
    assert_eq!(buffer.pop(&mut out), Pop::Empty); // target was raised to 80
    push_frame(&mut buffer, 16, 16);
    assert!(packet(&mut buffer, 9, &mut out));
    assert!(!packet(&mut buffer, 10, &mut out));
}

#[test]
fn percentile_ignores_less_than_five_percent_high_outliers() {
    let mut buffer = JitterBuffer::new(JitterConfig::default());
    for seq in 0..43 {
        // 40 baseline samples, followed by three 60 ms high transits.
        let extra = if seq >= 40 { 60_000 } else { 0 };
        assert_eq!(
            buffer.push(
                seq,
                HOST_US + u64::from(seq) * FRAME_US - extra,
                &[1],
                RECV_US + u64::from(seq) * FRAME_US
            ),
            PushResult::Accepted
        );
        assert_eq!(buffer.target_ms(), if seq < 42 { 40 } else { 80 });
    }
}

#[test]
fn sample_history_retains_only_latest_two_hundred_fifty_six_samples() {
    let mut buffer = JitterBuffer::new(JitterConfig {
        window_us: u64::MAX,
        ..JitterConfig::default()
    });
    let mut out = [0; MAX_PACKET];
    for seq in 0..=1_356 {
        let extra = if seq == 0 { 0 } else { 60_000 };
        assert_eq!(
            buffer.push(
                seq,
                HOST_US + u64::from(seq) * FRAME_US - extra,
                &[1],
                RECV_US + u64::from(seq) * FRAME_US
            ),
            PushResult::Accepted
        );
        buffer.pop(&mut out);
        if (1..756).contains(&seq) {
            assert_eq!(buffer.target_ms(), 80, "at frame {seq}");
        }
        if seq == 756 {
            assert_eq!(buffer.target_ms(), 70);
        }
    }
    // The old minimum would keep the target at 80 if history were unbounded.
    assert_eq!(buffer.target_ms(), 40);
}

#[test]
fn samples_expire_by_receive_time_before_the_count_limit() {
    let mut buffer = JitterBuffer::new(JitterConfig {
        window_us: 200_000,
        ..JitterConfig::default()
    });
    let mut out = [0; MAX_PACKET];
    for seq in 0..=521 {
        let extra = if seq == 0 { 0 } else { 60_000 };
        buffer.push(
            seq,
            HOST_US + u64::from(seq) * FRAME_US - extra,
            &[1],
            RECV_US + u64::from(seq) * FRAME_US,
        );
        buffer.pop(&mut out);
        if (1..521).contains(&seq) {
            assert_eq!(buffer.target_ms(), 80, "at frame {seq}");
        }
    }
    assert_eq!(buffer.target_ms(), 70);
}

#[test]
fn demand_rising_below_current_target_restarts_slow_decrease_timer() {
    let mut buffer = JitterBuffer::new(JitterConfig {
        window_us: 20_000,
        ..JitterConfig::default()
    });
    let mut out = [0; MAX_PACKET];
    for seq in 0..4 {
        push_frame(&mut buffer, seq, seq.into());
    }
    packet(&mut buffer, 0, &mut out);
    for _ in 0..4 {
        assert_eq!(push_frame(&mut buffer, 0, 0), PushResult::Late);
    }
    for seq in 4..=900 {
        let extra = if seq == 400 { 30_000 } else { 0 };
        buffer.push(
            seq,
            HOST_US + u64::from(seq) * FRAME_US - extra,
            &seq.to_be_bytes(),
            RECV_US + u64::from(seq) * FRAME_US,
        );
        buffer.pop(&mut out);
        assert_eq!(
            buffer.target_ms(),
            if seq < 900 { 80 } else { 70 },
            "at frame {seq}"
        );
    }
}

#[test]
fn equal_receive_timestamps_evict_samples_in_arrival_order() {
    let mut buffer = JitterBuffer::new(JitterConfig {
        window_us: u64::MAX,
        ..JitterConfig::default()
    });
    for seq in 0..=257 {
        let extra = if seq == 1 { 0 } else { 60_000 };
        assert_eq!(
            buffer.push(seq, HOST_US - extra, &[1], RECV_US),
            PushResult::Accepted
        );
    }
    assert_eq!(buffer.target_ms(), 80);
    assert_eq!(
        buffer.push(258, HOST_US + 5_000_000 - 60_000, &[1], RECV_US + 5_000_000),
        PushResult::Accepted
    );
    assert_eq!(buffer.target_ms(), 70);
}
