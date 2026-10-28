//! rs-bench：对照基准的时间码、解码、对时、采集、画质。
pub mod capture;
pub mod clock;
pub mod decode;
pub mod scene;
pub mod ssim;
pub mod synth;
pub mod timecode;

#[cfg(target_os = "macos")]
pub mod capture_mac;
