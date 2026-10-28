//! 控制端窗口采集：公共类型与采集循环。
use std::fs::{self, File};
use std::io::{BufWriter, Write};
use std::path::{Path, PathBuf};
use std::thread;
use std::time::{Duration, Instant, SystemTime, UNIX_EPOCH};

use anyhow::{Context, Result};
use clap::Parser;
use serde::Serialize;

use crate::decode::{Rect, TrackEvent, Tracker, TrackerConfig};
use crate::timecode::{GrayImage, PxRect};

/// 一个可采集的窗口。
#[derive(Debug, Clone, PartialEq, Eq, serde::Serialize)]
pub struct WindowInfo {
    /// macOS CGWindowID；Windows HWND 数值。
    pub id: u64,
    pub pid: i32,
    /// macOS：bundle id；Windows：exe 文件名（小写，无路径）。
    pub app: String,
    pub title: String,
    /// 像素。
    pub width: u32,
    pub height: u32,
    pub on_screen: bool,
}

/// 采到的一帧：满幅亮度 0–255，行紧排（luma.len() == width * height）。
#[derive(Debug, Clone, PartialEq, Eq)]
pub struct TimedFrame {
    /// 这一帧在控制端屏幕上合成显示的时刻（本机 unix 微秒）。
    pub t_us: u64,
    pub width: u32,
    pub height: u32,
    pub luma: Vec<u8>,
}

/// 一路窗口采集。drop 时停止，之后不再回调。
pub trait FrameSource: Send {
    /// 最多等 timeout；超时返回 Ok(None)；窗口关闭等不可恢复的错误返回 Err。
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<TimedFrame>, String>;
    /// 因队列满丢掉的帧数（累计）。
    fn dropped(&self) -> u64;
}

/// 平台采集能力。
pub trait CapturePlatform {
    /// 写进 frames.jsonl 的 source，例如 "mac-sck"。
    fn name(&self) -> &'static str;
    /// 是否已有屏幕录制权限；request 为真时允许弹系统授权框。
    fn check_permission(&self, request: bool) -> bool;
    fn list_windows(&self) -> Result<Vec<WindowInfo>, String>;
    /// fps：最高采集帧率。
    fn open(&self, window: &WindowInfo, fps: u32) -> Result<Box<dyn FrameSource>, String>;
}

/// 本平台的采集实现；不支持的平台返回 None。
pub fn platform() -> Option<Box<dyn CapturePlatform>> {
    #[cfg(target_os = "macos")]
    {
        crate::capture_mac::platform()
    }
    #[cfg(not(target_os = "macos"))]
    {
        None
    }
}

/// 窗口筛选条件。
#[derive(Debug, Clone, Default)]
pub struct WindowFilter {
    pub app: Option<String>,
    pub title: Option<String>,
    pub pid: Option<i32>,
}

/// 在满足筛选条件的可见窗口中选择面积最大的窗口。
pub fn select_window(windows: &[WindowInfo], filter: &WindowFilter) -> Option<WindowInfo> {
    windows
        .iter()
        .filter(|window| {
            window.on_screen
                && window.width >= 64
                && window.height >= 64
                && filter
                    .app
                    .as_ref()
                    .is_none_or(|app| window.app.eq_ignore_ascii_case(app))
                && filter
                    .title
                    .as_ref()
                    .is_none_or(|title| window.title.contains(title))
                && filter.pid.is_none_or(|pid| window.pid == pid)
        })
        .fold(None, |best: Option<&WindowInfo>, window| {
            let replace = best.is_none_or(|current| {
                let area = u64::from(window.width) * u64::from(window.height);
                let current_area = u64::from(current.width) * u64::from(current.height);
                area > current_area || (area == current_area && window.id < current.id)
            });
            if replace {
                Some(window)
            } else {
                best
            }
        })
        .cloned()
}

/// 采集循环选项。
#[derive(Debug, Clone)]
pub struct CaptureOpts {
    /// 页面场景的时长；TIME 之后额外容许 10 秒。
    pub seconds: u32,
    pub calib_timeout: Duration,
    pub fps: u32,
    pub save_frames: bool,
    pub save_count: u32,
    pub save_interval: Duration,
    pub poll: Duration,
}

