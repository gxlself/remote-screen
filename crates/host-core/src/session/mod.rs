//! session 模块。见 docs/dev/06-host.md host-core。

pub mod approval;
pub mod conn;
pub mod handlers;
pub mod handshake;
pub mod manager;

// 会话功能的逻辑层（各自独立，由会话运行时驱动）。
pub mod activity;
pub mod clipboard;
pub mod cursor;
pub mod displays;
pub mod idle;
pub mod journal;
pub mod power;
pub mod roster;
pub mod streamgate;
pub mod takeover;
