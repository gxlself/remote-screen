#![cfg(target_os = "linux")]

use std::fs::{self, OpenOptions};
use std::io::{BufRead, BufReader, Write};
use std::os::unix::fs::PermissionsExt;
use std::os::unix::net::UnixStream;
use std::path::{Path, PathBuf};
use std::process::{Child, Command, ExitStatus, Stdio};
use std::sync::atomic::{AtomicU64, Ordering};
use std::thread;
use std::time::{Duration, Instant};

use serde_json::{json, Value};

static NEXT_DIR: AtomicU64 = AtomicU64::new(0);
const STARTUP_TIMEOUT: Duration = Duration::from_secs(10);

struct TestDir(PathBuf);

impl TestDir {
    fn new() -> Self {
        let path = std::env::temp_dir().join(format!(
            "v8x-{}-{}",
            std::process::id(),
            NEXT_DIR.fetch_add(1, Ordering::Relaxed)
        ));
        fs::create_dir(&path).expect("test directory");
        Self(path)
    }

    fn path(&self) -> &Path {
        &self.0
    }
}

impl Drop for TestDir {
    fn drop(&mut self) {
        let _ = fs::remove_dir_all(&self.0);
    }
}

struct Agent {
    child: Child,
    output: PathBuf,
}

impl Agent {
    fn spawn(dir: &Path, args: &[&str]) -> Self {
        Self::spawn_exe(Path::new(env!("CARGO_BIN_EXE_rs-host")), dir, args)
    }

    fn spawn_exe(exe: &Path, dir: &Path, args: &[&str]) -> Self {
        let output = dir.join("process.log");
        let log = fs::File::create(&output).expect("process log");
        let child = Command::new(exe)
            .args(args)
            .arg("--state-dir")
            .arg(dir)
            .current_dir(dir)
            .env("TOKIO_WORKER_THREADS", "2")
            .env("HOME", dir)
            .env("XDG_CONFIG_HOME", dir.join("xdg-config"))
            .env("XDG_STATE_HOME", dir.join(".local/state"))
            .env("XDG_RUNTIME_DIR", dir)
            .env(
                "DBUS_SYSTEM_BUS_ADDRESS",
                format!("unix:path={}/no-bus", dir.display()),
            )
            .env_remove("DISPLAY")
            .env_remove("WAYLAND_DISPLAY")
            .env_remove("XDG_SESSION_TYPE")
            .env_remove("DBUS_SESSION_BUS_ADDRESS")
            .stdout(Stdio::from(log.try_clone().expect("clone process log")))
            .stderr(Stdio::from(log))
            .spawn()
            .expect("spawn rs-host");
        let agent = Self { child, output };
        if let Ok(task) = std::env::var("RS_TEST_TASK") {
            let mut pids = OpenOptions::new()
                .create(true)
                .append(true)
                .open(std::env::temp_dir().join(format!("{task}.pids")))
                .expect("PID log");
            writeln!(pids, "{}", agent.child.id()).expect("record child PID");
        }
        agent
    }

    fn output(&self) -> String {
        fs::read_to_string(&self.output).expect("read process log")
    }

    fn request(&mut self, method: &str) -> Value {
        // Relative socket names avoid SUN_LEN limits in long worktree/temp paths.
        let socket = PathBuf::from(format!("/proc/{}/cwd/agent.sock", self.child.id()));
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll rs-host") {
                panic!("rs-host exited with {status}: {}", self.output());
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for IPC {method} response: {}",
                self.output()
            );
            if let Ok(mut stream) = UnixStream::connect(&socket) {
                stream
                    .set_read_timeout(Some(
                        deadline
                            .saturating_duration_since(Instant::now())
                            .max(Duration::from_millis(1)),
                    ))
                    .expect("set IPC read timeout");
                stream
                    .set_write_timeout(Some(Duration::from_secs(1)))
                    .expect("set IPC write timeout");
                let params = if method == "logs.export" {
                    json!({"mask": true})
                } else {
                    json!({})
                };
                writeln!(
                    stream,
                    "{}",
                    json!({"jsonrpc": "2.0", "id": 1, "method": method, "params": params})
                )
                .expect("write IPC request");
                for line in BufReader::new(stream).lines() {
                    let line = line.unwrap_or_else(|error| {
                        panic!("read IPC response: {error}: {}", self.output())
                    });
                    let response: Value = serde_json::from_str(&line).expect("IPC response JSON");
                    if response["id"] == 1 {
                        assert_eq!(response["jsonrpc"], "2.0", "{response}");
                        return response;
                    }
                }
            }
            thread::sleep(Duration::from_millis(25));
        }
    }

    fn status(&mut self) -> Value {
        let response = self.request("status.get");
        assert!(response.get("error").is_none(), "{response}");
        assert_eq!(response["result"]["online"], true, "{response}");
        assert_eq!(response["result"]["version"], env!("CARGO_PKG_VERSION"));
        response["result"].clone()
    }

    fn wait_for_exit(&mut self) -> ExitStatus {
        let deadline = Instant::now() + STARTUP_TIMEOUT;
        loop {
            if let Some(status) = self.child.try_wait().expect("poll rs-host") {
                return status;
            }
            assert!(
                Instant::now() < deadline,
                "timed out waiting for rs-host to exit: {}",
                self.output()
            );
            thread::sleep(Duration::from_millis(25));
        }
    }
}

