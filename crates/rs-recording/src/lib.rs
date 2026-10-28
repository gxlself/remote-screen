//! 会话录制：自有加密容器、录制目录、回放时间线与 MP4 导出（M3 V5，docs/dev/07-app.md「会话录制」）。
//! 纯 Rust，不依赖会话和平台；rs-mobile 的 `rscore::recording` 负责接线。
pub mod chunk;
pub mod crypto;
pub mod file;
pub mod mp4;
pub mod record;
pub mod store;
pub mod timeline;
