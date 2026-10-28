use std::cell::RefCell;
use std::collections::VecDeque;
use std::fs;
use std::io::Write;
use std::path::{Path, PathBuf};
use std::process::{Command, Output};
use std::sync::atomic::{AtomicU64, Ordering};
use std::sync::Arc;
use std::time::Duration;

use rs_bench::capture::{
    cli_with, run_capture, select_window, CaptureOpts, CapturePlatform, CaptureSummary, EndReason,
    FrameSource, TimedFrame, WindowFilter, WindowInfo,
};
use rs_bench::decode::{decode_at, Rect};
use rs_bench::synth::{viewport, Background};
use rs_bench::timecode::{render_calib, Code, GrayImage, PxRect};
use serde_json::{json, Value};

const WIN_W: u32 = 800;
const WIN_H: u32 = 500;
const VIEW_W: u32 = 640;
const VIEW_H: u32 = 360;
const BASE_US: u64 = 1_792_507_200_000_000;
const STEP_US: u64 = 16_667;

fn window(id: u64, app: &str, title: &str, width: u32, height: u32, on_screen: bool) -> WindowInfo {
    WindowInfo {
        id,
        pid: id as i32,
        app: app.to_owned(),
        title: title.to_owned(),
        width,
        height,
        on_screen,
    }
}

fn client_window() -> WindowInfo {
    window(9, "com.example.client", "Remote Client", WIN_W, WIN_H, true)
}

fn blit(view: &GrayImage, win: &mut GrayImage, x: u32, y: u32) {
    for row in 0..view.height {
        let src = (row * view.width) as usize;
        let dst = ((y + row) * win.width + x) as usize;
        win.data[dst..dst + view.width as usize]
            .copy_from_slice(&view.data[src..src + view.width as usize]);
    }
}

fn placed(view: &GrayImage) -> GrayImage {
    let mut image = GrayImage::new(WIN_W, WIN_H, 20);
    image.fill_rect(
        PxRect {
            x0: 0,
            y0: 0,
            x1: WIN_W,
            y1: 40,
        },
        200,
    );
    blit(view, &mut image, 80, 70);
    image
}

fn time_view(code: u64) -> GrayImage {
    viewport(
        VIEW_W,
        VIEW_H,
        Background::Solid(40),
        Some(Code::Time(code)),
    )
}

fn fail_view() -> GrayImage {
    GrayImage::new(VIEW_W, VIEW_H, 128)
}

fn end_view() -> GrayImage {
    viewport(VIEW_W, VIEW_H, Background::Solid(0), Some(Code::End))
}

fn calibration() -> Vec<GrayImage> {
    [true, false, true, false]
        .into_iter()
        .map(|white| render_calib(VIEW_W, VIEW_H, white))
        .collect()
}

fn timed(view: &GrayImage, index: usize) -> TimedFrame {
    let image = placed(view);
    TimedFrame {
        t_us: BASE_US + index as u64 * STEP_US,
        width: image.width,
        height: image.height,
        luma: image.data,
    }
}

struct FakeSource {
    frames: VecDeque<Result<TimedFrame, String>>,
    clock: Arc<AtomicU64>,
    dropped: u64,
    idle_step: Duration,
    expected_poll: Duration,
}

impl FakeSource {
    fn new(views: impl IntoIterator<Item = GrayImage>) -> Self {
        Self {
            frames: views
                .into_iter()
                .enumerate()
                .map(|(index, view)| Ok(timed(&view, index)))
                .collect(),
            clock: Arc::new(AtomicU64::new(BASE_US)),
            dropped: 0,
            idle_step: Duration::from_secs(30),
            expected_poll: CaptureOpts::default().poll,
        }
    }
}

impl FrameSource for FakeSource {
    fn next_frame(&mut self, timeout: Duration) -> Result<Option<TimedFrame>, String> {
        assert_eq!(timeout, self.expected_poll);
        match self.frames.pop_front() {
            Some(Ok(frame)) => {
                self.clock.store(frame.t_us, Ordering::SeqCst);
                Ok(Some(frame))
            }
            Some(Err(error)) => Err(error),
            None => {
                self.clock
                    .fetch_add(self.idle_step.as_micros() as u64, Ordering::SeqCst);
                Ok(None)
            }
        }
    }