impl Default for CaptureOpts {
    fn default() -> Self {
        Self {
            seconds: 60,
            calib_timeout: Duration::from_secs(30),
            fps: 60,
            save_frames: false,
            save_count: 5,
            save_interval: Duration::from_secs(5),
            poll: Duration::from_millis(200),
        }
    }
}

#[derive(Debug, Clone, Copy, PartialEq, Eq)]
pub enum EndReason {
    EndCode,
    Timeout,
    CalibTimeout,
    SourceClosed,
    NoWindow,
}

impl EndReason {
    pub fn as_str(&self) -> &'static str {
        match self {
            Self::EndCode => "end-code",
            Self::Timeout => "timeout",
            Self::CalibTimeout => "calib-timeout",
            Self::SourceClosed => "source-closed",
            Self::NoWindow => "no-window",
        }
    }

    pub fn exit_code(&self) -> i32 {
        match self {
            Self::EndCode | Self::Timeout => 0,
            Self::CalibTimeout => 3,
            Self::SourceClosed => 4,
            Self::NoWindow => 5,
        }
    }
}

#[derive(Debug, Clone, PartialEq, Eq)]
pub struct CaptureSummary {
    pub reason: EndReason,
    pub frames: u64,
    pub decoded: u64,
    pub dropped: u64,
    pub saved: u32,
}

#[derive(Debug, Serialize)]
struct StartRecord<'a> {
    #[serde(rename = "type")]
    record_type: &'static str,
    schema: u8,
    tool: &'static str,
    source: &'a str,
    window: Option<&'a WindowInfo>,
    opts: StartOpts,
    t_us: u64,
}

#[derive(Debug, Serialize)]
struct StartOpts {
    seconds: u32,
    calib_timeout_s: u64,
    fps: u32,
}

#[derive(Debug, Serialize)]
struct CalibRecord {
    #[serde(rename = "type")]
    record_type: &'static str,
    t_us: u64,
    rect: [f64; 4],
    host_w: u32,
    host_h: u32,
    frame_w: u32,
    frame_h: u32,
}

#[derive(Debug, Serialize)]
struct FrameRecord<'a> {
    #[serde(rename = "type")]
    record_type: &'static str,
    t_us: u64,
    code: Option<u64>,
    #[serde(skip_serializing_if = "Option::is_none")]
    err: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct SavedRecord {
    #[serde(rename = "type")]
    record_type: &'static str,
    t_us: u64,
    path: String,
    code: u64,
}

#[derive(Debug, Serialize)]
struct EndRecord<'a> {
    #[serde(rename = "type")]
    record_type: &'static str,
    t_us: u64,
    reason: &'static str,
    frames: u64,
    decoded: u64,
    dropped: u64,
    #[serde(skip_serializing_if = "Option::is_none")]
    error: Option<&'a str>,
}

#[derive(Debug, Serialize)]
struct CheckRecord<'a> {
    platform: &'a str,
    permission: bool,
}

fn start_opts(opts: &CaptureOpts) -> StartOpts {
    StartOpts {
        seconds: opts.seconds,
        calib_timeout_s: opts.calib_timeout.as_secs(),
        fps: opts.fps,
    }
}

fn write_json_line<W: Write, T: Serialize>(writer: &mut W, value: &T) -> Result<()> {
    serde_json::to_writer(&mut *writer, value).context("encode capture JSON")?;
    writer.write_all(b"\n").context("write capture JSON line")?;
    Ok(())
}

fn create_parent(path: &Path) -> Result<()> {
    if let Some(parent) = path
        .parent()
        .filter(|parent| !parent.as_os_str().is_empty())
    {
        fs::create_dir_all(parent)
            .with_context(|| format!("create output directory {}", parent.display()))?;
    }
    Ok(())
}

fn open_writer(path: &Path) -> Result<BufWriter<File>> {
    create_parent(path)?;
    let file = File::create(path).with_context(|| format!("create {}", path.display()))?;
    Ok(BufWriter::new(file))
}

