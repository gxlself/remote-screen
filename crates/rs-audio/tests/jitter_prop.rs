use proptest::prelude::*;
use rs_audio::jitter::{JitterBuffer, JitterConfig, Pop, PushResult};
use rs_audio::{FRAME_US, MAX_PACKET};

fn check_levels(buffer: &JitterBuffer) -> Result<(), TestCaseError> {
    let stats = buffer.stats();
    prop_assert!((40..=80).contains(&stats.target_ms));
    prop_assert_eq!(stats.target_ms % 10, 0);
    prop_assert!(stats.level_ms <= 80 + 20 + 10, "{stats:?}");
    prop_assert_eq!(stats.target_ms, buffer.target_ms());
    prop_assert_eq!(stats.level_ms, buffer.level_ms());
    Ok(())
}

fn check_output(result: Pop, out: &[u8], previous: &mut Option<u16>) -> Result<(), TestCaseError> {
    let seq = match result {
        Pop::Packet { seq, len, .. } => {
            prop_assert_eq!(len, 2);
            prop_assert_eq!(&out[..len], &seq.to_be_bytes());
            seq
        }
        Pop::Missing { seq, fec_len } => {
            if let Some(len) = fec_len {
                prop_assert_eq!(len, 2);
                prop_assert_eq!(&out[..len], &seq.wrapping_add(1).to_be_bytes());
            }
            seq
        }
        Pop::Empty => return Ok(()),
    };
    if let Some(last) = *previous {
        prop_assert!(seq.wrapping_sub(last) as i16 > 0, "{last} -> {seq}");
    }
    *previous = Some(seq);
    Ok(())
}

proptest! {
    #![proptest_config(ProptestConfig {
        cases: 192,
        // Do not create regression artifacts outside the task's owned files.
        failure_persistence: None,
        ..ProptestConfig::default()
    })]

    #[test]
    fn random_network_preserves_order_bounds_and_fec(
        start in any::<u16>(),
        // Loss, duplication, delay, silence, and variable playout pacing.
        frames in prop::collection::vec((0_u8..10, any::<bool>(), 0_u64..250_001, 0_u8..5, 5_000_u64..30_001), 1..500),
        jump_frame in 0_usize..500,
        negative_clock_offset in any::<bool>(),
    ) {
        let mut arrivals = Vec::new();
        let mut pop_times = Vec::new();
        let mut seq = start;
        let mut pop_at = 0;
        for (frame, &(loss, duplicate, delay, silence, pop_step)) in frames.iter().enumerate() {
            let capture = frame as u64 * FRAME_US;
            while pop_at <= capture {
                pop_times.push(pop_at);
                pop_at += pop_step;
            }
            if frame == jump_frame {
                seq = seq.wrapping_add(5_000);
            }
            if silence == 0 {
                continue; // Suppression does not advance the sender sequence.
            }
            if loss != 0 {
                arrivals.push((capture + delay, seq, capture));
                if duplicate {
                    arrivals.push((capture + delay + 1_234, seq, capture));
                }
            }
            seq = seq.wrapping_add(1); // A lost send attempt still advances it.
        }
        let last_arrival = arrivals.iter().map(|&(recv, _, _)| recv).max().unwrap_or(0);
        let end = last_arrival.max(frames.len() as u64 * FRAME_US) + 500_000;
        while pop_at <= end {
            pop_times.push(pop_at);
            pop_at += FRAME_US;
        }
        arrivals.sort_unstable();
        let mut next = 0;
        let mut buffer = JitterBuffer::new(JitterConfig::default());
        let mut out = [0; MAX_PACKET];
        let mut previous = None;
        let capture_base = if negative_clock_offset { 20_000_000 } else { 1_000_000 };
        for pop_time in pop_times {
            while let Some(&(recv, seq, capture)) = arrivals.get(next) {
                if recv > pop_time {
                    break;
                }
                let result = buffer.push(seq, capture_base + capture, &seq.to_be_bytes(), 10_000_000 + recv);
                if result == PushResult::Resync {
                    previous = None;
                }
                prop_assert_ne!(result, PushResult::Invalid);
                check_levels(&buffer)?;
                next += 1;
            }
            check_output(buffer.pop(&mut out), &out, &mut previous)?;
            check_levels(&buffer)?;
        }
    }

    #[test]
    fn arbitrary_api_inputs_and_timestamp_extremes_do_not_panic(
        operations in prop::collection::vec((any::<u16>(), any::<u64>(), any::<u64>(), 0_usize..1_280, 0_u8..8), 1..400),
        capacity in 0_usize..70,
    ) {
        let mut buffer = JitterBuffer::new(JitterConfig { capacity, ..JitterConfig::default() });
        let payload = [3; MAX_PACKET + 4];
        let mut out = [0; MAX_PACKET];
        let mut previous = None;
        for (seq, capture, recv, len, action) in operations {
            match action {
                0 => {
                    buffer.reset();
                    previous = None;
                }
                1..=3 => {
                    let result = buffer.push(seq, capture, &payload[..len], recv);
                    if result == PushResult::Resync {
                        previous = None;
                    }
                    prop_assert_eq!(result == PushResult::Invalid, len == 0 || len > MAX_PACKET);
                }
                _ => {
                    let result = buffer.pop(&mut out);
                    let seq = match result {
                        Pop::Packet { seq, len, .. } => {
                            prop_assert!((1..=MAX_PACKET).contains(&len));
                            prop_assert!(out[..len].iter().all(|&byte| byte == 3));
                            Some(seq)
                        }
                        Pop::Missing { seq, fec_len } => {
                            if let Some(len) = fec_len {
                                prop_assert!((1..=MAX_PACKET).contains(&len));
                                prop_assert!(out[..len].iter().all(|&byte| byte == 3));
                            }
                            Some(seq)
                        }
                        Pop::Empty => None,
                    };
                    if let Some(seq) = seq {
                        if let Some(last) = previous {
                            prop_assert!(seq.wrapping_sub(last) as i16 > 0);
                        }
                        previous = Some(seq);
                    }
                }
            }
            check_levels(&buffer)?;
        }
    }
}
