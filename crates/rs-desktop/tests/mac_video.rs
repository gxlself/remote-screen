#![cfg(target_os = "macos")]

use std::{
    ffi::c_void,
    sync::{mpsc, OnceLock},
    time::{Duration, Instant},
};

use rs_desktop::{
    frames::{has_parameter_sets, to_desktop_frame},
    mac_video::{can_decode, MacVideoSink},
    platform::{DecodeResult, DesktopFrame, FrameDone, FrameSink, StreamInfo, VideoCodec},
};
use rsp_client::ReceivedFrame;
use rsp_core::video::Frame;

#[repr(C)]
struct RsDtBuf {
    ptr: *mut u8,
    len: usize,
}

// SAFETY: these symbols are emitted by the Swift files compiled by build.rs.
unsafe extern "C" {
    fn rs_dt_test_encode(codec: i32, frame_count: u32, output: *mut c_void) -> i32;
    fn rs_dt_buf_free(pointer: *mut u8);
}

fn encoded(codec: VideoCodec) -> &'static [Vec<u8>] {
    static HEVC: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    static H264: OnceLock<Vec<Vec<u8>>> = OnceLock::new();
    let slot = match codec {
        VideoCodec::Hevc => &HEVC,
        VideoCodec::H264 => &H264,
    };
    slot.get_or_init(|| {
        let mut output = RsDtBuf {
            ptr: std::ptr::null_mut(),
            len: 0,
        };
        // SAFETY: the Swift test encoder writes one owned `RsDtBuf` and the
        // matching free function releases its allocation after copying.
        let status = unsafe {
            rs_dt_test_encode(
                if codec == VideoCodec::Hevc { 0 } else { 1 },
                10,
                (&mut output as *mut RsDtBuf).cast(),
            )
        };
        assert_eq!(status, 0, "VideoToolbox test encoder failed");
        let bytes = if output.ptr.is_null() || output.len == 0 {
            Vec::new()
        } else {
            // SAFETY: Swift returned a valid allocation for `len` bytes.
            unsafe { std::slice::from_raw_parts(output.ptr, output.len).to_vec() }
        };
        // SAFETY: the output allocation belongs to Swift and is released once.
        unsafe { rs_dt_buf_free(output.ptr) };
        let mut frames = Vec::new();
        let mut offset = 0;
        while offset < bytes.len() {
            assert!(bytes.len() - offset >= 4, "truncated encoded record");
            let length = u32::from_be_bytes(bytes[offset..offset + 4].try_into().unwrap()) as usize;
            offset += 4;
            assert!(
                length > 0 && length <= bytes.len() - offset,
                "invalid encoded length"
            );
            frames.push(bytes[offset..offset + length].to_vec());
            offset += length;
        }
        assert_eq!(frames.len(), 10);
        for (index, frame) in frames.iter().enumerate() {
            assert_eq!(
                has_parameter_sets(frame, codec),
                index % 5 == 0,
                "test encoder must emit a keyframe every five frames"
            );
        }
        frames
    })
}

fn desktop_frame(codec: VideoCodec, id: u32, keyframe: bool, data: Vec<u8>) -> DesktopFrame {
    let received = ReceivedFrame {
        frame: Frame {
            frame_id: id,
            epoch: 1,
            display_id: 1,
            flags: u8::from(keyframe),
            keyframe,
            capture_ts_us: 1_000_000 + u64::from(id),
            data,
        },
        received_us: 2_000_000 + u64::from(id),
        capture_local_us: None,
    };
    to_desktop_frame(
        &received,
        codec,
        rs_desktop::platform::BitstreamFormat::Avcc,
        1,
    )
}

fn configure(sink: &MacVideoSink, codec: VideoCodec) {
    sink.configure(&StreamInfo {
        codec,
        width: 320,
        height: 240,
        display_id: 1,
    });
}

fn submit_one(sink: &MacVideoSink, frame: DesktopFrame) -> mpsc::Receiver<DecodeResult> {
    let (sender, receiver) = mpsc::channel();
    sink.submit(
        frame,
        FrameDone::new(move |result| {
            let _ = sender.send(result);
        }),
    );
    receiver
}

