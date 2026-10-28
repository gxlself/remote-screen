#![cfg(target_os = "macos")]

use std::time::{Duration, Instant};

use host_core::platform::Platform;
use host_mac::{sysquery, MacPlatform};

#[test]
fn run_echo_captures_status_and_stdout() {
    let output = sysquery::run("/bin/echo", &["hi"], Duration::from_secs(2))
        .expect("echo should start and finish");
    assert_eq!(output.status, Some(0));
    assert_eq!(output.stdout, "hi\n");
}

#[test]
fn run_timeout_kills_child() {
    let started = Instant::now();
    assert!(sysquery::run("/bin/sleep", &["5"], Duration::from_millis(200)).is_none());
    assert!(
        started.elapsed() < Duration::from_secs(1),
        "timeout query took {:?}",
        started.elapsed()
    );
}

#[test]
fn run_returns_none_when_program_cannot_start() {
    assert!(sysquery::run("/nonexistent/x", &[], Duration::from_secs(2)).is_none());
}

#[test]
fn system_queries_are_safe_and_fast() {
    let started = Instant::now();
    let filevault = sysquery::filevault_enabled();
    let filevault_elapsed = started.elapsed();
    let started = Instant::now();
    let screen_sharing = sysquery::screen_sharing_enabled();
    let screen_sharing_elapsed = started.elapsed();

    eprintln!(
        "filevault_enabled() = {:?} ({:?}); screen_sharing_enabled() = {:?} ({:?})",
        filevault, filevault_elapsed, screen_sharing, screen_sharing_elapsed
    );
    assert!(filevault_elapsed < Duration::from_millis(2500));
    assert!(screen_sharing_elapsed < Duration::from_millis(2500));
}

#[test]
fn platform_queries_match_direct_queries() {
    let direct_filevault = sysquery::filevault_enabled();
    let direct_screen_sharing = sysquery::screen_sharing_enabled();
    let platform = MacPlatform::new();
    let platform: &dyn Platform = &platform;

    assert_eq!(platform.filevault_enabled(), direct_filevault);
    assert_eq!(platform.screen_sharing_enabled(), direct_screen_sharing);
}
