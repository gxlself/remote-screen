#![no_main]

use std::collections::BTreeMap;

use arbitrary::Unstructured;
use libfuzzer_sys::fuzz_target;
use rsp_core::fec::{self, MAX_DATA_SHARDS, MAX_PARITY_SHARDS};

const MAX_SLOTS: usize = 8;
const MAX_RECORDS: usize = 64;
type Slot = Vec<(usize, Vec<u8>)>;
type Slots = BTreeMap<(u8, u32), Slot>;

fn data_shard_count(value: u16) -> usize {
    match value % 6 {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 7,
        4 => 16,
        _ => MAX_DATA_SHARDS,
    }
}

fn parity_shard_count(value: u8) -> usize {
    match value % 5 {
        0 => 0,
        1 => 1,
        2 => 2,
        3 => 12,
        _ => MAX_PARITY_SHARDS,
    }
}

fuzz_target!(|data: &[u8]| {
    let mut input = Unstructured::new(data);
    let rounds = input.arbitrary::<u8>().unwrap_or(0).min(32) as usize;
    let mut slots: Slots = BTreeMap::new();

    for _ in 0..rounds {
        let epoch = input.arbitrary::<u8>().unwrap_or(0);
        let frame_id = input.arbitrary::<u32>().unwrap_or(0);
        let k = data_shard_count(input.arbitrary::<u16>().unwrap_or(0));
        let m = parity_shard_count(input.arbitrary::<u8>().unwrap_or(0));
        let shard_len = input.arbitrary::<u16>().unwrap_or(0) as usize;
        let index = input.arbitrary::<u32>().unwrap_or(0) as usize;
        let payload_len = input.arbitrary::<u16>().unwrap_or(0).min(256) as usize;
        let Ok(payload) = input.bytes(payload_len) else {
            break;
        };

        let key = (epoch, frame_id);
        if !slots.contains_key(&key) && slots.len() >= MAX_SLOTS {
            if let Some(oldest) = slots.keys().next().copied() {
                slots.remove(&oldest);
            }
        }
        let slot = slots.entry(key).or_default();
        if slot.len() < MAX_RECORDS {
            slot.push((index, payload.to_vec()));
        }
        let received: Vec<(usize, &[u8])> = slot
            .iter()
            .map(|(idx, bytes)| (*idx, bytes.as_slice()))
            .collect();
        let result = fec::recover(k, m, shard_len, &received);
        if let Ok(shards) = result {
            assert_eq!(shards.len(), k);
            assert!(shards.iter().all(|shard| shard.len() == shard_len));
        }
        assert!(slots.len() <= MAX_SLOTS);
    }
});
