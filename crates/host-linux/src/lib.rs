//! host-linux：Linux 平台层（M3 V8）。
//!
//! 纯逻辑模块在所有目标上编译，单元测试在 Linux 和 macOS 上都能跑；PipeWire、
//! xdg-desktop-portal、X11、FFmpeg 胶水只在 Linux 目标编译。见 docs/dev/06-host.md「Linux」。

pub mod capture_model;
pub mod clip_model;
pub mod enc_worker;
pub mod ff_model;
pub mod frame;
pub mod input_plan;
pub mod keymap;
pub mod keystore;
pub mod portal_model;
pub mod spa_pod;
pub mod sys_model;

#[cfg(target_os = "linux")]
pub mod audio;
#[cfg(target_os = "linux")]
pub mod ctl;
#[cfg(target_os = "linux")]
pub mod dl;
#[cfg(target_os = "linux")]
pub mod ff_encoder;
#[cfg(target_os = "linux")]
pub mod ffmpeg;
#[cfg(target_os = "linux")]
pub mod install;
#[cfg(target_os = "linux")]
pub mod logind;
#[cfg(target_os = "linux")]
pub mod notify;
#[cfg(target_os = "linux")]
pub mod platform;
#[cfg(target_os = "linux")]
pub mod portal;
#[cfg(target_os = "linux")]
pub mod portal_input;
#[cfg(target_os = "linux")]
pub mod pw;
#[cfg(target_os = "linux")]
pub mod pw_ffi;
#[cfg(target_os = "linux")]
pub mod sw_encoder;
#[cfg(target_os = "linux")]
pub mod update_appimage;
#[cfg(target_os = "linux")]
pub mod wl_capture;
#[cfg(target_os = "linux")]
pub mod x11_capture;
#[cfg(target_os = "linux")]
pub mod x11_clip;
#[cfg(target_os = "linux")]
pub mod x11_input;
