use std::any::Any;
use std::sync::Arc;

use host_core::platform::CapturedFrame;
use host_linux::frame::{copy_to_bgrx, FramePool, LinuxFrame, SrcFormat};

#[test]
fn copy_bgrx_identity_with_padding() {
    let src = [
        1, 2, 3, 4, 5, 6, 7, 8, 9, 10, 11, 12, // row 0
        13, 14, 15, 16, 17, 18, 19, 20, 21, 22, 23, 24, // row 1
    ];
    let mut dst = [0xaa; 16];
    assert!(copy_to_bgrx(&src, 12, SrcFormat::Bgrx, 2, 2, &mut dst, 8,));
    assert_eq!(&dst[..8], &src[..8]);
    assert_eq!(&dst[8..16], &src[12..20]);
}

#[test]
fn copy_rgbx_swaps() {
    for format in [SrcFormat::Rgbx, SrcFormat::Rgba] {
        let mut dst = [0; 4];
        assert!(copy_to_bgrx(&[1, 2, 3, 4], 4, format, 1, 1, &mut dst, 4));
        assert_eq!(dst, [3, 2, 1, 4]);
    }
    let mut dst = [0; 4];
    assert!(copy_to_bgrx(
        &[1, 2, 3, 4],
        4,
        SrcFormat::Bgra,
        1,
        1,
        &mut dst,
        4,
    ));
    assert_eq!(dst, [1, 2, 3, 4]);
}

#[test]
fn copy_rejects_short_buffers() {
    let mut dst = [0xaa; 8];
    let original = dst;
    assert!(!copy_to_bgrx(
        &[1, 2, 3, 4, 5, 6, 7],
        8,
        SrcFormat::Bgrx,
        2,
        1,
        &mut dst,
        8,
    ));
    assert_eq!(dst, original);
    assert!(!copy_to_bgrx(
        &[1; 8],
        8,
        SrcFormat::Bgrx,
        2,
        1,
        &mut dst[..7],
        8,
    ));
    assert_eq!(dst, original);
    assert!(!copy_to_bgrx(
        &[1; 8],
        7,
        SrcFormat::Bgrx,
        2,
        1,
        &mut dst,
        8,
    ));
    assert_eq!(dst, original);
    assert!(copy_to_bgrx(&[], 0, SrcFormat::Bgrx, 2, 0, &mut [], 0,));
}

#[test]
fn pool_reuses_after_release() {
    let mut pool = FramePool::new(2);
    let first = pool.acquire(4, 2, 16, 1, |_| {}).expect("first frame");
    assert_eq!(pool.len(), 1);
    drop(first);
    let reused = pool.acquire(4, 2, 16, 2, |_| {}).expect("reused frame");
    assert_eq!(pool.len(), 1);
    let second = pool.acquire(4, 2, 16, 3, |_| {}).expect("second frame");
    assert_eq!(pool.len(), 2);
    drop((reused, second));
}

#[test]
fn pool_limit() {
    let mut pool = FramePool::new(2);
    let first = pool.acquire(1, 1, 4, 1, |_| {}).expect("first frame");
    let second = pool.acquire(1, 1, 4, 1, |_| {}).expect("second frame");
    assert!(pool.acquire(1, 1, 4, 1, |_| {}).is_none());
    drop((first, second));
}

#[test]
fn pool_resizes_and_sets_fields() {
    let mut pool = FramePool::new(1);
    let first = pool
        .acquire(4, 2, 16, 1, |pixels| assert_eq!(pixels.len(), 32))
        .expect("first frame");
    drop(first);
    let resized = pool
        .acquire(8, 4, 32, 7, |pixels| assert_eq!(pixels.len(), 128))
        .expect("resized frame");
    assert_eq!(resized.width, 8);
    assert_eq!(resized.height, 4);
    assert_eq!(resized.stride, 32);
    assert_eq!(resized.generation, 7);
    assert_eq!(resized.pixels.len(), 128);
}

#[test]
fn row_access() {
    let frame = LinuxFrame {
        width: 2,
        height: 2,
        stride: 12,
        generation: 1,
        pixels: vec![0; 24],
    };
    assert_eq!(frame.row(0).expect("row 0").len(), 8);
    assert!(frame.row(2).is_none());
}

#[test]
fn downcast() {
    let frame = LinuxFrame {
        width: 1,
        height: 1,
        stride: 4,
        generation: 0,
        pixels: vec![0; 4],
    };
    let captured = CapturedFrame {
        display_id: 1,
        width: 1,
        height: 1,
        capture_ts_us: 0,
        native: Arc::new(frame) as Arc<dyn Any + Send + Sync>,
    };
    assert!(LinuxFrame::of(&captured).is_some());
    let other = CapturedFrame {
        display_id: 1,
        width: 1,
        height: 1,
        capture_ts_us: 0,
        native: Arc::new(5_u32) as Arc<dyn Any + Send + Sync>,
    };
    assert!(LinuxFrame::of(&other).is_none());
}
