//! Real-display probe. Run in a logged-in graphical session on the build Mac.
//! This temporarily changes the desktop layout; never run as a unit test.

#[cfg(not(target_os = "macos"))]
fn main() -> std::process::ExitCode {
    eprintln!("vdisplay_probe only runs on macOS");
    std::process::ExitCode::FAILURE
}

#[cfg(target_os = "macos")]
fn main() -> std::process::ExitCode {
    mac::main()
}

#[cfg(target_os = "macos")]
mod mac {
    use std::io::{self, Write};
    use std::os::unix::process::ExitStatusExt;
    use std::process::{ExitCode, Stdio};
    use std::str::FromStr;
    use std::sync::Arc;
    use std::time::{Duration, Instant};

    use host_core::platform::{CaptureConfig, Platform, PlatformError};
    use host_core::vdisplay::arrange::{mac_rect, place_beside};
    use host_core::vdisplay::mode::{choose, VdRequest};
    use host_core::vdisplay::{VdCreated, VdMode, VirtualDisplayBackend};
    use host_mac::{vdisplay, MacPlatform};
    use tokio::io::{AsyncBufReadExt, BufReader};
    use tokio::process::{ChildStdout, Command};
    use tokio::sync::mpsc;

    #[derive(Clone, Copy, PartialEq, Eq)]
    enum Run {
        Normal,
        Diag,
        CrashChild,
        Child,
        Query(u32),
        ListOnline,
    }

    struct Args {
        mode: VdMode,
        run: Run,
        diag_groups: Option<Vec<char>>,
    }

    #[derive(Debug, Clone)]
    struct QuerySnapshot {
        id: u32,
        width_pt: u32,
        height_pt: u32,
        width_px: u32,
        height_px: u32,
        refresh_hz: Option<f64>,
        modes: usize,
        hidpi_mode: bool,
        line: String,
    }

    enum ProbeError {
        Unsupported,
        NoScreenRecording,
        Failed(String),
    }

    fn failed(message: impl Into<String>) -> ProbeError {
        ProbeError::Failed(message.into())
    }

    fn parse_value<T: FromStr>(
        values: &mut impl Iterator<Item = String>,
        flag: &str,
    ) -> Result<T, ProbeError> {
        values
            .next()
            .and_then(|value| value.parse().ok())
            .ok_or_else(|| failed(format!("{flag} requires a valid value")))
    }

    fn parse_args() -> Result<Option<Args>, ProbeError> {
        let mut request = VdRequest {
            width_px: 2732,
            height_px: 2048,
            scale: 2.0,
            refresh_hz: 60,
        };
        let mut run = Run::Normal;
        let mut diag_groups = None;
        let mut values = std::env::args().skip(1);
        while let Some(flag) = values.next() {
            match flag.as_str() {
                "--width" => request.width_px = parse_value(&mut values, &flag)?,
                "--height" => request.height_px = parse_value(&mut values, &flag)?,
                "--scale" => request.scale = parse_value(&mut values, &flag)?,
                "--hz" => request.refresh_hz = parse_value(&mut values, &flag)?,
                "--diag" | "--crash-child" | "--child" => {
                    if run != Run::Normal {
                        return Err(failed(
                            "choose only one run mode (--diag, --crash-child, --child, or --query-display)",
                        ));
                    }
                    run = match flag.as_str() {
                        "--diag" => Run::Diag,
                        "--child" => Run::Child,
                        _ => Run::CrashChild,
                    };
                }
                "--list-online" => {
                    if run != Run::Normal {
                        return Err(failed(
                            "choose only one run mode (--diag, --crash-child, --child, or --query-display)",
                        ));
                    }
                    run = Run::ListOnline;
                }
                "--query-display" => {
                    if run != Run::Normal {
                        return Err(failed(
                            "choose only one run mode (--diag, --crash-child, --child, or --query-display)",
                        ));
                    }
                    let id: u32 = parse_value(&mut values, &flag)?;
                    if id == 0 {
                        return Err(failed("--query-display requires a non-zero display ID"));
                    }
                    run = Run::Query(id);
                }
                "--diag-groups" => {
                    let value: String = parse_value(&mut values, &flag)?;
                    let groups = value
                        .split(',')
                        .filter_map(|group| group.trim().chars().next())
                        .map(|group| group.to_ascii_uppercase())
                        .collect::<Vec<_>>();
                    if groups.is_empty() || groups.iter().any(|group| !matches!(group, 'A'..='H')) {
                        return Err(failed("--diag-groups must contain letters A-H"));
                    }
                    diag_groups = Some(groups);
                }
                "--help" | "-h" => {
                    println!("usage: vdisplay_probe [--width 2732 --height 2048 --scale 2 --hz 60] [--diag [--diag-groups A,B,E]] [--query-display ID] [--list-online] [--crash-child]");
                    return Ok(None);
                }
                _ => return Err(failed(format!("unknown argument: {flag}"))),
            }
        }
        if matches!(run, Run::Query(_) | Run::ListOnline) {
            return Ok(Some(Args {
                mode: VdMode {
                    width_px: request.width_px,
                    height_px: request.height_px,
                    scale: request.scale,
                    refresh_hz: request.refresh_hz,
                },
                run,
                diag_groups,
            }));
        }
        let caps = vdisplay::mac_caps();
        if !caps.scales.contains(&request.scale)
            || !caps.refresh_rates.contains(&request.refresh_hz)
        {
            return Err(failed("--scale must be 1 or 2; --hz must be 30 or 60"));
        }
        let mode = choose(&request, &caps).map_err(|error| failed(error.to_string()))?;
        if mode.width_px != request.width_px || mode.height_px != request.height_px {
            return Err(failed(
                "requested dimensions must fit Mac caps and be multiples of 4",
            ));
        }
        Ok(Some(Args {
            mode,
            run,
            diag_groups,
        }))
    }

