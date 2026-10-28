#![cfg(windows)]

use std::{
    sync::mpsc,
    time::{Duration, Instant},
};

use host_fake::clip::Clip;
use rs_desktop::{
    platform::{DecodeResult, DesktopFrame, FrameDone, FrameSink, StreamInfo, VideoCodec},
    win_present::WinVideoSink,
};
use windows::{
    core::w,
    Win32::{
        Foundation::{HWND, LPARAM, LRESULT, WPARAM},
        System::LibraryLoader::GetModuleHandleW,
        UI::WindowsAndMessaging::{
            CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, PeekMessageW,
            RegisterClassW, TranslateMessage, MSG, PM_REMOVE, WNDCLASSW, WS_OVERLAPPEDWINDOW,
            WS_VISIBLE,
        },
    },
};

unsafe extern "system" fn test_window_proc(
    hwnd: HWND,
    message: u32,
    wparam: WPARAM,
    lparam: LPARAM,
) -> LRESULT {
    // SAFETY: forwarding the test window's messages to DefWindowProcW is the
    // documented default procedure for an otherwise empty top-level window.
    unsafe { DefWindowProcW(hwnd, message, wparam, lparam) }
}

fn window() -> HWND {
    // SAFETY: class data and UTF-16 strings are static for the duration of the
    // process; the returned HWND is owned by this test until DestroyWindow.
    unsafe {
        let instance = GetModuleHandleW(None).expect("module handle");
        let class = WNDCLASSW {
            lpfnWndProc: Some(test_window_proc),
            hInstance: instance.into(),
            lpszClassName: w!("RsV6QVideoTest"),
            ..Default::default()
        };
        let _ = RegisterClassW(&class);
        CreateWindowExW(
            Default::default(),
            w!("RsV6QVideoTest"),
            w!("Remote Screen video test"),
            WS_OVERLAPPEDWINDOW | WS_VISIBLE,
            0,
            0,
            640,
            360,
            None,
            None,
            Some(instance.into()),
            None,
        )
        .expect("CreateWindowExW")
    }
}

fn pump_until(timeout: Duration, mut condition: impl FnMut() -> bool) -> bool {
    let deadline = Instant::now() + timeout;
    while Instant::now() < deadline {
        // SAFETY: the message pointer is valid and this test owns the window's
        // thread, so removing and dispatching messages is thread-safe.
        unsafe {
            let mut message = MSG::default();
            while PeekMessageW(&mut message, None, 0, 0, PM_REMOVE).as_bool() {
                let _ = TranslateMessage(&message);
                DispatchMessageW(&message);
            }
        }
        if condition() {
            return true;
        }
        std::hint::spin_loop();
    }
    condition()
}

fn frame(id: u32, data: &[u8], keyframe: bool) -> DesktopFrame {
    DesktopFrame {
        frame_id: id,
        epoch: 1,
        display_id: 1,
        codec: VideoCodec::Hevc,
        keyframe,
        data: data.to_vec(),
        vps: None,
        sps: None,
        pps: None,
        capture_ts_us: 1_000_000 + u64::from(id),
        capture_local_us: None,
        received_us: 2_000_000 + u64::from(id),
    }
}

fn configure(sink: &WinVideoSink) {
    sink.configure(&StreamInfo {
        codec: VideoCodec::Hevc,
        width: 640,
        height: 360,
        display_id: 1,
    });
}

