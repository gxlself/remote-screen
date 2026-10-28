//! System setting queries (screen sharing, FileVault) with a hard timeout.

use std::process::{Command, Stdio};
use std::sync::mpsc;
use std::thread;
use std::time::Duration;

use host_core::sysquery::{
    parse_fdesetup_status, parse_print_disabled, screen_sharing_state, SCREEN_SHARING_LABEL,
};

/// Maximum amount of time allowed for one system query.
pub const QUERY_TIMEOUT: Duration = Duration::from_secs(2);

/// Result of a completed child process.
pub struct Output {
    pub status: Option<i32>,
    pub stdout: String,
}

fn kill_child(pid: u32) {
    // SAFETY: `pid` was returned by a live `Child` immediately before its
    // worker started waiting; SIGKILL bounds the timeout path and the worker
    // reaps the process with `wait_with_output`.
    let _ = unsafe { libc::kill(pid as libc::pid_t, libc::SIGKILL) };
}

/// Run a command while keeping the caller bounded by `timeout`.
///
/// The child waits in a worker thread so the caller can interrupt it when the
/// timeout expires. `wait_with_output` drains stdout without blocking a child
/// that writes more than a pipe's capacity. Invalid UTF-8 is converted
/// lossily.
pub fn run(program: &str, args: &[&str], timeout: Duration) -> Option<Output> {
    let child = Command::new(program)
        .args(args)
        .stdin(Stdio::null())
        .stderr(Stdio::null())
        .stdout(Stdio::piped())
        .spawn()
        .ok()?;
    let pid = child.id();
    let (tx, rx) = mpsc::channel();

    thread::spawn(move || {
        let output = child.wait_with_output().ok().map(|output| Output {
            status: output.status.code(),
            stdout: String::from_utf8_lossy(&output.stdout).into_owned(),
        });
        let _ = tx.send(output);
    });

    match rx.recv_timeout(timeout) {
        Ok(output) => output,
        Err(mpsc::RecvTimeoutError::Timeout) => {
            kill_child(pid);
            None
        }
        Err(mpsc::RecvTimeoutError::Disconnected) => None,
    }
}

/// Query the current FileVault encryption state.
pub fn filevault_enabled() -> Option<bool> {
    let output = run("/usr/bin/fdesetup", &["status"], QUERY_TIMEOUT)?;
    if output.status != Some(0) {
        return None;
    }
    parse_fdesetup_status(&output.stdout)
}

/// Query whether the macOS screen-sharing service is enabled.
pub fn screen_sharing_enabled() -> Option<bool> {
    let started = std::time::Instant::now();
    let output = run(
        "/bin/launchctl",
        &["print-disabled", "system"],
        QUERY_TIMEOUT,
    )?;
    if output.status != Some(0) {
        return None;
    }

    let override_ = parse_print_disabled(&output.stdout, SCREEN_SHARING_LABEL);
    let service_loaded = if override_.is_some() {
        None
    } else {
        let remaining = QUERY_TIMEOUT.saturating_sub(started.elapsed());
        if remaining.is_zero() {
            return None;
        }
        let service = run(
            "/bin/launchctl",
            &["print", "system/com.apple.screensharing"],
            remaining,
        )?;
        match service.status {
            Some(0) => Some(true),
            Some(113) => Some(false),
            _ => None,
        }
    };

    screen_sharing_state(override_, service_loaded)
}
