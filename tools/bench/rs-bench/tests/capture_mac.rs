#![cfg(target_os = "macos")]

use std::io::Write;
use std::path::PathBuf;
use std::process::{Command, Output, Stdio};
use std::time::{SystemTime, UNIX_EPOCH};

use rs_bench::capture;
use rs_bench::capture_mac::{mach_now_unix_us, pack_plane};

fn gui_script() -> PathBuf {
    PathBuf::from(env!("CARGO_MANIFEST_DIR")).join("../mac/gui-run.sh")
}

fn gui_run(args: &[&str]) -> Output {
    Command::new("/bin/bash")
        .arg(gui_script())
        .args(args)
        .env("PATH", "/usr/bin:/bin:/usr/sbin:/sbin")
        .output()
        .expect("run gui-run.sh")
}

#[test]
fn pack_plane_cases() {
    assert_eq!(
        pack_plane(3, 2, 3, &[1, 2, 3, 4, 5, 6]),
        Some(vec![1, 2, 3, 4, 5, 6])
    );
    assert_eq!(
        pack_plane(3, 2, 6, &[1, 2, 3, 90, 91, 92, 4, 5, 6, 93, 94, 95]),
        Some(vec![1, 2, 3, 4, 5, 6])
    );
    assert_eq!(
        pack_plane(3, 2, 6, &[1, 2, 3, 90, 91, 92, 4, 5, 6]),
        Some(vec![1, 2, 3, 4, 5, 6])
    );
    assert_eq!(pack_plane(3, 2, 2, &[1, 2, 3, 4, 5, 6]), None);
    assert_eq!(pack_plane(3, 2, 6, &[1, 2, 3, 90, 91, 92, 4, 5]), None);
    assert_eq!(pack_plane(3, 1, 6, &[1, 2, 3]), Some(vec![1, 2, 3]));
    assert_eq!(pack_plane(0, 2, 0, &[]), Some(vec![]));
    assert_eq!(pack_plane(3, 0, 3, &[]), Some(vec![]));
    assert_eq!(pack_plane(2, 3, usize::MAX, &[]), None);
}

#[test]
fn platform_present() {
    let platform = capture::platform().expect("macOS capture platform");
    assert_eq!(platform.name(), "mac-sck");
}

#[test]
fn list_windows_no_panic() {
    let platform = capture::platform().expect("macOS capture platform");
    let allowed = platform.check_permission(false);
    if let Err(message) = platform.list_windows() {
        assert!(!message.is_empty(), "enumeration errors must have a reason");
        assert!(!allowed, "listing with permission must succeed: {message}");
    }
}

#[test]
fn now_matches_system_time() {
    let mach_us = mach_now_unix_us();
    let system_us = SystemTime::now()
        .duration_since(UNIX_EPOCH)
        .expect("system clock after unix epoch")
        .as_micros() as u64;
    assert!(
        mach_us.abs_diff(system_us) < 5_000,
        "mach unix {mach_us} differs from system unix {system_us}"
    );
}

#[test]
fn gui_run_dry_run() {
    let output = gui_run(&[
        "--dry-run",
        "--label",
        "ai.gxlself.rs-bench.test",
        "--",
        "/bin/echo",
        "a b",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plist = String::from_utf8(output.stdout).expect("UTF-8 plist");
    for expected in [
        "<key>Label</key>",
        "ai.gxlself.rs-bench.test",
        "Interactive",
        "<string>/bin/echo</string>",
        "<string>a b</string>",
        "<key>LimitLoadToSessionType</key><string>Aqua</string>",
        "<key>ExitTimeOut</key><integer>5</integer>",
    ] {
        assert!(plist.contains(expected), "missing {expected}: {plist}");
    }
    assert!(
        !plist.contains("/bin/bash"),
        "target must be launched directly"
    );
}

#[test]
fn gui_run_usage_errors() {
    for args in [
        vec!["--dry-run", "--"],
        vec!["--dry-run", "/bin/echo"],
        vec!["--dry-run", "--timeout", "x", "--", "/bin/echo"],
        vec!["--dry-run", "--timeout", "0", "--", "/bin/echo"],
        vec!["--dry-run", "--label", "a/b", "--", "/bin/echo"],
        vec!["--dry-run", "--label", "..", "--", "/bin/echo"],
        vec!["--stop", "ai.gxlself.rs-bench.test", "--detach"],
    ] {
        let output = gui_run(&args);
        assert_eq!(output.status.code(), Some(2), "args: {args:?}");
    }
}

fn parse_status(sample: &str) -> String {
    let mut child = Command::new("/bin/bash")
        .arg(gui_script())
        .arg("--parse-status")
        .stdin(Stdio::piped())
        .stdout(Stdio::piped())
        .stderr(Stdio::piped())
        .spawn()
        .expect("run status parser");
    child
        .stdin
        .take()
        .expect("piped stdin")
        .write_all(sample.as_bytes())
        .expect("write status sample");
    let output = child.wait_with_output().expect("status parser result");
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    String::from_utf8(output.stdout).expect("UTF-8 status")
}

#[test]
fn gui_run_parse_status() {
    for (sample, expected) in [
        (
            "gui/501/ai.gxlself.rs-bench.test = {\n\tstate = running\n\tpid = 42\n\tlast exit code = 3\n}\n",
            "running\n",
        ),
        (
            "gui/501/ai.gxlself.rs-bench.test = {\n\tstate = not running\n\tlast exit code = 0\n}\n",
            "exit 0\n",
        ),
        (
            "gui/501/ai.gxlself.rs-bench.test = {\n\tstate = not running\n\tlast exit code = 3\n}\n",
            "exit 3\n",
        ),
        (
            "gui/501/ai.gxlself.rs-bench.test = {\n\tstate = not running\n\tlast terminating signal = Terminated: 15\n}\n",
            "signal 15\n",
        ),
        (
            "gui/501/ai.gxlself.rs-bench.test = {\n\tstate = spawn scheduled\n\tlast exit code = (never exited)\n}\n",
            "unknown\n",
        ),
        (
            "state = not running\nlast exit code = 0\nlast terminating signal = Terminated: 15\n",
            "signal 15\n",
        ),
    ] {
        assert_eq!(parse_status(sample), expected, "input: {sample}");
    }
}

#[test]
fn gui_run_escapes_arguments_and_resolves_command() {
    let output = gui_run(&[
        "--dry-run",
        "--cwd",
        "/bin",
        "--label",
        "ai.gxlself.rs-bench.test",
        "--",
        "./echo",
        "<&> 中文",
        "",
        "line one\nline two\n",
    ]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plist = String::from_utf8(output.stdout).expect("UTF-8 plist");
    assert!(plist.contains("<string>/bin/echo</string>"));
    assert!(plist.contains("<string>&lt;&amp;&gt; 中文</string>"));
    assert!(plist.contains("<string></string>"));
    assert!(plist.contains("<string>line one\nline two\n</string>"));

    let output = gui_run(&["--dry-run", "--", "echo"]);
    assert!(
        output.status.success(),
        "{}",
        String::from_utf8_lossy(&output.stderr)
    );
    let plist = String::from_utf8(output.stdout).expect("UTF-8 plist");
    assert!(plist.contains("<string>/bin/echo</string>"));

    let output = gui_run(&["--dry-run", "--", "rs-bench-command-that-does-not-exist"]);
    assert_eq!(output.status.code(), Some(127));
}