    // The backend singleton outlives the probe, so owning its Arc alone does not
    // release the screen. Always retain the token in an explicit Drop guard.
    struct DisplayGuard {
        backend: Arc<dyn VirtualDisplayBackend>,
        created: VdCreated,
        live: bool,
    }

    impl DisplayGuard {
        async fn create(
            backend: Arc<dyn VirtualDisplayBackend>,
            mode: VdMode,
        ) -> Result<Self, ProbeError> {
            // Host also calls create from spawn_blocking. In particular, do
            // not accidentally hide a main-thread requirement in this probe.
            tokio::task::spawn_blocking(move || {
                let created = backend
                    .create(mode, "Remote Screen Probe")
                    .map_err(|error| failed(error.to_string()))?;
                let guard = Self {
                    backend,
                    created,
                    live: true,
                };
                eprintln!("MODE worker backend returned {:?}", guard.created.mode);
                if let Some(id) = guard.created.platform_id {
                    let (_, active) = mode_snapshot("worker", id);
                    eprintln!(
                        "MODE worker report snapshot {}",
                        if active.and_then(|mode| mode.actual()) == Some(guard.created.mode) {
                            "confirmed"
                        } else {
                            "unconfirmed (未确认)"
                        }
                    );
                }
                Ok(guard)
            })
            .await
            .map_err(|error| failed(format!("create worker: {error}")))?
        }

        fn id(&self) -> Result<u32, ProbeError> {
            self.created
                .platform_id
                .filter(|&id| id != 0)
                .ok_or_else(|| failed("backend returned no display ID"))
        }

        fn destroy(&mut self) -> Result<(), ProbeError> {
            if self.live {
                self.backend
                    .destroy(self.created.token)
                    .map_err(|error| failed(error.to_string()))?;
                self.live = false;
            }
            Ok(())
        }
    }

    impl Drop for DisplayGuard {
        fn drop(&mut self) {
            if let Err(ProbeError::Failed(message)) = self.destroy() {
                eprintln!("FAIL cleanup: {message}");
            }
        }
    }

    fn mode_snapshot(
        context: &str,
        id: u32,
    ) -> (Vec<vdisplay::DisplayMode>, Option<vdisplay::DisplayMode>) {
        let available = vdisplay::display_modes(id);
        let active = vdisplay::current_mode(id);
        // stderr also reaches the parent of --child; stdout remains the ID
        // handshake. Log empty lists explicitly instead of losing evidence.
        eprintln!(
            "MODE {context} display {id} available count {}",
            available.len()
        );
        for candidate in &available {
            eprintln!(
                "MODE {context} {} pixels {}x{} points {}x{} refresh {} Hz usable {}",
                candidate.mode_id,
                candidate.width_px,
                candidate.height_px,
                candidate.width_pt,
                candidate.height_pt,
                candidate.refresh_hz,
                candidate.usable
            );
        }
        eprintln!("MODE {context} active {active:?}");
        (available, active)
    }

    fn parse_dimensions(value: &str, suffix: &str) -> Option<(u32, u32)> {
        let value = value.strip_suffix(suffix).unwrap_or(value);
        let (width, height) = value.split_once('x')?;
        Some((width.parse().ok()?, height.parse().ok()?))
    }

    fn parse_query_line(line: &str) -> Option<QuerySnapshot> {
        let mut fields = line.split_whitespace();
        if fields.next()? != "QUERY" {
            return None;
        }
        let id = fields.next()?.strip_prefix("id=")?.parse().ok()?;
        let current = fields.next()?.strip_prefix("cur=")?;
        let (points, pixels_and_rate) = current.split_once("pt/")?;
        let (pixels, refresh) = pixels_and_rate.split_once('@')?;
        let (width_pt, height_pt) = parse_dimensions(points, "")?;
        let (width_px, height_px) = parse_dimensions(pixels, "px")?;
        let refresh_hz = match refresh.strip_suffix("Hz")? {
            "unknown" => None,
            value => Some(value.parse().ok()?),
        };
        let modes = fields.next()?.strip_prefix("modes=")?.parse().ok()?;
        let hidpi_mode = match fields.next()?.strip_prefix("hidpi_mode=")? {
            "yes" => true,
            "no" => false,
            _ => return None,
        };
        Some(QuerySnapshot {
            id,
            width_pt,
            height_pt,
            width_px,
            height_px,
            refresh_hz,
            modes,
            hidpi_mode,
            line: line.to_owned(),
        })
    }