fn receive_all(
    receivers: Vec<mpsc::Receiver<DecodeResult>>,
    deadline: Instant,
) -> Vec<DecodeResult> {
    receivers
        .into_iter()
        .map(|receiver| {
            let result = receiver
                .recv_timeout(deadline.saturating_duration_since(Instant::now()))
                .expect("one decode receipt before the shared deadline");
            assert_eq!(
                receiver.recv_timeout(deadline.saturating_duration_since(Instant::now())),
                Err(mpsc::RecvTimeoutError::Disconnected),
                "exactly one receipt per frame"
            );
            result
        })
        .collect()
}

fn assert_roundtrip(codec: VideoCodec) {
    let frames = encoded(codec);
    let sink = MacVideoSink::headless(320, 240);
    configure(&sink, codec);
    assert_eq!(sink.format(), rs_desktop::platform::BitstreamFormat::Avcc);
    let mut receivers = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    for (index, data) in frames.iter().enumerate() {
        receivers.push(submit_one(
            &sink,
            desktop_frame(codec, index as u32, index % 5 == 0, data.clone()),
        ));
    }
    for result in receive_all(receivers, deadline) {
        match result {
            DecodeResult::Shown { decode_us } => assert!(decode_us > 0),
            DecodeResult::Failed => panic!("encoded frame failed"),
        }
    }
    let state = sink.debug_state();
    assert_eq!((state.last_width, state.last_height), (320, 240));
    assert_eq!(state.decoded, 10);
    assert_eq!(state.failed, 0);
}

#[test]
fn hw_supported() {
    assert!(can_decode(VideoCodec::Hevc));
    assert!(can_decode(VideoCodec::H264));
}

#[test]
fn hevc_decode_roundtrip() {
    assert_roundtrip(VideoCodec::Hevc);
}

#[test]
fn h264_decode_roundtrip() {
    assert_roundtrip(VideoCodec::H264);
}

#[test]
fn corrupt_delta_fails() {
    let codec = VideoCodec::Hevc;
    let sink = MacVideoSink::headless(320, 240);
    configure(&sink, codec);
    let frames = encoded(codec);
    let mut receivers = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    for (index, data) in frames.iter().enumerate() {
        let mut frame = desktop_frame(codec, index as u32, index % 5 == 0, data.clone());
        if index == 2 {
            assert!(!frame.keyframe);
            assert!(!frame.data.is_empty());
            frame.data.fill(0xff);
        }
        receivers.push(submit_one(&sink, frame));
    }
    let results = receive_all(receivers, deadline);
    assert!(matches!(results[2], DecodeResult::Failed));
    assert!(matches!(results[5], DecodeResult::Shown { .. }));
}

#[test]
fn delta_without_session_fails() {
    let codec = VideoCodec::H264;
    let sink = MacVideoSink::headless(320, 240);
    configure(&sink, codec);
    let frame = desktop_frame(codec, 0, false, encoded(codec)[1].clone());
    assert!(matches!(
        submit_one(&sink, frame)
            .recv_timeout(Duration::from_secs(5))
            .unwrap(),
        DecodeResult::Failed
    ));
}

#[test]
fn close_fails_pending() {
    let codec = VideoCodec::Hevc;
    let sink = MacVideoSink::headless(320, 240);
    configure(&sink, codec);
    let frames = encoded(codec);
    let mut receivers = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    for (index, data) in frames.iter().enumerate() {
        receivers.push(submit_one(
            &sink,
            desktop_frame(codec, index as u32, index % 5 == 0, data.clone()),
        ));
    }
    sink.close();
    let mut shown = 0;
    let mut failed = 0;
    for result in receive_all(receivers, deadline) {
        match result {
            DecodeResult::Shown { .. } => shown += 1,
            DecodeResult::Failed => failed += 1,
        }
    }
    assert_eq!(shown + failed, 10);
    let after_close = submit_one(&sink, desktop_frame(codec, 99, false, vec![1, 2, 3]));
    assert_eq!(
        after_close.try_recv(),
        Ok(DecodeResult::Failed),
        "closed sinks must finish synchronously"
    );
}

#[test]
fn drop_sink_no_leak() {
    let codec = VideoCodec::H264;
    let frame = desktop_frame(codec, 0, true, encoded(codec)[0].clone());
    let mut receivers = Vec::new();
    let deadline = Instant::now() + Duration::from_secs(5);
    for index in 0..100 {
        let sink = MacVideoSink::headless(320, 240);
        configure(&sink, codec);
        receivers.push(submit_one(
            &sink,
            DesktopFrame {
                frame_id: index,
                ..frame.clone()
            },
        ));
        drop(sink); // Receipts must also survive dropping a pending pipeline.
    }
    assert_eq!(receive_all(receivers, deadline).len(), 100);
}

