//! Real DXGI capture -> WinEncoder probe. Run release builds on Windows GPUs.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = probe::run() {
        eprintln!("encode_probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod probe {
    use std::{
        cell::RefCell,
        error::Error,
        fs::{File, OpenOptions},
        io::{BufWriter, Write},
        path::PathBuf,
        sync::{
            atomic::{AtomicBool, Ordering},
            Arc, Mutex, MutexGuard,
        },
        thread::{self, JoinHandle},
        time::{Duration, Instant},
    };

    use host_core::{
        display_model::{display_id, MonitorRecord, RectPx},
        encode_model::{parse_encoder_pref, LatencyWindow},
    };
    use host_core::{
        pipeline::{capture::dimensions, policy::StillRefiner},
        platform::{
            CaptureConfig, CapturedFrame, DisplayInfo, EncodeOpts, EncodedFrame, Encoder,
            EncoderConfig,
        },
    };
    use host_win::{
        capture, displays,
        encode::{self, EncodePrefs, EncoderStatus},
        ffmpeg::FfLib,
    };
    use rsp_core::{clock::unix_micros, pb::Codec};
    use tokio::sync::mpsc;
    use windows::{
        core::w,
        Win32::{
            Foundation::{COLORREF, FILETIME, HWND, LPARAM, LRESULT, RECT, WPARAM},
            Graphics::Gdi::{
                BeginPaint, CreateSolidBrush, DeleteObject, EndPaint, FillRect, InvalidateRect,
                SetBkMode, SetTextColor, TextOutW, HDC, PAINTSTRUCT, TRANSPARENT,
            },
            System::{
                LibraryLoader::GetModuleHandleW,
                ProcessStatus::{
                    GetProcessMemoryInfo, PROCESS_MEMORY_COUNTERS, PROCESS_MEMORY_COUNTERS_EX,
                },
                Threading::{GetCurrentProcess, GetProcessHandleCount, GetProcessTimes},
            },
            UI::WindowsAndMessaging::{
                CreateWindowExW, DefWindowProcW, DestroyWindow, DispatchMessageW, GetClientRect,
                GetMessageW, KillTimer, PostQuitMessage, RegisterClassW, SetTimer,
                UnregisterClassW, MSG, WM_CLOSE, WM_DESTROY, WM_PAINT, WM_TIMER, WNDCLASSW,
                WS_EX_TOOLWINDOW, WS_EX_TOPMOST, WS_POPUP, WS_VISIBLE,
            },
        },
    };

    type Result<T> = std::result::Result<T, Box<dyn Error + Send + Sync>>;

    fn lock<T>(value: &Mutex<T>) -> MutexGuard<'_, T> {
        value.lock().unwrap_or_else(|e| e.into_inner())
    }

    struct Args {
        list: bool,
        display: Option<u32>,
        prefs: EncodePrefs,
        ffmpeg_dir: Option<PathBuf>,
        codec: Codec,
        size: Option<(u32, u32)>,
        fps: u32,
        kbps: u32,
        seconds: f64,
        motion: bool,
        still_after: Option<f64>,
        still_refine: bool,
        keyframe_every: Option<f64>,
        rate_step: Option<(u32, f64)>,
        slow_sink: Option<(u64, f64, f64)>,
        out: Option<PathBuf>,
        cycles: Option<u32>,
    }

    fn seconds(value: &str) -> Result<f64> {
        let value: f64 = value.parse()?;
        if !value.is_finite() || !(0.0..=86_400.0).contains(&value) {
            return Err("seconds must be finite and between 0 and 86400".into());
        }
        Ok(value)
    }

    impl Args {
        fn parse() -> Result<Option<Self>> {
            let mut out = Self {
                list: false,
                display: None,
                prefs: EncodePrefs::default(),
                ffmpeg_dir: None,
                codec: Codec::Hevc,
                size: None,
                fps: 60,
                kbps: 10_000,
                seconds: 20.0,
                motion: false,
                still_after: None,
                still_refine: false,
                keyframe_every: None,
                rate_step: None,
                slow_sink: None,
                out: None,
                cycles: None,
            };
            let mut args = std::env::args().skip(1);
            while let Some(arg) = args.next() {
                match arg.as_str() {
                    "--help" | "-h" => {
                        println!("encode_probe [--list] [--display ID] [--encoder auto|nvenc|amf|qsv|mf] [--ffmpeg-dir PATH] [--codec hevc|h264] [--size WxH|auto] [--fps 60] [--kbps 10000] [--seconds 20] [--motion] [--still-after SEC] [--still-refine] [--keyframe-every SEC] [--rate-step KBPS@SEC] [--slow-sink MS@START-END] [--out FILE] [--cycles N]\nCycles run for 3 seconds each. CPU includes capture, animation and encoding. Receiver capture latency excludes repeated timestamps and still refinement. Logs are written beside the executable under logs/.");
                        return Ok(None);
                    }
                    "--list" => out.list = true,
                    "--motion" => out.motion = true,
                    "--still-refine" => out.still_refine = true,
                    _ => {
                        let value = args
                            .next()
                            .ok_or_else(|| format!("missing value for {arg}"))?;
                        match arg.as_str() {
                            "--display" => out.display = Some(value.parse()?),
                            "--encoder" => {
                                if !["auto", "nvenc", "amf", "qsv", "mf"].contains(&value.as_str())
                                {
                                    return Err("encoder must be auto|nvenc|amf|qsv|mf".into());
                                }
                                out.prefs.encoder =
                                    parse_encoder_pref(&value).ok_or("invalid encoder")?;
                            }
                            "--ffmpeg-dir" => out.ffmpeg_dir = Some(value.into()),
                            "--codec" => {
                                out.codec = match value.as_str() {
                                    "hevc" => Codec::Hevc,
                                    "h264" => Codec::H264,
                                    _ => return Err("codec must be hevc|h264".into()),
                                }
                            }
                            "--size" if value == "auto" => out.size = None,
                            "--size" => {
                                let (w, h) =
                                    value.split_once('x').ok_or("size must be WxH|auto")?;
                                out.size = Some((w.parse()?, h.parse()?));
                            }
                            "--fps" => out.fps = value.parse()?,
                            "--kbps" => out.kbps = value.parse()?,
                            "--seconds" => out.seconds = seconds(&value)?,
                            "--still-after" => out.still_after = Some(seconds(&value)?),
                            "--keyframe-every" => out.keyframe_every = Some(seconds(&value)?),
                            "--rate-step" => {
                                let (bps, at) =
                                    value.split_once('@').ok_or("rate step must be KBPS@SEC")?;
                                out.rate_step = Some((bps.parse()?, seconds(at)?));
                            }
                            "--slow-sink" => {
                                let (ms, interval) = value
                                    .split_once('@')
                                    .ok_or("slow sink must be MS@START-END")?;
                                let (start, end) = interval
                                    .split_once('-')
                                    .ok_or("slow sink requires START-END")?;
                                let (start, end) = (seconds(start)?, seconds(end)?);
                                if end <= start {
                                    return Err("slow sink end must exceed start".into());
                                }
                                out.slow_sink = Some((ms.parse()?, start, end));
                            }
                            "--out" => out.out = Some(value.into()),
                            "--cycles" => out.cycles = Some(value.parse()?),
                            _ => return Err(format!("unknown argument: {arg}").into()),
                        }
                    }
                }
            }
            if !(1..=120).contains(&out.fps)
                || out.kbps == 0
                || out.seconds == 0.0
                || out.cycles == Some(0)
                || out.keyframe_every == Some(0.0)
                || out.rate_step.is_some_and(|(bps, _)| bps == 0)
            {
                return Err("fps must be 1..120; bitrate, duration, cycles and key interval must be positive".into());
            }
            Ok(Some(out))
        }
    }

    pub fn run() -> Result<()> {
        let Some(args) = Args::parse()? else {
            return Ok(());
        };
        // Before logging, runtime threads, warm-up or any FFmpeg load.
        if let Some(dir) = &args.ffmpeg_dir {
            std::env::set_var("RS_FFMPEG_DIR", dir);
        }
        let executable = std::env::current_exe()?;
        let log_dir = executable
            .parent()
            .ok_or("executable has no parent")?
            .join("logs");
        let _logs = host_core::logs::init(&log_dir, "info")?;
        encode::configure(args.prefs.clone());
        displays::ensure_dpi_awareness();
        if args.list {
            let started = Instant::now();
            encode::warm_up();
            let table = encode::wait_probe(Duration::from_secs(10));
            match FfLib::shared() {
                Ok(lib) => println!(
                    "ffmpeg avcodec={} avutil={} dir={} license={}",
                    lib.versions().avcodec,
                    lib.versions().avutil,
                    lib.versions().dir.display(),
                    lib.versions().license
                ),
                Err(error) => println!("ffmpeg unavailable: {error}"),
            }
            if let Some(table) = table {
                for entry in table.entries() {
                    println!("probe {entry:?}");
                }
            } else {
                println!("probe timed out after 10 seconds");
            }
            println!(
                "probe_total_ms={:.3}",
                started.elapsed().as_secs_f64() * 1000.0
            );
            for monitor in displays::enumerate() {
                println!(
                    "display id={} name={} rect={:?} primary={} rotation={}",
                    display_id(&monitor.device_name),
                    monitor.friendly_name,
                    monitor.rect,
                    monitor.primary,
                    monitor.orientation
                );
            }
            return Ok(());
        }
        let monitors = displays::enumerate();
        let monitor = if let Some(id) = args.display {
            monitors.iter().find(|m| display_id(&m.device_name) == id)
        } else {
            monitors
                .iter()
                .find(|m| m.primary)
                .or_else(|| monitors.first())
        }
        .ok_or("display not found")?;
        let d = DisplayInfo {
            id: display_id(&monitor.device_name),
            name: monitor.friendly_name.clone(),
            width_px: monitor.rect.width,
            height_px: monitor.rect.height,
            scale: monitor.dpi as f32 / 96.0,
            x: monitor.rect.left,
            y: monitor.rect.top,
            refresh_hz: monitor.refresh_hz,
            primary: monitor.primary,
            rotation: monitor.orientation,
        };
        let (width, height) = match args.size {
            Some(size) => size,
            None => dimensions(&d, 1920, 0)?,
        };
        let runtime = tokio::runtime::Builder::new_multi_thread()
            .worker_threads(2)
            .enable_all()
            .build()?;
        runtime.block_on(async {
            for cycle in 0..args.cycles.unwrap_or(1) {
                run_cycle(&args, monitor, width, height, cycle).await?;
                let resources = Resources::read()?;
                println!(
                    "cycle={} handles={} priv_mb={:.3}",
                    cycle + 1,
                    resources.handles,
                    resources.private_mb()
                );
            }
            Ok(())
        })
    }

    struct Resources {
        cpu_100ns: u64,
        at: Instant,
        handles: u32,
        private_bytes: usize,
    }

    impl Resources {
        fn read() -> Result<Self> {
            let mut creation = FILETIME::default();
            let mut exit = FILETIME::default();
            let mut kernel = FILETIME::default();
            let mut user = FILETIME::default();
            let mut handles = 0;
            let mut memory = PROCESS_MEMORY_COUNTERS_EX {
                cb: std::mem::size_of::<PROCESS_MEMORY_COUNTERS_EX>() as u32,
                ..Default::default()
            };
            // SAFETY: the current-process pseudo handle is valid and does not
            // need closing; all writable pointers reference initialized locals.
            unsafe {
                let process = GetCurrentProcess();
                GetProcessTimes(process, &mut creation, &mut exit, &mut kernel, &mut user)?;
                GetProcessHandleCount(process, &mut handles)?;
                GetProcessMemoryInfo(
                    process,
                    (&mut memory as *mut PROCESS_MEMORY_COUNTERS_EX)
                        .cast::<PROCESS_MEMORY_COUNTERS>(),
                    memory.cb,
                )?;
            }
            let ticks =
                |t: FILETIME| (u64::from(t.dwHighDateTime) << 32) | u64::from(t.dwLowDateTime);
            Ok(Self {
                cpu_100ns: ticks(kernel) + ticks(user),
                at: Instant::now(),
                handles,
                private_bytes: memory.PrivateUsage,
            })
        }

        fn private_mb(&self) -> f64 {
            self.private_bytes as f64 / (1024.0 * 1024.0)
        }

        fn cpu_since(&self, previous: &Self) -> f64 {
            let cores = thread::available_parallelism()
                .map(|n| n.get())
                .unwrap_or(1) as f64;
            let wall = self.at.duration_since(previous.at).as_secs_f64();
            self.cpu_100ns.saturating_sub(previous.cpu_100ns) as f64
                / 10_000_000.0
                / (wall.max(f64::EPSILON) * cores)
                * 100.0
        }
    }

    #[derive(Default)]
    struct Metrics {
        frames: u64,
        bytes: u64,
        keys: u64,
        refines: u64,
        latest_capture: u64,
        cap: LatencyWindow,
        all_cap: Vec<u64>,
        key_request: Option<(Instant, u64)>,
        rate_at: Option<u64>,
        rate_bytes: [u64; 10],
    }

    async fn receive(
        mut receiver: mpsc::Receiver<EncodedFrame>,
        metrics: Arc<Mutex<Metrics>>,
        origin: Instant,
        slow: Option<(u64, f64, f64)>,
        mut file: Option<BufWriter<File>>,
    ) -> Result<()> {
        while let Some(frame) = receiver.recv().await {
            {
                let mut metrics = lock(&metrics);
                metrics.frames += 1;
                metrics.bytes += frame.data.len() as u64;
                metrics.keys += u64::from(frame.keyframe);
                metrics.refines += u64::from(frame.still_refine);
                // Idle IDRs also reuse capture_ts_us. Exclude duplicate capture
                // timestamps just as EncoderStatus excludes retry PTS values.
                if !frame.still_refine && frame.capture_ts_us > metrics.latest_capture {
                    let latency = frame.encoded_ts_us.saturating_sub(frame.capture_ts_us);
                    metrics.cap.push(latency);
                    metrics.all_cap.push(latency);
                    metrics.latest_capture = frame.capture_ts_us;
                }
                if frame.keyframe {
                    if let Some((request, timestamp)) = metrics.key_request {
                        if frame.encoded_ts_us >= timestamp {
                            println!(
                                "key_latency_ms={:.3} request_to_encoded_ms={:.3}",
                                request.elapsed().as_secs_f64() * 1000.0,
                                frame.encoded_ts_us.saturating_sub(timestamp) as f64 / 1000.0
                            );
                            metrics.key_request = None;
                        }
                    }
                }
                if let Some(step) = metrics.rate_at {
                    if frame.encoded_ts_us >= step {
                        let bin = frame.encoded_ts_us.saturating_sub(step) / 100_000;
                        if let Some(bytes) = metrics.rate_bytes.get_mut(bin as usize) {
                            *bytes += frame.data.len() as u64;
                        }
                    }
                }
            }
            if let Some(file) = file.as_mut() {
                file.write_all(&frame.data)?;
            }
            if let Some((ms, start, end)) = slow {
                if (start..end).contains(&origin.elapsed().as_secs_f64()) {
                    tokio::time::sleep(Duration::from_millis(ms)).await;
                }
            }
        }
        if let Some(file) = file.as_mut() {
            file.flush()?;
        }
        Ok(())
    }

    fn milliseconds(value: Option<u64>) -> String {
        value
            .map(|value| format!("{:.3}", value as f64 / 1000.0))
            .unwrap_or_else(|| "-".into())
    }

    fn drops(status: &EncoderStatus) -> String {
        format!(
            "{}/{}/{}/{}/{}",
            status.stats.mailbox_dropped,
            status.busy_dropped,
            status.failed_dropped,
            status.stats.sink_dropped,
            status.stats.gated
        )
    }

    fn percentile(values: &[u64], percent: usize) -> Option<u64> {
        if values.is_empty() {
            return None;
        }
        let mut values = values.to_vec();
        values.sort_unstable();
        Some(
            values[(values.len() * percent)
                .div_ceil(100)
                .saturating_sub(1)
                .min(values.len() - 1)],
        )
    }

    async fn run_cycle(
        args: &Args,
        monitor: &MonitorRecord,
        width: u32,
        height: u32,
        cycle: u32,
    ) -> Result<()> {
        let id = display_id(&monitor.device_name);
        let (sink, packets) = mpsc::channel(4);
        let mut encoder = encode::create_encoder(EncoderConfig {
            codec: args.codec,
            width,
            height,
            fps: args.fps,
            bitrate_kbps: args.kbps,
            sink,
        })?;
        let origin = Instant::now();
        let motion = if args.motion {
            Some(Motion::start(monitor.rect, args.still_after)?)
        } else {
            None
        };
        let (frame_tx, mut frames) = mpsc::channel::<CapturedFrame>(4);
        let capture = capture::start(
            id,
            CaptureConfig {
                fps: args.fps,
                max_width: width,
                max_height: height,
            },
            frame_tx,
        )?;
        let metrics = Arc::new(Mutex::new(Metrics::default()));
        let file = args
            .out
            .as_ref()
            .map(|path| {
                OpenOptions::new()
                    .write(true)
                    .create(true)
                    .append(cycle != 0)
                    .truncate(cycle == 0)
                    .open(path)
                    .map(BufWriter::new)
            })
            .transpose()?;
        let receiver = tokio::spawn(receive(
            packets,
            Arc::clone(&metrics),
            origin,
            args.slow_sink,
            file,
        ));
        let first_resources = Resources::read()?;
        let mut previous_resources = Resources::read()?;
        let mut max_private = first_resources.private_bytes;
        let mut last = None;
        let mut refiner = StillRefiner::new(args.still_refine);
        let mut tick = tokio::time::interval(Duration::from_millis(10));
        tick.set_missed_tick_behavior(tokio::time::MissedTickBehavior::Skip);
        let mut captured = 0u64;
        let mut previous_input = 0u64;
        let mut previous_output = 0u64;
        let mut previous_bytes = 0u64;
        let mut previous_keys = 0u64;
        let mut previous_refines = 0u64;
        let mut last_report = origin;
        let mut next_report = 1.0;
        let mut next_key = args.keyframe_every;
        let mut step_done = false;
        let mut step_start = None;
        let mut printed_bins = 0;
        let duration = if args.cycles.is_some() {
            3.0
        } else {
            args.seconds
        };
        println!(
            "cycle={} display={} codec={:?} size={}x{} fps={} kbps={}",
            cycle + 1,
            id,
            args.codec,
            width,
            height,
            args.fps,
            args.kbps
        );
        loop {
            tokio::select! {
                frame = frames.recv() => {
                    let Some(frame) = frame else { break };
                    captured += 1;
                    refiner.on_frame(origin.elapsed().as_micros() as u64);
                    last = Some(frame.clone());
                    encoder.encode(frame, EncodeOpts::default())?;
                }
                _ = tick.tick() => {
                    let elapsed = origin.elapsed().as_secs_f64();
                    if elapsed >= duration { break; }
                    if next_key.is_some_and(|at| elapsed >= at) {
                        let mut m = lock(&metrics);
                        if m.key_request.is_none() { m.key_request = Some((Instant::now(), unix_micros())); }
                        encoder.force_keyframe();
                        next_key = args.keyframe_every.map(|gap| elapsed + gap);
                    }
                    if let Some((kbps, at)) = args.rate_step {
                        if !step_done && elapsed >= at {
                            lock(&metrics).rate_at = Some(unix_micros());
                            step_start = Some(Instant::now());
                            encoder.set_rate(kbps, args.fps);
                            step_done = true;
                            println!("rate_step t={elapsed:.3} kbps={kbps}");
                        }
                    }
                    if refiner.poll(origin.elapsed().as_micros() as u64) {
                        if let Some(frame) = last.clone() {
                            encoder.encode(frame, EncodeOpts { quality_boost: true, ..Default::default() })?;
                        }
                    }
                    if let Some(start) = step_start {
                        let complete = ((start.elapsed().as_millis() / 100) as usize).min(10);
                        let m = lock(&metrics);
                        while printed_bins < complete {
                            println!("rate_bin_ms={}-{} out_kbps={:.3}", printed_bins * 100,
                                (printed_bins + 1) * 100, m.rate_bytes[printed_bins] as f64 * 8.0 / 100.0);
                            printed_bins += 1;
                        }
                    }
                    if elapsed >= next_report {
                        let resources = Resources::read()?;
                        max_private = max_private.max(resources.private_bytes);
                        let status = encoder.status();
                        let m = lock(&metrics);
                        let interval = last_report.elapsed().as_secs_f64();
                        println!("t={elapsed:.2} in_fps={:.1} out_fps={:.1} out_kbps={:.1} key={} refine={} cap_enc_p50_ms={} cap_enc_p95_ms={} status_cap_p50_ms={} status_cap_p95_ms={} sub_enc_p95_ms={} drop(mailbox/busy/failed/sink/gated)={} reopens={} failures={} backend={}({}) cpu%={:.2} handles={} priv_mb={:.2}",
                            (captured - previous_input) as f64 / interval,
                            (m.frames - previous_output) as f64 / interval,
                            (m.bytes - previous_bytes) as f64 * 8.0 / interval / 1000.0,
                            m.keys - previous_keys, m.refines - previous_refines,
                            milliseconds(m.cap.percentile(50)), milliseconds(m.cap.percentile(95)),
                            milliseconds(status.capture_to_encoded_p50_us), milliseconds(status.capture_to_encoded_p95_us),
                            milliseconds(status.submit_to_encoded_p95_us), drops(&status), status.stats.reopens,
                            status.stats.failures, status.backend.map(|b| b.name()).unwrap_or("pending"),
                            status.encoder_name, resources.cpu_since(&previous_resources), resources.handles, resources.private_mb());
                        previous_input = captured;
                        previous_output = m.frames;
                        previous_bytes = m.bytes;
                        previous_keys = m.keys;
                        previous_refines = m.refines;
                        previous_resources = resources;
                        last_report = Instant::now();
                        next_report = elapsed + 1.0;
                    }
                }
            }
        }
        drop(capture);
        drop(frames);
        drop(last);
        // Allow already submitted packets to drain before sampling final status.
        tokio::time::sleep(Duration::from_millis(100)).await;
        let status = encoder.status();
        drop(encoder);
        receiver.await??;
        drop(motion);
        let resources = Resources::read()?;
        max_private = max_private.max(resources.private_bytes);
        let m = lock(&metrics);
        println!("summary cycle={} captured={} encoded={} key={} refine={} drop(mailbox/busy/failed/sink/gated)={} cap_enc_p50_ms={} cap_enc_p95_ms={} cap_enc_p99_ms={} avg_cpu%={:.2} max_priv_mb={:.2} status={status:?}",
            cycle + 1, captured, m.frames, m.keys, m.refines, drops(&status),
            milliseconds(percentile(&m.all_cap, 50)), milliseconds(percentile(&m.all_cap, 95)),
            milliseconds(percentile(&m.all_cap, 99)), resources.cpu_since(&first_resources),
            max_private as f64 / (1024.0 * 1024.0));
        Ok(())
    }

    struct Animation {
        origin: Instant,
        still_after: Option<f64>,
        stop: Arc<AtomicBool>,
        frame: u64,
    }

    thread_local! {
        static ANIMATION: RefCell<Option<Animation>> = const { RefCell::new(None) };
    }

    struct Motion {
        stop: Arc<AtomicBool>,
        thread: Option<JoinHandle<()>>,
    }

    impl Motion {
        fn start(rect: RectPx, still_after: Option<f64>) -> Result<Self> {
            let stop = Arc::new(AtomicBool::new(false));
            let request = Arc::clone(&stop);
            let (ready_tx, ready_rx) = std::sync::mpsc::channel();
            let thread = thread::Builder::new()
                .name("rs-encode-motion".into())
                .spawn(move || {
                    let result = motion_window(rect, still_after, request, &ready_tx);
                    if let Err(error) = result {
                        let _ = ready_tx.send(Err(error.to_string()));
                    }
                })?;
            let motion = Self {
                stop,
                thread: Some(thread),
            };
            match ready_rx.recv_timeout(Duration::from_secs(3))? {
                Ok(()) => Ok(motion),
                Err(error) => Err(error.into()),
            }
        }
    }

    impl Drop for Motion {
        fn drop(&mut self) {
            self.stop.store(true, Ordering::Relaxed);
            if let Some(thread) = self.thread.take() {
                let _ = thread.join();
            }
        }
    }

    fn motion_window(
        rect: RectPx,
        still_after: Option<f64>,
        stop: Arc<AtomicBool>,
        ready: &std::sync::mpsc::Sender<std::result::Result<(), String>>,
    ) -> Result<()> {
        // SAFETY: class/window/message data lives on this UI thread. Strings are
        // static UTF-16, and this thread destroys/unregisters everything it owns.
        unsafe {
            let instance = GetModuleHandleW(None)?.into();
            let class = w!("RsEncodeProbeMotion");
            let wc = WNDCLASSW {
                lpfnWndProc: Some(window_proc),
                hInstance: instance,
                lpszClassName: class,
                ..Default::default()
            };
            if RegisterClassW(&wc) == 0 {
                return Err(windows::core::Error::from_thread().into());
            }
            ANIMATION.with(|state| {
                *state.borrow_mut() = Some(Animation {
                    origin: Instant::now(),
                    still_after,
                    stop,
                    frame: 0,
                })
            });
            let window = CreateWindowExW(
                WS_EX_TOPMOST | WS_EX_TOOLWINDOW,
                class,
                w!("Encoder motion probe"),
                WS_POPUP | WS_VISIBLE,
                rect.left,
                rect.top,
                rect.width as i32,
                rect.height as i32,
                None,
                None,
                Some(instance),
                None,
            );
            let result = match window {
                Ok(hwnd) => {
                    if SetTimer(Some(hwnd), 1, 16, None) == 0 {
                        let error = windows::core::Error::from_thread();
                        let _ = DestroyWindow(hwnd);
                        Err(error.into())
                    } else {
                        let _ = ready.send(Ok(()));
                        let mut message = MSG::default();
                        let result = loop {
                            let result = GetMessageW(&mut message, None, 0, 0).0;
                            if result == -1 {
                                break Err(windows::core::Error::from_thread().into());
                            }
                            if result == 0 {
                                break Ok(());
                            }
                            DispatchMessageW(&message);
                        };
                        let _ = KillTimer(Some(hwnd), 1);
                        let _ = DestroyWindow(hwnd);
                        result
                    }
                }
                Err(error) => Err(error.into()),
            };
            ANIMATION.with(|state| *state.borrow_mut() = None);
            let _ = UnregisterClassW(class, Some(instance));
            result
        }
    }

    unsafe extern "system" fn window_proc(
        hwnd: HWND,
        message: u32,
        w: WPARAM,
        l: LPARAM,
    ) -> LRESULT {
        // SAFETY: invoked by Windows on the window-owning thread; ANIMATION is
        // thread local, and paint resources are paired within WM_PAINT.
        unsafe {
            match message {
                WM_TIMER => {
                    let (stop, redraw) = ANIMATION.with(|state| {
                        let mut state = state.borrow_mut();
                        let Some(state) = state.as_mut() else {
                            return (false, false);
                        };
                        let redraw = state
                            .still_after
                            .is_none_or(|at| state.origin.elapsed().as_secs_f64() < at);
                        if redraw {
                            state.frame += 1;
                        }
                        (state.stop.load(Ordering::Relaxed), redraw)
                    });
                    if stop {
                        let _ = DestroyWindow(hwnd);
                    } else if redraw {
                        let _ = InvalidateRect(Some(hwnd), None, false);
                    }
                    LRESULT(0)
                }
                WM_PAINT => {
                    let mut paint = PAINTSTRUCT::default();
                    let dc = BeginPaint(hwnd, &mut paint);
                    let mut rect = RECT::default();
                    if GetClientRect(hwnd, &mut rect).is_ok() {
                        let frame = ANIMATION
                            .with(|state| state.borrow().as_ref().map(|s| s.frame).unwrap_or(0));
                        paint_motion(dc, rect, frame);
                    }
                    let _ = EndPaint(hwnd, &paint);
                    LRESULT(0)
                }
                WM_CLOSE => {
                    let _ = DestroyWindow(hwnd);
                    LRESULT(0)
                }
                WM_DESTROY => {
                    PostQuitMessage(0);
                    LRESULT(0)
                }
                _ => DefWindowProcW(hwnd, message, w, l),
            }
        }
    }

    fn fill(dc: HDC, rect: RECT, color: u32) {
        // SAFETY: dc is from BeginPaint on this thread; the temporary solid
        // brush is deleted after FillRect and is never selected into the DC.
        unsafe {
            let brush = CreateSolidBrush(COLORREF(color));
            FillRect(dc, &rect, brush);
            let _ = DeleteObject(brush.into());
        }
    }

    fn paint_motion(dc: HDC, rect: RECT, frame: u64) {
        fill(dc, rect, 0x181818);
        let (width, height) = (rect.right - rect.left, rect.bottom - rect.top);
        let (left, top, right, bottom) = (width / 5, height / 5, width * 4 / 5, height * 4 / 5);
        let mut random = frame.wrapping_add(1).wrapping_mul(0x9e3779b97f4a7c15);
        for y in (top..bottom).step_by(64) {
            for x in (left..right).step_by(64) {
                random ^= random << 13;
                random ^= random >> 7;
                random ^= random << 17;
                fill(
                    dc,
                    RECT {
                        left: x,
                        top: y,
                        right: (x + 64).min(right),
                        bottom: (y + 64).min(bottom),
                    },
                    random as u32 & 0xffffff,
                );
            }
        }
        let x = (frame.wrapping_mul(13) % width.max(1) as u64) as i32;
        fill(
            dc,
            RECT {
                left: x,
                top: 0,
                right: (x + 12).min(width),
                bottom: height,
            },
            0xffffff,
        );
        let text: Vec<u16> = format!("frame {frame}").encode_utf16().collect();
        // SAFETY: dc is a live paint DC and text remains allocated during TextOutW.
        unsafe {
            SetBkMode(dc, TRANSPARENT);
            SetTextColor(dc, COLORREF(0xffffff));
            let _ = TextOutW(dc, 16, 16, &text);
        }
    }
}
