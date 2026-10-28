use std::{collections::BTreeSet, hint::black_box, time::Instant};

use proptest::prelude::*;
use rsp_core::fec::{self, FecError};
use serde::Deserialize;
use sha2::{Digest, Sha256};

fn next_u64(state: &mut u64) -> u64 {
    // xorshift64*: tiny deterministic PRNG used by the vectors and simulations.
    *state ^= *state >> 12;
    *state ^= *state << 25;
    *state ^= *state >> 27;
    state.wrapping_mul(2_685_821_657_736_338_717)
}

fn make_data(k: usize, shard_len: usize, seed: u64) -> Vec<Vec<u8>> {
    let mut state = seed;
    (0..k)
        .map(|_| (0..shard_len).map(|_| next_u64(&mut state) as u8).collect())
        .collect()
}

fn sha256_hex(chunks: impl IntoIterator<Item = impl AsRef<[u8]>>) -> String {
    let mut hasher = Sha256::new();
    for chunk in chunks {
        hasher.update(chunk);
    }
    let digest = hasher.finalize();
    digest.iter().map(|byte| format!("{byte:02x}")).collect()
}

fn recover_with_removed(
    data: &[Vec<u8>],
    parity: &[Vec<u8>],
    removed: &BTreeSet<usize>,
) -> Result<Vec<Vec<u8>>, FecError> {
    let mut shards = Vec::with_capacity(data.len() + parity.len() - removed.len());
    for (idx, shard) in data.iter().enumerate() {
        if !removed.contains(&idx) {
            shards.push((idx, shard.as_slice()));
        }
    }
    for (idx, shard) in parity.iter().enumerate() {
        let idx = data.len() + idx;
        if !removed.contains(&idx) {
            shards.push((idx, shard.as_slice()));
        }
    }
    fec::recover(data.len(), parity.len(), data[0].len(), &shards)
}

#[test]
fn parity_count_and_ratio_follow_protocol() {
    assert_eq!(fec::KEYFRAME_MIN_REDUNDANCY, 0.25);
    assert_eq!(fec::parity_count(1, 0.0), 0);
    assert_eq!(fec::parity_count(1, 0.01), 1);
    assert_eq!(fec::parity_count(7, 0.05), 1);
    assert_eq!(fec::parity_count(120, 0.30), 36);
    assert_eq!(fec::parity_count(4096, 1.0), 64);
    assert_eq!(fec::redundancy_ratio(0.0), 0.05);
    assert_eq!(fec::redundancy_ratio(0.01), 0.09);
    assert_eq!(fec::redundancy_ratio(0.10), 0.30);
    assert_eq!(fec::redundancy_ratio(-1.0), 0.05);
}

#[test]
fn invalid_parameters_and_bad_shards_are_reported() {
    assert_eq!(
        fec::encode(&[], 0),
        Err(FecError::InvalidParams {
            k: 0,
            m: 0,
            shard_len: 0,
        })
    );
    assert_eq!(
        fec::encode(&[&[1, 2, 3]], 0),
        Err(FecError::InvalidParams {
            k: 1,
            m: 0,
            shard_len: 3,
        })
    );
    let data = [vec![1, 2], vec![3, 4]];
    let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    assert_eq!(
        fec::encode(&refs, 65),
        Err(FecError::InvalidParams {
            k: 2,
            m: 65,
            shard_len: 2,
        })
    );
    assert_eq!(
        fec::recover(2, 1, 3, &[]),
        Err(FecError::InvalidParams {
            k: 2,
            m: 1,
            shard_len: 3,
        })
    );

    let shards = [(0, &[1, 2][..]), (3, &[3, 4][..])];
    assert_eq!(
        fec::recover(2, 1, 2, &shards),
        Err(FecError::BadShard { idx: 3 })
    );
    let shards = [(0, &[1][..]), (1, &[3, 4][..])];
    assert_eq!(
        fec::recover(2, 1, 2, &shards),
        Err(FecError::BadShard { idx: 0 })
    );
}

#[test]
fn roundtrip_selected_loss_patterns() {
    let ks = [1, 2, 7, 120, 300, 4096];
    let ms = [0, 1, 2, 12, 64];
    for (k_index, &k) in ks.iter().enumerate() {
        for &m in &ms {
            let data = make_data(k, 2, 0x1234_5678_9abc_def0 ^ k_index as u64 ^ m as u64);
            let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
            let parity = fec::encode(&refs, m).expect("valid test parameters");
            assert_eq!(parity.len(), m);

            let mut patterns = vec![BTreeSet::new()];
            if m > 0 {
                patterns.push((k..k + m).collect());
                patterns.push((0..m.min(k)).collect());
                patterns.push((0..=m).collect());
                if k == 1 {
                    patterns.push([0].into_iter().collect());
                }
            } else {
                patterns.push([0].into_iter().collect());
            }

            for (pattern_index, removed) in patterns.iter().enumerate() {
                let recovered = recover_with_removed(&data, &parity, removed);
                if removed.len() <= m {
                    assert_eq!(
                        recovered,
                        Ok(data.clone()),
                        "k={k}, m={m}, pattern={pattern_index}"
                    );
                } else {
                    assert_eq!(
                        recovered,
                        Err(FecError::NotEnoughShards {
                            have: k + m - removed.len(),
                            need: k,
                        }),
                        "k={k}, m={m}, pattern={pattern_index}"
                    );
                }
            }
        }
    }
}

#[derive(Deserialize)]
struct FecVector {
    k: usize,
    m: usize,
    shard_len: usize,
    seed: u64,
    data_sha256: String,
    parity_sha256: Vec<String>,
}