#[test]
fn submit_is_fast() {
    let codec = VideoCodec::H264;
    let sink = MacVideoSink::headless(320, 240);
    configure(&sink, codec);
    let frame = desktop_frame(codec, 0, true, encoded(codec)[0].clone());
    let warmup = submit_one(&sink, frame.clone());
    assert!(matches!(
        warmup.recv_timeout(Duration::from_secs(5)).unwrap(),
        DecodeResult::Shown { .. }
    ));
    let mut receivers = Vec::with_capacity(100);
    let mut max_elapsed = Duration::ZERO;
    let deadline = Instant::now() + Duration::from_secs(5);
    for index in 0..100 {
        let (sender, receiver) = mpsc::channel();
        let done = FrameDone::new(move |result| {
            let _ = sender.send(result);
        });
        let frame = DesktopFrame {
            frame_id: index + 1,
            capture_ts_us: frame.capture_ts_us + u64::from(index) + 1,
            ..frame.clone()
        };
        let started = Instant::now();
        sink.submit(frame, done);
        max_elapsed = max_elapsed.max(started.elapsed());
        receivers.push(receiver);
    }
    assert!(receive_all(receivers, deadline)
        .iter()
        .all(|result| matches!(result, DecodeResult::Shown { .. })));
    let limit = Duration::from_millis(if cfg!(debug_assertions) { 3 } else { 1 });
    assert!(
        max_elapsed < limit,
        "submit took {max_elapsed:?}, limit {limit:?}"
    );
}

#[test]
fn cursor_position() {
    let sink = MacVideoSink::headless(800, 600);
    sink.configure(&StreamInfo {
        codec: VideoCodec::H264,
        width: 1920,
        height: 1080,
        display_id: 1,
    });
    sink.debug_move_cursor(0.5, 0.5);
    let state = sink.debug_state();
    assert!((state.cursor_x - 400.0).abs() <= 0.5);
    assert!((state.cursor_y - 300.0).abs() <= 0.5);
    sink.debug_move_cursor(0.0, 0.0);
    let state = sink.debug_state();
    assert!(state.cursor_x.abs() <= 0.5);
    assert!((state.cursor_y - 75.0).abs() <= 0.5);
    sink.debug_move_cursor(1.0, 1.0);
    let state = sink.debug_state();
    assert!((state.cursor_x - 800.0).abs() <= 0.5);
    assert!((state.cursor_y - 525.0).abs() <= 0.5);
}

#[test]
fn reconfigure_resets_session_and_switches_codec() {
    let sink = MacVideoSink::headless(320, 240);
    for codec in [VideoCodec::H264, VideoCodec::Hevc, VideoCodec::H264] {
        let frames = encoded(codec);
        configure(&sink, codec);
        let deadline = Instant::now() + Duration::from_secs(5);
        let delta = submit_one(&sink, desktop_frame(codec, 1, false, frames[1].clone()));
        assert_eq!(
            receive_all(vec![delta], deadline),
            vec![DecodeResult::Failed]
        );
        let key = submit_one(&sink, desktop_frame(codec, 2, true, frames[0].clone()));
        assert!(matches!(
            receive_all(vec![key], deadline)[0],
            DecodeResult::Shown { .. }
        ));
    }
    let state = sink.debug_state();
    assert_eq!((state.decoded, state.failed), (3, 3));
}

#[test]
fn callback_panic_does_not_cross_ffi() {
    let codec = VideoCodec::H264;
    let frame = desktop_frame(codec, 0, true, encoded(codec)[0].clone());
    let sink = MacVideoSink::headless(320, 240);
    configure(&sink, codec);
    let (sender, receiver) = mpsc::channel();
    sink.submit(
        frame.clone(),
        FrameDone::new(move |_| {
            let _ = sender.send(());
            panic!("intentional frame completion panic");
        }),
    );
    receiver
        .recv_timeout(Duration::from_secs(5))
        .expect("callback invoked");
    let next = submit_one(
        &sink,
        DesktopFrame {
            frame_id: 1,
            capture_ts_us: frame.capture_ts_us + 1,
            ..frame
        },
    );
    let results = receive_all(vec![next], Instant::now() + Duration::from_secs(5));
    assert!(matches!(results[0], DecodeResult::Shown { .. }));
}