fn submit(sink: &WinVideoSink, frame: DesktopFrame) -> mpsc::Receiver<DecodeResult> {
    let (sender, receiver) = mpsc::channel();
    sink.submit(
        frame,
        FrameDone::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    receiver
}

fn collect_done(
    receivers: &mut Vec<mpsc::Receiver<DecodeResult>>,
    results: &mut Vec<DecodeResult>,
) -> bool {
    let mut remaining = Vec::with_capacity(receivers.len());
    for receiver in receivers.drain(..) {
        match receiver.try_recv() {
            Ok(result) => results.push(result),
            Err(mpsc::TryRecvError::Empty) => remaining.push(receiver),
            Err(mpsc::TryRecvError::Disconnected) => {}
        }
    }
    *receivers = remaining;
    receivers.is_empty()
}

#[test]
fn sink_decodes_and_presents_clip() {
    let hwnd = window();
    let sink = WinVideoSink::new(hwnd, 640, 360);
    configure(&sink);
    let clip = Clip::bundled();
    let mut receivers = Vec::new();
    for id in 0..60u32 {
        let index = id as usize % clip.len();
        receivers.push(submit(
            &sink,
            frame(id, clip.au(index), clip.is_keyframe(index)),
        ));
    }
    let mut results = Vec::new();
    assert!(pump_until(Duration::from_secs(10), || {
        collect_done(&mut receivers, &mut results)
    }));
    assert_eq!(results.len(), 60);
    assert!(results
        .iter()
        .all(|result| matches!(result, DecodeResult::Shown { .. })));
    assert_eq!(sink.video_size(), Some((640, 360)));
    assert!(sink.stats().decoded >= 1);
    sink.shutdown();
    // SAFETY: this test owns the window and no render-thread join is required by
    // the FrameSink contract.
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[test]
fn resize_while_streaming() {
    let hwnd = window();
    let sink = WinVideoSink::new(hwnd, 640, 360);
    configure(&sink);
    let clip = Clip::bundled();
    let mut receivers = Vec::new();
    for id in 0..20u32 {
        sink.resize(800, 450);
        sink.resize(320, 180);
        sink.resize(0, 0);
        sink.resize(640, 360);
        let index = id as usize % clip.len();
        receivers.push(submit(
            &sink,
            frame(id, clip.au(index), clip.is_keyframe(index)),
        ));
    }
    let mut results = Vec::new();
    assert!(pump_until(Duration::from_secs(10), || {
        collect_done(&mut receivers, &mut results)
    }));
    assert_eq!(results.len(), 20);
    assert!(results
        .iter()
        .all(|result| matches!(result, DecodeResult::Shown { .. })));
    assert_eq!(sink.stats().device_resets, 0);
    sink.shutdown();
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[test]
fn close_keeps_window_and_fails_new_frames() {
    let hwnd = window();
    let sink = WinVideoSink::new(hwnd, 640, 360);
    configure(&sink);
    sink.close();
    let mut receivers = vec![submit(&sink, frame(99, &[0xff; 1000], false))];
    let mut results = Vec::new();
    assert!(pump_until(Duration::from_secs(2), || {
        collect_done(&mut receivers, &mut results)
    }));
    assert_eq!(results, vec![DecodeResult::Failed]);
    sink.resize(640, 360);
    sink.shutdown();
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[test]
fn garbage_then_keyframe() {
    let hwnd = window();
    let sink = WinVideoSink::new(hwnd, 640, 360);
    configure(&sink);
    let bad = submit(&sink, frame(0, &[0xff; 1000], false));
    let clip = Clip::bundled();
    let index = clip.keyframe_indices()[0];
    let good = submit(&sink, frame(1, clip.au(index), true));
    let mut receivers = vec![bad, good];
    let mut results = Vec::new();
    assert!(pump_until(Duration::from_secs(10), || {
        collect_done(&mut receivers, &mut results)
    }));
    assert_eq!(results.len(), 2);
    assert!(matches!(results[0], DecodeResult::Failed));
    assert!(matches!(results[1], DecodeResult::Shown { .. }));
    sink.shutdown();
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}

#[test]
fn create_destroy_repeat() {
    let clip = Clip::bundled();
    for cycle in 0..10u32 {
        let hwnd = window();
        let sink = WinVideoSink::new(hwnd, 640, 360);
        configure(&sink);
        let index = clip.keyframe_indices()[0];
        let _ = submit(&sink, frame(cycle, clip.au(index), true));
        sink.shutdown();
        unsafe {
            let _ = DestroyWindow(hwnd);
        }
    }
}

#[test]
fn submit_is_fast() {
    let hwnd = window();
    let sink = WinVideoSink::new(hwnd, 640, 360);
    configure(&sink);
    let clip = Clip::bundled();
    let index = clip.keyframe_indices()[0];
    let mut longest = Duration::ZERO;
    for id in 0..60u32 {
        let start = Instant::now();
        let _ = submit(&sink, frame(id, clip.au(index), true));
        longest = longest.max(start.elapsed());
    }
    assert!(
        longest < Duration::from_millis(1),
        "submit took {longest:?}"
    );
    sink.shutdown();
    unsafe {
        let _ = DestroyWindow(hwnd);
    }
}