    fn print_query_display(id: u32) -> Result<(), ProbeError> {
        let current = vdisplay::current_mode(id)
            .ok_or_else(|| failed(format!("display {id} has no current mode")))?;
        let modes = vdisplay::display_modes(id);
        let hidpi_mode = current.width_pt > 0
            && current.height_pt > 0
            && current.width_px == current.width_pt.saturating_mul(2)
            && current.height_px == current.height_pt.saturating_mul(2);
        let refresh = if current.refresh_hz.is_finite() && current.refresh_hz > 0.0 {
            current.refresh_hz.to_string()
        } else {
            "unknown".to_owned()
        };
        println!(
            "QUERY id={} cur={}x{}pt/{}x{}px@{}Hz modes={} hidpi_mode={}",
            id,
            current.width_pt,
            current.height_pt,
            current.width_px,
            current.height_px,
            refresh,
            modes.len(),
            if hidpi_mode { "yes" } else { "no" }
        );
        for mode in modes {
            eprintln!(
                "QUERY_MODE id={} pixels={}x{} points={}x{} refresh={} usable={}",
                mode.mode_id,
                mode.width_px,
                mode.height_px,
                mode.width_pt,
                mode.height_pt,
                mode.refresh_hz,
                mode.usable
            );
        }
        Ok(())
    }

    async fn query_display_child(id: u32) -> Result<QuerySnapshot, ProbeError> {
        let executable = std::env::current_exe()
            .map_err(|error| failed(format!("locate probe executable: {error}")))?;
        let mut last_error = String::from("query child returned no QUERY line");
        for _ in 0..3 {
            // A fresh process can take several seconds to reach WindowServer
            // right after a display is created (1.1-4 s measured on a Mac build machine).
            let Ok(output) = tokio::time::timeout(
                Duration::from_secs(5),
                Command::new(&executable)
                    .args(["--query-display".to_owned(), id.to_string()])
                    .stdin(Stdio::null())
                    .stdout(Stdio::piped())
                    .stderr(Stdio::inherit())
                    .kill_on_drop(true)
                    .output(),
            )
            .await
            else {
                last_error = String::from("query child timed out");
                continue;
            };
            let output = output.map_err(|error| failed(format!("query child: {error}")))?;
            let stdout = String::from_utf8_lossy(&output.stdout);
            if let Some(line) = stdout.lines().find(|line| line.starts_with("QUERY ")) {
                if let Some(snapshot) = parse_query_line(line) {
                    return Ok(snapshot);
                }
                last_error = format!("malformed query output: {line}");
            } else {
                last_error = format!(
                    "query child exited with {} and output {:?}",
                    output.status,
                    stdout.trim()
                );
            }
            tokio::time::sleep(Duration::from_millis(150)).await;
        }
        Err(failed(last_error))
    }