    fn dropped(&self) -> u64 {
        self.dropped
    }
}

struct TempOutput {
    dir: PathBuf,
    out: PathBuf,
}

impl TempOutput {
    fn new(name: &str) -> Self {
        let dir =
            std::env::temp_dir().join(format!("rs-bench-capture-{}-{name}", std::process::id()));
        let _ = fs::remove_dir_all(&dir);
        let out = dir.join("nested/frames.jsonl");
        Self { dir, out }
    }
}

impl Drop for TempOutput {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.dir);
    }
}

fn read_jsonl(out: &Path) -> Vec<Value> {
    let text = fs::read_to_string(out).expect("read frames.jsonl");
    assert!(text.ends_with('\n'), "last JSON record needs a newline");
    text.lines()
        .map(|line| serde_json::from_str(line).expect("JSON record"))
        .collect()
}

fn records<'a>(lines: &'a [Value], kind: &str) -> Vec<&'a Value> {
    lines.iter().filter(|line| line["type"] == kind).collect()
}

fn capture(
    name: &str,
    mut source: FakeSource,
    opts: &CaptureOpts,
) -> (TempOutput, CaptureSummary, Vec<Value>) {
    let output = TempOutput::new(name);
    let clock = Arc::clone(&source.clock);
    let mut now = || clock.load(Ordering::SeqCst);
    let summary = run_capture(
        &mut source,
        &client_window(),
        "fake",
        opts,
        &output.out,
        &mut now,
    )
    .expect("capture");
    let lines = read_jsonl(&output.out);
    (output, summary, lines)
}

fn full_source() -> FakeSource {
    let views = calibration()
        .into_iter()
        .chain([fail_view()])
        .chain((0..300).map(|index| {
            if index == 150 {
                fail_view()
            } else {
                time_view(1000 + index / 2)
            }
        }))
        .chain([end_view()]);
    let mut source = FakeSource::new(views);
    source.dropped = 7;
    source
}

fn full_opts() -> CaptureOpts {
    CaptureOpts {
        seconds: 5,
        save_frames: true,
        save_count: 2,
        save_interval: Duration::from_secs(1),
        ..CaptureOpts::default()
    }
}

#[test]
fn select_window_rules() {
    let windows = vec![
        window(5, "com.example.client", "Remote title", 800, 500, true),
        window(2, "COM.EXAMPLE.CLIENT", "Other", 900, 500, true),
        window(1, "com.example.client", "Remote title", 900, 500, true),
        window(0, "com.example.client", "Remote title", 1200, 800, false),
        window(7, "com.example.client", "Remote title", 63, 500, true),
        window(8, "com.example.client", "Remote title", 1200, 63, true),
    ];
    let filter = WindowFilter {
        app: Some("Com.Example.Client".to_owned()),
        title: Some("Remote".to_owned()),
        pid: None,
    };
    assert_eq!(select_window(&windows, &filter), Some(windows[2].clone()));
    assert_eq!(
        select_window(
            &windows,
            &WindowFilter {
                pid: Some(5),
                ..filter.clone()
            }
        ),
        Some(windows[0].clone())
    );
    // 面积相同时不依赖枚举顺序。
    let reversed = windows.iter().rev().cloned().collect::<Vec<_>>();
    assert_eq!(
        select_window(&reversed, &WindowFilter::default()).map(|w| w.id),
        Some(1)
    );
    for filter in [
        WindowFilter {
            app: Some("other.client".to_owned()),
            ..filter.clone()
        },
        WindowFilter {
            title: Some("remote".to_owned()),
            ..filter.clone()
        },
        WindowFilter {
            pid: Some(2),
            ..filter.clone()
        },
    ] {
        assert!(select_window(&windows, &filter).is_none());
    }
    let minimum = window(3, "client", "title", 64, 64, true);
    assert_eq!(
        select_window(std::slice::from_ref(&minimum), &WindowFilter::default()),
        Some(minimum)
    );
    assert!(select_window(&[], &WindowFilter::default()).is_none());
    let huge = vec![
        window(11, "client", "", u32::MAX, u32::MAX, true),
        window(10, "client", "", 1000, 1000, true),
    ];
    assert_eq!(
        select_window(&huge, &WindowFilter::default()).map(|w| w.id),
        Some(11)
    );
}

