//! Manual Windows capture probe; no Tokio runtime or native calls in the example.

#[cfg(not(windows))]
fn main() {
    eprintln!("Windows only");
}

#[cfg(windows)]
fn main() {
    if let Err(error) = probe::run() {
        eprintln!("capture_probe: {error}");
        std::process::exit(1);
    }
}

#[cfg(windows)]
mod probe {
    use host_core::platform::{CaptureConfig, CapturedFrame, CursorUpdate};
    use host_core::{
        capture_model::{bgra_to_rgba, CaptureStats},
        display_model::display_id,
    };
    use host_win::{
        capture::{self, WinCapture, WinFrame},
        displays,
    };
    use std::{
        error::Error,
        fs::{self, File},
        io::BufWriter,
        path::{Path, PathBuf},
        sync::{
            atomic::{AtomicU64, Ordering},
            Arc, Mutex,
        },
        thread,
        time::{Duration, Instant},
    };
    use tokio::sync::mpsc;

    type Result<T> = std::result::Result<T, Box<dyn Error>>;
    struct Args {
        list: bool,
        display: Option<u32>,
        fps: u32,
        seconds: u64,
        dump_dir: Option<PathBuf>,
        dump_every: u64,
        subs: u32,
        cursor: bool,
    }
    impl Args {
        fn parse() -> Result<Option<Self>> {
            let mut out = Self {
                list: false,
                display: None,
                fps: 30,
                seconds: 10,
                dump_dir: None,
                dump_every: 2,
                subs: 1,
                cursor: false,
            };
            let mut args = std::env::args().skip(1);
            while let Some(arg) = args.next() {
                if arg == "--help" || arg == "-h" {
                    println!("capture_probe [--list] [--display <id>|all] [--fps <n>] [--seconds <n>] [--subs <n>] [--cursor] [--dump-dir <dir> --dump-every <seconds>]");
                    return Ok(None);
                }
                match arg.as_str() {
                    "--list" => out.list = true,
                    "--cursor" => out.cursor = true,
                    "--display" => {
                        let v = args.next().ok_or("missing --display value")?;
                        out.display = if v == "all" { None } else { Some(v.parse()?) };
                    }
                    "--fps" => out.fps = args.next().ok_or("missing --fps value")?.parse()?,
                    "--seconds" => {
                        out.seconds = args.next().ok_or("missing --seconds value")?.parse()?
                    }
                    "--subs" => out.subs = args.next().ok_or("missing --subs value")?.parse()?,
                    "--dump-every" => {
                        out.dump_every = args.next().ok_or("missing --dump-every value")?.parse()?
                    }
                    "--dump-dir" => {
                        out.dump_dir = Some(args.next().ok_or("missing --dump-dir value")?.into())
                    }
                    _ => return Err(format!("unknown argument: {arg}").into()),
                }
            }
            if out.fps == 0 || out.fps > 120 {
                return Err("capture fps must be 1..120".into());
            }
            if out.subs == 0 || out.seconds == 0 || out.dump_every == 0 {
                return Err("subs, seconds and dump-every must be positive".into());
            }
            Ok(Some(out))
        }
    }

