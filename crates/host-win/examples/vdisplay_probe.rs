//! Manual Windows virtual-display probe.

#[cfg(not(windows))]
fn main() {
    println!("Windows only");
}

#[cfg(windows)]
fn main() {
    std::process::exit(probe::run());
}

#[cfg(windows)]
mod probe {
    use std::sync::Arc;
    use std::thread;
    use std::time::{Duration, Instant};

    use host_core::display_model::display_id;
    use host_core::platform::CaptureConfig;
    use host_core::vdisplay::mode::VdRequest;
    use host_core::vdisplay::{mode, VdMode, VirtualDisplayBackend};
    use host_win::{capture, displays, vdisplay};
    use tokio::sync::mpsc;

    struct Args {
        width: u32,
        height: u32,
        refresh_hz: u32,
        crash: bool,
        check_clean: bool,
    }

    impl Args {
        fn parse() -> Result<Self, String> {
            let mut args = Self {
                width: 2560,
                height: 1600,
                refresh_hz: 60,
                crash: false,
                check_clean: false,
            };
            let mut values = std::env::args().skip(1);
            while let Some(argument) = values.next() {
                match argument.as_str() {
                    "--width" => args.width = parse_value(&mut values, "--width")?,
                    "--height" => args.height = parse_value(&mut values, "--height")?,
                    "--hz" => args.refresh_hz = parse_value(&mut values, "--hz")?,
                    "--crash" => args.crash = true,
                    "--check-clean" => args.check_clean = true,
                    "--help" | "-h" => {
                        println!(
                            "vdisplay_probe [--width 2560 --height 1600 --hz 60] [--crash] [--check-clean]"
                        );
                        return Err(String::new());
                    }
                    other => return Err(format!("unknown argument: {other}")),
                }
            }
            if args.width == 0 || args.height == 0 || args.refresh_hz == 0 {
                return Err("width, height and hz must be positive".to_owned());
            }
            Ok(args)
        }
    }

    fn parse_value<T: std::str::FromStr>(
        values: &mut impl Iterator<Item = String>,
        argument: &str,
    ) -> Result<T, String> {
        values
            .next()
            .ok_or_else(|| format!("missing value for {argument}"))?
            .parse()
            .map_err(|_| format!("invalid value for {argument}"))
    }

    struct DisplayGuard {
        backend: Arc<dyn VirtualDisplayBackend>,
        token: Option<u64>,
    }

    impl DisplayGuard {
        fn new(backend: Arc<dyn VirtualDisplayBackend>, token: u64) -> Self {
            Self {
                backend,
                token: Some(token),
            }
        }

        fn disarm(&mut self) {
            self.token = None;
        }
    }

    impl Drop for DisplayGuard {
        fn drop(&mut self) {
            if let Some(token) = self.token.take() {
                let _ = self.backend.destroy(token);
            }
        }
    }

    pub fn run() -> i32 {
        let args = match Args::parse() {
            Ok(args) => args,
            Err(error) if error.is_empty() => return 0,
            Err(error) => {
                println!("PROBE FAIL {error}");
                return 1;
            }
        };
        displays::ensure_dpi_awareness();
        if args.check_clean {
            return check_clean();
        }

        let Some(backend) = vdisplay::backend() else {
            println!("UNSUPPORTED (driver not installed or not elevated)");
            return 2;
        };
        let Some(caps) = backend.caps() else {
            println!("UNSUPPORTED (driver not installed or not elevated)");
            return 2;
        };
        println!(
            "caps max {}x{} refresh {:?} max_monitors {}",
            caps.max_long_px, caps.max_short_px, caps.refresh_rates, caps.max_displays
        );
        backend.cleanup_stale();

        match run_default(args, backend, caps) {
            Ok(()) => {
                println!("PROBE PASS");
                0
            }
            Err(error) => {
                println!("PROBE FAIL {error}");
                1
            }
        }
    }

    fn check_clean() -> i32 {
        let deadline = Instant::now() + Duration::from_secs(10);
        loop {
            let stale = displays::enumerate()
                .into_iter()
                .any(|monitor| monitor.friendly_name.starts_with("RS Virtual"));
            if !stale {
                println!("PROBE PASS");
                return 0;
            }
            if Instant::now() >= deadline {
                println!("PROBE FAIL stale virtual display remains");
                return 1;
            }
            thread::sleep(Duration::from_millis(500));
        }
    }

    fn run_default(
        args: Args,
        backend: Arc<dyn VirtualDisplayBackend>,
        caps: host_core::vdisplay::VdCaps,
    ) -> Result<(), String> {
        let mode = mode::choose(
            &VdRequest {
                width_px: args.width,
                height_px: args.height,
                scale: 0.0,
                refresh_hz: args.refresh_hz,
            },
            &caps,
        )
        .map_err(|error| format!("choose mode: {error}"))?;
        let created = backend
            .create(mode, "")
            .map_err(|error| format!("create: {error}"))?;
        let mut guard = DisplayGuard::new(Arc::clone(&backend), created.token);
        let display_id = wait_for_display(&created.match_name, Duration::from_secs(10))?
            .ok_or_else(|| "display did not appear".to_owned())?;

        if args.crash {
            std::process::abort();
        }

        capture_one(display_id, mode)?;
        backend
            .destroy(created.token)
            .map_err(|error| format!("destroy: {error}"))?;
        guard.disarm();
        if wait_for_display(&created.match_name, Duration::from_secs(10))?.is_some() {
            return Err("display did not disappear".to_owned());
        }
        Ok(())
    }

    fn wait_for_display(name: &Option<String>, timeout: Duration) -> Result<Option<u32>, String> {
        let Some(name) = name.as_deref() else {
            return Err("backend returned no display name".to_owned());
        };
        let deadline = Instant::now() + timeout;
        loop {
            if let Some(monitor) = displays::enumerate()
                .into_iter()
                .find(|monitor| monitor.friendly_name == name)
            {
                return Ok(Some(display_id(&monitor.device_name)));
            }
            if Instant::now() >= deadline {
                return Ok(None);
            }
            thread::sleep(Duration::from_millis(250));
        }
    }

    fn capture_one(id: u32, mode: VdMode) -> Result<(), String> {
        let (tx, mut rx) = mpsc::channel(4);
        let capture = capture::start(
            id,
            CaptureConfig {
                fps: 10,
                max_width: 0,
                max_height: 0,
            },
            tx,
        )
        .map_err(|error| format!("capture start: {error}"))?;
        let deadline = Instant::now() + Duration::from_secs(5);
        let mut received = false;
        while Instant::now() < deadline {
            match rx.try_recv() {
                Ok(frame) => {
                    if frame.width != mode.width_px || frame.height != mode.height_px {
                        drop(capture);
                        return Err(format!(
                            "frame size {}x{} != {}x{}",
                            frame.width, frame.height, mode.width_px, mode.height_px
                        ));
                    }
                    received = true;
                    break;
                }
                Err(mpsc::error::TryRecvError::Empty) => {
                    thread::sleep(Duration::from_millis(25));
                }
                Err(mpsc::error::TryRecvError::Disconnected) => break,
            }
        }
        drop(capture);
        if received {
            Ok(())
        } else {
            Err("no frame received within 5 seconds".to_owned())
        }
    }
}
