//! Reed-Solomon 视频冗余。见 docs/dev/03-protocol.md §5.2–5.3。

use reed_solomon_simd::{ReedSolomonDecoder, ReedSolomonEncoder};

/// Maximum number of data shards in one frame.
pub const MAX_DATA_SHARDS: usize = 4096;
/// Maximum number of parity shards in one frame.
pub const MAX_PARITY_SHARDS: usize = 64;
/// Minimum redundancy ratio for keyframes (03 §5.2).
pub const KEYFRAME_MIN_REDUNDANCY: f32 = 0.25;

/// Errors returned by FEC encoding and recovery.
#[derive(Debug, thiserror::Error, PartialEq, Eq)]
pub enum FecError {
    /// The shard counts or shard length are outside the protocol limits.
    #[error("invalid parameters: k={k} m={m} shard_len={shard_len}")]
    InvalidParams {
        /// Number of data shards.
        k: usize,
        /// Number of parity shards.
        m: usize,
        /// Bytes in each shard.
        shard_len: usize,
    },
    /// Fewer than k distinct shards were supplied to the decoder.
    #[error("not enough shards: have {have}, need {need}")]
    NotEnoughShards {
        /// Number of distinct shards supplied.
        have: usize,
        /// Number of shards required for recovery.
        need: usize,
    },
    /// A supplied shard has an index outside the frame or the wrong length.
    #[error("shard {idx} has wrong length")]
    BadShard {
        /// Shard index supplied by the caller.
        idx: usize,
    },
    /// Error reported by the Reed-Solomon implementation.
    #[error("codec: {0}")]
    Codec(String),
}

/// Computes the number of parity shards for a data-shard count and redundancy.
///
/// The result follows protocol section 03 §5.2: zero redundancy produces no
/// parity shards; otherwise the ceiling is clamped to the range 1..=64.
pub fn parity_count(k: usize, r: f32) -> usize {
    if r == 0.0 {
        return 0;
    }

    // `r` is specified as 0.0..=1.0 by the protocol. Treat non-finite and
    // negative values as zero so this helper remains total for callers that
    // calculate the ratio from external telemetry.
    if !r.is_finite() || r < 0.0 {
        return 0;
    }

    ((k as f32 * r).ceil().clamp(1.0, MAX_PARITY_SHARDS as f32)) as usize
}

/// Computes the FEC redundancy ratio from the EWMA packet-loss estimate.
pub fn redundancy_ratio(loss_ewma: f32) -> f32 {
    if !loss_ewma.is_finite() {
        return 0.05;
    }
    (0.05 + 4.0 * loss_ewma).clamp(0.05, 0.30)
}

fn validate_params(k: usize, m: usize, shard_len: usize) -> Result<(), FecError> {
    if !(1..=MAX_DATA_SHARDS).contains(&k)
        || m > MAX_PARITY_SHARDS
        || shard_len == 0
        || !shard_len.is_multiple_of(2)
    {
        return Err(FecError::InvalidParams { k, m, shard_len });
    }
    Ok(())
}

fn codec_error(error: reed_solomon_simd::Error) -> FecError {
    FecError::Codec(error.to_string())
}

/// Encodes k equal-size data shards into m parity shards.
pub fn encode(data: &[&[u8]], m: usize) -> Result<Vec<Vec<u8>>, FecError> {
    let k = data.len();
    let shard_len = data.first().map_or(0, |shard| shard.len());
    validate_params(k, m, shard_len)?;

    if data.iter().any(|shard| shard.len() != shard_len) {
        return Err(FecError::InvalidParams { k, m, shard_len });
    }

    if m == 0 {
        return Ok(Vec::new());
    }

    let mut encoder = ReedSolomonEncoder::new(k, m, shard_len).map_err(codec_error)?;
    for shard in data {
        encoder.add_original_shard(shard).map_err(codec_error)?;
    }
    let result = encoder.encode().map_err(codec_error)?;
    Ok(result.recovery_iter().map(<[u8]>::to_vec).collect())
}

/// Recovers all k data shards from any k distinct data/parity shards.
pub fn recover(
    k: usize,
    m: usize,
    shard_len: usize,
    shards: &[(usize, &[u8])],
) -> Result<Vec<Vec<u8>>, FecError> {
    validate_params(k, m, shard_len)?;

    let total = k + m;
    let mut seen = vec![false; total];
    let mut originals: Vec<Option<Vec<u8>>> = (0..k).map(|_| None).collect();
    let mut recovery: Vec<Option<Vec<u8>>> = (0..m).map(|_| None).collect();
    let mut have = 0;

    for &(idx, shard) in shards {
        if idx >= total {
            return Err(FecError::BadShard { idx });
        }
        if seen[idx] {
            continue;
        }
        seen[idx] = true;

        if shard.len() != shard_len {
            return Err(FecError::BadShard { idx });
        }
        have += 1;
        if idx < k {
            originals[idx] = Some(shard.to_vec());
        } else {
            recovery[idx - k] = Some(shard.to_vec());
        }
    }

    if have < k {
        return Err(FecError::NotEnoughShards { have, need: k });
    }

    // This is common for low-loss frames and avoids constructing the codec.
    if originals.iter().all(Option::is_some) {
        return Ok(originals.into_iter().flatten().collect());
    }

    // m == 0 cannot reach this point: with no parity, have >= k implies all
    // data shards are present. Keep the guard explicit for future codec changes.
    if m == 0 {
        return Err(FecError::NotEnoughShards { have, need: k });
    }

    let mut decoder = ReedSolomonDecoder::new(k, m, shard_len).map_err(codec_error)?;
    for (idx, shard) in originals.iter().enumerate() {
        if let Some(shard) = shard {
            decoder
                .add_original_shard(idx, shard)
                .map_err(codec_error)?;
        }
    }
    for (idx, shard) in recovery.iter().enumerate() {
        if let Some(shard) = shard {
            decoder
                .add_recovery_shard(idx, shard)
                .map_err(codec_error)?;
        }
    }

    let result = decoder.decode().map_err(codec_error)?;
    for (idx, shard) in result.restored_original_iter() {
        originals[idx] = Some(shard.to_vec());
    }

    originals
        .into_iter()
        .enumerate()
        .map(|(idx, shard)| {
            shard.ok_or_else(|| FecError::Codec(format!("missing restored shard {idx}")))
        })
        .collect()
}