fn duration_us(duration: Duration) -> u64 {
    duration.as_micros().min(u64::MAX as u128) as u64
}

fn crop_coordinate(value: f64, limit: u32) -> u32 {
    if !value.is_finite() {
        return if value.is_sign_negative() { 0 } else { limit };
    }
    value.round().clamp(0.0, f64::from(limit)) as u32
}

fn save_frame(out: &Path, image: &GrayImage, rect: Rect, index: u32) -> Result<String> {
    let x0 = crop_coordinate(rect.x, image.width);
    let y0 = crop_coordinate(rect.y, image.height);
    let x1 = crop_coordinate(rect.x + rect.w, image.width);
    let y1 = crop_coordinate(rect.y + rect.h, image.height);
    let crop = image.crop(PxRect { x0, y0, x1, y1 });

    let parent = out.parent().filter(|parent| !parent.as_os_str().is_empty());
    let frames_dir = parent.map_or_else(|| PathBuf::from("frames"), |parent| parent.join("frames"));
    fs::create_dir_all(&frames_dir)
        .with_context(|| format!("create frame directory {}", frames_dir.display()))?;
    let filename = format!("frame-{index}.pgm");
    let path = frames_dir.join(&filename);
    fs::write(&path, crop.to_pgm())
        .with_context(|| format!("write saved frame {}", path.display()))?;
    Ok(format!("frames/{filename}"))
}

/// 从窗口采集、标定并写出 `frames.jsonl`。
///
/// `now_us` 返回本机 Unix 微秒，所有超时均使用此时钟；帧时刻只用于
/// JSONL 的逐帧记录和裁图间隔。锁定后尚未收到 TIME 时，从锁定时刻计时。
pub fn run_capture(
    source: &mut dyn FrameSource,
    window: &WindowInfo,
    source_name: &str,
    opts: &CaptureOpts,
    out: &Path,
    now_us: &mut dyn FnMut() -> u64,
) -> Result<CaptureSummary> {
    let mut writer = open_writer(out)?;
    let start_us = now_us();
    let start = StartRecord {
        record_type: "start",
        schema: 1,
        tool: "rs-bench capture",
        source: source_name,
        window: Some(window),
        opts: start_opts(opts),
        t_us: start_us,
    };
    write_json_line(&mut writer, &start)?;
    writer.flush().context("flush capture start")?;
    let mut last_flush_us = start_us;

    let mut tracker = Tracker::new(TrackerConfig::default());
    let mut locked = false;
    let mut locked_us = None;
    let mut first_time_us = None;
    let mut next_save_us = None;
    let mut frames = 0_u64;
    let mut decoded = 0_u64;
    let mut saved = 0_u32;
    let mut source_error = None;

    let calib_timeout_us = duration_us(opts.calib_timeout);
    let run_timeout_us = u64::from(opts.seconds)
        .saturating_add(10)
        .saturating_mul(1_000_000);

    let reason = loop {
        let current_us = now_us();
        if current_us.saturating_sub(last_flush_us) >= 1_000_000 {
            writer.flush().context("flush capture JSON")?;
            last_flush_us = current_us;
        }
        if !locked && current_us.saturating_sub(start_us) >= calib_timeout_us {
            break EndReason::CalibTimeout;
        }
        if locked {
            // 没有 TIME 时以锁定时刻兜底；首个 TIME 到达后重新开始计时。
            let timeout_start = first_time_us.or(locked_us);
            if timeout_start.is_some_and(|start| current_us.saturating_sub(start) >= run_timeout_us)
            {
                break EndReason::Timeout;
            }
        }

        let frame = match source.next_frame(opts.poll) {
            Ok(Some(frame)) => frame,
            Ok(None) => continue,
            Err(error) => {
                source_error = Some(error);
                break EndReason::SourceClosed;
            }
        };
        let image = match GrayImage::from_raw(frame.width, frame.height, frame.luma) {
            Ok(image) => image,
            Err(error) => {
                source_error = Some(error);
                break EndReason::SourceClosed;
            }
        };

        match tracker.push(&image) {
            TrackEvent::Locked {
                rect,
                host_w,
                host_h,
            } => {
                locked = true;
                locked_us = Some(now_us());
                let record = CalibRecord {
                    record_type: "calib",
                    t_us: frame.t_us,
                    rect: [rect.x, rect.y, rect.w, rect.h],
                    host_w,
                    host_h,
                    frame_w: frame.width,
                    frame_h: frame.height,
                };
                write_json_line(&mut writer, &record)?;
            }
            TrackEvent::Time(code) => {
                if first_time_us.is_none() {
                    first_time_us = Some(now_us());
                    next_save_us = Some(frame.t_us.saturating_add(duration_us(opts.save_interval)));
                }
                let record = FrameRecord {
                    record_type: "frame",
                    t_us: frame.t_us,
                    code: Some(code),
                    err: None,
                };
                write_json_line(&mut writer, &record)?;
                frames += 1;
                decoded += 1;

                if opts.save_frames
                    && saved < opts.save_count
                    && next_save_us.is_some_and(|next| frame.t_us >= next)
                {
                    let Some((rect, _, _)) = tracker.locked() else {
                        continue;
                    };
                    let index = saved + 1;
                    let path = save_frame(out, &image, rect, index)?;
                    let saved_record = SavedRecord {
                        record_type: "saved",
                        t_us: frame.t_us,
                        path,
                        code,
                    };
                    write_json_line(&mut writer, &saved_record)?;
                    saved += 1;
                    next_save_us = next_save_us
                        .map(|next| next.saturating_add(duration_us(opts.save_interval)));
                }
            }
            TrackEvent::Fail(error) if first_time_us.is_some() => {
                let record = FrameRecord {
                    record_type: "frame",
                    t_us: frame.t_us,
                    code: None,
                    err: Some(error.as_str()),
                };
                write_json_line(&mut writer, &record)?;
                frames += 1;
            }
            TrackEvent::End => {
                break EndReason::EndCode;
            }
            TrackEvent::Searching | TrackEvent::Calibrating | TrackEvent::Calib => {}
            TrackEvent::Fail(_) => {}
        }
    };

    let end = EndRecord {
        record_type: "end",
        t_us: now_us(),
        reason: reason.as_str(),
        frames,
        decoded,
        dropped: source.dropped(),
        error: source_error.as_deref(),
    };
    write_json_line(&mut writer, &end)?;
    writer.flush().context("flush capture end")?;

    Ok(CaptureSummary {
        reason,
        frames,
        decoded,
        dropped: end.dropped,
        saved,
    })
}