#[test]
fn full_sequence() {
    let (_output, summary, lines) = capture("full", full_source(), &full_opts());
    assert_eq!(
        summary,
        CaptureSummary {
            reason: EndReason::EndCode,
            frames: 300,
            decoded: 299,
            dropped: 7,
            saved: 2,
        }
    );
    assert_eq!(lines.len(), 305);
    assert_eq!(
        lines[0],
        json!({
            "type": "start", "schema": 1, "tool": "rs-bench capture", "source": "fake",
            "window": client_window(), "opts": { "seconds": 5, "calib_timeout_s": 30, "fps": 60 },
            "t_us": BASE_US,
        })
    );
    let calib = &lines[1];
    assert_eq!(calib["type"], "calib");
    assert_eq!(calib["host_w"], VIEW_W);
    assert_eq!(calib["host_h"], VIEW_H);
    assert_eq!(calib["frame_w"], WIN_W);
    assert_eq!(calib["frame_h"], WIN_H);
    for (actual, expected) in calib["rect"]
        .as_array()
        .expect("rect")
        .iter()
        .zip([80.0, 70.0, 640.0, 360.0])
    {
        assert!((actual.as_f64().expect("coordinate") - expected).abs() <= 3.0);
    }
    let frame_lines = records(&lines, "frame");
    assert_eq!(frame_lines.len(), 300);
    assert_eq!(frame_lines[0]["code"], 1000);
    for (index, frame) in frame_lines.iter().enumerate() {
        assert_eq!(frame["t_us"], BASE_US + (index as u64 + 5) * STEP_US);
        if index == 150 {
            assert_eq!(frame["code"], Value::Null);
            assert_eq!(frame["err"], "low-contrast");
        } else {
            assert_eq!(frame["code"], 1000 + index as u64 / 2);
            assert!(frame.get("err").is_none());
        }
    }
    assert_eq!(records(&lines, "saved").len(), 2);
    for (index, line) in lines.iter().enumerate().skip(2).take(lines.len() - 3) {
        assert!(line["type"] == "frame" || line["type"] == "saved");
        if line["type"] == "saved" {
            assert_eq!(lines[index - 1]["type"], "frame");
            assert_eq!(lines[index - 1]["t_us"], line["t_us"]);
            assert_eq!(lines[index - 1]["code"], line["code"]);
        }
    }
    assert_eq!(
        lines.last().expect("end"),
        &json!({
            "type": "end", "t_us": BASE_US + 305 * STEP_US, "reason": "end-code",
            "frames": 300, "decoded": 299, "dropped": 7,
        })
    );
}

#[test]
fn saved_frames_decode() {
    let (output, summary, lines) = capture("saved", full_source(), &full_opts());
    assert_eq!(summary.saved, 2);
    for (index, saved) in records(&lines, "saved").iter().enumerate() {
        assert_eq!(saved["path"], format!("frames/frame-{}.pgm", index + 1));
        let path = output
            .out
            .parent()
            .expect("parent")
            .join(saved["path"].as_str().expect("path"));
        let image = GrayImage::from_pgm(&fs::read(path).expect("saved PGM")).expect("PGM image");
        assert!(image.width.abs_diff(VIEW_W) <= 4);
        assert!(image.height.abs_diff(VIEW_H) <= 4);
        assert_eq!(
            decode_at(
                &image,
                Rect {
                    x: 0.0,
                    y: 0.0,
                    w: image.width as f64,
                    h: image.height as f64,
                }
            ),
            Ok(Code::Time(saved["code"].as_u64().expect("TIME payload")))
        );
        assert_eq!(saved["t_us"], BASE_US + (65 + index as u64 * 60) * STEP_US);
    }
}

