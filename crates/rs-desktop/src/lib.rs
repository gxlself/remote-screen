//! 电脑端（Mac / Windows）控制端：共享逻辑与平台胶水（M3 V6，见 07 · 电脑端）。
//!
//! 纯逻辑模块在所有目标上编译，Linux 上测；平台模块按 cfg 隔开。
//! 平台接口见 `platform`。

pub mod platform;

pub mod bridge;
pub mod controller;
pub mod frames;
pub mod keymap;
pub mod keys;
pub mod pointer;
pub mod scroll;
pub mod session;
pub mod winset;
// Windows 平台层的纯逻辑部分：所有目标编译，Linux 上测。
#[cfg(target_os = "macos")]
mod mac_ffi;
#[cfg(target_os = "macos")]
pub mod mac_signer;
#[cfg(target_os = "macos")]
pub mod mac_video;
#[cfg(target_os = "macos")]
pub mod mac_window;
#[cfg(windows)]
pub mod win_ffmpeg;
pub mod win_ffmpeg_abi;
pub mod win_model;
#[cfg(windows)]
pub mod win_present;
pub mod win_present_model;
#[cfg(windows)]
pub mod win_signer;
#[cfg(windows)]
pub mod win_window;