#[derive(Debug, Parser)]
#[command(name = "rs-bench capture", about = "capture a remote client window")]
struct CaptureArgs {
    #[arg(long)]
    app: Option<String>,
    #[arg(long)]
    title: Option<String>,
    #[arg(long)]
    pid: Option<i32>,
    #[arg(long, value_name = "PATH")]
    out: Option<PathBuf>,
    #[arg(long, default_value_t = 60)]
    seconds: u32,
    #[arg(long = "calib-timeout", default_value_t = 30)]
    calib_timeout: u64,
    #[arg(long = "wait-window", default_value_t = 10)]
    wait_window: u64,
    #[arg(long, default_value_t = 60)]
    fps: u32,
    #[arg(long)]
    save_frames: bool,
    #[arg(long, default_value_t = 5)]
    save_count: u32,
    #[arg(long = "save-interval", default_value_t = 5)]
    save_interval: u64,
    #[arg(long)]
    list_windows: bool,
    #[arg(long)]
    check: bool,
    #[arg(long)]
    request_permission: bool,
}

fn write_terminal_result(
    out: &Path,
    source_name: &str,
    opts: &CaptureOpts,
    window: Option<&WindowInfo>,
    reason: EndReason,
    error: Option<&str>,
    t_us: u64,
) -> Result<()> {
    let mut writer = open_writer(out)?;
    let start = StartRecord {
        record_type: "start",
        schema: 1,
        tool: "rs-bench capture",
        source: source_name,
        window,
        opts: start_opts(opts),
        t_us,
    };
    write_json_line(&mut writer, &start)?;
    let end = EndRecord {
        record_type: "end",
        t_us: system_now_us(),
        reason: reason.as_str(),
        frames: 0,
        decoded: 0,
        dropped: 0,
        error,
    };
    write_json_line(&mut writer, &end)?;
    writer.flush().context("flush capture end")
}