    struct Receiver {
        handle: Option<WinCapture>,
        thread: Option<thread::JoinHandle<()>>,
        latest: Arc<Mutex<Option<CapturedFrame>>>,
        received: Arc<AtomicU64>,
        fps: u32,
    }
    impl Drop for Receiver {
        fn drop(&mut self) {
            // Dropping all handles before joining is done by run(). This fallback
            // also covers a partially initialized group on a start error.
            drop(self.handle.take());
            // Other subscribers can keep the worker running; this receiver's
            // own sink was removed by its handle, so blocking_recv can finish.
            if let Some(t) = self.thread.take() {
                let _ = t.join();
            }
        }
    }
    fn subscribe(id: u32, fps: u32) -> Result<Receiver> {
        let (tx, mut rx) = mpsc::channel::<CapturedFrame>(4);
        let handle = capture::start(
            id,
            CaptureConfig {
                fps,
                max_width: 0,
                max_height: 0,
            },
            tx,
        )?;
        let latest = Arc::new(Mutex::new(None));
        let last = Arc::clone(&latest);
        let received = Arc::new(AtomicU64::new(0));
        let count = Arc::clone(&received);
        let thread = thread::spawn(move || {
            while let Some(frame) = rx.blocking_recv() {
                *last.lock().unwrap_or_else(|e| e.into_inner()) = Some(frame);
                count.fetch_add(1, Ordering::Relaxed);
            }
        });
        Ok(Receiver {
            handle: Some(handle),
            thread: Some(thread),
            latest,
            received,
            fps,
        })
    }
    struct Screen {
        id: u32,
        receivers: Vec<Receiver>,
        previous: CaptureStats,
        dump_number: u64,
    }
    fn delta(a: CaptureStats, b: CaptureStats) -> CaptureStats {
        CaptureStats {
            acquired: a.acquired.saturating_sub(b.acquired),
            published: a.published.saturating_sub(b.published),
            sent: a.sent.saturating_sub(b.sent),
            skipped: a.skipped.saturating_sub(b.skipped),
            sink_full: a.sink_full.saturating_sub(b.sink_full),
            pool_exhausted: a.pool_exhausted.saturating_sub(b.pool_exhausted),
            dup_creates: a.dup_creates.saturating_sub(b.dup_creates),
            device_creates: a.device_creates.saturating_sub(b.device_creates),
            desktop_switches: a.desktop_switches.saturating_sub(b.desktop_switches),
            subscribers: a.subscribers,
        }
    }
    fn dump(screen: &mut Screen, dir: &Path) -> Result<()> {
        let latest = screen.receivers[0]
            .latest
            .lock()
            .unwrap_or_else(|e| e.into_inner())
            .clone();
        let Some(frame) = latest else {
            return Ok(());
        };
        let native = frame
            .native
            .downcast_ref::<WinFrame>()
            .ok_or("unexpected native frame type")?;
        let bgra = capture::read_back_bgra(native, frame.width, frame.height)?;
        let rgba = bgra_to_rgba(&bgra, frame.width, frame.height, frame.width as usize * 4)
            .ok_or("invalid BGRA readback")?;
        screen.dump_number += 1;
        let path = dir.join(format!("d{}-{}.png", screen.id, screen.dump_number));
        let mut png = png::Encoder::new(
            BufWriter::new(File::create(&path)?),
            frame.width,
            frame.height,
        );
        png.set_color(png::ColorType::Rgba);
        png.set_depth(png::BitDepth::Eight);
        let mut writer = png.write_header()?;
        writer.write_image_data(&rgba)?;
        writer.finish()?;
        println!(
            "dump {} {}x{} device_gen={} vendor={:#x} rotation={}",
            path.display(),
            frame.width,
            frame.height,
            native.device_gen,
            native.vendor_id,
            native.rotation
        );
        Ok(())
    }
    fn cursors(rx: &mut mpsc::Receiver<CursorUpdate>) {
        while let Ok(update) = rx.try_recv() {
            match update {
                CursorUpdate::Shape(s) => println!(
                    "cursor Shape id={} {}x{} hot=({}, {}) scale={}",
                    s.id, s.width, s.height, s.hot_x, s.hot_y, s.scale
                ),
                other => println!("cursor {other:?}"),
            }
        }
    }
    pub fn run() -> Result<()> {
        let Some(args) = Args::parse()? else {
            return Ok(());
        };
        tracing::subscriber::set_global_default(ProbeLog)?;
        displays::ensure_dpi_awareness();
        let monitors = displays::enumerate();
        if args.list {
            for output in capture::list_outputs()? {
                println!("DXGI {output:?}");
            }
            for m in monitors {
                println!(
                    "display device_name={} display_id={} rect={:?} dpi={} primary={}",
                    m.device_name,
                    display_id(&m.device_name),
                    m.rect,
                    m.dpi,
                    m.primary
                );
            }
            return Ok(());
        }
        if let Some(dir) = &args.dump_dir {
            fs::create_dir_all(dir)?;
        }
        let ids = if let Some(id) = args.display {
            vec![id]
        } else {
            monitors
                .iter()
                .map(|m| display_id(&m.device_name))
                .collect()
        };
        if ids.is_empty() {
            return Err("no attached displays".into());
        }
        let (cursor_tx, mut cursor_rx) = mpsc::channel(1024);
        if args.cursor {
            capture::watch_cursor(cursor_tx);
        }
        let mut screens = Vec::new();
        for id in ids {
            let mut receivers = Vec::new();
            for sub in 0..args.subs {
                let fps = args.fps.checked_shr(sub).unwrap_or(0).max(1);
                receivers.push(subscribe(id, fps)?);
            }
            screens.push(Screen {
                id,
                receivers,
                previous: CaptureStats::default(),
                dump_number: 0,
            });
        }
        let started = Instant::now();
        let mut printed = 0;
        let mut dumped = 0;
        while started.elapsed() < Duration::from_secs(args.seconds) {
            // A short control tick drains cursor messages; frames still use
            // blocking_recv on dedicated standard threads without a runtime.
            thread::sleep(Duration::from_millis(20));
            if args.cursor {
                cursors(&mut cursor_rx);
            }
            let seconds = started.elapsed().as_secs();
            if seconds > printed {
                printed = seconds;
                for screen in &mut screens {
                    if let Some(handle) = &screen.receivers[0].handle {
                        let stats = handle.stats();
                        println!(
                            "t={seconds}s display={} delta={:?}",
                            screen.id,
                            delta(stats, screen.previous)
                        );
                        screen.previous = stats;
                    }
                }
            }
            if seconds.saturating_sub(dumped) >= args.dump_every {
                dumped = seconds;
                if let Some(dir) = &args.dump_dir {
                    for screen in &mut screens {
                        if let Err(e) = dump(screen, dir) {
                            eprintln!("display={} dump failed: {e}", screen.id);
                        }
                    }
                }
            }
        }
        for screen in &mut screens {
            // Snapshot totals before dropping the last handle (which joins).
            let total = screen.receivers[0].handle.as_ref().map(WinCapture::stats);
            for receiver in &mut screen.receivers {
                drop(receiver.handle.take());
            }
            for (sub, receiver) in screen.receivers.iter_mut().enumerate() {
                if let Some(t) = receiver.thread.take() {
                    t.join().map_err(|_| "frame receiver panicked")?;
                }
                println!(
                    "display={} subscriber={} fps={} received={}",
                    screen.id,
                    sub,
                    receiver.fps,
                    receiver.received.load(Ordering::Relaxed)
                );
            }
            println!("display={} total={total:?}", screen.id);
        }
        if args.cursor {
            cursors(&mut cursor_rx);
        }
        Ok(())
    }