#[test]
fn calib_timeout() {
    let source = FakeSource::new((0..10).map(|_| render_calib(VIEW_W, VIEW_H, true)));
    let (_output, summary, lines) = capture("calib-timeout", source, &CaptureOpts::default());
    assert_eq!(summary.reason, EndReason::CalibTimeout);
    assert_eq!(summary.reason.exit_code(), 3);
    assert_eq!(summary.frames, 0);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["type"], "start");
    assert_eq!(lines[1]["reason"], "calib-timeout");
}

#[test]
fn source_closed() {
    let mut source = FakeSource::new(calibration().into_iter().chain((0..10).map(time_view)));
    source.frames.push_back(Err("window closed".to_owned()));
    let (_output, summary, lines) = capture("source-closed", source, &CaptureOpts::default());
    assert_eq!(summary.reason, EndReason::SourceClosed);
    assert_eq!(summary.reason.exit_code(), 4);
    assert_eq!(summary.frames, 10);
    assert_eq!(summary.decoded, 10);
    assert_eq!(lines.last().expect("end")["error"], "window closed");
    assert_eq!(lines.last().expect("end")["frames"], 10);
}

#[test]
fn timeout_without_end() {
    let source = FakeSource::new(calibration().into_iter().chain((0..60).map(time_view)));
    let opts = CaptureOpts {
        seconds: 2,
        ..CaptureOpts::default()
    };
    let (_output, summary, lines) = capture("timeout", source, &opts);
    assert_eq!(summary.reason, EndReason::Timeout);
    assert_eq!(summary.reason.exit_code(), 0);
    assert_eq!(summary.frames, 60);
    assert_eq!(lines.last().expect("end")["reason"], "timeout");
}

#[test]
fn locked_without_time_times_out() {
    let opts = CaptureOpts {
        seconds: 2,
        ..CaptureOpts::default()
    };
    let (_output, summary, lines) =
        capture("locked-no-time", FakeSource::new(calibration()), &opts);
    assert_eq!(summary.reason, EndReason::Timeout);
    assert_eq!(summary.frames, 0);
    assert_eq!(
        lines
            .iter()
            .map(|line| line["type"].as_str().expect("type"))
            .collect::<Vec<_>>(),
        ["start", "calib", "end"]
    );
}

#[test]
fn end_before_time() {
    let source = FakeSource::new(calibration().into_iter().chain([end_view()]));
    let (_output, summary, lines) = capture("early-end", source, &CaptureOpts::default());
    assert_eq!(summary.reason, EndReason::EndCode);
    assert_eq!(summary.frames, 0);
    assert_eq!(summary.decoded, 0);
    assert_eq!(lines.last().expect("end")["reason"], "end-code");
    assert_eq!(lines.len(), 3);
}

#[test]
fn malformed_frame_is_source_closed() {
    let mut source = FakeSource::new(calibration().into_iter().chain([time_view(1000)]));
    let mut frame = timed(&time_view(1001), 5);
    frame.luma.pop();
    source.frames.push_back(Ok(frame));
    let (_output, summary, lines) = capture("malformed", source, &CaptureOpts::default());
    assert_eq!(summary.reason, EndReason::SourceClosed);
    assert_eq!(summary.frames, 1);
    assert!(lines.last().expect("end")["error"]
        .as_str()
        .expect("error")
        .contains("raw image length mismatch"));
}

#[test]
fn calib_after_time_is_ignored() {
    let source = FakeSource::new(calibration().into_iter().chain([
        fail_view(),
        time_view(1000),
        render_calib(VIEW_W, VIEW_H, true),
        time_view(1001),
        end_view(),
    ]));
    let (_output, summary, lines) = capture("later-calib", source, &CaptureOpts::default());
    assert_eq!(summary.reason, EndReason::EndCode);
    assert_eq!(summary.frames, 2);
    assert_eq!(records(&lines, "calib").len(), 1);
    assert_eq!(
        records(&lines, "frame")
            .iter()
            .map(|line| line["code"].as_u64().expect("code"))
            .collect::<Vec<_>>(),
        [1000, 1001]
    );
}