fn system_now_us() -> u64 {
    SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .map(|duration| duration.as_micros().min(u64::MAX as u128) as u64)
        .unwrap_or(0)
}

fn cli_capture(platform: &dyn CapturePlatform, options: CaptureArgs) -> Result<i32> {
    if options.app.is_none() && options.title.is_none() && options.pid.is_none() {
        eprintln!("error: one of --app, --title, or --pid is required");
        return Ok(2);
    }
    let Some(out) = options.out else {
        eprintln!("error: --out is required for capture");
        return Ok(2);
    };

    if !platform.check_permission(false) {
        eprintln!("no screen recording permission");
        return Ok(1);
    }

    let filter = WindowFilter {
        app: options.app,
        title: options.title,
        pid: options.pid,
    };
    let wait_start = Instant::now();
    let wait_window = Duration::from_secs(options.wait_window);
    let window = loop {
        let windows = match platform.list_windows() {
            Ok(windows) => windows,
            Err(error) => {
                eprintln!("cannot list windows: {error}");
                return Ok(1);
            }
        };
        if let Some(window) = select_window(&windows, &filter) {
            break Some(window);
        }
        if wait_start.elapsed() >= wait_window {
            break None;
        }
        let remaining = wait_window.saturating_sub(wait_start.elapsed());
        thread::sleep(remaining.min(Duration::from_millis(500)));
    };

    let capture_opts = CaptureOpts {
        seconds: options.seconds,
        calib_timeout: Duration::from_secs(options.calib_timeout),
        fps: options.fps,
        save_frames: options.save_frames,
        save_count: options.save_count,
        save_interval: Duration::from_secs(options.save_interval),
        ..CaptureOpts::default()
    };

    let Some(window) = window else {
        write_terminal_result(
            &out,
            platform.name(),
            &capture_opts,
            None,
            EndReason::NoWindow,
            None,
            system_now_us(),
        )?;
        return Ok(EndReason::NoWindow.exit_code());
    };

    let mut source = match platform.open(&window, capture_opts.fps) {
        Ok(source) => source,
        Err(error) => {
            write_terminal_result(
                &out,
                platform.name(),
                &capture_opts,
                Some(&window),
                EndReason::SourceClosed,
                Some(&error),
                system_now_us(),
            )?;
            return Ok(EndReason::SourceClosed.exit_code());
        }
    };
    let mut now = system_now_us;
    let summary = run_capture(
        source.as_mut(),
        &window,
        platform.name(),
        &capture_opts,
        &out,
        &mut now,
    )?;
    Ok(summary.reason.exit_code())
}

/// 供测试注入平台的命令行入口。
pub fn cli_with(platform: Option<&dyn CapturePlatform>, args: Vec<String>) -> Result<i32> {
    let Some(platform) = platform else {
        eprintln!("capture not supported on this platform");
        return Ok(2);
    };

    let options = match CaptureArgs::try_parse_from(args) {
        Ok(options) => options,
        Err(error) => {
            let code = error.exit_code();
            let _ = error.print();
            return Ok(code);
        }
    };

    if options.check {
        let permission = platform.check_permission(options.request_permission);
        let output = CheckRecord {
            platform: platform.name(),
            permission,
        };
        println!(
            "{}",
            serde_json::to_string(&output).context("encode permission JSON")?
        );
        return Ok(i32::from(!permission));
    }
    if options.list_windows {
        match platform.list_windows() {
            Ok(windows) => {
                println!(
                    "{}",
                    serde_json::to_string(&windows).context("encode windows JSON")?
                );
                return Ok(0);
            }
            Err(error) => {
                eprintln!("cannot list windows: {error}");
                return Ok(1);
            }
        }
    }

    cli_capture(platform, options)
}

pub fn cli(args: Vec<String>) -> anyhow::Result<i32> {
    let platform = platform();
    cli_with(platform.as_deref(), args)
}
