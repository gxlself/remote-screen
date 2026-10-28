//! Windows 平台层：捕获、编码、输入、服务与虚拟显示器。
//!
//! 纯逻辑模块在所有目标上编译，单元测试在 Linux 上跑；Win32 调用层只在
//! Windows 目标编译，见 docs/dev/06-host.md「Windows」。

pub mod account;
pub mod addr_watch_model;
pub mod audio_convert;
pub mod console_model;
pub mod firewall_model;
pub mod frame_gate;
pub mod input_raw;
pub mod keyfile;
pub mod msi;
pub mod paths;
pub mod pipe_proto;
pub mod proxy;
pub mod proxy_media;
pub mod session_platform;
pub mod session_run;
pub mod session_server;
pub mod supervisor;
pub mod svc_net_model;
pub mod svc_os_model;
pub mod svc_session_model;
pub mod user_dirs_model;
pub mod vdisplay_edid;
pub mod vdisplay_ioctl;
pub mod vdisplay_plan;
pub mod viewport_crop;
pub mod vpmf_model;
pub mod winproc_model;
pub mod wol;

#[cfg(windows)]
pub mod activity;
#[cfg(windows)]
pub mod addr_watch;
#[cfg(windows)]
pub mod audio;
#[cfg(windows)]
pub mod capture;
#[cfg(windows)]
pub mod clipboard;
#[cfg(windows)]
pub mod console;
#[cfg(windows)]
pub mod deploy;
#[cfg(windows)]
pub mod desktop;
#[cfg(windows)]
pub mod displays;
#[cfg(windows)]
pub mod dpapi;
#[cfg(windows)]
pub mod encode;
#[cfg(windows)]
pub mod ffmpeg;
#[cfg(windows)]
pub mod firewall;
#[cfg(windows)]
pub mod inject;
#[cfg(windows)]
pub mod mf_enc;
#[cfg(windows)]
pub mod nics;
#[cfg(windows)]
pub mod pipe;
#[cfg(windows)]
pub mod power;
#[cfg(windows)]
pub mod scm;
#[cfg(windows)]
pub mod spawn;
#[cfg(windows)]
pub mod svc_local;
pub mod svc_session;
#[cfg(windows)]
pub mod sysauth;
#[cfg(windows)]
pub mod tray;
#[cfg(windows)]
pub mod update;
#[cfg(windows)]
pub mod user_dirs;
#[cfg(windows)]
pub mod vdisplay;
#[cfg(windows)]
pub mod vdisplay_names;
#[cfg(windows)]
pub mod vproc;
#[cfg(windows)]
pub mod win_session;
#[cfg(windows)]
pub mod wts;