impl Drop for Agent {
    fn drop(&mut self) {
        if self.child.try_wait().ok().flatten().is_none() {
            let _ = self.child.kill();
        }
        let _ = self.child.wait();
    }
}

#[test]
fn starts_headless_and_answers_ipc() {
    let dir = TestDir::new();
    let mut agent = Agent::spawn(
        dir.path(),
        &[
            "--platform",
            "linux",
            "--bind",
            "127.0.0.1:0",
            "--ipc-socket",
            "agent.sock",
            "--log",
            "info",
        ],
    );
    let status = agent.status();
    assert!(dir.path().join("agent.sock").exists());
    assert_eq!(status["listening"], true);
    assert_eq!(status["permissions"]["screen_recording"], false);
    let keys = dir.path().join("keys");
    assert!(keys.is_dir());
    assert_eq!(
        fs::metadata(&keys)
            .expect("keys metadata")
            .permissions()
            .mode()
            & 0o777,
        0o700
    );
    assert!(fs::read_dir(keys).expect("key files").next().is_some());
    assert!(agent.output().contains("not_installed"));
    let update = agent.request("update.status");
    assert_eq!(update["error"]["data"]["code"], "unsupported", "{update}");
}

#[test]
fn legacy_notify_uninstall_exits_without_starting() {
    let mut codes = Vec::new();
    for platform in ["linux", "fake"] {
        let dir = TestDir::new();
        fs::write(dir.path().join("config.toml"), "").expect("config");
        let mut agent = Agent::spawn(
            dir.path(),
            &[
                "--platform",
                platform,
                "--notify-uninstall",
                "--log",
                "info",
            ],
        );
        codes.push(agent.wait_for_exit().code());
        assert!(!dir.path().join("keys").exists());
        assert!(!dir.path().join("agent.sock").exists());
    }
    assert_eq!(codes, [Some(0), Some(0)]);
}

#[test]
fn default_platform_is_fake() {
    let dir = TestDir::new();
    fs::create_dir(dir.path().join("Downloads")).expect("fallback downloads directory");
    let mut agent = Agent::spawn(
        dir.path(),
        &[
            "--bind",
            "127.0.0.1:0",
            "--ipc-socket",
            "agent.sock",
            "--log",
            "error",
        ],
    );
    let status = agent.status();
    assert_eq!(status["permissions"]["screen_recording"], true);
    let exported = agent.request("pairing.exportFile");
    assert!(exported.get("error").is_none(), "{exported}");
    assert_eq!(
        Path::new(exported["result"]["path"].as_str().expect("export path")).parent(),
        Some(dir.path().join("Downloads").as_path())
    );
}

#[test]
fn installed_appimage_enables_updates_and_uses_xdg_downloads() {
    let dir = TestDir::new();
    let lib_dir = host_linux::update_appimage::default_lib_dir(dir.path());
    let app_name = format!("app-{}", env!("CARGO_PKG_VERSION"));
    let app = lib_dir.join(&app_name);
    let bin = app.join("usr/bin");
    fs::create_dir_all(&bin).expect("installed bin directory");
    let exe = bin.join("rs-host");
    fs::hard_link(env!("CARGO_BIN_EXE_rs-host"), &exe)
        .or_else(|_| fs::copy(env!("CARGO_BIN_EXE_rs-host"), &exe).map(|_| ()))
        .expect("installed executable");
    fs::write(app.join("VERSION"), env!("CARGO_PKG_VERSION")).expect("installed version");
    std::os::unix::fs::symlink(&app_name, lib_dir.join("current")).expect("current symlink");
    let config_home = dir.path().join("xdg-config");
    fs::create_dir(&config_home).expect("XDG config directory");
    fs::write(
        config_home.join("user-dirs.dirs"),
        "XDG_DOWNLOAD_DIR=\"$HOME/下载\"\n",
    )
    .expect("localized downloads config");
    let downloads = dir.path().join("下载");
    fs::create_dir(&downloads).expect("downloads directory");
    let mut agent = Agent::spawn_exe(
        &exe,
        dir.path(),
        &[
            "--platform",
            "linux",
            "--bind",
            "127.0.0.1:0",
            "--ipc-socket",
            "agent.sock",
            "--log",
            "info",
        ],
    );
    agent.status();
    let update = agent.request("update.status");
    assert!(update.get("error").is_none(), "{update}");
    assert_eq!(update["result"]["state"], "idle", "{update}");
    assert!(!agent.output().contains("not_installed"));
    for method in ["pairing.exportFile", "logs.export"] {
        let response = agent.request(method);
        assert!(response.get("error").is_none(), "{method}: {response}");
        let path = Path::new(response["result"]["path"].as_str().expect("export path"));
        assert_eq!(path.parent(), Some(downloads.as_path()), "{method}");
        assert!(path.is_file(), "{method}");
    }
}