    fn print_online() {
        let identities = vdisplay::online_display_identities();
        let mut ids = identities
            .iter()
            .map(|display| display.id)
            .collect::<Vec<_>>();
        let mut virtual_ids = identities
            .iter()
            .filter(|display| display.vendor == 21075 && display.model == 22084)
            .map(|display| display.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        virtual_ids.sort_unstable();
        println!("ONLINE ids={ids:?} vd={virtual_ids:?}");
    }

    fn parse_ids(value: &str) -> Option<Vec<u32>> {
        let value = value.strip_prefix('[')?.strip_suffix(']')?;
        if value.is_empty() {
            return Some(Vec::new());
        }
        value.split(',').map(|id| id.trim().parse().ok()).collect()
    }

    /// Online display IDs as a fresh process sees them. The creating process
    /// keeps a stale CoreGraphics display cache on macOS 26.1 unless its main
    /// run loop is serviced, so removal is judged from a child.
    async fn online_child() -> Result<(Vec<u32>, Vec<u32>), ProbeError> {
        let executable = std::env::current_exe()
            .map_err(|error| failed(format!("locate probe executable: {error}")))?;
        let output = tokio::time::timeout(
            Duration::from_secs(2),
            Command::new(&executable)
                .arg("--list-online")
                .stdin(Stdio::null())
                .stdout(Stdio::piped())
                .stderr(Stdio::null())
                .output(),
        )
        .await
        .map_err(|_| failed("list-online child timed out"))?
        .map_err(|error| failed(format!("list-online child: {error}")))?;
        let stdout = String::from_utf8_lossy(&output.stdout);
        stdout
            .lines()
            .find_map(|line| {
                let rest = line.strip_prefix("ONLINE ids=")?;
                let (ids, virtual_ids) = rest.split_once(" vd=")?;
                Some((parse_ids(ids)?, parse_ids(virtual_ids)?))
            })
            .ok_or_else(|| failed(format!("malformed list-online output: {:?}", stdout.trim())))
    }

    /// Polls fresh processes until `done` holds for the online virtual display
    /// IDs. Returns the last list seen and how long it took.
    async fn wait_child(
        timeout: Duration,
        mut done: impl FnMut(&[u32]) -> bool,
    ) -> (bool, Vec<u32>, Duration) {
        let started = Instant::now();
        let mut last = Vec::new();
        loop {
            if let Ok((_, virtual_ids)) = online_child().await {
                if done(&virtual_ids) {
                    return (true, virtual_ids, started.elapsed());
                }
                last = virtual_ids;
            }
            if started.elapsed() >= timeout {
                return (false, last, started.elapsed());
            }
            tokio::time::sleep(Duration::from_millis(300)).await;
        }
    }

    fn check(ok: bool, description: impl AsRef<str>) -> bool {
        println!(
            "{} {}",
            if ok { "OK" } else { "FAIL" },
            description.as_ref()
        );
        ok
    }

    async fn wait_until(timeout: Duration, mut predicate: impl FnMut() -> bool) -> bool {
        let deadline = Instant::now() + timeout;
        loop {
            if predicate() {
                return true;
            }
            let remaining = deadline.saturating_duration_since(Instant::now());
            if remaining.is_zero() {
                return false;
            }
            tokio::time::sleep(remaining.min(Duration::from_millis(50))).await;
        }
    }

    async fn absent(platform: &MacPlatform, id: u32, timeout: Duration) -> bool {
        wait_until(timeout, || {
            platform.displays().iter().all(|display| display.id != id)
        })
        .await
    }

    fn display_ids(platform: &MacPlatform) -> Vec<u32> {
        let mut ids = platform
            .displays()
            .iter()
            .map(|display| display.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        ids
    }

    fn report_orphans() {
        let mut ids = vdisplay::online_display_identities()
            .into_iter()
            .filter(|display| display.vendor == 21075 && display.model == 22084)
            .map(|display| display.id)
            .collect::<Vec<_>>();
        ids.sort_unstable();
        if !ids.is_empty() {
            println!("ORPHANS ids={ids:?}");
        }
    }

    async fn diag_step(
        run_loop: &vdisplay::diagnostics::MainRunLoop,
        pump: bool,
        deadline: Instant,
    ) {
        let slice = deadline
            .saturating_duration_since(Instant::now())
            .min(Duration::from_millis(50));
        if slice.is_zero() {
            return;
        }
        if pump {
            // main() uses Tokio's current_thread runtime, so this is the
            // original process main thread, including while create is pending.
            run_loop.pump(slice);
            tokio::task::yield_now().await;
        } else {
            tokio::time::sleep(slice).await;
        }
    }

    fn matches_mode(
        active: Option<vdisplay::DisplayMode>,
        preferred: vdisplay::DisplayMode,
    ) -> bool {
        active.is_some_and(|active| {
            active.actual() == preferred.actual()
                && active.actual().is_some()
                && active.width_pt == preferred.width_pt
                && active.height_pt == preferred.height_pt
                && (active.refresh_hz - preferred.refresh_hz).abs() < 0.5
        })
    }

    async fn diag_case(
        platform: &MacPlatform,
        run_loop: &vdisplay::diagnostics::MainRunLoop,
        mode: VdMode,
        variant: vdisplay::diagnostics::Variant,
        label: &'static str,
        pump: bool,
        deadline: Instant,
    ) -> Result<bool, ProbeError> {
        let before = run_loop.events();
        let create = tokio::task::spawn_blocking(move || {
            // Same private creation/descriptor queue as Host; deliberately
            // skip the backend's metadata timeout and placement transaction.
            let display = vdisplay::diagnostics::Display::create(mode, variant)?;
            mode_snapshot(
                &format!("diag {label} runloop={} worker", u8::from(pump)),
                display.id(),
            );
            Ok::<_, host_core::vdisplay::VdError>(display)
        });
        while !create.is_finished() {
            if Instant::now() >= deadline {
                // A detached blocking task drops its owned result on return,
                // including a display created after this diagnostic timed out.
                drop(create);
                let after = run_loop.events();
                println!(
                    "DIAG {label} runloop={} available=0 active=None ok=0 callbacks={} error=create-timeout",
                    u8::from(pump),
                    after.callbacks.saturating_sub(before.callbacks)
                );
                return Err(failed(format!(
                    "DIAG {label} creation exceeded its 3 second budget"
                )));
            }
            diag_step(run_loop, pump, deadline).await;
        }
        let display = match create
            .await
            .map_err(|error| failed(format!("diag create worker: {error}")))?
        {
            Ok(display) => display,
            Err(error) => {
                let after = run_loop.events();
                println!(
                    "DIAG {label} runloop={} available=0 active=None ok=0 callbacks={} error={error}",
                    u8::from(pump),
                    after.callbacks.saturating_sub(before.callbacks)
                );
                return Ok(false);
            }
        };
        // The guard owns the native object before querying or printing. Every
        // return/unwind destroys it, and the caller checks the desktop is back.
        let id = display.id();
        let mut selected = None;
        while Instant::now() < deadline {
            let modes = vdisplay::display_modes(id);
            if let Some(preferred) = vdisplay::closest_mode(&modes, &mode) {
                let active = vdisplay::current_mode(id);
                if !matches_mode(active, preferred) && selected != Some(preferred.mode_id) {
                    let error = display.select_mode(preferred.mode_id);
                    selected = Some(preferred.mode_id);
                    println!("DIAG select {label} runloop={} before={active:?} target={preferred:?} CGError={error}", u8::from(pump));
                }
            }
            diag_step(run_loop, pump, deadline).await;
        }
        let (available, active) =
            mode_snapshot(&format!("diag {label} runloop={} main", u8::from(pump)), id);
        let closest = vdisplay::closest_mode(&available, &mode);
        let active_text = active.map_or_else(
            || "None".to_owned(),
            |active| {
                format!(
                    "{}x{} px / {}x{} pt @{}Hz",
                    active.width_px,
                    active.height_px,
                    active.width_pt,
                    active.height_pt,
                    active.refresh_hz
                )
            },
        );
        let after = run_loop.events();
        let callback_count = after.callbacks.saturating_sub(before.callbacks);
        println!(
            "DIAG {label} runloop={} available={} active=<{active_text}> ok={} callbacks={}",
            u8::from(pump),
            available.len(),
            u8::from(closest.is_some_and(|preferred| matches_mode(active, preferred))),
            callback_count
        );
        if let Some(enumerated) = platform.displays().iter().find(|display| display.id == id) {
            eprintln!(
                "DIAG enumeration {label} runloop={} reported={enumerated:?} refresh_source={}",
                u8::from(pump),
                refresh_source(active)
            );
        }
        println!("DIAG events {label} runloop={} callbacks={} main_callbacks={} last_display={} flags_seen={:#x}", u8::from(pump), callback_count, after.main_callbacks.saturating_sub(before.main_callbacks), after.last_display_id, after.flags);
        let query = match query_display_child(id).await {
            Ok(query) => query,
            Err(error) => {
                let message = match error {
                    ProbeError::Failed(message) => message,
                    ProbeError::Unsupported => "unsupported".to_owned(),
                    ProbeError::NoScreenRecording => "screen-recording-permission".to_owned(),
                };
                println!(
                    "DIAG query {label} runloop={} ok=0 error={message}",
                    u8::from(pump)
                );
                if let Err(error) = display.destroy() {
                    println!(
                        "DIAG destroy {label} runloop={} ok=0 error={error}",
                        u8::from(pump)
                    );
                }
                return Ok(false);
            }
        };
        let hidpi_geometry = query.width_px == query.width_pt.saturating_mul(2)
            && query.height_px == query.height_pt.saturating_mul(2);
        let query_ok = query.id == id
            && query.modes > 0
            && query.width_px == mode.width_px
            && query.height_px == mode.height_px
            && (!vdisplay::is_hidpi(&mode) || (query.hidpi_mode && hidpi_geometry))
            && query
                .refresh_hz
                .is_some_and(|refresh| (refresh - f64::from(mode.refresh_hz)).abs() < 0.5);
        println!(
            "DIAG query {label} runloop={} {} ok={}",
            u8::from(pump),
            query.line,
            u8::from(query_ok)
        );
        if let Err(error) = display.destroy() {
            println!(
                "DIAG destroy {label} runloop={} ok=0 error={error}",
                u8::from(pump)
            );
            return Ok(false);
        }
        Ok(query_ok)
    }

    /// Judges cleanup from a fresh process (up to 10 s) and records whether the
    /// in-process list caught up, which shows if run-loop servicing refreshes it.
    async fn diag_cleanup(
        platform: &MacPlatform,
        run_loop: &vdisplay::diagnostics::MainRunLoop,
        pump: bool,
        label: &str,
        initial: &[u32],
        initial_vd: &[u32],
    ) -> bool {
        let started = Instant::now();
        let mut inproc_ms = None;
        let mut child = None;
        while started.elapsed() < Duration::from_secs(10) {
            if inproc_ms.is_none() && display_ids(platform) == initial {
                inproc_ms = Some(started.elapsed().as_millis());
            }
            if child.is_none() {
                if let Ok((_, virtual_ids)) = online_child().await {
                    if virtual_ids == initial_vd {
                        child = Some((started.elapsed().as_millis(), virtual_ids));
                    }
                }
            }
            if child.is_some() && inproc_ms.is_some() {
                break;
            }
            diag_step(run_loop, pump, Instant::now() + Duration::from_millis(300)).await;
        }
        let child_vd = match &child {
            Some((_, ids)) => ids.clone(),
            None => online_child().await.map(|(_, ids)| ids).unwrap_or_default(),
        };
        println!(
            "DIAG cleanup {label} runloop={} removed={} child_ms={} inproc_caught_up={} inproc_ms={} before_vd={initial_vd:?} child_vd={child_vd:?} inproc_after_ids={:?}",
            u8::from(pump),
            u8::from(child.is_some()),
            child.as_ref().map_or_else(|| "-".to_owned(), |(ms, _)| ms.to_string()),
            u8::from(inproc_ms.is_some()),
            inproc_ms.map_or_else(|| "-".to_owned(), |ms| ms.to_string()),
            display_ids(platform)
        );
        child.is_some()
    }

    async fn diagnose(
        platform: &MacPlatform,
        mode: VdMode,
        selected_groups: Option<&[char]>,
    ) -> Result<(), ProbeError> {
        use vdisplay::diagnostics::{MainRunLoop, Variant};

        let started = Instant::now();
        let suite_deadline = started + Duration::from_secs(180);
        let selected = |label: char| selected_groups.is_none_or(|groups| groups.contains(&label));
        // This must be the first diagnostic output. It records every private
        // class method/property before an experiment can alter AppKit state.
        vdisplay::diagnostics::introspect();
        println!("DIAG introspection emitted=1");
        // Registration installs a notification observer and a dormant source;
        // only pump=1 calls CFRunLoopRunInMode. No NSApplication/SCK is started.
        let run_loop = MainRunLoop::register().map_err(|error| failed(error.to_string()))?;
        println!("DIAG observer registered=1 main_thread=1 pump_slice_ms=50 case_budget_ms=3000 total_budget_ms=180000");
        let (queue, dispatch_queue) = vdisplay::diagnostics::queue_selectors();
        println!(
            "DIAG selectors setQueue={} setDispatchQueue={} selected={}",
            u8::from(queue),
            u8::from(dispatch_queue),
            if queue {
                "setQueue:"
            } else {
                "setDispatchQueue:"
            }
        );
        println!("DIAG request {mode:?}; A=current, B=pixels, C=points, D=pixels+points, E=AppKit+main queue, F=DeskPad pixel list, G=delayed apply, H=retained objects; PASS uses child QUERY geometry and refresh");
        let initial = display_ids(platform);
        let (_, initial_vd) = online_child().await?;
        println!("DIAG initial child_vd={initial_vd:?}");
        for &id in &initial {
            println!(
                "DIAG control display={id} available={} active={:?}",
                vdisplay::display_modes(id).len(),
                vdisplay::current_mode(id)
            );
        }
        let mut modes_ok = 0;
        for (variant, label) in [
            (Variant::Current, "A"),
            (Variant::Pixels, "B"),
            (Variant::Points, "C"),
            (Variant::PixelsAndPoints, "D"),
        ] {
            let label_char = label.chars().next().unwrap_or('A');
            if !selected(label_char) {
                continue;
            }
            for pump in [false, true] {
                if Instant::now() + Duration::from_secs(5) > suite_deadline {
                    return Err(failed("DIAG stopped at the total time budget"));
                }
                let ok = diag_case(
                    platform,
                    &run_loop,
                    mode,
                    variant,
                    label,
                    pump,
                    Instant::now() + Duration::from_secs(3),
                )
                .await?;
                modes_ok += usize::from(ok);
                let removed =
                    diag_cleanup(platform, &run_loop, pump, label, &initial, &initial_vd).await;
                if !removed {
                    return Err(failed(
                        "DIAG desktop not restored after destroy; stopping further creation",
                    ));
                }
            }
        }
        let appkit_selected = ['E', 'F', 'G', 'H'].iter().copied().any(selected);
        if appkit_selected {
            // E-H explicitly model DeskPad's AppKit lifecycle. This call is on
            // the process main thread because the Tokio runtime is current-
            // thread and main() entered it without spawning a worker.
            vdisplay::diagnostics::initialize_appkit()
                .map_err(|error| failed(error.to_string()))?;
            println!("DIAG appkit initialized=1 activation_policy=accessory");
        }
        for (variant, label) in [
            (Variant::AppKit, 'E'),
            (Variant::DeskPad, 'F'),
            (Variant::DelayedApply, 'G'),
            (Variant::RetainedObjects, 'H'),
        ] {
            if !selected(label) {
                continue;
            }
            if Instant::now() + Duration::from_secs(5) > suite_deadline {
                return Err(failed("DIAG stopped at the total time budget"));
            }
            let ok = diag_case(
                platform,
                &run_loop,
                mode,
                variant,
                match label {
                    'E' => "E",
                    'F' => "F",
                    'G' => "G",
                    _ => "H",
                },
                true,
                Instant::now() + Duration::from_secs(3),
            )
            .await?;
            modes_ok += usize::from(ok);
            let label = match label {
                'E' => "E",
                'F' => "F",
                'G' => "G",
                _ => "H",
            };
            let removed =
                diag_cleanup(platform, &run_loop, true, label, &initial, &initial_vd).await;
            if !removed {
                return Err(failed(
                    "DIAG desktop not restored after destroy; stopping further creation",
                ));
            }
        }
        // Some controls are intentionally unsuccessful; diagnostic success
        // means every selected experiment completed and every display was removed.
        let selected_case_count = selected_groups.map_or(12, |groups| {
            groups
                .iter()
                .map(|group| if *group <= 'D' { 2 } else { 1 })
                .sum::<usize>()
        });
        println!(
            "DIAG complete cases={selected_case_count} modes_ok={modes_ok}/{selected_case_count} elapsed_ms={}",
            started.elapsed().as_millis()
        );
        Ok(())
    }

    fn refresh_source(active: Option<vdisplay::DisplayMode>) -> &'static str {
        if active.is_some_and(|mode| mode.refresh_hz.is_finite() && mode.refresh_hz > 0.0) {
            "CGDisplayModeGetRefreshRate (measured)"
        } else {
            "unmeasured (rs_list_displays default omitted)"
        }
    }

    async fn capture_frame(platform: &MacPlatform, id: u32) -> Result<(u32, u32), ProbeError> {
        let (tx, mut rx) = mpsc::channel(4);
        let platform = platform.clone();
        // Include SCK startup in the five-second deadline without blocking the
        // timer thread. A late handle is dropped even if its join is abandoned.
        let start = tokio::task::spawn_blocking(move || {
            platform.capture(
                id,
                CaptureConfig {
                    fps: 10,
                    max_width: 0,
                    max_height: 0,
                },
                tx,
            )
        });
        tokio::time::timeout(Duration::from_secs(5), async {
            let _capture = start
                .await
                .map_err(|error| failed(format!("capture worker: {error}")))?
                .map_err(|error| match error {
                    PlatformError::PermissionDenied(_) => ProbeError::NoScreenRecording,
                    other => failed(format!("SCK startup: {other}")),
                })?;
            let frame = rx
                .recv()
                .await
                .ok_or_else(|| failed("SCK frame channel closed"))?;
            if frame.display_id != id {
                return Err(failed("SCK returned a frame from another display"));
            }
            Ok((frame.width, frame.height))
        })
        .await
        .map_err(|_| failed("SCK did not deliver a frame within 5 seconds"))?
    }

    async fn normal(
        platform: &MacPlatform,
        backend: Arc<dyn VirtualDisplayBackend>,
        mode: VdMode,
    ) -> Result<(), ProbeError> {
        let before = platform.displays();
        let rects = before
            .iter()
            .map(|display| (mac_rect(display), display.primary))
            .collect::<Vec<_>>();
        let (expected_x, expected_y) = place_beside(&rects);
        let mut guard = DisplayGuard::create(backend, mode).await?;
        let id = guard.id()?;
        // The creating process may expose stale 1x metadata on macOS 26.1.
        // Keep it as evidence only; mode PASS/FAIL comes from a fresh process.
        let (_, active) = mode_snapshot("probe", id);
        let current = platform.displays();
        eprintln!(
            "MODE probe platform refresh source: {}",
            refresh_source(active)
        );
        let display = current.iter().find(|display| display.id == id);
        let mut passed = check(display.is_some(), format!("display {id} enumerated"));
        if let Some(display) = display {
            passed &= check(
                (display.x, display.y) == (expected_x, expected_y),
                format!(
                    "origin ({}, {}) (expected {expected_x}, {expected_y})",
                    display.x, display.y
                ),
            );
            passed &= check(
                !display.primary && !before.iter().any(|old| old.id == id),
                "new display is not primary",
            );
        }

        let query = query_display_child(id).await?;
        println!("{}", query.line);
        passed &= check(query.id == id, format!("query returned display {id}"));
        passed &= check(
            query.modes > 0,
            format!("query returned {} modes", query.modes),
        );
        let hidpi_geometry = query.width_px == query.width_pt.saturating_mul(2)
            && query.height_px == query.height_pt.saturating_mul(2);
        if vdisplay::is_hidpi(&mode) {
            passed &= check(
                query.hidpi_mode && hidpi_geometry,
                format!(
                    "child query HiDPI {}x{}pt/{}x{}px",
                    query.width_pt, query.height_pt, query.width_px, query.height_px
                ),
            );
        }
        passed &= check(
            query.width_px == mode.width_px && query.height_px == mode.height_px,
            format!(
                "child query pixels {}x{} (expected {}x{})",
                query.width_px, query.height_px, mode.width_px, mode.height_px
            ),
        );
        passed &= check(
            query
                .refresh_hz
                .is_some_and(|refresh| (refresh - f64::from(mode.refresh_hz)).abs() < 0.5),
            format!(
                "child query refresh {} Hz (requested {})",
                query
                    .refresh_hz
                    .map_or_else(|| "unknown".to_owned(), |refresh| refresh.to_string()),
                mode.refresh_hz
            ),
        );

        let capture = capture_frame(platform, id).await;
        match &capture {
            Ok((width, height)) => {
                passed &= check(
                    (*width, *height) == (mode.width_px, mode.height_px),
                    format!("SCK frame {width}x{height}"),
                );
            }
            Err(ProbeError::NoScreenRecording) => println!("NO_SCREEN_RECORDING"),
            Err(ProbeError::Failed(message)) => {
                check(false, message);
            }
            Err(ProbeError::Unsupported) => {
                check(false, "SCK is unsupported");
            }
        }
        // Cleanup and its disappearance check run even when capture is denied.
        // A fresh process is authoritative; the in-process view is evidence.
        let destroyed = guard.destroy();
        let (removed, child_vd, waited) =
            wait_child(Duration::from_secs(10), |ids| !ids.contains(&id)).await;
        passed &= check(
            removed,
            format!(
                "display {id} removed after destroy (child view, {} ms, vd={child_vd:?})",
                waited.as_millis()
            ),
        );
        let inproc_removed = absent(platform, id, Duration::from_millis(500)).await;
        println!(
            "INFO in-process list {} display {id}; backend destroy {}",
            if inproc_removed {
                "dropped"
            } else {
                "still lists"
            },
            match &destroyed {
                Ok(()) => "ok".to_owned(),
                Err(ProbeError::Failed(message)) => format!("error: {message}"),
                Err(_) => "error".to_owned(),
            }
        );
        if !removed {
            destroyed?;
        }
        capture?;
        if !passed {
            return Err(failed("virtual display geometry or cleanup check failed"));
        }
        Ok(())
    }

    async fn child(
        backend: Arc<dyn VirtualDisplayBackend>,
        mode: VdMode,
    ) -> Result<(), ProbeError> {
        let guard = DisplayGuard::create(backend, mode).await?;
        println!("ID {}", guard.id()?);
        io::stdout()
            .flush()
            .map_err(|error| failed(format!("flush display ID: {error}")))?;
        // Remain alive until the parent sends SIGKILL. Ctrl-C permits a manual
        // --child run to unwind the guard and destroy its display normally.
        tokio::signal::ctrl_c()
            .await
            .map_err(|error| failed(format!("wait for signal: {error}")))?;
        Ok(())
    }

    async fn child_id(stdout: ChildStdout) -> Result<u32, ProbeError> {
        let mut lines = BufReader::new(stdout).lines();
        tokio::time::timeout(Duration::from_secs(10), async {
            while let Some(line) = lines
                .next_line()
                .await
                .map_err(|error| failed(format!("read child stdout: {error}")))?
            {
                if let Some(value) = line.strip_prefix("ID ") {
                    return value
                        .parse::<u32>()
                        .ok()
                        .filter(|&id| id != 0)
                        .ok_or_else(|| failed("child returned an invalid display ID"));
                }
                if line == "UNSUPPORTED" {
                    return Err(ProbeError::Unsupported);
                }
                if line.starts_with("FAIL ") {
                    return Err(failed(format!("child: {line}")));
                }
            }
            Err(failed("child exited before returning a display ID"))
        })
        .await
        .map_err(|_| failed("child did not report a display ID within 10 seconds"))?
    }

    async fn crash_child(platform: &MacPlatform, mode: VdMode) -> Result<(), ProbeError> {
        let executable = std::env::current_exe()
            .map_err(|error| failed(format!("locate probe executable: {error}")))?;
        let mut child = Command::new(executable)
            .args([
                "--child".to_owned(),
                "--width".to_owned(),
                mode.width_px.to_string(),
                "--height".to_owned(),
                mode.height_px.to_string(),
                "--scale".to_owned(),
                mode.scale.to_string(),
                "--hz".to_owned(),
                mode.refresh_hz.to_string(),
            ])
            .stdin(Stdio::null())
            .stdout(Stdio::piped())
            .stderr(Stdio::inherit())
            // Child's Drop is also a guard for every error/unwind after spawn.
            .kill_on_drop(true)
            .spawn()
            .map_err(|error| failed(format!("start child: {error}")))?;
        let ready = match child.stdout.take() {
            Some(stdout) => child_id(stdout).await,
            None => Err(failed("child stdout was not piped")),
        };
        // The child's display appeared after this process started, so only a
        // fresh process reliably sees it.
        let visible = match &ready {
            Ok(id) => {
                wait_child(Duration::from_secs(3), |ids| ids.contains(id))
                    .await
                    .0
            }
            Err(_) => false,
        };
        // Tokio sends SIGKILL on Unix, targeting only this owned child.
        // Always kill and reap, including missing/invalid ID and timeout paths.
        let killed_at = Instant::now();
        let killed = child.start_kill();
        let exited = tokio::time::timeout(Duration::from_secs(5), child.wait()).await;
        let id = ready?;
        let remaining = Duration::from_secs(5).saturating_sub(killed_at.elapsed());
        let (removed, _, _) = wait_child(remaining, |ids| !ids.contains(&id)).await;
        println!(
            "INFO in-process list {} child display {id}",
            if absent(platform, id, Duration::from_millis(200)).await {
                "dropped"
            } else {
                "still lists"
            }
        );
        let mut passed = check(visible, format!("child display {id} enumerated"));
        passed &= check(killed.is_ok(), "sent SIGKILL to child");
        passed &= check(
            matches!(exited, Ok(Ok(status)) if status.signal() == Some(9)),
            "child exited on SIGKILL",
        );
        passed &= check(
            removed,
            "child display removed within 5 seconds after SIGKILL",
        );
        if passed {
            Ok(())
        } else {
            Err(failed("crash cleanup check failed"))
        }
    }

    #[tokio::main(flavor = "current_thread")]
    pub async fn main() -> ExitCode {
        let result = async {
            let Some(args) = parse_args()? else {
                return Ok(false);
            };
            let platform = MacPlatform::new();
            eprintln!("macOS {}", platform.info().os_version);
            if !matches!(args.run, Run::Query(_) | Run::ListOnline) {
                report_orphans();
            }
            match args.run {
                Run::Diag => diagnose(&platform, args.mode, args.diag_groups.as_deref()).await?,
                Run::Normal => {
                    let backend = vdisplay::backend().ok_or(ProbeError::Unsupported)?;
                    normal(&platform, backend, args.mode).await?
                }
                Run::CrashChild => {
                    if std::env::var("RS_VD_ALLOW_CRASH").as_deref() == Ok("1") {
                        crash_child(&platform, args.mode).await?
                    } else {
                        println!("SKIP crash-child");
                    }
                }
                Run::Child => {
                    let backend = vdisplay::backend().ok_or(ProbeError::Unsupported)?;
                    child(backend, args.mode).await?
                }
                Run::Query(id) => print_query_display(id)?,
                Run::ListOnline => print_online(),
            }
            Ok(true)
        }
        .await;
        let code = match result {
            Ok(false) => return ExitCode::SUCCESS,
            Ok(true) => 0,
            Err(ProbeError::Unsupported) => {
                println!("UNSUPPORTED");
                2
            }
            Err(ProbeError::NoScreenRecording) => 3,
            Err(ProbeError::Failed(message)) => {
                println!("FAIL {message}");
                1
            }
        };
        println!("PROBE {}", if code == 0 { "PASS" } else { "FAIL" });
        ExitCode::from(code)
    }
}
