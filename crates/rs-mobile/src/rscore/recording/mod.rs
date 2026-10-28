//! 会话录制接线（M3 V5）：录制挂点、写盘线程、回放「假会话」、uniffi 接口。
pub mod api;
pub mod dto;
pub mod export;
pub mod ffi_wrap;
pub mod notice;
pub mod playback;
pub mod recorder;
pub mod slot;
