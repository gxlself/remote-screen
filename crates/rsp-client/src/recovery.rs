//! 换路、后台与恢复。见 docs/dev/03-protocol.md §4.5、§10.4。
//!
//! 纯逻辑子模块：不碰网络，时间由调用方传入；由会话层驱动状态转换。

pub mod background;
pub mod driver;
pub mod failover;
pub mod liveness;
pub mod upgrade;