fn check_vector(json: &str) {
    let vector: FecVector = serde_json::from_str(json).expect("valid FEC vector JSON");
    let data = make_data(vector.k, vector.shard_len, vector.seed);
    let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    let parity = fec::encode(&refs, vector.m).expect("valid vector parameters");
    let parity_hashes: Vec<String> = parity.iter().map(|shard| sha256_hex([shard])).collect();
    assert_eq!(sha256_hex(&data), vector.data_sha256);
    assert_eq!(parity_hashes, vector.parity_sha256);
    let removed: BTreeSet<usize> = (0..vector.m).collect();
    assert_eq!(recover_with_removed(&data, &parity, &removed), Ok(data));
}

#[test]
fn fixed_fec_vectors_are_stable() {
    check_vector(include_str!("vectors/fec/k1_m1.json"));
    check_vector(include_str!("vectors/fec/k7_m2.json"));
    check_vector(include_str!("vectors/fec/k120_m12.json"));
}

#[derive(Clone, Copy, Debug)]
struct LossCase {
    loss: f32,
    threshold: u64,
}

fn frame_recovered(k: usize, m: usize, loss: LossCase, state: &mut u64) -> bool {
    let received = (0..k + m)
        .filter(|_| next_u64(state) % 1_000_000 >= loss.threshold)
        .count();
    received >= k
}

fn run_recovery_rate_table() -> Vec<(f32, f64)> {
    let cases = [
        LossCase {
            loss: 0.01,
            threshold: 10_000,
        },
        LossCase {
            loss: 0.03,
            threshold: 30_000,
        },
        LossCase {
            loss: 0.05,
            threshold: 50_000,
        },
        LossCase {
            loss: 0.10,
            threshold: 100_000,
        },
    ];
    let mut table = Vec::with_capacity(cases.len());
    println!("loss   recovered frames");
    println!("-----  ----------------");
    for loss in cases {
        let mut state = 0xfeed_face_cafe_babe;
        let mut recovered = 0usize;
        let mut total = 0usize;
        for &k in &[150usize, 8] {
            let m = fec::parity_count(k, fec::redundancy_ratio(loss.loss));
            for _ in 0..5_000 {
                recovered += usize::from(frame_recovered(k, m, loss, &mut state));
                total += 1;
            }
        }
        let rate = recovered as f64 / total as f64;
        println!(
            "{:>4.0}%   {rate:.3}% ({recovered}/{total})",
            loss.loss * 100.0
        );
        table.push((loss.loss, rate));
    }
    table
}

#[test]
fn recovery_rate_meets_protocol_targets() {
    let table = run_recovery_rate_table();
    assert!(
        table[0].1 >= 0.995,
        "1% recovery rate was {:.3}%",
        table[0].1 * 100.0
    );
    assert!(
        table[1].1 >= 0.98,
        "3% recovery rate was {:.3}%",
        table[1].1 * 100.0
    );
}

#[test]
#[ignore = "manual release performance check"]
fn performance_encode_and_recover() {
    let k = 171;
    let m = 52;
    let shard_len = 1170;
    let data = make_data(k, shard_len, 0x0bad_5eed);
    let data_refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
    let mut encode_times = Vec::with_capacity(200);
    let mut recover_times = Vec::with_capacity(200);

    // Keep the recovery-rate table in the manual output as well as in the
    // ordinary test, so the requested release command prints both sections.
    let _ = run_recovery_rate_table();
    for _ in 0..200 {
        let start = Instant::now();
        let parity = fec::encode(&data_refs, m).expect("valid benchmark parameters");
        encode_times.push(start.elapsed());

        let mut received = Vec::with_capacity(k);
        for (idx, shard) in data.iter().enumerate().skip(m).take(k - m) {
            received.push((idx, shard.as_slice()));
        }
        for (parity_idx, shard) in parity.iter().enumerate() {
            received.push((k + parity_idx, shard.as_slice()));
        }
        let start = Instant::now();
        let restored = fec::recover(k, m, shard_len, &received).expect("benchmark recovery");
        recover_times.push(start.elapsed());
        black_box(restored);
    }

    encode_times.sort_unstable();
    recover_times.sort_unstable();
    println!(
        "performance (k={k}, m={m}, shard_len={shard_len}, n=200): encode median={} us, recover median={} us",
        encode_times[99].as_micros(),
        recover_times[99].as_micros()
    );
}

proptest! {
    #![proptest_config(ProptestConfig { cases: 24, .. ProptestConfig::default() })]

    #[test]
    fn random_roundtrips(
        k_index in 0usize..6,
        m_index in 0usize..5,
        seed in any::<u64>(),
        loss_seed in any::<u64>(),
    ) {
        let k = [1, 2, 7, 120, 300, 4096][k_index];
        let m = [0, 1, 2, 12, 64][m_index];
        let shard_len = 2 * (1 + (seed as usize % 8));
        let data = make_data(k, shard_len, seed);
        let refs: Vec<&[u8]> = data.iter().map(Vec::as_slice).collect();
        let parity = fec::encode(&refs, m).expect("generated parameters are valid");

        let remove_count = if m == 0 { 0 } else { loss_seed as usize % (m + 1) };
        let mut ranked: Vec<(u64, usize)> = (0..k + m)
            .map(|idx| {
                let mut rank = loss_seed.wrapping_add(idx as u64);
                rank ^= rank >> 29;
                rank = rank.wrapping_mul(0x9e37_79b9_7f4a_7c15);
                (rank, idx)
            })
            .collect();
        ranked.sort_unstable();
        let removed: BTreeSet<usize> = ranked.into_iter().take(remove_count).map(|(_, idx)| idx).collect();
        prop_assert_eq!(recover_with_removed(&data, &parity, &removed), Ok(data));
    }
}
