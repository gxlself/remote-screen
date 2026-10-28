#![no_main]

use arbitrary::{Arbitrary, Unstructured};
use libfuzzer_sys::fuzz_target;
use rsp_core::video::{Reassembler, ReassemblyEvent, MAX_REASSEMBLY_BYTES, MAX_SLOTS};

const MAX_OPERATIONS: usize = 256;
const MAX_DATAGRAM_BYTES: usize = 1500;

#[derive(Debug)]
enum Operation {
    Push { datagram: Vec<u8>, delta_us: u64 },
    Tick { delta_us: u64 },
    SetFps(u32),
    PauseDeadlines,
}

impl<'a> Arbitrary<'a> for Operation {
    fn arbitrary(input: &mut Unstructured<'a>) -> arbitrary::Result<Self> {
        match input.arbitrary::<u8>()? % 4 {
            0 => {
                let len = usize::from(input.arbitrary::<u16>()?) % (MAX_DATAGRAM_BYTES + 1);
                let datagram = input.bytes(len)?.to_vec();
                Ok(Self::Push {
                    datagram,
                    delta_us: input.arbitrary::<u64>()?,
                })
            }
            1 => Ok(Self::Tick {
                delta_us: input.arbitrary::<u64>()?,
            }),
            2 => Ok(Self::SetFps(input.arbitrary::<u32>()?)),
            _ => Ok(Self::PauseDeadlines),
        }
    }
}

fn check_events(events: &[ReassemblyEvent], buffered_before: usize, added_bytes: usize) {
    let buffered_bound = buffered_before.saturating_add(added_bytes);
    for event in events {
        if let ReassemblyEvent::Frame(frame) = event {
            assert!(
                frame.data.len() <= buffered_bound,
                "an output frame is longer than the bytes available to the reassembler"
            );
        }
    }
}

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    let initial_fps = input.arbitrary::<u32>().unwrap_or(60);
    let operations = usize::from(input.arbitrary::<u16>().unwrap_or(0)) % (MAX_OPERATIONS + 1);
    let mut reassembler = Reassembler::with_fps(initial_fps);
    let mut now_us = 0u64;
    let mut previous_now_us = now_us;

    for _ in 0..operations {
        let Ok(operation) = Operation::arbitrary(&mut input) else {
            break;
        };
        let buffered_before = reassembler.buffered_bytes();
        let mut added_bytes = 0usize;
        let events = match operation {
            Operation::Push { datagram, delta_us } => {
                now_us = now_us.saturating_add(delta_us);
                added_bytes = datagram.len();
                reassembler.push(&datagram, now_us).unwrap_or_default()
            }
            Operation::Tick { delta_us } => {
                now_us = now_us.saturating_add(delta_us);
                reassembler.tick(now_us)
            }
            Operation::SetFps(fps) => {
                reassembler.set_fps(fps);
                Vec::new()
            }
            Operation::PauseDeadlines => {
                // Use the same timestamp for both endpoints: this is always a
                // nonnegative, ordered pause and does not advance the clock.
                reassembler.pause_deadlines(now_us, now_us);
                Vec::new()
            }
        };

        assert!(
            now_us >= previous_now_us,
            "the fuzz operation clock went backwards"
        );
        previous_now_us = now_us;
        check_events(&events, buffered_before, added_bytes);
        assert!(
            reassembler.slot_count() <= MAX_SLOTS,
            "reassembler retained more slots than its limit"
        );
        assert!(
            reassembler.buffered_bytes() <= MAX_REASSEMBLY_BYTES,
            "reassembler retained more bytes than its limit"
        );
    }
});