#[test]
fn timeouts_use_local_clock() {
    let mut source = FakeSource::new(calibration().into_iter().chain([
        time_view(1000),
        time_view(1001),
        end_view(),
    ]));
    let output = TempOutput::new("local-clock");
    let clock = Arc::clone(&source.clock);
    let mut now = || clock.load(Ordering::SeqCst) + 1_000_000_000;
    let summary = run_capture(
        &mut source,
        &client_window(),
        "fake",
        &CaptureOpts::default(),
        &output.out,
        &mut now,
    )
    .expect("capture");
    assert_eq!(summary.reason, EndReason::EndCode);
    assert_eq!(summary.frames, 2);
    let lines = read_jsonl(&output.out);
    assert_eq!(lines[0]["t_us"], BASE_US + 1_000_000_000);
    assert_eq!(records(&lines, "frame")[0]["t_us"], BASE_US + 4 * STEP_US);
    assert_eq!(
        lines.last().expect("end")["t_us"],
        BASE_US + 6 * STEP_US + 1_000_000_000
    );
}

#[test]
fn first_time_restarts_timeout() {
    let mut source = FakeSource::new(calibration().into_iter().chain([
        time_view(1000),
        time_view(1001),
        end_view(),
    ]));
    for (frame, offset) in source.frames.iter_mut().skip(4).zip([11, 13, 14]) {
        frame.as_mut().expect("frame").t_us = BASE_US + offset * 1_000_000;
    }
    let opts = CaptureOpts {
        seconds: 2,
        ..CaptureOpts::default()
    };
    let (_output, summary, _) = capture("first-time-deadline", source, &opts);
    assert_eq!(summary.reason, EndReason::EndCode);
    assert_eq!(summary.frames, 2);
}

#[test]
fn flushes_during_idle() {
    struct ObservingSource {
        inner: FakeSource,
        out: PathBuf,
        idle_polls: u32,
    }

    impl FrameSource for ObservingSource {
        fn next_frame(&mut self, timeout: Duration) -> Result<Option<TimedFrame>, String> {
            if self.inner.frames.is_empty() {
                self.idle_polls += 1;
                if self.idle_polls == 2 {
                    let lines = read_jsonl(&self.out);
                    assert_eq!(records(&lines, "frame").len(), 1);
                    assert!(records(&lines, "end").is_empty());
                    return Err("finished observing".to_owned());
                }
            }
            self.inner.next_frame(timeout)
        }

        fn dropped(&self) -> u64 {
            self.inner.dropped()
        }
    }

    let output = TempOutput::new("flush-idle");
    let mut inner = FakeSource::new(calibration().into_iter().chain([time_view(1000)]));
    inner.idle_step = Duration::from_secs(1);
    inner.expected_poll = Duration::from_millis(25);
    let clock = Arc::clone(&inner.clock);
    let mut source = ObservingSource {
        inner,
        out: output.out.clone(),
        idle_polls: 0,
    };
    let opts = CaptureOpts {
        poll: Duration::from_millis(25),
        ..CaptureOpts::default()
    };
    let mut now = || clock.load(Ordering::SeqCst);
    let summary = run_capture(
        &mut source,
        &client_window(),
        "fake",
        &opts,
        &output.out,
        &mut now,
    )
    .expect("capture");
    assert_eq!(summary.reason, EndReason::SourceClosed);
    assert_eq!(source.idle_polls, 2);
    assert_eq!(read_jsonl(&output.out).last().expect("end")["type"], "end");
}

struct FakePlatform {
    windows: Result<Vec<WindowInfo>, String>,
    permission: bool,
    grant_request: bool,
    permission_requests: RefCell<Vec<bool>>,
    open_calls: RefCell<Vec<(u64, u32)>>,
    source: RefCell<Result<FakeSource, String>>,
}

