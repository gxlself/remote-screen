//! 声音：Opus 封装、分帧、静音门限、抖动缓冲、播放器、测试信号与指标。见 docs/dev/03 §8.3。
pub mod codec;
pub mod framer;
pub mod gen;
pub mod jitter;
pub mod metrics;
pub mod pcm;
pub mod playout;
pub mod silence;

pub const SAMPLE_RATE: u32 = 48_000;
pub const CHANNELS: usize = 2;
pub const FRAME_SAMPLES: usize = 480; // 每声道，10 ms
pub const FRAME_LEN: usize = FRAME_SAMPLES * CHANNELS; // 960，交错
pub const FRAME_US: u64 = 10_000;
pub const MAX_PACKET: usize = 1275;

#[derive(Debug, thiserror::Error)]
pub enum AudioError {
    #[error("opus {function}: {}", code.description())]
    Codec {
        function: &'static str,
        code: opus::ErrorCode,
    },
    #[error("invalid: {0}")]
    Invalid(&'static str),
}
