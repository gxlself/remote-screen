#![cfg(target_os = "linux")]

use host_linux::audio::{chunk_start_us, f32le_samples};

#[test]
fn f32_conversion() {
    let bytes: Vec<_> = [0.5_f32, -2.0, f32::NAN, f32::INFINITY]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .chain([1, 2, 3])
        .collect();
    assert_eq!(f32le_samples(&bytes), [0.5, -1.0, 0.0, 0.0]);
}

#[test]
fn conversion_edges() {
    assert!(f32le_samples(&[]).is_empty());
    assert!(f32le_samples(&[1, 2, 3]).is_empty());
    let bytes: Vec<_> = [f32::NEG_INFINITY, f32::MAX, f32::MIN, -0.25, 1.0]
        .into_iter()
        .flat_map(f32::to_le_bytes)
        .collect();
    assert_eq!(f32le_samples(&bytes), [0.0, 1.0, -1.0, -0.25, 1.0]);
}

#[test]
fn chunk_time() {
    assert_eq!(chunk_start_us(1_000_000, 480, 48_000), 990_000);
    assert_eq!(chunk_start_us(5, 480, 48_000), 0);
}

#[test]
fn chunk_time_edges() {
    assert_eq!(chunk_start_us(1_000_000, 0, 48_000), 1_000_000);
    assert_eq!(chunk_start_us(1_000_000, 1, 48_000), 999_980);
    assert_eq!(chunk_start_us(1_000_000, 480, 0), 0);
    assert_eq!(
        chunk_start_us(u64::MAX, u32::MAX, 1),
        u64::MAX - u64::from(u32::MAX) * 1_000_000
    );
}