    // A tiny event-only logger keeps the probe dependency-free. TRACE is
    // disabled so frame-by-frame logging does not perturb capture measurements.
    struct ProbeLog;
    impl tracing::Subscriber for ProbeLog {
        fn enabled(&self, m: &tracing::Metadata<'_>) -> bool {
            *m.level() <= tracing::Level::DEBUG
        }
        fn new_span(&self, _: &tracing::span::Attributes<'_>) -> tracing::span::Id {
            static NEXT: AtomicU64 = AtomicU64::new(1);
            tracing::span::Id::from_u64(NEXT.fetch_add(1, Ordering::Relaxed))
        }
        fn record(&self, _: &tracing::span::Id, _: &tracing::span::Record<'_>) {}
        fn record_follows_from(&self, _: &tracing::span::Id, _: &tracing::span::Id) {}
        fn enter(&self, _: &tracing::span::Id) {}
        fn exit(&self, _: &tracing::span::Id) {}
        fn event(&self, event: &tracing::Event<'_>) {
            struct Fields(String);
            impl tracing::field::Visit for Fields {
                fn record_debug(
                    &mut self,
                    field: &tracing::field::Field,
                    value: &dyn std::fmt::Debug,
                ) {
                    use std::fmt::Write;
                    let _ = write!(&mut self.0, " {}={value:?}", field.name());
                }
            }
            let mut fields = Fields(String::new());
            event.record(&mut fields);
            eprintln!(
                "{} {}{}",
                event.metadata().level(),
                event.metadata().target(),
                fields.0
            );
        }
    }
}