impl FakePlatform {
    fn new() -> Self {
        Self {
            windows: Ok(vec![
                client_window(),
                window(10, "com.example.other", "Other", 640, 480, true),
            ]),
            permission: true,
            grant_request: false,
            permission_requests: RefCell::default(),
            open_calls: RefCell::default(),
            source: RefCell::new(Ok(FakeSource::new(
                calibration()
                    .into_iter()
                    .chain([time_view(1000), end_view()]),
            ))),
        }
    }
}

impl CapturePlatform for FakePlatform {
    fn name(&self) -> &'static str {
        "fake"
    }

    fn check_permission(&self, request: bool) -> bool {
        self.permission_requests.borrow_mut().push(request);
        self.permission || (request && self.grant_request)
    }

    fn list_windows(&self) -> Result<Vec<WindowInfo>, String> {
        self.windows.clone()
    }

    fn open(&self, window: &WindowInfo, fps: u32) -> Result<Box<dyn FrameSource>, String> {
        self.open_calls.borrow_mut().push((window.id, fps));
        let result = std::mem::replace(
            &mut *self.source.borrow_mut(),
            Err("source already opened".to_owned()),
        );
        result.map(|source| Box::new(source) as Box<dyn FrameSource>)
    }
}

fn cli_args(args: &[&str]) -> Vec<String> {
    std::iter::once("rs-bench capture")
        .chain(args.iter().copied())
        .map(str::to_owned)
        .collect()
}

fn invoke_capture(fake: &FakePlatform, out: &Path, extra: &[&str]) -> i32 {
    let mut args = cli_args(&["--app", "COM.EXAMPLE.CLIENT", "--wait-window", "0", "--out"]);
    args.push(out.to_str().expect("output path").to_owned());
    args.extend(extra.iter().map(|arg| (*arg).to_owned()));
    cli_with(Some(fake), args).expect("capture CLI")
}

// 自己的测试进程承载假平台，用 --nocapture 让父进程检查真实 stdout/stderr。
// 不重定向当前进程的全局 fd，避免与并行测试互相干扰。
const CLI_CHILD: &str = "RS_BENCH_CAPTURE_CLI_CHILD";

fn cli_stdout_child(mode: &str) -> Output {
    Command::new(std::env::current_exe().expect("test binary"))
        .args(["--exact", "cli_with_fake_platform", "--nocapture"])
        .env(CLI_CHILD, mode)
        .output()
        .expect("run fake-platform CLI child")
}

fn stdout_json(output: &Output) -> Value {
    let values = String::from_utf8(output.stdout.clone())
        .expect("UTF-8 stdout")
        .lines()
        .filter_map(|line| serde_json::from_str::<Value>(line).ok())
        .collect::<Vec<_>>();
    assert_eq!(
        values.len(),
        1,
        "stdout: {}",
        String::from_utf8_lossy(&output.stdout)
    );
    values[0].clone()
}

#[test]
fn cli_with_fake_platform() {
    if let Ok(mode) = std::env::var(CLI_CHILD) {
        let mut fake = FakePlatform::new();
        fake.permission = false;
        let args = match mode.as_str() {
            "check" => cli_args(&["--check"]),
            "request" => {
                fake.grant_request = true;
                cli_args(&["--check", "--request-permission"])
            }
            "list" => cli_args(&["--list-windows"]),
            "list-error" => {
                fake.windows = Err("enumeration failed".to_owned());
                cli_args(&["--list-windows"])
            }
            _ => panic!("unknown child mode"),
        };
        let code = cli_with(Some(&fake), args).expect("CLI child");
        if mode == "check" {
            assert_eq!(&*fake.permission_requests.borrow(), &[false]);
        }
        if mode == "request" {
            assert_eq!(&*fake.permission_requests.borrow(), &[true]);
        }
        std::io::stdout().flush().expect("flush child stdout");
        std::process::exit(code);
    }

    let output = TempOutput::new("cli-success");
    let fake = FakePlatform::new();
    assert_eq!(invoke_capture(&fake, &output.out, &["--seconds", "5"]), 0);
    let lines = read_jsonl(&output.out);
    assert_eq!(lines[0]["window"]["app"], "com.example.client");
    assert_eq!(lines[0]["source"], fake.name());
    assert_eq!(
        lines[0]["opts"],
        json!({"seconds": 5, "calib_timeout_s": 30, "fps": 60})
    );
    assert_eq!(&*fake.open_calls.borrow(), &[(9, 60)]);
    assert_eq!(&*fake.permission_requests.borrow(), &[false]);

    let checked = cli_stdout_child("check");
    assert_eq!(checked.status.code(), Some(1));
    assert_eq!(
        stdout_json(&checked),
        json!({"platform": "fake", "permission": false})
    );
    let requested = cli_stdout_child("request");
    assert_eq!(requested.status.code(), Some(0));
    assert_eq!(
        stdout_json(&requested),
        json!({"platform": "fake", "permission": true})
    );
    let listed = cli_stdout_child("list");
    assert_eq!(listed.status.code(), Some(0));
    assert_eq!(stdout_json(&listed).as_array().expect("windows").len(), 2);
    let failed_list = cli_stdout_child("list-error");
    assert_eq!(failed_list.status.code(), Some(1));
    assert!(String::from_utf8_lossy(&failed_list.stderr).contains("enumeration failed"));

    assert_eq!(
        cli_with(Some(&fake), cli_args(&[])).expect("missing filter"),
        2
    );
    assert_eq!(
        cli_with(Some(&fake), cli_args(&["--app", "client"])).expect("missing output"),
        2
    );
    assert_eq!(
        cli_with(Some(&fake), cli_args(&["--unknown"])).expect("unknown option"),
        2
    );
    assert_eq!(
        cli_with(Some(&fake), cli_args(&["--seconds", "no"])).expect("invalid seconds"),
        2
    );
    assert_eq!(
        cli_with(Some(&fake), cli_args(&["--help"])).expect("help"),
        0
    );

    let no_window = TempOutput::new("cli-no-window");
    let args = cli_args(&[
        "--app",
        "nobody",
        "--wait-window",
        "0",
        "--out",
        no_window.out.to_str().expect("path"),
    ]);
    assert_eq!(cli_with(Some(&fake), args).expect("no window"), 5);
    let lines = read_jsonl(&no_window.out);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["type"], "start");
    assert_eq!(lines[0]["window"], Value::Null);
    assert_eq!(lines[1]["reason"], "no-window");
    assert_eq!(lines[1]["frames"], 0);
    assert_eq!(lines[1]["decoded"], 0);
    assert_eq!(lines[1]["dropped"], 0);

    let open_error = TempOutput::new("cli-open-error");
    let fake = FakePlatform::new();
    *fake.source.borrow_mut() = Err("cannot open window".to_owned());
    assert_eq!(invoke_capture(&fake, &open_error.out, &[]), 4);
    let lines = read_jsonl(&open_error.out);
    assert_eq!(lines.len(), 2);
    assert_eq!(lines[0]["window"]["id"], 9);
    assert_eq!(lines[1]["reason"], "source-closed");
    assert_eq!(lines[1]["error"], "cannot open window");

    let denied = TempOutput::new("cli-no-permission");
    let mut fake = FakePlatform::new();
    fake.permission = false;
    assert_eq!(invoke_capture(&fake, &denied.out, &[]), 1);
    assert!(!denied.out.exists());
    assert!(fake.open_calls.borrow().is_empty());
}

#[test]
fn cli_unsupported_platform() {
    assert_eq!(
        cli_with(None, cli_args(&["--list-windows"])).expect("unsupported platform"),
        2
    );
}

#[cfg(not(target_os = "macos"))]
#[test]
fn cli_bin_smoke() {
    let output = Command::new(env!("CARGO_BIN_EXE_rs-bench"))
        .args(["capture", "--list-windows"])
        .output()
        .expect("run rs-bench");
    assert_eq!(output.status.code(), Some(2));
    assert!(
        String::from_utf8_lossy(&output.stderr).contains("capture not supported on this platform")
    );
    assert!(output.stdout.is_empty());
}
